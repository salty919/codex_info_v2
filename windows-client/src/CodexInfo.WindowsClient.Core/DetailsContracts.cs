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
    /// <summary>The opaque identity of the accepted details response pair.</summary>
    public PublishedPairIdentity? PublishedPair { get; init; }

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
    public double TotalDollars => InputDollars + CachedInputDollars + OutputDollars;
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
    double SolDollars,
    double TerraDollars,
    double LunaDollars,
    ulong SolTokens,
    ulong TerraTokens,
    ulong LunaTokens)
{
    public IReadOnlyList<ApiHistoryModelSample> Models =>
    [
        new ApiHistoryModelSample("SOL", SolTokens, 0, 0, SolDollars),
        new ApiHistoryModelSample("TERRA", TerraTokens, 0, 0, TerraDollars),
        new ApiHistoryModelSample("LUNA", LunaTokens, 0, 0, LunaDollars),
    ];
}

/// <summary>One model's cumulative values at a history point.</summary>
public sealed record ApiHistoryModelSample(
    string Name,
    ulong InputTokens,
    ulong CachedInputTokens,
    ulong OutputTokens,
    double Dollars);

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
