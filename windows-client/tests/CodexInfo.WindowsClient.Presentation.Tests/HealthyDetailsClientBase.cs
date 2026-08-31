// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;

namespace CodexInfo.WindowsClient.Presentation.Tests;

/// <summary>
/// Healthy details client base for presentation fixtures. Each fixture supplies
/// a complete details generation directly, matching the production authority.
/// </summary>
internal abstract class HealthyDetailsClientBase : ILoopbackDetailsClient, ILoopbackHealthClient
{
    public virtual Task<HealthFetchResult> FetchHealthAsync(
        CancellationToken cancellationToken = default) =>
        Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "codex-info")));

    public Task<DetailsFetchResult> FetchDetailsAsync(
        CancellationToken cancellationToken = default) =>
        FetchDetailsFixtureAsync(cancellationToken);

    protected abstract Task<DetailsFetchResult> FetchDetailsFixtureAsync(
        CancellationToken cancellationToken = default);
}
