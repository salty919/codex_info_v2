// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Security.Cryptography;
using System.Diagnostics;
using System.Text.Json;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Infrastructure;
using CodexInfo.WindowsClient.Updates;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class WindowsUpdateCoordinatorTests
{
    [Fact]
    public async Task SetupStartPersistsAtomicPendingOwnerMetadata()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 8, 6, 7, 5 };
        var release = ReleaseFor(payload);
        var client = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        var launcher = new RecordingLauncher();
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);
        await coordinator.CheckAsync();

        Assert.Equal(UpdateStartStatus.Started, await coordinator.StartAvailableUpdateAsync());

        using var document = JsonDocument.Parse(
            await File.ReadAllTextAsync(Path.Combine(directory.Path, ".update.pending.json")));
        var root = document.RootElement;
        Assert.Equal("pending", root.GetProperty("status").GetString());
        Assert.False(string.IsNullOrWhiteSpace(root.GetProperty("attempt_id").GetString()));
        Assert.Equal("1.2.3", root.GetProperty("target_version").GetString());
        Assert.Equal(release.InstallerUri.AbsoluteUri, root.GetProperty("target_source").GetString());
        Assert.Equal(release.Sha256, root.GetProperty("installer_sha256").GetString());
        Assert.Equal(release.Size, root.GetProperty("installer_size").GetInt64());
        Assert.Equal(JsonValueKind.Null, root.GetProperty("process_id").ValueKind);
    }

    [Fact]
    public async Task PendingTargetReadbackClosesSuccessWithoutLaunchingAgain()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 1, 4, 1, 4 };
        var release = ReleaseFor(payload);
        var firstClient = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        var firstLauncher = new RecordingLauncher();
        using (var first = new WindowsUpdateCoordinator(
                   firstClient,
                   firstLauncher,
                   new Version(1, 0, 0),
                   directory.Path))
        {
            await first.CheckAsync();
            Assert.Equal(UpdateStartStatus.Started, await first.StartAvailableUpdateAsync());
        }

        var secondLauncher = new RecordingLauncher();
        using var second = new WindowsUpdateCoordinator(
            new FakeUpdateClient(WindowsUpdateCheckResult.NoUpdate()),
            secondLauncher,
            new Version(1, 2, 3),
            directory.Path);

        Assert.False((await second.CheckAsync()).IsFailure);
        Assert.Equal(UpdateStartStatus.NoAvailableUpdate, await second.StartAvailableUpdateAsync());
        Assert.Empty(secondLauncher.Paths);
        using var document = JsonDocument.Parse(
            await File.ReadAllTextAsync(Path.Combine(directory.Path, ".update.pending.json")));
        Assert.Equal("success", document.RootElement.GetProperty("status").GetString());
    }

    [Fact]
    public async Task ExactLiveSetupIdentityReturnsBusyWithoutDownloadingAgain()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 2, 7, 1, 8 };
        var release = ReleaseFor(payload);
        var identity = CurrentProcessIdentity();
        var firstClient = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        var firstLauncher = new RecordingProcessLauncher(identity);
        using var first = new WindowsUpdateCoordinator(
            firstClient,
            firstLauncher,
            new Version(1, 0, 0),
            directory.Path);
        await first.CheckAsync();
        Assert.Equal(UpdateStartStatus.Started, await first.StartAvailableUpdateAsync());

        var secondClient = new FakeUpdateClient(WindowsUpdateCheckResult.Success(release));
        var secondLauncher = new RecordingProcessLauncher(identity);
        using var second = new WindowsUpdateCoordinator(
            secondClient,
            secondLauncher,
            new Version(1, 0, 0),
            directory.Path);
        await second.CheckAsync();

        Assert.Equal(UpdateStartStatus.Busy, await second.StartAvailableUpdateAsync());
        Assert.Equal(0, secondClient.DownloadCount);
        Assert.Empty(secondLauncher.Paths);
    }

    [Fact]
    public async Task MissingSetupIdentityStaysSafeBlockedAndNeverRetriesAutomatically()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 3, 2, 3, 2 };
        var release = ReleaseFor(payload);
        var client = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        var launcher = new RecordingProcessLauncher(null);
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);
        await coordinator.CheckAsync();

        Assert.Equal(UpdateStartStatus.SafeBlocked, await coordinator.StartAvailableUpdateAsync());
        Assert.Equal(UpdateStartStatus.SafeBlocked, await coordinator.StartAvailableUpdateAsync());
        Assert.Single(launcher.Paths);
        using var document = JsonDocument.Parse(
            await File.ReadAllTextAsync(Path.Combine(directory.Path, ".update.pending.json")));
        Assert.Equal("safe_blocked", document.RootElement.GetProperty("status").GetString());
    }

    [Fact]
    public async Task EndedExactSetupIsOldVersionFailureBeforeAnyNewLaunch()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 6, 2, 6, 2 };
        var release = ReleaseFor(payload);
        var client = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        using var exited = Process.Start(new ProcessStartInfo
        {
            FileName = "/bin/sleep",
            Arguments = "0.1",
            UseShellExecute = false,
            CreateNoWindow = true,
        });
        Assert.NotNull(exited);
        var identity = new WindowsInstallerProcessIdentity(
            exited!.Id,
            new DateTimeOffset(exited.StartTime.ToUniversalTime(), TimeSpan.Zero),
            exited.MainModule!.FileName!);

        var launcher = new RecordingProcessLauncher(identity);
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);
        await coordinator.CheckAsync();

        // The fixture writes the pending owner first, as a prior trigger did.
        Assert.Equal(UpdateStartStatus.Started, await coordinator.StartAvailableUpdateAsync());
        await exited.WaitForExitAsync();

        var nextLauncher = new RecordingProcessLauncher(identity);
        using var next = new WindowsUpdateCoordinator(
            new FakeUpdateClient(WindowsUpdateCheckResult.Success(release)),
            nextLauncher,
            new Version(1, 0, 0),
            directory.Path);
        await next.CheckAsync();

        Assert.Equal(UpdateStartStatus.OldVersionFailed, await next.StartAvailableUpdateAsync());
        Assert.Empty(nextLauncher.Paths);
    }

    [Fact]
    public async Task UpdateOnlyUsesFiniteExitCodes()
    {
        using (var currentDirectory = new TemporaryDirectory())
        {
            using var current = new WindowsUpdateCoordinator(
                new FakeUpdateClient(WindowsUpdateCheckResult.NoUpdate()),
                new RecordingLauncher(),
                new Version(1, 0, 0),
                currentDirectory.Path);
            Assert.Equal(UpdateOnlyExitCode.Current, await current.RunUpdateOnlyAsync());
        }

        using (var startDirectory = new TemporaryDirectory())
        {
            var payload = new byte[] { 9, 9, 1 };
            var release = ReleaseFor(payload);
            var client = new FakeUpdateClient(
                WindowsUpdateCheckResult.Success(release),
                async (destination, cancellationToken) =>
                {
                    await destination.WriteAsync(payload, cancellationToken);
                    return WindowsUpdateDownloadResult.Success();
                });
            using var start = new WindowsUpdateCoordinator(
                client,
                new RecordingLauncher(),
                new Version(1, 0, 0),
                startDirectory.Path);
            Assert.Equal(UpdateOnlyExitCode.SetupStarted, await start.RunUpdateOnlyAsync());
        }

        using var failureDirectory = new TemporaryDirectory();
        using var failure = new WindowsUpdateCoordinator(
            new FakeUpdateClient(WindowsUpdateCheckResult.FromFailure(WindowsUpdateFailure.Response)),
            new RecordingLauncher(),
            new Version(1, 0, 0),
            failureDirectory.Path);
        Assert.Equal(UpdateOnlyExitCode.DiscoveryFailure, await failure.RunUpdateOnlyAsync());
    }

    [Fact]
    public async Task CheckOnlyPublishesNoticeAndDoesNotDownloadOrLaunch()
    {
        using var directory = new TemporaryDirectory();
        var release = ReleaseFor([1, 2, 3]);
        var client = new FakeUpdateClient(WindowsUpdateCheckResult.Success(release));
        var launcher = new RecordingLauncher();
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);

        var result = await coordinator.CheckAsync(CancellationToken.None);

        Assert.Equal("1.2.3", result.AvailableVersion);
        Assert.False(result.IsFailure);
        Assert.Equal(0, client.DownloadCount);
        Assert.Empty(launcher.Paths);
        Assert.Empty(Directory.EnumerateFileSystemEntries(directory.Path));
    }

    [Fact]
    public async Task ExplicitStartDownloadsVerifiedBytesAndLaunchesOrdinarySetup()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 1, 3, 5, 7 };
        var release = ReleaseFor(payload);
        var client = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        var launcher = new RecordingLauncher();
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);
        await coordinator.CheckAsync(CancellationToken.None);

        var result = await coordinator.StartAvailableUpdateAsync(CancellationToken.None);

        Assert.Equal(UpdateStartStatus.Started, result);
        var path = Assert.Single(launcher.Paths);
        Assert.Equal("CodexInfo.WindowsClient.Setup.exe", System.IO.Path.GetFileName(path));
        Assert.Equal(payload, await File.ReadAllBytesAsync(path));
        Assert.False(File.Exists(path + ".download"));
    }

    [Fact]
    public async Task StartWithoutAvailableReleaseHasNoFilesystemOrLauncherMutation()
    {
        using var directory = new TemporaryDirectory();
        var client = new FakeUpdateClient(WindowsUpdateCheckResult.NoUpdate());
        var launcher = new RecordingLauncher();
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);

        var result = await coordinator.StartAvailableUpdateAsync(CancellationToken.None);

        Assert.Equal(UpdateStartStatus.NoAvailableUpdate, result);
        Assert.Equal(0, client.DownloadCount);
        Assert.Empty(launcher.Paths);
        Assert.Empty(Directory.EnumerateFileSystemEntries(directory.Path));
    }

    [Theory]
    [InlineData(WindowsUpdateFailure.Transport, UpdateStartStatus.DownloadFailed)]
    [InlineData(WindowsUpdateFailure.Response, UpdateStartStatus.DownloadFailed)]
    [InlineData(WindowsUpdateFailure.Integrity, UpdateStartStatus.IntegrityFailed)]
    public async Task FailedDownloadDeletesPartialFileAndNeverLaunches(
        WindowsUpdateFailure failure,
        UpdateStartStatus expected)
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 2, 4, 6 };
        var release = ReleaseFor(payload);
        var client = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                await destination.WriteAsync(payload.AsMemory(0, 1), cancellationToken);
                return WindowsUpdateDownloadResult.FromFailure(failure);
            });
        var launcher = new RecordingLauncher();
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);
        await coordinator.CheckAsync(CancellationToken.None);

        var result = await coordinator.StartAvailableUpdateAsync(CancellationToken.None);

        Assert.Equal(expected, result);
        Assert.Empty(launcher.Paths);
        Assert.Empty(Directory.EnumerateFiles(directory.Path, "*.download", SearchOption.AllDirectories));
    }

    [Fact]
    public async Task ConcurrentSecondStartIsDroppedWithoutQueueingAnotherDownload()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 9, 8, 7 };
        var entered = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var releaseDownload = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var release = ReleaseFor(payload);
        var client = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                entered.SetResult();
                await releaseDownload.Task.WaitAsync(cancellationToken);
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        var launcher = new RecordingLauncher();
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);
        await coordinator.CheckAsync(CancellationToken.None);

        var first = coordinator.StartAvailableUpdateAsync(CancellationToken.None);
        await entered.Task.WaitAsync(TimeSpan.FromSeconds(2));
        var second = await coordinator.StartAvailableUpdateAsync(CancellationToken.None);
        releaseDownload.SetResult();

        Assert.Equal(UpdateStartStatus.Busy, second);
        Assert.Equal(UpdateStartStatus.Started, await first);
        Assert.Equal(1, client.DownloadCount);
        Assert.Single(launcher.Paths);
    }

    [Fact]
    public async Task SeparateCoordinatorsShareAnExclusiveInstallRootLease()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 9, 1, 4 };
        var entered = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var releaseDownload = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var release = ReleaseFor(payload);

        var firstClient = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                entered.SetResult();
                await releaseDownload.Task.WaitAsync(cancellationToken);
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        var secondClient = new FakeUpdateClient(WindowsUpdateCheckResult.Success(release));
        using var first = new WindowsUpdateCoordinator(
            firstClient,
            new RecordingLauncher(),
            new Version(1, 0, 0),
            directory.Path);
        using var second = new WindowsUpdateCoordinator(
            secondClient,
            new RecordingLauncher(),
            new Version(1, 0, 0),
            directory.Path);
        await first.CheckAsync(CancellationToken.None);
        await second.CheckAsync(CancellationToken.None);

        var firstStart = first.StartAvailableUpdateAsync(CancellationToken.None);
        await entered.Task.WaitAsync(TimeSpan.FromSeconds(2));
        var secondStart = await second.StartAvailableUpdateAsync(CancellationToken.None);
        releaseDownload.SetResult();

        Assert.Equal(UpdateStartStatus.Busy, secondStart);
        Assert.Equal(UpdateStartStatus.Started, await firstStart);
        Assert.Equal(0, secondClient.DownloadCount);
    }

    [Fact]
    public async Task ReparseLeaseTargetFailsClosedWithoutDownloading()
    {
        using var directory = new TemporaryDirectory();
        var marker = Path.Combine(directory.Path, "marker");
        await File.WriteAllTextAsync(marker, "keep");
        var leasePath = Path.Combine(directory.Path, ".update.lease");
        try
        {
            File.CreateSymbolicLink(leasePath, marker);
        }
        catch (UnauthorizedAccessException)
        {
            // Some Windows test hosts require Developer Mode or an elevated
            // token for symlink creation; the physical-host gate covers that
            // platform-specific branch.
            return;
        }

        var release = ReleaseFor([1, 2, 3]);
        var client = new FakeUpdateClient(WindowsUpdateCheckResult.Success(release));
        using var coordinator = new WindowsUpdateCoordinator(
            client, new RecordingLauncher(), new Version(1, 0, 0), directory.Path);
        await coordinator.CheckAsync(CancellationToken.None);

        Assert.Equal(UpdateStartStatus.Busy, await coordinator.StartAvailableUpdateAsync(CancellationToken.None));
        Assert.Equal(0, client.DownloadCount);
        Assert.Equal("keep", await File.ReadAllTextAsync(marker));
    }

    [Fact]
    public async Task ReparseParentUpdateRootFailsClosedWithoutDownloading()
    {
        using var directory = new TemporaryDirectory();
        var realRoot = Path.Combine(directory.Path, "real-root");
        Directory.CreateDirectory(realRoot);
        var linkedRoot = Path.Combine(directory.Path, "linked-root");
        try
        {
            Directory.CreateSymbolicLink(linkedRoot, realRoot);
        }
        catch (UnauthorizedAccessException)
        {
            return;
        }

        var release = ReleaseFor([3, 1, 4]);
        var client = new FakeUpdateClient(WindowsUpdateCheckResult.Success(release));
        using var coordinator = new WindowsUpdateCoordinator(
            client, new RecordingLauncher(), new Version(1, 0, 0), linkedRoot);
        await coordinator.CheckAsync(CancellationToken.None);

        Assert.Equal(UpdateStartStatus.Busy, await coordinator.StartAvailableUpdateAsync(CancellationToken.None));
        Assert.Equal(0, client.DownloadCount);
        Assert.Empty(Directory.EnumerateFileSystemEntries(realRoot));
    }

    [Fact]
    public async Task LaunchFailureIsTypedAndVerifiedInstallerRemainsForExplicitRetry()
    {
        using var directory = new TemporaryDirectory();
        var payload = new byte[] { 4, 2 };
        var release = ReleaseFor(payload);
        var client = new FakeUpdateClient(
            WindowsUpdateCheckResult.Success(release),
            async (destination, cancellationToken) =>
            {
                await destination.WriteAsync(payload, cancellationToken);
                return WindowsUpdateDownloadResult.Success();
            });
        var launcher = new RecordingLauncher { Result = false };
        using var coordinator = new WindowsUpdateCoordinator(
            client, launcher, new Version(1, 0, 0), directory.Path);
        await coordinator.CheckAsync(CancellationToken.None);

        var result = await coordinator.StartAvailableUpdateAsync(CancellationToken.None);

        Assert.Equal(UpdateStartStatus.LaunchFailed, result);
        var path = Assert.Single(launcher.Paths);
        Assert.True(File.Exists(path));
    }

    [Theory]
    [InlineData("relative.exe")]
    [InlineData("")]
    public void SystemLauncherRejectsUntrustedPathWithoutStartingIt(string path)
    {
        var launcher = new WindowsInstallerLauncher();

        Assert.False(launcher.TryLaunch(path));
    }

    private static WindowsUpdateRelease ReleaseFor(byte[] payload)
    {
        var hash = Convert.ToHexString(SHA256.HashData(payload)).ToLowerInvariant();
        return new WindowsUpdateRelease(
            new Version(1, 2, 3),
            new Uri("https://github.com/salty919/codex_info_v2/releases/download/windows-v1.2.3/CodexInfo.WindowsClient.Setup.exe"),
            hash,
            payload.Length);
    }

    private static WindowsInstallerProcessIdentity CurrentProcessIdentity()
    {
        using var process = Process.GetCurrentProcess();
        return new WindowsInstallerProcessIdentity(
            process.Id,
            new DateTimeOffset(process.StartTime.ToUniversalTime(), TimeSpan.Zero),
            process.MainModule!.FileName!);
    }

    private sealed class FakeUpdateClient : IWindowsUpdateClient, IDisposable
    {
        private readonly WindowsUpdateCheckResult checkResult;
        private readonly Func<Stream, CancellationToken, Task<WindowsUpdateDownloadResult>> download;

        public FakeUpdateClient(
            WindowsUpdateCheckResult checkResult,
            Func<Stream, CancellationToken, Task<WindowsUpdateDownloadResult>>? download = null)
        {
            this.checkResult = checkResult;
            this.download = download ?? ((_, _) => Task.FromResult(WindowsUpdateDownloadResult.Success()));
        }

        public int DownloadCount { get; private set; }

        public Task<WindowsUpdateCheckResult> CheckAsync(
            Version current,
            CancellationToken cancellationToken = default) => Task.FromResult(checkResult);

        public Task<WindowsUpdateDownloadResult> DownloadAsync(
            WindowsUpdateRelease release,
            Stream destination,
            CancellationToken cancellationToken = default)
        {
            DownloadCount++;
            return download(destination, cancellationToken);
        }

        public void Dispose()
        {
        }
    }

    private sealed class RecordingLauncher : IWindowsInstallerLauncher
    {
        public List<string> Paths { get; } = [];

        public bool Result { get; init; } = true;

        public bool TryLaunch(string installerPath)
        {
            Paths.Add(installerPath);
            return Result;
        }
    }

    private sealed class RecordingProcessLauncher(
        WindowsInstallerProcessIdentity? identity) :
        IWindowsInstallerLauncher,
        IWindowsInstallerProcessLauncher
    {
        public List<string> Paths { get; } = [];

        public bool TryLaunch(string installerPath)
        {
            Paths.Add(installerPath);
            return true;
        }

        public WindowsInstallerLaunchResult TryLaunchWithIdentity(string installerPath)
        {
            Paths.Add(installerPath);
            return new WindowsInstallerLaunchResult(true, identity);
        }
    }

    private sealed class TemporaryDirectory : IDisposable
    {
        public TemporaryDirectory()
        {
            Path = System.IO.Path.Combine(
                System.IO.Path.GetTempPath(),
                "codex-info-update-tests-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(Path);
        }

        public string Path { get; }

        public void Dispose()
        {
            if (Directory.Exists(Path))
            {
                Directory.Delete(Path, recursive: true);
            }
        }
    }
}
