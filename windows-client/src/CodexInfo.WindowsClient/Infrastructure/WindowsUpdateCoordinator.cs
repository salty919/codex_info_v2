// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Diagnostics;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Updates;

namespace CodexInfo.WindowsClient.Infrastructure;

/// <summary>Starts the ordinary Windows installer UI after validation.</summary>
public interface IWindowsInstallerLauncher
{
    bool TryLaunch(string installerPath);
}

/// <summary>
/// Launches only an existing absolute executable. No unattended installer
/// argument is added: the user remains in control of the standard Setup UI.
/// </summary>
public sealed class WindowsInstallerLauncher : IWindowsInstallerLauncher
{
    public bool TryLaunch(string installerPath)
    {
        if (string.IsNullOrWhiteSpace(installerPath) ||
            !Path.IsPathFullyQualified(installerPath) ||
            !string.Equals(Path.GetExtension(installerPath), ".exe", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        try
        {
            var file = new FileInfo(installerPath);
            if (WindowsPathSafety.ContainsReparsePoint(file.FullName) ||
                !WindowsPathSafety.IsMissingOrRegularFile(file.FullName) ||
                !file.Exists || file.Length <= 0)
            {
                return false;
            }

            using var process = Process.Start(new ProcessStartInfo
            {
                FileName = file.FullName,
                WorkingDirectory = file.DirectoryName!,
                UseShellExecute = true,
            });
            return process is not null;
        }
        catch
        {
            return false;
        }
    }
}

/// <summary>
/// Owns the user-initiated mutation boundary. <see cref="CheckAsync"/> only
/// stores validated metadata; bytes are downloaded and Setup is started only
/// from <see cref="StartAvailableUpdateAsync"/>.
/// </summary>
public sealed class WindowsUpdateCoordinator : IWindowsUpdateCoordinator
{
    private const string InstallerName = "CodexInfo.WindowsClient.Setup.exe";
    private const string LeaseFileName = ".update.lease";

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

    public async Task<UpdateCheckResult> CheckAsync(CancellationToken cancellationToken)
    {
        if (Volatile.Read(ref disposed) != 0)
        {
            return new UpdateCheckResult(null, true);
        }

        availableRelease = null;
        var result = await client.CheckAsync(currentVersion, cancellationToken).ConfigureAwait(false);
        if (!result.IsSuccess)
        {
            return new UpdateCheckResult(null, true);
        }

        availableRelease = result.Release;
        return new UpdateCheckResult(
            result.Release is null ? null : FormatVersion(result.Release.Version),
            false);
    }

    public async Task<UpdateStartStatus> StartAvailableUpdateAsync(CancellationToken cancellationToken)
    {
        if (Volatile.Read(ref disposed) != 0)
        {
            return UpdateStartStatus.DownloadFailed;
        }

        if (Interlocked.CompareExchange(ref startInProgress, 1, 0) != 0)
        {
            return UpdateStartStatus.Busy;
        }

        string? partialPath = null;
        FileStream? lease = null;
        try
        {
            var release = availableRelease;
            if (release is null)
            {
                return UpdateStartStatus.NoAvailableUpdate;
            }

            // The in-process gate above prevents duplicate work in one UI, but
            // the install root is shared by separate client processes too.
            // An exclusive OS file lease makes that boundary fail closed after
            // crashes as well: the stale file is reusable once its owner exits.
            lease = TryAcquireUpdateLease();
            if (lease is null)
            {
                return UpdateStartStatus.Busy;
            }

            var versionDirectory = Path.Combine(updateRoot, FormatVersion(release.Version));
            if (!WindowsPathSafety.EnsureDirectoryTreeWithoutReparse(versionDirectory))
            {
                return UpdateStartStatus.DownloadFailed;
            }

            PruneObsoleteSetups(release.Version, cancellationToken);

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
            return launcher.TryLaunch(finalPath)
                ? UpdateStartStatus.Started
                : UpdateStartStatus.LaunchFailed;
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

    private static string FormatVersion(Version version) =>
        $"{version.Major}.{version.Minor}.{version.Build}";

    private void PruneObsoleteSetups(Version targetVersion, CancellationToken cancellationToken)
    {
        foreach (var versionDirectory in Directory.EnumerateFileSystemEntries(updateRoot))
        {
            cancellationToken.ThrowIfCancellationRequested();
            var name = Path.GetFileName(versionDirectory);
            if (!Version.TryParse(name, out var version) ||
                version.Build < 0 ||
                !string.Equals(name, FormatVersion(version), StringComparison.Ordinal) ||
                version.CompareTo(targetVersion) >= 0)
            {
                continue;
            }

            try
            {
                var directoryAttributes = File.GetAttributes(versionDirectory);
                if ((directoryAttributes & (FileAttributes.Directory | FileAttributes.ReparsePoint)) !=
                        FileAttributes.Directory ||
                    WindowsPathSafety.ContainsReparsePoint(versionDirectory))
                {
                    continue;
                }

                var entries = Directory.GetFileSystemEntries(versionDirectory);
                if (entries.Length != 1 ||
                    !string.Equals(Path.GetFileName(entries[0]), InstallerName, StringComparison.Ordinal))
                {
                    continue;
                }

                var setupPath = entries[0];
                var setupAttributes = File.GetAttributes(setupPath);
                if ((setupAttributes & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0 ||
                    WindowsPathSafety.ContainsReparsePoint(setupPath))
                {
                    continue;
                }

                // An active Setup cannot grant this exclusive open. Deletion is
                // bound to the opened file, so a later path replacement is not
                // removed through a separate File.Delete call.
                using (new FileStream(
                           setupPath, FileMode.Open, FileAccess.ReadWrite,
                           FileShare.None, 1, FileOptions.DeleteOnClose))
                {
                }

                if (!WindowsPathSafety.ContainsReparsePoint(versionDirectory) &&
                    Directory.GetFileSystemEntries(versionDirectory).Length == 0)
                {
                    Directory.Delete(versionDirectory);
                }
            }
            catch (IOException)
            {
                // Unclear, active, or changed entries are not cleanup targets.
            }
            catch (UnauthorizedAccessException)
            {
                // Keep entries that this updater cannot prove it may mutate.
            }
        }
    }

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
            // Cleanup is best-effort. A stale CreateNew target still fails
            // closed on the next explicit attempt instead of being executed.
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
}
