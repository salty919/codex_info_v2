// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;

namespace CodexInfo.WindowsClient.Core;

/// <remarks>
/// The endpoint and all network policy are fixed here so callers cannot accidentally
/// turn the client into a general-purpose HTTP client.  A handler constructor is
/// provided solely to make the transport boundary testable.
/// </remarks>
public sealed class LoopbackStatusClient : ILoopbackHealthClient, ILoopbackDetailsClient, IDisposable
{
    private const string DetailsV3Endpoint = "http://127.0.0.1:8787/v3/details";
    private const string DetailsV2Endpoint = "http://127.0.0.1:8787/v2/details";
    private const string DetailsEndpoint = "http://127.0.0.1:8787/v1/details";
    private const string HealthEndpoint = "http://127.0.0.1:8787/v1/health";
    private const string PublishedPairHeader = "Codex-Info-Published-Pair";
    private const int MaxResponseHeaderBytes = 8 * 1024;
    private const int MaxHealthBodyBytes = 1024;
    // SQLite retains three months, but one details response is bounded to one
    // 31-day month of minute buckets. The byte envelope is independent.
    private const int MaxDetailsBodyBytes = 32 * 1024 * 1024;
    private const long MaxUnixSeconds = 253_402_300_799;
    private const int MaxHistoryPeriods = 128;
    private const int MaxHistorySamples = 31 * 24 * 60;
    private const int MaxHistoryGaps = 4_096;
    private const int MaxThreads = 256;
    private const int MaxDetailsModels = 1_024;
    private const long ResetAtToleranceSeconds = 60;

    private static readonly HashSet<string> HealthProperties = CreatePropertySet(
        "api_version",
        "service",
        "product_version");

    private static readonly HashSet<string> QuotaProperties = CreatePropertySet(
        "remaining_percent",
        "reset_at",
        "window_seconds",
        "monthly");

    private static readonly HashSet<string> DetailsTopLevelProperties = CreatePropertySet(
        "api_version",
        "state",
        "observed_at",
        "authenticated",
        "plan_label",
        "quota",
        "models",
        "active_thread_count",
        "history_periods",
        "history_samples",
        "history_gaps",
        "threads",
        "estimated_cost_label");

    private static readonly HashSet<string> DetailsModelProperties = CreatePropertySet(
        "name",
        "input_tokens",
        "cached_input_tokens",
        "output_tokens",
        "input_dollars",
        "cached_input_dollars",
        "output_dollars");

    private static readonly HashSet<string> DetailsV3TopLevelProperties = CreatePropertySet(
        "api_version",
        "state",
        "observed_at",
        "authenticated",
        "plan_label",
        "quota",
        "models",
        "active_thread_count",
        "history_periods",
        "history_samples",
        "history_gaps",
        "threads");

    private static readonly HashSet<string> DetailsV3ModelProperties = CreatePropertySet(
        "model",
        "total_tokens",
        "input_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "output_tokens",
        "estimated_cost");

    private static readonly HashSet<string> DetailsV3CostProperties = CreatePropertySet(
        "price_version",
        "ordinary_input_dollars",
        "cached_input_dollars",
        "cache_write_input_dollars",
        "output_dollars",
        "total_dollars");

    private static readonly HashSet<string> HistoryPeriodProperties = CreatePropertySet(
        "id",
        "start_at",
        "end_at",
        "reset_at",
        "label",
        "current");

    private static readonly HashSet<string> HistorySampleProperties = CreatePropertySet(
        "timestamp",
        "reset_at",
        "remaining_percent",
        "sol_dollars",
        "terra_dollars",
        "luna_dollars",
        "sol_tokens",
        "terra_tokens",
        "luna_tokens");

    private static readonly HashSet<string> HistorySampleV2Properties = CreatePropertySet(
        "timestamp",
        "reset_at",
        "remaining_percent",
        "sol_dollars",
        "terra_dollars",
        "luna_dollars",
        "sol_tokens",
        "terra_tokens",
        "luna_tokens",
        "model_source");

    private static readonly HashSet<string> HistorySampleV3Properties = CreatePropertySet(
        "timestamp",
        "reset_at",
        "remaining_percent",
        "models",
        "models_complete",
        "model_source");

    private static readonly HashSet<string> HistoryModelV3Properties = CreatePropertySet(
        "model",
        "total_tokens",
        "input_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "output_tokens",
        "total_dollars");

    private static readonly HashSet<string> HistoryGapProperties = CreatePropertySet(
        "gap_id",
        "reset_at",
        "start_at",
        "end_at",
        "reason");

    private static readonly HashSet<string> HistoryGapReasons = CreatePropertySet(
        "daemon_stop_unrecoverable",
        "reset_hint_expired",
        "auth_epoch_tombstoned");

    private static readonly HashSet<string> ThreadProperties = CreatePropertySet(
        "id",
        "title",
        "parent_thread_id",
        "model",
        "model_label",
        "total_tokens",
        "context_usage_tokens",
        "context_window_tokens",
        "created_at",
        "last_user_message_at",
        "is_subagent",
        "depth");

    private readonly HttpClient _httpClient;
    private readonly object _v3CacheGate = new();
    private ApiDetailsSnapshot? _lastV3Snapshot;
    private PublishedPairIdentity? _lastV3PublishedPair;

    public LoopbackStatusClient()
        : this(CreateDefaultHandler())
    {
    }

    public LoopbackStatusClient(HttpMessageHandler handler)
    {
        ArgumentNullException.ThrowIfNull(handler);
        if (handler is HttpClientHandler httpClientHandler)
        {
            // Keep the policy true even when a caller supplies an
            // HttpClientHandler as a test seam.
            httpClientHandler.UseProxy = false;
            httpClientHandler.AllowAutoRedirect = false;
            httpClientHandler.AutomaticDecompression = DecompressionMethods.None;
            httpClientHandler.UseCookies = false;
            httpClientHandler.MaxResponseHeadersLength = MaxResponseHeaderBytes / 1024;
        }

        _httpClient = new HttpClient(handler, disposeHandler: true)
        {
            Timeout = TimeSpan.FromSeconds(1),
        };
    }

    public async Task<HealthFetchResult> FetchHealthAsync(
        CancellationToken cancellationToken = default)
    {
        try
        {
            using var request = new HttpRequestMessage(HttpMethod.Get, HealthEndpoint);
            request.Headers.Accept.Clear();
            request.Headers.Accept.Add(new MediaTypeWithQualityHeaderValue("application/json"));

            using var response = await _httpClient.SendAsync(
                    request,
                    HttpCompletionOption.ResponseHeadersRead,
                    cancellationToken)
                .ConfigureAwait(false);

            if (response.StatusCode != HttpStatusCode.OK)
            {
                return HealthFetchResult.FromFailure(HealthFetchFailure.Response);
            }

            if (!HasAcceptableHeaderSize(response) ||
                !HasRequiredResponseHeaders(response) ||
                !TryGetContentLength(response.Content, out var contentLength) ||
                contentLength is not long declaredLength)
            {
                return HealthFetchResult.FromFailure(HealthFetchFailure.Response);
            }

            if (declaredLength > MaxHealthBodyBytes)
            {
                return HealthFetchResult.FromFailure(HealthFetchFailure.Response);
            }

            var bodyStatus = await ReadBodyAsync(
                    response.Content,
                    declaredLength,
                    cancellationToken,
                    MaxHealthBodyBytes)
                .ConfigureAwait(false);

            if (bodyStatus.Kind is BodyReadKind.Oversize)
            {
                return HealthFetchResult.FromFailure(HealthFetchFailure.Response);
            }

            if (bodyStatus.Kind is BodyReadKind.Transport || bodyStatus.Body is null)
            {
                return HealthFetchResult.FromFailure(HealthFetchFailure.Transport);
            }

            if (bodyStatus.Body.LongLength != declaredLength)
            {
                return HealthFetchResult.FromFailure(HealthFetchFailure.Response);
            }

            if (!TryParseHealth(bodyStatus.Body, out var snapshot) || snapshot is null)
            {
                return HealthFetchResult.FromFailure(HealthFetchFailure.Response);
            }

            return HealthFetchResult.Success(snapshot);
        }
        catch (OperationCanceledException)
        {
            return HealthFetchResult.FromFailure(HealthFetchFailure.Transport);
        }
        catch (HttpRequestException)
        {
            return HealthFetchResult.FromFailure(HealthFetchFailure.Transport);
        }
        catch (IOException)
        {
            return HealthFetchResult.FromFailure(HealthFetchFailure.Transport);
        }
        catch (Exception)
        {
            return HealthFetchResult.FromFailure(HealthFetchFailure.Transport);
        }
    }

    /// <summary>
    /// Reads the independent details document.  This deliberately has a
    /// separate public result type so callers cannot accidentally turn a
    /// details failure into a status failure.
    /// </summary>
    public async Task<DetailsFetchResult> FetchDetailsAsync(
        CancellationToken cancellationToken = default)
    {
        var v3Attempt = await FetchDetailsEndpointAsync(
                DetailsV3Endpoint,
                "v3",
                cancellationToken)
            .ConfigureAwait(false);
        if (!v3Attempt.NotFound)
        {
            return v3Attempt.Result;
        }

        var v2Attempt = await FetchDetailsEndpointAsync(
                DetailsV2Endpoint,
                "v2",
                cancellationToken)
            .ConfigureAwait(false);
        if (!v2Attempt.NotFound)
        {
            return v2Attempt.Result;
        }

        var v1Attempt = await FetchDetailsEndpointAsync(
                DetailsEndpoint,
                "v1",
                cancellationToken)
            .ConfigureAwait(false);
        return v1Attempt.Result;
    }

    private async Task<DetailsAttemptResult> FetchDetailsEndpointAsync(
        string endpoint,
        string expectedApiVersion,
        CancellationToken cancellationToken)
    {
        try
        {
            using var request = new HttpRequestMessage(HttpMethod.Get, endpoint);
            request.Headers.Accept.Clear();
            request.Headers.Accept.Add(new MediaTypeWithQualityHeaderValue("application/json"));

            PublishedPairIdentity? conditionalPair = null;
            ApiDetailsSnapshot? conditionalSnapshot = null;
            // Only v3 has the conditional request contract. Released v1/v2
            // daemons may reject If-None-Match, so compatibility fallbacks
            // must remain byte-for-byte legacy requests.
            if (expectedApiVersion == "v3")
            {
                lock (_v3CacheGate)
                {
                    conditionalPair = _lastV3PublishedPair;
                    conditionalSnapshot = _lastV3Snapshot;
                }

                if (conditionalPair is { } pair && conditionalSnapshot is not null)
                {
                    request.Headers.IfNoneMatch.Add(new EntityTagHeaderValue($"\"{pair}\""));
                }
            }

            using var response = await _httpClient.SendAsync(
                    request,
                    HttpCompletionOption.ResponseHeadersRead,
                    cancellationToken)
                .ConfigureAwait(false);

            if (response.StatusCode == HttpStatusCode.NotFound)
            {
                return expectedApiVersion is "v3" or "v2"
                    ? DetailsAttemptResult.NotFoundResult()
                    : DetailsAttemptResult.Failure(DetailsFetchFailure.Transport);
            }

            if (response.StatusCode == HttpStatusCode.NotModified)
            {
                if (expectedApiVersion != "v3" ||
                    conditionalPair is not { } expectedPair ||
                    conditionalSnapshot is null ||
                    !HasAcceptableHeaderSize(response) ||
                    !HasRequiredResponseHeaders(response) ||
                    !TryGetContentLength(response.Content, out var notModifiedLength) ||
                    notModifiedLength != 0 ||
                    !TryGetPublishedPairIdentity(response, out var actualPair) ||
                    actualPair != expectedPair)
                {
                    return DetailsAttemptResult.Failure(DetailsFetchFailure.Response);
                }

                var notModifiedBody = await ReadBodyAsync(
                        response.Content,
                        notModifiedLength,
                        cancellationToken,
                        maximumBodyBytes: 0)
                    .ConfigureAwait(false);
                if (notModifiedBody.Kind is not BodyReadKind.Success ||
                    notModifiedBody.Body is null ||
                    notModifiedBody.Body.LongLength != 0)
                {
                    return DetailsAttemptResult.Failure(DetailsFetchFailure.Response);
                }

                return DetailsAttemptResult.NotModifiedResult(expectedSnapshot: conditionalSnapshot);
            }

            if (response.StatusCode != HttpStatusCode.OK)
            {
                return DetailsAttemptResult.Failure(DetailsFetchFailure.Transport);
            }

            if (!HasAcceptableHeaderSize(response) ||
                !HasRequiredResponseHeaders(response) ||
                !TryGetContentLength(response.Content, out var contentLength))
            {
                return DetailsAttemptResult.Failure(DetailsFetchFailure.Response);
            }

            if (!TryGetPublishedPairIdentity(response, out var publishedPair))
            {
                return DetailsAttemptResult.Failure(DetailsFetchFailure.Response);
            }

            if (contentLength is > MaxDetailsBodyBytes)
            {
                return DetailsAttemptResult.Failure(DetailsFetchFailure.Response);
            }

            var bodyStatus = await ReadBodyAsync(
                    response.Content,
                    contentLength,
                    cancellationToken,
                    MaxDetailsBodyBytes)
                .ConfigureAwait(false);

            if (bodyStatus.Kind is BodyReadKind.Oversize)
            {
                return DetailsAttemptResult.Failure(DetailsFetchFailure.Response);
            }

            if (bodyStatus.Kind is BodyReadKind.Transport || bodyStatus.Body is null)
            {
                return DetailsAttemptResult.Failure(DetailsFetchFailure.Transport);
            }

            if (!TryParseDetails(bodyStatus.Body, expectedApiVersion, out var snapshot) || snapshot is null)
            {
                return DetailsAttemptResult.Failure(DetailsFetchFailure.Response);
            }

            var accepted = snapshot with { PublishedPair = publishedPair };
            if (expectedApiVersion == "v3")
            {
                lock (_v3CacheGate)
                {
                    _lastV3Snapshot = accepted;
                    _lastV3PublishedPair = publishedPair;
                }
            }

            return DetailsAttemptResult.Success(accepted);
        }
        catch (OperationCanceledException)
        {
            return DetailsAttemptResult.Failure(DetailsFetchFailure.Transport);
        }
        catch (HttpRequestException)
        {
            return DetailsAttemptResult.Failure(DetailsFetchFailure.Transport);
        }
        catch (IOException)
        {
            return DetailsAttemptResult.Failure(DetailsFetchFailure.Transport);
        }
        catch (Exception)
        {
            return DetailsAttemptResult.Failure(DetailsFetchFailure.Transport);
        }
    }

    public void Dispose() => _httpClient.Dispose();

    private static HttpMessageHandler CreateDefaultHandler() => new HttpClientHandler
    {
        UseProxy = false,
        AllowAutoRedirect = false,
        AutomaticDecompression = DecompressionMethods.None,
        UseCookies = false,
        MaxResponseHeadersLength = MaxResponseHeaderBytes / 1024,
    };

    private static HashSet<string> CreatePropertySet(params string[] properties) =>
        new(properties, StringComparer.Ordinal);

    private static DetailsFetchResult DetailsFailure(DetailsFetchFailure failure) =>
        DetailsFetchResult.FromFailure(failure);

    private static bool HasAcceptableHeaderSize(HttpResponseMessage response)
    {
        try
        {
            long bytes = 2; // final CRLF
            bytes = CountHeaders(response.Headers, bytes);
            bytes = CountHeaders(response.Content?.Headers, bytes);
            return bytes <= MaxResponseHeaderBytes;
        }
        catch (Exception)
        {
            return false;
        }
    }

    private static long CountHeaders(HttpContentHeaders? headers, long current)
    {
        if (headers is null)
        {
            return current;
        }

        return CountHeaders((IEnumerable<KeyValuePair<string, IEnumerable<string>>>)headers, current);
    }

    private static long CountHeaders(HttpResponseHeaders headers, long current) =>
        CountHeaders((IEnumerable<KeyValuePair<string, IEnumerable<string>>>)headers, current);

    private static long CountHeaders(
        IEnumerable<KeyValuePair<string, IEnumerable<string>>> headers,
        long current)
    {
        var bytes = current;
        foreach (var header in headers)
        {
            var values = header.Value.ToArray();
            if (values.Length == 0)
            {
                bytes = checked(bytes + Encoding.UTF8.GetByteCount(header.Key) + 4);
                continue;
            }

            foreach (var value in values)
            {
                // Each value is counted as an independent wire header field;
                // this is conservative for coalesced multi-value headers.
                bytes = checked(
                    bytes +
                    Encoding.UTF8.GetByteCount(header.Key) +
                    2 +
                    Encoding.UTF8.GetByteCount(value) +
                    2);
            }
        }

        return bytes;
    }

    private static bool HasRequiredResponseHeaders(HttpResponseMessage response)
    {
        try
        {
            var contentType = response.Content?.Headers.ContentType;
            var mediaType = contentType?.MediaType;
            var charset = contentType?.CharSet?.Trim('"');
            return mediaType is not null &&
                   mediaType.Equals("application/json", StringComparison.OrdinalIgnoreCase) &&
                   charset is not null &&
                   charset.Equals("utf-8", StringComparison.OrdinalIgnoreCase) &&
                   response.Headers.CacheControl?.NoStore == true;
        }
        catch (Exception)
        {
            return false;
        }
    }

    private static bool TryGetPublishedPairIdentity(
        HttpResponseMessage response,
        out PublishedPairIdentity identity)
    {
        identity = default;

        try
        {
            if (!response.Headers.TryGetValues(PublishedPairHeader, out var values))
            {
                return false;
            }

            var materializedValues = values.ToArray();
            return materializedValues.Length == 1 &&
                   PublishedPairIdentity.TryCreate(materializedValues[0], out identity);
        }
        catch (Exception)
        {
            identity = default;
            return false;
        }
    }

    private static bool TryGetContentLength(HttpContent content, out long? contentLength)
    {
        try
        {
            contentLength = content.Headers.ContentLength;
            return contentLength is null or >= 0;
        }
        catch (Exception)
        {
            contentLength = null;
            return false;
        }
    }

    private static async Task<BodyReadResult> ReadBodyAsync(
        HttpContent content,
        long? contentLength,
        CancellationToken cancellationToken,
        int maximumBodyBytes)
    {
        try
        {
            await using var stream = await content.ReadAsStreamAsync(cancellationToken).ConfigureAwait(false);
            using var body = new MemoryStream(
                capacity: contentLength is long length && length >= 0 && length <= maximumBodyBytes
                    ? (int)length
                    : maximumBodyBytes);
            var buffer = new byte[8 * 1024];
            var count = 0;

            while (true)
            {
                var read = await stream.ReadAsync(buffer.AsMemory(), cancellationToken).ConfigureAwait(false);
                if (read == 0)
                {
                    return BodyReadResult.Success(body.ToArray());
                }

                if (read > maximumBodyBytes - count)
                {
                    return BodyReadResult.Oversize();
                }

                body.Write(buffer, 0, read);
                count += read;
            }
        }
        catch (OperationCanceledException)
        {
            return BodyReadResult.Transport();
        }
        catch (HttpRequestException)
        {
            return BodyReadResult.Transport();
        }
        catch (IOException)
        {
            return BodyReadResult.Transport();
        }
        catch (Exception)
        {
            return BodyReadResult.Transport();
        }
    }

    private static bool TryParseHealth(byte[] body, out ApiHealthSnapshot? snapshot)
    {
        snapshot = null;

        try
        {
            using var document = JsonDocument.Parse(
                body,
                new JsonDocumentOptions
                {
                    AllowTrailingCommas = false,
                    CommentHandling = JsonCommentHandling.Disallow,
                    MaxDepth = 4,
                });

            var root = document.RootElement;
            if (!HasExactlyProperties(root, HealthProperties, 3) ||
                !TryGetString(root, "api_version", out var apiVersion) ||
                apiVersion != "v1" ||
                !TryGetBoundedString(root, "service", 1, 64, out var service) ||
                service != "codex-info" ||
                !TryGetBoundedString(root, "product_version", 1, 32, out var productVersion) ||
                !IsCanonicalStableVersion(productVersion))
            {
                return false;
            }

            snapshot = new ApiHealthSnapshot(apiVersion, service, productVersion);
            return true;
        }
        catch (JsonException)
        {
            return false;
        }
        catch (ArgumentException)
        {
            return false;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
        catch (Exception)
        {
            return false;
        }
    }

    private static bool IsCanonicalStableVersion(string value)
    {
        var components = value.Split('.');
        return components.Length == 3 && components.All(component =>
            component.Length > 0 &&
            (component.Length == 1 || component[0] != '0') &&
            component.All(character => character is >= '0' and <= '9'));
    }

    private static bool TryParseDetails(
        byte[] body,
        string expectedApiVersion,
        out ApiDetailsSnapshot? snapshot)
    {
        if (expectedApiVersion == "v3")
        {
            return TryParseDetailsV3(body, out snapshot);
        }

        snapshot = null;

        try
        {
            using var document = JsonDocument.Parse(
                body,
                new JsonDocumentOptions
                {
                    AllowTrailingCommas = false,
                    CommentHandling = JsonCommentHandling.Disallow,
                    MaxDepth = 24,
                });

            var root = document.RootElement;
            var isV1 = expectedApiVersion == "v1";
            var topLevelPropertyCount = 13;
            if (!HasExactlyProperties(root, DetailsTopLevelProperties, topLevelPropertyCount) ||
                !TryGetString(root, "api_version", out var apiVersion) ||
                apiVersion != expectedApiVersion)
            {
                return false;
            }

            if (!TryGetState(root, out var state) ||
                !TryGetNullableUnixSeconds(root, "observed_at", out var observedAt) ||
                !TryGetBoolean(root, "authenticated", out var authenticated) ||
                !TryGetNullablePlanLabel(root, out var planLabel) ||
                !TryGetQuota(root, out var quota) ||
                !HasValidDetailsRootDomain(state, authenticated, planLabel, quota) ||
                !TryGetDetailsModels(root, out var models) ||
                !TryGetUInt64(root, "active_thread_count", out var activeThreadCount) ||
                !TryGetHistoryPeriods(root, observedAt, out var historyPeriods) ||
                !TryGetFlatHistorySamples(root, historyPeriods, expectedApiVersion, out var historySamples) ||
                !TryGetHistoryGaps(root, historyPeriods, out var historyGaps) ||
                !TryGetThreads(root, out var threads) ||
                !TryGetBoundedString(root, "estimated_cost_label", 1, 160, out var estimatedCostLabel))
            {
                return false;
            }

            historyPeriods = historyPeriods
                .Select(period => period with
                {
                    Samples = SamplesForCanonicalPeriod(period, historySamples),
                })
                .ToList();

            snapshot = new ApiDetailsSnapshot(
                state,
                observedAt,
                authenticated,
                planLabel,
                quota,
                new System.Collections.ObjectModel.ReadOnlyCollection<ApiDetailsModelUsage>(models),
                activeThreadCount,
                new System.Collections.ObjectModel.ReadOnlyCollection<ApiHistoryPeriod>(historyPeriods),
                new System.Collections.ObjectModel.ReadOnlyCollection<ApiHistorySample>(historySamples),
                new System.Collections.ObjectModel.ReadOnlyCollection<ApiThreadDetails>(threads),
                estimatedCostLabel);
            snapshot = snapshot with
            {
                ApiVersion = apiVersion,
                HistoryGaps = new System.Collections.ObjectModel.ReadOnlyCollection<ApiHistoryGap>(historyGaps),
            };
            return true;
        }
        catch (JsonException)
        {
            return false;
        }
        catch (ArgumentException)
        {
            return false;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
        catch (Exception)
        {
            return false;
        }
    }

    private static bool TryParseDetailsV3(
        byte[] body,
        out ApiDetailsSnapshot? snapshot)
    {
        snapshot = null;

        try
        {
            using var document = JsonDocument.Parse(
                body,
                new JsonDocumentOptions
                {
                    AllowTrailingCommas = false,
                    CommentHandling = JsonCommentHandling.Disallow,
                    MaxDepth = 24,
                });

            var root = document.RootElement;
            if (!HasExactlyProperties(root, DetailsV3TopLevelProperties, 12) ||
                !TryGetString(root, "api_version", out var apiVersion) ||
                apiVersion != "v3" ||
                !TryGetState(root, out var state) ||
                !TryGetNullableUnixSeconds(root, "observed_at", out var observedAt) ||
                !TryGetBoolean(root, "authenticated", out var authenticated) ||
                !TryGetNullablePlanLabel(root, out var planLabel) ||
                !TryGetQuota(root, out var quota) ||
                !HasValidDetailsRootDomain(state, authenticated, planLabel, quota) ||
                !TryGetDetailsModelsV3(root, out var models) ||
                !TryGetUInt64(root, "active_thread_count", out var activeThreadCount) ||
                !TryGetHistoryPeriods(root, observedAt, out var historyPeriods) ||
                !TryGetHistorySamplesV3(root, historyPeriods, out var historySamples) ||
                !TryGetHistoryGaps(root, historyPeriods, out var historyGaps) ||
                !TryGetThreads(root, out var threads))
            {
                return false;
            }

            historyPeriods = historyPeriods
                .Select(period => period with
                {
                    Samples = SamplesForCanonicalPeriod(period, historySamples),
                })
                .ToList();

            snapshot = new ApiDetailsSnapshot(
                state,
                observedAt,
                authenticated,
                planLabel,
                quota,
                new System.Collections.ObjectModel.ReadOnlyCollection<ApiDetailsModelUsage>(models),
                activeThreadCount,
                new System.Collections.ObjectModel.ReadOnlyCollection<ApiHistoryPeriod>(historyPeriods),
                new System.Collections.ObjectModel.ReadOnlyCollection<ApiHistorySample>(historySamples),
                new System.Collections.ObjectModel.ReadOnlyCollection<ApiThreadDetails>(threads),
                // v3 intentionally has no estimated_cost_label root field.
                "概算 —")
            {
                ApiVersion = apiVersion,
                HistoryGaps = new System.Collections.ObjectModel.ReadOnlyCollection<ApiHistoryGap>(historyGaps),
            };
            return true;
        }
        catch (JsonException)
        {
            return false;
        }
        catch (ArgumentException)
        {
            return false;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
        catch (Exception)
        {
            return false;
        }
    }

    private static bool TryGetDetailsModelsV3(
        JsonElement parent,
        out List<ApiDetailsModelUsage> models)
    {
        models = new List<ApiDetailsModelUsage>();
        if (!parent.TryGetProperty("models", out var property) ||
            property.ValueKind != JsonValueKind.Array ||
            property.GetArrayLength() > MaxDetailsModels)
        {
            return false;
        }

        var names = new HashSet<string>(StringComparer.Ordinal);
        foreach (var model in property.EnumerateArray())
        {
            if (!HasExactlyProperties(model, DetailsV3ModelProperties, 7) ||
                !TryGetBoundedString(model, "model", 1, 128, out var name) ||
                !names.Add(name) ||
                !TryGetUInt64(model, "total_tokens", out var totalTokens) ||
                !TryGetUInt64(model, "input_tokens", out var inputTokens) ||
                !TryGetUInt64(model, "cached_input_tokens", out var cachedInputTokens) ||
                !TryGetNullableUInt64(model, "cache_write_input_tokens", out var cacheWriteInputTokens) ||
                !TryGetUInt64(model, "output_tokens", out var outputTokens) ||
                cachedInputTokens > inputTokens ||
                (cacheWriteInputTokens is ulong cacheWrite &&
                 (cacheWrite > inputTokens || inputTokens - cacheWrite < cachedInputTokens)) ||
                !TryGetV3ModelCost(model, out var cost))
            {
                return false;
            }

            models.Add(new ApiDetailsModelUsage(
                name,
                inputTokens,
                cachedInputTokens,
                outputTokens,
                cost?.OrdinaryInputDollars ?? double.NaN,
                cost?.CachedInputDollars ?? double.NaN,
                cost?.OutputDollars ?? double.NaN)
            {
                TotalTokens = totalTokens,
                CacheWriteInputTokens = cacheWriteInputTokens,
                CacheWriteInputDollars = cost?.CacheWriteInputDollars ?? double.NaN,
                PriceVersion = cost?.PriceVersion,
                EstimatedTotalDollars = cost?.TotalDollars,
            });
        }

        return true;
    }

    private static bool TryGetV3ModelCost(
        JsonElement parent,
        out V3ModelCost? cost)
    {
        cost = null;
        if (!parent.TryGetProperty("estimated_cost", out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (!HasExactlyProperties(property, DetailsV3CostProperties, 6) ||
            !TryGetBoundedString(property, "price_version", 1, 128, out var priceVersion) ||
            !TryGetNonNegativeFiniteDouble(property, "ordinary_input_dollars", out var ordinaryInputDollars) ||
            !TryGetNonNegativeFiniteDouble(property, "cached_input_dollars", out var cachedInputDollars) ||
            !TryGetNonNegativeFiniteDouble(property, "cache_write_input_dollars", out var cacheWriteInputDollars) ||
            !TryGetNonNegativeFiniteDouble(property, "output_dollars", out var outputDollars) ||
            !TryGetNonNegativeFiniteDouble(property, "total_dollars", out var totalDollars))
        {
            return false;
        }

        var sum = ordinaryInputDollars + cachedInputDollars + cacheWriteInputDollars + outputDollars;
        if (!double.IsFinite(sum) || Math.Abs(sum - totalDollars) > 0.000001)
        {
            return false;
        }

        cost = new V3ModelCost(
            priceVersion,
            ordinaryInputDollars,
            cachedInputDollars,
            cacheWriteInputDollars,
            outputDollars,
            totalDollars);
        return true;
    }

    private static bool TryGetHistorySamplesV3(
        JsonElement parent,
        IReadOnlyList<ApiHistoryPeriod> periods,
        out List<ApiHistorySample> samples)
    {
        samples = new List<ApiHistorySample>();
        if (!parent.TryGetProperty("history_samples", out var property) ||
            property.ValueKind != JsonValueKind.Array ||
            property.GetArrayLength() > MaxHistorySamples)
        {
            return false;
        }

        long previousResetAt = 0;
        long previousTimestamp = 0;
        var identities = new HashSet<(long ResetAt, long Timestamp)>();
        var canonicalIdentities = new HashSet<(string PeriodId, long Timestamp)>();
        foreach (var sample in property.EnumerateArray())
        {
            if (!HasExactlyProperties(sample, HistorySampleV3Properties, 6) ||
                !TryGetUnixSeconds(sample, "timestamp", out var timestamp) ||
                !TryGetUnixSeconds(sample, "reset_at", out var resetAt) ||
                timestamp % 60 != 0 ||
                !TryGetNullableRemainingPercent(sample, out var remainingPercent) ||
                !TryGetBoolean(sample, "models_complete", out var modelsComplete) ||
                !TryGetString(sample, "model_source", out var modelSource) ||
                modelSource is not ApiHistorySample.ConfirmedModelSource and
                    not ApiHistorySample.UnavailableModelSource and
                    not ApiHistorySample.LegacyUnknownModelSource ||
                !TryGetHistoryModelsV3(sample, modelSource, modelsComplete, out var modelSamples))
            {
                return false;
            }

            if (!identities.Add((resetAt, timestamp)) ||
                (samples.Count > 0 &&
                 (resetAt < previousResetAt ||
                  (resetAt == previousResetAt && timestamp <= previousTimestamp))))
            {
                return false;
            }

            var matchingPeriods = periods
                .Where(period => resetAt >= period.ResetAt - ResetAtToleranceSeconds &&
                                 resetAt <= period.ResetAt)
                .ToArray();
            if (matchingPeriods.Length != 1 ||
                timestamp < matchingPeriods[0].StartAt ||
                timestamp > matchingPeriods[0].EndAt ||
                !canonicalIdentities.Add((matchingPeriods[0].Id, timestamp)))
            {
                return false;
            }

            samples.Add(new ApiHistorySample(
                timestamp,
                resetAt,
                remainingPercent,
                null,
                null,
                null,
                null,
                null,
                null,
                modelSource)
            {
                ModelsComplete = modelsComplete,
                ModelSamples = modelSamples,
            });
            previousResetAt = resetAt;
            previousTimestamp = timestamp;
        }

        return true;
    }

    private static bool TryGetHistoryModelsV3(
        JsonElement parent,
        string modelSource,
        bool modelsComplete,
        out IReadOnlyList<ApiHistoryModelSample>? models)
    {
        models = null;
        if (!parent.TryGetProperty("models", out var property))
        {
            return false;
        }

        if (modelSource == ApiHistorySample.UnavailableModelSource)
        {
            return property.ValueKind == JsonValueKind.Null && !modelsComplete;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return modelSource == ApiHistorySample.LegacyUnknownModelSource && !modelsComplete;
        }

        if (property.ValueKind != JsonValueKind.Array ||
            property.GetArrayLength() > MaxDetailsModels ||
            modelSource == ApiHistorySample.ConfirmedModelSource && !modelsComplete ||
            modelSource == ApiHistorySample.LegacyUnknownModelSource && modelsComplete)
        {
            return false;
        }

        var parsed = new List<ApiHistoryModelSample>();
        var names = new HashSet<string>(StringComparer.Ordinal);
        foreach (var model in property.EnumerateArray())
        {
            if (!HasAllowedProperties(model, HistoryModelV3Properties, 2, 7) ||
                !TryGetBoundedString(model, "model", 1, 128, out var name) ||
                !names.Add(name) ||
                !TryGetUInt64(model, "total_tokens", out var totalTokens) ||
                !TryGetOptionalNullableUInt64(model, "input_tokens", out var inputTokens) ||
                !TryGetOptionalNullableUInt64(model, "cached_input_tokens", out var cachedInputTokens) ||
                !TryGetOptionalNullableUInt64(model, "cache_write_input_tokens", out var cacheWriteInputTokens) ||
                !TryGetOptionalNullableUInt64(model, "output_tokens", out var outputTokens) ||
                !TryGetOptionalNullableNonNegativeFiniteDouble(model, "total_dollars", out var totalDollars))
            {
                return false;
            }

            var anyComponents = inputTokens is not null || cachedInputTokens is not null || outputTokens is not null;
            var allComponents = inputTokens is not null && cachedInputTokens is not null && outputTokens is not null;
            if (anyComponents != allComponents ||
                (!allComponents && cacheWriteInputTokens is not null) ||
                (modelSource == ApiHistorySample.ConfirmedModelSource && !allComponents))
            {
                return false;
            }

            if (allComponents)
            {
                var input = inputTokens!.Value;
                var cached = cachedInputTokens!.Value;
                if (cached > input ||
                    (cacheWriteInputTokens is ulong cacheWrite &&
                     (cacheWrite > input || input - cacheWrite < cached)))
                {
                    return false;
                }
            }

            parsed.Add(new ApiHistoryModelSample(
                name,
                inputTokens,
                cachedInputTokens,
                outputTokens,
                totalDollars)
            {
                CacheWriteInputTokens = cacheWriteInputTokens,
                TotalTokens = totalTokens,
            });
        }

        models = new System.Collections.ObjectModel.ReadOnlyCollection<ApiHistoryModelSample>(parsed);
        return true;
    }

    private static IReadOnlyList<ApiHistorySample> SamplesForCanonicalPeriod(
        ApiHistoryPeriod period,
        IReadOnlyList<ApiHistorySample> samples)
    {
        return new System.Collections.ObjectModel.ReadOnlyCollection<ApiHistorySample>(
            samples
                .Where(sample => sample.ResetAt >= period.ResetAt - ResetAtToleranceSeconds &&
                                 sample.ResetAt <= period.ResetAt)
                .Select(sample => sample with { ResetAt = period.ResetAt })
                .ToList());
    }

    private static bool TryGetHistoryModelSource(
        JsonElement sample,
        out string modelSource)
    {
        modelSource = string.Empty;
        if (!TryGetString(sample, "model_source", out var candidate))
        {
            return false;
        }

        if (candidate is not ApiHistorySample.ConfirmedModelSource and
            not ApiHistorySample.UnavailableModelSource and
            not ApiHistorySample.LegacyUnknownModelSource)
        {
            return false;
        }

        modelSource = candidate;
        return true;
    }

    private static bool TryGetDetailsModels(
        JsonElement parent,
        out List<ApiDetailsModelUsage> models)
    {
        models = new List<ApiDetailsModelUsage>();
        if (!parent.TryGetProperty("models", out var property) ||
            property.ValueKind != JsonValueKind.Array ||
            property.GetArrayLength() > 3)
        {
            return false;
        }

        var names = new HashSet<string>(StringComparer.Ordinal);
        foreach (var model in property.EnumerateArray())
        {
            if (!HasExactlyProperties(model, DetailsModelProperties, 7) ||
                !TryGetString(model, "name", out var name) ||
                !names.Add(name) ||
                !IsSupportedModel(name) ||
                !TryGetUInt64(model, "input_tokens", out var inputTokens) ||
                !TryGetUInt64(model, "cached_input_tokens", out var cachedInputTokens) ||
                !TryGetUInt64(model, "output_tokens", out var outputTokens) ||
                !TryGetNonNegativeFiniteDouble(model, "input_dollars", out var inputDollars) ||
                !TryGetNonNegativeFiniteDouble(model, "cached_input_dollars", out var cachedInputDollars) ||
                !TryGetNonNegativeFiniteDouble(model, "output_dollars", out var outputDollars))
            {
                return false;
            }

            models.Add(new ApiDetailsModelUsage(
                name,
                inputTokens,
                cachedInputTokens,
                outputTokens,
                inputDollars,
                cachedInputDollars,
                outputDollars));
        }

        return true;
    }

    private static bool TryGetHistoryPeriods(
        JsonElement parent,
        long? observedAt,
        out List<ApiHistoryPeriod> periods)
    {
        periods = new List<ApiHistoryPeriod>();
        if (!parent.TryGetProperty("history_periods", out var property) ||
            property.ValueKind != JsonValueKind.Array ||
            property.GetArrayLength() > MaxHistoryPeriods)
        {
            return false;
        }

        var periodIds = new HashSet<string>(StringComparer.Ordinal);
        var resetAts = new HashSet<long>();
        var currentCount = 0;
        ApiHistoryPeriod? previous = null;
        foreach (var period in property.EnumerateArray())
        {
            if (!HasExactlyProperties(period, HistoryPeriodProperties, 6) ||
                !TryGetBoundedString(period, "id", 1, 512, out var id) ||
                !periodIds.Add(id) ||
                !TryGetUnixSeconds(period, "start_at", out var startAt) ||
                !TryGetUnixSeconds(period, "end_at", out var endAt) ||
                !TryGetUnixSeconds(period, "reset_at", out var resetAt) ||
                endAt < startAt ||
                resetAt < endAt ||
                !resetAts.Add(resetAt) ||
                !TryGetBoundedString(period, "label", 1, 512, out var label) ||
                !TryGetBoolean(period, "current", out var current))
            {
                return false;
            }

            var candidate = new ApiHistoryPeriod(
                id,
                startAt,
                endAt,
                current,
                label)
            {
                ResetAt = resetAt,
            };
            if (previous is not null &&
                (candidate.StartAt > previous.StartAt ||
                 (candidate.StartAt == previous.StartAt && candidate.ResetAt > previous.ResetAt) ||
                 (candidate.StartAt == previous.StartAt && candidate.ResetAt == previous.ResetAt &&
                  string.CompareOrdinal(candidate.Id, previous.Id) > 0)))
            {
                return false;
            }

            if (current)
            {
                currentCount++;
                if (observedAt is not long observed || candidate.EndAt != Math.Min(candidate.ResetAt, observed))
                {
                    return false;
                }
            }

            previous = candidate;
            periods.Add(candidate);
        }

        return currentCount <= 1;
    }

    private static bool TryGetFlatHistorySamples(
        JsonElement parent,
        IReadOnlyList<ApiHistoryPeriod> periods,
        string apiVersion,
        out List<ApiHistorySample> samples)
    {
        samples = new List<ApiHistorySample>();
        var isV1 = apiVersion == "v1";
        var sampleProperties = isV1
            ? HistorySampleProperties
            : HistorySampleV2Properties;
        var samplePropertyCount = isV1 ? 9 : 10;
        if (!parent.TryGetProperty("history_samples", out var property) ||
            property.ValueKind != JsonValueKind.Array ||
            property.GetArrayLength() > MaxHistorySamples)
        {
            return false;
        }

        long previousResetAt = 0;
        long previousTimestamp = 0;
        var identities = new HashSet<(long ResetAt, long Timestamp)>();
        var canonicalIdentities = new HashSet<(string PeriodId, long Timestamp)>();
        foreach (var sample in property.EnumerateArray())
        {
            if (!HasExactlyProperties(sample, sampleProperties, samplePropertyCount) ||
                !TryGetUnixSeconds(sample, "timestamp", out var timestamp) ||
                !TryGetUnixSeconds(sample, "reset_at", out var resetAt) ||
                timestamp % 60 != 0 ||
                !TryGetNullableRemainingPercent(sample, out var remainingPercent) ||
                !TryGetNullableNonNegativeFiniteDouble(sample, "sol_dollars", out var solDollars) ||
                !TryGetNullableNonNegativeFiniteDouble(sample, "terra_dollars", out var terraDollars) ||
                !TryGetNullableNonNegativeFiniteDouble(sample, "luna_dollars", out var lunaDollars) ||
                !TryGetNullableUInt64(sample, "sol_tokens", out var solTokens) ||
                !TryGetNullableUInt64(sample, "terra_tokens", out var terraTokens))
            {
                return false;
            }

            ulong? lunaTokens = null;
            if (!TryGetNullableUInt64(sample, "luna_tokens", out lunaTokens))
            {
                return false;
            }

            var modelSource = ApiHistorySample.LegacyUnknownModelSource;
            if (!isV1)
            {
                if (!TryGetHistoryModelSource(sample, out modelSource))
                {
                    return false;
                }

                var allModelValuesNull = solDollars is null &&
                    terraDollars is null &&
                    lunaDollars is null &&
                    solTokens is null &&
                    terraTokens is null &&
                    lunaTokens is null;
                var allModelValuesPresent = solDollars is not null &&
                    terraDollars is not null &&
                    lunaDollars is not null &&
                    solTokens is not null &&
                    terraTokens is not null &&
                    lunaTokens is not null;
                if (modelSource == ApiHistorySample.UnavailableModelSource
                        ? !allModelValuesNull
                        : !allModelValuesPresent)
                {
                    return false;
                }
            }
            else if (solDollars is null ||
                     terraDollars is null ||
                     lunaDollars is null ||
                     solTokens is null ||
                     terraTokens is null ||
                     lunaTokens is null)
            {
                return false;
            }

            if (!identities.Add((resetAt, timestamp)) ||
                (samples.Count > 0 &&
                 (resetAt < previousResetAt ||
                  (resetAt == previousResetAt && timestamp <= previousTimestamp))))
            {
                return false;
            }

            var matchingPeriods = periods
                .Where(period => resetAt >= period.ResetAt - ResetAtToleranceSeconds &&
                                 resetAt <= period.ResetAt)
                .ToArray();
            if (matchingPeriods.Length != 1 ||
                timestamp < matchingPeriods[0].StartAt ||
                timestamp > matchingPeriods[0].EndAt ||
                !canonicalIdentities.Add((matchingPeriods[0].Id, timestamp)))
            {
                return false;
            }

            samples.Add(new ApiHistorySample(
                timestamp,
                resetAt,
                remainingPercent,
                solDollars,
                terraDollars,
                lunaDollars,
                solTokens,
                terraTokens,
                lunaTokens,
                modelSource)
            {
                ModelsComplete = modelSource == ApiHistorySample.ConfirmedModelSource,
            });
            previousResetAt = resetAt;
            previousTimestamp = timestamp;
        }

        return true;
    }

    private static bool TryGetHistoryGaps(
        JsonElement parent,
        IReadOnlyList<ApiHistoryPeriod> periods,
        out List<ApiHistoryGap> gaps)
    {
        gaps = new List<ApiHistoryGap>();
        if (!parent.TryGetProperty("history_gaps", out var property) ||
            property.ValueKind != JsonValueKind.Array ||
            property.GetArrayLength() > MaxHistoryGaps)
        {
            return false;
        }

        var gapIds = new HashSet<string>(StringComparer.Ordinal);
        ApiHistoryGap? previous = null;
        foreach (var gap in property.EnumerateArray())
        {
            if (!HasExactlyProperties(gap, HistoryGapProperties, 5) ||
                !TryGetString(gap, "gap_id", out var gapId) ||
                !IsLowercaseHexId(gapId) ||
                !gapIds.Add(gapId) ||
                !TryGetUnixSeconds(gap, "reset_at", out var resetAt) ||
                !TryGetUnixSeconds(gap, "start_at", out var startAt) ||
                !TryGetUnixSeconds(gap, "end_at", out var endAt) ||
                startAt > endAt ||
                !TryGetString(gap, "reason", out var reason) ||
                !HistoryGapReasons.Contains(reason))
            {
                return false;
            }

            var matchingPeriods = periods.Where(period => period.ResetAt == resetAt).ToArray();
            if (matchingPeriods.Length != 1 ||
                startAt < matchingPeriods[0].StartAt ||
                endAt > matchingPeriods[0].EndAt)
            {
                return false;
            }

            var candidate = new ApiHistoryGap(gapId, resetAt, startAt, endAt, reason);
            if (previous is not null && CompareHistoryGaps(previous, candidate) >= 0)
            {
                return false;
            }

            if (previous is not null && previous.ResetAt == candidate.ResetAt &&
                candidate.StartAt <= previous.EndAt)
            {
                return false;
            }

            previous = candidate;
            gaps.Add(candidate);
        }

        return true;
    }

    private static int CompareHistoryGaps(ApiHistoryGap left, ApiHistoryGap right)
    {
        var comparison = left.ResetAt.CompareTo(right.ResetAt);
        if (comparison != 0)
        {
            return comparison;
        }

        comparison = left.StartAt.CompareTo(right.StartAt);
        if (comparison != 0)
        {
            return comparison;
        }

        comparison = left.EndAt.CompareTo(right.EndAt);
        return comparison != 0 ? comparison : string.CompareOrdinal(left.GapId, right.GapId);
    }

    private static bool IsLowercaseHexId(string value) =>
        value.Length == 32 && value.All(character => character is >= '0' and <= '9' or >= 'a' and <= 'f');

    private static bool TryGetThreads(
        JsonElement parent,
        out List<ApiThreadDetails> threads)
    {
        threads = new List<ApiThreadDetails>();
        if (!parent.TryGetProperty("threads", out var property) ||
            property.ValueKind != JsonValueKind.Array ||
            property.GetArrayLength() > MaxThreads)
        {
            return false;
        }

        var ids = new HashSet<string>(StringComparer.Ordinal);
        var pending = new List<(string Id, string Title, string? ParentId, string Model, string ModelLabel, ulong? TotalTokens, ulong? ContextTokens, ulong? ContextLimit, long? CreatedAt, long? LastUserMessageAt, bool IsSubAgent, int? Depth)>();
        foreach (var thread in property.EnumerateArray())
        {
            if (!HasExactlyProperties(thread, ThreadProperties, 12) ||
                !TryGetBoundedString(thread, "id", 1, 512, out var id) ||
                !ids.Add(id) ||
                !TryGetBoundedString(thread, "title", 1, 512, out var title) ||
                !TryGetNullableBoundedString(thread, "parent_thread_id", 1, 512, out var parentId) ||
                !TryGetBoundedString(thread, "model", 1, 128, out var model) ||
                !TryGetBoundedString(thread, "model_label", 1, 24, out var modelLabel) ||
                !TryGetNullableUInt64(thread, "total_tokens", out var totalTokens) ||
                !TryGetNullableUInt64(thread, "context_usage_tokens", out var contextTokens) ||
                !TryGetNullableUInt64(thread, "context_window_tokens", out var contextLimit) ||
                !TryGetNullableUnixSeconds(thread, "created_at", out var createdAt) ||
                !TryGetNullableUnixSeconds(thread, "last_user_message_at", out var lastUserMessageAt) ||
                !TryGetBoolean(thread, "is_subagent", out var isSubAgent) ||
                !TryGetNullableDepth(thread, out var depth))
            {
                return false;
            }

            pending.Add((id, title, parentId, model, modelLabel, totalTokens, contextTokens, contextLimit, createdAt, lastUserMessageAt, isSubAgent, depth));
        }

        foreach (var item in pending)
        {
            var isOrphan = item.ParentId is { } parentId && !ids.Contains(parentId);
            threads.Add(new ApiThreadDetails(item.Id, item.Title, item.ParentId, item.Model, item.ModelLabel,
                item.TotalTokens, item.ContextTokens, item.ContextLimit, item.CreatedAt,
                item.LastUserMessageAt, item.IsSubAgent, item.Depth, isOrphan));
        }

        // A cycle has no valid parent-first projection and must reject the
        // complete details generation. Orphans remain representable because
        // their missing parent is an explicit display state.
        var parentById = pending.ToDictionary(item => item.Id, item => item.ParentId, StringComparer.Ordinal);
        foreach (var id in parentById.Keys)
        {
            var seen = new HashSet<string>(StringComparer.Ordinal) { id };
            var current = id;
            while (parentById.TryGetValue(current, out var parentId) && parentId is not null &&
                   parentById.ContainsKey(parentId))
            {
                if (!seen.Add(parentId))
                {
                    threads.Clear();
                    return false;
                }
                current = parentId;
            }
        }

        return true;
    }

    private static bool HasExactlyProperties(
        JsonElement value,
        HashSet<string> expected,
        int expectedCount)
    {
        if (value.ValueKind != JsonValueKind.Object)
        {
            return false;
        }

        var seen = new HashSet<string>(StringComparer.Ordinal);
        var count = 0;
        foreach (var property in value.EnumerateObject())
        {
            count++;
            if (!seen.Add(property.Name) || !expected.Contains(property.Name))
            {
                return false;
            }
        }

        return count == expectedCount;
    }

    private static bool HasAllowedProperties(
        JsonElement value,
        HashSet<string> allowed,
        int minimumCount,
        int maximumCount)
    {
        if (value.ValueKind != JsonValueKind.Object)
        {
            return false;
        }

        var seen = new HashSet<string>(StringComparer.Ordinal);
        var count = 0;
        foreach (var property in value.EnumerateObject())
        {
            count++;
            if (!seen.Add(property.Name) || !allowed.Contains(property.Name))
            {
                return false;
            }
        }

        return count >= minimumCount && count <= maximumCount;
    }

    private static bool TryGetString(JsonElement parent, string name, out string value)
    {
        value = string.Empty;
        if (!parent.TryGetProperty(name, out var property) || property.ValueKind != JsonValueKind.String)
        {
            return false;
        }

        value = property.GetString() ?? string.Empty;
        return true;
    }

    private static bool TryGetBoundedString(
        JsonElement parent,
        string name,
        int minimum,
        int maximum,
        out string value)
    {
        value = string.Empty;
        if (!TryGetString(parent, name, out var candidate) ||
            !IsSafeText(candidate, minimum, maximum))
        {
            return false;
        }

        value = candidate;
        return true;
    }

    private static bool TryGetNullableBoundedString(
        JsonElement parent,
        string name,
        int minimum,
        int maximum,
        out string? value)
    {
        value = null;
        if (!parent.TryGetProperty(name, out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (property.ValueKind != JsonValueKind.String)
        {
            return false;
        }

        var candidate = property.GetString();
        if (candidate is null || !IsSafeText(candidate, minimum, maximum))
        {
            return false;
        }

        value = candidate;
        return true;
    }

    private static bool TryGetNullableRemainingPercent(
        JsonElement parent,
        out double? value)
    {
        return TryGetNullableFiniteDouble(parent, "remaining_percent", 0, 100, out value);
    }

    private static bool TryGetNullableFiniteDouble(
        JsonElement parent,
        string name,
        double minimum,
        double maximum,
        out double? value)
    {
        value = null;
        if (!parent.TryGetProperty(name, out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (property.ValueKind != JsonValueKind.Number ||
            !property.TryGetDouble(out var candidate) ||
            !double.IsFinite(candidate) ||
            candidate < minimum ||
            candidate > maximum)
        {
            return false;
        }

        value = candidate;
        return true;
    }

    private static bool TryGetNonNegativeFiniteDouble(
        JsonElement parent,
        string name,
        out double value)
    {
        value = default;
        if (!parent.TryGetProperty(name, out var property) ||
            property.ValueKind != JsonValueKind.Number ||
            !property.TryGetDouble(out value) ||
            !double.IsFinite(value) ||
            value < 0 ||
            value > 1_000_000_000_000)
        {
            value = default;
            return false;
        }

        return true;
    }

    private static bool TryGetNullableNonNegativeFiniteDouble(
        JsonElement parent,
        string name,
        out double? value)
    {
        value = null;
        if (!parent.TryGetProperty(name, out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (!TryGetNonNegativeFiniteDouble(property, out var candidate))
        {
            return false;
        }

        value = candidate;
        return true;
    }

    private static bool TryGetOptionalNullableNonNegativeFiniteDouble(
        JsonElement parent,
        string name,
        out double? value)
    {
        if (!parent.TryGetProperty(name, out _))
        {
            value = null;
            return true;
        }

        return TryGetNullableNonNegativeFiniteDouble(parent, name, out value);
    }

    private static bool TryGetNonNegativeFiniteDouble(
        JsonElement property,
        out double value)
    {
        value = default;
        return property.ValueKind == JsonValueKind.Number &&
               property.TryGetDouble(out value) &&
               double.IsFinite(value) &&
               value >= 0 &&
               value <= 1_000_000_000_000;
    }

    private static bool TryGetNullableUInt64(JsonElement parent, string name, out ulong? value)
    {
        value = null;
        if (!parent.TryGetProperty(name, out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (!TryGetUInt64(property, out var candidate))
        {
            return false;
        }

        value = candidate;
        return true;
    }

    private static bool TryGetOptionalNullableUInt64(
        JsonElement parent,
        string name,
        out ulong? value)
    {
        if (!parent.TryGetProperty(name, out _))
        {
            value = null;
            return true;
        }

        return TryGetNullableUInt64(parent, name, out value);
    }

    private static bool TryGetDepth(JsonElement parent, out int value)
    {
        value = default;
        if (!TryGetUInt64(parent, "depth", out var candidate) || candidate > 64)
        {
            return false;
        }

        value = (int)candidate;
        return true;
    }

    private static bool TryGetNullableDepth(JsonElement parent, out int? value)
    {
        value = null;
        if (!parent.TryGetProperty("depth", out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (!TryGetUInt64(property, out var candidate) || candidate > 1024)
        {
            return false;
        }

        value = (int)candidate;
        return true;
    }

    private static bool IsSupportedModel(string name) =>
        name is "SOL" or "TERRA" or "LUNA";

    private static bool IsSafeText(string value, int minimum, int maximum)
    {
        if (!HasUnicodeScalarLength(value, minimum, maximum))
        {
            return false;
        }

        foreach (var character in value)
        {
            if (character <= '\u001F' ||
                character is >= '\u007F' and <= '\u009F' ||
                character is '\u2028' or '\u2029' ||
                character is >= '\u202A' and <= '\u202E' ||
                character is >= '\u2066' and <= '\u2069')
            {
                return false;
            }
        }

        return true;
    }

    private static bool TryGetState(JsonElement parent, out ApiState state)
    {
        state = default;
        if (!TryGetString(parent, "state", out var value))
        {
            return false;
        }

        state = value switch
        {
            "initializing" => ApiState.Initializing,
            "ready" => ApiState.Ready,
            "auth_required" => ApiState.AuthRequired,
            "error" => ApiState.Error,
            _ => default,
        };

        return value is "initializing" or "ready" or "auth_required" or "error";
    }

    private static bool TryGetBoolean(JsonElement parent, string name, out bool value)
    {
        value = default;
        if (!parent.TryGetProperty(name, out var property) ||
            (property.ValueKind is not JsonValueKind.True and not JsonValueKind.False))
        {
            return false;
        }

        value = property.GetBoolean();
        return true;
    }

    private static bool TryGetNullableUnixSeconds(
        JsonElement parent,
        string name,
        out long? value)
    {
        value = null;
        if (!parent.TryGetProperty(name, out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (!TryGetUInt64(property, out var unsigned) ||
            unsigned is < 1 or > MaxUnixSeconds)
        {
            return false;
        }

        value = (long)unsigned;
        return true;
    }

    private static bool TryGetNullablePlanLabel(JsonElement parent, out string? value)
    {
        value = null;
        if (!parent.TryGetProperty("plan_label", out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (property.ValueKind != JsonValueKind.String)
        {
            return false;
        }

        var text = property.GetString();
        if (text is null || !IsSafePlanLabel(text))
        {
            return false;
        }

        value = text;
        return true;
    }

    private static bool TryGetQuota(JsonElement parent, out ApiQuota? quota)
    {
        quota = null;
        if (!parent.TryGetProperty("quota", out var property))
        {
            return false;
        }

        if (property.ValueKind == JsonValueKind.Null)
        {
            return true;
        }

        if (!HasExactlyProperties(property, QuotaProperties, 4) ||
            !TryGetFiniteDouble(property, "remaining_percent", out var remainingPercent) ||
            remainingPercent is < 0 or > 100 ||
            !TryGetUnixSeconds(property, "reset_at", out var resetAt) ||
            !TryGetPositiveInt64(property, "window_seconds", out var windowSeconds) ||
            !TryGetBoolean(property, "monthly", out var monthly))
        {
            return false;
        }

        quota = new ApiQuota(remainingPercent, resetAt, windowSeconds, monthly);
        return true;
    }

    private static bool HasValidDetailsRootDomain(
        ApiState state,
        bool authenticated,
        string? planLabel,
        ApiQuota? quota)
    {
        if ((state == ApiState.Ready && !authenticated) ||
            (state == ApiState.AuthRequired && authenticated))
        {
            return false;
        }

        if (planLabel is null)
        {
            return state != ApiState.Ready && quota is null;
        }

        if (!TryGetCanonicalMonthly(planLabel, out var expectedMonthly))
        {
            return false;
        }

        return quota is null || quota.Monthly == expectedMonthly;
    }

    private static bool TryGetCanonicalMonthly(string planLabel, out bool monthly)
    {
        monthly = false;
        switch (planLabel)
        {
            case "無料":
            case "Go":
            case "Plus":
            case "Pro":
            case "Pro Lite":
            case "Team":
            case "Business":
            case "教育":
            case "プラン未設定":
                return true;
            case "エンタープライズ":
                monthly = true;
                return true;
            default:
                return false;
        }
    }

    private static bool TryGetUnixSeconds(JsonElement parent, string name, out long value)
    {
        value = default;
        if (!TryGetUInt64(parent, name, out var unsigned) || unsigned is < 1 or > MaxUnixSeconds)
        {
            return false;
        }

        value = (long)unsigned;
        return true;
    }

    private static bool TryGetPositiveInt64(JsonElement parent, string name, out long value)
    {
        value = default;
        if (!TryGetUInt64(parent, name, out var unsigned) || unsigned is < 1 or > long.MaxValue)
        {
            return false;
        }

        value = (long)unsigned;
        return true;
    }

    private static bool TryGetUInt64(JsonElement parent, string name, out ulong value)
    {
        value = default;
        return parent.TryGetProperty(name, out var property) && TryGetUInt64(property, out value);
    }

    private static bool TryGetUInt64(JsonElement property, out ulong value)
    {
        value = default;
        if (property.ValueKind != JsonValueKind.Number || !IsIntegerLexeme(property.GetRawText()))
        {
            return false;
        }

        return property.TryGetUInt64(out value);
    }

    private static bool TryGetFiniteDouble(JsonElement parent, string name, out double value)
    {
        value = default;
        if (!parent.TryGetProperty(name, out var property) || property.ValueKind != JsonValueKind.Number)
        {
            return false;
        }

        return property.TryGetDouble(out value) && double.IsFinite(value);
    }

    private static bool IsIntegerLexeme(string raw)
    {
        if (raw.Length == 0)
        {
            return false;
        }

        foreach (var character in raw)
        {
            if (character is < '0' or > '9')
            {
                return false;
            }
        }

        return true;
    }

    private static bool HasUnicodeScalarLength(string value, int minimum, int maximum)
    {
        var scalarCount = 0;
        for (var index = 0; index < value.Length; index++)
        {
            var character = value[index];
            if (char.IsHighSurrogate(character))
            {
                if (index + 1 >= value.Length || !char.IsLowSurrogate(value[index + 1]))
                {
                    return false;
                }

                index++;
            }
            else if (char.IsLowSurrogate(character))
            {
                return false;
            }

            scalarCount++;
            if (scalarCount > maximum)
            {
                return false;
            }
        }

        return scalarCount >= minimum;
    }

    private static bool IsSafePlanLabel(string value)
    {
        if (!HasUnicodeScalarLength(value, 1, 64))
        {
            return false;
        }

        foreach (var character in value)
        {
            if (character <= '\u001F' ||
                character is >= '\u007F' and <= '\u009F' ||
                character is '\u2028' or '\u2029' ||
                character is >= '\u202A' and <= '\u202E' ||
                character is >= '\u2066' and <= '\u2069')
            {
                return false;
            }
        }

        return true;
    }

    private enum BodyReadKind
    {
        Success,
        Oversize,
        Transport,
    }

    private readonly record struct DetailsAttemptResult(
        DetailsFetchResult Result,
        bool NotFound,
        bool NotModified)
    {
        public static DetailsAttemptResult Success(ApiDetailsSnapshot snapshot) =>
            new(DetailsFetchResult.Success(snapshot), false, false);

        public static DetailsAttemptResult Failure(DetailsFetchFailure failure) =>
            new(DetailsFetchResult.FromFailure(failure), false, false);

        public static DetailsAttemptResult NotFoundResult() =>
            new(DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport), true, false);

        public static DetailsAttemptResult NotModifiedResult(ApiDetailsSnapshot expectedSnapshot) =>
            new(DetailsFetchResult.Success(expectedSnapshot), false, true);
    }

    private sealed record V3ModelCost(
        string PriceVersion,
        double OrdinaryInputDollars,
        double CachedInputDollars,
        double CacheWriteInputDollars,
        double OutputDollars,
        double TotalDollars);

    private readonly record struct BodyReadResult(BodyReadKind Kind, byte[]? Body)
    {
        public static BodyReadResult Success(byte[] body) => new(BodyReadKind.Success, body);

        public static BodyReadResult Oversize() => new(BodyReadKind.Oversize, null);

        public static BodyReadResult Transport() => new(BodyReadKind.Transport, null);
    }
}
