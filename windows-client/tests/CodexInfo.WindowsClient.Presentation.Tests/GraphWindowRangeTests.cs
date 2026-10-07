// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections.Concurrent;
using System.Collections.Specialized;
using System.Diagnostics;
using System.Reflection;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphWindowRangeTests
{
    private const string RangeTypeName = "CodexInfo.WindowsClient.Graphing.GraphTimeRange";
    private const long DaySeconds = 86_400;
    private const long WeekSeconds = 604_800;
    private const long FixedNow = 1_800_000_000;
    private static readonly PublishedPairIdentity Pair = PublishedPairIdentity.Create(
        $"v1:{new string('f', 64)}");

    [Fact]
    public async Task ResetPeriodRemainsTheDefaultAndItsEndRemainsServerOwned()
    {
        var period = CreatePeriod(
            "current",
            FixedNow - WeekSeconds,
            FixedNow - 60,
            resetAt: FixedNow + 3_600,
            current: true,
            sol: 17);
        using var main = await StartMainAsync([period]);
        using var graph = CreateGraph(main, () => FixedNow + 3_600);

        var range = RequiredProperty(graph, "SelectedTimeRange").GetValue(graph);
        Assert.NotNull(range);
        Assert.Equal("ResetPeriod", range!.ToString());
        Assert.True(ReadBool(graph, "IsPeriodView"));
        Assert.False(ReadBool(graph, "Is24HourView"));
        Assert.False(ReadBool(graph, "IsWeekView"));
        Assert.Equal(period.EndAt, graph.Scene.PeriodEndAt);
    }

    [Fact]
    public async Task ResetPeriodNavigationUsesPublishedOrderForThreePeriodRoundTrip()
    {
        var current = CreatePeriod(
            "current",
            FixedNow - WeekSeconds,
            FixedNow - 60,
            resetAt: FixedNow + 3_600,
            current: true,
            sol: 17);
        var middle = CreatePeriod(
            "middle",
            FixedNow - 4 * 3_600,
            FixedNow - 2 * 3_600,
            resetAt: FixedNow - 3_600,
            current: false,
            sol: 18);
        var oldest = CreatePeriod(
            "oldest",
            FixedNow - 10 * 3_600,
            FixedNow - 8 * 3_600,
            resetAt: FixedNow - 7 * 3_600,
            current: false,
            sol: 19);
        using var main = await StartMainAsync([current, middle, oldest]);
        using var graph = CreateGraph(main, () => FixedNow);
        var changed = new ConcurrentQueue<string?>();
        graph.PropertyChanged += (_, eventArgs) => changed.Enqueue(eventArgs.PropertyName);

        Assert.Equal("current", graph.SelectedPeriod?.Id);
        Assert.True(graph.CanGoBack);
        Assert.False(graph.CanGoForward);

        graph.GoBack();
        Assert.Equal("middle", graph.SelectedPeriod?.Id);
        Assert.Equal(middle.StartAt, graph.Scene.PeriodStartAt);
        Assert.Equal(middle.EndAt, graph.Scene.PeriodEndAt);
        Assert.True(graph.CanGoBack);
        Assert.True(graph.CanGoForward);

        graph.GoBack();
        Assert.Equal("oldest", graph.SelectedPeriod?.Id);
        Assert.Equal(oldest.StartAt, graph.Scene.PeriodStartAt);
        Assert.Equal(oldest.EndAt, graph.Scene.PeriodEndAt);
        Assert.False(graph.CanGoBack);
        Assert.True(graph.CanGoForward);

        var oldestScene = graph.Scene;
        graph.GoBack();
        Assert.Same(oldestScene, graph.Scene);
        Assert.Equal("oldest", graph.SelectedPeriod?.Id);

        graph.GoForward();
        Assert.Equal("middle", graph.SelectedPeriod?.Id);
        graph.GoForward();
        Assert.Equal("current", graph.SelectedPeriod?.Id);
        Assert.True(graph.CanGoBack);
        Assert.False(graph.CanGoForward);
        Assert.Contains(nameof(GraphWindowViewModel.CanGoBack), changed);
        Assert.Contains(nameof(GraphWindowViewModel.CanGoForward), changed);
    }

    [Fact]
    public async Task ResetPeriodNavigationDisablesEmptyAndSinglePeriodBoundaries()
    {
        using var emptyMain = await StartMainAsync(Array.Empty<ApiHistoryPeriod>());
        using var emptyGraph = CreateGraph(emptyMain, () => FixedNow);
        var emptyScene = emptyGraph.Scene;

        Assert.Null(emptyGraph.SelectedPeriod);
        Assert.False(emptyGraph.CanGoBack);
        Assert.False(emptyGraph.CanGoForward);
        emptyGraph.GoBack();
        emptyGraph.GoForward();
        Assert.Same(emptyScene, emptyGraph.Scene);

        var only = CreatePeriod(
            "only",
            FixedNow - WeekSeconds,
            FixedNow - 60,
            resetAt: FixedNow + 3_600,
            current: true,
            sol: 17);
        using var singleMain = await StartMainAsync([only]);
        using var singleGraph = CreateGraph(singleMain, () => FixedNow);
        var singleScene = singleGraph.Scene;

        Assert.Equal("only", singleGraph.SelectedPeriod?.Id);
        Assert.False(singleGraph.CanGoBack);
        Assert.False(singleGraph.CanGoForward);
        singleGraph.GoBack();
        singleGraph.GoForward();
        Assert.Same(singleScene, singleGraph.Scene);
    }

    [Fact]
    public async Task FailedResetPeriodNavigationKeepsAcceptedSelectionAndSceneAndCanRetry()
    {
        var current = CreatePeriod(
            "current",
            FixedNow - WeekSeconds,
            FixedNow - 60,
            resetAt: FixedNow + 3_600,
            current: true,
            sol: 17);
        var past = CreatePeriod(
            "past",
            FixedNow - 4 * 3_600,
            FixedNow - 2 * 3_600,
            resetAt: FixedNow - 3_600,
            current: false,
            sol: 18);
        var client = new RangeResourceClient([current, past], Pair);
        using var main = CreateResourceMain(client);
        using var graph = CreateGraph(main, () => FixedNow);
        await EventuallyAsync(() => graph.HasPoints && !graph.IsLoading);
        var acceptedPeriod = graph.SelectedPeriod;
        var acceptedLabel = graph.SelectedPeriodText;
        var acceptedScene = graph.Scene;
        client.FailPeriodId = "past";
        client.FailPages = true;

        graph.GoBack();

        Assert.Same(acceptedPeriod, graph.SelectedPeriod);
        Assert.Equal(acceptedLabel, graph.SelectedPeriodText);
        Assert.Same(acceptedScene, graph.Scene);
        await EventuallyAsync(() => !graph.IsLoading && graph.HasLoadError);
        Assert.Same(acceptedPeriod, graph.SelectedPeriod);
        Assert.Equal(acceptedLabel, graph.SelectedPeriodText);
        Assert.Same(acceptedScene, graph.Scene);
        Assert.True(graph.CanGoBack);
        Assert.False(graph.CanGoForward);

        client.FailPages = false;
        graph.GoBack();
        await EventuallyAsync(() => !graph.IsLoading && graph.SelectedPeriod?.Id == "past");
        Assert.False(graph.HasLoadError);
        Assert.Equal(past.StartAt, graph.Scene.PeriodStartAt);
        Assert.Equal(past.EndAt, graph.Scene.PeriodEndAt);
        Assert.False(graph.CanGoBack);
        Assert.True(graph.CanGoForward);

        graph.GoForward();
        await EventuallyAsync(() => !graph.IsLoading && graph.SelectedPeriod?.Id == "current");
        Assert.True(graph.CanGoBack);
        Assert.False(graph.CanGoForward);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task ResetPeriodRangeAndSceneCommitTogetherWhenLargeBuildCompletes(bool useSplitResourceClient)
    {
        var blockingModels = new BlockingModelSamples(
        [
            new ApiHistoryModelSample("SOL", 1, 0, 0, 17)
            {
                TotalTokens = 17,
            },
        ]);
        var period = CreateLargePeriod(blockingModels);
        var dispatcher = new QueuedUiDispatcher();
        var resourceClient = useSplitResourceClient ? new RangeResourceClient([period], Pair) : null;
        using var main = useSplitResourceClient
            ? CreateResourceMain(resourceClient!)
            : await StartMainAsync([period]);
        using var graph = CreateGraph(main, () => FixedNow, dispatcher.Post);

        await RunQueuedUntilAsync(dispatcher, () =>
            graph.HasPoints && !graph.IsLoading && !IsViewport(graph.Scene));
        Assert.True(graph.HasPoints);
        Assert.False(graph.IsLoading);

        SetRange(graph, "Last24Hours");
        await RunQueuedUntilAsync(dispatcher, () =>
            RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)?.ToString() == "Last24Hours" &&
            !graph.IsLoading);
        Assert.Equal("Last24Hours", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        Assert.True(IsViewport(graph.Scene));
        var acceptedScene = graph.Scene;
        var acceptedLabel = ReadString(graph, "RangeLabel");

        blockingModels.Arm();
        try
        {
            var historyPeriodsCallsBeforeReset = resourceClient?.HistoryPeriodsCalls ?? 0;
            SetRange(graph, "ResetPeriod");
            if (useSplitResourceClient)
            {
                await EventuallyAsync(() => resourceClient!.HistoryPeriodsCalls > historyPeriodsCallsBeforeReset);
                (await dispatcher.TakeAsync())(); // Publish the complete reset resource candidate and start projection.
            }
            await blockingModels.EnumerationStarted.Task.WaitAsync(TimeSpan.FromSeconds(2));

            Assert.Equal("Last24Hours", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
            Assert.Same(acceptedScene, graph.Scene);
            Assert.Equal(acceptedLabel, ReadString(graph, "RangeLabel"));
            Assert.True(graph.IsLoading);
        }
        finally
        {
            blockingModels.Release();
        }

        await RunQueuedUntilAsync(dispatcher, () =>
            RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)?.ToString() == "ResetPeriod" &&
            !graph.IsLoading && !IsViewport(graph.Scene));

        Assert.Equal("ResetPeriod", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        Assert.False(IsViewport(graph.Scene));
        Assert.Equal(period.StartAt, graph.Scene.PeriodStartAt);
        Assert.Equal(period.EndAt, graph.Scene.PeriodEndAt);
        Assert.NotSame(acceptedScene, graph.Scene);
        Assert.False(graph.IsLoading);
    }

    [Fact]
    public async Task NewFixedRangeSupersedesQueuedResetProjection()
    {
        var blockingModels = new BlockingModelSamples(
        [
            new ApiHistoryModelSample("SOL", 1, 0, 0, 17)
            {
                TotalTokens = 17,
            },
        ]);
        var period = CreateLargePeriod(blockingModels);
        var dispatcher = new QueuedUiDispatcher();
        using var main = await StartMainAsync([period]);
        using var graph = CreateGraph(main, () => FixedNow, dispatcher.Post);
        await RunQueuedUntilAsync(dispatcher, () => graph.HasPoints && !graph.IsLoading && !IsViewport(graph.Scene));

        SetRange(graph, "Last24Hours");
        await RunQueuedUntilAsync(dispatcher, () =>
            RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)?.ToString() == "Last24Hours" &&
            !graph.IsLoading);
        Assert.Equal("Last24Hours", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());

        blockingModels.Arm();
        try
        {
            SetRange(graph, "ResetPeriod");
            await blockingModels.EnumerationStarted.Task.WaitAsync(TimeSpan.FromSeconds(2));
        }
        finally
        {
            blockingModels.Release();
        }

        var staleResetPublish = await dispatcher.TakeAsync();
        SetRange(graph, "Last7Days");
        await RunQueuedUntilAsync(dispatcher, () =>
            RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)?.ToString() == "Last7Days" &&
            !graph.IsLoading);
        Assert.Equal("Last7Days", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        AssertViewport(graph.Scene, FixedNow - WeekSeconds, FixedNow);
        var acceptedWeekScene = graph.Scene;

        staleResetPublish();

        Assert.Equal("Last7Days", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        AssertViewport(graph.Scene, FixedNow - WeekSeconds, FixedNow);
        Assert.Same(acceptedWeekScene, graph.Scene);
    }

    [Fact]
    public async Task EmptyFixedWindowKeepsExactViewportAndPlot()
    {
        var oldEnd = FixedNow - 2 * DaySeconds;
        var oldPeriod = CreatePeriod(
            "old",
            oldEnd - WeekSeconds,
            oldEnd,
            resetAt: oldEnd,
            current: false,
            sol: 17);
        using var main = await StartMainAsync([oldPeriod]);
        using var graph = CreateGraph(main, () => FixedNow);

        SetRange(graph, "Last24Hours");

        AssertViewport(graph.Scene, FixedNow - DaySeconds, FixedNow);
        Assert.False(graph.Scene.HasPoints);
        Assert.Empty(ReadPeriodScenes(graph.Scene));
        Assert.True(ReadBool(graph, "HasPlot"));
    }

    [Fact]
    public async Task HistoricalPanAndRangeSwitchStayPinnedUntilForward()
    {
        var period = CreatePeriod(
            "current",
            FixedNow - 10 * DaySeconds,
            FixedNow - 60,
            resetAt: FixedNow + 3_600,
            current: true,
            sol: 17);
        using var main = await StartMainAsync([period]);
        var now = FixedNow;
        using var graph = CreateGraph(main, () => now);

        SetRange(graph, "Last24Hours");
        graph.GetType().GetMethod("GoBack", BindingFlags.Public | BindingFlags.Instance)!
            .Invoke(graph, null);
        AssertViewport(graph.Scene, FixedNow - 2 * DaySeconds, FixedNow - DaySeconds);

        now += 1_200;
        SetRange(graph, "Last7Days");
        AssertViewport(graph.Scene, FixedNow - DaySeconds - WeekSeconds, FixedNow - DaySeconds);
        Assert.True(ReadBool(graph, "CanGoForward"));

        graph.GetType().GetMethod("GoForward", BindingFlags.Public | BindingFlags.Instance)!
            .Invoke(graph, null);
        AssertViewport(graph.Scene, now - WeekSeconds, now);
        Assert.False(ReadBool(graph, "CanGoForward"));
    }

    [Fact]
    public async Task ForwardReturnsToLiveWhenItReachesTheOriginalEdgeAfterClockAdvances()
    {
        var period = CreatePeriod(
            "current",
            FixedNow - 10 * DaySeconds,
            FixedNow - 60,
            resetAt: FixedNow + 3_600,
            current: true,
            sol: 17);
        using var main = await StartMainAsync([period]);
        var now = FixedNow;
        using var graph = CreateGraph(main, () => now);

        SetRange(graph, "Last24Hours");
        AssertViewport(graph.Scene, FixedNow - DaySeconds, FixedNow);
        graph.GetType().GetMethod("GoBack", BindingFlags.Public | BindingFlags.Instance)!
            .Invoke(graph, null);
        AssertViewport(graph.Scene, FixedNow - 2 * DaySeconds, FixedNow - DaySeconds);

        now += 10;
        graph.GetType().GetMethod("GoForward", BindingFlags.Public | BindingFlags.Instance)!
            .Invoke(graph, null);

        AssertViewport(graph.Scene, now - DaySeconds, now);
        Assert.False(ReadBool(graph, "CanGoForward"));
    }

    [Fact]
    public async Task TimeWindowBuildsResetScopedScenesFromEveryIntersectingPeriod()
    {
        var periods = CreateTwoResetPeriods(FixedNow, accountOffset: 0)
            .Select((period, index) =>
            {
                if (index != 0)
                {
                    return period;
                }

                var resetAt = period.EndAt + 30;
                return period with
                {
                    ResetAt = resetAt,
                    Samples = period.Samples.Select(sample => sample with { ResetAt = resetAt }).ToArray(),
                };
            })
            .ToArray();
        var client = new RangeResourceClient(periods, Pair);
        using var main = CreateResourceMain(client);
        using var graph = CreateGraph(main, () => FixedNow);
        await EventuallyAsync(() => graph.HasPoints);
        client.ClearPageRequests();
        var sceneChanges = 0;
        graph.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(GraphWindowViewModel.Scene))
            {
                sceneChanges++;
            }
        };

        SetRange(graph, "Last7Days");
        await EventuallyAsync(() => IsViewport(graph.Scene));

        AssertViewport(graph.Scene, FixedNow - WeekSeconds, FixedNow);
        var children = ReadPeriodScenes(graph.Scene);
        Assert.Equal(2, children.Count);
        Assert.NotEqual(periods[0].EndAt, periods[0].ResetAt);
        Assert.Equal(periods[0].ResetAt, ReadNullableLong(children[0], "ResetAt"));
        Assert.Contains(100d, children[0].Sol);
        Assert.Contains(5d, children[1].Sol);
        Assert.DoesNotContain((double)FixedNow, children[1].Timestamps);
        Assert.False(children[1].ModelSynthetic[^1]);
        Assert.Equal(1, sceneChanges);
        Assert.Equal(
            new[] { "reset-old", "reset-current" },
            client.PageRequests.Select(request => request.PeriodId).Distinct().ToArray());
        Assert.All(client.PageRequests, request => Assert.Null(request.Cursor));
    }

    [Fact]
    public async Task SplitResourcePeriodReplacementTransientNullDoesNotClearAcceptedScene()
    {
        var periods = CreateTwoResetPeriods(FixedNow, accountOffset: 0);
        var client = new RangeResourceClient(periods, Pair);
        using var main = CreateResourceMain(client);
        using var graph = CreateGraph(main, () => FixedNow);
        await EventuallyAsync(() => graph.HasPoints && !graph.IsLoading);
        var selectedPeriod = Assert.Single(graph.Periods, period => period.Id == "reset-old");
        var acceptedScene = graph.Scene;
        var acceptedLabel = graph.SelectedPeriodText;
        var transientNullClearedState = false;
        var directoryResetCount = 0;
        ((INotifyCollectionChanged)graph.Periods).CollectionChanged += (_, args) =>
        {
            if (args.Action != NotifyCollectionChangedAction.Reset)
            {
                return;
            }

            directoryResetCount++;
            graph.SelectedPeriod = null;
            transientNullClearedState |= graph.SelectedPeriod is null ||
                !ReferenceEquals(acceptedScene, graph.Scene) ||
                !graph.Scene.HasPoints ||
                graph.SelectedPeriodText != acceptedLabel;
        };

        graph.SelectedPeriod = selectedPeriod;
        await EventuallyAsync(() => !graph.IsLoading &&
            graph.SelectedPeriod?.Id == selectedPeriod.Id && graph.Scene.HasPoints);

        Assert.Equal(1, directoryResetCount);
        Assert.False(transientNullClearedState, "A split-resource directory reset's transient null must not clear the accepted selector or scene.");
        Assert.Equal(selectedPeriod.Id, graph.SelectedPeriod?.Id);
        Assert.NotSame(acceptedScene, graph.Scene);
        Assert.True(graph.Scene.HasPoints);
    }

    [Fact]
    public async Task PanningReusesTheAcceptedResetProjectionChild()
    {
        var period = CreatePeriod(
            "current",
            FixedNow - 10 * DaySeconds,
            FixedNow - 60,
            resetAt: FixedNow + 3_600,
            current: true,
            sol: 17);
        using var main = await StartMainAsync([period]);
        using var graph = CreateGraph(main, () => FixedNow);
        SetRange(graph, "Last24Hours");
        var child = Assert.Single(ReadPeriodScenes(graph.Scene));

        graph.GetType().GetMethod("GoBack", BindingFlags.Public | BindingFlags.Instance)!
            .Invoke(graph, null);

        AssertViewport(graph.Scene, FixedNow - 2 * DaySeconds, FixedNow - DaySeconds);
        Assert.Same(child, Assert.Single(ReadPeriodScenes(graph.Scene)));
    }

    [Fact]
    public async Task FailedSecondPeriodKeepsAcceptedRangeLabelAndScene()
    {
        var periods = CreateTwoResetPeriods(FixedNow, accountOffset: 0);
        var client = new RangeResourceClient(periods, Pair);
        using var main = CreateResourceMain(client);
        using var graph = CreateGraph(main, () => FixedNow);
        await EventuallyAsync(() => graph.HasPoints);
        var acceptedScene = graph.Scene;
        var acceptedLabel = ReadString(graph, "RangeLabel");
        client.ClearPageRequests();
        client.FailPeriodId = "reset-current";
        client.FailPages = true;

        SetRange(graph, "Last7Days");
        await EventuallyAsync(() => graph.HasLoadError);

        Assert.Equal("ResetPeriod", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        Assert.Equal(acceptedLabel, ReadString(graph, "RangeLabel"));
        Assert.Same(acceptedScene, graph.Scene);
        Assert.Contains("reset-old", client.PageRequests.Select(request => request.PeriodId));
        Assert.Contains("reset-current", client.PageRequests.Select(request => request.PeriodId));
    }

    [Fact]
    public async Task MixedPublishedPairCannotPublishAPartialWindow()
    {
        var periods = CreateTwoResetPeriods(FixedNow, accountOffset: 0);
        var client = new RangeResourceClient(periods, Pair)
        {
            MismatchPairForPeriodId = "reset-old",
        };
        using var main = CreateResourceMain(client);
        using var graph = CreateGraph(main, () => FixedNow);
        await EventuallyAsync(() => graph.HasPoints);
        var acceptedScene = graph.Scene;
        var acceptedLabel = ReadString(graph, "RangeLabel");
        client.ClearPageRequests();

        SetRange(graph, "Last7Days");
        await EventuallyAsync(() => graph.HasLoadError);

        Assert.Equal("ResetPeriod", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        Assert.Equal(acceptedLabel, ReadString(graph, "RangeLabel"));
        Assert.Same(acceptedScene, graph.Scene);
        Assert.Contains("reset-old", client.PageRequests.Select(request => request.PeriodId));
        Assert.True(client.HistoryPeriodsCalls >= 3);
    }

    [Fact]
    public async Task MetricChangeDuringWindowProjectionCannotPublishAnOldMetricScene()
    {
        var periods = CreateTwoResetPeriods(FixedNow, accountOffset: 0);
        var blockingModels = new BlockingModelSamples(periods[1].Samples[0].Models);
        var currentSample = periods[1].Samples[0] with { ModelSamples = blockingModels };
        periods =
        [
            periods[0],
            periods[1] with { Samples = [currentSample] },
        ];
        var client = new RangeResourceClient(periods, Pair);
        using var main = CreateResourceMain(client);
        using var graph = CreateGraph(main, () => FixedNow);
        await EventuallyAsync(() => graph.HasPoints);
        graph.SelectedMetric = graph.Texts.GraphDollarMetric;
        await EventuallyAsync(() =>
            !graph.IsLoading &&
            graph.Scene.Metric == GraphMetric.Dollars);
        blockingModels.Arm();

        try
        {
            SetRange(graph, "Last7Days");
            await blockingModels.EnumerationStarted.Task.WaitAsync(TimeSpan.FromSeconds(2));
            graph.SelectedMetric = graph.Texts.GraphTokenMetric;
        }
        finally
        {
            blockingModels.Release();
        }

        await EventuallyAsync(() =>
            !graph.IsLoading &&
            IsViewport(graph.Scene) &&
            RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString() == "Last7Days");

        Assert.Equal("Last7Days", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        Assert.Equal(GraphMetric.Tokens, graph.Scene.Metric);
        Assert.Equal(graph.Texts.GraphTokenDescription, graph.MetricAxisText);
    }

    [Fact]
    public async Task DetailsRefreshWhileLargeStaticWindowCandidateIsPendingKeepsAcceptedRangeAndLatestSnapshot()
    {
        const int sampleCount = 3_000;
        var resetAt = FixedNow + 3_600;
        var blockingModels = new BlockingModelSamples(
        [
            new ApiHistoryModelSample("SOL", 1, 0, 0, 17)
            {
                TotalTokens = 17,
            },
        ]);
        var oldSamples = Enumerable.Range(0, sampleCount)
            .Select(index => new ApiHistorySample(
                FixedNow - 30_000 + index * 10,
                resetAt,
                80,
                17,
                2,
                3,
                17,
                20,
                30))
            .ToArray();
        oldSamples[^1] = oldSamples[^1] with { ModelSamples = blockingModels };
        var period = CreatePeriod(
            "current",
            FixedNow - WeekSeconds,
            FixedNow,
            resetAt,
            current: true,
            sol: 17) with
        {
            Samples = oldSamples,
        };
        var nextPair = PublishedPairIdentity.Create($"v1:{new string('a', 64)}");
        var latestPeriod = period with
        {
            Samples = oldSamples
                .Select(sample => sample with { SolDollars = 777, ModelSamples = null })
                .ToArray(),
        };
        var initialDetails = CreateDetails([period], Pair);
        var latestDetails = CreateDetails([latestPeriod], nextPair, FixedNow + 1);
        using var main = new MainWindowViewModel(new SequenceDetailsClient(initialDetails, latestDetails));
        main.Start();
        await EventuallyAsync(() => main.HasDetails);
        using var graph = CreateGraph(main, () => FixedNow);
        graph.SelectedMetric = graph.Texts.GraphDollarMetric;
        await EventuallyAsync(() =>
            graph.HasPoints &&
            !graph.IsLoading &&
            graph.Scene.Metric == GraphMetric.Dollars);
        var acceptedScene = graph.Scene;
        blockingModels.Arm();

        var sceneStayedAcceptedWhileRefreshing = false;
        try
        {
            SetRange(graph, "Last24Hours");
            await blockingModels.EnumerationStarted.Task.WaitAsync(TimeSpan.FromSeconds(2));

            await InvokePrivateTaskAsync(main, "RunPeriodicRefreshAsync");
            Assert.Equal(nextPair, main.DetailsSnapshot?.PublishedPair);
            sceneStayedAcceptedWhileRefreshing = ReferenceEquals(acceptedScene, graph.Scene);
        }
        finally
        {
            blockingModels.Release();
        }

        await EventuallyAsync(() =>
            !graph.IsLoading &&
            graph.HasPoints &&
            graph.Scene.Metric == GraphMetric.Dollars &&
            ReadPeriodScenes(graph.Scene).Any(period => period.Sol.Contains(777d)));

        Assert.Equal("Last24Hours", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        Assert.True(sceneStayedAcceptedWhileRefreshing, "A details refresh must not replace the accepted scene while a range candidate is pending.");
        Assert.True(IsViewport(graph.Scene));
        Assert.NotEmpty(ReadString(graph, "RangeLabel"));
        Assert.Equal(GraphMetric.Dollars, graph.Scene.Metric);
        Assert.Contains(777d, Assert.Single(ReadPeriodScenes(graph.Scene)).Sol);
    }

    [Fact]
    public async Task DetailsDirectoryReplacementTransientNullDoesNotClearAcceptedScene()
    {
        var period = CreatePeriod(
            "current",
            FixedNow - DaySeconds,
            FixedNow + 100,
            FixedNow + 3_600,
            current: true,
            sol: 17);
        var updatedPeriod = period with
        {
            Samples = [period.Samples[0] with { SolDollars = 73 }],
        };
        var nextPair = PublishedPairIdentity.Create($"v1:{new string('b', 64)}");
        var initialDetails = CreateDetails([period], Pair);
        var latestDetails = CreateDetails([updatedPeriod], nextPair, FixedNow + 1);
        using var main = new MainWindowViewModel(new SequenceDetailsClient(initialDetails, latestDetails));
        main.Start();
        await EventuallyAsync(() => main.HasDetails);
        using var graph = CreateGraph(main, () => FixedNow);
        graph.SelectedMetric = graph.Texts.GraphDollarMetric;
        Assert.True(graph.HasPoints);
        Assert.Equal(GraphMetric.Dollars, graph.Scene.Metric);
        var acceptedScene = graph.Scene;
        var acceptedLabel = graph.SelectedPeriodText;
        var transientNullClearedState = false;
        ((INotifyCollectionChanged)graph.Periods).CollectionChanged += (_, args) =>
        {
            if (args.Action != NotifyCollectionChangedAction.Reset)
            {
                return;
            }

            graph.SelectedPeriod = null;
            transientNullClearedState |= graph.SelectedPeriod is null ||
                !ReferenceEquals(acceptedScene, graph.Scene) ||
                !graph.Scene.HasPoints ||
                graph.SelectedPeriodText != acceptedLabel;
        };

        await InvokePrivateTaskAsync(main, "RunPeriodicRefreshAsync");

        Assert.False(transientNullClearedState, "A directory reset's transient null must not clear the accepted selector or scene.");
        Assert.Equal("current", graph.SelectedPeriod?.Id);
        Assert.NotSame(acceptedScene, graph.Scene);
        Assert.Equal(GraphMetric.Dollars, graph.Scene.Metric);
        Assert.Contains(73d, graph.Scene.Sol);
    }

    [Fact]
    public async Task SparseHistoricalViewportUsesVisibleCurveForPlotState()
    {
        var viewportStart = FixedNow - 2 * DaySeconds;
        var viewportEnd = FixedNow - DaySeconds;
        var periodStart = viewportStart - 200;
        var periodEnd = viewportEnd + 200;
        var period = CreatePeriod(
            "sparse",
            periodStart,
            periodEnd,
            periodEnd,
            current: false,
            sol: 17) with
        {
            Samples =
            [
                new ApiHistorySample(viewportStart - 100, periodEnd, 80, 2, 1, 1, 2, 1, 1),
                new ApiHistorySample(viewportEnd + 100, periodEnd, 40, 8, 1, 1, 8, 1, 1),
            ],
        };
        using var main = await StartMainAsync([period]);
        using var graph = CreateGraph(main, () => FixedNow);

        SetRange(graph, "Last24Hours");
        graph.GetType().GetMethod("GoBack", BindingFlags.Public | BindingFlags.Instance)!
            .Invoke(graph, null);

        AssertViewport(graph.Scene, viewportStart, viewportEnd);
        Assert.Empty(graph.Points);
        Assert.True(graph.Scene.HasPoints);
        Assert.True(graph.HasPoints);
        Assert.False(graph.HasNoPoints);
    }

    [Fact]
    public async Task AccountChangeWhileRangeFetchIsPendingRejectsTheOldAccountCandidate()
    {
        var firstAccount = CreateTwoResetPeriods(FixedNow, accountOffset: 0);
        var secondAccount = CreateTwoResetPeriods(FixedNow, accountOffset: 200);
        var client = new RangeResourceClient(
            new Dictionary<string, IReadOnlyList<ApiHistoryPeriod>>(StringComparer.Ordinal)
            {
                ["account-1"] = firstAccount,
                ["account-2"] = secondAccount,
            },
            Pair);
        using var main = CreateResourceMain(client);
        using var graph = CreateGraph(main, () => FixedNow);
        await EventuallyAsync(() => graph.HasPoints);
        client.DelayPeriodId = "reset-old";

        SetRange(graph, "Last7Days");
        await client.DelayedPageStarted.Task.WaitAsync(TimeSpan.FromSeconds(2));

        Assert.True(main.SelectAccount("account-2"));
        client.ReleaseDelayedPage.TrySetResult();
        await EventuallyAsync(() => graph.HasPoints &&
            main.SelectedAccount?.Id == "account-2" &&
            graph.Scene.Sol.Contains(205d));

        Assert.Equal("ResetPeriod", RequiredProperty(graph, "SelectedTimeRange").GetValue(graph)!.ToString());
        Assert.DoesNotContain(5d, graph.Scene.Sol);
        Assert.All(client.AccountIds, accountId => Assert.Contains(accountId, new[] { "account-1", "account-2" }));
    }

    private static async Task<MainWindowViewModel> StartMainAsync(IReadOnlyList<ApiHistoryPeriod> periods)
    {
        var details = CreateDetails(periods, Pair);
        var client = new StaticDetailsClient(details);
        var main = new MainWindowViewModel(client);
        main.Start();
        await EventuallyAsync(() => main.HasDetails);
        return main;
    }

    private static MainWindowViewModel CreateResourceMain(RangeResourceClient client)
    {
        var main = new MainWindowViewModel(new StaticHealthClient(), client);
        ApplyAccounts(main);
        return main;
    }

    private static GraphWindowViewModel CreateGraph(
        MainWindowViewModel main,
        Func<long> clock,
        Action<Action>? postToUi = null)
    {
        var constructor = typeof(GraphWindowViewModel).GetConstructor(
            BindingFlags.NonPublic | BindingFlags.Instance,
            binder: null,
            types: [typeof(MainWindowViewModel), typeof(Action<Action>), typeof(Func<long>)],
            modifiers: null);
        Assert.True(constructor is not null, "Missing GraphWindowViewModel clock-injection constructor.");
        return Assert.IsType<GraphWindowViewModel>(constructor!.Invoke(
            [main, postToUi ?? (Action<Action>)(action => action()), clock]));
    }

    private static ApiHistoryPeriod CreateLargePeriod(BlockingModelSamples blockingModels)
    {
        const int sampleCount = 3_000;
        var startAt = FixedNow - WeekSeconds;
        var resetAt = FixedNow + 3_600;
        var samples = Enumerable.Range(0, sampleCount)
            .Select(index => new ApiHistorySample(
                startAt + index * 2,
                resetAt,
                80,
                17,
                2,
                3,
                17,
                20,
                30))
            .ToArray();
        samples[^1] = samples[^1] with { ModelSamples = blockingModels };
        return new ApiHistoryPeriod("current", startAt, FixedNow, true, "current")
        {
            ResetAt = resetAt,
            Samples = samples,
        };
    }

    private static ApiHistoryPeriod CreatePeriod(
        string id,
        long startAt,
        long endAt,
        long resetAt,
        bool current,
        double sol)
    {
        var sampleAt = Math.Min(endAt, startAt + 60);
        var sample = new ApiHistorySample(
            sampleAt,
            resetAt,
            80,
            sol,
            2,
            3,
            (ulong)sol,
            20,
            30);
        return new ApiHistoryPeriod(id, startAt, endAt, current, id)
        {
            ResetAt = resetAt,
            Samples = [sample],
        };
    }

    private static IReadOnlyList<ApiHistoryPeriod> CreateTwoResetPeriods(long now, long accountOffset)
    {
        var oldStart = now - 10 * DaySeconds;
        var oldEnd = now - 4 * DaySeconds;
        var currentStart = oldEnd;
        var currentEnd = now + 3_600;
        return
        [
            CreatePeriod("reset-old", oldStart, oldEnd, oldEnd, current: false, sol: 100 + accountOffset),
            CreatePeriod("reset-current", currentStart, currentEnd, now + 3_600, current: true, sol: 5 + accountOffset),
        ];
    }

    private static ApiDetailsSnapshot CreateDetails(
        IReadOnlyList<ApiHistoryPeriod> periods,
        PublishedPairIdentity pair,
        long? observedAt = FixedNow) =>
        new(
            ApiState.Ready,
            observedAt,
            true,
            "Pro",
            null,
            [],
            0,
            periods,
            periods.SelectMany(period => period.Samples).ToArray(),
            [],
            "estimated")
        {
            PublishedPair = pair,
        };

    private static void ApplyAccounts(MainWindowViewModel main)
    {
        var method = typeof(MainWindowViewModel).GetMethod(
            "ApplyAccountsSnapshot",
            BindingFlags.NonPublic | BindingFlags.Instance);
        Assert.True(method is not null, "Missing account-directory test seam.");
        var result = method!.Invoke(main,
        [
            new ApiAccountsSnapshot(
                "account-1",
                [
                    new ApiAccount("account-1", true, null, null),
                    new ApiAccount("account-2", false, null, null),
                ]),
        ]);
        Assert.True(Assert.IsType<bool>(result));
    }

    private static void SetRange(GraphWindowViewModel graph, string rangeName)
    {
        var property = RequiredProperty(graph, "SelectedTimeRange");
        var range = Enum.Parse(property.PropertyType, rangeName);
        property.SetValue(graph, range);
    }

    private static PropertyInfo RequiredProperty(object instance, string name)
    {
        var property = instance.GetType().GetProperty(name, BindingFlags.Public | BindingFlags.Instance);
        Assert.True(property is not null, $"Missing GraphWindowViewModel property: {name}.");
        return property!;
    }

    private static bool ReadBool(object instance, string name) =>
        Assert.IsType<bool>(RequiredProperty(instance, name).GetValue(instance));

    private static string ReadString(object instance, string name) =>
        Assert.IsType<string>(RequiredProperty(instance, name).GetValue(instance));

    private static long? ReadNullableLong(object instance, string name) =>
        RequiredProperty(instance, name).GetValue(instance) is long value ? value : null;

    private static IReadOnlyList<GraphScene> ReadPeriodScenes(GraphScene scene)
    {
        var property = scene.GetType().GetProperty("PeriodScenes", BindingFlags.Public | BindingFlags.Instance);
        Assert.True(property is not null, "Missing GraphScene.PeriodScenes composite viewport contract.");
        var value = Assert.IsAssignableFrom<IEnumerable<GraphScene>>(property!.GetValue(scene));
        return value.ToArray();
    }

    private static bool IsViewport(GraphScene scene)
    {
        var property = scene.GetType().GetProperty("IsViewport", BindingFlags.Public | BindingFlags.Instance);
        Assert.True(property is not null, "Missing GraphScene.IsViewport composite viewport contract.");
        return Assert.IsType<bool>(property!.GetValue(scene));
    }

    private static void AssertViewport(GraphScene scene, long startAt, long endAt)
    {
        Assert.True(IsViewport(scene));
        Assert.Equal(startAt, scene.PeriodStartAt);
        Assert.Equal(endAt, scene.PeriodEndAt);
    }

    private static async Task EventuallyAsync(Func<bool> condition)
    {
        var timer = Stopwatch.StartNew();
        while (!condition())
        {
            if (timer.Elapsed > TimeSpan.FromSeconds(5))
            {
                throw new TimeoutException("The graph range state did not reach its expected publication.");
            }
            await Task.Delay(5);
        }
    }

    private static async Task RunQueuedUntilAsync(QueuedUiDispatcher dispatcher, Func<bool> condition)
    {
        var timer = Stopwatch.StartNew();
        while (!condition())
        {
            if (timer.Elapsed > TimeSpan.FromSeconds(5))
            {
                throw new TimeoutException("The queued graph publication did not reach its expected state.");
            }
            (await dispatcher.TakeAsync())();
        }
    }

    private sealed class StaticDetailsClient(ApiDetailsSnapshot details) :
        ILoopbackHealthClient,
        ILoopbackDetailsClient
    {
        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "loopback", "test")));

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(DetailsFetchResult.Success(details));
    }

    private sealed class SequenceDetailsClient(params ApiDetailsSnapshot[] snapshots) :
        ILoopbackHealthClient,
        ILoopbackDetailsClient
    {
        private int detailsCallCount;

        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "loopback", "test")));

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var index = Math.Min(Interlocked.Increment(ref detailsCallCount) - 1, snapshots.Length - 1);
            return Task.FromResult(DetailsFetchResult.Success(snapshots[index]));
        }
    }

    private static async Task InvokePrivateTaskAsync(object instance, string methodName)
    {
        var method = instance.GetType().GetMethod(methodName, BindingFlags.NonPublic | BindingFlags.Instance);
        Assert.True(method is not null, $"Missing MainWindowViewModel test seam: {methodName}.");
        var invocation = method!.Invoke(instance, null);
        await Assert.IsAssignableFrom<Task>(invocation);
    }

    private sealed class BlockingModelSamples(IReadOnlyList<ApiHistoryModelSample> samples) :
        IReadOnlyList<ApiHistoryModelSample>
    {
        private readonly TaskCompletionSource enumerationStarted =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private readonly ManualResetEventSlim released = new(false);
        private int armed;
        private int hasBlocked;

        public TaskCompletionSource EnumerationStarted => enumerationStarted;

        public int Count => samples.Count;

        public ApiHistoryModelSample this[int index] => samples[index];

        public void Arm() => Volatile.Write(ref armed, 1);

        public void Release() => released.Set();

        public IEnumerator<ApiHistoryModelSample> GetEnumerator()
        {
            if (Volatile.Read(ref armed) != 0 && Interlocked.Exchange(ref hasBlocked, 1) == 0)
            {
                enumerationStarted.TrySetResult();
                released.Wait(TimeSpan.FromSeconds(5));
            }

            return samples.GetEnumerator();
        }

        System.Collections.IEnumerator System.Collections.IEnumerable.GetEnumerator() => GetEnumerator();
    }

    private sealed class QueuedUiDispatcher
    {
        private readonly ConcurrentQueue<Action> actions = new();
        private readonly SemaphoreSlim available = new(0);

        public void Post(Action action)
        {
            actions.Enqueue(action);
            available.Release();
        }

        public async Task<Action> TakeAsync()
        {
            await available.WaitAsync(TimeSpan.FromSeconds(5));
            Assert.True(actions.TryDequeue(out var action), "Queued UI dispatcher signaled without an action.");
            return action!;
        }
    }

    private sealed class StaticHealthClient : ILoopbackHealthClient
    {
        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "loopback", "test")));
    }

    private sealed class RangeResourceClient :
        ILoopbackDetailsClient,
        ILoopbackResourceClient,
        ILoopbackAccountResourceClient
    {
        private readonly IReadOnlyDictionary<string, IReadOnlyList<ApiHistoryPeriod>> periodsByAccount;
        private readonly PublishedPairIdentity pair;
        private readonly object gate = new();
        private readonly List<(string AccountId, string PeriodId, string? Cursor)> pageRequests = [];
        private readonly TaskCompletionSource delayedPageStarted =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
        private int historyPeriodsCalls;
        private bool delayHasBeenReleased;

        public RangeResourceClient(IReadOnlyList<ApiHistoryPeriod> periods, PublishedPairIdentity pair)
            : this(new Dictionary<string, IReadOnlyList<ApiHistoryPeriod>>(StringComparer.Ordinal)
            {
                ["account-1"] = periods,
                ["account-2"] = periods,
                [string.Empty] = periods,
            }, pair)
        {
        }

        public RangeResourceClient(
            IReadOnlyDictionary<string, IReadOnlyList<ApiHistoryPeriod>> periodsByAccount,
            PublishedPairIdentity pair)
        {
            this.periodsByAccount = periodsByAccount;
            this.pair = pair;
        }

        public string? FailPeriodId { get; set; }
        public bool FailPages { get; set; }
        public string? MismatchPairForPeriodId { get; set; }
        public string? DelayPeriodId { get; set; }
        public int HistoryPeriodsCalls => Volatile.Read(ref historyPeriodsCalls);
        public TaskCompletionSource DelayedPageStarted => delayedPageStarted;
        public TaskCompletionSource ReleaseDelayedPage { get; } =
            new(TaskCreationOptions.RunContinuationsAsynchronously);

        public IReadOnlyList<(string AccountId, string PeriodId, string? Cursor)> PageRequests
        {
            get
            {
                lock (gate)
                {
                    return pageRequests.ToArray();
                }
            }
        }

        public IReadOnlyList<string> AccountIds
        {
            get
            {
                lock (gate)
                {
                    return pageRequests.Select(request => request.AccountId).ToArray();
                }
            }
        }

        public void ClearPageRequests()
        {
            lock (gate)
            {
                pageRequests.Clear();
            }
        }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var detailsPeriods = periodsByAccount.TryGetValue(string.Empty, out var periods)
                ? periods
                : periodsByAccount.Values.First();
            return Task.FromResult(DetailsFetchResult.Success(CreateDetails(detailsPeriods, pair)));
        }

        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(CurrentFetchResult.FromFailure(DetailsFetchFailure.Response));

        public Task<CurrentFetchResult> FetchCurrentAsync(
            string accountId,
            CancellationToken cancellationToken = default) =>
            Task.FromResult(CurrentFetchResult.FromFailure(DetailsFetchFailure.Response));

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(CancellationToken cancellationToken = default) =>
            FetchHistoryPeriodsForAccountAsync(string.Empty);

        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(
            string accountId,
            CancellationToken cancellationToken = default) =>
            FetchHistoryPeriodsForAccountAsync(accountId);

        public Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default) =>
            FetchHistoryPageForAccountAsync(string.Empty, periodId, cursor, cancellationToken);

        public Task<HistoryPageFetchResult> FetchHistoryPageAsync(
            string accountId,
            string periodId,
            string? cursor = null,
            CancellationToken cancellationToken = default) =>
            FetchHistoryPageForAccountAsync(accountId, periodId, cursor, cancellationToken);

        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(ThreadsFetchResult.FromFailure(DetailsFetchFailure.Response));

        public Task<ThreadsFetchResult> FetchThreadsAsync(
            string accountId,
            CancellationToken cancellationToken = default) =>
            Task.FromResult(ThreadsFetchResult.FromFailure(DetailsFetchFailure.Response));

        private Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsForAccountAsync(string accountId)
        {
            Interlocked.Increment(ref historyPeriodsCalls);
            if (!periodsByAccount.TryGetValue(accountId, out var periods))
            {
                return Task.FromResult(HistoryPeriodsFetchResult.FromFailure(DetailsFetchFailure.Response));
            }

            return Task.FromResult(HistoryPeriodsFetchResult.Success(new ApiHistoryPeriodsSnapshot(
                periods.Select(period => period with { Samples = [] }).ToArray(),
                pair)
            {
                AccountId = accountId.Length == 0 ? null : accountId,
            }));
        }

        private async Task<HistoryPageFetchResult> FetchHistoryPageForAccountAsync(
            string accountId,
            string periodId,
            string? cursor,
            CancellationToken cancellationToken)
        {
            lock (gate)
            {
                pageRequests.Add((accountId, periodId, cursor));
            }

            if (periodId == DelayPeriodId && !delayHasBeenReleased)
            {
                delayedPageStarted.TrySetResult();
                await ReleaseDelayedPage.Task.WaitAsync(cancellationToken);
                delayHasBeenReleased = true;
            }

            if (FailPages && periodId == FailPeriodId)
            {
                return HistoryPageFetchResult.FromFailure(DetailsFetchFailure.Transport);
            }

            if (!periodsByAccount.TryGetValue(accountId, out var periods) ||
                periods.FirstOrDefault(period => period.Id == periodId) is not { } period)
            {
                return HistoryPageFetchResult.FromFailure(DetailsFetchFailure.Response);
            }

            var pagePair = periodId == MismatchPairForPeriodId
                ? PublishedPairIdentity.Create($"v1:{new string('e', 64)}")
                : pair;
            return HistoryPageFetchResult.Success(new ApiHistoryPage(
                period.Id,
                period.Samples,
                [],
                NextCursor: null,
                ResumeCursor: $"resume-{accountId}-{periodId}",
                pagePair)
            {
                AccountId = accountId.Length == 0 ? null : accountId,
            });
        }
    }
}
