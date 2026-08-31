// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Diagnostics;
using System.Collections.Specialized;
using System.ComponentModel;
using System.Reflection;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.Updates;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class MainWindowViewModelTests
{
    private const string CanonicalPublishedPair =
        "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";
    private const string OtherPublishedPair =
        "v1:ffeeddccbbaa9988776655443322110000000000000000000000000000000002";

    [Fact]
    public void CombinedClientRequiresTheFixedHealthReadinessBoundary()
    {
        var detailsOnly = new SequenceDetailsClient(
            DetailsFetchResult.Success(DetailsSnapshot(1)));

        var exception = Assert.Throws<ArgumentException>(() => new MainWindowViewModel(detailsOnly));

        Assert.Equal("client", exception.ParamName);
    }

    [Fact]
    public async Task Startup_keeps_content_hidden_until_the_first_snapshot_is_complete()
    {
        var client = new BlockingClient();
        using var viewModel = new MainWindowViewModel(client);

        viewModel.Start();
        await EventuallyAsync(() => client.CallCount == 1);

        Assert.True(viewModel.IsStartupLoading);
        Assert.False(viewModel.ShowAuthenticatedContent);

        client.Complete(ValidSnapshot());
        await EventuallyAsync(() => viewModel.IsAuthenticated && !viewModel.IsStartupLoading);

        Assert.True(viewModel.ShowAuthenticatedContent);
    }

    [Fact]
    public async Task Startup_failure_releases_spinner_and_exposes_retry_state()
    {
        using var viewModel = new MainWindowViewModel(new SequenceClient(
            DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport)));

        viewModel.Start();
        await EventuallyAsync(() => !viewModel.IsStartupLoading);

        Assert.False(viewModel.IsStartupLoading);
        Assert.False(viewModel.ShowAuthenticatedContent);
        Assert.Equal("接続エラー", viewModel.StatusTitle);
        Assert.True(viewModel.CanRefresh);
    }

    [Fact]
    public async Task ShowLastReceivedNotifiesWhenUpdateVisibilityChangesAcrossFailureAndRecovery()
    {
        var supervisor = new RecordingSupervisor();
        var client = new SequenceClient(
            DetailsFetchResult.Success(ValidSnapshot()),
            DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport),
            DetailsFetchResult.Success(ValidSnapshot(observedAt: 3)));
        using var updates = new TestUpdateCoordinator(new UpdateCheckResult("1.2.3", false));
        using var viewModel = new MainWindowViewModel(
            client,
            connectionSupervisor: supervisor,
            updateCoordinator: updates);

        viewModel.Start();
        await EventuallyAsync(() => viewModel.IsAuthenticated && viewModel.IsUpdateNotificationVisible);
        Assert.False(viewModel.ShowLastReceived);

        var failureVisible = NewSignal<bool>();
        var recoveryHidden = NewSignal<bool>();
        PropertyChangedEventHandler handler = (_, args) =>
        {
            if (args.PropertyName != nameof(viewModel.ShowLastReceived)) return;
            if (viewModel.ShowLastReceived)
            {
                failureVisible.TrySetResult(true);
            }
            else
            {
                recoveryHidden.TrySetResult(false);
            }
        };
        viewModel.PropertyChanged += handler;
        try
        {
            viewModel.RefreshCommand.Execute(null);
            await failureVisible.Task.WaitAsync(TimeSpan.FromSeconds(2));
            Assert.True(viewModel.ShowLastReceived);
            Assert.True(viewModel.IsRetryVisible);

            viewModel.RefreshCommand.Execute(null);
            await recoveryHidden.Task.WaitAsync(TimeSpan.FromSeconds(2));
            Assert.False(viewModel.ShowLastReceived);
            Assert.True(viewModel.IsUpdateNotificationVisible);
        }
        finally
        {
            viewModel.PropertyChanged -= handler;
        }
    }

    [Fact]
    public async Task InitialFailureExposesOneRetryAndRecoversThroughOneExplicitGeneration()
    {
        var supervisor = new RecordingSupervisor();
        var client = new BlockingRetryClient(
            DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport),
            DetailsFetchResult.Success(ValidSnapshot(observedAt: 2)));
        using var viewModel = new MainWindowViewModel(client, client, supervisor);

        viewModel.Start();
        await EventuallyAsync(() => viewModel.IsRetryVisible);

        Assert.False(viewModel.IsRefreshingVisible);
        Assert.True(viewModel.IsRetryVisible);
        Assert.True(viewModel.RefreshCommand.CanExecute(null));

        viewModel.RetryCommand.Execute(null);
        await EventuallyAsync(() => supervisor.RestartExplicitCount == 1 && client.CallCount == 2);

        Assert.False(viewModel.IsRetryVisible);
        Assert.True(viewModel.IsRefreshingVisible);
        client.CompleteSecond();
        await EventuallyAsync(() => viewModel.IsAuthenticated && !viewModel.IsRefreshingVisible);

        Assert.False(viewModel.IsRetryVisible);
        Assert.Equal(1, supervisor.RestartExplicitCount);
        Assert.Equal(2, client.CallCount);
    }

    [Fact]
    public async Task RapidRetryAndRefreshDuplicatesAddNoExplicitGenerationOrRequest()
    {
        var supervisor = new RecordingSupervisor();
        var client = new BlockingRetryClient(
            DetailsFetchResult.Success(ValidSnapshot()),
            DetailsFetchResult.Success(ValidSnapshot(observedAt: 2)));
        using var viewModel = new MainWindowViewModel(client, client, supervisor);

        viewModel.Start();
        await EventuallyAsync(() => viewModel.IsAuthenticated);
        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => supervisor.RestartExplicitCount == 1 && client.CallCount == 2);

        viewModel.RefreshCommand.Execute(null);
        viewModel.RetryCommand.Execute(null);
        Assert.Equal(1, supervisor.RestartExplicitCount);
        Assert.Equal(2, client.CallCount);

        client.CompleteSecond();
        await EventuallyAsync(() => !viewModel.IsRefreshingVisible);
    }

    [Fact]
    public async Task InvalidSettingsLeaveLastGoodRootAndValidRestartFailureSendsNoRequest()
    {
        var supervisor = new RecordingSupervisor();
        var client = new CountingSequenceClient(DetailsFetchResult.Success(ValidSnapshot()));
        using var viewModel = new MainWindowViewModel(client, client, supervisor);

        viewModel.Start();
        await EventuallyAsync(() => viewModel.IsAuthenticated);
        var observed = viewModel.ObservedAtText;
        var remaining = viewModel.RemainingPercentValue;
        var calls = client.CallCount;

        Assert.False(viewModel.ApplyConnectionSettings(new ClientSettings("xx", true)));
        Assert.Equal(calls, client.CallCount);
        Assert.Equal(observed, viewModel.ObservedAtText);
        Assert.Equal(remaining, viewModel.RemainingPercentValue);

        supervisor.RestartOutcome = ConnectionRestartOutcome.StartFailed;
        Assert.True(viewModel.ApplyConnectionSettings(ValidWslSettings()));
        await EventuallyAsync(() => viewModel.IsRetryVisible);

        Assert.Equal(calls, client.CallCount);
        Assert.Equal(observed, viewModel.ObservedAtText);
        Assert.Equal(remaining, viewModel.RemainingPercentValue);
        Assert.True(viewModel.IsAuthenticated);
    }

    [Fact]
    public async Task LateDetailsAfterSettingsSupersessionAddsNoMutationOrNotification()
    {
        var supervisor = new RecordingSupervisor();
        var client = new BlockingSupersessionCombinedClient(ValidSnapshot(observedAt: 2));
        using var viewModel = new MainWindowViewModel(client, client, supervisor);

        viewModel.Start();
        await EventuallyAsync(() => client.CallCount == 1);
        var retiredContext = CurrentContext(viewModel);
        Assert.True(viewModel.ApplyConnectionSettings(ValidWslSettings()));
        await EventuallyAsync(() => viewModel.IsAuthenticated && client.CallCount == 2);

        var baseline = CapturePresentationState(viewModel);
        var notifications = new NotificationCounter(viewModel);
        var notificationBaseline = notifications.Capture();
        var retiredPipeline = WaitForPipelineCompletion(viewModel, retiredContext);
        client.CompleteSuperseded();
        await retiredPipeline;

        AssertPresentationState(viewModel, baseline);
        notifications.AssertUnchanged(notificationBaseline);
    }

    [Fact]
    public async Task LateDetailsAfterRetrySupersessionAddsNoMutationOrNotification()
    {
        var supervisor = new RecordingSupervisor();
        var details = new BlockingSupersessionDetailsClient(DetailsSnapshot(2.5, observedAt: 2));
        using var viewModel = new MainWindowViewModel(
            new CountingSequenceClient(
                DetailsFetchResult.Success(ValidSnapshot()),
                DetailsFetchResult.Success(ValidSnapshot(observedAt: 2))),
            details,
            supervisor);

        viewModel.Start();
        await EventuallyAsync(() => details.CallCount == 1);
        var retiredContext = CurrentContext(viewModel);
        Assert.True(viewModel.ApplyConnectionSettings(ValidWslSettings()));
        await EventuallyAsync(() => viewModel.IsAuthenticated && details.CallCount == 2);

        var baseline = CapturePresentationState(viewModel);
        var notifications = new NotificationCounter(viewModel);
        var notificationBaseline = notifications.Capture();
        var retiredPipeline = WaitForPipelineCompletion(viewModel, retiredContext);
        details.CompleteSuperseded();
        await retiredPipeline;

        AssertPresentationState(viewModel, baseline);
        notifications.AssertUnchanged(notificationBaseline);
    }

    [Theory]
    [InlineData(DisposeEndpoint.Health, DisposeResult.Success)]
    [InlineData(DisposeEndpoint.Health, DisposeResult.Failure)]
    [InlineData(DisposeEndpoint.Details, DisposeResult.Success)]
    [InlineData(DisposeEndpoint.Details, DisposeResult.Failure)]
    public Task DisposeFencesEveryEndpointAndResultWithPipelineBarriers(
        DisposeEndpoint endpoint,
        DisposeResult result) => AssertDisposeCase(endpoint, result);

    [Fact]
    public async Task StatusBannerRuntimeTableHasOnePrioritizedCtaForEveryFixedRow()
    {
        var loadingClient = new BlockingClient();
        using (var loading = new MainWindowViewModel(loadingClient))
        {
            loading.Start();
            var loadingContext = CurrentContext(loading);
            await EventuallyAsync(() => loadingClient.CallCount == 1);
            AssertCtaRow(loading, CtaRow.InitialLoading);
            loadingClient.Complete(ValidSnapshot());
            await WaitForPipelineCompletion(loading, loadingContext);
        }

        using (var initialFailure = new MainWindowViewModel(new SequenceClient(
                   DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport))))
        {
            initialFailure.Start();
            await WaitForPipelineCompletion(initialFailure, CurrentContext(initialFailure));
            AssertCtaRow(initialFailure, CtaRow.InitialFailure);
        }

        using (var periodic = new MainWindowViewModel(new CountingSequenceClient(
                   DetailsFetchResult.Success(ValidSnapshot()),
                   DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport))))
        {
            periodic.Start();
            await WaitForPipelineCompletion(periodic, CurrentContext(periodic));
            await InvokePrivateTask(periodic, "RunPeriodicRefreshAsync");
            Assert.True(periodic.ShowAuthenticatedContent);
            AssertCtaRow(periodic, CtaRow.PeriodicFailureWithLastGood);
        }

        var retryClient = new BlockingRetryClient(
            DetailsFetchResult.Success(ValidSnapshot()),
            DetailsFetchResult.Success(ValidSnapshot(observedAt: 2)));
        using (var retrying = new MainWindowViewModel(retryClient, retryClient, new RecordingSupervisor()))
        {
            retrying.Start();
            await WaitForPipelineCompletion(retrying, CurrentContext(retrying));
            retrying.RefreshCommand.Execute(null);
            await retryClient.SecondStarted.WaitAsync(TimeSpan.FromSeconds(2));
            var retryContext = CurrentContext(retrying);
            AssertCtaRow(retrying, CtaRow.Retrying);
            retryClient.CompleteSecond();
            await WaitForPipelineCompletion(retrying, retryContext);
        }

        using (var authBefore = await CreateAuthViewModelAsync(() => true))
        {
            AssertCtaRow(authBefore, CtaRow.AuthRequiredBeforeLaunch);
            authBefore.AuthCommand.Execute(null);
            await EventuallyAsync(() => authBefore.IsAuthCheckVisible);
            AssertCtaRow(authBefore, CtaRow.AuthRequiredAfterLaunch);
        }

        using (var authFailure = await CreateAuthViewModelAsync(() => false))
        {
            authFailure.AuthCommand.Execute(null);
            await EventuallyAsync(() => authFailure.IsAuthStartVisible && !authFailure.IsAuthCheckVisible);
            AssertCtaRow(authFailure, CtaRow.AuthLaunchFailure);
        }

        using (var updateCoordinator = new TestUpdateCoordinator(new UpdateCheckResult("1.2.3", false)))
        using (var update = new MainWindowViewModel(
                   new SequenceClient(DetailsFetchResult.Success(ValidSnapshot())),
                   updateCoordinator: updateCoordinator))
        {
            update.Start();
            await EventuallyAsync(() =>
                update.IsAuthenticated &&
                update.IsUpdateNotificationVisible &&
                update.IsUpdateActionVisible);
            AssertCtaRow(update, CtaRow.UpdateAvailable);
        }

        using (var ready = new MainWindowViewModel(new SequenceClient(
                   DetailsFetchResult.Success(ValidSnapshot()))))
        {
            ready.Start();
            await EventuallyAsync(() => ready.IsAuthenticated && !ready.IsStartupLoading);
            AssertCtaRow(ready, CtaRow.ReadyWithoutUpdate);
        }
    }

    [Fact]
    public async Task UpdateAvailableRuntimeContractRequiresVisiblePrimaryAction()
    {
        using var updateCoordinator = new TestUpdateCoordinator(new UpdateCheckResult("1.2.3", false));
        using var update = new MainWindowViewModel(
            new SequenceClient(DetailsFetchResult.Success(ValidSnapshot())),
            updateCoordinator: updateCoordinator);

        update.Start();
        await EventuallyAsync(() =>
            update.IsAuthenticated &&
            update.IsUpdateNotificationVisible &&
            update.IsUpdateActionVisible);

        AssertCtaRow(update, CtaRow.UpdateAvailable);
    }

    [Fact]
    public async Task ExplicitSchedulerBarrierRejectsEveryDuplicateEntryPointIndependentlyOfCommandState()
    {
        var supervisor = new RecordingSupervisor();
        var client = new ExplicitBarrierClient();
        using var viewModel = new MainWindowViewModel(
            client,
            client,
            supervisor);

        viewModel.Start();
        var initialContext = CurrentContext(viewModel);
        await WaitForPipelineCompletion(viewModel, initialContext);
        Assert.True(viewModel.RefreshCommand.CanExecute(null));

        viewModel.RefreshCommand.Execute(null);
        await client.SecondDetailsStarted.Task.WaitAsync(TimeSpan.FromSeconds(2));
        var activeContext = CurrentContext(viewModel);
        Assert.NotSame(initialContext, activeContext);
        Assert.False(viewModel.RefreshCommand.CanExecute(null));

        var refreshDuplicate = InvokePrivateTask(viewModel, "RefreshManuallyAsync");
        var authDuplicate = InvokePrivateTask(viewModel, "CheckAuthenticationAsync");
        var settingsDuplicate = Task.Run(() => viewModel.ApplyConnectionSettings(ValidWslSettings()));
        viewModel.RefreshCommand.Execute(null);
        viewModel.RetryCommand.Execute(null);

        Assert.False(await settingsDuplicate);
        await Task.WhenAll(refreshDuplicate, authDuplicate);
        Assert.Same(activeContext, CurrentContext(viewModel));
        Assert.Equal(1, supervisor.RestartExplicitCount);
        Assert.Equal(1, supervisor.ExplicitChildGenerationCount);
        Assert.Equal(1, supervisor.EnsureStartedCount);
        Assert.Equal(2, client.HealthCallCount);
        Assert.IsAssignableFrom<ILoopbackDetailsClient>(client);
        Assert.Equal(2, client.DetailsCallCount);

        client.CompleteSecond(DetailsSnapshot(2.5, observedAt: 2));
        await WaitForPipelineCompletion(viewModel, activeContext);
        Assert.Equal(2, client.DetailsCallCount);
    }

    [Fact]
    public async Task AuthCheckBarrierUsesEnsureWithoutRestartAndOneCompleteRequest()
    {
        var supervisor = new RecordingSupervisor();
        var client = new AuthCheckBarrierClient();
        using var viewModel = new MainWindowViewModel(
            client,
            client,
            supervisor,
            authenticationLauncher: () => true);

        viewModel.Start();
        var initialContext = CurrentContext(viewModel);
        await WaitForPipelineCompletion(viewModel, initialContext);
        Assert.True(viewModel.IsAuthRequired);
        viewModel.AuthCommand.Execute(null);
        await EventuallyAsync(() => viewModel.IsAuthCheckVisible);

        viewModel.CheckAuthCommand.Execute(null);
        await client.SecondDetailsStarted.Task.WaitAsync(TimeSpan.FromSeconds(2));
        var activeContext = CurrentContext(viewModel);
        var duplicate = InvokePrivateTask(viewModel, "CheckAuthenticationAsync");
        await duplicate;

        Assert.Equal(2, supervisor.EnsureStartedCount);
        Assert.Equal(0, supervisor.RestartExplicitCount);
        Assert.Equal(0, supervisor.ExplicitChildGenerationCount);
        Assert.Equal(2, client.HealthCallCount);
        Assert.IsAssignableFrom<ILoopbackDetailsClient>(client);
        Assert.Equal(2, client.DetailsCallCount);

        client.CompleteSecond(DetailsSnapshot(2.5, observedAt: 2));
        await WaitForPipelineCompletion(viewModel, activeContext);
        Assert.Equal(2, client.DetailsCallCount);
        Assert.True(viewModel.IsAuthenticated);
    }

    [Fact]
    public async Task Startup_waits_for_the_details_generation_before_publishing_content()
    {
        var details = new BlockingDetailsClient();
        using var viewModel = new MainWindowViewModel(
            new SequenceClient(DetailsFetchResult.Success(ValidSnapshot())),
            details);

        viewModel.Start();
        await EventuallyAsync(() => details.CallCount == 1);

        Assert.True(viewModel.IsStartupLoading);
        Assert.False(viewModel.IsAuthenticated);

        details.Complete(DetailsFetchResult.Success(DetailsSnapshot(1)));
        await EventuallyAsync(() => viewModel.IsAuthenticated && !viewModel.IsStartupLoading);

        Assert.True(viewModel.ShowAuthenticatedContent);
    }

    [Fact]
    public async Task ProductCompositionRequestsHealthThenOneDetailsGenerationOnly()
    {
        var healthBoundary = new CountingSequenceClient(DetailsFetchResult.Success(ValidSnapshot(observedAt: 999)));
        var details = DetailsSnapshot(2.5, observedAt: 10);
        using var viewModel = new MainWindowViewModel(
            healthBoundary,
            new SequenceDetailsClient(DetailsFetchResult.Success(details)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.ShowAuthenticatedContent);

        Assert.True(viewModel.IsAuthenticated);
        Assert.Equal(2.5, viewModel.DetailsSnapshot!.Models[0].InputDollars);
        Assert.Equal("10", viewModel.DetailsSnapshot.ObservedAt?.ToString());
        Assert.Equal(0, healthBoundary.CallCount);
    }

    [Fact]
    public async Task DetailsPublishedPairRequiresNoCrossResponseComparison()
    {
        var healthBoundary = new CountingSequenceClient(DetailsFetchResult.Success(ValidSnapshot()));
        var details = DetailsSnapshot(2.5) with { PublishedPair = PublishedPair(OtherPublishedPair) };
        using var viewModel = new MainWindowViewModel(
            healthBoundary,
            new SequenceDetailsClient(DetailsFetchResult.Success(details)));

        viewModel.Start();
        await EventuallyAsync(() => !viewModel.IsStartupLoading);

        Assert.True(viewModel.IsAuthenticated);
        Assert.True(viewModel.HasDetails);
        Assert.Equal(OtherPublishedPair, viewModel.DetailsSnapshot!.PublishedPair?.ToString());
        Assert.Equal(0, healthBoundary.CallCount);
    }

    [Theory]
    [InlineData(DetailsFetchFailure.Transport)]
    [InlineData(DetailsFetchFailure.Response)]
    public async Task DetailsFailureRetainsThePublishedPairLastGoodGeneration(DetailsFetchFailure failure)
    {
        var healthBoundary = new CountingSequenceClient(DetailsFetchResult.Success(ValidSnapshot()));
        using var viewModel = new MainWindowViewModel(
            healthBoundary,
            new SequenceDetailsClient(
                DetailsFetchResult.Success(DetailsSnapshot(2.5)),
                DetailsFetchResult.FromFailure(failure)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails);
        var lastGood = viewModel.DetailsSnapshot;
        Assert.Equal(CanonicalPublishedPair, lastGood!.PublishedPair?.ToString());

        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => viewModel.DetailsStatusAutomationText == "error");

        Assert.Same(lastGood, viewModel.DetailsSnapshot);
        Assert.Equal(CanonicalPublishedPair, viewModel.DetailsSnapshot!.PublishedPair?.ToString());
        Assert.Equal(
            failure == DetailsFetchFailure.Transport
                ? "詳細データ: 前回値を表示（接続エラー）"
                : "詳細データ: 前回値を表示（応答エラー）",
            viewModel.DetailsStatusText);
        var originalLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            LocalizationService.SetLanguage("en");
            Assert.Equal(
                failure == DetailsFetchFailure.Transport
                    ? "Details: Unavailable (Connection error)"
                    : "Details: Unavailable (Linux API error)",
                viewModel.DetailsStatusText);
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
        Assert.Equal(0, healthBoundary.CallCount);
    }

    [Fact]
    public async Task DetailsCoreIsAuthoritativeForEveryVisibleSurface()
    {
        var healthBoundary = new CountingSequenceClient(DetailsFetchResult.Success(ValidSnapshot(observedAt: 2)));
        var details = DetailsSnapshot(2.5, observedAt: 3);
        using var viewModel = new MainWindowViewModel(
            healthBoundary,
            new SequenceDetailsClient(DetailsFetchResult.Success(details)));

        viewModel.Start();
        await EventuallyAsync(() => !viewModel.IsStartupLoading);

        Assert.True(viewModel.IsAuthenticated);
        Assert.True(viewModel.HasDetails);
        Assert.Equal(3, viewModel.DetailsSnapshot!.ObservedAt);
        Assert.Equal(0, healthBoundary.CallCount);
    }

    [Fact]
    public async Task DetailsFailureRetainsEveryLastCompleteValue()
    {
        var oldDetails = DetailsSnapshot(1.25, observedAt: 1);
        var healthBoundary = new CountingSequenceClient(DetailsFetchResult.Success(ValidSnapshot(observedAt: 99)));
        using var viewModel = new MainWindowViewModel(
            healthBoundary,
            new SequenceDetailsClient(
                DetailsFetchResult.Success(oldDetails),
                DetailsFetchResult.FromFailure(DetailsFetchFailure.Response)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails && !viewModel.IsStartupLoading);
        var oldObserved = viewModel.ObservedAtText;
        var oldRemaining = viewModel.RemainingPercentValue;
        var oldDollars = viewModel.Models[0].InputDollarsText;

        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => viewModel.DetailsStatusAutomationText == "error");

        Assert.Equal(oldObserved, viewModel.ObservedAtText);
        Assert.Equal(oldRemaining, viewModel.RemainingPercentValue);
        Assert.Equal(oldDollars, viewModel.Models[0].InputDollarsText);
        Assert.Equal(0, healthBoundary.CallCount);
    }

    [Fact]
    public async Task InitialDetailsFailureEndsLoadingWithoutAuthenticatedContent()
    {
        using var viewModel = new MainWindowViewModel(
            new SequenceClient(DetailsFetchResult.Success(ValidSnapshot())),
            new SequenceDetailsClient(DetailsFetchResult.FromFailure(DetailsFetchFailure.Response)));

        viewModel.Start();
        await EventuallyAsync(() => !viewModel.IsStartupLoading);

        Assert.False(viewModel.ShowAuthenticatedContent);
        Assert.False(viewModel.IsAuthenticated);
        Assert.Empty(viewModel.Models);
        Assert.Equal(0, viewModel.ActiveSolCount);
        Assert.Equal("未取得", viewModel.RemainingPercentText);
        Assert.Equal("詳細データ: 未取得（応答エラー）", viewModel.DetailsStatusText);
    }

    [Fact]
    public async Task RefreshNotificationsNeverExposeMixedDetailsGenerations()
    {
        using var viewModel = new MainWindowViewModel(
            new SequenceClient(
                DetailsFetchResult.Success(ValidSnapshot(observedAt: 1, quota: new ApiQuota(98.5, 2, 604800, false))),
                DetailsFetchResult.Success(ValidSnapshot(observedAt: 86_402, quota: new ApiQuota(41, 2, 604800, false)))),
            new SequenceDetailsClient(
                DetailsFetchResult.Success(DetailsSnapshot(1.25, observedAt: 1)),
                DetailsFetchResult.Success(DetailsSnapshot(
                    2.5,
                    observedAt: 86_402,
                    quota: new ApiQuota(41, 2, 604800, false)))));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails && !viewModel.IsStartupLoading);

        var observations = new List<(string ObservedAt, double Dollars, double Remaining)>();
        void Observe() => observations.Add((
            viewModel.ObservedAtText,
            viewModel.DetailsSnapshot?.Models.FirstOrDefault()?.InputDollars ?? -1,
            viewModel.RemainingPercentValue));
        PropertyChangedEventHandler propertyHandler = (_, args) =>
        {
            if (args.PropertyName is nameof(MainWindowViewModel.ObservedAtText) or
                nameof(MainWindowViewModel.DetailsSnapshot) or
                nameof(MainWindowViewModel.RemainingPercentValue))
            {
                Observe();
            }
        };
        NotifyCollectionChangedEventHandler collectionHandler = (_, _) => Observe();
        viewModel.PropertyChanged += propertyHandler;
        ((INotifyCollectionChanged)viewModel.Models).CollectionChanged += collectionHandler;
        ((INotifyCollectionChanged)viewModel.QuotaSegments).CollectionChanged += collectionHandler;

        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => viewModel.DetailsSnapshot?.Models[0].InputDollars == 2.5);

        viewModel.PropertyChanged -= propertyHandler;
        ((INotifyCollectionChanged)viewModel.Models).CollectionChanged -= collectionHandler;
        ((INotifyCollectionChanged)viewModel.QuotaSegments).CollectionChanged -= collectionHandler;

        var completeGeneration = (
            viewModel.ObservedAtText,
            viewModel.DetailsSnapshot!.Models[0].InputDollars,
            viewModel.RemainingPercentValue);
        Assert.NotEmpty(observations);
        Assert.All(observations, observation => Assert.Equal(completeGeneration, observation));
    }

    [Fact]
    public void ApplyingSavedConnectionStartsTheSelectedWslService()
    {
        var child = new TestConnectionChildProcess();
        var factory = new TestConnectionChildProcessFactory(child);
        using var supervisor = new ConnectionSupervisor(factory);
        var client = new NeverCalledClient();
        using var viewModel = new MainWindowViewModel(client, client, supervisor);

        var settings = new ClientSettings("ja", false)
        {
            ConnectionConfigured = true,
            ConnectionProfile = ConnectionProfiles.Wsl,
            ConnectionSelector = "Ubuntu-24.04",
        };

        Assert.True(viewModel.ApplyConnectionSettings(settings));
        Assert.Single(factory.StartInfos);
        Assert.Equal(
            ["--distribution", "Ubuntu-24.04", "--", "codex_info", "--port", "8787"],
            factory.StartInfos[0].ArgumentList);
    }

    [Fact]
    public async Task NullableQuotaAndEmptyModelsHaveExplicitPresentation()
    {
        var noDataSnapshot = PublishedPairTestFixtures.DetailsGeneration(
            ApiState.Ready,
            null,
            true,
            "Pro",
            null,
            [],
            3);
        using var viewModel = new MainWindowViewModel(new SequenceClient(
            DetailsFetchResult.Success(noDataSnapshot)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == "正常");

        Assert.Equal("未取得", viewModel.RemainingPercentText);
        Assert.Equal("Pro", viewModel.PlanText);
        Assert.Equal("未取得", viewModel.ResetAtText);
        Assert.Equal("未取得", viewModel.ObservedAtText);
        Assert.True(viewModel.HasNoModels);
        Assert.Empty(viewModel.Models);
    }

    [Fact]
    public async Task TransportFailureKeepsSnapshotAndMarksItStale()
    {
        var success = DetailsFetchResult.Success(ValidSnapshot());
        var failure = DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport);
        using var viewModel = new MainWindowViewModel(new SequenceClient(success, failure));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == "正常");
        var initialRemaining = viewModel.RemainingPercentText;

        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => viewModel.StatusTitle == "接続エラー");

        Assert.Equal(initialRemaining, viewModel.RemainingPercentText);
        Assert.Contains("前回受信の値", viewModel.StatusDetail, StringComparison.Ordinal);
        Assert.Contains("現在は更新できていません", viewModel.LastReceivedText, StringComparison.Ordinal);
    }

    [Fact]
    public async Task ApiErrorIsNotPresentedAsTransportFailure()
    {
        using var viewModel = new MainWindowViewModel(new SequenceClient(
            DetailsFetchResult.Success(ValidSnapshot(state: ApiState.Error))));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == "Linux 側の取得エラー");

        Assert.DoesNotContain("更新できていません", viewModel.StatusDetail, StringComparison.Ordinal);
        Assert.Contains("接続経路", viewModel.StatusDetail, StringComparison.Ordinal);
        Assert.Equal("98.5%", viewModel.RemainingPercentText);
    }

    [Theory]
    [InlineData(2, "残量不足")]
    [InlineData(10, "残量警告")]
    public async Task ReadySnapshotWarnsAtQuotaThresholds(
        double remainingPercent,
        string expectedStatusTitle)
    {
        var quota = new ApiQuota(
            remainingPercent,
            DateTimeOffset.UtcNow.AddDays(2).ToUnixTimeSeconds(),
            604800,
            false);
        using var viewModel = new MainWindowViewModel(new SequenceClient(
            DetailsFetchResult.Success(ValidSnapshot(quota: quota))));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == expectedStatusTitle);

        Assert.Equal($"{remainingPercent:0.#}%", viewModel.RemainingPercentText);
    }

    [Fact]
    public async Task ReadySnapshotWarnsBeforeResetWhenQuotaIsNotLow()
    {
        var quota = new ApiQuota(
            98.5,
            DateTimeOffset.UtcNow.AddHours(1).ToUnixTimeSeconds(),
            604800,
            false);
        using var viewModel = new MainWindowViewModel(new SequenceClient(
            DetailsFetchResult.Success(ValidSnapshot(quota: quota))));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == "リセット警告");

        Assert.Contains("24 時間以内", viewModel.StatusDetail, StringComparison.Ordinal);
    }

    [Fact]
    public async Task QuotaDangerTakesPriorityOverResetWarning()
    {
        var quota = new ApiQuota(
            0,
            DateTimeOffset.UtcNow.AddHours(1).ToUnixTimeSeconds(),
            604800,
            false);
        using var viewModel = new MainWindowViewModel(new SequenceClient(
            DetailsFetchResult.Success(ValidSnapshot(quota: quota))));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == "残量不足");
    }

    [Theory]
    [InlineData(ApiState.Initializing, "Linux 側で準備中")]
    [InlineData(ApiState.AuthRequired, "Linux 側で認証が必要です")]
    public async Task NonReadyWireStatesHaveTheirOwnPresentation(
        ApiState state,
        string expectedStatusTitle)
    {
        var snapshot = PublishedPairTestFixtures.DetailsGeneration(state, null, false, null, null, [], 0);
        using var viewModel = new MainWindowViewModel(new SequenceClient(
            DetailsFetchResult.Success(snapshot)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == expectedStatusTitle);

        Assert.Equal("未取得", viewModel.RemainingPercentText);
        Assert.DoesNotContain("更新できていません", viewModel.StatusDetail, StringComparison.Ordinal);
        if (state == ApiState.Initializing)
        {
            Assert.Contains("自動で更新", viewModel.StatusDetail, StringComparison.Ordinal);
        }
    }

    [Fact]
    public async Task FirstTransportFailureShowsNoSyntheticValues()
    {
        using var viewModel = new MainWindowViewModel(new SequenceClient(
            DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == "接続エラー");

        Assert.Equal("未取得", viewModel.RemainingPercentText);
        Assert.Equal("前回受信: 未取得", viewModel.LastReceivedText);
        Assert.Equal("詳細データ: 未取得（接続エラー）", viewModel.DetailsStatusText);
        Assert.Contains("接続できません", viewModel.StatusDetail, StringComparison.Ordinal);
    }

    [Fact]
    public async Task HealthFailureStopsTheCycleBeforeDetailsAndKeepsValuesUnavailable()
    {
        var client = new HealthAwareClient(
            HealthFetchResult.FromFailure(HealthFetchFailure.Response),
            DetailsFetchResult.Success(ValidSnapshot()));
        using var viewModel = new MainWindowViewModel(client);

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusDetail.Contains("有効な応答", StringComparison.Ordinal));

        Assert.Equal(["health"], client.Calls);
        Assert.Equal("未取得", viewModel.RemainingPercentText);
    }

    [Fact]
    public async Task HealthyCycleRequestsHealthBeforeDetails()
    {
        var client = new HealthAwareClient(
            HealthFetchResult.Success(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version)),
            DetailsFetchResult.Success(ValidSnapshot()));
        using var viewModel = new MainWindowViewModel(client);

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == "正常");

        Assert.Equal(["health", "details"], client.Calls);
    }

    [Fact]
    public async Task ManualRefreshDoesNotQueueBehindAnActiveRequest()
    {
        var client = new BlockingClient();
        using var viewModel = new MainWindowViewModel(client);

        viewModel.Start();
        await EventuallyAsync(() => client.CallCount == 1);
        Assert.False(viewModel.CanRefresh);

        viewModel.RefreshCommand.Execute(null);
        Assert.Equal(1, client.CallCount);

        client.Complete(ValidSnapshot());
        await EventuallyAsync(() => viewModel.StatusTitle == "正常");
    }

    [Fact]
    public async Task RefreshPublishesModelAndQuotaCollectionsAsOneAtomicReset()
    {
        var firstDetails = DetailsSnapshot(1.25);
        var secondDetails = DetailsSnapshot(2.5);
        using var viewModel = new MainWindowViewModel(
            new SequenceClient(
                DetailsFetchResult.Success(ValidSnapshot()),
                DetailsFetchResult.Success(ValidSnapshot())),
            new SequenceDetailsClient(
                DetailsFetchResult.Success(firstDetails),
                DetailsFetchResult.Success(secondDetails)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails && !viewModel.IsStartupLoading);

        var modelChanges = 0;
        var quotaChanges = 0;
        NotifyCollectionChangedEventHandler modelHandler = (_, _) => modelChanges++;
        NotifyCollectionChangedEventHandler quotaHandler = (_, _) => quotaChanges++;
        ((INotifyCollectionChanged)viewModel.Models).CollectionChanged += modelHandler;
        ((INotifyCollectionChanged)viewModel.QuotaSegments).CollectionChanged += quotaHandler;

        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => viewModel.Models[0].InputDollarsText == "$2.50");

        ((INotifyCollectionChanged)viewModel.Models).CollectionChanged -= modelHandler;
        ((INotifyCollectionChanged)viewModel.QuotaSegments).CollectionChanged -= quotaHandler;
        Assert.Equal(1, modelChanges);
        Assert.Equal(1, quotaChanges);
    }

    [Fact]
    public async Task DetailsFailureKeepsTheCompleteLastDetailsGeneration()
    {
        var details = new ApiDetailsSnapshot(
            ApiState.Ready,
            1,
            true,
            "Pro",
            new ApiQuota(98.5, 2, 604800, false),
            [new ApiDetailsModelUsage("SOL", 1, 2, 3, 1.25, 0.25, 0.5)],
            3,
            [],
            [],
            [],
            "概算 $2")
        {
            PublishedPair = PublishedPair(CanonicalPublishedPair),
        };
        using var viewModel = new MainWindowViewModel(
            new SequenceClient(DetailsFetchResult.Success(ValidSnapshot()), DetailsFetchResult.Success(ValidSnapshot())),
            new SequenceDetailsClient(
                DetailsFetchResult.Success(details),
                DetailsFetchResult.FromFailure(DetailsFetchFailure.Response)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails);
        Assert.Equal("ready", viewModel.DetailsStatusAutomationText);
        Assert.Equal("$1.25", viewModel.Models[0].InputDollarsText);

        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => viewModel.StatusDetail.Contains("前回受信の値", StringComparison.Ordinal));

        Assert.True(viewModel.HasDetails);
        Assert.Equal("error", viewModel.DetailsStatusAutomationText);
        Assert.Equal("$1.25", viewModel.Models[0].InputDollarsText);
        Assert.Contains("前回受信の値", viewModel.StatusDetail, StringComparison.Ordinal);
    }

    [Fact]
    public async Task NewDetailsGenerationReplacesCoreWithoutStatusComparison()
    {
        var completeDetails = new ApiDetailsSnapshot(
            ApiState.Ready, 1, true, "Pro",
            new ApiQuota(98.5, 2, 604800, false),
            [new ApiDetailsModelUsage("SOL", 1, 2, 3, 1.25, 0.25, 0.5)],
            3, [], [], [], "概算 $2");
        completeDetails = completeDetails with
        {
            PublishedPair = PublishedPair(CanonicalPublishedPair),
        };
        var mismatchedDetails = completeDetails with { ObservedAt = 99 };
        using var viewModel = new MainWindowViewModel(
            new SequenceClient(
                DetailsFetchResult.Success(ValidSnapshot()),
                DetailsFetchResult.Success(ValidSnapshot(observedAt: 2))),
            new SequenceDetailsClient(
                DetailsFetchResult.Success(completeDetails),
                DetailsFetchResult.Success(mismatchedDetails)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails);
        var lastCompleteObservedAt = viewModel.ObservedAtText;
        var lastCompleteDollars = viewModel.Models[0].InputDollarsText;

        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => viewModel.DetailsSnapshot?.ObservedAt == 99);

        Assert.NotEqual(lastCompleteObservedAt, viewModel.ObservedAtText);
        Assert.Equal(lastCompleteDollars, viewModel.Models[0].InputDollarsText);
        Assert.Equal(99, viewModel.DetailsSnapshot!.ObservedAt);
    }

    [Fact]
    public async Task DetailsScalarActiveThreadCountRemainsAuthoritativeWhenRowsAreBounded()
    {
        var details = new ApiDetailsSnapshot(
            ApiState.Ready,
            1,
            true,
            "Pro",
            null,
            [],
            3,
            [],
            [],
            [],
            "概算 —")
        {
            PublishedPair = PublishedPair(CanonicalPublishedPair),
        };
        using var viewModel = new MainWindowViewModel(
            new SequenceClient(DetailsFetchResult.Success(details)),
            new SequenceDetailsClient(DetailsFetchResult.Success(details)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails);

        // Details rows are intentionally bounded and may contain fewer rows
        // than the scalar generation count. The summary must not invent a
        // lower count from the presentation list.
        Assert.Equal(3UL, viewModel.ActiveThreadCount);
        Assert.Contains("3", viewModel.ActiveThreadCountText, StringComparison.Ordinal);
    }

    [Fact]
    public async Task ValidAuthRequiredDetailsGenerationClearsAccountDetails()
    {
        var details = new ApiDetailsSnapshot(
            ApiState.Ready,
            1,
            true,
            "Pro",
            new ApiQuota(98.5, 2, 604800, false),
            [new ApiDetailsModelUsage("SOL", 1, 2, 3, 1.25, 0.25, 0.5)],
            3,
            [],
            [],
            [],
            "概算 $2")
        {
            PublishedPair = PublishedPair(CanonicalPublishedPair),
        };
        using var viewModel = new MainWindowViewModel(
            new SequenceClient(DetailsFetchResult.Success(ValidSnapshot())),
            new SequenceDetailsClient(
                DetailsFetchResult.Success(details),
                DetailsFetchResult.Success(new ApiDetailsSnapshot(
                    ApiState.AuthRequired,
                    null,
                    false,
                    null,
                    null,
                    [],
                    0,
                    [],
                    [],
                    [],
                    "概算 —"))));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails);
        viewModel.RefreshCommand.Execute(null);
        await EventuallyAsync(() => viewModel.StatusTitle == "Linux 側で認証が必要です");

        Assert.False(viewModel.HasDetails);
        Assert.True(viewModel.HasNoModels);
        Assert.Contains("Linux 側", viewModel.StatusDetail, StringComparison.Ordinal);
    }

    private static async Task AssertDisposeCase(
        DisposeEndpoint endpoint,
        DisposeResult result)
    {
        var client = new DisposeMatrixClient(endpoint);
        var supervisor = new RecordingSupervisor();
        using var viewModel = new MainWindowViewModel(
            client,
            client,
            supervisor);

        viewModel.Start();
        var initialContext = CurrentContext(viewModel);
        await WaitForPipelineCompletion(viewModel, initialContext);
        Assert.True(viewModel.IsAuthenticated);

        viewModel.RefreshCommand.Execute(null);
        await client.SecondEndpointStarted.WaitAsync(TimeSpan.FromSeconds(2));
        var activeContext = CurrentContext(viewModel);
        var retiredPipeline = WaitForPipelineCompletion(viewModel, activeContext);
        using var notifications = new NotificationCounter(viewModel);

        viewModel.Dispose();
        var stateAfterDispose = CapturePresentationState(viewModel);
        var notificationsAfterDispose = notifications.Capture();

        client.CompleteLate(result);
        await retiredPipeline;

        AssertPresentationState(viewModel, stateAfterDispose);
        notifications.AssertUnchanged(notificationsAfterDispose);
        Assert.False(viewModel.CanRefresh);
        Assert.False(viewModel.RefreshCommand.CanExecute(null));
        Assert.False(viewModel.AuthCommand.CanExecute(null));
        Assert.False(viewModel.CheckAuthCommand.CanExecute(null));
    }

    private static async Task<MainWindowViewModel> CreateAuthViewModelAsync(Func<bool> launcher)
    {
        var viewModel = new MainWindowViewModel(
            new SequenceClient(DetailsFetchResult.Success(
                PublishedPairTestFixtures.DetailsGeneration(
                    ApiState.AuthRequired, 1, false, null, null, [], 0))),
            authenticationLauncher: launcher);
        viewModel.Start();
        await WaitForPipelineCompletion(viewModel, CurrentContext(viewModel));
        return viewModel;
    }

    private static void AssertCtaRow(MainWindowViewModel viewModel, CtaRow row)
    {
        var flags = new[]
        {
            viewModel.IsAuthStartVisible,
            viewModel.IsAuthCheckVisible,
            viewModel.IsRetryVisible,
            viewModel.IsRefreshingVisible,
            viewModel.IsUpdateActionVisible,
        };
        var expectedIndex = row switch
        {
            CtaRow.AuthRequiredBeforeLaunch or CtaRow.AuthLaunchFailure => 0,
            CtaRow.AuthRequiredAfterLaunch => 1,
            CtaRow.InitialFailure or CtaRow.PeriodicFailureWithLastGood => 2,
            CtaRow.Retrying => 3,
            CtaRow.UpdateAvailable => 4,
            _ => -1,
        };
        Assert.Equal(expectedIndex < 0 ? 0 : 1, flags.Count(static value => value));
        if (expectedIndex >= 0)
        {
            Assert.True(flags[expectedIndex]);
            Assert.Equal(1, flags.Count(static value => value));
        }

        var refreshEnabled = row is not CtaRow.InitialLoading and not CtaRow.Retrying;
        Assert.Equal(refreshEnabled, viewModel.RefreshCommand.CanExecute(null));
        Assert.Equal(
            row is CtaRow.AuthRequiredBeforeLaunch or CtaRow.AuthLaunchFailure,
            viewModel.AuthCommand.CanExecute(null));
        Assert.Equal(
            row is CtaRow.AuthRequiredAfterLaunch,
            viewModel.CheckAuthCommand.CanExecute(null));

        if (row is CtaRow.InitialFailure or CtaRow.PeriodicFailureWithLastGood)
        {
            Assert.NotEmpty(LocalizationService.Current.Retry);
        }
        if (row is CtaRow.AuthRequiredBeforeLaunch or CtaRow.AuthLaunchFailure)
        {
            Assert.NotEmpty(LocalizationService.Current.AuthStart);
        }
        if (row == CtaRow.AuthRequiredAfterLaunch)
        {
            Assert.NotEmpty(LocalizationService.Current.AuthCheck);
        }
        if (row == CtaRow.Retrying)
        {
            Assert.Equal(LocalizationService.Current.Refreshing, viewModel.RefreshButtonText);
        }
        if (row == CtaRow.UpdateAvailable)
        {
            Assert.True(viewModel.IsUpdateNotificationVisible);
            Assert.True(viewModel.IsUpdateActionVisible);
            Assert.NotNull(viewModel.UpdateCommand);
            Assert.True(viewModel.UpdateCommand!.CanExecute(null));
            Assert.Equal(LocalizationService.Current.UpdateButtonText, viewModel.UpdateButtonText);
        }
        else
        {
            Assert.False(viewModel.IsUpdateNotificationVisible);
            Assert.False(viewModel.IsUpdateActionVisible);
        }
    }

    private static object CurrentContext(MainWindowViewModel viewModel) =>
        typeof(MainWindowViewModel)
            .GetField("currentContext", BindingFlags.Instance | BindingFlags.NonPublic)
            ?.GetValue(viewModel)
        ?? throw new InvalidOperationException("The VM did not install a generation context.");

    private static Task InvokePrivateTask(MainWindowViewModel viewModel, string methodName) =>
        (Task)(typeof(MainWindowViewModel)
            .GetMethod(methodName, BindingFlags.Instance | BindingFlags.NonPublic)
            ?.Invoke(viewModel, null)
        ?? throw new MissingMethodException(typeof(MainWindowViewModel).FullName, methodName));

    private static async Task WaitForPipelineCompletion(
        MainWindowViewModel viewModel,
        object context)
    {
        await viewModel.GenerationPipelineCompletionAsync(context)
            .WaitAsync(TimeSpan.FromSeconds(2));
    }

    private static TaskCompletionSource<T> NewSignal<T>() =>
        new(TaskCreationOptions.RunContinuationsAsynchronously);

    private static PresentationState CapturePresentationState(MainWindowViewModel viewModel) =>
        new(
            viewModel.StatusTitle,
            viewModel.StatusDetail,
            viewModel.IsAuthenticated,
            viewModel.IsAuthRequired,
            viewModel.IsStartupLoading,
            viewModel.ShowAuthenticatedContent,
            viewModel.ShowLastReceived,
            viewModel.IsRetryVisible,
            viewModel.IsRefreshingVisible,
            viewModel.IsAuthStartVisible,
            viewModel.IsAuthCheckVisible,
            viewModel.IsUpdateNotificationVisible,
            viewModel.IsUpdateActionVisible,
            viewModel.CanRefresh,
            viewModel.RefreshCommand.CanExecute(null),
            viewModel.AuthCommand.CanExecute(null),
            viewModel.CheckAuthCommand.CanExecute(null),
            viewModel.HasDetails,
            viewModel.DetailsStatusAutomationText,
            viewModel.ObservedAtText,
            viewModel.RemainingPercentValue,
            viewModel.Models.Select(model => $"{model.Name}:{model.InputDollarsText}:{model.OutputDollarsText}").ToArray(),
            viewModel.QuotaSegments.Select(segment => segment.Fill).ToArray());

    private static void AssertPresentationState(
        MainWindowViewModel viewModel,
        PresentationState expected)
    {
        var actual = CapturePresentationState(viewModel);
        Assert.Equal(expected.StatusTitle, actual.StatusTitle);
        Assert.Equal(expected.StatusDetail, actual.StatusDetail);
        Assert.Equal(expected.IsAuthenticated, actual.IsAuthenticated);
        Assert.Equal(expected.IsAuthRequired, actual.IsAuthRequired);
        Assert.Equal(expected.IsStartupLoading, actual.IsStartupLoading);
        Assert.Equal(expected.ShowAuthenticatedContent, actual.ShowAuthenticatedContent);
        Assert.Equal(expected.ShowLastReceived, actual.ShowLastReceived);
        Assert.Equal(expected.IsRetryVisible, actual.IsRetryVisible);
        Assert.Equal(expected.IsRefreshingVisible, actual.IsRefreshingVisible);
        Assert.Equal(expected.IsAuthStartVisible, actual.IsAuthStartVisible);
        Assert.Equal(expected.IsAuthCheckVisible, actual.IsAuthCheckVisible);
        Assert.Equal(expected.IsUpdateNotificationVisible, actual.IsUpdateNotificationVisible);
        Assert.Equal(expected.IsUpdateActionVisible, actual.IsUpdateActionVisible);
        Assert.Equal(expected.CanRefresh, actual.CanRefresh);
        Assert.Equal(expected.RefreshCanExecute, actual.RefreshCanExecute);
        Assert.Equal(expected.AuthCanExecute, actual.AuthCanExecute);
        Assert.Equal(expected.CheckAuthCanExecute, actual.CheckAuthCanExecute);
        Assert.Equal(expected.HasDetails, actual.HasDetails);
        Assert.Equal(expected.DetailsStatusAutomationText, actual.DetailsStatusAutomationText);
        Assert.Equal(expected.ObservedAtText, actual.ObservedAtText);
        Assert.Equal(expected.RemainingPercentValue, actual.RemainingPercentValue);
        Assert.Equal(expected.ModelRows, actual.ModelRows);
        Assert.Equal(expected.QuotaFills, actual.QuotaFills);
    }

    private sealed record PresentationState(
        string StatusTitle,
        string StatusDetail,
        bool IsAuthenticated,
        bool IsAuthRequired,
        bool IsStartupLoading,
        bool ShowAuthenticatedContent,
        bool ShowLastReceived,
        bool IsRetryVisible,
        bool IsRefreshingVisible,
        bool IsAuthStartVisible,
        bool IsAuthCheckVisible,
        bool IsUpdateNotificationVisible,
        bool IsUpdateActionVisible,
        bool CanRefresh,
        bool RefreshCanExecute,
        bool AuthCanExecute,
        bool CheckAuthCanExecute,
        bool HasDetails,
        string DetailsStatusAutomationText,
        string ObservedAtText,
        double RemainingPercentValue,
        string[] ModelRows,
        double[] QuotaFills);

    private sealed class NotificationCounter : IDisposable
    {
        private readonly MainWindowViewModel viewModel;
        private readonly Dictionary<string, int> properties = new(StringComparer.Ordinal);
        private int modelCollections;
        private int quotaCollections;

        public NotificationCounter(MainWindowViewModel viewModel)
        {
            this.viewModel = viewModel;
            viewModel.PropertyChanged += OnPropertyChanged;
            ((INotifyCollectionChanged)viewModel.Models).CollectionChanged += OnModelsChanged;
            ((INotifyCollectionChanged)viewModel.QuotaSegments).CollectionChanged += OnQuotaChanged;
        }

        public NotificationSnapshot Capture() => new(
            new Dictionary<string, int>(properties, StringComparer.Ordinal),
            modelCollections,
            quotaCollections);

        public void AssertUnchanged(NotificationSnapshot expected)
        {
            Assert.Equal(expected.ModelCollections, modelCollections);
            Assert.Equal(expected.QuotaCollections, quotaCollections);
            Assert.Equal(expected.Properties.Count, properties.Count);
            foreach (var (name, count) in expected.Properties)
            {
                Assert.True(properties.TryGetValue(name, out var actual), $"Unexpected property: {name}");
                Assert.Equal(count, actual);
            }
        }

        public void Dispose()
        {
            viewModel.PropertyChanged -= OnPropertyChanged;
            ((INotifyCollectionChanged)viewModel.Models).CollectionChanged -= OnModelsChanged;
            ((INotifyCollectionChanged)viewModel.QuotaSegments).CollectionChanged -= OnQuotaChanged;
        }

        private void OnPropertyChanged(object? sender, PropertyChangedEventArgs args)
        {
            var name = args.PropertyName ?? "<null>";
            properties[name] = properties.TryGetValue(name, out var count) ? count + 1 : 1;
        }

        private void OnModelsChanged(object? sender, NotifyCollectionChangedEventArgs args) => modelCollections++;

        private void OnQuotaChanged(object? sender, NotifyCollectionChangedEventArgs args) => quotaCollections++;
    }

    private sealed record NotificationSnapshot(
        Dictionary<string, int> Properties,
        int ModelCollections,
        int QuotaCollections);

    private enum CtaRow
    {
        InitialLoading,
        InitialFailure,
        PeriodicFailureWithLastGood,
        Retrying,
        AuthRequiredBeforeLaunch,
        AuthRequiredAfterLaunch,
        AuthLaunchFailure,
        UpdateAvailable,
        ReadyWithoutUpdate,
    }

    public enum DisposeEndpoint
    {
        Health,
        Details,
    }

    public enum DisposeResult
    {
        Success,
        Failure,
    }

    private static ApiDetailsSnapshot ValidSnapshot(
        ApiState state = ApiState.Ready,
        long? observedAt = 1,
        string? planLabel = "Pro",
        ApiQuota? quota = null,
        IReadOnlyList<ApiDetailsModelUsage>? models = null)
    {
        return PublishedPairTestFixtures.DetailsGeneration(
            state,
            observedAt,
            true,
            planLabel,
            quota ?? new ApiQuota(98.5, 2, 604800, false),
            models ?? [new ApiDetailsModelUsage("SOL", 1, 2, 3, 0, 0, 0)],
            3);
    }

    private static ClientSettings ValidWslSettings() => new("ja", true)
    {
        ConnectionConfigured = true,
        ConnectionProfile = ConnectionProfiles.Wsl,
        ConnectionSelector = "Ubuntu-24.04",
    };

    private static ApiDetailsSnapshot DetailsSnapshot(
        double inputDollars,
        long observedAt = 1,
        ApiQuota? quota = null) => new(
            ApiState.Ready,
            observedAt,
            true,
            "Pro",
            quota ?? new ApiQuota(98.5, 2, 604800, false),
            [new ApiDetailsModelUsage("SOL", 1, 2, 3, inputDollars, 0, 0)],
            3,
            [],
            [],
            [],
            "概算 —")
        {
            PublishedPair = PublishedPair(CanonicalPublishedPair),
        };

    private static PublishedPairIdentity PublishedPair(string value)
    {
        var method = typeof(PublishedPairIdentity).GetMethod(
            "TryCreate",
            BindingFlags.NonPublic | BindingFlags.Static);
        Assert.NotNull(method);
        var arguments = new object?[] { value, null };
        Assert.True((bool)method!.Invoke(null, arguments)!);
        return (PublishedPairIdentity)arguments[1]!;
    }

    private static async Task EventuallyAsync(Func<bool> condition)
    {
        var stopwatch = Stopwatch.StartNew();
        while (!condition())
        {
            if (stopwatch.Elapsed > TimeSpan.FromSeconds(2))
            {
                throw new TimeoutException("Expected presentation state was not reached.");
            }

            await Task.Delay(10);
        }
    }

    private sealed class SequenceClient(params DetailsFetchResult[] results) : HealthyDetailsClientBase
    {
        private int index;

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            var result = results[Math.Min(index, results.Length - 1)];
            index++;
            return Task.FromResult(result);
        }
    }

    private sealed class CountingSequenceClient(params DetailsFetchResult[] results) : HealthyDetailsClientBase
    {
        private int index;

        public int CallCount => Volatile.Read(ref index);

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref index);
            return Task.FromResult(results[Math.Min(call - 1, results.Length - 1)]);
        }
    }

    private sealed class BlockingRetryClient(
        DetailsFetchResult first,
        DetailsFetchResult second) : HealthyDetailsClientBase
    {
        private readonly TaskCompletionSource<DetailsFetchResult> secondCompletion = new(
            TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly TaskCompletionSource<object?> secondStarted = NewSignal<object?>();
        private int calls;

        public int CallCount => Volatile.Read(ref calls);
        public Task SecondStarted => secondStarted.Task;

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref calls);
            if (call == 1)
            {
                return Task.FromResult(first);
            }

            secondStarted.TrySetResult(null);
            return secondCompletion.Task;
        }

        public void CompleteSecond() => secondCompletion.TrySetResult(second);
    }

    private sealed class BlockingSupersessionCombinedClient(ApiDetailsSnapshot replacement)
        : HealthyDetailsClientBase
    {
        private readonly TaskCompletionSource<DetailsFetchResult> supersededCompletion = new(
            TaskCreationOptions.RunContinuationsAsynchronously);
        private int calls;

        public int CallCount => Volatile.Read(ref calls);

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref calls);
            return call == 1
                ? supersededCompletion.Task
                : Task.FromResult(DetailsFetchResult.Success(replacement));
        }

        public void CompleteSuperseded() => supersededCompletion.TrySetResult(
            DetailsFetchResult.Success(ValidSnapshot(observedAt: 1)));
    }

    private sealed class BlockingSupersessionDetailsClient(ApiDetailsSnapshot replacement)
        : ILoopbackDetailsClient
    {
        private readonly TaskCompletionSource<DetailsFetchResult> supersededCompletion = new(
            TaskCreationOptions.RunContinuationsAsynchronously);
        private int calls;

        public int CallCount => Volatile.Read(ref calls);

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref calls);
            return call == 1
                ? supersededCompletion.Task
                : Task.FromResult(DetailsFetchResult.Success(replacement));
        }

        public void CompleteSuperseded() => supersededCompletion.TrySetResult(
            DetailsFetchResult.Success(DetailsSnapshot(1.25, observedAt: 1)));
    }

    private sealed class BlockingHealthClient : HealthyDetailsClientBase
    {
        private readonly TaskCompletionSource<HealthFetchResult> completion = new(
            TaskCreationOptions.RunContinuationsAsynchronously);
        private int calls;

        public int CallCount => Volatile.Read(ref calls);

        public override Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref calls);
            return completion.Task;
        }

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Status must not be requested while health is blocked.");

        public void Complete(HealthFetchResult result) => completion.TrySetResult(result);
    }

    private sealed class BlockingDetailsClientIgnoringCancellation : ILoopbackDetailsClient
    {
        private readonly TaskCompletionSource<DetailsFetchResult> completion = new(
            TaskCreationOptions.RunContinuationsAsynchronously);
        private int calls;

        public int CallCount => Volatile.Read(ref calls);

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref calls);
            return completion.Task;
        }

        public void Complete(DetailsFetchResult result) => completion.TrySetResult(result);
    }

    private sealed class TestUpdateCoordinator(UpdateCheckResult result) : IWindowsUpdateCoordinator
    {
        public Task<UpdateCheckResult> CheckAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(result);

        public Task<UpdateStartStatus> StartAvailableUpdateAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(UpdateStartStatus.Started);

        public void Dispose()
        {
        }
    }

    private sealed class ExplicitBarrierClient : ILoopbackHealthClient, ILoopbackDetailsClient
    {
        private readonly TaskCompletionSource<DetailsFetchResult> secondDetails = NewSignal<DetailsFetchResult>();
        private int healthCalls;
        private int detailsCalls;

        public TaskCompletionSource<object?> SecondDetailsStarted { get; } = NewSignal<object?>();

        public int HealthCallCount => Volatile.Read(ref healthCalls);
        public int DetailsCallCount => Volatile.Read(ref detailsCalls);

        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref healthCalls);
            return Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version)));
        }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref detailsCalls);
            if (call == 1)
            {
                return Task.FromResult(DetailsFetchResult.Success(DetailsSnapshot(2.5, observedAt: call)));
            }

            SecondDetailsStarted.TrySetResult(null);
            return secondDetails.Task;
        }

        public void CompleteSecond(ApiDetailsSnapshot snapshot) =>
            secondDetails.TrySetResult(DetailsFetchResult.Success(snapshot));
    }

    private sealed class AuthCheckBarrierClient : ILoopbackHealthClient, ILoopbackDetailsClient
    {
        private readonly TaskCompletionSource<DetailsFetchResult> secondDetails = NewSignal<DetailsFetchResult>();
        private int healthCalls;
        private int detailsCalls;

        public TaskCompletionSource<object?> SecondDetailsStarted { get; } = NewSignal<object?>();

        public int HealthCallCount => Volatile.Read(ref healthCalls);
        public int DetailsCallCount => Volatile.Read(ref detailsCalls);

        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref healthCalls);
            return Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version)));
        }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref detailsCalls);
            if (call == 1)
            {
                return Task.FromResult(DetailsFetchResult.Success(new ApiDetailsSnapshot(
                    ApiState.AuthRequired,
                    1,
                    false,
                    null,
                    null,
                    [],
                    0,
                    [],
                    [],
                    [],
                    "概算 —")));
            }

            SecondDetailsStarted.TrySetResult(null);
            return secondDetails.Task;
        }

        public void CompleteSecond(ApiDetailsSnapshot snapshot) =>
            secondDetails.TrySetResult(DetailsFetchResult.Success(snapshot));
    }

    private sealed class DisposeMatrixClient(DisposeEndpoint endpoint)
        : ILoopbackHealthClient, ILoopbackDetailsClient
    {
        private readonly TaskCompletionSource<HealthFetchResult> healthGate = NewSignal<HealthFetchResult>();
        private readonly TaskCompletionSource<DetailsFetchResult> detailsGate = NewSignal<DetailsFetchResult>();
        private readonly TaskCompletionSource<object?> secondEndpointStarted = NewSignal<object?>();
        private int healthCalls;
        private int detailsCalls;

        public Task SecondEndpointStarted => secondEndpointStarted.Task;

        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref healthCalls);
            if (endpoint == DisposeEndpoint.Health && call == 2)
            {
                secondEndpointStarted.TrySetResult(null);
                return healthGate.Task;
            }

            return Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version)));
        }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref detailsCalls);
            if (endpoint == DisposeEndpoint.Details && call == 2)
            {
                secondEndpointStarted.TrySetResult(null);
                return detailsGate.Task;
            }

            return Task.FromResult(DetailsFetchResult.Success(DetailsSnapshot(1.25, observedAt: call)));
        }

        public void CompleteLate(DisposeResult result)
        {
            switch (endpoint)
            {
                case DisposeEndpoint.Health:
                    healthGate.TrySetResult(result == DisposeResult.Success
                        ? HealthFetchResult.Success(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version))
                        : HealthFetchResult.FromFailure(HealthFetchFailure.Response));
                    break;
                case DisposeEndpoint.Details:
                    detailsGate.TrySetResult(result == DisposeResult.Success
                        ? DetailsFetchResult.Success(DetailsSnapshot(2.5, observedAt: 2))
                        : DetailsFetchResult.FromFailure(DetailsFetchFailure.Response));
                    break;
            }
        }
    }

    private sealed class RecordingSupervisor : IConnectionSupervisor
    {
        public int EnsureStartedCount { get; private set; }
        public int RestartExplicitCount { get; private set; }
        public int ExplicitChildGenerationCount { get; private set; }
        public ConnectionRestartOutcome RestartOutcome { get; set; } = ConnectionRestartOutcome.Started;

        public bool EnsureStarted(ClientSettings settings)
        {
            EnsureStartedCount++;
            return true;
        }

        public ConnectionRestartOutcome RestartExplicit(ClientSettings settings)
        {
            RestartExplicitCount++;
            if (RestartOutcome == ConnectionRestartOutcome.Started)
            {
                ExplicitChildGenerationCount++;
            }
            return RestartOutcome;
        }

        public void Dispose()
        {
        }
    }

    private sealed class BlockingClient : HealthyDetailsClientBase
    {
        private readonly TaskCompletionSource<DetailsFetchResult> completion = new(
            TaskCreationOptions.RunContinuationsAsynchronously);

        public int CallCount { get; private set; }

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            CallCount++;
            return completion.Task.WaitAsync(cancellationToken);
        }

        public void Complete(ApiDetailsSnapshot snapshot)
        {
            completion.TrySetResult(DetailsFetchResult.Success(snapshot));
        }
    }

    private sealed class HealthAwareClient(
        HealthFetchResult healthResult,
        DetailsFetchResult detailsFixture) : HealthyDetailsClientBase
    {
        public List<string> Calls { get; } = [];

        public override Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default)
        {
            Calls.Add("health");
            return Task.FromResult(healthResult);
        }

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            Calls.Add("details");
            return Task.FromResult(detailsFixture);
        }
    }

    private sealed class SequenceDetailsClient(params DetailsFetchResult[] results) : ILoopbackDetailsClient
    {
        private int index;

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var result = results[Math.Min(index, results.Length - 1)];
            index++;
            return Task.FromResult(result);
        }
    }

    private sealed class BlockingDetailsClient : ILoopbackDetailsClient
    {
        private readonly TaskCompletionSource<DetailsFetchResult> completion = new(
            TaskCreationOptions.RunContinuationsAsynchronously);

        public int CallCount { get; private set; }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            CallCount++;
            return completion.Task.WaitAsync(cancellationToken);
        }

        public void Complete(DetailsFetchResult result) => completion.TrySetResult(result);
    }

    private sealed class NeverCalledClient : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("The connection start test must not depend on HTTP.");
    }

    private sealed class TestConnectionChildProcessFactory(TestConnectionChildProcess child)
        : IConnectionChildProcessFactory
    {
        public List<ProcessStartInfo> StartInfos { get; } = [];

        public IConnectionChildProcess Create(ProcessStartInfo startInfo)
        {
            StartInfos.Add(startInfo);
            return child;
        }
    }

    private sealed class TestConnectionChildProcess : IConnectionChildProcess
    {
        public event EventHandler? Exited
        {
            add { }
            remove { }
        }
        public bool HasExited { get; private set; }

        public bool Start() => true;
        public void Kill() => HasExited = true;
        public void WaitForExit(int milliseconds) { }
        public void Dispose() { }
    }
}
