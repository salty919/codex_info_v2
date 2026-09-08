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
        Assert.Equal(graph.Texts.UnavailableValue, graph.SelectedPeriodText);

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
            Assert.Equal(graph.Texts.Dollars, graph.MetricOptions[0]);
            Assert.Equal(graph.Texts.Tokens, graph.MetricOptions[1]);
            Assert.Equal(graph.Texts.UnavailableValue, graph.SelectedPeriodText);
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
        Assert.Equal(second.Samples[^1].Timestamp, graph.Points[^1].Timestamp);
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

        graph.SelectedPeriod = second;
        Assert.True(graph.HasPoints);
        Assert.Equal(second.Id, graph.SelectedPeriod?.Id);
        Assert.Equal(second.Label, graph.SelectedPeriodText);

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

        ((INotifyCollectionChanged)graph.Periods).CollectionChanged += (_, _) =>
        {
            if (graph.Periods.Count > 0)
            {
                graph.SelectedPeriod = graph.Periods[0];
            }
        };

        await PumpUiUntilAsync(pendingUi, () => graph.HasPoints);
        await Task.Delay(50);

        Assert.Equal(1, resourceClient.HistoryPeriodsCalls);
        Assert.Equal(1, resourceClient.HistoryPageCalls);
        Assert.Same(graph.Periods[0], graph.SelectedPeriod);
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
    public async Task GraphWindow_PublishedPairChangeUsesProvenCursorAndAtomicallyAppends()
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
        Assert.Single(graph.Points);
        Assert.Equal(firstSample.Timestamp, graph.Points[0].Timestamp);

        await RefreshGraphResourceAsync(graph, period.Id);
        await PumpUiUntilAsync(
            pendingUi,
            () => !graph.HasLoadError &&
                graph.Points.Any(point => point.Timestamp == secondSample.Timestamp));

        Assert.Equal([null, "A0"], resourceClient.Cursors);
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
        Assert.Single(graph.Points);
        Assert.Equal(samples[0].Timestamp, graph.Points[0].Timestamp);
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
    public void ThreadsWindow_ViewportShowsSixCardsAndScrollsOnlyTheList()
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
        Assert.Equal("56", cardHeightSetter.Attribute("Value")?.Value);

        const int cardHeight = 56;
        const int cardGap = 4;
        const int visibleCardCount = 6;
        const int viewportHeight = cardHeight * visibleCardCount + cardGap * visibleCardCount;

        var listScrollViewer = Assert.Single(document.Descendants(), element => element.Name.LocalName == "ScrollViewer");
        Assert.Equal("2", listScrollViewer.Attribute("Grid.Row")?.Value);
        Assert.Equal(viewportHeight.ToString(CultureInfo.InvariantCulture), listScrollViewer.Attribute("Height")?.Value);
        Assert.Equal("Top", listScrollViewer.Attribute("VerticalAlignment")?.Value);
        Assert.Equal("Disabled", listScrollViewer.Attribute("HorizontalScrollBarVisibility")?.Value);
        Assert.Equal("Auto", listScrollViewer.Attribute("VerticalScrollBarVisibility")?.Value);

        var card = document.Descendants()
            .Single(element => element.Name.LocalName == "Border" && element.Attribute("Classes")?.Value == "thread-card");
        Assert.Equal("0,0,0,4", card.Attribute("Margin")?.Value);
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
            var pair = Interlocked.Increment(ref periodsCalls) == 1 ? firstPair : secondPair;
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
            var sample = call == 1 ? firstSample : secondSample;
            var pair = call == 1 ? firstPair : secondPair;
            return Task.FromResult(HistoryPageFetchResult.Success(new ApiHistoryPage(
                periodId,
                [sample],
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
