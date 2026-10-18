// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Core;

public enum ApiRecordedCoverage { Complete, Partial, Unknown }

public sealed record ApiRecordedUsageScope(long StartAt, long EndAt, ApiRecordedCoverage Coverage);

/// <summary>Qualified facts in an explicit recording range. Unknown components stay null.</summary>
public sealed record ApiRecordedModelUsage(string Name, ulong TotalTokens, ulong? InputTokens,
    ulong? CachedInputTokens, ulong? CacheWriteInputTokens, ulong? OutputTokens, ApiRecordedCost? Cost);

public sealed record ApiRecordedCost(string PriceVersion, double OrdinaryInputDollars,
    double CachedInputDollars, double CacheWriteInputDollars, double OutputDollars, double TotalDollars);

public sealed record ApiRecordedScope(string Id, string Kind, long StartAt, long EndAt, bool Current,
    bool RecordingsAvailable, bool SavedObservationsAvailable, ApiRecordedCoverage Coverage);

public sealed record ApiRecordedSample(long Timestamp, IReadOnlyList<ApiRecordedModelUsage> Models);
public sealed record ApiRecordedCoverageInterval(long StartAt, long EndAt, ApiRecordedCoverage Coverage);
public sealed record ApiRecordedQuotaObservation(ulong Sequence, long ObservedAt, long ResetAt,
    long WindowSeconds, double RemainingPercent);
public sealed record ApiSavedModelObservation(string StoredModelKey, ulong? TotalTokens, ulong? InputTokens,
    ulong? CachedInputTokens, ulong? CacheWriteInputTokens, ulong? OutputTokens, double? TotalDollars);
public sealed record ApiSavedObservation(string ObservationId, long Timestamp, long? RawResetAt, string StorageKind,
    IReadOnlyList<ApiSavedModelObservation> Models, IReadOnlyList<string> UnresolvedReasons);

/// <summary>Only a complete same-account, same-pair bundle can become visible.</summary>
public sealed record ApiRecordedGraphBundle(string? AccountId, PublishedPairIdentity PublishedPair,
    IReadOnlyList<ApiRecordedScope> Scopes, ApiRecordedScope? SelectedScope, ApiRecordedUsageScope? Scope,
    IReadOnlyList<ApiRecordedCoverageInterval> CoverageIntervals, IReadOnlyList<ApiRecordedSample> Samples,
    IReadOnlyList<ApiRecordedQuotaObservation> QuotaObservations, IReadOnlyList<ApiSavedObservation> Observations);

public sealed record RecordedGraphFetchResult(ApiRecordedGraphBundle? Bundle, DetailsFetchFailure? Failure,
    bool LegacyNotFound = false)
{
    public bool IsSuccess => Bundle is not null && Failure is null;
}

public interface ILoopbackRecordedScopeClient
{
    Task<RecordedGraphFetchResult> FetchRecordedGraphAsync(string? accountId, string? scopeId = null,
        long? startAt = null, long? endAt = null, CancellationToken cancellationToken = default);
}
