// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Reflection;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphHistoryRecoveryEvidenceTests : IDisposable
{
    private readonly string previousLanguage = LocalizationService.Current.LanguageCode;

    public GraphHistoryRecoveryEvidenceTests() => LocalizationService.SetLanguage("en");

    public void Dispose() => LocalizationService.SetLanguage(previousLanguage);

    [Fact]
    public void FirstObservationAndUnrecordedOwnershipAreVisibleWithoutInventingUsage()
    {
        using var main = new MainWindowViewModel(new UnusedClient());
        using var graph = new GraphWindowViewModel(main);
        var scene = GraphScene.Create(
            [Sample(1_000, 89), Sample(1_060, 75), Sample(4_000, 67)],
            GraphMetric.Tokens, 1_000, 4_000,
            confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals: [new(1_000, 1_060), new(4_000, null)],
            resetAt: 605_800, publishedPeriodStartAt: 1_000);
        SetScene(graph, scene);

        Assert.Contains("89%", graph.GraphGapHintText);
        Assert.Contains("First recorded in this view", graph.GraphGapHintText);
        Assert.Contains("Dashed:", graph.GraphGapHintText);
        Assert.Contains("not recorded for this account", graph.GraphGapHintText);
        Assert.Contains("unknown", graph.GraphGapHintText);
        Assert.DoesNotContain("100%", graph.GraphGapHintText);
        Assert.Equal(89, scene.Remaining[0]);
    }

    [Theory]
    [InlineData(1_000, 4_000, 1_060, 4_000)]
    [InlineData(2_000, 3_000, 2_000, 3_000)]
    public void TimeWindowKeepsClippedUnrecordedAccountIntervals(long start, long end, long expectedStart, long expectedEnd)
    {
        using var main = new MainWindowViewModel(new UnusedClient());
        using var graph = new GraphWindowViewModel(main);
        var child = GraphScene.Create([Sample(1_000, 89), Sample(4_000, 67)],
            GraphMetric.Tokens, 1_000, 4_000,
            confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals: [new(1_000, 1_060), new(4_000, null)]);
        var viewport = GraphScene.CreateViewport(start, end, GraphMetric.Tokens, [child]);
        SetScene(graph, viewport);
        var interval = Assert.Single(viewport.NonOwnedIntervals);
        Assert.Equal(expectedStart, interval.StartAt);
        Assert.Equal(expectedEnd, interval.EndAt);
        Assert.Contains("not recorded for this account", graph.GraphGapHintText);
        Assert.Contains("unknown", graph.GraphGapHintText);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void MissingQuotaKeepsUncertaintyExplanationWithoutInventingFirstValue(bool nonOwned)
    {
        using var main = new MainWindowViewModel(new UnusedClient());
        using var graph = new GraphWindowViewModel(main);
        var scene = GraphScene.Create(
            [Sample(1_000, 89) with { RemainingPercent = null }, Sample(4_000, 75) with { RemainingPercent = null }],
            GraphMetric.Tokens, 1_000, 4_000,
            confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals: nonOwned ? [new(1_000, 1_060), new(4_000, null)] : null);
        SetScene(graph, scene);
        Assert.Contains("Dashed:", graph.GraphGapHintText);
        Assert.DoesNotContain("First recorded", graph.GraphGapHintText);
        Assert.DoesNotContain("%", graph.GraphGapHintText);
        if (nonOwned) Assert.Contains("not recorded for this account", graph.GraphGapHintText);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void PublishedSceneNotifiesVisibleHistoryEvidence(bool timeWindow)
    {
        using var main = new MainWindowViewModel(new UnusedClient());
        using var graph = new GraphWindowViewModel(main);
        var scene = GraphScene.Create([Sample(1_000, 89), Sample(1_060, 75)], GraphMetric.Tokens, 1_000, 1_060);
        string? publishedText = null;
        graph.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(GraphWindowViewModel.GraphGapHintText)) publishedText = graph.GraphGapHintText;
        };
        var projectionType = typeof(GraphWindowViewModel).GetNestedType(timeWindow ? "WindowGraphProjection" : "GraphProjection", BindingFlags.NonPublic)!;
        var projection = Activator.CreateInstance(projectionType, timeWindow
            ? [Array.Empty<GraphPointViewModel>(), scene, null]
            : [Array.Empty<GraphPointViewModel>(), scene]);
        var publish = typeof(GraphWindowViewModel).GetMethod(timeWindow ? "PublishWindowGraphProjection" : "PublishPoints", BindingFlags.Instance | BindingFlags.NonPublic)!;
        publish.Invoke(graph, timeWindow ? [projection, GraphTimeRange.Last24Hours, null] : [projection, null, GraphMetric.Tokens]);
        Assert.NotNull(publishedText);
        Assert.Contains("89%", publishedText);
    }

    [Fact]
    public void RestoredQuotaIsShownWithoutClaimingUnknownModelsOrMissingUsageAreZero()
    {
        using var main = new MainWindowViewModel(new UnusedClient());
        using var graph = new GraphWindowViewModel(main);
        var recovered = Sample(1_060, 80) with
        {
            ModelSource = ApiHistorySample.UnavailableModelSource,
            ModelSamples = null,
            ModelsComplete = false,
        };
        var scene = GraphScene.Create(
            [Sample(1_000, 89), recovered, Sample(1_120, 75)],
            GraphMetric.Tokens, 1_000, 1_120,
            confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals: null,
            resetAt: 605_800, publishedPeriodStartAt: 1_000);
        SetScene(graph, scene);

        Assert.Contains("First recorded in this view", graph.GraphGapHintText);
        Assert.Contains("Dashed:", graph.GraphGapHintText);
        Assert.Contains("89%", graph.GraphGapHintText);
        Assert.DoesNotContain("not recorded for this account", graph.GraphGapHintText);
        Assert.Equal(80, scene.Remaining[1]);
        Assert.Null(recovered.ModelSamples);
        Assert.False(recovered.ModelsComplete);
    }

    [Theory]
    [InlineData(null, null)]
    [InlineData(50d, 45d)]
    public void LocalUnusedTokensDoNotRequireAnUnchangedQuota(double? firstQuota, double? laterQuota)
    {
        // The recorded local token tuple is unchanged for exactly ten minutes.
        // Account quota may be unavailable or change independently of this log.
        var samples = Enumerable.Range(0, 11).Select(index =>
            new ApiHistorySample(1_020 + index * 60, 605_800,
                index == 0 ? firstQuota : laterQuota, null, null, null, null, null, null)
            {
                ModelsComplete = true,
                ModelSamples =
                [
                    new("gpt-6.1-sol", 90, 40, 10, 0.02)
                    {
                        TotalTokens = 100,
                        CacheWriteInputTokens = 0,
                    },
                ],
                TaskActiveSincePrevious = true,
            }).ToArray();
        var scene = GraphScene.Create(samples, GraphMetric.Tokens, 1_020, 1_620,
            confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals: [new(1_000, null)]);

        var idle = Assert.Single(scene.IdleIntervals);
        Assert.Equal(1_020, idle.StartAt);
        Assert.Equal(1_620, idle.EndAt);
    }

    [Theory]
    [InlineData(GraphMetric.Tokens)]
    [InlineData(GraphMetric.Dollars)]
    public void LocalUnusedBandPreservesIndependentMeasuredQuotaPlateaus(GraphMetric metric)
    {
        double[] quota = [90, 90, 89, 89, 88];
        var samples = quota.Select((remaining, index) =>
            new ApiHistorySample(1_020 + index * 300, 605_800,
                remaining, null, null, null, null, null, null)
            {
                ModelsComplete = true,
                ModelSamples = [new("gpt-6.1-sol", 90, 40, 10, 0.02)
                    { TotalTokens = 100, CacheWriteInputTokens = 0 }],
                LocalUsage = index == 0
                    ? new(1_020, 2_221, [new(1_020, 2_221, "unused")])
                    : null,
            }).ToArray();
        var scene = GraphScene.Create(samples, metric, 1_020, 2_220,
            confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals: [new(1_000, null)]);

        var idle = Assert.Single(scene.IdleIntervals);
        Assert.Equal(1_020, idle.StartAt);
        Assert.Equal(2_220, idle.EndAt);
        var line = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Empty(line.Idle.X);
        Assert.Empty(line.Dashed.X);
        Assert.Equal([1_020d, 1_320d, 1_620d, 1_920d, 2_220d], line.Solid.X);
        Assert.Equal(quota, line.Solid.Y);
        Assert.Equal(quota, samples.Select(sample => sample.RemainingPercent!.Value));
    }

    [Theory]
    [InlineData(GraphMetric.Tokens)]
    [InlineData(GraphMetric.Dollars)]
    public void LocalLosslessPartialTokensProduceTheSameUnusedBandInBothMetrics(GraphMetric metric)
    {
        var samples = Enumerable.Range(0, 11).Select(index =>
            new ApiHistorySample(1020 + index * 60, 605800, null, null, null, null, null, null, null)
            {
                ModelsComplete = false,
                ModelSamples = [new("gpt-6.1-sol", 90, 40, 10, 0.02)
                    { TotalTokens = 100, CacheWriteInputTokens = 0 }],
                TaskActiveSincePrevious = true,
            }).ToArray();
        var scene = GraphScene.Create(samples, metric, 1020, 1620,
            confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals: [new(1000, null)]);
        var interval = Assert.Single(scene.IdleIntervals);
        Assert.Equal(1020, interval.StartAt);
        Assert.Equal(1620, interval.EndAt);
        Assert.All(samples, sample => Assert.False(sample.ModelsComplete));
    }

    [Fact]
    public void LocalLogEvidenceHoldsKnownTokensAcrossQuotaAndModelSamplingGaps()
    {
        var evidence = new ApiLocalUsage(1020, 1681,
            [new(1020, 1080, "used"), new(1080, 1681, "unused")]);
        var known = new ApiHistorySample(1020, 605800, 50, null, null, null, null, null, null)
        {
            ModelsComplete = false,
            ModelSamples = [new("gpt-6.1-sol", 90, 40, 10, 0.02)
                { TotalTokens = 100, CacheWriteInputTokens = 0 }],
            LocalUsage = evidence,
        };
        var missing = new ApiHistorySample(1620, 605800, 45, null, null, null, null, null, null,
            ApiHistorySample.UnavailableModelSource)
        { ModelsComplete = false };
        var scene = GraphScene.Create([known, missing], GraphMetric.Tokens, 1020, 1680,
            confirmedGaps: [new(1100, 1600)],
            hiddenModelNames: null, accountOwnershipIntervals: [new(1000, null)]);
        var interval = Assert.Single(scene.IdleIntervals);
        Assert.Equal(1080, interval.StartAt);
        Assert.Equal(1680, interval.EndAt);
        var line = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        Assert.Contains(1080d, line.Idle.X);
        Assert.Contains(1680d, line.Idle.X);
        Assert.All(line.Idle.Y.Where(double.IsFinite), value => Assert.Equal(100, value));
        Assert.DoesNotContain(line.Dashed.X.Where(double.IsFinite), at => at > 1080 && at < 1680);
        Assert.Null(missing.ModelSamples);
        Assert.Equal(45, missing.RemainingPercent);

        var unavailable = known with { LocalUsage = new(1020, 1681, [new(1020, 1681, "unavailable")]) };
        var unknownScene = GraphScene.Create([unavailable, missing], GraphMetric.Tokens, 1020, 1680,
            confirmedGaps: null, hiddenModelNames: null, accountOwnershipIntervals: [new(1000, null)]);
        Assert.Empty(unknownScene.IdleIntervals);
    }

    [Fact]
    public void ValidEmptyLocalIndexCanMarkUnusedWithoutCreatingANumericSeries()
    {
        var sample = new ApiHistorySample(1020, 605800, null, null, null, null, null, null, null,
            ApiHistorySample.UnavailableModelSource)
        { ModelsComplete = false, LocalUsage = new(1020, 1681, [new(1020, 1681, "unused")]) };
        var scene = GraphScene.Create([sample], GraphMetric.Tokens, 1020, 1680,
            confirmedGaps: null, hiddenModelNames: null, accountOwnershipIntervals: [new(1000, null)]);
        Assert.Single(scene.IdleIntervals);
        Assert.All(scene.Sol, value => Assert.False(double.IsFinite(value)));
    }

    private static ApiHistorySample Sample(long timestamp, double remaining) =>
        new(timestamp, 605_800, remaining, 1, 0, 0, 100, 0, 0);

    private static void SetScene(GraphWindowViewModel graph, GraphScene scene)
    {
        var field = typeof(GraphWindowViewModel).GetField("scene", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        field.SetValue(graph, scene);
    }

    private sealed class UnusedClient : ILoopbackHealthClient, ILoopbackDetailsClient
    {
        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("This projection test must not fetch health.");

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("This projection test must not fetch details.");
    }
}
