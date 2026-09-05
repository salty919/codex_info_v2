// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Serialization;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Updates;

namespace CodexInfo.WindowsClient.Infrastructure;

/// <summary>Identity recorded for a Setup process that owns an update attempt.</summary>
public sealed record WindowsInstallerProcessIdentity(
    int ProcessId,
    DateTimeOffset StartTimeUtc,
    string ExecutablePath)
{
    public int Pid => ProcessId;

    public DateTimeOffset ProcessStartTimeUtc => StartTimeUtc;

    public string Path => ExecutablePath;
}

/// <summary>
/// Result of launching Setup. A started process without a complete identity is
/// deliberately distinguishable from a process creation failure: the former
/// is SAFE_BLOCKED because another Setup must never be layered on top of it.
/// </summary>
public sealed record WindowsInstallerLaunchResult(
    bool Started,
    WindowsInstallerProcessIdentity? Identity)
{
    public static WindowsInstallerLaunchResult Failed() => new(false, null);

    public static WindowsInstallerLaunchResult StartedWith(
        WindowsInstallerProcessIdentity identity) => new(true, identity);
}

/// <summary>Optional process-identity seam used by the production launcher and tests.</summary>
public interface IWindowsInstallerProcessLauncher
{
    WindowsInstallerLaunchResult TryLaunchWithIdentity(string installerPath);
}

/// <summary>Starts the verified per-user Setup transaction.</summary>
public interface IWindowsInstallerLauncher
{
    bool TryLaunch(string installerPath);
}

/// <summary>
/// Starts Setup without elevation and captures its exact process identity.
/// Inno Setup's per-user installer owns the payload transaction; the client
/// only downloads verified bytes and starts that transaction once.
/// </summary>
public sealed class WindowsInstallerLauncher :
    IWindowsInstallerLauncher,
    IWindowsInstallerProcessLauncher
{
    private const string SetupArguments =
        "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS";

    public bool TryLaunch(string installerPath) => TryLaunchWithIdentity(installerPath).Started;

    public WindowsInstallerLaunchResult TryLaunchWithIdentity(string installerPath)
    {
        if (string.IsNullOrWhiteSpace(installerPath) ||
            !Path.IsPathFullyQualified(installerPath) ||
            !string.Equals(Path.GetExtension(installerPath), ".exe", StringComparison.OrdinalIgnoreCase))
        {
            return WindowsInstallerLaunchResult.Failed();
        }

        try
        {
            var file = new FileInfo(Path.GetFullPath(installerPath));
            if (WindowsPathSafety.ContainsReparsePoint(file.FullName) ||
                !WindowsPathSafety.IsMissingOrRegularFile(file.FullName) ||
                !file.Exists || file.Length <= 0)
            {
                return WindowsInstallerLaunchResult.Failed();
            }

            using var process = Process.Start(new ProcessStartInfo
            {
                FileName = file.FullName,
                Arguments = SetupArguments,
                WorkingDirectory = file.DirectoryName!,
                UseShellExecute = false,
                CreateNoWindow = true,
            });
            if (process is null)
            {
                return WindowsInstallerLaunchResult.Failed();
            }

            // Process.Start does not guarantee that all identity properties
            // are available to a non-admin caller. A started process with a
            // missing field is returned as SAFE_BLOCKED by the coordinator.
            try
            {
                var startTime = process.StartTime.ToUniversalTime();
                var executablePath = process.MainModule?.FileName;
                if (startTime == default || string.IsNullOrWhiteSpace(executablePath))
                {
                    return new WindowsInstallerLaunchResult(true, null);
                }

                return WindowsInstallerLaunchResult.StartedWith(
                    new WindowsInstallerProcessIdentity(
                        process.Id,
                        new DateTimeOffset(startTime, TimeSpan.Zero),
                        Path.GetFullPath(executablePath)));
            }
            catch
            {
                return new WindowsInstallerLaunchResult(true, null);
            }
        }
        catch
        {
            return WindowsInstallerLaunchResult.Failed();
        }
    }
}

/// <summary>
/// A non-secret, atomically persisted logical owner for one Setup attempt.
/// The state remains after the OS lease is released so a later trigger can
/// read back the installed generation before reporting success or retrying.
/// </summary>
public sealed record WindowsUpdatePendingState(
    [property: JsonPropertyName("attempt_id")] string AttemptId,
    [property: JsonPropertyName("target_version")] string TargetVersion,
    [property: JsonPropertyName("target_source")] string TargetSource,
    [property: JsonPropertyName("installer_sha256")] string InstallerSha256,
    [property: JsonPropertyName("installer_size")] long InstallerSize,
    [property: JsonPropertyName("installer_path")] string InstallerPath,
    [property: JsonPropertyName("attempt_started_at_utc")] DateTimeOffset AttemptStartedAtUtc,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("process_id")] int? ProcessId = null,
    [property: JsonPropertyName("process_start_time_utc")] DateTimeOffset? ProcessStartTimeUtc = null,
    [property: JsonPropertyName("process_path")] string? ProcessPath = null,
    [property: JsonPropertyName("failure")] string? Failure = null,
    [property: JsonPropertyName("completed_at_utc")] DateTimeOffset? CompletedAtUtc = null);

/// <summary>
/// Shared update state machine for UI startup, retry, and --update-only.
/// </summary>
public sealed class WindowsUpdateCoordinator : IWindowsUpdateCoordinator
{
    private const string InstallerName = "CodexInfo.WindowsClient.Setup.exe";
    private const string LeaseFileName = ".update.lease";
    private const string PendingStateFileName = ".update.pending.json";
    private const string PendingStateTempSuffix = ".tmp";
    private const string PendingStatus = "pending";
    private const string FailedStatus = "failed";
    private const string SafeBlockedStatus = "safe_blocked";
    private const string SuccessStatus = "success";
    private static readonly TimeSpan TriggerDeadline = TimeSpan.FromMinutes(10);

    private static readonly JsonSerializerOptions PendingJsonOptions = new()
    {
        PropertyNamingPolicy = null,
        WriteIndented = false,
    };

    private readonly IWindowsUpdateClient client;
    private readonly IWindowsInstallerLauncher launcher;
    private readonly Version currentVersion;
    private readonly string updateRoot;
    private WindowsUpdateRelease? availableRelease;
    private int startInProgress;
    private int disposed;

    public WindowsUpdateCoordinator(
        IWindowsUpdateClient client,
        IWindowsInstallerLauncher launcher,
        Version currentVersion,
        string updateRoot)
    {
        this.client = client ?? throw new ArgumentNullException(nameof(client));
        this.launcher = launcher ?? throw new ArgumentNullException(nameof(launcher));
        this.currentVersion = currentVersion ?? throw new ArgumentNullException(nameof(currentVersion));
        if (string.IsNullOrWhiteSpace(updateRoot) || !Path.IsPathFullyQualified(updateRoot))
        {
            throw new ArgumentException("The update root must be an absolute path.", nameof(updateRoot));
        }

        this.updateRoot = Path.GetFullPath(updateRoot);
    }

    public static WindowsUpdateCoordinator CreateDefault()
    {
        var current = typeof(WindowsUpdateCoordinator).Assembly.GetName().Version
            ?? new Version(1, 0, 0);
        var root = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "CodexInfo",
            "updates");
        return new WindowsUpdateCoordinator(
            new WindowsUpdateClient(),
            new WindowsInstallerLauncher(),
            current,
            root);
    }

    public async Task<UpdateCheckResult> CheckAsync(CancellationToken cancellationToken = default)
    {
        if (Volatile.Read(ref disposed) != 0)
        {
            return new UpdateCheckResult(null, true);
        }

        availableRelease = null;
        ClosePendingWhenCurrentGenerationIsInstalled();
        try
        {
            var result = await client.CheckAsync(currentVersion, cancellationToken)
                .ConfigureAwait(false);
            if (!result.IsSuccess)
            {
                return new UpdateCheckResult(null, true);
            }

            availableRelease = result.Release;
            return new UpdateCheckResult(
                result.Release is null ? null : FormatVersion(result.Release.Version),
                false);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            return new UpdateCheckResult(null, true);
        }
        catch
        {
            return new UpdateCheckResult(null, true);
        }
    }

    /// <summary>
    /// Executes the same check/start coordinator used by the UI, but returns
    /// the finite headless contract required by scheduled update-only tasks.
    /// </summary>
    public async Task<UpdateOnlyExitCode> RunUpdateOnlyAsync(
        CancellationToken cancellationToken = default)
    {
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        deadline.CancelAfter(TriggerDeadline);
        try
        {
            UpdateCheckResult check;
            try
            {
                check = await CheckAsync(deadline.Token)
                    .WaitAsync(deadline.Token)
                    .ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (deadline.IsCancellationRequested)
            {
                return UpdateOnlyExitCode.DiscoveryFailure;
            }

            if (check.IsFailure)
            {
                return UpdateOnlyExitCode.DiscoveryFailure;
            }

            try
            {
                return MapExitCode(
                    await StartAvailableUpdateAsync(deadline.Token)
                        .WaitAsync(deadline.Token)
                        .ConfigureAwait(false));
            }
            catch (OperationCanceledException) when (deadline.IsCancellationRequested)
            {
                // At this point discovery succeeded; a trigger deadline while
                // downloading/applying is therefore not code 21.
                return UpdateOnlyExitCode.DownloadOrIntegrityFailure;
            }
        }
        catch
        {
            return UpdateOnlyExitCode.LaunchFailure;
        }
    }

    public async Task<UpdateStartStatus> StartAvailableUpdateAsync(
        CancellationToken cancellationToken = default)
    {
        if (Volatile.Read(ref disposed) != 0)
        {
            return UpdateStartStatus.LaunchFailed;
        }

        if (Interlocked.CompareExchange(ref startInProgress, 1, 0) != 0)
        {
            return UpdateStartStatus.Busy;
        }

        string? partialPath = null;
        FileStream? lease = null;
        try
        {
            var pendingResult = ResolvePendingState(ReadPendingState());
            if (pendingResult is not null)
            {
                return pendingResult.Value;
            }

            var release = availableRelease;
            if (release is null)
            {
                return UpdateStartStatus.NoAvailableUpdate;
            }

            // Waiting is intentionally zero: another trigger must observe the
            // owner and return busy instead of creating a queue or a second
            // Setup process.
            lease = TryAcquireUpdateLease();
            if (lease is null)
            {
                return UpdateStartStatus.Busy;
            }

            // Re-read after acquiring the OS lease to close the race between
            // two processes that both observed no pending state above.
            pendingResult = ResolvePendingState(ReadPendingState());
            if (pendingResult is not null)
            {
                return pendingResult.Value;
            }

            var versionDirectory = Path.Combine(updateRoot, FormatVersion(release.Version));
            if (!WindowsPathSafety.EnsureDirectoryTreeWithoutReparse(versionDirectory))
            {
                return UpdateStartStatus.DownloadFailed;
            }

            var finalPath = Path.Combine(versionDirectory, InstallerName);
            partialPath = finalPath + ".download";
            DeleteIfPresent(partialPath);

            WindowsUpdateDownloadResult download;
            await using (var destination = new FileStream(
                             partialPath,
                             FileMode.CreateNew,
                             FileAccess.Write,
                             FileShare.None,
                             64 * 1024,
                             FileOptions.Asynchronous | FileOptions.SequentialScan))
            {
                download = await client.DownloadAsync(release, destination, cancellationToken)
                    .ConfigureAwait(false);
                await destination.FlushAsync(cancellationToken).ConfigureAwait(false);
            }

            if (!download.IsSuccess)
            {
                DeleteIfPresent(partialPath);
                return download.Failure == WindowsUpdateFailure.Integrity
                    ? UpdateStartStatus.IntegrityFailed
                    : UpdateStartStatus.DownloadFailed;
            }

            if (!WindowsPathSafety.EnsureDirectoryTreeWithoutReparse(versionDirectory) ||
                !WindowsPathSafety.IsMissingOrRegularFile(finalPath))
            {
                DeleteIfPresent(partialPath);
                return UpdateStartStatus.DownloadFailed;
            }

            File.Move(partialPath, finalPath, overwrite: true);
            partialPath = null;

            var attempt = new WindowsUpdatePendingState(
                Guid.NewGuid().ToString("N"),
                FormatVersion(release.Version),
                release.InstallerUri.AbsoluteUri,
                release.Sha256,
                release.Size,
                finalPath,
                DateTimeOffset.UtcNow,
                PendingStatus);

            // The logical owner must be durable before Setup is created. If
            // this write fails, no installer process is allowed to start.
            if (!TryWritePendingState(attempt))
            {
                return UpdateStartStatus.SafeBlocked;
            }

            var launch = LaunchSetup(finalPath);
            if (!launch.Started)
            {
                TryWritePendingState(attempt with
                {
                    Status = FailedStatus,
                    Failure = "setup-launch",
                    CompletedAtUtc = DateTimeOffset.UtcNow,
                });
                return UpdateStartStatus.LaunchFailed;
            }

            if (launch.Identity is null)
            {
                // A production launcher always supplies the exact identity.
                // Legacy test seams are retained for source compatibility and
                // remain pending until a subsequent read-back trigger.
                if (launcher is IWindowsInstallerProcessLauncher)
                {
                    TryWritePendingState(attempt with
                    {
                        Status = SafeBlockedStatus,
                        Failure = "setup-identity",
                    });
                    return UpdateStartStatus.SafeBlocked;
                }

                return UpdateStartStatus.Started;
            }

            var started = attempt with
            {
                ProcessId = launch.Identity.ProcessId,
                ProcessStartTimeUtc = launch.Identity.StartTimeUtc,
                ProcessPath = launch.Identity.ExecutablePath,
            };
            // PID/start/path append is part of the same logical owner. A
            // failed append blocks all future Setup launches until a human or
            // a later read-back can safely resolve the state.
            if (!TryWritePendingState(started))
            {
                return UpdateStartStatus.SafeBlocked;
            }

            return UpdateStartStatus.Started;
        }
        catch (OperationCanceledException)
        {
            DeleteIfPresent(partialPath);
            return UpdateStartStatus.DownloadFailed;
        }
        catch
        {
            DeleteIfPresent(partialPath);
            return UpdateStartStatus.DownloadFailed;
        }
        finally
        {
            lease?.Dispose();
            Volatile.Write(ref startInProgress, 0);
        }
    }

    public void Dispose()
    {
        if (Interlocked.Exchange(ref disposed, 1) != 0)
        {
            return;
        }

        availableRelease = null;
        if (client is IDisposable disposable)
        {
            disposable.Dispose();
        }
    }

    private WindowsInstallerLaunchResult LaunchSetup(string installerPath)
    {
        try
        {
            if (launcher is IWindowsInstallerProcessLauncher processLauncher)
            {
                return processLauncher.TryLaunchWithIdentity(installerPath);
            }

            return launcher.TryLaunch(installerPath)
                ? new WindowsInstallerLaunchResult(true, null)
                : WindowsInstallerLaunchResult.Failed();
        }
        catch
        {
            return WindowsInstallerLaunchResult.Failed();
        }
    }

    private void ClosePendingWhenCurrentGenerationIsInstalled()
    {
        var pending = ReadPendingState().State;
        if (pending is null ||
            !TryParseVersion(pending.TargetVersion, out var targetVersion) ||
            currentVersion.CompareTo(targetVersion) != 0)
        {
            return;
        }

        TryWritePendingState(pending with
        {
            Status = SuccessStatus,
            Failure = null,
            CompletedAtUtc = DateTimeOffset.UtcNow,
        });
    }

    private UpdateStartStatus? ResolvePendingState(PendingReadResult pendingResult)
    {
        if (pendingResult.Invalid)
        {
            return UpdateStartStatus.SafeBlocked;
        }

        var pending = pendingResult.State;
        if (pending is null)
        {
            return null;
        }

        if (!TryParseVersion(pending.TargetVersion, out var targetVersion))
        {
            return UpdateStartStatus.SafeBlocked;
        }

        if (currentVersion.CompareTo(targetVersion) == 0)
        {
            return TryWritePendingState(pending with
            {
                Status = SuccessStatus,
                Failure = null,
                CompletedAtUtc = DateTimeOffset.UtcNow,
            })
                ? UpdateStartStatus.NoAvailableUpdate
                : UpdateStartStatus.SafeBlocked;
        }

        if (string.Equals(pending.Status, SuccessStatus, StringComparison.Ordinal))
        {
            return UpdateStartStatus.SafeBlocked;
        }

        var active = IsExactSetupAlive(pending, out var identityAvailable);
        if (active)
        {
            var age = DateTimeOffset.UtcNow - pending.AttemptStartedAtUtc;
            if (age >= TriggerDeadline)
            {
                TryWritePendingState(pending with
                {
                    Status = SafeBlockedStatus,
                    Failure = "setup-timeout",
                });
                return UpdateStartStatus.SafeBlocked;
            }

            return UpdateStartStatus.Busy;
        }

        if (!identityAvailable && string.Equals(pending.Status, PendingStatus, StringComparison.Ordinal))
        {
            // A state without an exact process identity may represent a
            // process whose PID append failed. Keep it SAFE_BLOCKED forever
            // until a trustworthy target read-back resolves it; never infer
            // that the unknown Setup ended and launch another one.
            TryWritePendingState(pending with
            {
                Status = SafeBlockedStatus,
                Failure = pending.Failure ?? "missing-setup-identity",
            });
            return UpdateStartStatus.SafeBlocked;
        }

        if (string.Equals(pending.Status, SafeBlockedStatus, StringComparison.Ordinal))
        {
            if (!identityAvailable)
            {
                return UpdateStartStatus.SafeBlocked;
            }

            TryWritePendingState(pending with
            {
                Status = FailedStatus,
                Failure = pending.Failure ?? "old-version",
                CompletedAtUtc = DateTimeOffset.UtcNow,
            });
            return UpdateStartStatus.OldVersionFailed;
        }

        if (string.Equals(pending.Status, PendingStatus, StringComparison.Ordinal))
        {
            TryWritePendingState(pending with
            {
                Status = FailedStatus,
                Failure = pending.Failure ?? "old-version",
                CompletedAtUtc = DateTimeOffset.UtcNow,
            });
            return UpdateStartStatus.OldVersionFailed;
        }

        // A failed attempt belongs to the previous trigger. The current
        // trigger is the explicit next attempt and may proceed.
        return null;
    }

    private bool IsExactSetupAlive(
        WindowsUpdatePendingState pending,
        out bool identityAvailable)
    {
        if (pending.ProcessId is not > 0 ||
            pending.ProcessStartTimeUtc is not { } expectedStart ||
            string.IsNullOrWhiteSpace(pending.ProcessPath))
        {
            identityAvailable = false;
            return false;
        }

        identityAvailable = true;

        try
        {
            using var process = Process.GetProcessById(pending.ProcessId!.Value);
            if (process.HasExited)
            {
                return false;
            }

            var actualStart = process.StartTime.ToUniversalTime();
            var actualPath = process.MainModule?.FileName;
            return actualPath is not null &&
                DateTimeOffset.Equals(
                    new DateTimeOffset(actualStart, TimeSpan.Zero),
                    expectedStart) &&
                string.Equals(
                    Path.GetFullPath(actualPath),
                    Path.GetFullPath(pending.ProcessPath!),
                    StringComparison.OrdinalIgnoreCase);
        }
        catch (ArgumentException)
        {
            // A missing PID is an ended process, not a live owner. PID reuse
            // is still rejected below by the start-time/path comparison.
            return false;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
        catch (System.ComponentModel.Win32Exception)
        {
            return true;
        }
        catch (UnauthorizedAccessException)
        {
            return true;
        }
        catch
        {
            // An inaccessible process cannot be proven to be a different
            // owner. Keep the logical lease busy rather than risking a
            // second Setup beside a still-running transaction.
            return true;
        }
    }

    private PendingReadResult ReadPendingState()
    {
        var path = Path.Combine(updateRoot, PendingStateFileName);
        try
        {
            if (WindowsPathSafety.ContainsReparsePoint(updateRoot) ||
                !Directory.Exists(updateRoot))
            {
                return PendingReadResult.Missing;
            }

            if (!WindowsPathSafety.IsMissingOrRegularFile(path))
            {
                return PendingReadResult.InvalidResult;
            }

            if (!File.Exists(path))
            {
                return PendingReadResult.Missing;
            }

            var info = new FileInfo(path);
            if (info.Length <= 0 || info.Length > 64 * 1024)
            {
                return PendingReadResult.InvalidResult;
            }

            var state = JsonSerializer.Deserialize<WindowsUpdatePendingState>(
                File.ReadAllText(path),
                PendingJsonOptions);
            if (state is null ||
                string.IsNullOrWhiteSpace(state.AttemptId) ||
                string.IsNullOrWhiteSpace(state.TargetVersion) ||
                string.IsNullOrWhiteSpace(state.TargetSource) ||
                string.IsNullOrWhiteSpace(state.InstallerSha256) ||
                state.InstallerSize <= 0 ||
                !Path.IsPathFullyQualified(state.InstallerPath) ||
                string.IsNullOrWhiteSpace(state.InstallerPath) ||
                state.AttemptStartedAtUtc == default ||
                (state.ProcessPath is not null && !Path.IsPathFullyQualified(state.ProcessPath)) ||
                !IsKnownStatus(state.Status))
            {
                return PendingReadResult.InvalidResult;
            }

            return new PendingReadResult(state, false);
        }
        catch
        {
            return PendingReadResult.InvalidResult;
        }
    }

    private bool TryWritePendingState(WindowsUpdatePendingState state)
    {
        var path = Path.Combine(updateRoot, PendingStateFileName);
        var temporary = path + "." + Guid.NewGuid().ToString("N") + PendingStateTempSuffix;
        try
        {
            if (!WindowsPathSafety.EnsureDirectoryTreeWithoutReparse(updateRoot) ||
                WindowsPathSafety.ContainsReparsePoint(updateRoot) ||
                !WindowsPathSafety.IsMissingOrRegularFile(path))
            {
                return false;
            }

            using (var stream = new FileStream(
                       temporary,
                       FileMode.CreateNew,
                       FileAccess.Write,
                       FileShare.None,
                       4096,
                       FileOptions.SequentialScan))
            {
                JsonSerializer.Serialize(stream, state, PendingJsonOptions);
                stream.Flush(flushToDisk: true);
            }

            if (!WindowsPathSafety.EnsureDirectoryTreeWithoutReparse(updateRoot) ||
                !WindowsPathSafety.IsMissingOrRegularFile(path))
            {
                return false;
            }

            File.Move(temporary, path, overwrite: true);
            return true;
        }
        catch
        {
            return false;
        }
        finally
        {
            DeleteIfPresent(temporary);
        }
    }

    private static bool IsKnownStatus(string? status) =>
        status is PendingStatus or FailedStatus or SafeBlockedStatus or SuccessStatus;

    private static UpdateOnlyExitCode MapExitCode(UpdateStartStatus status) => status switch
    {
        UpdateStartStatus.Started => UpdateOnlyExitCode.SetupStarted,
        UpdateStartStatus.NoAvailableUpdate => UpdateOnlyExitCode.Current,
        UpdateStartStatus.Busy or UpdateStartStatus.SafeBlocked => UpdateOnlyExitCode.Busy,
        UpdateStartStatus.DiscoveryFailed => UpdateOnlyExitCode.DiscoveryFailure,
        UpdateStartStatus.OldVersionFailed => UpdateOnlyExitCode.LaunchFailure,
        UpdateStartStatus.DownloadFailed or UpdateStartStatus.IntegrityFailed => UpdateOnlyExitCode.DownloadOrIntegrityFailure,
        UpdateStartStatus.LaunchFailed => UpdateOnlyExitCode.LaunchFailure,
        _ => UpdateOnlyExitCode.LaunchFailure,
    };

    private static bool TryParseVersion(string? text, out Version version)
    {
        if (!Version.TryParse(text, out var parsed))
        {
            version = new Version();
            return false;
        }

        version = parsed;
        return version.Build >= 0 && version.Revision < 0;
    }

    private static string FormatVersion(Version version) =>
        $"{version.Major}.{version.Minor}.{version.Build}";

    private static void DeleteIfPresent(string? path)
    {
        if (path is null)
        {
            return;
        }

        try
        {
            if (!WindowsPathSafety.IsMissingOrRegularFile(path))
            {
                return;
            }

            File.Delete(path);
        }
        catch
        {
            // Cleanup is best-effort. A stale target remains fail closed.
        }
    }

    private FileStream? TryAcquireUpdateLease()
    {
        try
        {
            if (!WindowsPathSafety.EnsureDirectoryTreeWithoutReparse(updateRoot) ||
                WindowsPathSafety.ContainsReparsePoint(updateRoot))
            {
                return null;
            }

            var leasePath = Path.Combine(updateRoot, LeaseFileName);
            if (!WindowsPathSafety.IsMissingOrRegularFile(leasePath))
            {
                return null;
            }

            return new FileStream(
                leasePath,
                FileMode.OpenOrCreate,
                FileAccess.ReadWrite,
                FileShare.None,
                1,
                FileOptions.DeleteOnClose);
        }
        catch (IOException)
        {
            return null;
        }
        catch (UnauthorizedAccessException)
        {
            return null;
        }
    }

    private sealed record PendingReadResult(
        WindowsUpdatePendingState? State,
        bool Invalid)
    {
        public static PendingReadResult Missing { get; } = new(null, false);

        public static PendingReadResult InvalidResult { get; } = new(null, true);
    }
}
