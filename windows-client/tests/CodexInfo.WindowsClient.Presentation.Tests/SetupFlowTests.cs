// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Diagnostics;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class SetupFlowTests
{
    [Fact]
    public async Task AdvancePersistsConnectionThenCompletionAsTwoExplicitGenerations()
    {
        var observedAt = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var details = PublishedPairTestFixtures.DetailsGeneration(
            ApiState.Ready,
            observedAt,
            true,
            "Pro",
            new ApiQuota(75, observedAt + 604_800, 604_800, false),
            [],
            0);
        using var main = new MainWindowViewModel(new SingleDetailsClient(DetailsFetchResult.Success(details)));
        var session = new RecordingSettingsSession(ClientSettings.Default with { SettingsCorrupt = true });
        using var setup = new SetupViewModel(main, session);
        main.Start();
        await EventuallyAsync(() => main.IsAuthenticated);

        Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
        Assert.True(setup.IsAuthStep);
        Assert.Single(session.Generations);
        Assert.True(session.Current.ConnectionConfigured);
        Assert.False(session.Current.SettingsCorrupt);
        Assert.False(session.Current.SetupCompleted);

        Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
        Assert.True(setup.IsDoneStep);
        Assert.Single(session.Generations);

        Assert.Equal(SetupAdvanceOutcome.CloseRequested, setup.Advance());
        Assert.Equal(2, session.Generations.Count);
        Assert.True(session.Current.ConnectionConfigured);
        Assert.True(session.Current.SetupCompleted);
    }

    [Fact]
    public async Task InvalidConnectionCannotAdvanceOrPublishSettings()
    {
        var observedAt = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var details = PublishedPairTestFixtures.DetailsGeneration(
            ApiState.Ready,
            observedAt,
            true,
            "Pro",
            new ApiQuota(75, observedAt + 604_800, 604_800, false),
            [],
            0);
        using var main = new MainWindowViewModel(new SingleDetailsClient(DetailsFetchResult.Success(details)));
        var session = new RecordingSettingsSession(ClientSettings.Default);
        using var setup = new SetupViewModel(main, session) { SshHost = "host;whoami" };
        main.Start();
        await EventuallyAsync(() => main.IsAuthenticated);

        Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
        Assert.True(setup.IsConnectionStep);
        Assert.Empty(session.Generations);
        Assert.Equal(ClientSettings.Default, session.Current);
    }

    [Fact]
    public async Task SetupSaveFailureKeepsTheWindowOpenAndDoesNotAdvanceCompletion()
    {
        var observedAt = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var details = PublishedPairTestFixtures.DetailsGeneration(
            ApiState.Ready,
            observedAt,
            true,
            "Pro",
            new ApiQuota(75, observedAt + 604_800, 604_800, false),
            [],
            0);
        using var main = new MainWindowViewModel(new SingleDetailsClient(DetailsFetchResult.Success(details)));
        var session = new FailOnSaveNumberSession(ClientSettings.Default, 2);
        using var setup = new SetupViewModel(main, session);
        main.Start();
        await EventuallyAsync(() => main.IsAuthenticated);

        Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
        Assert.True(setup.IsAuthStep);
        Assert.Equal(1, session.SaveCalls);

        Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
        Assert.True(setup.IsDoneStep);

        Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
        Assert.True(setup.IsDoneStep);
        Assert.True(setup.SettingsSaveFailed);
        Assert.Equal(2, session.SaveCalls);
    }

    [Fact]
    public void ClientSettingsSessionPublishesOnlyAfterDurableSave()
    {
        var root = Directory.CreateTempSubdirectory("codex-info-settings-session-test");
        try
        {
            var store = new ClientSettingsStore(Path.Combine(root.FullName, "settings.json"));
            var current = ClientSettings.Default;
            var session = new ClientSettingsSession(store, () => current, value => current = value);
            var valid = current with { ConnectionConfigured = true };

            session.Save(valid);

            Assert.Equal(valid, current);
            Assert.Equal(valid, store.Load());
            var recovery = valid with { SettingsCorrupt = true };
            session.Save(recovery);
            Assert.False(current.SettingsCorrupt);
            Assert.False(store.Load().SettingsCorrupt);
            var invalid = valid with { ConnectionProfile = "invalid" };
            Assert.Throws<ArgumentException>(() => session.Save(invalid));
            Assert.Equal(valid with { SettingsCorrupt = false }, current);
        }
        finally
        {
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public void SetupLaunchPolicyRejectsNullSettings()
    {
        Assert.Throws<ArgumentNullException>(() => SetupLaunchPolicy.ShouldOpen(null!));
    }

    private static async Task EventuallyAsync(Func<bool> condition)
    {
        var stopwatch = Stopwatch.StartNew();
        while (!condition())
        {
            if (stopwatch.Elapsed > TimeSpan.FromSeconds(2))
            {
                throw new TimeoutException("Expected setup state was not reached.");
            }

            await Task.Delay(10);
        }
    }

    private sealed class RecordingSettingsSession(ClientSettings initial) : IClientSettingsSession
    {
        public ClientSettings Current { get; private set; } = initial;
        public List<ClientSettings> Generations { get; } = [];

        public void Save(ClientSettings settings)
        {
            Current = settings;
            Generations.Add(settings);
        }
    }

    private sealed class FailOnSaveNumberSession(ClientSettings initial, int failingSaveNumber) : IClientSettingsSession
    {
        public ClientSettings Current { get; private set; } = initial;
        public int SaveCalls { get; private set; }

        public void Save(ClientSettings settings)
        {
            SaveCalls++;
            if (SaveCalls == failingSaveNumber)
            {
                throw new IOException("test-only durable save failure");
            }

            Current = settings;
        }
    }

    private sealed class SingleDetailsClient(DetailsFetchResult result) : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(result);
    }
}
