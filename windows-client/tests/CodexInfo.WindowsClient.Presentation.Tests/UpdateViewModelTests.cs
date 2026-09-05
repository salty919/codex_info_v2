// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Threading;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Updates;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class UpdateViewModelTests
{
    [Fact]
    public async Task NoUpdate_HidesNotification()
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult(null, false));
        using var viewModel = new UpdateViewModel(coordinator);

        await viewModel.StartAsync();

        Assert.False(viewModel.HasAvailableUpdate);
        Assert.False(viewModel.IsNotificationVisible);
    }

    [Fact]
    public async Task FailedBackgroundCheckIsSilent()
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult(null, true));
        using var viewModel = new UpdateViewModel(coordinator);

        await viewModel.StartAsync();

        Assert.False(viewModel.IsNotificationVisible);
        Assert.Empty(viewModel.NotificationText);
        Assert.False(viewModel.IsUpdateActionVisible);
    }

    [Fact]
    public async Task AvailableUpdate_IsAutomaticallyStartedOnUiStartup()
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult("1.2.3", false))
        {
            StartResult = UpdateStartStatus.IntegrityFailed,
        };
        using var viewModel = new UpdateViewModel(coordinator);

        await viewModel.StartAsync();

        Assert.True(viewModel.HasAvailableUpdate);
        Assert.True(viewModel.IsNotificationVisible);
        Assert.Equal(1, coordinator.StartCount);
        Assert.Equal(UpdateStartStatus.IntegrityFailed, viewModel.StartStatus);
    }

    [Fact]
    public async Task StartupConvergenceUsesTheUpdateCoordinator()
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult("1.2.3", false))
        {
            StartResult = UpdateStartStatus.Started,
        };
        using var viewModel = new UpdateViewModel(coordinator);

        await viewModel.StartAsync();

        Assert.Equal(1, coordinator.CheckCount);
        Assert.Equal(1, coordinator.StartCount);
        Assert.Equal(UpdateStartStatus.Started, viewModel.StartStatus);
    }

    [Fact]
    public async Task FailedStartupRemainsRetryableByTheCommand()
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult("1.2.3", false))
        {
            StartResult = UpdateStartStatus.IntegrityFailed,
        };
        using var viewModel = new UpdateViewModel(coordinator);
        await viewModel.StartAsync();

        await viewModel.StartAvailableUpdateAsync();

        Assert.Equal(2, coordinator.StartCount);
        Assert.Equal(UpdateStartStatus.IntegrityFailed, viewModel.StartStatus);
    }

    [Fact]
    public async Task BusyPreventsDoubleExecution()
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult("1.2.3", false));
        using var viewModel = new UpdateViewModel(coordinator);
        coordinator.StartGate = new TaskCompletionSource<UpdateStartStatus>(TaskCreationOptions.RunContinuationsAsynchronously);

        var startup = viewModel.StartAsync();
        await WaitUntilAsync(() => viewModel.IsBusy);
        viewModel.UpdateCommand.Execute(null);
        viewModel.UpdateCommand.Execute(null);
        await Task.Delay(30);

        Assert.Equal(1, coordinator.StartCount);
        Assert.True(viewModel.IsBusy);
        coordinator.StartGate.SetResult(UpdateStartStatus.Started);
        await startup;
        await WaitUntilAsync(() => !viewModel.IsBusy);
    }

    [Fact]
    public async Task StartFailureRemainsVisible()
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult("1.2.3", false))
        {
            StartResult = UpdateStartStatus.IntegrityFailed,
        };
        using var viewModel = new UpdateViewModel(coordinator);
        await viewModel.StartAsync();

        Assert.True(viewModel.IsNotificationVisible);
        Assert.True(viewModel.IsUpdateActionVisible);
        Assert.Equal(UpdateStartStatus.IntegrityFailed, viewModel.StartStatus);
        Assert.NotEmpty(viewModel.StatusText);
        Assert.Equal(CodexInfo.WindowsClient.Localization.LocalizationService.Current.Retry, viewModel.ActionText);

        await viewModel.StartAvailableUpdateAsync();
        Assert.Equal(2, coordinator.StartCount);
    }

    [Fact]
    public async Task DisposeCancelsAndIsIdempotent()
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult(null, false));
        var viewModel = new UpdateViewModel(coordinator);

        viewModel.Dispose();
        viewModel.Dispose();
        await viewModel.StartAsync();

        Assert.Equal(0, coordinator.CheckCount);
        Assert.Equal(1, coordinator.DisposeCount);
    }

    [Theory]
    [InlineData(ApiState.Ready, true, true)]
    [InlineData(ApiState.AuthRequired, false, false)]
    public async Task MainStatusBannerShowsUpdateOnlyWhenAuthenticationCtaDoesNotOwnTheSlot(
        ApiState state,
        bool authenticated,
        bool expectedUpdateVisible)
    {
        using var coordinator = new FakeCoordinator(new UpdateCheckResult("1.2.3", false))
        {
            StartResult = UpdateStartStatus.Started,
        };
        var details = PublishedPairTestFixtures.DetailsGeneration(
            state,
            DateTimeOffset.UtcNow.ToUnixTimeSeconds(),
            authenticated,
            "Pro",
            new ApiQuota(48, DateTimeOffset.UtcNow.AddDays(4).ToUnixTimeSeconds(), 604_800, false),
            [],
            0);
        using var main = new MainWindowViewModel(
            new SingleDetailsClient(DetailsFetchResult.Success(details)),
            updateCoordinator: coordinator);

        main.Start();
        await WaitUntilAsync(() => coordinator.CheckCount == 1 &&
            (main.IsAuthenticated || main.IsAuthRequired) &&
            main.Update?.StartStatus is not null &&
            !main.IsStartupLoading);

        Assert.True(
            main.IsUpdateNotificationVisible == expectedUpdateVisible,
            $"update-visible={main.IsUpdateNotificationVisible}, update-status={main.Update?.StartStatus}, update-notice={main.Update?.IsNotificationVisible}, startup={main.IsStartupLoading}, auth-required={main.IsAuthRequired}, details={main.HasDetails}, state={main.StatusTitle}");
        Assert.False(main.IsUpdateActionVisible);
        Assert.Equal(authenticated && !expectedUpdateVisible, main.ShowLastReceived);
    }

    private static async Task WaitUntilAsync(Func<bool> predicate)
    {
        for (var index = 0; index < 100 && !predicate(); index++) await Task.Delay(10);
        Assert.True(predicate());
    }

    private sealed class FakeCoordinator(UpdateCheckResult checkResult) : IWindowsUpdateCoordinator
    {
        public int CheckCount { get; private set; }
        public int StartCount { get; private set; }
        public int DisposeCount { get; private set; }
        public UpdateStartStatus StartResult { get; set; } = UpdateStartStatus.NoAvailableUpdate;
        public TaskCompletionSource<UpdateStartStatus>? StartGate { get; set; }

        public Task<UpdateCheckResult> CheckAsync(CancellationToken cancellationToken = default)
        {
            CheckCount++;
            return Task.FromResult(checkResult);
        }

        public Task<UpdateStartStatus> StartAvailableUpdateAsync(CancellationToken cancellationToken = default)
        {
            StartCount++;
            return StartGate?.Task ?? Task.FromResult(StartResult);
        }

        public void Dispose() => DisposeCount++;
    }

    private sealed class SingleDetailsClient(DetailsFetchResult result) : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(result);
    }
}
