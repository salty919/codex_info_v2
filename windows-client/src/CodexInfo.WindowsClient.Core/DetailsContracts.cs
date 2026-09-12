// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Core;

/// <summary>A failure reading the independent details resource.</summary>
public enum DetailsFetchFailure
{
    Transport,
    Response,
}

/// <summary>
/// The immutable, whitelisted data returned by GET /v1/details. Core values,
/// history, models, and threads form one atomic visible generation.
/// </summary>
public sealed record ApiDetailsSnapshot(
    ApiState State,
    long? ObservedAt,
    bool Authenticated,
    string? PlanLabel,
    ApiQuota? Quota,
    IReadOnlyList<ApiDetailsModelUsage> Models,
    ulong ActiveThreadCount,
    IReadOnlyList<ApiHistoryPeriod> HistoryPeriods,
    IReadOnlyList<ApiHistorySample> HistorySamples,
    IReadOnlyList<ApiThreadDetails> Threads,
    string EstimatedCostLabel)
{
    /// <summary>The wire contract version of this accepted details document.</summary>
    public string ApiVersion { get; init; } = "v1";

    /// <summary>The opaque identity of the accepted details response pair.</summary>
    public PublishedPairIdentity? PublishedPair { get; init; }

    /// <summary>Explicit account used for this resource, when scoped.</summary>
    public string? AccountId { get; init; }

    public IReadOnlyList<ApiHistoryPeriod> History => HistoryPeriods;

    /// <summary>Confirmed, redacted recorder gaps for the history periods.</summary>
    public IReadOnlyList<ApiHistoryGap> HistoryGaps { get; init; } = [];

    /// <summary>Compatibility view for callers that provide bundled notices.</summary>
    public IReadOnlyList<ApiLegalNotice> LegalNotices { get; init; } = [];

    public ApiDetailsSnapshot(
        IReadOnlyList<ApiDetailsModelUsage> models,
        IReadOnlyList<ApiHistoryPeriod> historyPeriods,
        IReadOnlyList<ApiThreadDetails> threads,
        IReadOnlyList<ApiLegalNotice> legalNotices)
        : this(
            ApiState.Ready,
            null,
            true,
            null,
            null,
            models,
            0,
            historyPeriods,
            Array.Empty<ApiHistorySample>(),
            threads,
            "未取得")
    {
        LegalNotices = legalNotices;
    }
}

/// <summary>Token and expected-dollar totals for one model in the current quota period.</summary>
public sealed record ApiDetailsModelUsage(
    string Name,
    ulong InputTokens,
    ulong CachedInputTokens,
    ulong OutputTokens,
    double InputDollars,
    double CachedInputDollars,
    double OutputDollars)
{
    /// <summary>The server-provided cumulative token total when using v3.</summary>
    public ulong TotalTokens { get; init; } = AddTokens(InputTokens, CachedInputTokens, OutputTokens);

    /// <summary>v3 cache-write input tokens, which are absent from v1/v2.</summary>
    public ulong? CacheWriteInputTokens { get; init; }

    /// <summary>v3 cache-write input dollars, which are absent from v1/v2.</summary>
    public double CacheWriteInputDollars { get; init; } = double.NaN;

    /// <summary>Opaque price-table identity supplied by the v3 server.</summary>
    public string? PriceVersion { get; init; }

    /// <summary>
    /// The v3 server-provided total dollar value. A null value means the
    /// server had no validated price for this model; it is never inferred by
    /// the Windows client.
    /// </summary>
    public double? EstimatedTotalDollars { get; init; }

    public bool HasEstimatedCost => EstimatedTotalDollars is { } value && double.IsFinite(value);

    /// <summary>
    /// Keeps the compatibility behavior for v1/v2 while preferring the exact
    /// v3 total when one was published.
    /// </summary>
    public double TotalDollars => EstimatedTotalDollars ??
        (double.IsFinite(InputDollars + CachedInputDollars + OutputDollars)
            ? InputDollars + CachedInputDollars + OutputDollars +
              (double.IsFinite(CacheWriteInputDollars) ? CacheWriteInputDollars : 0)
            : double.NaN);

    private static ulong AddTokens(ulong input, ulong cached, ulong output) =>
        SaturatingAdd(SaturatingAdd(input, cached), output);

    private static ulong SaturatingAdd(ulong left, ulong right) =>
        ulong.MaxValue - left < right ? ulong.MaxValue : left + right;
}

/// <summary>A persisted quota period shown by the history/graph view.</summary>
public sealed record ApiHistoryPeriod(
    string Id,
    long StartAt,
    long EndAt,
    bool Current,
    string Label)
{
    // `id` remains opaque. Historical `end_at` can be clipped by the start of
    // a newer period, so sample ownership uses the explicit canonical reset
    // boundary from the wire contract instead of parsing the ID or guessing
    // from the graph end.
    public long ResetAt { get; init; } = EndAt;

    public long WindowSeconds => Math.Max(1, EndAt - StartAt);

    public bool Monthly => WindowSeconds > TimeSpan.FromDays(7).TotalSeconds + 86_400;

    public IReadOnlyList<ApiHistorySample> Samples { get; init; } = [];
}

/// <summary>A confirmed, redacted gap in one canonical history period.</summary>
public sealed record ApiHistoryGap(
    string GapId,
    long ResetAt,
    long StartAt,
    long EndAt,
    string Reason);

/// <summary>A single validated history point.</summary>
public sealed record ApiHistorySample(
    long Timestamp,
    long ResetAt,
    double? RemainingPercent,
    double? SolDollars,
    double? TerraDollars,
    double? LunaDollars,
    ulong? SolTokens,
    ulong? TerraTokens,
    ulong? LunaTokens,
    string ModelSource = "confirmed")
{
    public const string ConfirmedModelSource = "confirmed";
    public const string ReconstructedFromSessionModelSource = "reconstructed-from-session";
    public const string UnavailableModelSource = "unavailable";
    public const string LegacyUnknownModelSource = "legacy-unknown";

    /// <summary>
    /// True only when the v3 producer established the complete model set for
    /// this observation. It is false for the legacy fixed-column projection;
    /// session reconstruction can carry either complete or partial model sets.
    /// </summary>
    public bool ModelsComplete { get; init; } = true;

    /// <summary>
    /// Whether the task was active since the preceding history sample. A null
    /// value means that the producer did not establish the activity state;
    /// consumers must not infer an idle interval from it.
    /// </summary>
    public bool? TaskActiveSincePrevious { get; init; }

    /// <summary>
    /// Presentation-only bounded hold added at a period edge. Wire parsers
    /// never set this flag and the value is never persisted or republished.
    /// </summary>
    public bool IsSyntheticTail { get; init; }

    /// <summary>
    /// Generic v3 model rows. Null is distinct from an empty, complete list:
    /// null means that the producer did not publish model values at all.
    /// </summary>
    public IReadOnlyList<ApiHistoryModelSample>? ModelSamples { get; init; }

    public IReadOnlyList<ApiHistoryModelSample> Models => ModelSamples ??
    [
        // Legacy history columns only prove cumulative totals. Do not expose
        // unknown input/cache/output components as zero-valued facts.
        new ApiHistoryModelSample("SOL", null, null, null, SolDollars) { TotalTokens = SolTokens },
        new ApiHistoryModelSample("TERRA", null, null, null, TerraDollars) { TotalTokens = TerraTokens },
        new ApiHistoryModelSample("LUNA", null, null, null, LunaDollars) { TotalTokens = LunaTokens },
    ];
}

/// <summary>One model's cumulative values at a history point.</summary>
public sealed record ApiHistoryModelSample(
    string Name,
    ulong? InputTokens,
    ulong? CachedInputTokens,
    ulong? OutputTokens,
    double? Dollars)
{
    /// <summary>v3 cache-write input tokens, if the producer observed them.</summary>
    public ulong? CacheWriteInputTokens { get; init; }

    /// <summary>Exact cumulative token total from the history row.</summary>
    public ulong? TotalTokens { get; init; }

    /// <summary>Alias for the v3 total dollar field.</summary>
    public double? TotalDollars => Dollars;
}

/// <summary>A currently running thread and its validated tree metadata.</summary>
public sealed record ApiThreadDetails(
    string Id,
    string Title,
    string? ParentId,
    string Model,
    string ModelLabel,
    ulong? CumulativeTokens,
    ulong? ContextTokens,
    ulong? ContextLimit,
    long? CreatedAt,
    long? LastUserMessageAt,
    bool IsSubAgent,
    int? Depth,
    bool IsOrphan)
{
    public double? ContextPercent => ContextTokens is { } used && ContextLimit is { } limit && limit > 0
        ? Math.Clamp(used * 100.0 / limit, 0, 100)
        : null;
}

/// <summary>
/// An account choice exposed by the v3 account selector.  The request id is
/// the public account-N key; the optional login id is display metadata and is
/// never used to scope a resource request.
/// </summary>
public sealed record ApiAccount(
    string Id,
    bool IsCurrent,
    long? ActivationAt,
    long? DeactivationAt,
    string? LoginId = null)
{
    /// <summary>Localized state marker; it never contains a lifecycle time.</summary>
    public string? DisplayStatusSuffix { get; init; }

    /// <summary>
    /// Public id suffix used only when two login-id labels are ambiguous. It
    /// never contains an email, hash, or filesystem path.
    /// </summary>
    public string? DisplayLabelSuffix { get; init; }

    /// <summary>
    /// Selector text derived only from the validated optional login id. A
    /// missing login id is rendered as the public account number. Lifecycle
    /// boundaries, tokens, hashes, and filesystem paths are never included.
    /// </summary>
    public string DisplayLabel
    {
        get
        {
            var disambiguator = string.IsNullOrEmpty(DisplayLabelSuffix)
                ? string.Empty
                : $" · {DisplayLabelSuffix}";
            var identity = LoginId is { Length: > 0 } loginId
                ? loginId
                : $"アカウント {GetAccountNumber()} · ID未復元";
            return $"{identity}{DisplayStatusSuffix}{disambiguator}";
        }
    }

    private string GetAccountNumber() => Id.StartsWith("account-", StringComparison.Ordinal)
        ? Id["account-".Length..]
        : Id;

    /// <summary>
    /// Keeps ordinary labels compact while making duplicate login-id labels
    /// deterministic and unambiguous for keyboard/UI automation selection.
    /// </summary>
    public static IReadOnlyList<ApiAccount> EnsureUniqueDisplayLabels(
        IEnumerable<ApiAccount> source)
    {
        ArgumentNullException.ThrowIfNull(source);
        var accounts = source.ToArray();
        var duplicateIds = accounts
            .GroupBy(account => account.DisplayLabel, StringComparer.Ordinal)
            .Where(group => group.Count() > 1)
            .SelectMany(group => group)
            .Select(account => account.Id)
            .ToHashSet(StringComparer.Ordinal);
        return accounts
            .Select(account => duplicateIds.Contains(account.Id)
                ? account with { DisplayLabelSuffix = account.Id }
                : account)
            .ToArray();
    }
}

/// <summary>Static legal/licensing text safe to render in the client.</summary>
public sealed record ApiLegalNotice(string Name, string Text);

/// <summary>A result for details, with no response body or exception details.</summary>
public sealed record DetailsFetchResult(
    ApiDetailsSnapshot? Snapshot,
    DetailsFetchFailure? Failure)
{
    public bool IsSuccess => Snapshot is not null && Failure is null;

    public static DetailsFetchResult Success(ApiDetailsSnapshot snapshot)
    {
        ArgumentNullException.ThrowIfNull(snapshot);
        return new DetailsFetchResult(snapshot, null);
    }

    public static DetailsFetchResult FromFailure(DetailsFetchFailure failure) =>
        new(null, failure);
}

/// <summary>The independent details endpoint used by the auxiliary windows.</summary>
public interface ILoopbackDetailsClient
{
    Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default);
}

/// <summary>
/// The bounded v3 resources used by the current Windows client.  Each
/// resource carries the server's opaque published-pair identity so callers
/// can stage a view and commit it only when all of that view's pages agree.
/// </summary>
public interface ILoopbackResourceClient
{
    Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default);

    Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
        CancellationToken cancellationToken = default);

    Task<HistoryPageFetchResult> FetchHistoryPageAsync(
        string periodId,
        string? cursor = null,
        CancellationToken cancellationToken = default);

    Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default);
}

/// <summary>Reads the redacted account choices for the selector.</summary>
public interface ILoopbackAccountsClient
{
    Task<AccountsFetchResult> FetchAccountsAsync(CancellationToken cancellationToken = default);
}

/// <summary>
/// Account-scoped v3 resources. The account key is explicit at the transport
/// boundary so a previous account cannot be silently reused by a child view.
/// </summary>
public interface ILoopbackAccountResourceClient
{
    Task<CurrentFetchResult> FetchCurrentAsync(
        string accountId,
        CancellationToken cancellationToken = default);

    Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
        string accountId,
        CancellationToken cancellationToken = default);

    Task<HistoryPageFetchResult> FetchHistoryPageAsync(
        string accountId,
        string periodId,
        string? cursor = null,
        CancellationToken cancellationToken = default);

    Task<ThreadsFetchResult> FetchThreadsAsync(
        string accountId,
        CancellationToken cancellationToken = default);
}

/// <summary>Validated account selector response from /v3/accounts.</summary>
public sealed record ApiAccountsSnapshot(
    string DefaultAccountId,
    IReadOnlyList<ApiAccount> Accounts)
{
    public string ApiVersion { get; init; } = "v3";
}

/// <summary>The v3/current resource. History and thread rows are intentionally absent.</summary>
public sealed record ApiCurrentSnapshot(
    ApiState State,
    long? ObservedAt,
    bool Authenticated,
    string? PlanLabel,
    ApiQuota? Quota,
    IReadOnlyList<ApiDetailsModelUsage> Models,
    ulong ActiveThreadCount,
    PublishedPairIdentity PublishedPair)
{
    public string ApiVersion { get; init; } = "v3";

    /// <summary>Explicit account used for this resource, when scoped.</summary>
    public string? AccountId { get; init; }

    internal static ApiCurrentSnapshot FromDetails(ApiDetailsSnapshot details)
    {
        ArgumentNullException.ThrowIfNull(details);
        return new ApiCurrentSnapshot(
            details.State,
            details.ObservedAt,
            details.Authenticated,
            details.PlanLabel,
            details.Quota,
            details.Models,
            details.ActiveThreadCount,
            details.PublishedPair ?? default)
        {
            ApiVersion = details.ApiVersion,
        };
    }
}

/// <summary>Validated period metadata returned by v3/history/periods.</summary>
public sealed record ApiHistoryPeriodsSnapshot(
    IReadOnlyList<ApiHistoryPeriod> Periods,
    PublishedPairIdentity PublishedPair)
{
    public string ApiVersion { get; init; } = "v3";

    public string? AccountId { get; init; }

    internal static ApiHistoryPeriodsSnapshot FromDetails(ApiDetailsSnapshot details) =>
        new(details.HistoryPeriods, details.PublishedPair ?? default)
        {
            ApiVersion = details.ApiVersion,
        };
}

/// <summary>One bounded history page and its opaque continuation cursor.</summary>
public sealed record ApiHistoryPage(
    string PeriodId,
    IReadOnlyList<ApiHistorySample> Samples,
    IReadOnlyList<ApiHistoryGap> HistoryGaps,
    string? NextCursor,
    string? ResumeCursor,
    PublishedPairIdentity PublishedPair)
{
    public string ApiVersion { get; init; } = "v3";

    public string? AccountId { get; init; }

    public bool IsLegacyFallback { get; init; }

    internal static ApiHistoryPage? FromDetails(ApiDetailsSnapshot details, string periodId)
    {
        var period = details.HistoryPeriods.FirstOrDefault(candidate => candidate.Id == periodId);
        return period is null
            ? null
            : new ApiHistoryPage(
                period.Id,
                period.Samples,
                details.HistoryGaps
                    .Where(gap => gap.ResetAt == period.ResetAt)
                    .ToArray(),
                null,
                "legacy",
                details.PublishedPair ?? default)
            {
                ApiVersion = details.ApiVersion,
                IsLegacyFallback = true,
            };
    }
}

/// <summary>Validated thread rows returned by v3/threads.</summary>
public sealed record ApiThreadsSnapshot(
    IReadOnlyList<ApiThreadDetails> Threads,
    PublishedPairIdentity PublishedPair)
{
    public string ApiVersion { get; init; } = "v3";

    public string? AccountId { get; init; }

    internal static ApiThreadsSnapshot FromDetails(ApiDetailsSnapshot details) =>
        new(details.Threads, details.PublishedPair ?? default)
        {
            ApiVersion = details.ApiVersion,
        };
}

public sealed record CurrentFetchResult(
    ApiCurrentSnapshot? Snapshot,
    DetailsFetchFailure? Failure)
{
    public bool IsSuccess => Snapshot is not null && Failure is null;

    public static CurrentFetchResult Success(ApiCurrentSnapshot snapshot)
    {
        ArgumentNullException.ThrowIfNull(snapshot);
        return new CurrentFetchResult(snapshot, null);
    }

    public static CurrentFetchResult FromFailure(DetailsFetchFailure failure) =>
        new(null, failure);
}

public sealed record HistoryPeriodsFetchResult(
    ApiHistoryPeriodsSnapshot? Snapshot,
    DetailsFetchFailure? Failure)
{
    public bool IsSuccess => Snapshot is not null && Failure is null;

    public static HistoryPeriodsFetchResult Success(ApiHistoryPeriodsSnapshot snapshot)
    {
        ArgumentNullException.ThrowIfNull(snapshot);
        return new HistoryPeriodsFetchResult(snapshot, null);
    }

    public static HistoryPeriodsFetchResult FromFailure(DetailsFetchFailure failure) =>
        new(null, failure);
}

public sealed record HistoryPageFetchResult(
    ApiHistoryPage? Page,
    DetailsFetchFailure? Failure,
    bool CursorRejected)
{
    public bool IsSuccess => Page is not null && Failure is null && !CursorRejected;

    public static HistoryPageFetchResult Success(ApiHistoryPage page)
    {
        ArgumentNullException.ThrowIfNull(page);
        return new HistoryPageFetchResult(page, null, false);
    }

    public static HistoryPageFetchResult FromFailure(DetailsFetchFailure failure) =>
        new(null, failure, false);

    public static HistoryPageFetchResult FromRejectedCursor() =>
        new(null, DetailsFetchFailure.Response, true);
}

public sealed record ThreadsFetchResult(
    ApiThreadsSnapshot? Snapshot,
    DetailsFetchFailure? Failure)
{
    public bool IsSuccess => Snapshot is not null && Failure is null;

    public static ThreadsFetchResult Success(ApiThreadsSnapshot snapshot)
    {
        ArgumentNullException.ThrowIfNull(snapshot);
        return new ThreadsFetchResult(snapshot, null);
    }

    public static ThreadsFetchResult FromFailure(DetailsFetchFailure failure) =>
        new(null, failure);
}

public sealed record AccountsFetchResult(
    ApiAccountsSnapshot? Snapshot,
    DetailsFetchFailure? Failure)
{
    public bool IsSuccess => Snapshot is not null && Failure is null;

    public static AccountsFetchResult Success(ApiAccountsSnapshot snapshot)
    {
        ArgumentNullException.ThrowIfNull(snapshot);
        return new AccountsFetchResult(snapshot, null);
    }

    public static AccountsFetchResult FromFailure(DetailsFetchFailure failure) =>
        new(null, failure);
}
