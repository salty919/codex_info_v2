// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Updates;

/// <summary>Shared Windows update discovery, ownership, and install boundary.</summary>
public interface IWindowsUpdateCoordinator : IDisposable
{
    Task<UpdateCheckResult> CheckAsync(CancellationToken cancellationToken = default);

    Task<UpdateStartStatus> StartAvailableUpdateAsync(CancellationToken cancellationToken = default);
}

public sealed record UpdateCheckResult(string? AvailableVersion, bool IsFailure);

/// <summary>Finite result returned by the headless update-only entry point.</summary>
public enum UpdateOnlyExitCode
{
    Current = 0,
    SetupStarted = 10,
    Busy = 20,
    DiscoveryFailure = 21,
    DownloadOrIntegrityFailure = 22,
    LaunchFailure = 23,
}

public enum UpdateStartStatus
{
    Started,
    Busy,
    NoAvailableUpdate,
    DiscoveryFailed,
    DownloadFailed,
    IntegrityFailed,
    LaunchFailed,
    OldVersionFailed,
    SafeBlocked,
}
