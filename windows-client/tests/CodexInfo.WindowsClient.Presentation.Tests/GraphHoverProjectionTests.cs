// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphHoverProjectionTests
{
    [Fact]
    public void FindUsesNearestObservationAndBreaksTiesTowardEarlierTimestamp()
    {
        var scene = CreateScene(
        [
            Sample(100, 1, 90, Model("SOL", 100, 1)),
            Sample(200, 1, 80, Model("SOL", 200, 2)),
            Sample(300, 1, 70, Model("SOL", 300, 3)),
        ],
        GraphMetric.Tokens,
        100,
        300);

        Assert.Equal(200, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(scene, 176, Visible(GraphSeries.Sol))).Timestamp);
        Assert.Equal(100, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(scene, 150, Visible(GraphSeries.Sol))).Timestamp);
    }

    [Fact]
    public void FindRejectsCoordinatesOutsideSceneAndViewportBounds()
    {
        var scene = CreateScene(
        [
            Sample(100, 1, 90, Model("SOL", 100, 1)),
            Sample(200, 1, 80, Model("SOL", 200, 2)),
            Sample(300, 1, 70, Model("SOL", 300, 3)),
        ],
        GraphMetric.Tokens,
        100,
        300);

        Assert.Null(GraphHoverProjection.Find(scene, 99, Visible(GraphSeries.Sol)));
        Assert.Null(GraphHoverProjection.Find(scene, 301, Visible(GraphSeries.Sol)));

        var clippedScene = CreateScene(
            [Sample(100, 1, 90, Model("SOL", 100, 1)), Sample(300, 1, 70, Model("SOL", 300, 3))],
            GraphMetric.Tokens,
            150,
            250);
        Assert.Null(GraphHoverProjection.Find(clippedScene, 150, Visible(GraphSeries.Sol)));
        Assert.Null(GraphHoverProjection.Find(clippedScene, 250, Visible(GraphSeries.Sol)));

        var viewport = GraphScene.CreateViewport(150, 250, GraphMetric.Tokens, [scene]);
        Assert.Null(GraphHoverProjection.Find(viewport, 149, Visible(GraphSeries.Sol)));
        Assert.Null(GraphHoverProjection.Find(viewport, 251, Visible(GraphSeries.Sol)));
        Assert.Equal(200, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(viewport, 150, Visible(GraphSeries.Sol))).Timestamp);
    }

    [Fact]
    public void FindNeverBorrowsAnObservationFromAnotherResetPeriod()
    {
        var firstPeriod = CreateScene(
            [Sample(110, 100, 90, Model("SOL", 10, 1)), Sample(190, 100, 80, Model("SOL", 20, 2))],
            GraphMetric.Tokens,
            100,
            199,
            resetAt: 100);
        var secondPeriod = CreateScene(
            [Sample(210, 200, 70, Model("SOL", 30, 3)), Sample(290, 200, 60, Model("SOL", 40, 4))],
            GraphMetric.Tokens,
            200,
            300,
            resetAt: 200);
        var viewport = GraphScene.CreateViewport(100, 300, GraphMetric.Tokens, [firstPeriod, secondPeriod]);

        Assert.Equal(190, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(viewport, 199, Visible(GraphSeries.Sol))).Timestamp);
        Assert.Null(GraphHoverProjection.Find(viewport, 199.5, Visible(GraphSeries.Sol)));
        Assert.Equal(210, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(viewport, 200, Visible(GraphSeries.Sol))).Timestamp);
    }

    [Fact]
    public void FindDoesNotSelectOrCrossAConfirmedGap()
    {
        var scene = GraphScene.Create(
        [
            Sample(100, 1, 90, Model("SOL", 100, 1)),
            Sample(200, 1, 80, Model("SOL", 200, 2)),
            Sample(300, 1, 70, Model("SOL", 300, 3)),
        ],
        GraphMetric.Tokens,
        100,
        300,
        [new GraphConfirmedGap(150, 250)]);

        Assert.Equal(100, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(scene, 120, Visible(GraphSeries.Sol))).Timestamp);
        Assert.Null(GraphHoverProjection.Find(scene, 175, Visible(GraphSeries.Sol)));
        Assert.Equal(300, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(scene, 280, Visible(GraphSeries.Sol))).Timestamp);
    }

    [Fact]
    public void FindDoesNotSelectOrCrossANonOwnedInterval()
    {
        var scene = GraphScene.Create(
            [
                Sample(100, 1, 90, Model("SOL", 100, 1)),
                Sample(200, 1, 80, Model("SOL", 200, 2)),
                Sample(300, 1, 70, Model("SOL", 300, 3)),
            ],
            GraphMetric.Tokens,
            100,
            300,
            confirmedGaps: null,
            hiddenModelNames: null,
            accountOwnershipIntervals:
            [
                new GraphAccountOwnershipInterval(100, 150),
                new GraphAccountOwnershipInterval(250, 300),
            ]);

        Assert.Equal(100, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(scene, 120, Visible(GraphSeries.Sol))).Timestamp);
        Assert.Null(GraphHoverProjection.Find(scene, 200, Visible(GraphSeries.Sol)));
        Assert.Equal(300, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(scene, 280, Visible(GraphSeries.Sol))).Timestamp);
    }

    [Fact]
    public void FindExcludesSyntheticTailAndRowsDroppedAsRecoverableSamplingJitter()
    {
        var synthetic = Sample(200, 1, 80, Model("SOL", 200, 2)) with { IsSyntheticTail = true };
        var syntheticScene = CreateScene(
            [Sample(100, 1, 90, Model("SOL", 100, 1)), synthetic],
            GraphMetric.Tokens,
            100,
            200);

        Assert.Equal(100, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(syntheticScene, 199, Visible(GraphSeries.Sol))).Timestamp);

        var jitterScene = CreateScene(
        [
            Sample(1_000, 1, 90, Model("SOL", 100, 1)),
            Sample(1_060, 1, 90) with
            {
                ModelSource = ApiHistorySample.UnavailableModelSource,
                ModelsComplete = false,
            },
            Sample(1_120, 1, 90, Model("SOL", 100, 1)),
        ],
        GraphMetric.Tokens,
        1_000,
        1_120);

        Assert.Equal(1_000, Assert.IsType<GraphHoverSnapshot>(
            GraphHoverProjection.Find(jitterScene, 1_060, Visible(GraphSeries.Sol))).Timestamp);
    }

    [Fact]
    public void FindReturnsVisibleRowsAtOneTimestampAndLeavesMissingValuesUnknown()
    {
        var scene = CreateScene(
        [
            Sample(100, 1, 90, Model("SOL", 100, 1), Model("LUNA", 50, 0.5)),
            Sample(
                200,
                1,
                null,
                Model("SOL", null, null),
                Model("TERRA", null, null),
                Model("LUNA", null, null),
                Model("ASTRA", null, null)),
        ],
        GraphMetric.Tokens,
        100,
        200);

        var snapshot = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            scene,
            200,
            Visible(GraphSeries.Remaining, GraphSeries.Luna, GraphSeries.Terra, GraphSeries.Sol, GraphSeries.Astra)));

        Assert.Equal(200, snapshot.Timestamp);
        Assert.Equal(
            new[]
            {
                GraphSeries.Remaining,
                GraphSeries.Luna,
                GraphSeries.Terra,
                GraphSeries.Sol,
                GraphSeries.Astra,
            },
            snapshot.Rows.Select(row => row.Series));
        Assert.All(snapshot.Rows, row =>
        {
            Assert.Null(row.TokenValue);
            Assert.Null(row.NumericValue);
        });
    }

    [Fact]
    public void FindReturnsEveryVisibleValueFromTheSameObservedTimestamp()
    {
        const ulong solTokens = 9_007_199_254_740_993;
        const ulong terraTokens = 4_000_000_000_000_007;
        const ulong lunaTokens = 3_000_000_000_000_009;
        const ulong astraTokens = 2_000_000_000_000_011;
        var scene = CreateScene(
        [
            Sample(
                200,
                1,
                73,
                Model("SOL", solTokens, 1),
                Model("TERRA", terraTokens, 2),
                Model("LUNA", lunaTokens, 3),
                Model("ASTRA", astraTokens, 4)),
        ],
        GraphMetric.Tokens,
        100,
        300);

        var snapshot = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            scene,
            200,
            Visible(GraphSeries.Remaining, GraphSeries.Luna, GraphSeries.Terra, GraphSeries.Sol, GraphSeries.Astra)));

        Assert.Equal(200, snapshot.Timestamp);
        Assert.Equal(73, Assert.Single(snapshot.Rows, row => row.Series == GraphSeries.Remaining).NumericValue);
        Assert.Equal(solTokens, Assert.Single(snapshot.Rows, row => row.Series == GraphSeries.Sol).TokenValue);
        Assert.Equal(terraTokens, Assert.Single(snapshot.Rows, row => row.Series == GraphSeries.Terra).TokenValue);
        Assert.Equal(lunaTokens, Assert.Single(snapshot.Rows, row => row.Series == GraphSeries.Luna).TokenValue);
        Assert.Equal(astraTokens, Assert.Single(snapshot.Rows, row => row.Series == GraphSeries.Astra).TokenValue);
        Assert.All(snapshot.Rows, row =>
        {
            if (row.Series == GraphSeries.Remaining)
            {
                Assert.Null(row.TokenValue);
            }
            else
            {
                Assert.Null(row.NumericValue);
            }
        });
    }

    [Fact]
    public void FindUsesAcceptedDollarProjectionAndExactRawTokenTotals()
    {
        var dollarScene = CreateScene(
        [
            Sample(100, 1, 90, Model("SOL", 10, 1)),
            Sample(200, 1, 80, Model("SOL", 10, 5)),
            Sample(300, 1, 70, Model("SOL", 10, 9)),
        ],
        GraphMetric.Dollars,
        100,
        300);

        var dollar = Assert.Single(Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            dollarScene,
            200,
            Visible(GraphSeries.Sol))).Rows);
        Assert.Equal(dollarScene.ModelSeries["SOL"][1], dollar.NumericValue);
        Assert.NotEqual(5, dollar.NumericValue);
        Assert.Null(dollar.TokenValue);

        const ulong exactTokens = 9_007_199_254_740_993;
        var tokenScene = CreateScene(
            [Sample(100, 1, 90, Model("SOL", exactTokens, 1))],
            GraphMetric.Tokens,
            100,
            200);
        var token = Assert.Single(Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            tokenScene,
            100,
            Visible(GraphSeries.Sol))).Rows);

        Assert.Equal(exactTokens, token.TokenValue);
        Assert.Null(token.NumericValue);
    }

    [Fact]
    public void FindKeepsTheSelectedTimestampIndependentOfVisibleSeries()
    {
        var scene = CreateScene(
        [
            Sample(100, 1, 90, Model("SOL", 100, 1)),
            Sample(200, 1, 80, Model("SOL", 200, 2)),
        ],
        GraphMetric.Tokens,
        100,
        200);

        var solOnly = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            scene,
            170,
            Visible(GraphSeries.Sol)));
        var remainingOnly = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            scene,
            170,
            Visible(GraphSeries.Remaining)));
        var noneVisible = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            scene,
            170,
            new HashSet<GraphSeries>()));

        Assert.Equal(solOnly.Timestamp, remainingOnly.Timestamp);
        Assert.Equal(solOnly.Timestamp, noneVisible.Timestamp);
        Assert.Equal(new[] { GraphSeries.Sol }, solOnly.Rows.Select(row => row.Series));
        Assert.Equal(new[] { GraphSeries.Remaining }, remainingOnly.Rows.Select(row => row.Series));
        Assert.Empty(noneVisible.Rows);
    }

    [Fact]
    public void FindDoesNotExposeHeldOrInterpolatedValuesAsObservedMeasurements()
    {
        var scene = CreateScene(
        [
            Sample(100, 1, 90, Model("SOL", 100, 1)),
            Sample(200, 1, null) with
            {
                ModelSource = ApiHistorySample.UnavailableModelSource,
                ModelsComplete = false,
            },
            Sample(300, 1, 80, Model("SOL", 120, 2)),
        ],
        GraphMetric.Dollars,
        100,
        300);
        Assert.True(double.IsFinite(scene.Remaining[1]));
        Assert.True(double.IsFinite(scene.ModelSeries["SOL"][1]));

        var snapshot = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            scene,
            200,
            Visible(GraphSeries.Remaining, GraphSeries.Sol)));

        Assert.Equal(200, snapshot.Timestamp);
        Assert.All(snapshot.Rows, row =>
        {
            Assert.Null(row.TokenValue);
            Assert.Null(row.NumericValue);
        });
    }

    private static GraphScene CreateScene(
        IReadOnlyList<ApiHistorySample> samples,
        GraphMetric metric,
        long periodStart,
        long periodEnd,
        long? resetAt = null) =>
        GraphScene.Create(
            samples,
            metric,
            periodStart,
            periodEnd,
            confirmedGaps: null,
            hiddenModelNames: null,
            accountOwnershipIntervals: null,
            resetAt: resetAt);

    private static ApiHistorySample Sample(
        long timestamp,
        long resetAt,
        double? remaining,
        params ApiHistoryModelSample[] models) =>
        new(
            timestamp,
            resetAt,
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
            ModelSamples = models,
        };

    private static ApiHistoryModelSample Model(string name, ulong? tokens, double? dollars) =>
        new(name, null, null, null, dollars) { TotalTokens = tokens };

    private static IReadOnlySet<GraphSeries> Visible(params GraphSeries[] series) => series.ToHashSet();
}
