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

        var changed = new HashSet<string>();
        graph.PropertyChanged += (_, args) => changed.Add(args.PropertyName ?? string.Empty);
        var previousLanguage = LocalizationService.Current.LanguageCode;
        var nextLanguage = previousLanguage.Equals("en", StringComparison.OrdinalIgnoreCase) ? "ja" : "en";

        try
        {
            LocalizationService.SetLanguage(nextLanguage);

            Assert.Contains(nameof(GraphWindowViewModel.Texts), changed);
            Assert.Contains(nameof(GraphWindowViewModel.MetricOptions), changed);
            Assert.Contains(nameof(GraphWindowViewModel.MetricAxisText), changed);
            Assert.Equal(graph.Texts.GraphDollarMetric, graph.MetricOptions[0]);
            Assert.Equal(graph.Texts.Tokens, graph.MetricOptions[1]);
            Assert.Equal($"{graph.Texts.PeriodSelectorHeading}｜{graph.Texts.UnavailableValue}", graph.SelectedPeriodText);
        }
        finally
        {
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    [Fact]
    public async Task GraphWindow_DetailsRefreshKeepsMetricOptionsIdentityAndNotificationSilent()
    {
        var period = CreateSmallPeriod("current", 2_000_000, 2_000_120, current: false, remaining: 80, token: 100);
        using var main = await StartMainAsync(CreateDetails([period], Array.Empty<ApiThreadDetails>()));
        using var graph = new GraphWindowViewModel(main);

        await EventuallyAsync(() => main.CanRefresh);
        var initialOptions = graph.MetricOptions;
        var changed = new List<string?>();
        graph.PropertyChanged += (_, args) => changed.Add(args.PropertyName);

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => changed.Contains(nameof(GraphWindowViewModel.SelectedPeriod)));

        Assert.Same(initialOptions, graph.MetricOptions);
        Assert.DoesNotContain(nameof(GraphWindowViewModel.MetricOptions), changed);

        var previousLanguage = LocalizationService.Current.LanguageCode;
        var nextLanguage = previousLanguage.Equals("en", StringComparison.OrdinalIgnoreCase) ? "ja" : "en";
        try
        {
            changed.Clear();
            LocalizationService.SetLanguage(nextLanguage);

            Assert.Contains(nameof(GraphWindowViewModel.MetricOptions), changed);
            Assert.NotSame(initialOptions, graph.MetricOptions);
        }
        finally
        {
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    [Fact]
    public async Task GraphWindow_CancelledLargeBuildCannotOverwriteLatestPeriod()
    {
        var first = CreateLargePeriod("first", 2_100_000, 2_104_600, seed: 1);
        var second = CreateLargePeriod("second", 2_110_000, 2_114_600, seed: 2);
        using var main = await StartMainAsync(CreateDetails(new[] { first, second }, Array.Empty<ApiThreadDetails>()));
        var pendingUi = new ConcurrentQueue<Action>();
        using var graph = new GraphWindowViewModel(main, action => pendingUi.Enqueue(action));

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
    }

    [Fact]
    public async Task GraphWindow_MissingPeriodMetricAndToggleBoundariesAreStable()
    {
        var first = CreateSmallPeriod("first", 3_000_000, 3_000_120, current: false, remaining: 80, token: 100);
        var second = CreateSmallPeriod("second", 3_001_000, 3_001_120, current: false, remaining: 40, token: 200);
        using var main = await StartMainAsync(CreateDetails(new[] { first, second }, Array.Empty<ApiThreadDetails>()));
        using var graph = new GraphWindowViewModel(main);

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
        Assert.Equal(main.DetailsStatusText, threads.DetailsStatusText);

        var previousLanguage = LocalizationService.Current.LanguageCode;
        var nextLanguage = previousLanguage.Equals("en", StringComparison.OrdinalIgnoreCase) ? "ja" : "en";
        try
        {
            LocalizationService.SetLanguage(nextLanguage);
            Assert.Equal(threads.Texts.NoRunningThreads, threads.EmptyText);
            Assert.NotEqual(string.Empty, threads.DetailsStatusText);
        }
        finally
        {
            LocalizationService.SetLanguage(previousLanguage);
        }
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
        Assert.Equal("2", listScrollViewer.Attribute("Grid.Row")?.Value);
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
        var method = typeof(GraphWindowViewModel).GetMethod(
            "RefreshSplitResourceAsync",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(method);
        var task = Assert.IsAssignableFrom<Task>(method!.Invoke(
            graph,
            [false, periodId, CancellationToken.None]));
        await task;
    }

    private sealed class StaticCombinedClient(DetailsFetchResult result) : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(result);
    }

    private sealed class StaticDetailsClient(DetailsFetchResult result) : ILoopbackDetailsClient
    {
        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(result);
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
