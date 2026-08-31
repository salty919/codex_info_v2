// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Core;

/// <summary>The fixed listener-health document returned by GET /v1/health.</summary>
public sealed record ApiHealthSnapshot(string ApiVersion, string Service, string ProductVersion);

/// <summary>A health result that never exposes response text or exceptions.</summary>
public sealed record HealthFetchResult(
    ApiHealthSnapshot? Snapshot,
    HealthFetchFailure? Failure)
{
    public bool IsSuccess => Snapshot is not null && Failure is null;

    public static HealthFetchResult Success(ApiHealthSnapshot snapshot)
    {
        ArgumentNullException.ThrowIfNull(snapshot);
        return new HealthFetchResult(snapshot, null);
    }

    public static HealthFetchResult FromFailure(HealthFetchFailure failure) =>
        new(null, failure);
}

/// <summary>The deliberately small health failure surface.</summary>
public enum HealthFetchFailure
{
    Transport,
    Response,
}

/// <summary>Reads the fixed loopback health endpoint before a client cycle.</summary>
public interface ILoopbackHealthClient
{
    Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default);
}
