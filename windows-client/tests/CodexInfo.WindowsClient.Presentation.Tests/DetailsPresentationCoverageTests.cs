// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections.Concurrent;
using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
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
}
