// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Core;

/// <summary>The account-independent GET /v1/runtime process observations.</summary>
public sealed record ApiRuntimeVersionsSnapshot(
    string ApiVersion, string RestVersion, string? RecorderVersion, string RecorderStatus);

public sealed record RuntimeVersionsFetchResult(ApiRuntimeVersionsSnapshot? Snapshot, HealthFetchFailure? Failure)
{
    public bool IsSuccess => Snapshot is not null && Failure is null;
}

public interface ILoopbackRuntimeVersionsClient
{
    Task<RuntimeVersionsFetchResult> FetchRuntimeVersionsAsync(CancellationToken cancellationToken = default);
}
