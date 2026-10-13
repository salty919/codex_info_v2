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
