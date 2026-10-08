// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections.Concurrent;
using System.Collections.Specialized;
using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.Reflection;
using System.Xml.Linq;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class DetailsPresentationCoverageTests
{
    [Fact]
    public async Task GraphWindow_EmptyStateAndLocaleChangeRemainObservable()
    {
        using var main = await StartMainAsync(CreateDetails(Array.Empty<ApiHistoryPeriod>(), Array.Empty<ApiThreadDetails>()));
        using var graph = new GraphWindowViewModel(main);

        Assert.False(graph.HasPeriods);
        Assert.False(graph.HasPoints);
        Assert.True(graph.HasNoPoints);
        Assert.Empty(graph.Points);
        Assert.False(graph.Scene.HasPoints);
        Assert.Null(graph.SelectedPeriod);
        Assert.Equal($"{graph.Texts.PeriodSelectorHeading}｜{graph.Texts.UnavailableValue}", graph.SelectedPeriodText);
        Assert.Equal(graph.Texts.GraphTokenMetric, graph.SelectedMetric);
        Assert.True(graph.IsTokensMetric);
        Assert.False(graph.IsDollarsMetric);

        var changed = new HashSet<string>();
        graph.PropertyChanged += (_, args) => changed.Add(args.PropertyName ?? string.Empty);
        var previousLanguage = LocalizationService.Current.LanguageCode;
        var nextLanguage = previousLanguage.Equals("en", StringComparison.OrdinalIgnoreCase) ? "ja" : "en";

        try
        {
            LocalizationService.SetLanguage(nextLanguage);

            Assert.Contains(nameof(GraphWindowViewModel.Texts), changed);
            Assert.Contains(nameof(GraphWindowViewModel.MetricAxisText), changed);
            Assert.Equal(graph.Texts.GraphTokenMetric, graph.SelectedMetric);
            Assert.True(graph.IsTokensMetric);
            Assert.Equal($"{graph.Texts.PeriodSelectorHeading}｜{graph.Texts.UnavailableValue}", graph.SelectedPeriodText);
        }
        finally
        {
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    [Fact]
    public async Task GraphWindow_DetailsRefreshPreservesSelectedMetricAndNotificationSilent()
    {
        var period = CreateSmallPeriod("current", 2_000_000, 2_000_120, current: false, remaining: 80, token: 100);
        using var main = await StartMainAsync(CreateDetails([period], Array.Empty<ApiThreadDetails>()));
        using var graph = new GraphWindowViewModel(main);

        await EventuallyAsync(() => main.CanRefresh);
        graph.SelectedMetric = graph.Texts.GraphDollarMetric;
        await EventuallyAsync(() =>
            !graph.IsLoading &&
            graph.Scene.Metric == GraphMetric.Dollars);
        Assert.True(graph.IsDollarsMetric);
        Assert.False(graph.IsTokensMetric);

        var changed = new List<string?>();
        graph.PropertyChanged += (_, args) => changed.Add(args.PropertyName);

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => changed.Contains(nameof(GraphWindowViewModel.SelectedPeriod)));

        Assert.Equal(GraphMetric.Dollars, graph.Scene.Metric);
        Assert.DoesNotContain(nameof(GraphWindowViewModel.SelectedMetric), changed);
        Assert.True(graph.IsDollarsMetric);
        Assert.False(graph.IsTokensMetric);

        var previousLanguage = LocalizationService.Current.LanguageCode;
        var nextLanguage = previousLanguage.Equals("en", StringComparison.OrdinalIgnoreCase) ? "ja" : "en";
        try
        {
            changed.Clear();
            LocalizationService.SetLanguage(nextLanguage);

            Assert.Contains(nameof(GraphWindowViewModel.Texts), changed);
            Assert.Equal(graph.Texts.GraphDollarMetric, graph.SelectedMetric);
            Assert.True(graph.IsDollarsMetric);
            Assert.False(graph.IsTokensMetric);
        }
        finally
        {
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    [Fact]
    public async Task GraphWindow_CurrentDataErrorIsVisibleWhilePlotIsRetainedAndClearsOnRecovery()
    {
        var period = CreateSmallPeriod("current", 2_050_000, 2_050_120, current: true, remaining: 80, token: 100);
        var ready = CreateDetails([period], Array.Empty<ApiThreadDetails>());
        var apiError = ready with { State = ApiState.Error, ObservedAt = ready.ObservedAt + 1 };
        var recovered = ready with { ObservedAt = ready.ObservedAt + 4 };
        var client = new SequenceCombinedClient(
            DetailsFetchResult.Success(ready),
            DetailsFetchResult.Success(apiError),
            DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport),
            DetailsFetchResult.FromFailure(DetailsFetchFailure.Response),
            DetailsFetchResult.Success(recovered));
        using var main = new MainWindowViewModel(client);

        main.Start();
        await EventuallyAsync(() => main.HasDetails && !main.IsStartupLoading);
        using var graph = new GraphWindowViewModel(main);
        await EventuallyAsync(() => graph.HasPoints && !graph.IsLoading);

        var acceptedPointCount = graph.Points.Count;
        var acceptedPeriodId = graph.SelectedPeriod?.Id;
        var statusNotifications = 0;
        graph.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(GraphWindowViewModel.DetailsStatusText))
            {
                statusNotifications++;
            }
        };

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => main.DetailsSnapshot?.State == ApiState.Error);

        Assert.Equal("error", main.DetailsStatusAutomationText);
        Assert.Contains(main.Texts.ApiErrorSnapshotNotice, graph.DetailsStatusText, StringComparison.Ordinal);
        Assert.True(graph.HasPoints);
        Assert.False(graph.HasBlockingLoadError);
        Assert.Equal(acceptedPointCount, graph.Points.Count);
        Assert.Equal(acceptedPeriodId, graph.SelectedPeriod?.Id);

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => graph.DetailsStatusText.Contains(main.Texts.TransportError, StringComparison.Ordinal));

        Assert.Equal(main.DetailsStatusText, graph.DetailsStatusText);
        Assert.True(graph.HasPoints);
        Assert.False(graph.HasBlockingLoadError);
        Assert.Equal(acceptedPointCount, graph.Points.Count);

        main.RefreshCommand.Execute(null);
        var responseErrorText = main.Texts.LanguageCode == "ja"
            ? "詳細データ: 前回値を表示（応答エラー）"
            : $"{main.Texts.Details}: {main.Texts.Unavailable} ({main.Texts.ApiError})";
        await EventuallyAsync(() => graph.DetailsStatusText == responseErrorText);

        Assert.Equal("error", main.DetailsStatusAutomationText);
        Assert.True(graph.HasPoints);
        Assert.False(graph.HasBlockingLoadError);
        Assert.Equal(acceptedPointCount, graph.Points.Count);

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => main.DetailsSnapshot?.ObservedAt == recovered.ObservedAt);

        Assert.Equal("ready", main.DetailsStatusAutomationText);
        Assert.Contains(main.Texts.Latest, graph.DetailsStatusText, StringComparison.Ordinal);
        Assert.True(graph.HasPoints);
        Assert.False(graph.HasBlockingLoadError);
        Assert.Equal(acceptedPointCount, graph.Points.Count);
        Assert.Equal(acceptedPeriodId, graph.SelectedPeriod?.Id);
        Assert.True(statusNotifications >= 4);
    }

    [Theory]
    [InlineData(GraphMetric.Tokens)]
    [InlineData(GraphMetric.Dollars)]
    public async Task GraphWindow_CancelledLargeBuildCannotOverwriteLatestPeriod(GraphMetric metric)
    {
        var first = CreateLargePeriod("first", 2_100_000, 2_104_600, seed: 1);
        var second = CreateLargePeriod("second", 2_110_000, 2_114_600, seed: 2);
        using var main = await StartMainAsync(CreateDetails(new[] { first, second }, Array.Empty<ApiThreadDetails>()));
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        graph.SelectedMetric = metric == GraphMetric.Dollars
            ? graph.Texts.GraphDollarMetric
            : graph.Texts.GraphTokenMetric;
        Assert.Equal(first.Id, graph.SelectedPeriod?.Id);
        graph.SelectedPeriod = second;

        await PumpUiUntilAsync(
            pendingUi,
            () => !graph.IsLoading && graph.SelectedPeriodStartAt == second.StartAt);

        Assert.False(graph.HasLoadError);
        Assert.Equal(second.Id, graph.SelectedPeriod?.Id);
        Assert.Equal(second.StartAt, graph.SelectedPeriodStartAt);
        Assert.Equal(second.EndAt, graph.SelectedPeriodEndAt);
        Assert.NotEmpty(graph.Points);
        Assert.Equal(second.EndAt, graph.Points[^1].Timestamp);
        Assert.Equal(metric, graph.Scene.Metric);
    }

    [Fact]
    public async Task GraphWindow_MissingPeriodMetricAndToggleBoundariesAreStable()
    {
        var first = CreateSmallPeriod("first", 3_000_000, 3_000_120, current: false, remaining: 80, token: 100);
        var second = CreateSmallPeriod("second", 3_001_000, 3_001_120, current: false, remaining: 40, token: 200);
        using var main = await StartMainAsync(CreateDetails(new[] { first, second }, Array.Empty<ApiThreadDetails>()));
        using var graph = new GraphWindowViewModel(main);

        Assert.Equal(graph.Texts.GraphTokenMetric, graph.SelectedMetric);
        Assert.True(graph.IsTokensMetric);
        Assert.False(graph.IsDollarsMetric);

        graph.SelectedPeriod = null;
        Assert.True(graph.HasNoPoints);
        Assert.Empty(graph.Points);
        Assert.Equal(0, graph.SelectedPeriodEndAt);
        Assert.Equal(main.SelectedAccountText, graph.SelectedAccountValueText);
        Assert.Equal($"{graph.Texts.Account}｜{graph.SelectedAccountValueText}", graph.SelectedAccountText);
        Assert.Equal(graph.Texts.UnavailableValue, graph.SelectedPeriodValueText);

        graph.SelectedPeriod = second;
        Assert.True(graph.HasPoints);
        Assert.Equal(second.Id, graph.SelectedPeriod?.Id);
        Assert.Equal($"{graph.Texts.PeriodSelectorHeading}｜{second.Label}", graph.SelectedPeriodText);
        Assert.Equal(second.Label, graph.SelectedPeriodValueText);

        graph.SelectedMetric = graph.Texts.Tokens;
        Assert.False(graph.IsDollars);
        Assert.Contains(graph.Points, point => point.SolValue == 200);
        graph.SelectedMetric = "unknown metric";
        Assert.True(graph.IsDollars);
        Assert.True(graph.IsDollarsMetric);
        Assert.False(graph.IsTokensMetric);

        graph.ShowRemaining = false;
        graph.ShowModels = false;
        graph.ShowSol = false;
        graph.ShowTerra = false;
        graph.ShowLuna = false;
        Assert.False(graph.ShowRemaining);
        Assert.False(graph.ShowModels);
        Assert.False(graph.ShowSol);
        Assert.False(graph.ShowTerra);
        Assert.False(graph.ShowLuna);

        graph.ShowRemaining = true;
        graph.ShowModels = true;
        graph.ShowSol = true;
        graph.ShowTerra = true;
        graph.ShowLuna = true;
        Assert.True(graph.ShowRemaining && graph.ShowModels && graph.ShowSol && graph.ShowTerra && graph.ShowLuna);
    }

    [Fact]
    public async Task GraphWindow_SplitResourcePublicationDoesNotRetriggerHistoryFetch()
    {
        var pair = PublishedPairIdentity.Create($"v1:{new string('a', 64)}");
        var period = CreateSmallPeriod("current", 4_000_000, 4_000_120, current: true, remaining: 80, token: 100);
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>());
        var resourceClient = new CountingHistoryResourceClient(details, period, pair);
        using var main = new MainWindowViewModel(
            new StaticCombinedClient(DetailsFetchResult.Success(details)),
            resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        Assert.True(graph.IsLoading);
        Assert.False(graph.HasNoPoints);

        ((INotifyCollectionChanged)graph.Periods).CollectionChanged += (_, _) =>
        {
            if (graph.Periods.Count > 0)
            {
                graph.SelectedPeriod = graph.Periods[0];
            }
        };

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints);
        await Task.Delay(50);

        Assert.False(graph.IsLoading);
        Assert.False(graph.HasNoPoints);
        Assert.Equal(1, resourceClient.HistoryPeriodsCalls);
        Assert.Equal(1, resourceClient.HistoryPageCalls);
        Assert.Same(graph.Periods[0], graph.SelectedPeriod);
    }

    [Fact]
    public async Task GraphWindow_PeriodSwitchKeepsLoadingDistinctFromConfirmedEmpty()
    {
        var pair = PublishedPairIdentity.Create($"v1:{new string('9', 64)}");
        var current = CreateSmallPeriod("current", 4_100_000, 4_100_120, current: true, remaining: 80, token: 100);
        var empty = new ApiHistoryPeriod("empty", 4_200_000, 4_200_120, false, "empty");
        var details = CreateDetails([current, empty], Array.Empty<ApiThreadDetails>());
        var resourceClient = new DeferredEmptyHistoryResourceClient(details, current, empty, pair);
        using var main = new MainWindowViewModel(
            new StaticCombinedClient(DetailsFetchResult.Success(details)),
            resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);
        graph.SelectedPeriod = Assert.Single(graph.Periods, period => period.Id == empty.Id);

        Assert.True(graph.IsLoading);
        Assert.False(graph.HasNoPoints);
        Assert.True(graph.HasPoints);
        Assert.False(graph.HasLoadError);
        await EventuallyAsync(() => resourceClient.EmptyRequestStarted);

        resourceClient.CompleteEmptyRequest();
        await PumpUiUntilAsync(pendingUi, () => !graph.IsLoading && graph.HasNoPoints);

        Assert.False(graph.HasLoadError);
        Assert.False(graph.HasPoints);
        Assert.False(graph.Scene.HasPoints);
        Assert.Equal(empty.Id, graph.SelectedPeriod?.Id);
    }

    [Fact]
    public async Task GraphWindow_FailedPeriodSwitchKeepsAcceptedSelectorAndSceneTogether()
    {
        var pair = PublishedPairIdentity.Create($"v1:{new string('8', 64)}");
        var current = CreateSmallPeriod("current", 4_300_000, 4_300_120, current: true, remaining: 80, token: 100);
        var rejected = new ApiHistoryPeriod("rejected", 4_400_000, 4_400_120, false, "rejected");
        var details = CreateDetails([current, rejected], Array.Empty<ApiThreadDetails>());
        var resourceClient = new DeferredEmptyHistoryResourceClient(
            details,
            current,
            rejected,
            pair,
            failEmpty: true);
        using var main = new MainWindowViewModel(
            new StaticCombinedClient(DetailsFetchResult.Success(details)),
            resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);
        var acceptedPeriod = graph.SelectedPeriod;
        var acceptedText = graph.SelectedPeriodText;
        var acceptedScene = graph.Scene;

        graph.SelectedPeriod = Assert.Single(graph.Periods, period => period.Id == rejected.Id);

        Assert.True(graph.IsLoading);
        Assert.Same(acceptedPeriod, graph.SelectedPeriod);
        Assert.Equal(acceptedText, graph.SelectedPeriodText);
        Assert.Same(acceptedScene, graph.Scene);
        await EventuallyAsync(() => resourceClient.EmptyRequestStarted);

        resourceClient.CompleteEmptyRequest();
        await PumpUiUntilAsync(pendingUi, () => graph.HasLoadError);

        Assert.False(graph.IsLoading);
        Assert.Same(acceptedPeriod, graph.SelectedPeriod);
        Assert.Equal(acceptedText, graph.SelectedPeriodText);
        Assert.Same(acceptedScene, graph.Scene);
    }

    [Fact]
    public async Task GraphWindow_NonResourceGapsAreScopedToTheSelectedReset()
    {
        var selected = CreateSmallPeriod("selected", 4_500_000, 4_500_120, current: true, remaining: 80, token: 100);
        var other = CreateSmallPeriod("other", 4_500_000, 4_600_120, current: false, remaining: 60, token: 200);
        var selectedGap = new ApiHistoryGap("selected-gap", selected.ResetAt, 4_500_030, 4_500_050, "collector");
        var otherGap = new ApiHistoryGap("other-gap", other.ResetAt, 4_500_040, 4_500_060, "collector");
        var details = CreateDetails([selected, other], Array.Empty<ApiThreadDetails>()) with
        {
            HistoryGaps = [selectedGap, otherGap],
        };
        using var main = await StartMainAsync(details);
        using var graph = new GraphWindowViewModel(main);

        var gap = Assert.Single(graph.Scene.ConfirmedGaps);
        Assert.Equal(selectedGap.StartAt, gap.StartAt);
        Assert.Equal(selectedGap.EndAt, gap.EndAt);
    }

    [Fact]
    public async Task GraphWindow_ExactStaleCursorRetriesHeadOnceAndPreservesResetStateAcrossFailures()
    {
        const long start = 6_000_000;
        var pair = PublishedPairIdentity.Create($"v1:{new string('b', 64)}");
        var samples = new[]
        {
            new ApiHistorySample(start, start + 120, 96, 1, 0, 0, 1, 0, 0),
            new ApiHistorySample(start + 60, start + 120, 95, 2, 0, 0, 2, 0, 0),
            new ApiHistorySample(start + 120, start + 120, 94, 3, 0, 0, 3, 0, 0),
        };
        var period = new ApiHistoryPeriod("cursor-period", start, start + 120, false, "cursor-period");
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>());
        var resourceClient = new CursorRecoveryHistoryResourceClient(details, period, samples, pair);
        using var main = new MainWindowViewModel(
            new StaticCombinedClient(DetailsFetchResult.Success(details)),
            resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints);
        Assert.Equal([null], resourceClient.Cursors);
        Assert.Equal(1, graph.Points[^1].SolValue);
        var initialScene = graph.Scene;

        // Saved C0 is rejected exactly, so the same cycle attempts one head
        // request. That head fails and the complete last-good scene remains.
        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(pendingUi, () => graph.HasLoadError);
        Assert.Equal([null, "C0", null], resourceClient.Cursors);
        Assert.Same(initialScene, graph.Scene);
        Assert.Equal(1, graph.Points[^1].SolValue);

        // reset-required sends no C0 on the next cycle. A complete head root
        // replaces the scene and clears reset-required with the new C1.
        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(pendingUi, () => !graph.HasLoadError && graph.Points[^1].SolValue == 2);
        Assert.Equal([null, "C0", null, null], resourceClient.Cursors);
        var recoveredScene = graph.Scene;

        // An ordinary saved-cursor failure performs no head retry and does
        // not set reset-required; the following cycle sends C1 again.
        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(pendingUi, () => graph.HasLoadError);
        Assert.Equal([null, "C0", null, null, "C1"], resourceClient.Cursors);
        Assert.Same(recoveredScene, graph.Scene);

        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(pendingUi, () => !graph.HasLoadError && graph.Points[^1].SolValue == 3);
        Assert.Equal([null, "C0", null, null, "C1", "C1"], resourceClient.Cursors);
    }

    [Fact]
    public async Task GraphWindow_PublishedPairAdvanceRealignsOnceWithoutFalseFailure()
    {
        const long start = 6_100_000;
        var firstPair = PublishedPairIdentity.Create($"v1:{new string('c', 64)}");
        var secondPair = PublishedPairIdentity.Create($"v1:{new string('d', 64)}");
        var period = new ApiHistoryPeriod("pair-period", start, start + 120, false, "pair-period");
        var firstSample = new ApiHistorySample(start, start + 120, 96, 1, 0, 0, 1, 0, 0);
        var secondSample = new ApiHistorySample(start + 60, start + 120, 95, 20, 0, 0, 20, 0, 0);
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>());
        var resourceClient = new PairTransitionHistoryResourceClient(
            details,
            period,
            firstSample,
            secondSample,
            firstPair,
            secondPair);
        using var main = new MainWindowViewModel(
            new StaticCombinedClient(DetailsFetchResult.Success(details)),
            resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints);
        Assert.Equal([null], resourceClient.Cursors);
        Assert.Equal(2, graph.Points.Count);
        Assert.Equal(firstSample.Timestamp, graph.Points[0].Timestamp);
        Assert.Equal(period.EndAt, graph.Points[^1].Timestamp);
        Assert.Null(graph.Points[^1].RemainingPercent);

        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(
            pendingUi,
            () => !graph.HasLoadError &&
                graph.Points.Any(point => point.Timestamp == secondSample.Timestamp));

        Assert.Equal([null, "A0", null], resourceClient.Cursors);
        Assert.False(graph.HasLoadError);
        Assert.Contains(graph.Points, point => point.Timestamp == firstSample.Timestamp && point.SolValue == 1);
        Assert.Contains(graph.Points, point => point.Timestamp == secondSample.Timestamp && point.SolValue == 20);
    }

    [Fact]
    public async Task GraphWindow_MainQuotaBoundaryChangeRefreshesAndFollowsCurrentWithoutStaleOverwrite()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        const long windowSeconds = 7 * 24 * 60 * 60;
        var oldResetAt = now + 300;
        var newResetAt = oldResetAt + windowSeconds;
        var oldCurrent = CreateSmallPeriod(
            "old-current",
            oldResetAt - windowSeconds,
            oldResetAt,
            current: true,
            remaining: 80,
            token: 100);
        var historical = CreateSmallPeriod(
            "historical",
            oldResetAt - 2 * windowSeconds,
            oldResetAt - windowSeconds,
            current: false,
            remaining: 70,
            token: 50);
        var newCurrent = CreateSmallPeriod(
            "new-current",
            oldResetAt,
            newResetAt,
            current: true,
            remaining: 95,
            token: 300);
        var firstPair = PublishedPairIdentity.Create($"v1:{new string('f', 64)}");
        var nextPair = PublishedPairIdentity.Create($"v1:{new string('a', 64)}");
        var initialDetails = CreateDetails([oldCurrent, historical], Array.Empty<ApiThreadDetails>()) with
        {
            Quota = new ApiQuota(80, oldResetAt, windowSeconds, false),
            PublishedPair = firstPair,
        };
        var resetDetails = initialDetails with
        {
            ObservedAt = initialDetails.ObservedAt + 1,
            Quota = new ApiQuota(100, newResetAt, windowSeconds, false),
            PublishedPair = nextPair,
        };
        var staleOldSamples = oldCurrent.Samples
            .Select((sample, index) => sample with
            {
                SolTokens = (ulong)(200 + index * 10),
            })
            .ToArray();
        var resourceClient = new DeferredQuotaBoundaryHistoryResourceClient(
            initialDetails,
            resetDetails,
            oldCurrent,
            historical,
            newCurrent,
            staleOldSamples,
            firstPair,
            nextPair);
        using var main = new MainWindowViewModel(resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        main.Start();
        await EventuallyAsync(() => main.HasDetails && !main.IsStartupLoading);
        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);
        Assert.Equal(oldCurrent.Id, graph.SelectedPeriod?.Id);
        var acceptedOldScene = graph.Scene;
        Assert.Contains(graph.Points, point => point.Timestamp == oldCurrent.Samples[1].Timestamp && point.SolValue == 110);

        var inFlightOldRefresh = StartGraphResourceRefresh(graph, oldCurrent.Id);
        await EventuallyAsync(() => resourceClient.SecondOldPageStarted);

        Assert.True(main.CanRefresh);
        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => main.DetailsSnapshot is { Quota.ResetAt: var observedResetAt } &&
            observedResetAt == newResetAt);
        resourceClient.ReleaseSecondOldPage();
        await inFlightOldRefresh;
        var currentPageStarted = resourceClient.NewCurrentPageStartedSignal;
        Assert.True(
            await Task.WhenAny(currentPageStarted, Task.Delay(TimeSpan.FromSeconds(5))) == currentPageStarted,
            "A Main quota reset/window change must trigger a current-period refresh without waiting for the timer.");

        while (pendingUi.TryDequeue(out var action))
        {
            action();
        }

        Assert.Same(acceptedOldScene, graph.Scene);
        Assert.Equal(oldCurrent.Id, graph.SelectedPeriod?.Id);

        resourceClient.ReleaseNewCurrentPage();
        await PumpUiUntilAsync(
            pendingUi,
            () => !graph.IsLoading && graph.SelectedPeriod?.Id == newCurrent.Id);

        Assert.Equal(newCurrent.Id, graph.SelectedPeriod?.Id);
        Assert.Contains(graph.Periods, period => period.Id == newCurrent.Id && period.Current);
        Assert.Contains(graph.Points, point => point.Timestamp == newCurrent.Samples[1].Timestamp && point.SolValue == 310);
        Assert.Equal(3, resourceClient.HistoryPeriodsCalls);
        Assert.Equal(3, resourceClient.HistoryPageCalls);
    }

    [Fact]
    public async Task GraphWindow_QuotaBoundaryDuringPendingWindowRequestRestartsLatestRangeAndFencesOldCandidate()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        const long windowSeconds = 7 * 24 * 60 * 60;
        var oldResetAt = now - 60;
        var newResetAt = oldResetAt + windowSeconds;
        var oldCurrent = CreateSmallPeriod(
            "window-old-current",
            oldResetAt - windowSeconds,
            oldResetAt,
            current: true,
            remaining: 80,
            token: 100);
        var historical = CreateSmallPeriod(
            "window-historical",
            oldResetAt - 2 * windowSeconds,
            oldResetAt - windowSeconds,
            current: false,
            remaining: 70,
            token: 50);
        var newCurrent = CreateSmallPeriod(
            "window-new-current",
            oldResetAt,
            newResetAt,
            current: true,
            remaining: 95,
            token: 300);
        var firstPair = PublishedPairIdentity.Create($"v1:{new string('7', 64)}");
        var nextPair = PublishedPairIdentity.Create($"v1:{new string('8', 64)}");
        var initialDetails = CreateDetails([oldCurrent, historical], Array.Empty<ApiThreadDetails>()) with
        {
            Quota = new ApiQuota(80, oldResetAt, windowSeconds, false),
            PublishedPair = firstPair,
        };
        var resetDetails = initialDetails with
        {
            ObservedAt = initialDetails.ObservedAt + 1,
            Quota = new ApiQuota(100, newResetAt, windowSeconds, false),
            PublishedPair = nextPair,
        };
        var staleOldSamples = oldCurrent.Samples
            .Select((sample, index) => sample with
            {
                SolTokens = (ulong)(800 + index * 10),
            })
            .ToArray();
        var resourceClient = new DeferredWindowQuotaBoundaryHistoryResourceClient(
            initialDetails,
            resetDetails,
            oldCurrent,
            historical,
            newCurrent,
            staleOldSamples,
            firstPair,
            nextPair);
        using var main = new MainWindowViewModel(resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        main.Start();
        await EventuallyAsync(() => main.HasDetails && !main.IsStartupLoading);
        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);
        var acceptedResetScene = graph.Scene;
        Assert.Equal(oldCurrent.Id, graph.SelectedPeriod?.Id);

        graph.SelectedTimeRange = GraphTimeRange.Last24Hours;
        await EventuallyAsync(() => resourceClient.PendingOldWindowPageStarted);

        Assert.True(main.CanRefresh);
        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => main.DetailsSnapshot is { Quota.ResetAt: var observedResetAt } &&
            observedResetAt == newResetAt);

        resourceClient.ReleasePendingOldWindowPage();
        await PumpUiUntilAsync(
            pendingUi,
            () => resourceClient.NewCurrentPageStarted || !graph.IsLoading);
        Assert.True(
            resourceClient.NewCurrentPageStarted,
            "A quota boundary must invalidate the pending window candidate and refetch the latest user range against the new current period.");
        Assert.Same(acceptedResetScene, graph.Scene);
        Assert.Equal(GraphTimeRange.ResetPeriod, graph.SelectedTimeRange);

        resourceClient.ReleaseNewCurrentPage();
        await PumpUiUntilAsync(
            pendingUi,
            () => graph.SelectedTimeRange == GraphTimeRange.Last24Hours &&
                graph.Points.Any(point => point.Timestamp == newCurrent.Samples[0].Timestamp && point.SolValue == 300));

        Assert.Equal(GraphTimeRange.Last24Hours, graph.SelectedTimeRange);
        Assert.Contains(graph.Points, point =>
            point.Timestamp == newCurrent.Samples[1].Timestamp && point.SolValue == 310);
        Assert.Equal(3, resourceClient.HistoryPeriodsCalls);
        Assert.Equal(4, resourceClient.HistoryPageCalls);
    }

    [Fact]
    public async Task GraphWindow_ManualHistoricalSelectionSurvivesMainQuotaBoundaryChange()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        const long windowSeconds = 7 * 24 * 60 * 60;
        var oldResetAt = now + 300;
        var newResetAt = oldResetAt + windowSeconds;
        var oldCurrent = CreateSmallPeriod("old-current", oldResetAt - windowSeconds, oldResetAt, true, 80, 100);
        var historical = CreateSmallPeriod("historical", oldResetAt - 2 * windowSeconds, oldResetAt - windowSeconds, false, 70, 50);
        var newCurrent = CreateSmallPeriod("new-current", oldResetAt, newResetAt, true, 95, 300);
        var firstPair = PublishedPairIdentity.Create($"v1:{new string('b', 64)}");
        var nextPair = PublishedPairIdentity.Create($"v1:{new string('c', 64)}");
        var initialDetails = CreateDetails([oldCurrent, historical], Array.Empty<ApiThreadDetails>()) with
        {
            Quota = new ApiQuota(80, oldResetAt, windowSeconds, false),
            PublishedPair = firstPair,
        };
        var resetDetails = initialDetails with
        {
            ObservedAt = initialDetails.ObservedAt + 1,
            Quota = new ApiQuota(100, newResetAt, windowSeconds, false),
            PublishedPair = nextPair,
        };
        var periodSnapshots = new[]
        {
            new ApiHistoryPeriodsSnapshot([oldCurrent, historical], firstPair),
            new ApiHistoryPeriodsSnapshot([oldCurrent, historical], firstPair),
            new ApiHistoryPeriodsSnapshot([newCurrent, oldCurrent with { Current = false }, historical], nextPair),
        };
        var resourceClient = new SequencedHistoryResourceClient(
            [initialDetails, resetDetails],
            periodSnapshots,
            new Dictionary<string, IReadOnlyList<ApiHistorySample>>(StringComparer.Ordinal)
            {
                [oldCurrent.Id] = oldCurrent.Samples,
                [historical.Id] = historical.Samples,
                [newCurrent.Id] = newCurrent.Samples,
            });
        using var main = new MainWindowViewModel(resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        main.Start();
        await EventuallyAsync(() => main.HasDetails && !main.IsStartupLoading);
        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);
        graph.SelectedPeriod = Assert.Single(graph.Periods, period => period.Id == historical.Id);
        await PumpUiUntilAsync(
            pendingUi,
            () => !graph.IsLoading && graph.SelectedPeriod?.Id == historical.Id);

        Assert.True(main.CanRefresh);
        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => main.DetailsSnapshot is { Quota.ResetAt: var observedResetAt } &&
            observedResetAt == newResetAt);
        var directoryRefreshStarted = resourceClient.ThirdHistoryPeriodsCallStarted;
        Assert.True(
            await Task.WhenAny(directoryRefreshStarted, Task.Delay(TimeSpan.FromSeconds(5))) == directoryRefreshStarted,
            "A Main quota boundary must refresh the current-period directory even while a past period is selected.");
        await PumpUiUntilAsync(
            pendingUi,
            () => !graph.IsLoading && graph.SelectedPeriod?.Id == historical.Id &&
                graph.Periods.Any(period => period.Id == newCurrent.Id));

        Assert.Equal(historical.Id, graph.SelectedPeriod?.Id);
        Assert.Contains(graph.Periods, period => period.Id == newCurrent.Id && period.Current);
        Assert.Contains(graph.Points, point => point.Timestamp == historical.Samples[1].Timestamp && point.SolValue == 60);
        Assert.Equal(3, resourceClient.HistoryPeriodsCalls);
    }

    [Fact]
    public async Task GraphWindow_SameQuotaPairAdvanceDoesNotTriggerBoundaryRefresh()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        const long windowSeconds = 7 * 24 * 60 * 60;
        var resetAt = now + 300;
        var current = CreateSmallPeriod("current", resetAt - windowSeconds, resetAt, true, 80, 100);
        var firstPair = PublishedPairIdentity.Create($"v1:{new string('d', 64)}");
        var nextPair = PublishedPairIdentity.Create($"v1:{new string('e', 64)}");
        var initialDetails = CreateDetails([current], Array.Empty<ApiThreadDetails>()) with
        {
            Quota = new ApiQuota(80, resetAt, windowSeconds, false),
            PublishedPair = firstPair,
        };
        var pairOnlyAdvance = initialDetails with
        {
            ObservedAt = initialDetails.ObservedAt + 1,
            PublishedPair = nextPair,
        };
        var resourceClient = new SequencedHistoryResourceClient(
            [initialDetails, pairOnlyAdvance],
            [new ApiHistoryPeriodsSnapshot([current], firstPair)],
            new Dictionary<string, IReadOnlyList<ApiHistorySample>>(StringComparer.Ordinal)
            {
                [current.Id] = current.Samples,
            });
        using var main = new MainWindowViewModel(resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        main.Start();
        await EventuallyAsync(() => main.HasDetails && !main.IsStartupLoading);
        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);
        Assert.Equal(1, resourceClient.HistoryPeriodsCalls);
        Assert.Equal(1, resourceClient.HistoryPageCalls);

        Assert.True(main.CanRefresh);
        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => main.DetailsSnapshot is { PublishedPair: var observedPair } &&
            observedPair == nextPair);
        await Task.Delay(100);

        var acceptedDetails = Assert.IsType<ApiDetailsSnapshot>(main.DetailsSnapshot);
        Assert.Equal(resetAt, acceptedDetails.Quota?.ResetAt);
        Assert.Equal(windowSeconds, acceptedDetails.Quota?.WindowSeconds);
        Assert.Equal(1, resourceClient.HistoryPeriodsCalls);
        Assert.Equal(1, resourceClient.HistoryPageCalls);
        Assert.Equal(current.Id, graph.SelectedPeriod?.Id);
    }

    [Fact]
    public async Task GraphWindow_VerifiedCurrentResetBaselineConnectsZeroAndFullQuotaToFirstNonzero95Percent()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        const long start = 1_800_000_000;
        const long firstObservation = start + 60;
        const long end = start + 3_600;
        var period = CreateSingleAstraPeriod("current", start, end, current: true, firstObservation, 95, 257, 42.5);
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>()) with
        {
            Quota = new ApiQuota(95, end, end - start, false),
            ObservedAt = now,
        };
        var pair = PublishedPairIdentity.Create($"v1:{new string('1', 64)}");
        var resourceClient = new CountingHistoryResourceClient(details, period, pair);
        using var main = new MainWindowViewModel(new StaticCombinedClient(DetailsFetchResult.Success(details)), resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);

        var geometry = GraphPlotProjection.PrepareGeometry(graph.Scene);
        var model = geometry.ModelLines[GraphSeries.Astra].Rising.Line;
        var quota = geometry.RemainingLines;
        Assert.Contains((double)start, model.X);
        Assert.Equal(0d, model.Y[Array.IndexOf(model.X.ToArray(), start)]);
        Assert.InRange(model.X.Max(), firstObservation - 4d, firstObservation + 4d);
        Assert.InRange(model.Y[^1], 256d, 258d);
        Assert.Contains((double)start, quota.Solid.Line.X);
        Assert.Equal(100d, quota.Solid.Line.Y[Array.IndexOf(quota.Solid.Line.X.ToArray(), start)]);
        Assert.InRange(quota.Solid.Line.X.Max(), firstObservation - 4d, firstObservation + 4d);
        Assert.InRange(quota.Solid.Line.Y[^1], 94.9d, 95.1d);
        Assert.DoesNotContain((double)start, quota.Dashed.Line.X);
        Assert.DoesNotContain((double)start, graph.Scene.Timestamps);
        Assert.Single(graph.Scene.HoverObservations);
        Assert.Equal(firstObservation, graph.Scene.HoverObservations[0].Timestamp);
        Assert.Contains(95d, graph.Scene.ObservedRemainingValues);
        Assert.DoesNotContain(100d, graph.Scene.ObservedRemainingValues);
        Assert.Contains(257d, graph.Scene.Astra);
        Assert.DoesNotContain(0d, graph.Scene.Astra);
    }

    [Fact]
    public async Task GraphWindow_VerifiedCurrentResetBaselineConnectsObservedZeroWithoutAddingRawBaselineRows()
    {
        const long start = 1_805_000_000;
        const long firstObservation = start + 60;
        const long end = start + 3_600;
        var period = CreateSingleAstraPeriod("current-zero", start, end, current: true, firstObservation, 100, 0, 0);
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>()) with
        {
            Quota = new ApiQuota(100, end, end - start, false),
        };
        var pair = PublishedPairIdentity.Create($"v1:{new string('5', 64)}");
        var resourceClient = new CountingHistoryResourceClient(details, period, pair);
        using var main = new MainWindowViewModel(new StaticCombinedClient(DetailsFetchResult.Success(details)), resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);

        var geometry = GraphPlotProjection.PrepareGeometry(graph.Scene);
        var model = geometry.ModelLines[GraphSeries.Astra].Flat.Line;
        var quota = geometry.RemainingLines.Solid.Line;
        Assert.Contains((double)start, model.X);
        Assert.All(model.Y.Where(double.IsFinite), value => Assert.Equal(0d, value));
        Assert.InRange(model.X.Max(), firstObservation - 4d, firstObservation + 4d);
        Assert.Contains((double)start, quota.X);
        Assert.All(quota.Y.Where(double.IsFinite), value => Assert.Equal(100d, value));
        Assert.InRange(quota.X.Max(), firstObservation - 4d, firstObservation + 4d);
        Assert.DoesNotContain((double)start, graph.Scene.Timestamps);
        Assert.Single(graph.Scene.HoverObservations);
        Assert.Equal(firstObservation, graph.Scene.HoverObservations[0].Timestamp);
        Assert.Contains(0d, graph.Scene.Astra);
        Assert.Contains(100d, graph.Scene.ObservedRemainingValues);
    }

    [Fact]
    public async Task GraphWindow_VerifiedCurrentResetBaselineSupportsLateFirstObservationWithoutRawZeroHundred()
    {
        const long start = 1_810_000_000;
        const long firstObservation = start + 300;
        const long end = start + 3_600;
        var period = CreateSingleAstraPeriod("late-current", start, end, current: true, firstObservation, 95, 257, 42.5);
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>()) with
        {
            Quota = new ApiQuota(95, end, end - start, false),
        };
        var pair = PublishedPairIdentity.Create($"v1:{new string('2', 64)}");
        var resourceClient = new CountingHistoryResourceClient(details, period, pair);
        using var main = new MainWindowViewModel(new StaticCombinedClient(DetailsFetchResult.Success(details)), resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);

        var geometry = GraphPlotProjection.PrepareGeometry(graph.Scene);
        var model = geometry.ModelLines[GraphSeries.Astra].Rising.Line;
        var quota = geometry.RemainingLines.Solid.Line;
        Assert.Contains((double)start, model.X);
        Assert.InRange(model.X.Max(), firstObservation - 4d, firstObservation + 4d);
        Assert.Contains((double)start, quota.X);
        Assert.InRange(quota.X.Max(), firstObservation - 4d, firstObservation + 4d);
        Assert.DoesNotContain((double)start, graph.Scene.Timestamps);
        Assert.Single(graph.Scene.HoverObservations);
        Assert.Equal(firstObservation, graph.Scene.HoverObservations[0].Timestamp);
        Assert.DoesNotContain(0d, graph.Scene.Astra);
        Assert.DoesNotContain(100d, graph.Scene.ObservedRemainingValues);
    }

    [Fact]
    public async Task GraphWindow_HistoricalAndUnknownPeriodsHaveNoResetStartBaseline()
    {
        const long start = 1_820_000_000;
        const long firstObservation = start + 300;
        const long end = start + 3_600;
        var period = CreateSingleAstraPeriod("historical", start, end, current: false, firstObservation, 95, 257, 42.5);
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>());
        var pair = PublishedPairIdentity.Create($"v1:{new string('3', 64)}");
        var resourceClient = new CountingHistoryResourceClient(details, period, pair);
        using var main = new MainWindowViewModel(new StaticCombinedClient(DetailsFetchResult.Success(details)), resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);

        var geometry = GraphPlotProjection.PrepareGeometry(graph.Scene);
        Assert.DoesNotContain((double)start, geometry.ModelLines[GraphSeries.Astra].Rising.Line.X);
        Assert.DoesNotContain((double)start, geometry.RemainingLines.Solid.Line.X);
        Assert.DoesNotContain((double)start, geometry.RemainingLines.Dashed.Line.X);
        Assert.DoesNotContain((double)start, graph.Scene.Timestamps);
        Assert.Single(graph.Scene.HoverObservations);
    }

    [Fact]
    public async Task GraphWindow_WindowViewportStartDoesNotBecomeResetBaseline()
    {
        const long start = 1_830_000_000;
        const long firstObservation = start + 300;
        const long viewportStart = start + 60;
        const long end = start + 3_600;
        var period = CreateSingleAstraPeriod("current-window", start, end, current: true, firstObservation, 95, 257, 42.5);
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>());
        var pair = PublishedPairIdentity.Create($"v1:{new string('4', 64)}");
        var resourceClient = new CountingHistoryResourceClient(details, period, pair);
        using var main = new MainWindowViewModel(new StaticCombinedClient(DetailsFetchResult.Success(details)), resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints && !graph.IsLoading);

        var viewport = GraphScene.CreateViewport(viewportStart, end, GraphMetric.Tokens, [graph.Scene]);
        var model = GraphPlotProjection.BuildViewportModelLines(viewport, GraphSeries.Astra).Rising.Line;
        var remaining = GraphPlotProjection.BuildViewportRemainingLines(viewport).Solid.Line;

        Assert.Contains((double)viewportStart, model.X);
        var modelStartIndex = model.X.ToList().IndexOf(viewportStart);
        Assert.True(model.Y[modelStartIndex] > 0d);
        Assert.Contains((double)viewportStart, remaining.X);
        var remainingStartIndex = remaining.X.ToList().IndexOf(viewportStart);
        Assert.InRange(remaining.Y[remainingStartIndex], 95d, 100d);
        Assert.True(remaining.Y[remainingStartIndex] < 100d);
        Assert.DoesNotContain((double)viewportStart, graph.Scene.Timestamps);
        Assert.DoesNotContain(100d, graph.Scene.ObservedRemainingValues);
    }

    [Fact]
    public async Task GraphWindow_DuplicateTimestampsRejectWholeCandidateAndPreserveLastGood()
    {
        const long start = 6_200_000;
        var pair = PublishedPairIdentity.Create($"v1:{new string('e', 64)}");
        var period = new ApiHistoryPeriod("duplicate-period", start, start + 180, false, "duplicate-period");
        var samples = new[]
        {
            new ApiHistorySample(start, start + 180, 96, 1, 0, 0, 1, 0, 0),
            new ApiHistorySample(start + 60, start + 180, 95, 2, 0, 0, 2, 0, 0),
        };
        var details = CreateDetails([period], Array.Empty<ApiThreadDetails>());
        var resourceClient = new DuplicateTimestampHistoryResourceClient(details, period, samples, pair);
        using var main = new MainWindowViewModel(
            new StaticCombinedClient(DetailsFetchResult.Success(details)),
            resourceClient);
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints);
        var lastGoodScene = graph.Scene;
        Assert.Equal([null], resourceClient.Cursors);

        // A continuation that repeats an already accepted timestamp is not
        // an append, even when the repeated row is byte-equivalent.
        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(pendingUi, () => graph.HasLoadError);
        Assert.Equal([null, "C0"], resourceClient.Cursors);
        Assert.Same(lastGoodScene, graph.Scene);

        // Two equivalent rows in one page are also a duplicate candidate.
        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(pendingUi, () => graph.HasLoadError);
        Assert.Equal([null, "C0", "C0"], resourceClient.Cursors);
        Assert.Same(lastGoodScene, graph.Scene);

        // Repeating the timestamp across two pages rejects the complete page
        // set atomically and leaves the prior scene and cursor untouched.
        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(pendingUi, () => graph.HasLoadError);
        Assert.Equal([null, "C0", "C0", "C0", "P1"], resourceClient.Cursors);
        Assert.Same(lastGoodScene, graph.Scene);
        Assert.Equal(2, graph.Points.Count);
        Assert.Equal(samples[0].Timestamp, graph.Points[0].Timestamp);
        Assert.Equal(period.EndAt, graph.Points[^1].Timestamp);
        Assert.Null(graph.Points[^1].RemainingPercent);
    }

    [Fact]
    public async Task ThreadsWindow_EmptyStateTracksLocalizedText()
    {
        using var main = await StartMainAsync(CreateDetails(Array.Empty<ApiHistoryPeriod>(), Array.Empty<ApiThreadDetails>()));
        using var threads = new ThreadsWindowViewModel(main);

        Assert.False(threads.HasThreads);
        Assert.True(threads.HasNoThreads);
        Assert.Equal(threads.Texts.NoRunningThreads, threads.EmptyText);

        var previousLanguage = LocalizationService.Current.LanguageCode;
        var nextLanguage = previousLanguage.Equals("en", StringComparison.OrdinalIgnoreCase) ? "ja" : "en";
        try
        {
            LocalizationService.SetLanguage(nextLanguage);
            Assert.Equal(threads.Texts.NoRunningThreads, threads.EmptyText);
        }
        finally
        {
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    [Fact]
    public async Task Issue362ThreadsWindowShowsObservedActivityStatus()
    {
        var source = new[]
        {
            new ApiThreadDetails("running", "Running", null, "SOL", "SOL", null, null, null, 1, 1, false, 0, false)
                { ActivityStatus = ApiThreadActivityStatus.Running },
            new ApiThreadDetails("stopped", "Stopped", "running", "LUNA", "LUNA", null, null, null, 1, 1, true, 1, false)
                { ActivityStatus = ApiThreadActivityStatus.Stopped },
            new ApiThreadDetails("unknown", "Unknown", null, "TERRA", "TERRA", null, null, null, 1, 1, false, 0, false)
                { ActivityStatus = ApiThreadActivityStatus.Unknown },
        };
        var snapshot = CreateDetails([], source) with
        {
            ApiVersion = "v3",
            OpenSessionThreadCount = 3,
        };
        var previousLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            LocalizationService.SetLanguage("ja");
            using var main = await StartMainAsync(snapshot);
            using var threads = new ThreadsWindowViewModel(main);

            Assert.Equal("動作中", threads.Threads.Single(item => item.Id == "running").ActivityStatusText);
            Assert.Equal("停止中", threads.Threads.Single(item => item.Id == "stopped").ActivityStatusText);
            Assert.Equal("未観測", threads.Threads.Single(item => item.Id == "unknown").ActivityStatusText);

            var window = XDocument.Parse(LoadRepositoryFile(
                "windows-client", "src", "CodexInfo.WindowsClient", "ThreadsWindow.axaml"));
            var activityStatus = window.Descendants()
                .Single(element => element.Name.LocalName == "TextBlock" &&
                    element.Attribute("Text")?.Value == "{Binding ActivityStatusText}");
            Assert.Equal("{Binding ActivityStatusHex}", activityStatus.Attribute("Foreground")?.Value);
            var foreground = typeof(ThreadItemViewModel).GetProperty("ActivityStatusHex");
            Assert.NotNull(foreground);
            Assert.Equal("#EF6A6A", foreground.GetValue(threads.Threads.Single(item => item.Id == "running")));
            Assert.Equal("#A8B7CA", foreground.GetValue(threads.Threads.Single(item => item.Id == "stopped")));
            Assert.Equal("#A8B7CA", foreground.GetValue(threads.Threads.Single(item => item.Id == "unknown")));
        }
        finally
        {
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    [Fact]
    public async Task ThreadsWindowHidesStoppedRootAndKeepsSubtrees()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var source = new[]
        {
            new ApiThreadDetails("stopped-root", "Stopped root", null, "model-stopped", "SOL", null, null, null,
                now, now, false, 0, false)
                { ActivityStatus = ApiThreadActivityStatus.Stopped },
            new ApiThreadDetails("running-root", "Running root", null, "model-running", "LUNA", null, null, null,
                now - 10, now - 10, false, 0, false)
                { ActivityStatus = ApiThreadActivityStatus.Running },
            new ApiThreadDetails("running-child", "Running child", "running-root", "model-child", "TERRA", null,
                null, null, now - 20, now - 20, true, 1, false)
                { ActivityStatus = ApiThreadActivityStatus.Running },
        };

        using var main = await StartMainAsync(CreateDetails(Array.Empty<ApiHistoryPeriod>(), source));
        using var threads = new ThreadsWindowViewModel(main);

        Assert.Equal(
            new[] { "running-root", "running-child" },
            threads.Threads.Select(item => item.Id));
        Assert.Equal([new ThreadTreeConnection(0, 1, 0)], threads.TreeConnections);
    }

    [Fact]
    public void ThreadsWindow_ViewportShowsFour96PixelRowsAndScrollsOnlyTheList()
    {
        var source = LoadRepositoryFile("windows-client", "src", "CodexInfo.WindowsClient", "ThreadsWindow.axaml");
        var document = XDocument.Parse(source);

        var window = document.Root;
        Assert.NotNull(window);
        Assert.Equal("900", window.Attribute("Width")?.Value);
        Assert.Equal("480", window.Attribute("Height")?.Value);
        Assert.Equal("900", window.Attribute("MinWidth")?.Value);
        Assert.Equal("480", window.Attribute("MinHeight")?.Value);
        Assert.Equal("900", window.Attribute("MaxWidth")?.Value);
        Assert.Equal("480", window.Attribute("MaxHeight")?.Value);
        Assert.Equal("False", window.Attribute("CanResize")?.Value);

        var cardStyle = document.Descendants()
            .Single(element => element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value == "Border.thread-card");
        var cardHeightSetter = cardStyle.Descendants()
            .Single(element => element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Height");
        Assert.Equal("84", cardHeightSetter.Attribute("Value")?.Value);

        const int cardHeight = 84;
        const int cardTopAndBottom = 12;
        const int visibleCardCount = 4;
        const int viewportHeight = (cardHeight + cardTopAndBottom) * visibleCardCount;

        var listScrollViewer = Assert.Single(document.Descendants(), element => element.Name.LocalName == "ScrollViewer");
        Assert.Equal("1", listScrollViewer.Attribute("Grid.Row")?.Value);
        Assert.Equal(viewportHeight.ToString(CultureInfo.InvariantCulture), listScrollViewer.Attribute("Height")?.Value);
        Assert.Equal("Top", listScrollViewer.Attribute("VerticalAlignment")?.Value);
        Assert.Equal("Disabled", listScrollViewer.Attribute("HorizontalScrollBarVisibility")?.Value);
        Assert.Equal("Auto", listScrollViewer.Attribute("VerticalScrollBarVisibility")?.Value);

        var card = document.Descendants()
            .Single(element => element.Name.LocalName == "Border" && element.Attribute("Classes")?.Value == "thread-card");
        Assert.Equal("80,6,16,6", card.Attribute("Margin")?.Value);
    }

    [Fact]
    public async Task ThreadsWindow_MissingFieldsOrphanAndLocaleRebuildAreBounded()
    {
        var now = DateTimeOffset.UtcNow;
        var threadsData = new[]
        {
            new ApiThreadDetails("root", "Root", null, "model-root", "", null, null, null, now.AddMinutes(-4).ToUnixTimeSeconds(), null, false, null, false),
            new ApiThreadDetails("orphan", "", "missing-parent", "fallback-model", "", 321, 55, null, null, null, true, null, true),
        };
        using var main = await StartMainAsync(CreateDetails(Array.Empty<ApiHistoryPeriod>(), threadsData));
        using var threads = new ThreadsWindowViewModel(main);

        Assert.Equal(2, threads.Threads.Count);
        Assert.All(threads.Threads, item => Assert.InRange(item.TreeDepth, 0, 3));
        Assert.Equal([0, 1], threads.TreeRootRows);

        var orphan = Assert.Single(threads.Threads, item => item.Id == "orphan");
        var root = Assert.Single(threads.Threads, item => item.Id == "root");
        Assert.False(root.ConnectedToParent);
        Assert.Contains("missing-parent", orphan.ParentText);
        Assert.Equal("fallback-model", orphan.ModelText);
        Assert.EndsWith("—", orphan.ContextText, StringComparison.Ordinal);
        Assert.Contains("321", orphan.TokenText, StringComparison.Ordinal);
        Assert.EndsWith("—", orphan.DepthText, StringComparison.Ordinal);

        var previousLanguage = LocalizationService.Current.LanguageCode;
        var nextLanguage = previousLanguage.Equals("en", StringComparison.OrdinalIgnoreCase) ? "ja" : "en";
        try
        {
            var previousOrphan = orphan;
            LocalizationService.SetLanguage(nextLanguage);
            var rebuiltOrphan = Assert.Single(threads.Threads, item => item.Id == "orphan");
            Assert.NotSame(previousOrphan, rebuiltOrphan);
            Assert.Contains(threads.Texts.SubThread, rebuiltOrphan.RoleText);
        }
        finally
        {
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    [Fact]
    public async Task ThreadsWindow_BuildsConnectionsForRootsNestedChildrenAndDistantSiblings()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var source = new[]
        {
            new ApiThreadDetails("root", "Root", null, "model-root", "ROOT", 100, 10, 100, now - 300, now - 60, false, 0, false),
            new ApiThreadDetails("child", "Child", "root", "model-child", "CHILD", 80, 8, 100, now - 240, now - 50, true, 1, false),
            new ApiThreadDetails("grandchild", "Grandchild", "child", "model-grandchild", "GRAND", 60, 6, 100, now - 180, now - 40, true, 2, false),
            new ApiThreadDetails("sibling", "Sibling", "root", "model-sibling", "SIBLING", 40, 4, 100, now - 120, now - 30, true, 1, false),
        };
        using var main = await StartMainAsync(CreateDetails(Array.Empty<ApiHistoryPeriod>(), source));
        using var threads = new ThreadsWindowViewModel(main);

        Assert.Equal(
            [
                new ThreadTreeConnection(0, 1, 0),
                new ThreadTreeConnection(1, 2, 1),
                new ThreadTreeConnection(0, 3, 0),
            ],
            threads.TreeConnections);
        Assert.Equal([0], threads.TreeRootRows);
        Assert.True(threads.TreeSurfaceHeight >= 4 * 96);
        Assert.True(threads.Threads[0].IsRootThread);
        Assert.False(threads.Threads[1].IsRootThread);
        Assert.False(threads.Threads[2].IsRootThread);
        Assert.False(threads.Threads[3].IsRootThread);
    }

    [Fact]
    public async Task ThreadsWindow_UsesDistinctModelAccentsAndKeepsUnknownNeutral()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var source = new[]
        {
            new ApiThreadDetails("sol", "SOL task", null, "gpt-sol", "SOL", null, null, null, now, null, false, 0, false),
            new ApiThreadDetails("terra", "TERRA task", null, "gpt-terra", "TERRA", null, null, null, now, null, false, 0, false),
            new ApiThreadDetails("luna", "LUNA task", null, "gpt-luna", "LUNA", null, null, null, now, null, false, 0, false),
            new ApiThreadDetails("astra", "ASTRA task", null, "gpt-astra", "ASTRA", null, null, null, now, null, false, 0, false),
            new ApiThreadDetails("unknown", "Unknown task", null, "gpt-other", "OTHER", null, null, null, now, null, false, 0, false),
        };
        using var main = await StartMainAsync(CreateDetails(Array.Empty<ApiHistoryPeriod>(), source));
        using var threads = new ThreadsWindowViewModel(main);

        Assert.Equal("#B79BFF", Assert.Single(threads.Threads, item => item.Id == "sol").ModelAccentHex);
        Assert.Equal("#71D39A", Assert.Single(threads.Threads, item => item.Id == "terra").ModelAccentHex);
        Assert.Equal("#F1B35A", Assert.Single(threads.Threads, item => item.Id == "luna").ModelAccentHex);
        Assert.Equal("#E86E9F", Assert.Single(threads.Threads, item => item.Id == "astra").ModelAccentHex);
        Assert.Equal("#A8B7CA", Assert.Single(threads.Threads, item => item.Id == "unknown").ModelAccentHex);
    }

    [Fact]
    public async Task ThreadsWindow_PreservesManySiblingConnectionsWithoutDepthCollapse()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var source = new List<ApiThreadDetails>
        {
            new("root", "Root", null, "model-root", "ROOT", null, null, null, now, null, false, 0, false),
        };
        for (var index = 1; index <= 8; index++)
        {
            source.Add(new ApiThreadDetails(
                $"child-{index}",
                $"Child {index}",
                "root",
                "model-child",
                "CHILD",
                null,
                null,
                null,
                now - index,
                null,
                true,
                1,
                false));
        }

        using var main = await StartMainAsync(CreateDetails(Array.Empty<ApiHistoryPeriod>(), source));
        using var threads = new ThreadsWindowViewModel(main);

        Assert.Equal(9, threads.Threads.Count);
        Assert.Equal(8, threads.TreeConnections.Count);
        Assert.All(threads.TreeConnections, connection =>
        {
            Assert.Equal(0, connection.ParentRow);
            Assert.InRange(connection.ChildRow, 1, 8);
        });
        Assert.Equal(9 * 96, threads.TreeSurfaceHeight);
    }

    [Fact]
    public async Task ThreadsWindow_PreservesMixedChildAndGrandchildBranches()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var source = new[]
        {
            new ApiThreadDetails("root", "Root", null, "model-root", "ROOT", null, null, null, now, null, false, 0, false),
            new ApiThreadDetails("child-a", "Child A", "root", "model-a", "A", null, null, null, now - 1, null, true, 0, false),
            new ApiThreadDetails("grand-a1", "Grandchild A1", "child-a", "model-a1", "A1", null, null, null, now - 2, null, true, 0, false),
            new ApiThreadDetails("grand-a2", "Grandchild A2", "child-a", "model-a2", "A2", null, null, null, now - 3, null, true, 0, false),
            new ApiThreadDetails("child-b", "Child B", "root", "model-b", "B", null, null, null, now - 4, null, true, 0, false),
            new ApiThreadDetails("grand-b1", "Grandchild B1", "child-b", "model-b1", "B1", null, null, null, now - 5, null, true, 0, false),
            new ApiThreadDetails("grand-b2", "Grandchild B2", "child-b", "model-b2", "B2", null, null, null, now - 6, null, true, 0, false),
            new ApiThreadDetails("child-c", "Child C", "root", "model-c", "C", null, null, null, now - 7, null, true, 0, false),
        };
        using var main = await StartMainAsync(CreateDetails(Array.Empty<ApiHistoryPeriod>(), source));
        using var threads = new ThreadsWindowViewModel(main);

        Assert.Equal(8, threads.Threads.Count);
        Assert.Equal(7, threads.TreeConnections.Count);
        Assert.Contains(threads.TreeConnections, connection => connection.ParentRow == 0 && connection.ChildRow == 1);
        Assert.Contains(threads.TreeConnections, connection => connection.ParentRow == 1 && connection.ChildRow == 2);
        Assert.Contains(threads.TreeConnections, connection => connection.ParentRow == 1 && connection.ChildRow == 3);
        Assert.Contains(threads.TreeConnections, connection => connection.ParentRow == 0 && connection.ChildRow == 4);
        Assert.Contains(threads.TreeConnections, connection => connection.ParentRow == 4 && connection.ChildRow == 5);
        Assert.Contains(threads.TreeConnections, connection => connection.ParentRow == 4 && connection.ChildRow == 6);
        Assert.Contains(threads.TreeConnections, connection => connection.ParentRow == 0 && connection.ChildRow == 7);
        Assert.All(threads.TreeConnections.Where(connection => connection.ParentRow == 0),
            connection => Assert.Equal(0, connection.ParentDepth));
        Assert.All(threads.TreeConnections.Where(connection => connection.ParentRow is 1 or 4),
            connection => Assert.Equal(1, connection.ParentDepth));
        Assert.All(threads.TreeConnections, connection => Assert.True(connection.ChildRow > connection.ParentRow));
        Assert.Equal([0], threads.TreeRootRows);
    }

    [Fact]
    public void ModelUsage_MissingMoneyLocaleAndDisposeBoundariesAreMeaningful()
    {
        var previousCulture = CultureInfo.CurrentCulture;
        var previousLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("en-US");
            LocalizationService.SetLanguage("en");

            using var usage = new ModelUsageViewModel(
                new ApiDetailsModelUsage("SOL", 12_345, 678, 9, double.NaN, double.NaN, 1.5));
            Assert.Equal("12,345", usage.InputTokensText);
            Assert.Equal("678", usage.CachedInputTokensText);
            Assert.Equal("9", usage.OutputTokensText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, usage.InputDollarsText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, usage.CachedInputDollarsText);
            Assert.Equal("$1.50", usage.OutputDollarsText);

            var changed = new HashSet<string>();
            usage.PropertyChanged += (_, args) => changed.Add(args.PropertyName ?? string.Empty);
            LocalizationService.SetLanguage("ja");
            Assert.Contains(nameof(ModelUsageViewModel.InputLabel), changed);
            Assert.Contains(nameof(ModelUsageViewModel.InputTokensText), changed);
            Assert.Contains(nameof(ModelUsageViewModel.OutputDollarsText), changed);
            Assert.NotEqual("Input", usage.InputLabel);

            usage.Dispose();
            usage.Dispose();
            changed.Clear();
            LocalizationService.SetLanguage("en");
            Assert.Empty(changed);
        }
        finally
        {
            CultureInfo.CurrentCulture = previousCulture;
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    private static ApiDetailsSnapshot CreateDetails(
        IReadOnlyList<ApiHistoryPeriod> periods,
        IReadOnlyList<ApiThreadDetails> threads)
    {
        var models = new[] { new ApiDetailsModelUsage("SOL", 100, 20, 30, 1, 1, 1) };
        return new ApiDetailsSnapshot(
            ApiState.Ready,
            DateTimeOffset.UtcNow.ToUnixTimeSeconds(),
            true,
            "Pro",
            new ApiQuota(100, DateTimeOffset.UtcNow.AddHours(1).ToUnixTimeSeconds(), 3_600, false),
            models,
            (ulong)threads.Count,
            periods,
            periods.SelectMany(period => period.Samples).ToArray(),
            threads,
            "estimated")
        {
            PublishedPair = PublishedPairTestFixtures.Canonical,
        };
    }

    private static ApiCurrentSnapshot ToCurrentSnapshot(ApiDetailsSnapshot details) =>
        new(
            details.State,
            details.ObservedAt,
            details.Authenticated,
            details.PlanLabel,
            details.Quota,
            details.Models,
            details.ActiveThreadCount,
            details.PublishedPair ?? default)
        {
            OpenSessionThreadCount = details.OpenSessionThreadCount,
            ApiVersion = details.ApiVersion,
            AccountId = details.AccountId,
        };

    private static string LoadRepositoryFile(params string[] segments)
    {
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory); directory is not null; directory = directory.Parent)
        {
            var candidate = Path.Combine([directory.FullName, .. segments]);
            if (File.Exists(candidate))
            {
                return File.ReadAllText(candidate);
            }
        }

        throw new FileNotFoundException($"Could not locate repository file: {Path.Combine(segments)}");
    }

    private static ApiHistoryPeriod CreateSmallPeriod(
        string id,
        long start,
        long end,
        bool current,
        double remaining,
        long token)
    {
        return new ApiHistoryPeriod(
            id,
            start,
            end,
            current,
            id)
        {
            Samples =
            [
                new ApiHistorySample(start + 1, end, remaining, 1, 2, 3, (ulong)token, (ulong)token + 1, (ulong)token + 2),
                new ApiHistorySample(start + 60, end, remaining - 1, 2, 3, 4, (ulong)token + 10, (ulong)token + 11, (ulong)token + 12),
            ],
        };
    }

    private static ApiHistoryPeriod CreateSingleAstraPeriod(
        string id,
        long start,
        long end,
        bool current,
        long timestamp,
        double remaining,
        ulong tokens,
        double dollars)
    {
        var sample = new ApiHistorySample(
            timestamp,
            end,
            remaining,
            null,
            null,
            null,
            null,
            null,
            null,
            ApiHistorySample.ConfirmedModelSource)
        {
            ModelsComplete = true,
            ModelSamples =
            [
                new ApiHistoryModelSample("ASTRA", null, null, null, dollars)
                {
                    TotalTokens = tokens,
                },
            ],
        };
        return new ApiHistoryPeriod(id, start, end, current, id)
        {
            ResetAt = end,
            Samples = [sample],
        };
    }

    private static ApiHistoryPeriod CreateLargePeriod(string id, long start, long end, long seed)
    {
        return new ApiHistoryPeriod(id, start, end, false, id)
        {
            Samples = Enumerable.Range(0, 2_200)
                .Select(index => new ApiHistorySample(
                    start + index * 2 + 1,
                    end,
                    100 - (index % 50),
                    seed + index,
                    seed + index + 1,
                    seed + index + 2,
                    (ulong)(seed * 100 + index),
                    (ulong)(seed * 100 + index + 1),
                    (ulong)(seed * 100 + index + 2)))
                .ToArray(),
        };
    }

    private static async Task<MainWindowViewModel> StartMainAsync(ApiDetailsSnapshot details)
    {
        var main = new MainWindowViewModel(
            new StaticCombinedClient(DetailsFetchResult.Success(details)),
            new StaticDetailsClient(DetailsFetchResult.Success(details)));
        main.Start();
        await EventuallyAsync(() => main.HasDetails);
        return main;
    }

    private static async Task PumpUiUntilAsync(ConcurrentQueue<Action> pendingUi, Func<bool> completed)
    {
        var stopwatch = Stopwatch.StartNew();
        while (!completed())
        {
            while (pendingUi.TryDequeue(out var action))
            {
                action();
            }

            if (stopwatch.Elapsed > TimeSpan.FromSeconds(10))
            {
                throw new TimeoutException("The latest graph projection was not published.");
            }

            await Task.Delay(5);
        }

        while (pendingUi.TryDequeue(out var action))
        {
            action();
        }
    }

    private static async Task EventuallyAsync(Func<bool> condition)
    {
        var stopwatch = Stopwatch.StartNew();
        while (!condition())
        {
            if (stopwatch.Elapsed > TimeSpan.FromSeconds(5))
            {
                throw new TimeoutException("The test fixture did not reach the expected details state.");
            }

            await Task.Delay(5);
        }
    }

    private static async Task RefreshGraphResourceAsync(GraphWindowViewModel graph, string periodId)
    {
        await StartGraphResourceRefresh(graph, periodId);
    }

    private static Task StartGraphResourceRefresh(GraphWindowViewModel graph, string periodId)
    {
        var method = typeof(GraphWindowViewModel).GetMethod(
            "RefreshSplitResourceAsync",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(method);
        return Assert.IsAssignableFrom<Task>(method!.Invoke(
            graph,
            [false, periodId, CancellationToken.None]));
    }

    private sealed class StaticCombinedClient(DetailsFetchResult result) : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(result);
    }

    private sealed class SequenceCombinedClient(params DetailsFetchResult[] results) : HealthyDetailsClientBase
    {
        private int calls;

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            var index = Math.Min(Interlocked.Increment(ref calls) - 1, results.Length - 1);
            return Task.FromResult(results[index]);
        }
    }

    private sealed class StaticDetailsClient(DetailsFetchResult result) : ILoopbackDetailsClient
    {
        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(result);
    }

    private sealed class SequencedHistoryResourceClient(
        IReadOnlyList<ApiDetailsSnapshot> detailSnapshots,
        IReadOnlyList<ApiHistoryPeriodsSnapshot> periodSnapshots,
        IReadOnlyDictionary<string, IReadOnlyList<ApiHistorySample>> samplesByPeriod)
        : HealthyDetailsClientBase, ILoopbackResourceClient
    {
        private readonly TaskCompletionSource thirdHistoryPeriodsCall =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private int currentCalls;
        private int historyPeriodsCalls;
        private int historyPageCalls;

        public int HistoryPeriodsCalls => Volatile.Read(ref historyPeriodsCalls);

        public int HistoryPageCalls => Volatile.Read(ref historyPageCalls);

        public Task ThirdHistoryPeriodsCallStarted => thirdHistoryPeriodsCall.Task;

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(
            CancellationToken cancellationToken = default)
            => throw new InvalidOperationException("Main must use the split current resource in this fixture.");

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default)
        {
            var index = Math.Min(Interlocked.Increment(ref currentCalls) - 1, detailSnapshots.Count - 1);
            return Task.FromResult(CurrentFetchResult.Success(ToCurrentSnapshot(detailSnapshots[index])));
        }

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            CancellationToken cancellationToken = default)
        {
            var index = Math.Min(Interlocked.Increment(ref historyPeriodsCalls) - 1, periodSnapshots.Count - 1);
            if (index >= 2)
            {
                thirdHistoryPeriodsCall.TrySetResult();
            }
            return Task.FromResult(HistoryPeriodsFetchResult.Success(periodSnapshots[index]));
        }

        public Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref historyPageCalls);
            var snapshotIndex = Math.Min(Volatile.Read(ref historyPeriodsCalls) - 1, periodSnapshots.Count - 1);
            var pair = periodSnapshots[snapshotIndex].PublishedPair;
            var samples = samplesByPeriod.TryGetValue(periodId, out var periodSamples)
                ? periodSamples
                : Array.Empty<ApiHistorySample>();
            return Task.FromResult(HistoryPageFetchResult.Success(new ApiHistoryPage(
                periodId,
                samples,
                Array.Empty<ApiHistoryGap>(),
                NextCursor: null,
                ResumeCursor: null,
                pair)));
        }

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Threads are outside this graph regression test.");
    }

    private sealed class DeferredWindowQuotaBoundaryHistoryResourceClient(
        ApiDetailsSnapshot initialDetails,
        ApiDetailsSnapshot resetDetails,
        ApiHistoryPeriod oldCurrent,
        ApiHistoryPeriod historical,
        ApiHistoryPeriod newCurrent,
        IReadOnlyList<ApiHistorySample> staleOldSamples,
        PublishedPairIdentity firstPair,
        PublishedPairIdentity nextPair)
        : HealthyDetailsClientBase, ILoopbackResourceClient
    {
        private readonly TaskCompletionSource pendingOldWindowPageStarted =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly TaskCompletionSource releasePendingOldWindowPage =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly TaskCompletionSource newCurrentPageStarted =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly TaskCompletionSource releaseNewCurrentPage =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private int currentCalls;
        private int historyPeriodsCalls;
        private int historyPageCalls;
        private int oldCurrentPageCalls;

        public int HistoryPeriodsCalls => Volatile.Read(ref historyPeriodsCalls);

        public int HistoryPageCalls => Volatile.Read(ref historyPageCalls);

        public bool PendingOldWindowPageStarted => pendingOldWindowPageStarted.Task.IsCompleted;

        public bool NewCurrentPageStarted => newCurrentPageStarted.Task.IsCompleted;

        public void ReleasePendingOldWindowPage() => releasePendingOldWindowPage.TrySetResult();

        public void ReleaseNewCurrentPage() => releaseNewCurrentPage.TrySetResult();

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(
            CancellationToken cancellationToken = default)
            => throw new InvalidOperationException("Main must use the split current resource in this fixture.");

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default)
        {
            var snapshot = Interlocked.Increment(ref currentCalls) == 1 ? initialDetails : resetDetails;
            return Task.FromResult(CurrentFetchResult.Success(ToCurrentSnapshot(snapshot)));
        }

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref historyPeriodsCalls);
            var periods = call < 3
                ? new[] { oldCurrent, historical }
                : new[] { newCurrent, oldCurrent with { Current = false }, historical };
            var pair = call < 3 ? firstPair : nextPair;
            return Task.FromResult(HistoryPeriodsFetchResult.Success(
                new ApiHistoryPeriodsSnapshot(periods, pair)));
        }

        public async Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref historyPageCalls);
            IReadOnlyList<ApiHistorySample> samples;
            PublishedPairIdentity pair;
            if (periodId == oldCurrent.Id)
            {
                var oldCall = Interlocked.Increment(ref oldCurrentPageCalls);
                if (oldCall == 1)
                {
                    samples = oldCurrent.Samples;
                    pair = firstPair;
                }
                else if (oldCall == 2)
                {
                    pendingOldWindowPageStarted.TrySetResult();
                    // Deliberately return after cancellation so the test proves
                    // that publication fencing, not transport cooperation,
                    // rejects this old generation.
                    await releasePendingOldWindowPage.Task;
                    samples = staleOldSamples;
                    pair = firstPair;
                }
                else
                {
                    samples = staleOldSamples;
                    pair = nextPair;
                }
            }
            else if (periodId == newCurrent.Id)
            {
                newCurrentPageStarted.TrySetResult();
                await releaseNewCurrentPage.Task;
                samples = newCurrent.Samples;
                pair = nextPair;
            }
            else if (periodId == historical.Id)
            {
                samples = historical.Samples;
                pair = Volatile.Read(ref historyPeriodsCalls) < 3 ? firstPair : nextPair;
            }
            else
            {
                return HistoryPageFetchResult.FromFailure(DetailsFetchFailure.Response);
            }

            return HistoryPageFetchResult.Success(new ApiHistoryPage(
                periodId,
                samples,
                Array.Empty<ApiHistoryGap>(),
                NextCursor: null,
                ResumeCursor: null,
                pair));
        }

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Threads are outside this graph regression test.");
    }

    private sealed class DeferredQuotaBoundaryHistoryResourceClient(
        ApiDetailsSnapshot initialDetails,
        ApiDetailsSnapshot resetDetails,
        ApiHistoryPeriod oldCurrent,
        ApiHistoryPeriod historical,
        ApiHistoryPeriod newCurrent,
        IReadOnlyList<ApiHistorySample> staleOldSamples,
        PublishedPairIdentity firstPair,
        PublishedPairIdentity nextPair)
        : HealthyDetailsClientBase, ILoopbackResourceClient
    {
        private readonly TaskCompletionSource secondOldPageStarted =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly TaskCompletionSource releaseSecondOldPage =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly TaskCompletionSource newCurrentPageStarted =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly TaskCompletionSource releaseNewCurrentPage =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private int currentCalls;
        private int historyPeriodsCalls;
        private int historyPageCalls;
        private int oldPageCalls;

        public int HistoryPeriodsCalls => Volatile.Read(ref historyPeriodsCalls);

        public int HistoryPageCalls => Volatile.Read(ref historyPageCalls);

        public bool SecondOldPageStarted => secondOldPageStarted.Task.IsCompleted;

        public Task NewCurrentPageStartedSignal => newCurrentPageStarted.Task;

        public void ReleaseSecondOldPage() => releaseSecondOldPage.TrySetResult();

        public void ReleaseNewCurrentPage() => releaseNewCurrentPage.TrySetResult();

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(
            CancellationToken cancellationToken = default)
            => throw new InvalidOperationException("Main must use the split current resource in this fixture.");

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default)
        {
            var snapshot = Interlocked.Increment(ref currentCalls) == 1 ? initialDetails : resetDetails;
            return Task.FromResult(CurrentFetchResult.Success(ToCurrentSnapshot(snapshot)));
        }

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref historyPeriodsCalls);
            var periods = call < 3
                ? new[] { oldCurrent, historical }
                : new[] { newCurrent, oldCurrent with { Current = false }, historical };
            var pair = call < 3 ? firstPair : nextPair;
            return Task.FromResult(HistoryPeriodsFetchResult.Success(
                new ApiHistoryPeriodsSnapshot(periods, pair)));
        }

        public async Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref historyPageCalls);
            IReadOnlyList<ApiHistorySample> samples;
            PublishedPairIdentity pair;
            if (periodId == oldCurrent.Id)
            {
                var oldCall = Interlocked.Increment(ref oldPageCalls);
                if (oldCall == 1)
                {
                    samples = oldCurrent.Samples;
                    pair = firstPair;
                }
                else
                {
                    if (oldCall == 2)
                    {
                        secondOldPageStarted.TrySetResult();
                        await releaseSecondOldPage.Task.WaitAsync(cancellationToken);
                    }
                    samples = staleOldSamples;
                    pair = firstPair;
                }
            }
            else if (periodId == newCurrent.Id)
            {
                newCurrentPageStarted.TrySetResult();
                await releaseNewCurrentPage.Task.WaitAsync(cancellationToken);
                samples = newCurrent.Samples;
                pair = nextPair;
            }
            else if (periodId == historical.Id)
            {
                samples = historical.Samples;
                pair = historyPeriodsCalls < 3 ? firstPair : nextPair;
            }
            else
            {
                return HistoryPageFetchResult.FromFailure(DetailsFetchFailure.Response);
            }

            return HistoryPageFetchResult.Success(new ApiHistoryPage(
                periodId,
                samples,
                Array.Empty<ApiHistoryGap>(),
                NextCursor: null,
                ResumeCursor: null,
                pair));
        }

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Threads are outside this graph regression test.");
    }

    private sealed class CountingHistoryResourceClient(
        ApiDetailsSnapshot details,
        ApiHistoryPeriod period,
        PublishedPairIdentity pair) : ILoopbackDetailsClient, ILoopbackResourceClient
    {
        private int historyPeriodsCalls;
        private int historyPageCalls;

        public int HistoryPeriodsCalls => Volatile.Read(ref historyPeriodsCalls);

        public int HistoryPageCalls => Volatile.Read(ref historyPageCalls);

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(DetailsFetchResult.Success(details));

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Current is outside this graph regression test.");

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref historyPeriodsCalls);
            return Task.FromResult(HistoryPeriodsFetchResult.Success(
                new ApiHistoryPeriodsSnapshot([period with { Samples = Array.Empty<ApiHistorySample>() }], pair)));
        }

        public Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default)
        {
            Interlocked.Increment(ref historyPageCalls);
            return Task.FromResult(HistoryPageFetchResult.Success(new ApiHistoryPage(
                periodId,
                period.Samples,
                Array.Empty<ApiHistoryGap>(),
                NextCursor: null,
                ResumeCursor: "resume",
                pair)));
        }

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Threads are outside this graph regression test.");
    }

    private sealed class DeferredEmptyHistoryResourceClient(
        ApiDetailsSnapshot details,
        ApiHistoryPeriod current,
        ApiHistoryPeriod empty,
        PublishedPairIdentity pair,
        bool failEmpty = false) : ILoopbackDetailsClient, ILoopbackResourceClient
    {
        private readonly TaskCompletionSource emptyRequestStarted =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly TaskCompletionSource releaseEmptyRequest =
            new(TaskCreationOptions.RunContinuationsAsynchronously);

        public bool EmptyRequestStarted => emptyRequestStarted.Task.IsCompleted;

        public void CompleteEmptyRequest() => releaseEmptyRequest.TrySetResult();

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(DetailsFetchResult.Success(details));

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Current is outside this graph regression test.");

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            CancellationToken cancellationToken = default) =>
            Task.FromResult(HistoryPeriodsFetchResult.Success(new ApiHistoryPeriodsSnapshot(
                [
                    current with { Samples = Array.Empty<ApiHistorySample>() },
                    empty with { Samples = Array.Empty<ApiHistorySample>() },
                ],
                pair)));

        public async Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default)
        {
            IReadOnlyList<ApiHistorySample> samples;
            if (periodId == current.Id)
            {
                samples = current.Samples;
            }
            else if (periodId == empty.Id)
            {
                emptyRequestStarted.TrySetResult();
                await releaseEmptyRequest.Task.WaitAsync(cancellationToken);
                if (failEmpty)
                {
                    return HistoryPageFetchResult.FromFailure(DetailsFetchFailure.Transport);
                }
                samples = Array.Empty<ApiHistorySample>();
            }
            else
            {
                throw new InvalidOperationException($"Unexpected period {periodId}.");
            }

            return HistoryPageFetchResult.Success(new ApiHistoryPage(
                periodId,
                samples,
                Array.Empty<ApiHistoryGap>(),
                NextCursor: null,
                ResumeCursor: $"resume-{periodId}",
                pair));
        }

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Threads are outside this graph regression test.");
    }

    private sealed class CursorRecoveryHistoryResourceClient(
        ApiDetailsSnapshot details,
        ApiHistoryPeriod period,
        ApiHistorySample[] samples,
        PublishedPairIdentity pair) : ILoopbackDetailsClient, ILoopbackResourceClient
    {
        private readonly object gate = new();
        private readonly List<string?> cursors = [];
        private int historyPageCalls;

        public IReadOnlyList<string?> Cursors
        {
            get
            {
                lock (gate)
                {
                    return cursors.ToArray();
                }
            }
        }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(DetailsFetchResult.Success(details));

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Current is outside this graph regression test.");

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            CancellationToken cancellationToken = default) =>
            Task.FromResult(HistoryPeriodsFetchResult.Success(
                new ApiHistoryPeriodsSnapshot([period], pair)));

        public Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default)
        {
            lock (gate)
            {
                cursors.Add(cursor);
            }
            var call = Interlocked.Increment(ref historyPageCalls);
            var result = call switch
            {
                1 => Page([samples[0]], "C0"),
                2 => HistoryPageFetchResult.FromRejectedCursor(),
                3 => HistoryPageFetchResult.FromFailure(DetailsFetchFailure.Transport),
                4 => Page([samples[0], samples[1]], "C1"),
                5 => HistoryPageFetchResult.FromFailure(DetailsFetchFailure.Transport),
                6 => Page([samples[2]], "C2"),
                _ => throw new InvalidOperationException($"Unexpected history page request {call}."),
            };
            return Task.FromResult(result);

            HistoryPageFetchResult Page(IReadOnlyList<ApiHistorySample> pageSamples, string resumeCursor) =>
                HistoryPageFetchResult.Success(new ApiHistoryPage(
                    periodId,
                    pageSamples,
                    Array.Empty<ApiHistoryGap>(),
                    NextCursor: null,
                    ResumeCursor: resumeCursor,
                    pair));
        }

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Threads are outside this graph regression test.");
    }

    private sealed class PairTransitionHistoryResourceClient(
        ApiDetailsSnapshot details,
        ApiHistoryPeriod period,
        ApiHistorySample firstSample,
        ApiHistorySample secondSample,
        PublishedPairIdentity firstPair,
        PublishedPairIdentity secondPair) : ILoopbackDetailsClient, ILoopbackResourceClient
    {
        private readonly object gate = new();
        private readonly List<string?> cursors = [];
        private int periodsCalls;
        private int pageCalls;

        public IReadOnlyList<string?> Cursors
        {
            get
            {
                lock (gate)
                {
                    return cursors.ToArray();
                }
            }
        }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(DetailsFetchResult.Success(details));

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Current is outside this graph regression test.");

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            CancellationToken cancellationToken = default)
        {
            // The second periods read deliberately races with a page from the
            // next root. The view-model must retry the complete transaction,
            // whose third periods read then aligns with that page generation.
            var pair = Interlocked.Increment(ref periodsCalls) <= 2 ? firstPair : secondPair;
            return Task.FromResult(HistoryPeriodsFetchResult.Success(
                new ApiHistoryPeriodsSnapshot([period], pair)));
        }

        public Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default)
        {
            lock (gate)
            {
                cursors.Add(cursor);
            }

            var call = Interlocked.Increment(ref pageCalls);
            var pair = call == 1 ? firstPair : secondPair;
            IReadOnlyList<ApiHistorySample> pageSamples = call switch
            {
                1 => [firstSample],
                2 => [secondSample],
                _ => [firstSample, secondSample],
            };
            return Task.FromResult(HistoryPageFetchResult.Success(new ApiHistoryPage(
                periodId,
                pageSamples,
                Array.Empty<ApiHistoryGap>(),
                NextCursor: null,
                ResumeCursor: call == 1 ? "A0" : "B0",
                pair)));
        }

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Threads are outside this graph regression test.");
    }

    private sealed class DuplicateTimestampHistoryResourceClient(
        ApiDetailsSnapshot details,
        ApiHistoryPeriod period,
        ApiHistorySample[] samples,
        PublishedPairIdentity pair) : ILoopbackDetailsClient, ILoopbackResourceClient
    {
        private readonly object gate = new();
        private readonly List<string?> cursors = [];
        private int pageCalls;

        public IReadOnlyList<string?> Cursors
        {
            get
            {
                lock (gate)
                {
                    return cursors.ToArray();
                }
            }
        }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(DetailsFetchResult.Success(details));

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Current is outside this graph regression test.");

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            CancellationToken cancellationToken = default) =>
            Task.FromResult(HistoryPeriodsFetchResult.Success(
                new ApiHistoryPeriodsSnapshot([period], pair)));

        public Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default)
        {
            lock (gate)
            {
                cursors.Add(cursor);
            }

            var call = Interlocked.Increment(ref pageCalls);
            var result = call switch
            {
                1 => Page([samples[0]], nextCursor: null, resumeCursor: "C0"),
                2 => Page([samples[0]], nextCursor: null, resumeCursor: "C0"),
                3 => Page([samples[1], samples[1]], nextCursor: null, resumeCursor: "C0"),
                4 => Page([samples[1]], nextCursor: "P1", resumeCursor: null),
                5 => Page([samples[1]], nextCursor: null, resumeCursor: "C0"),
                _ => throw new InvalidOperationException($"Unexpected history page request {call}."),
            };
            return Task.FromResult(result);

            HistoryPageFetchResult Page(
                IReadOnlyList<ApiHistorySample> pageSamples,
                string? nextCursor,
                string? resumeCursor) =>
                HistoryPageFetchResult.Success(new ApiHistoryPage(
                    periodId,
                    pageSamples,
                    Array.Empty<ApiHistoryGap>(),
                    nextCursor,
                    resumeCursor,
                    pair));
        }

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("Threads are outside this graph regression test.");
    }
}
