// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Diagnostics;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Infrastructure;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class SetupConnectionEnvironmentTests
{
    [Fact]
    public async Task InventorySelectorsDriveExactProfileValidation()
    {
        using var main = await ReadyMainAsync();
        var environment = new FakeSetupConnectionEnvironment(
            ["work.example"],
            ["Ubuntu-24.04"]);
        using var setup = new SetupViewModel(
            main,
            new RecordingSettingsSession(ClientSettings.Default),
            environment);

        Assert.Equal(3, setup.ConnectionProfileOptions.Count);
        setup.SelectedConnectionProfile = ConnectionProfiles.Wsl;
        Assert.Equal("Ubuntu-24.04", setup.SelectedConnectionSelector);
        Assert.Equal(["Ubuntu-24.04"], setup.ConnectionSelectorOptions);
        Assert.True(setup.IsConnectionSelectionValid);

        setup.SelectedConnectionProfile = ConnectionProfiles.SshConfigAlias;
        Assert.Equal("work.example", setup.SelectedConnectionSelector);
        Assert.Equal("work.example", setup.SshHost);
        Assert.Equal(["work.example"], setup.ConnectionSelectorOptions);
        Assert.True(setup.IsConnectionSelectionValid);

        setup.SelectedSshConfigAlias = "work.example";
        setup.SelectedSshConfigAlias = "work.example";
        Assert.Equal("work.example", setup.SshHost);
    }

    [Fact]
    public async Task ManualSshChildHasOneOwnerAndFiniteStopAndExitTransitions()
    {
        using var main = await ReadyMainAsync();
        var child = new FakeConnectionChildProcess();
        var environment = new FakeSetupConnectionEnvironment([], [], child);
        using var setup = new SetupViewModel(
            main,
            new RecordingSettingsSession(ClientSettings.Default),
            environment)
        {
            SshHost = "linux.example",
            SshUser = "salty",
        };

        Assert.Equal(Localization.LocalizationService.Current.SshReadyStatus, setup.SshStatusText);
        setup.StartOrStopSsh();
        Assert.Equal("salty@linux.example", environment.LastTarget);
        Assert.True(child.StartCalled);
        Assert.True(setup.SshRunning);
        Assert.Equal(Localization.LocalizationService.Current.SshStop, setup.SshActionText);

        setup.StartOrStopSsh();
        Assert.True(child.Killed);
        child.SignalExit();
        Assert.True(child.Disposed);
        Assert.False(setup.SshRunning);
        Assert.Equal(Localization.LocalizationService.Current.SshStart, setup.SshActionText);
    }

    [Fact]
    public async Task ManualSshCreationAndStartFailuresRemainVisibleAndRetryable()
    {
        using var main = await ReadyMainAsync();
        var failedChild = new FakeConnectionChildProcess { StartResult = false };
        var environment = new FakeSetupConnectionEnvironment([], [], failedChild);
        using var setup = new SetupViewModel(
            main,
            new RecordingSettingsSession(ClientSettings.Default),
            environment)
        {
            SshHost = "linux.example",
        };

        setup.StartOrStopSsh();
        Assert.True(failedChild.Disposed);
        Assert.Equal(Localization.LocalizationService.Current.SshLaunchFailedStatus, setup.SshStatusText);

        setup.SshHost = "linux2.example";
        Assert.Equal(Localization.LocalizationService.Current.SshReadyStatus, setup.SshStatusText);
        environment.CreateException = new InvalidOperationException();
        setup.StartOrStopSsh();
        Assert.Equal(Localization.LocalizationService.Current.SshLaunchFailedStatus, setup.SshStatusText);
    }

    [Fact]
    public void ManualSshUsesDirectArgumentListWithoutShellExecution()
    {
        var factory = new CapturingConnectionChildProcessFactory();
        var environment = new WindowsSetupConnectionEnvironment(factory);

        environment.CreateSshProcess("salty@linux.example");

        Assert.NotNull(factory.StartInfo);
        Assert.False(factory.StartInfo!.UseShellExecute);
        Assert.False(factory.StartInfo.CreateNoWindow);
        Assert.Equal("ssh.exe", factory.StartInfo.FileName);
        Assert.Equal(["-N", "-L", "8787:127.0.0.1:8787", "salty@linux.example"], factory.StartInfo.ArgumentList);
    }

    private static async Task<MainWindowViewModel> ReadyMainAsync()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var main = new MainWindowViewModel(new SingleDetailsClient(PublishedPairTestFixtures.DetailsGeneration(
            ApiState.Ready,
            now,
            true,
            "Pro",
            new ApiQuota(75, now + 604_800, 604_800, false),
            [],
            0)));
        main.Start();
        var stopwatch = Stopwatch.StartNew();
        while (!main.IsAuthenticated)
        {
            if (stopwatch.Elapsed > TimeSpan.FromSeconds(2))
            {
                main.Dispose();
                throw new TimeoutException("Ready main state was not reached.");
            }
            await Task.Delay(10);
        }
        return main;
    }

    private sealed class FakeSetupConnectionEnvironment(
        IReadOnlyList<string> aliases,
        IReadOnlyList<string> distributions,
        FakeConnectionChildProcess? child = null) : ISetupConnectionEnvironment
    {
        public Exception? CreateException { get; set; }
        public string? LastTarget { get; private set; }

        public IReadOnlyList<string> LoadSshConfigAliases() => aliases;
        public IReadOnlyList<string> LoadWslDistributions() => distributions;

        public IConnectionChildProcess CreateSshProcess(string target)
        {
            LastTarget = target;
            if (CreateException is not null) throw CreateException;
            return child ?? throw new InvalidOperationException("No child was configured.");
        }
    }

    private sealed class FakeConnectionChildProcess : IConnectionChildProcess
    {
        public event EventHandler? Exited;
        public bool StartResult { get; init; } = true;
        public bool StartCalled { get; private set; }
        public bool Killed { get; private set; }
        public bool Disposed { get; private set; }
        public bool HasExited { get; private set; }

        public bool Start()
        {
            StartCalled = true;
            return StartResult;
        }

        public void Kill()
        {
            Killed = true;
            HasExited = true;
        }

        public void WaitForExit(int milliseconds)
        {
        }

        public void SignalExit()
        {
            HasExited = true;
            Exited?.Invoke(this, EventArgs.Empty);
        }

        public void Dispose() => Disposed = true;
    }

    private sealed class RecordingSettingsSession(ClientSettings initial) : IClientSettingsSession
    {
        public ClientSettings Current { get; private set; } = initial;
        public void Save(ClientSettings settings) => Current = settings;
    }

    private sealed class CapturingConnectionChildProcessFactory : IConnectionChildProcessFactory
    {
        public ProcessStartInfo? StartInfo { get; private set; }

        public IConnectionChildProcess Create(ProcessStartInfo startInfo)
        {
            StartInfo = startInfo;
            return new FakeConnectionChildProcess();
        }
    }

    private sealed class SingleDetailsClient(ApiDetailsSnapshot snapshot) : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(DetailsFetchResult.Success(snapshot));
    }
}
