// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Text;
using System.Text.Json;

namespace CodexInfo.WindowsClient.Core;

public sealed partial class LoopbackStatusClient : ILoopbackRecordedScopeClient
{
    private sealed record RecordedDirectoryPage(long? ObservedAt, IReadOnlyList<ApiRecordedScope> Scopes,
        string? NextCursor, PublishedPairIdentity PublishedPair);
    private sealed record RecordingsPage(ApiRecordedUsageScope Scope, IReadOnlyList<ApiRecordedCoverageInterval> Coverage,
        IReadOnlyList<ApiRecordedSample> Samples, IReadOnlyList<ApiRecordedQuotaObservation> Quota,
        string? NextCursor, PublishedPairIdentity PublishedPair);
    private sealed record ObservationsPage(ApiRecordedUsageScope Scope, IReadOnlyList<ApiSavedObservation> Observations,
        string? NextCursor, PublishedPairIdentity PublishedPair);

    public async Task<RecordedGraphFetchResult> FetchRecordedGraphAsync(string? accountId, string? scopeId = null,
        long? startAt = null, long? endAt = null, CancellationToken cancellationToken = default)
    {
        if (accountId is not null && !IsSafeAccountId(accountId) ||
            startAt.HasValue != endAt.HasValue || startAt is { } s && !RecordedBounds(s, endAt!.Value))
            return new(null, DetailsFetchFailure.Response);
        var generation = BeginAccountScope(accountId);
        for (var attempt = 0; attempt < 2; attempt++)
        {
            var (result, stale) = await FetchRecordedGraphAttemptAsync(accountId, scopeId, startAt, endAt,
                generation, cancellationToken).ConfigureAwait(false);
            if (!stale || attempt != 0) return result;
        }
        return new(null, DetailsFetchFailure.Response);
    }

    private async Task<(RecordedGraphFetchResult Result, bool Stale)> FetchRecordedGraphAttemptAsync(
        string? account, string? scopeId, long? requestedStart, long? requestedEnd, long generation,
        CancellationToken token)
    {
        async Task<SplitFetchResult<T>> Read<T>(string endpoint, Func<byte[], PublishedPairIdentity, T?> parse,
            Func<T, PublishedPairIdentity> pair) where T : class =>
            await FetchSplitResourceAsync(endpoint, endpoint, (T?)null, parse, pair, _ => { }, () => { }, token,
                recognizeStaleCursor: endpoint.Contains("cursor=", StringComparison.Ordinal),
                scopeIsCurrent: () => IsAccountScopeCurrent(account, generation))
                .ConfigureAwait(false);
        static (RecordedGraphFetchResult, bool) Failure(bool stale = false, bool legacy = false) =>
            (new(null, DetailsFetchFailure.Response, legacy), stale);
        static string Cursor(string route, string? cursor) => cursor is null ? route :
            route + (route.Contains('?') ? "&" : "?") + "cursor=" + Uri.EscapeDataString(cursor);

        var scopes = new List<ApiRecordedScope>();
        var ids = new HashSet<string>(StringComparer.Ordinal);
        var cursors = new HashSet<string>(StringComparer.Ordinal);
        PublishedPairIdentity? pair = null;
        long? observed = null;
        string? next = null;
        do
        {
            var response = await Read(Cursor(AppendAccountQuery($"{EndpointRoot}/v6/history/scopes", account), next),
                ParseRecordedDirectory, page => page.PublishedPair).ConfigureAwait(false);
            if (response.Value is not { } page) return Failure(response.CursorRejected,
                legacy: scopes.Count == 0 && next is null && response.NotFound);
            if (pair is { } priorPair && (priorPair != page.PublishedPair || observed != page.ObservedAt)) return Failure();
            pair = page.PublishedPair;
            observed = page.ObservedAt;
            foreach (var scope in page.Scopes)
            {
                if (!ids.Add(scope.Id) || scope.Current && scopes.Count != 0 ||
                    scopes.LastOrDefault() is { Current: false } prior &&
                    (scope.Current || scope.StartAt >= prior.StartAt)) return Failure();
                scopes.Add(scope);
            }
            next = page.NextCursor;
            if (next is not null && !cursors.Add(next)) return Failure();
        } while (next is not null);

        if (scopes.Count == 0)
            return (new(new(account, pair!.Value, scopes.AsReadOnly(), null, null, [], [], [], []), null), false);
        var selected = scopeId is null ? scopes.FirstOrDefault() : scopes.FirstOrDefault(scope => scope.Id == scopeId);
        if (requestedStart is null && selected is null) return Failure();
        var start = requestedStart ?? selected!.StartAt;
        var end = requestedEnd ?? selected!.EndAt;
        var query = $"start={start.ToString(System.Globalization.CultureInfo.InvariantCulture)}&end={end.ToString(System.Globalization.CultureInfo.InvariantCulture)}";
        var accountQuery = account is null ? "" : "&account=" + Uri.EscapeDataString(account);
        var recordingsRoute = $"{EndpointRoot}/v4/history/recordings?{query}{accountQuery}";
        var observationsRoute = $"{EndpointRoot}/v4/history/observations?{query}{accountQuery}";
        var samples = new List<ApiRecordedSample>();
        var quota = new List<ApiRecordedQuotaObservation>();
        var saved = new List<ApiSavedObservation>();
        ApiRecordedUsageScope? metadata = null;
        IReadOnlyList<ApiRecordedCoverageInterval>? coverage = null;
        next = null;
        cursors.Clear();
        do
        {
            var response = await Read(Cursor(recordingsRoute, next), ParseRecordings, page => page.PublishedPair).ConfigureAwait(false);
            if (response.Value is not { } page) return Failure(response.CursorRejected);
            if (page.PublishedPair != pair || page.Scope.StartAt != start || page.Scope.EndAt != end ||
                metadata is not null && (metadata != page.Scope || !coverage!.SequenceEqual(page.Coverage))) return Failure();
            metadata = page.Scope;
            coverage = page.Coverage;
            foreach (var sample in page.Samples)
            {
                if (quota.Count != 0 || samples.LastOrDefault() is { } prior &&
                    (sample.Timestamp <= prior.Timestamp || !RecordingPrefixFollows(prior, sample))) return Failure();
                samples.Add(sample);
            }
            foreach (var observation in page.Quota)
            {
                if (quota.LastOrDefault() is { } prior && observation.Sequence <= prior.Sequence) return Failure();
                quota.Add(observation);
            }
            next = page.NextCursor;
            if (next is not null && !cursors.Add(next)) return Failure();
        } while (next is not null);
        next = null;
        cursors.Clear();
        var observationIds = new HashSet<string>(StringComparer.Ordinal);
        do
        {
            var response = await Read(Cursor(observationsRoute, next), ParseSavedObservations, page => page.PublishedPair).ConfigureAwait(false);
            if (response.Value is not { } page) return Failure(response.CursorRejected);
            if (page.PublishedPair != pair || page.Scope != metadata) return Failure();
            foreach (var observation in page.Observations)
            {
                if (!observationIds.Add(observation.ObservationId) ||
                    saved.LastOrDefault() is { } prior && CompareSaved(prior, observation) >= 0) return Failure();
                saved.Add(observation);
            }
            next = page.NextCursor;
            if (next is not null && !cursors.Add(next)) return Failure();
        } while (next is not null);
        if (!IsAccountScopeCurrent(account, generation)) return Failure();
        return (new(new(account, pair!.Value, scopes.AsReadOnly(), selected, metadata!, coverage!,
            samples.AsReadOnly(), quota.AsReadOnly(), saved.AsReadOnly()), null), false);
    }

    private static bool RecordingPrefixFollows(ApiRecordedSample before, ApiRecordedSample after)
    {
        foreach (var previous in before.Models)
        {
            var current = after.Models.FirstOrDefault(model => model.Name == previous.Name);
            if (current is null || current.TotalTokens < previous.TotalTokens ||
                current.InputTokens < previous.InputTokens || current.CachedInputTokens < previous.CachedInputTokens ||
                current.CacheWriteInputTokens < previous.CacheWriteInputTokens || current.OutputTokens < previous.OutputTokens)
                return false;
        }
        return true;
    }

    private static bool RecordedCursor(JsonElement root, out string? cursor)
    {
        cursor = null;
        if (!root.TryGetProperty("next_cursor", out var value)) return false;
        if (value.ValueKind == JsonValueKind.Null) return true;
        if (value.ValueKind != JsonValueKind.String || value.GetString() is not { } text ||
            !IsSafeOpaqueParameter(text) || text.Any(c => c > 0x7f)) return false;
        cursor = text;
        return true;
    }

    private static RecordedDirectoryPage? ParseRecordedDirectory(byte[] body, PublishedPairIdentity pair)
    {
        try
        {
            using var document = JsonDocument.Parse(body, new JsonDocumentOptions { MaxDepth = 24 });
            var root = document.RootElement;
            if (!Exact(root, "api_version", "observed_at", "period_identity", "scopes", "next_cursor") ||
                !TryGetString(root, "api_version", out var version) || version != "v6" || !UnknownPeriod(root) ||
                !TryGetNullableUnixSeconds(root, "observed_at", out var observed) || !RecordedCursor(root, out var next) ||
                root.GetProperty("scopes").ValueKind != JsonValueKind.Array) return null;
            var result = new List<ApiRecordedScope>();
            foreach (var value in root.GetProperty("scopes").EnumerateArray())
            {
                if (!Exact(value, "id", "kind", "start_at", "end_at", "current", "recordings_available", "saved_observations_available", "coverage") ||
                    !TryGetBoundedString(value, "id", 1, 512, out var id) || !IsSafeOpaqueParameter(id) || id.Any(c => c > 127) ||
                    !TryGetString(value, "kind", out var kind) || kind is not ("rolling" or "utc-month") ||
                    !TryGetPositiveInt64(value, "start_at", out var start) || !TryGetPositiveInt64(value, "end_at", out var end) ||
                    !RecordedBounds(start, end) || !TryGetBoolean(value, "current", out var current) ||
                    current != (kind == "rolling") || !TryGetBoolean(value, "recordings_available", out var recordings) ||
                    !TryGetBoolean(value, "saved_observations_available", out var saved) || !RecordedCoverage(value, out var coverage)) return null;
                result.Add(new(id, kind, start, end, current, recordings, saved, coverage));
            }
            return result.Count == 0 && next is not null ? null : new(observed, result.AsReadOnly(), next, pair);
        }
        catch (Exception) { return null; }
    }

    private static RecordingsPage? ParseRecordings(byte[] body, PublishedPairIdentity pair)
    {
        try
        {
            using var document = JsonDocument.Parse(body, new JsonDocumentOptions { MaxDepth = 24 });
            var root = document.RootElement;
            if (!Exact(root, "api_version", "scope", "period_identity", "coverage_intervals", "samples", "quota_observations", "next_cursor") ||
                !TryGetString(root, "api_version", out var version) || version != "v4" || !UnknownPeriod(root) ||
                !RecordedScope(root.GetProperty("scope"), out var scope) || !RecordedCursor(root, out var next) ||
                root.GetProperty("samples").ValueKind != JsonValueKind.Array ||
                root.GetProperty("quota_observations").ValueKind != JsonValueKind.Array ||
                root.GetProperty("coverage_intervals").ValueKind != JsonValueKind.Array) return null;
            var coverage = new List<ApiRecordedCoverageInterval>();
            var edge = scope!.StartAt;
            foreach (var value in root.GetProperty("coverage_intervals").EnumerateArray())
            {
                if (!Exact(value, "start_at", "end_at", "coverage") ||
                    !TryGetPositiveInt64(value, "start_at", out var start) || !TryGetPositiveInt64(value, "end_at", out var end) ||
                    !RecordedBounds(start, end) || start != edge || end > scope.EndAt || !RecordedCoverage(value, out var code) ||
                    coverage.LastOrDefault() is { } prior && prior.Coverage == code) return null;
                coverage.Add(new(start, end, code));
                edge = end;
            }
            if (edge != scope.EndAt || coverage.Count == 0) return null;
            var aggregate = coverage.Any(c => c.Coverage == ApiRecordedCoverage.Unknown) ? ApiRecordedCoverage.Unknown :
                coverage.Any(c => c.Coverage == ApiRecordedCoverage.Partial) ? ApiRecordedCoverage.Partial : ApiRecordedCoverage.Complete;
            if (scope.Coverage != aggregate) return null;
            var samples = new List<ApiRecordedSample>();
            foreach (var value in root.GetProperty("samples").EnumerateArray())
            {
                if (!Exact(value, "timestamp", "models") || !TryGetUnixSeconds(value, "timestamp", out var timestamp) ||
                    timestamp < scope.StartAt || timestamp >= scope.EndAt || !RecordedModels(value, out var models)) return null;
                samples.Add(new(timestamp, models));
            }
            var quota = new List<ApiRecordedQuotaObservation>();
            foreach (var value in root.GetProperty("quota_observations").EnumerateArray())
            {
                if (!Exact(value, "sequence", "observed_at", "reset_at", "window_seconds", "remaining_percent") ||
                    !TryGetString(value, "sequence", out var sequence) ||
                    !ulong.TryParse(sequence, System.Globalization.NumberStyles.None, System.Globalization.CultureInfo.InvariantCulture, out var number) ||
                    number.ToString(System.Globalization.CultureInfo.InvariantCulture) != sequence ||
                    !TryGetUnixSeconds(value, "observed_at", out var timestamp) || timestamp < scope.StartAt || timestamp >= scope.EndAt ||
                    !TryGetUnixSeconds(value, "reset_at", out var reset) || !TryGetPositiveInt64(value, "window_seconds", out var window) ||
                    !TryGetFiniteDouble(value, "remaining_percent", out var remaining) || remaining is < 0 or > 100) return null;
                quota.Add(new(number, timestamp, reset, window, remaining));
            }
            return new(scope, coverage.AsReadOnly(), samples.AsReadOnly(), quota.AsReadOnly(), next, pair);
        }
        catch (Exception) { return null; }
    }

    private static int CompareSaved(ApiSavedObservation a, ApiSavedObservation b)
    {
        var order = a.Timestamp.CompareTo(b.Timestamp);
        if (order == 0) order = Nullable.Compare(a.RawResetAt, b.RawResetAt);
        if (order == 0) order = string.CompareOrdinal(a.StorageKind, b.StorageKind);
        return order != 0 ? order : string.CompareOrdinal(a.ObservationId, b.ObservationId);
    }

    private static ObservationsPage? ParseSavedObservations(byte[] body, PublishedPairIdentity pair)
    {
        try
        {
            using var document = JsonDocument.Parse(body, new JsonDocumentOptions { MaxDepth = 24 });
            var root = document.RootElement;
            if (!Exact(root, "api_version", "scope", "period_identity", "observations", "next_cursor") ||
                !TryGetString(root, "api_version", out var version) || version != "v4" || !UnknownPeriod(root) ||
                !RecordedScope(root.GetProperty("scope"), out var scope) || !RecordedCursor(root, out var next) ||
                root.GetProperty("observations").ValueKind != JsonValueKind.Array) return null;
            var result = new List<ApiSavedObservation>();
            foreach (var value in root.GetProperty("observations").EnumerateArray())
            {
                if (!Exact(value, "observation_id", "timestamp", "raw_reset_at", "storage_kind", "models", "unresolved_reasons") ||
                    !TryGetBoundedString(value, "observation_id", 1, 512, out var id) || !IsSafeOpaqueParameter(id) || id.Any(c => c > 127) ||
                    !TryGetUnixSeconds(value, "timestamp", out var timestamp) || timestamp < scope!.StartAt || timestamp >= scope.EndAt ||
                    !TryGetNullableUnixSeconds(value, "raw_reset_at", out var reset) ||
                    !TryGetString(value, "storage_kind", out var kind) || kind is not ("legacy-fixed" or "model-sidecar" or "source-event") ||
                    value.GetProperty("models").ValueKind != JsonValueKind.Array ||
                    value.GetProperty("unresolved_reasons").ValueKind != JsonValueKind.Array) return null;
                var reasons = new List<string>();
                foreach (var reason in value.GetProperty("unresolved_reasons").EnumerateArray())
                {
                    if (reason.ValueKind != JsonValueKind.String || reason.GetString() is not { } code ||
                        code is not ("period-identity-unknown" or "source-recovery-pending" or "source-evidence-unavailable" or "duplicate-unresolved" or "invalid-saved-value") ||
                        reasons.LastOrDefault() is { } prior && string.CompareOrdinal(prior, code) >= 0) return null;
                    reasons.Add(code);
                }
                if (!reasons.Contains("period-identity-unknown")) return null;
                var models = new List<ApiSavedModelObservation>();
                var names = new HashSet<string>(StringComparer.Ordinal);
                foreach (var model in value.GetProperty("models").EnumerateArray())
                {
                    if (!Exact(model, "stored_model_key", "total_tokens", "input_tokens", "cached_input_tokens", "cache_write_input_tokens", "output_tokens", "total_dollars") ||
                        !TryGetBoundedString(model, "stored_model_key", 1, 128, out var name) || !names.Add(name) ||
                        !TryGetNullableUInt64(model, "total_tokens", out var total) ||
                        !TryGetNullableUInt64(model, "input_tokens", out var input) ||
                        !TryGetNullableUInt64(model, "cached_input_tokens", out var cached) ||
                        !TryGetNullableUInt64(model, "cache_write_input_tokens", out var write) ||
                        !TryGetNullableUInt64(model, "output_tokens", out var output) ||
                        !TryGetNullableFiniteDouble(model, "total_dollars", 0, double.MaxValue, out var dollars)) return null;
                    models.Add(new(name, total, input, cached, write, output, dollars));
                }
                result.Add(new(id, timestamp, reset, kind, models.AsReadOnly(), reasons.AsReadOnly()));
            }
            return new(scope!, result.AsReadOnly(), next, pair);
        }
        catch (Exception) { return null; }
    }

    private static bool Exact(JsonElement value, params string[] keys) =>
        HasExactlyProperties(value, CreatePropertySet(keys), keys.Length);

    private static bool UnknownPeriod(JsonElement root) => root.TryGetProperty("period_identity", out var value) &&
        Exact(value, "state", "reason") && TryGetString(value, "state", out var state) && state == "unknown" &&
        TryGetString(value, "reason", out var reason) && reason == "provider-period-identity-unavailable";

    private static bool RecordedBounds(long start, long end) => start >= 1 && start < end && end <= 253402300800;

    private static bool RecordedCoverage(JsonElement value, out ApiRecordedCoverage coverage)
    {
        coverage = default;
        if (!TryGetString(value, "coverage", out var code)) return false;
        var parsed = code switch
        {
            "complete" => ApiRecordedCoverage.Complete,
            "partial" => ApiRecordedCoverage.Partial,
            "unknown" => ApiRecordedCoverage.Unknown,
            _ => (ApiRecordedCoverage?)null,
        };
        if (parsed is null) return false;
        coverage = parsed.Value;
        return true;
    }

    private static bool RecordedScope(JsonElement value, out ApiRecordedUsageScope? scope)
    {
        scope = null;
        if (!Exact(value, "kind", "start_at", "end_at", "coverage") ||
            !TryGetString(value, "kind", out var kind) || kind != "recorded-time" ||
            !TryGetPositiveInt64(value, "start_at", out var start) ||
            !TryGetPositiveInt64(value, "end_at", out var end) || !RecordedBounds(start, end) ||
            !RecordedCoverage(value, out var coverage)) return false;
        scope = new(start, end, coverage);
        return true;
    }

    private static bool RecordedModels(JsonElement parent, out IReadOnlyList<ApiRecordedModelUsage> models)
    {
        models = [];
        if (!parent.TryGetProperty("models", out var value) || value.ValueKind != JsonValueKind.Array ||
            value.GetArrayLength() > MaxDetailsModels) return false;
        var result = new List<ApiRecordedModelUsage>();
        byte[]? previous = null;
        foreach (var model in value.EnumerateArray())
        {
            if (!Exact(model, "model", "total_tokens", "input_tokens", "cached_input_tokens",
                    "cache_write_input_tokens", "output_tokens", "estimated_cost") ||
                !TryGetBoundedString(model, "model", 1, 128, out var name) ||
                !TryGetUInt64(model, "total_tokens", out var total) ||
                !TryGetNullableUInt64(model, "input_tokens", out var input) ||
                !TryGetNullableUInt64(model, "cached_input_tokens", out var cached) ||
                !TryGetNullableUInt64(model, "cache_write_input_tokens", out var write) ||
                !TryGetNullableUInt64(model, "output_tokens", out var output) ||
                !TryGetV3ModelCost(model, out var cost)) return false;
            var bytes = Encoding.UTF8.GetBytes(name);
            if (previous is not null && previous.AsSpan().SequenceCompareTo(bytes) >= 0) return false;
            previous = bytes;
            if (input > total || output > total) return false;
            try
            {
                if (input is { } i && output is { } o && checked(i + o) != total) return false;
                if (checked(cached.GetValueOrDefault() + write.GetValueOrDefault()) >
                    (input ?? (output is { } knownOutput ? total - knownOutput : total))) return false;
            }
            catch (OverflowException) { return false; }
            result.Add(new(name, total, input, cached, write, output, cost is { } c
                ? new(c.PriceVersion, c.OrdinaryInputDollars, c.CachedInputDollars,
                    c.CacheWriteInputDollars, c.OutputDollars, c.TotalDollars) : null));
        }
        models = result.AsReadOnly();
        return true;
    }

    private static ApiCurrentSnapshot? ParseRecordedCurrent(byte[] body, PublishedPairIdentity pair)
    {
        try
        {
            using var document = JsonDocument.Parse(body, new JsonDocumentOptions { MaxDepth = 24 });
            var root = document.RootElement;
            if (!Exact(root, "api_version", "state", "observed_at", "authenticated", "plan_label", "quota",
                    "period_identity", "usage_scope", "models_status", "models", "active_thread_count", "open_session_thread_count") ||
                !TryGetString(root, "api_version", out var version) || version != "v5" || !UnknownPeriod(root) ||
                !TryGetState(root, out var state) || !TryGetNullableUnixSeconds(root, "observed_at", out var observed) ||
                !TryGetBoolean(root, "authenticated", out var authenticated) ||
                !TryGetNullablePlanLabel(root, out var plan) || !TryGetQuota(root, out var quota) ||
                !HasValidDetailsRootDomain(state, authenticated, plan, quota) ||
                !RecordedModels(root, out var models) || !TryGetString(root, "models_status", out var status) ||
                !TryGetUInt64(root, "active_thread_count", out var active) ||
                !TryGetUInt64(root, "open_session_thread_count", out var open) || open < active) return null;
            ApiRecordedUsageScope? scope = null;
            var wireScope = root.GetProperty("usage_scope");
            if (wireScope.ValueKind != JsonValueKind.Null && !RecordedScope(wireScope, out scope)) return null;
            var expectedStatus = scope?.Coverage == ApiRecordedCoverage.Complete ? "complete" :
                models.Count > 0 ? "partial" : "unavailable";
            if (status != expectedStatus || scope is null && models.Count != 0) return null;
            if ((!authenticated || state == ApiState.Initializing) &&
                (quota is not null || scope is not null || observed is not null || models.Count != 0 || active != 0 || open != 0)) return null;
            return new(state, observed, authenticated, plan, quota, [], active, pair)
            {
                ApiVersion = version,
                OpenSessionThreadCount = open,
                UsageScope = scope,
                RecordedModels = models,
                ModelsStatus = status switch
                {
                    "complete" => ApiCurrentModelsStatus.Complete,
                    "partial" => ApiCurrentModelsStatus.Partial,
                    _ => ApiCurrentModelsStatus.Unavailable,
                },
            };
        }
        catch (Exception) { return null; }
    }
}
