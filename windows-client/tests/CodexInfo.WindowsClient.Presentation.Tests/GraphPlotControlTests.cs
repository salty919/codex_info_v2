// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphPlotControlTests
{
    private const string PublishedPairHeader = "Codex-Info-Published-Pair";
    private const string CanonicalPublishedPair =
        "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";

    [Fact]
    public void ContractMaximumReductionIsViewportBoundedAndPreservesExactEndpoints()
    {
        // Exercise the largest response permitted by the one-month transport
        // contract. The parser rejects larger responses before rendering.
        var samples = Enumerable.Range(0, 44_640)
            .Select(index => new ApiHistorySample(index + 1, 200_000, 100 - index / 1_000d, index, index * 2, index * 3, (ulong)index, (ulong)index * 2, (ulong)index * 3))
            .ToArray();

        var reduced = GraphWindowViewModel.ReduceGraphSamples(samples);

        Assert.Equal(GraphWindowViewModel.MaxRenderedGraphPoints, reduced.Count);
        Assert.Equal(samples[0], reduced[0]);
        Assert.Equal(samples[^1], reduced[^1]);
        Assert.True(reduced.Zip(reduced.Skip(1)).All(pair => pair.First.Timestamp < pair.Second.Timestamp));
    }

    [Fact]
    public void MandatoryBoundariesUseSoftViewportCapUpToTheEndpointHistoryLimit()
    {
        const int endpointHistoryLimit = 44_640;
        var samples = Enumerable.Range(0, endpointHistoryLimit)
            .Select(index => new ApiHistorySample(
                index + 1,
                200_000,
                null,
                index,
                index * 2,
                index * 3,
                (ulong)index,
                (ulong)index * 2,
                (ulong)index * 3,
                index % 2 == 0
                    ? ApiHistorySample.ConfirmedModelSource
                    : ApiHistorySample.LegacyUnknownModelSource))
            .ToArray();

        var reduced = GraphWindowViewModel.ReduceGraphSamples(samples, 2_048);

        Assert.Equal(endpointHistoryLimit, reduced.Count);
        Assert.Equal(samples.Select(sample => sample.Timestamp), reduced.Select(sample => sample.Timestamp));
    }

    [Fact]
    public void Reduction_preserves_regression_quota_and_confirmed_gap_boundaries()
    {
        var samples = new[]
        {
            ConfirmedCumulativeSample(1, 176),
            ConfirmedCumulativeSample(2, 87),
            ConfirmedCumulativeSample(3, 88),
            ConfirmedCumulativeSample(4, 184),
        };

        var reduced = GraphWindowViewModel.ReduceGraphSamples(samples, maximum: 2);

        Assert.Equal(samples, reduced);
        Assert.Equal([1L, 2L, 3L, 4L], reduced.Select(sample => sample.Timestamp));

        var monotonic = Enumerable.Range(0, 6)
            .Select(index => ConfirmedCumulativeSample(index * 60, index))
            .ToArray();
        var acrossGap = GraphWindowViewModel.ReduceGraphSamples(
            monotonic,
            maximum: 2,
            confirmedGaps: [new GraphConfirmedGap(120, 180)]);
        Assert.Equal([0L, 120L, 180L, 300L], acrossGap.Select(sample => sample.Timestamp));

        var unattributedQuotaDrop = monotonic
            .Select((sample, index) => sample with
            {
                RemainingPercent = index < 2 ? 100 : 80,
                SolDollars = 0,
                TerraDollars = 0,
                LunaDollars = 0,
                SolTokens = 0,
                TerraTokens = 0,
                LunaTokens = 0,
            })
            .ToArray();
        var acrossQuotaDrop = GraphWindowViewModel.ReduceGraphSamples(
            unattributedQuotaDrop,
            maximum: 2);
        Assert.Equal([0L, 60L, 120L, 300L], acrossQuotaDrop.Select(sample => sample.Timestamp));
    }

    [Fact]
    public void ViewportReductionKeepsBothEdgesOfEveryMonotonicBucket()
    {
        var samples = Enumerable.Range(0, 12)
            .Select(index => new ApiHistorySample(
                index,
                100,
                100,
                index < 5 ? 0 : 10,
                0,
                0,
                0,
                0,
                0))
            .ToArray();

        var reduced = GraphWindowViewModel.ReduceGraphSamples(samples, 6);

        Assert.Equal([0L, 3L, 4L, 7L, 8L, 11L], reduced.Select(sample => sample.Timestamp));
        Assert.Equal(0, reduced[2].SolDollars);
        Assert.Equal(10, reduced[3].SolDollars);
    }

    [Fact]
    public void ScenePreservesAllIrregularSamplesAndExactEndpoints()
    {
        var source = Enumerable.Range(0, 2_048)
            .Select(index => new ApiHistorySample(
                index * index + 1,
                5_000_000,
                100 - index / 100d,
                index,
                index,
                index,
                (ulong)index,
                (ulong)index,
                (ulong)index))
            .ToArray();

        var scene = GraphScene.Create(source, GraphMetric.Dollars, 1, source[^1].Timestamp);

        Assert.Equal(source.Length, scene.Timestamps.Count);
        Assert.Equal(source[0].Timestamp, scene.Timestamps[0]);
        Assert.Equal(source[^1].Timestamp, scene.Timestamps[^1]);
        Assert.True(scene.Timestamps.Zip(scene.Timestamps.Skip(1)).All(pair => pair.First < pair.Second));
    }

    [Fact]
    public void EndpointLabelsStayNearTheirSeriesAndResolveOnlyActualCollisions()
    {
        var arranged = GraphScene.ArrangeEndpointLabelTops(
            [10, 80, 84, 170],
            top: 0,
            bottom: 200,
            labelHeight: 14,
            gap: 2);

        Assert.Equal(10, arranged[0]);
        Assert.Equal(80, arranged[1]);
        Assert.Equal(96, arranged[2]);
        Assert.Equal(170, arranged[3]);
        Assert.All(arranged, value => Assert.InRange(value, 0, 186));
        Assert.True(arranged.Zip(arranged.Skip(1)).All(pair => pair.Second - pair.First >= 16));
    }

    [Fact]
    public void EndpointLabelsAtBottomRemainBoundedAndNonCrossing()
    {
        var arranged = GraphScene.ArrangeEndpointLabelTops(
            [180, 181, 182, 183],
            top: 0,
            bottom: 200,
            labelHeight: 14,
            gap: 2);

        Assert.Equal(186, arranged[^1]);
        Assert.All(arranged, value => Assert.InRange(value, 0, 186));
        Assert.True(arranged.Zip(arranged.Skip(1)).All(pair => pair.Second - pair.First >= 16));
    }

    [Fact]
    public void PlotProjectionBuildsFrameworkIndependentAxisTicks()
    {
        var projection = GraphPlotProjection.BuildAxes(
            GraphScene.Empty(GraphMetric.Tokens),
            TimeZoneInfo.Utc,
            CultureInfo.InvariantCulture);

        Assert.Equal([0d, 0.25d, 0.5d, 0.75d, 1d], projection.BottomValues);
        Assert.Equal([0d, 0.25d, 0.5d, 0.75d, 1d], projection.ModelValues);
        Assert.Equal([0d, 25d, 50d, 75d, 100d], projection.RemainingValues);
        Assert.Equal(["0%", "25%", "50%", "75%", "100%"], projection.RemainingLabels);
        Assert.Equal(5, projection.BottomLabels.Count);
        Assert.Equal(5, projection.ModelLabels.Count);
    }

    [Fact]
    public void PlotProjectionReservesNativeHeadroomAndEndpointLabelGutter()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(2_000, 75, 2, 4, 6),
            ]);

        var projection = GraphPlotProjection.BuildAxes(
            scene,
            TimeZoneInfo.Utc,
            CultureInfo.InvariantCulture);

        Assert.True(projection.ModelDisplayMinimum < 0);
        Assert.True(projection.ModelDisplayMaximum > scene.ModelMaximum);
        Assert.Equal(0.01, (0 - projection.ModelDisplayMinimum) /
            (projection.ModelDisplayMaximum - projection.ModelDisplayMinimum), precision: 12);
        Assert.Equal(0.99, (scene.ModelMaximum - projection.ModelDisplayMinimum) /
            (projection.ModelDisplayMaximum - projection.ModelDisplayMinimum), precision: 12);
        Assert.True(projection.EndpointLabelAt > scene.PeriodEndAt);
        Assert.True(projection.DisplayEndAt > projection.EndpointLabelAt);
        Assert.Equal(scene.PeriodEndAt, projection.BottomValues[^1]);
    }

    [Fact]
    public void PlotProjectionKeepsMetricSpecificEndpointGutterFixedAcrossUnboundedWidths()
    {
        const double referenceWidth = 800;
        double[] currentWidths = [320, 800, 1_200, 10_000];
        var points = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(2_000, 75, 2, 4, 6),
        };

        foreach (var (metric, legacyGutterRatio) in new[]
        {
            (GraphMetric.Dollars, 0.20),
            (GraphMetric.Tokens, 0.27),
        })
        {
            var scene = GraphScene.Create(points, metric, points[0].Timestamp, points[^1].Timestamp);
            var expectedGutter = referenceWidth * legacyGutterRatio / (1 + legacyGutterRatio);
            var expectedLabelGap = referenceWidth * 0.018 / (1 + legacyGutterRatio);

            foreach (var currentWidth in currentWidths)
            {
                var projection = GraphPlotProjection.BuildAxes(
                    scene,
                    TimeZoneInfo.Utc,
                    CultureInfo.InvariantCulture,
                    currentWidth,
                    referenceWidth);
                var displaySpan = projection.DisplayEndAt - scene.PeriodStartAt;
                var plotWidth = currentWidth *
                    (scene.PeriodEndAt - scene.PeriodStartAt) / displaySpan;
                var labelGap = currentWidth *
                    (projection.EndpointLabelAt - scene.PeriodEndAt) / displaySpan;

                Assert.Equal(expectedGutter, currentWidth - plotWidth, precision: 9);
                Assert.Equal(expectedLabelGap, labelGap, precision: 9);
            }
        }
    }

    [Fact]
    public void PlotProjectionFallbackWidthRetainsReferenceEndpointCoordinates()
    {
        foreach (var metric in new[] { GraphMetric.Dollars, GraphMetric.Tokens })
        {
            var points = new[]
            {
                Point(1_000, 100, 0, 0, 0),
                Point(2_000, 75, 2, 4, 6),
            };
            var scene = GraphScene.Create(points, metric, points[0].Timestamp, points[^1].Timestamp);

            var legacy = GraphPlotProjection.BuildAxes(
                scene,
                TimeZoneInfo.Utc,
                CultureInfo.InvariantCulture);
            var sameWidth = GraphPlotProjection.BuildAxes(
                scene,
                TimeZoneInfo.Utc,
                CultureInfo.InvariantCulture,
                800,
                800);

            Assert.Equal(legacy.DisplayEndAt, sameWidth.DisplayEndAt);
            Assert.Equal(legacy.EndpointLabelAt, sameWidth.EndpointLabelAt);
        }
    }

    [Fact]
    public void PlotProjectionSeparatesFlatAndRisingSegmentsLikeTheNativeGraph()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 99, 0, 0, 1),
                Point(1_120, 99, 0, 0, 1),
                Point(1_180, 98, 0, 0, 2),
            ]);

        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Luna);

        Assert.Equal([1_060d, 1_120d], lines.Flat.X);
        Assert.Equal([1d, 1d], lines.Flat.Y);
        Assert.Equal([1_000d, 1_060d, double.NaN, 1_120d, 1_180d], lines.Rising.X);
        Assert.Equal([0d, 1d, double.NaN, 1d, 2d], lines.Rising.Y);
    }

    [Fact]
    public void V3AstraHistoryRendersWithoutLegacyModelRows()
    {
        var samples = new[]
        {
            new ApiHistorySample(
                1_000, 2_000, 90, null, null, null, null, null, null,
                ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
                ModelSamples =
                [
                    new ApiHistoryModelSample("ASTRA", 1, 0, 0, 10) { TotalTokens = 1 },
                    new ApiHistoryModelSample("LUNA", null, null, null, 1) { TotalTokens = 1 },
                ],
            },
            new ApiHistorySample(
                1_060, 2_000, 89, null, null, null, null, null, null,
                ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
                ModelSamples =
                [
                    new ApiHistoryModelSample("ASTRA", 2, 0, 0, 20) { TotalTokens = 2 },
                    new ApiHistoryModelSample("LUNA", null, null, null, 2) { TotalTokens = 2 },
                    new ApiHistoryModelSample("SOL", null, null, null, 3) { TotalTokens = 3 },
                ],
            },
            new ApiHistorySample(
                1_120, 2_000, 88, null, null, null, null, null, null,
                ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
                ModelSamples =
                [
                    new ApiHistoryModelSample("ASTRA", 3, 0, 0, 30) { TotalTokens = 3 },
                    new ApiHistoryModelSample("LUNA", null, null, null, 2) { TotalTokens = 2 },
                    new ApiHistoryModelSample("SOL", null, null, null, 4) { TotalTokens = 4 },
                ],
            },
        };

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_120);
        var astra = GraphPlotProjection.BuildModelLines(scene, scene.Astra);
        var luna = GraphPlotProjection.BuildModelLines(scene, scene.Luna);
        var sol = GraphPlotProjection.BuildModelLines(scene, scene.Sol);

        Assert.Equal([10d, 20d, 30d], scene.Astra);
        Assert.Equal([10d, 20d, 30d], scene.ModelSeries["ASTRA"]);
        Assert.Equal([1d, 2d, 2d], scene.Luna);
        Assert.True(double.IsNaN(scene.Sol[0]));
        Assert.Equal([3d, 4d], scene.Sol.Skip(1));
        Assert.Equal([1_000d, 1_060d, 1_120d], astra.Rising.X);
        Assert.Equal([10d, 20d, 30d], astra.Rising.Y);
        Assert.Empty(astra.Dashed.X);
        Assert.Equal([1_000d, 1_060d], luna.Rising.X);
        Assert.Equal([1_060d, 1_120d], luna.Flat.X);
        Assert.Empty(luna.Dashed.X);
        Assert.Equal([1_060d, 1_120d], sol.Rising.X);
        Assert.Empty(sol.Dashed.X);
        Assert.Empty(scene.IdleIntervals);
    }

    [Fact]
    public void Scene_hides_only_the_regressed_model_component_until_recovery()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 1, 2, 3),
                Point(1_060, 90, 2, 1, 4),
                Point(1_120, 80, 2, 2, 4),
                Point(1_180, 70, 75, 3, 5),
            ]);

        Assert.Equal([true, false, true, true], scene.ModelVectorAvailable);
        Assert.Equal([1d, 2d, 2d, 75d], scene.Sol);
        Assert.Equal([2d, double.NaN, 2d, 3d], scene.Terra);
        Assert.Equal([3d, 4d, 4d, 5d], scene.Luna);

        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        Assert.Equal([1_000d, 1_060d, double.NaN, 1_120d, 1_180d], lines.Rising.X);
        Assert.Equal([1d, 2d, double.NaN, 2d, 75d], lines.Rising.Y);
        Assert.Equal([1_060d, 1_120d], lines.Flat.X);
        Assert.Empty(lines.Dashed.X);

        var quotaLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Equal([1_120d, 1_180d], quotaLines.Solid.X);
        Assert.Equal([80d, 70d], quotaLines.Solid.Y);
        Assert.Equal([1_000d, 1_060d, 1_120d], quotaLines.Dashed.X);
        Assert.Equal([100d, 90d, 80d], quotaLines.Dashed.Y);
    }

    [Fact]
    public void Live_incident_regression_recovery_is_never_connected_as_solid()
    {
        var scene = GraphScene.Create(
            [
                Point(0, 85, 176.04, 0, 7.00),
                Point(60, 84, 87.48, 0, 5.00),
                Point(120, 83, 88.00, 0, 5.10),
                Point(180, 82, 89.00, 0, 5.20),
                Point(240, 81, 184.14, 0, 7.34),
            ],
            GraphMetric.Dollars,
            0,
            300);

        Assert.Equal([true, false, false, false, true], scene.ModelVectorAvailable);
        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Empty(model.Flat.X);
        Assert.Empty(model.Rising.X);
        Assert.Equal([0d, 240d], model.Dashed.X);
        Assert.Equal([176.04d, 184.14d], model.Dashed.Y);
        Assert.Empty(remaining.Solid.X);
        Assert.Equal([0d, 60d, 120d, 180d, 240d, 300d], remaining.Dashed.X);
        Assert.Equal([85d, 84d, 83d, 82d, 81d, 81d], remaining.Dashed.Y);
        Assert.True(remaining.Dashed.X
            .Zip(remaining.Dashed.X.Skip(1))
            .All(pair => pair.First < pair.Second));
    }

    [Fact]
    public void Model_lines_dash_only_increasing_unobserved_gaps()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 90, 1, 0, 0),
                Point(1_121, 80, 2, 0, 0),
                Point(1_242, 70, 2, 0, 0),
            ]);

        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);

        Assert.Equal([1_000d, 1_060d], lines.Rising.X);
        Assert.Equal([0d, 1d], lines.Rising.Y);
        Assert.Equal([1_060d, 1_121d], lines.Dashed.X);
        Assert.Equal([1d, 2d], lines.Dashed.Y);
        Assert.Equal([1_121d, 1_242d], lines.Flat.X);
        Assert.Equal([2d, 2d], lines.Flat.Y);
    }

    [Fact]
    public void Remaining_quota_observations_survive_flat_model_rows_as_unattributed_dashes()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 90, 1, 0, 0),
                Point(1_120, 70, 1, 0, 0),
            ]);

        var lines = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Equal([100d, 90d, 70d], scene.ObservedRemainingValues);
        Assert.Equal([1_000d, 1_060d], lines.Solid.X);
        Assert.Equal([100d, 90d], lines.Solid.Y);
        Assert.Equal([1_060d, 1_120d], lines.Dashed.X);
        Assert.Equal([90d, 70d], lines.Dashed.Y);
    }

    [Fact]
    public void Missing_remote_quota_is_never_painted_as_a_solid_bridge()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, null, 1, 0, 0),
                Point(1_120, 80, 2, 0, 0),
            ]);

        var lines = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Empty(lines.Solid.X);
        Assert.Equal([1_000d, 1_060d, 1_120d], lines.Dashed.X);
        Assert.Equal([100d, 90d, 80d], lines.Dashed.Y);
    }

    [Fact]
    public void One_old_local_observation_is_not_extended_but_remote_quota_is_held_dashed()
    {
        var period = new ApiHistoryPeriod("current", 1_000, 2_000, true, "current")
        {
            Samples = [Point(1_000, 80, 5, 2, 1)],
        };
        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_600);
        var scene = GraphScene.Create(
            samples,
            GraphMetric.Dollars,
            1_000,
            1_600);

        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);
        var labels = GraphPlotProjection.BuildEndpointLabels(scene, CultureInfo.InvariantCulture);

        Assert.Single(samples);
        Assert.Equal(1_000, samples[0].Timestamp);
        Assert.Empty(model.Flat.X);
        Assert.Empty(model.Rising.X);
        Assert.Empty(model.Dashed.X);
        Assert.Empty(remaining.Solid.X);
        Assert.Equal([1_000d, 1_600d], remaining.Dashed.X);
        Assert.Equal([80d, 80d], remaining.Dashed.Y);
        Assert.DoesNotContain(labels, label => label.Series == GraphSeries.Sol);
        Assert.Contains(labels, label => label.Series == GraphSeries.Remaining && label.Text == "80%");
    }

    [Fact]
    public void Confirmed_history_gap_ends_both_subpaths_without_a_cross_gap_connector()
    {
        var scene = GraphScene.Create(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 90, 1, 0, 0),
                Point(1_120, 80, 2, 0, 0),
                Point(1_180, 70, 3, 0, 0),
            ],
            GraphMetric.Dollars,
            1_000,
            1_180,
            [new GraphConfirmedGap(1_060, 1_120)]);

        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        var gap = Assert.Single(scene.ConfirmedGaps);
        Assert.Equal(1_060L, gap.StartAt);
        Assert.Equal(1_120L, gap.EndAt);
        Assert.Equal([1_000d, 1_060d, double.NaN, 1_120d, 1_180d], model.Rising.X);
        Assert.Empty(model.Dashed.X);
        Assert.Equal([1_000d, 1_060d, double.NaN, 1_120d, 1_180d], remaining.Solid.X);
        Assert.Empty(remaining.Dashed.X);
        Assert.Empty(scene.IdleIntervals);
    }

    [Fact]
    public void PlotProjectionDoesNotInventSpendDuringAnUnobservedGap()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 99, 0, 0, 1),
                Point(3_600, 98, 0, 0, 1),
                Point(3_660, 97, 0, 0, 2),
            ]);

        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Luna);

        Assert.Equal([1_060d, 3_600d], lines.Flat.X);
        Assert.Equal([1d, 1d], lines.Flat.Y);
        Assert.Equal([1_000d, 1_060d, double.NaN, 3_600d, 3_660d], lines.Rising.X);
        Assert.Equal([0d, 1d, double.NaN, 1d, 2d], lines.Rising.Y);
        Assert.Empty(lines.Dashed.X);
        Assert.Empty(lines.Dashed.Y);
    }

    [Fact]
    public void PlotProjectionDashesTheLongFirstIntervalAndKeepsLaterEvidenceSolid()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_120, 90, 1, 2, 3),
                Point(1_180, 80, 2, 3, 4),
            ]);

        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Empty(model.Flat.X);
        Assert.Equal([1_120d, 1_180d], model.Rising.X);
        Assert.Equal([1d, 2d], model.Rising.Y);
        Assert.Equal([1_000d, 1_120d], model.Dashed.X);
        Assert.Equal([0d, 1d], model.Dashed.Y);
        Assert.Equal([1_120d, 1_180d], remaining.Solid.X);
        Assert.Equal([90d, 80d], remaining.Solid.Y);
        Assert.Equal([1_000d, 1_120d], remaining.Dashed.X);
        Assert.Equal([100d, 90d], remaining.Dashed.Y);
        Assert.Empty(scene.IdleIntervals);
        Assert.DoesNotContain(model.Rising.X, timestamp => timestamp < 1_120d);
    }

    [Fact]
    public void PlotProjectionKeepsEachModelAndMetricIndependentAtTheFirstObservation()
    {
        var points = new[]
        {
            new ApiHistorySample(1_000, 2_000, 100, 0, 0, 0, 0, 0, 0),
            new ApiHistorySample(1_120, 2_000, 90, 1, 2, 3, 10, 20, 30),
            new ApiHistorySample(1_180, 2_000, 80, 2, 3, 4, 20, 30, 40),
        };
        var dollars = GraphScene.Create(points, GraphMetric.Dollars, 1_000, 1_180);
        var tokens = GraphScene.Create(points, GraphMetric.Tokens, 1_000, 1_180);

        Assert.Equal([0d, 1d, 2d], dollars.Sol);
        Assert.Equal([0d, 2d, 3d], dollars.Terra);
        Assert.Equal([0d, 3d, 4d], dollars.Luna);
        Assert.Equal([0d, 10d, 20d], tokens.Sol);
        Assert.Equal([0d, 20d, 30d], tokens.Terra);
        Assert.Equal([0d, 30d, 40d], tokens.Luna);
        Assert.Equal([100d, 90d, 80d], dollars.Remaining);
        Assert.Equal(dollars.Remaining, tokens.Remaining);

        AssertFirstObservationModel(GraphPlotProjection.BuildModelLines(dollars, dollars.Sol), 1, 2);
        AssertFirstObservationModel(GraphPlotProjection.BuildModelLines(dollars, dollars.Terra), 2, 3);
        AssertFirstObservationModel(GraphPlotProjection.BuildModelLines(dollars, dollars.Luna), 3, 4);
        AssertFirstObservationModel(GraphPlotProjection.BuildModelLines(tokens, tokens.Sol), 10, 20);
        AssertFirstObservationModel(GraphPlotProjection.BuildModelLines(tokens, tokens.Terra), 20, 30);
        AssertFirstObservationModel(GraphPlotProjection.BuildModelLines(tokens, tokens.Luna), 30, 40);
    }

    [Fact]
    public void PlotProjectionKeepsOnlyMeasuredIdleBands()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 100, 0, 0, 0),
                Point(2_000, 90, 1, 0, 0),
                Point(7_000, 90, 1, 0, 0),
            ],
            1_000,
            87_400);

        var visible = GraphPlotProjection.BuildVisibleIdleIntervals(scene);

        var idle = Assert.Single(visible);
        Assert.Equal((2_000L, 7_000L, false), (idle.StartAt, idle.EndAt, idle.PreserveBoundary));

        var boundaryScene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_120, 90, 1, 0, 0),
            ],
            1_000,
            87_400);
        Assert.Empty(GraphPlotProjection.BuildVisibleIdleIntervals(boundaryScene));
    }

    [Fact]
    public void IdleBandsUseTheDedicatedVisibleNeutralColor()
    {
        Assert.Equal("#3F5D7C", GraphPlotControl.IdleBandColorHex);
        Assert.Equal(0.22, GraphPlotControl.IdleBandOpacity);
    }

    [Fact]
    public void InferredLinesAreThinnerThanMeasuredModelLines()
    {
        Assert.Equal(3f, GraphPlotControl.MeasuredModelLineWidth);
        Assert.Equal(1f, GraphPlotControl.MeasuredFlatModelLineWidth);
        Assert.Equal(3f, GraphPlotControl.MeasuredRemainingLineWidth);
        Assert.Equal(1f, GraphPlotControl.InferredLineWidth);
        Assert.True(GraphPlotControl.InferredLineWidth < GraphPlotControl.MeasuredModelLineWidth);
        Assert.True(GraphPlotControl.InferredLineWidth < GraphPlotControl.MeasuredRemainingLineWidth);
        Assert.Equal(ScottPlot.LinePattern.DenselyDashed, GraphPlotControl.InferredLinePattern);
    }

    [Fact]
    public void GenericModelActivityIsNeverLabelledIdle()
    {
        var samples = new[]
        {
            Point(1_000, 100, 0, 0, 0) with
            {
                ModelSamples = [new ApiHistoryModelSample("future-model", null, null, null, 1)],
            },
            Point(1_060, 99, 0, 0, 0) with
            {
                ModelSamples = [new ApiHistoryModelSample("future-model", null, null, null, 2)],
            },
        };

        Assert.Empty(GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_060).IdleIntervals);
    }

    [Fact]
    public void PlotProjectionPreservesAxisValueFormattingBoundaries()
    {
        var culture = CultureInfo.InvariantCulture;

        Assert.Equal("$12.30", GraphPlotProjection.FormatAxisValue(12.3, GraphMetric.Dollars, culture));
        Assert.Equal("999", GraphPlotProjection.FormatAxisValue(999, GraphMetric.Tokens, culture));
        Assert.Equal("1.0K", GraphPlotProjection.FormatAxisValue(1_000, GraphMetric.Tokens, culture));
        Assert.Equal("1.0M", GraphPlotProjection.FormatAxisValue(1_000_000, GraphMetric.Tokens, culture));
        Assert.Equal("1.0B", GraphPlotProjection.FormatAxisValue(1_000_000_000, GraphMetric.Tokens, culture));
    }

    [Fact]
    public void PlotProjectionOrdersEndpointCandidatesAndReturnsAxisValues()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 1, 2, 3),
                Point(1_100, 75, 2, 4, 6),
            ]);

        var labels = GraphPlotProjection.BuildEndpointLabels(scene, CultureInfo.InvariantCulture);

        Assert.Equal(
            [GraphSeries.Luna, GraphSeries.Remaining, GraphSeries.Terra, GraphSeries.Sol],
            labels.Select(label => label.Series));
        Assert.Equal(["LUNA $6.00", "75%", "TERRA $4.00", "SOL $2.00"], labels.Select(label => label.Text));
        Assert.Equal(0d, labels[0].NormalizedTop);
        Assert.Equal(0.25d, labels[1].NormalizedTop);
        Assert.Equal(1d - 4d / 6d, labels[2].NormalizedTop, precision: 12);
        Assert.Equal(1d - 2d / 6d, labels[3].NormalizedTop, precision: 12);
        Assert.Equal(5.85d, labels[0].AxisValue, precision: 12);
        Assert.Equal(75d, labels[1].AxisValue, precision: 12);
        Assert.Equal(4d, labels[2].AxisValue, precision: 12);
        Assert.Equal(2d, labels[3].AxisValue, precision: 12);
        Assert.True(labels.Zip(labels.Skip(1)).All(pair => pair.Second.ArrangedTop - pair.First.ArrangedTop >= 0.062));
    }

    [Fact]
    public void PlotProjectionHandlesEmptyScenesAndRejectsNullInputs()
    {
        Assert.Empty(GraphPlotProjection.BuildEndpointLabels(GraphScene.Empty(), CultureInfo.InvariantCulture));
        Assert.Throws<ArgumentNullException>(() => GraphPlotProjection.BuildAxes(null!, TimeZoneInfo.Utc, CultureInfo.InvariantCulture));
        Assert.Throws<ArgumentNullException>(() => GraphPlotProjection.BuildEndpointLabels(GraphScene.Empty(), null!));
        Assert.Throws<ArgumentNullException>(() => GraphPlotProjection.FormatAxisValue(1, GraphMetric.Dollars, null!));
    }

    [Fact]
    public void Remaining_with_no_graph_points_stays_empty()
    {
        Assert.Empty(GraphScene.Empty().Remaining);
    }

    [Fact]
    public void Graph_samples_preserve_each_details_vector_without_component_wise_max()
    {
        var period = new ApiHistoryPeriod("2000", 1_020, 1_200, false, "history")
        {
            Samples =
            [
                new ApiHistorySample(1_080, 2_000, 90, 2, 1, 0, 20, 10, 0),
                new ApiHistorySample(1_140, 2_000, 80, 3, 3, 1, 30, 30, 10),
                new ApiHistorySample(1_200, 2_000, 70, 4, 2, 2, 40, 20, 20),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_200);

        Assert.Equal([1_080L, 1_140L, 1_200L], samples.Select(sample => sample.Timestamp));
        Assert.Equal([2d, 3d, 4d], samples.Select(sample => sample.SolDollars));
        Assert.Equal([1d, 3d, 2d], samples.Select(sample => sample.TerraDollars));
        Assert.Equal([0d, 1d, 2d], samples.Select(sample => sample.LunaDollars));
        Assert.Equal([20UL, 30UL, 40UL], samples.Select(sample => sample.SolTokens));
        Assert.Equal([10UL, 30UL, 20UL], samples.Select(sample => sample.TerraTokens));
        Assert.Equal([0UL, 10UL, 20UL], samples.Select(sample => sample.LunaTokens));
        Assert.Equal([90d, 80d, 70d], samples.Select(sample => sample.RemainingPercent!.Value));
    }

    [Fact]
    public void Graph_samples_preserve_a_missing_first_quota_observation()
    {
        var period = new ApiHistoryPeriod("2000", 1_020, 1_200, false, "history")
        {
            Samples =
            [
                new ApiHistorySample(1_020, 2_000, null, 1, 0, 0, 10, 0, 0),
                new ApiHistorySample(1_080, 2_000, 90, 2, 0, 0, 20, 0, 0),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_200);

        Assert.Equal([1_020L, 1_080L], samples.Select(sample => sample.Timestamp));
        Assert.Null(samples[0].RemainingPercent);
        Assert.Equal(1, samples[0].SolDollars);
        Assert.Equal(10UL, samples[0].SolTokens);
        Assert.Equal(90, samples[1].RemainingPercent);
        var scene = Scene(samples, period.StartAt, period.EndAt);
        Assert.True(double.IsNaN(scene.Remaining[0]));
        Assert.Equal(90, scene.Remaining[1]);
        var lines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Empty(lines.Solid.X);
        Assert.Equal([1_080d, 1_200d], lines.Dashed.X);
        Assert.Equal([90d, 90d], lines.Dashed.Y);
        Assert.DoesNotContain(lines.Dashed.X, timestamp => timestamp < 1_080d);
    }

    [Fact]
    public void Graph_samples_extend_only_recent_available_models_and_exclude_the_next_row()
    {
        var period = new ApiHistoryPeriod("current", 1_020, 1_200, true, "current")
        {
            Samples =
            [
                new ApiHistorySample(1_020, 2_000, 100, 0, 0, 0, 0, 0, 0),
                new ApiHistorySample(1_080, 2_000, 90, 7, 0, 0, 70, 0, 0),
                // Adapter-only invalid injection: not minute-start and not
                // admitted by Core; this row tests defensive end filtering.
                new ApiHistorySample(1_141, 2_000, 89, 99, 0, 0, 990, 0, 0),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_140);

        Assert.Equal([1_020L, 1_080L, 1_140L], samples.Select(sample => sample.Timestamp));
        Assert.Equal(7, samples[^1].SolDollars);
        Assert.Equal(70UL, samples[^1].SolTokens);
        Assert.Null(samples[^1].RemainingPercent);
        Assert.DoesNotContain(samples, sample => sample.SolDollars == 99);
        Assert.DoesNotContain(samples, sample => sample.SolTokens == 990);

        var unavailable = new ApiHistoryPeriod("unavailable", 1_020, 1_200, true, "current")
        {
            Samples =
            [
                new ApiHistorySample(
                    1_080,
                    2_000,
                    90,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    ApiHistorySample.UnavailableModelSource),
            ],
        };
        var unavailableSamples = GraphWindowViewModel.BuildGraphSamples(unavailable, 1_140);
        Assert.Equal(
            [1_080L],
            unavailableSamples.Select(sample => sample.Timestamp));
        var unavailableScene = GraphScene.Create(
            unavailableSamples,
            GraphMetric.Dollars,
            1_020,
            1_140);
        var unavailableModel = GraphPlotProjection.BuildModelLines(
            unavailableScene,
            unavailableScene.Sol);
        var unavailableRemaining = GraphPlotProjection.BuildRemainingLines(unavailableScene);
        Assert.Empty(unavailableModel.Flat.X);
        Assert.Empty(unavailableModel.Rising.X);
        Assert.Empty(unavailableModel.Dashed.X);
        Assert.Equal([1_080d, 1_140d], unavailableRemaining.Dashed.X);
        Assert.Equal([90d, 90d], unavailableRemaining.Dashed.Y);
    }

    [Fact]
    public void Idle_intervals_merge_flat_segments_but_keep_a_long_unobserved_boundary()
    {
        var points = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_060, 100, 0, 0, 0),
            Point(1_120, 90, 1, 0, 0),
            Point(1_180, 90, 1, 0, 0),
            Point(1_300, 90, 1, 0, 0),
        };

        var intervals = Scene(points, 1_000, 1_300).IdleIntervals;

        Assert.Equal(2, intervals.Count);
        Assert.Equal((1_000L, 1_060L, false), (intervals[0].StartAt, intervals[0].EndAt, intervals[0].PreserveBoundary));
        Assert.Equal((1_120L, 1_300L, false), (intervals[1].StartAt, intervals[1].EndAt, intervals[1].PreserveBoundary));

        var sparse = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_120, 90, 1, 0, 0),
            Point(1_300, 90, 1, 0, 0),
        };
        var sparseIntervals = Scene(sparse, 1_000, 1_300).IdleIntervals;

        var sparseIdle = Assert.Single(sparseIntervals);
        Assert.False(sparseIdle.PreserveBoundary);
        Assert.Equal((1_120L, 1_300L), (sparseIdle.StartAt, sparseIdle.EndAt));
    }

    [Fact]
    public void Remaining_accepts_a_delayed_lower_quota_after_unobserved_sol_usage()
    {
        var points = new[]
        {
            Point(1_000, 87, 0, 0, 0),
            Point(1_060, null, 140, 0, 0),
            Point(1_120, null, 420, 0, 0),
            Point(1_240, 1, 420, 0, 0),
        };

        var effective = Scene(points).Remaining;

        Assert.Equal(87d, effective[0]);
        Assert.True(effective[1] < 87d);
        Assert.Equal(1d, effective[2]);
        Assert.Equal(1d, effective[3]);
    }

    [Fact]
    public async Task Shared_graph_fixture_matches_the_native_history_oracle_through_details_http_parser()
    {
        var fixturePath = Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_delayed_quota.json");
        using var document = JsonDocument.Parse(File.ReadAllText(fixturePath));
        var root = document.RootElement;
        var detailsResponse = root.GetProperty("details_response");
        var detailsJson = detailsResponse.GetRawText();
        var responseBytes = Encoding.UTF8.GetBytes(detailsJson);
        var handler = new DetailsFixtureHandler(responseBytes);

        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(3, handler.RequestCount);
        Assert.Equal(HttpStatusCode.OK, handler.StatusCode);
        Assert.Equal(HttpMethod.Get, handler.LastRequest?.Method);
        Assert.Equal(
            "http://127.0.0.1:8787/v1/details",
            handler.LastRequest?.RequestUri?.AbsoluteUri);
        Assert.Equal(responseBytes, handler.ReturnedBytes);
        Assert.Equal(responseBytes.LongLength, handler.ContentLength);
        Assert.Equal("application/json; charset=utf-8", handler.ContentType);
        Assert.True(handler.NoStore);
        Assert.Equal([CanonicalPublishedPair], handler.PublishedPairValues);

        Assert.True(result.IsSuccess);
        Assert.Null(result.Failure);
        var snapshot = Assert.IsType<ApiDetailsSnapshot>(result.Snapshot);
        Assert.Equal(CanonicalPublishedPair, snapshot.PublishedPair?.ToString());

        var expectedPeriodStart = root.GetProperty("expected_period_start").GetInt64();
        var expectedPeriodEnd = root.GetProperty("expected_period_end").GetInt64();
        var expectedResetAt = root.GetProperty("expected_reset_at").GetInt64();
        var expectedRawTimestamps = root.GetProperty("expected_raw_timestamps")
            .EnumerateArray()
            .Select(value => value.GetInt64())
            .ToArray();
        var expectedRemaining = root.GetProperty("expected_remaining")
            .EnumerateArray()
            .Select(value => value.GetDouble())
            .ToArray();
        var expectedSolMax = root.GetProperty("expected_sol_max").GetDouble();
        var expectedPeriodCount = root.GetProperty("expected_period_count").GetInt32();

        Assert.Equal(expectedPeriodCount, snapshot.HistoryPeriods.Count);
        var period = Assert.Single(snapshot.HistoryPeriods);
        Assert.True(period.Current);
        Assert.Equal(expectedPeriodStart, period.StartAt);
        Assert.Equal(expectedPeriodEnd, period.EndAt);
        Assert.Equal(expectedResetAt, period.ResetAt);
        Assert.Equal(expectedPeriodEnd, snapshot.ObservedAt);

        var expectedRows = new[]
        {
            (Timestamp: 1_999_999_980L, Remaining: (double?)87d, SolDollars: 0d, TerraDollars: 30.50d, LunaDollars: 0d, SolTokens: 0UL, TerraTokens: 30UL, LunaTokens: 0UL),
            (Timestamp: 2_000_000_040L, Remaining: (double?)null, SolDollars: 140.97d, TerraDollars: 30.50d, LunaDollars: 0d, SolTokens: 141UL, TerraTokens: 30UL, LunaTokens: 0UL),
            (Timestamp: 2_000_000_100L, Remaining: (double?)null, SolDollars: 420.40d, TerraDollars: 30.50d, LunaDollars: 0d, SolTokens: 420UL, TerraTokens: 30UL, LunaTokens: 0UL),
            (Timestamp: 2_000_000_220L, Remaining: (double?)1d, SolDollars: 420.40d, TerraDollars: 30.50d, LunaDollars: 0d, SolTokens: 420UL, TerraTokens: 30UL, LunaTokens: 0UL),
            (Timestamp: 2_000_000_280L, Remaining: (double?)1d, SolDollars: 420.40d, TerraDollars: 30.50d, LunaDollars: 0d, SolTokens: 420UL, TerraTokens: 30UL, LunaTokens: 0UL),
        };

        var flatSamples = snapshot.HistorySamples;
        var ownedSamples = period.Samples;
        Assert.Equal(expectedRows.Length, flatSamples.Count);
        Assert.Equal(expectedRows.Length, ownedSamples.Count);
        Assert.All(flatSamples, sample =>
            Assert.Equal(ApiHistorySample.LegacyUnknownModelSource, sample.ModelSource));
        Assert.Equal(expectedRawTimestamps, flatSamples.Select(sample => sample.Timestamp));
        Assert.Equal(expectedRawTimestamps, ownedSamples.Select(sample => sample.Timestamp));
        for (var index = 0; index < expectedRows.Length; index++)
        {
            Assert.Equal(flatSamples[index], ownedSamples[index]);
            Assert.Equal(expectedResetAt, flatSamples[index].ResetAt);
            AssertHistorySample(flatSamples[index], expectedRows[index]);
        }
        Assert.Equal(expectedPeriodEnd, flatSamples[^1].Timestamp);
        Assert.Equal(expectedPeriodEnd, ownedSamples[^1].Timestamp);

        var graphSamples = GraphWindowViewModel.BuildGraphSamples(period, expectedPeriodEnd);
        var scene = GraphScene.Create(
            graphSamples,
            GraphMetric.Dollars,
            expectedPeriodStart,
            expectedPeriodEnd);
        var remainingLines = GraphPlotProjection.BuildRemainingLines(scene);
        var solLines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var terraLines = GraphPlotProjection.BuildModelLines(scene, scene.Terra);
        var lunaLines = GraphPlotProjection.BuildModelLines(scene, scene.Luna);

        Assert.Equal(expectedRawTimestamps.Select(timestamp => (double)timestamp), scene.Timestamps);
        Assert.Equal(expectedRawTimestamps, graphSamples.Select(sample => sample.Timestamp));
        Assert.Equal(expectedRemaining, scene.Remaining);
        Assert.Equal(expectedPeriodStart, scene.PeriodStartAt);
        Assert.Equal(expectedPeriodEnd, scene.PeriodEndAt);
        Assert.Equal(expectedSolMax, scene.ModelMaximum, precision: 6);
        Assert.Equal(expectedRows.Length, graphSamples.Count);
        Assert.Equal(expectedRawTimestamps, graphSamples.Select(sample => sample.Timestamp));
        Assert.Equal(flatSamples[^1], graphSamples[^1]);
        Assert.Equal(expectedPeriodEnd, graphSamples[^1].Timestamp);
        Assert.Equal(expectedPeriodEnd, scene.Timestamps[^1]);

        var firstObservation = expectedRawTimestamps[0];
        Assert.NotEmpty(terraLines.Flat.X);
        Assert.Empty(terraLines.Rising.X);
        Assert.Empty(terraLines.Dashed.X);
        Assert.DoesNotContain(expectedPeriodStart, terraLines.Flat.X);
        Assert.Contains(firstObservation, terraLines.Flat.X);
        Assert.NotEmpty(remainingLines.Dashed.X);
        Assert.Equal(firstObservation, remainingLines.Dashed.X[0]);
        Assert.Equal(87d, remainingLines.Dashed.Y[0]);
        Assert.DoesNotContain(remainingLines.Solid.X, timestamp => timestamp < firstObservation);
        Assert.DoesNotContain(remainingLines.Dashed.X, timestamp => timestamp < firstObservation);
        Assert.NotEmpty(solLines.Flat.X);
        Assert.NotEmpty(solLines.Rising.X);
        Assert.Empty(solLines.Dashed.X);
        Assert.NotEmpty(lunaLines.Flat.X);
        Assert.Empty(lunaLines.Rising.X);
        Assert.Empty(lunaLines.Dashed.X);
        Assert.Empty(remainingLines.Solid.X);
    }

    [Fact]
    public async Task Issue137_cumulative_correction_fixture_never_paints_a_solid_recovery_bridge()
    {
        var fixturePath = Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_cumulative_correction.json");
        using var document = JsonDocument.Parse(File.ReadAllText(fixturePath));
        var root = document.RootElement;
        var expectedPeriodStart = root.GetProperty("expected_period_start").GetInt64();
        var expectedPeriodEnd = root.GetProperty("expected_period_end").GetInt64();
        var expectedResetAt = root.GetProperty("expected_reset_at").GetInt64();
        var expectedGraphTimestamps = root
            .GetProperty("expected_graph_timestamps")
            .EnumerateArray()
            .Select(value => value.GetInt64())
            .ToArray();
        var expectedCorrectionStarts = root
            .GetProperty("expected_correction_starts")
            .EnumerateArray()
            .Select(value => value.GetInt64())
            .ToArray();
        var expectedLatestSampleTimestamp = root.GetProperty("expected_latest_sample_timestamp").GetInt64();
        var expectedLatestRemaining = root.GetProperty("expected_latest_remaining").GetDouble();
        var expectedLatestSolDollars = root.GetProperty("expected_latest_sol_dollars").GetDouble();
        var expectedLatestSolTokens = root.GetProperty("expected_latest_sol_tokens").GetUInt64();

        var handler = new DetailsFixtureHandler(
            Encoding.UTF8.GetBytes(root.GetProperty("details_response").GetRawText()));
        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(2, handler.RequestCount);
        Assert.Equal("http://127.0.0.1:8787/v2/details", handler.LastRequest?.RequestUri?.AbsoluteUri);
        var snapshot = Assert.IsType<ApiDetailsSnapshot>(result.Snapshot);
        Assert.Equal("v2", snapshot.ApiVersion);
        Assert.Equal(6, snapshot.HistorySamples.Count);
        Assert.All(
            snapshot.HistorySamples,
            sample => Assert.Equal(ApiHistorySample.ConfirmedModelSource, sample.ModelSource));

        var period = Assert.Single(snapshot.HistoryPeriods);
        Assert.True(period.Current);
        Assert.Equal(expectedPeriodStart, period.StartAt);
        Assert.Equal(expectedPeriodEnd, period.EndAt);
        Assert.Equal(expectedResetAt, period.ResetAt);
        Assert.Equal(expectedPeriodEnd, snapshot.ObservedAt);

        var graphSamples = GraphWindowViewModel.BuildGraphSamples(period, expectedPeriodEnd);
        Assert.Equal(
            expectedGraphTimestamps,
            graphSamples.Select(sample => sample.Timestamp).ToArray());
        Assert.Equal(
            expectedCorrectionStarts,
            graphSamples
                .Where(sample => expectedCorrectionStarts.Contains(sample.Timestamp))
                .Select(sample => sample.Timestamp)
                .ToArray());
        Assert.Equal(expectedLatestSampleTimestamp, graphSamples[^2].Timestamp);
        Assert.NotNull(graphSamples[^2].RemainingPercent);
        Assert.NotNull(graphSamples[^2].SolDollars);
        Assert.NotNull(graphSamples[^2].SolTokens);
        Assert.Equal(expectedLatestRemaining, graphSamples[^2].RemainingPercent!.Value);
        Assert.Equal(expectedLatestSolDollars, graphSamples[^2].SolDollars!.Value, precision: 6);
        Assert.Equal(expectedLatestSolTokens, graphSamples[^2].SolTokens!.Value);
        // A recent endpoint hold repeats only the local model vector. It must
        // not manufacture quota or recover the pre-correction cumulative value.
        Assert.InRange(expectedPeriodEnd - expectedLatestSampleTimestamp, 1, 60);
        Assert.Null(graphSamples[^1].RemainingPercent);
        Assert.NotNull(graphSamples[^1].SolDollars);
        Assert.Equal(expectedLatestSolDollars, graphSamples[^1].SolDollars!.Value, precision: 6);

        using var main = new MainWindowViewModel(
            new CountingReadyHealthClient(),
            new SingleDetailsClient(result),
            new AlwaysReadyConnectionSupervisor());
        main.Start();
        await EventuallyAsync(() => ReferenceEquals(main.DetailsSnapshot, snapshot));

        using var graph = new GraphWindowViewModel(main, static action => action());
        var scene = graph.Scene;
        Assert.Equal([true, true, false, false, false, false, false], scene.ModelVectorAvailable);
        Assert.Equal([73d, 73d, 73d, 70d, 70d, 62d, 62d], scene.Remaining);
        Assert.All(scene.Sol.Skip(2), value => Assert.True(double.IsNaN(value)));

        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Empty(model.Rising.X);
        Assert.NotEmpty(remaining.Dashed.X);
        Assert.DoesNotContain(
            remaining.Solid.Y.Zip(remaining.Solid.Y.Skip(1)),
            segment => segment.Second < segment.First);
    }

    [Fact]
    public async Task Shared_rollover_fixture_atomically_refreshes_open_main_graph_and_threads_from_details()
    {
        var fixturePath = Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_weekly_reset_rollover.json");
        using var document = JsonDocument.Parse(File.ReadAllText(fixturePath));
        var root = document.RootElement;
        Assert.Equal("/v1/details", root.GetProperty("endpoint").GetString());
        var generations = root.GetProperty("generations").EnumerateArray().ToArray();
        Assert.Equal(["A", "B"], generations.Select(generation => generation.GetProperty("name").GetString()));

        var first = await ParseDetailsFixtureAsync(generations[0].GetProperty("details_response"));
        var second = await ParseDetailsFixtureAsync(generations[1].GetProperty("details_response"));
        var firstCurrent = Assert.Single(first.HistoryPeriods, period => period.Current);
        var secondCurrent = Assert.Single(second.HistoryPeriods, period => period.Current);
        var expectedResetAt = root.GetProperty("expected_reset_at").GetInt64();
        Assert.Equal(expectedResetAt.ToString(CultureInfo.InvariantCulture), firstCurrent.Id);
        Assert.Equal(firstCurrent.Id, secondCurrent.Id);
        Assert.Equal(expectedResetAt, firstCurrent.ResetAt);
        Assert.Equal(expectedResetAt, secondCurrent.ResetAt);

        var health = new CountingReadyHealthClient();
        var details = new SequenceFixtureDetailsClient(first, second);
        using var supervisor = new AlwaysReadyConnectionSupervisor();
        using var main = new MainWindowViewModel(health, details, supervisor);
        main.Start();
        await EventuallyAsync(() => ReferenceEquals(main.DetailsSnapshot, first));
        using var graph = new GraphWindowViewModel(main, static action => action());
        using var threads = new ThreadsWindowViewModel(main);

        Assert.Equal(100, main.RemainingPercentValue);
        Assert.Equal("概算 $1", main.EstimatedCostText);
        Assert.Same(firstCurrent, graph.SelectedPeriod);
        Assert.Equal(firstCurrent.EndAt, graph.Scene.PeriodEndAt);
        Assert.Equal(100, graph.Points[^1].RemainingPercent);
        Assert.Equal("thread-a", Assert.Single(threads.Threads).Id);
        var firstEndpoint = graph.Scene.PeriodEndAt;
        var firstMaximum = graph.Scene.ModelMaximum;

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => ReferenceEquals(main.DetailsSnapshot, second));

        Assert.Equal(41, main.RemainingPercentValue);
        Assert.Equal("概算 $323.674247", main.EstimatedCostText);
        Assert.Equal(323.674247, main.DetailsSnapshot!.Models.Sum(model => model.TotalDollars), precision: 6);
        Assert.Same(secondCurrent, graph.SelectedPeriod);
        Assert.True(graph.SelectedPeriod!.Current);
        Assert.Equal(firstCurrent.Id, graph.SelectedPeriod.Id);
        Assert.Equal(secondCurrent.EndAt, graph.Scene.PeriodEndAt);
        Assert.NotEqual(firstEndpoint, graph.Scene.PeriodEndAt);
        Assert.Equal(secondCurrent.EndAt, graph.Points[^1].Timestamp);
        Assert.Null(graph.Points[^1].RemainingPercent);
        Assert.Equal(41, graph.Scene.Remaining[^1]);
        Assert.Equal(323.674247, graph.Points[^1].SolValue, precision: 6);
        // The refresh row regresses Terra/LUNA while SOL advances. Preserve
        // the exact SOL observation and withhold only the regressed models.
        Assert.NotEqual(firstMaximum, graph.Scene.ModelMaximum);
        Assert.Equal(323.674247, graph.Scene.ModelMaximum, precision: 6);
        Assert.False(graph.Scene.ModelVectorAvailable[^1]);
        Assert.Equal("thread-b", Assert.Single(threads.Threads).Id);
        Assert.Equal(2, health.CallCount);
        Assert.Equal(2, details.CallCount);
    }

    [Fact]
    public void Remaining_long_active_plateau_is_distributed_across_every_interval()
    {
        var points = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_060, 90, 1, 0, 0),
            Point(1_120, 90, 2, 0, 0),
            Point(1_180, 90, 3, 0, 0),
            Point(1_240, 80, 4, 0, 0),
        };

        var effective = Scene(points).Remaining;
        var lines = GraphPlotProjection.BuildRemainingLines(Scene(points));

        Assert.Equal(100d, effective[0]);
        Assert.Equal(90d, effective[1]);
        Assert.Equal(86.66666666666667d, effective[2], precision: 12);
        Assert.Equal(83.33333333333333d, effective[3], precision: 12);
        Assert.Equal(80d, effective[4]);
        // The two estimated interior values are not observed quota readings.
        // Their incoming intervals must remain visibly inferred.
        Assert.Equal([1_000d, 1_060d, double.NaN, 1_180d, 1_240d], lines.Solid.X);
        Assert.Equal([1_060d, 1_120d, 1_180d], lines.Dashed.X);
        Assert.Equal([90d, effective[2], effective[3]], lines.Dashed.Y);
        Assert.DoesNotContain(
            lines.Solid.X.Zip(lines.Solid.X.Skip(1)),
            pair => pair.First == pair.Second);
    }

    [Fact]
    public void Remaining_preserves_unattributed_remote_changes_and_does_not_fabricate_terminal_consumption()
    {
        var points = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_060, 90, 1, 0, 0),
            Point(1_120, 90, 1, 0, 0),
            Point(1_180, 80, 2, 0, 0),
        };

        var effective = Scene(points).Remaining;

        Assert.Equal(90d, effective[2]);
        Assert.Equal(80d, effective[3]);

        var idleReread = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_060, 90, 1, 0, 0),
            // A lower quota reread while all model totals are unchanged is
            // still remote evidence, but it cannot be model-attributed.
            Point(1_120, 70, 1, 0, 0),
            Point(1_180, 60, 2, 0, 0),
        };
        var idleRereadScene = Scene(idleReread);
        var idleRereadEffective = idleRereadScene.Remaining;
        var idleRereadLines = GraphPlotProjection.BuildRemainingLines(idleRereadScene);
        Assert.Equal(70d, idleRereadEffective[2]);
        Assert.Equal(60d, idleRereadEffective[3]);
        Assert.Equal([1_060d, 1_120d], idleRereadLines.Dashed.X);
        Assert.Equal([90d, 70d], idleRereadLines.Dashed.Y);

        var terminal = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_060, 90, 1, 0, 0),
            Point(1_120, 90, 2, 0, 0),
        };
        var terminalScene = Scene(terminal, end: 1_180);
        var terminalEffective = terminalScene.Remaining;
        var terminalLines = GraphPlotProjection.BuildRemainingLines(terminalScene);

        Assert.Equal(90d, terminalEffective[^1]);
        Assert.Equal([1_000d, 1_060d, double.NaN, 1_120d, 1_180d], terminalLines.Dashed.X);
        Assert.Equal([100d, 90d, double.NaN, 90d, 90d], terminalLines.Dashed.Y);
    }

    [Fact]
    public void Idle_band_never_labels_a_long_unobserved_spend_gap_as_idle()
    {
        var points = new[]
        {
            Point(1_000, 100, 1, 0, 0),
            Point(1_060, 100, 1, 0, 0),
            // No observations for two minutes; the cumulative increase is
            // only known at the endpoint and must not be rendered as usage
            // throughout the unobserved interval.
            Point(1_180, 90, 2, 0, 0),
        };

        Assert.DoesNotContain(Scene(points).IdleIntervals, candidate =>
            candidate.StartAt == 1_060 && candidate.EndAt == 1_180);
    }

    private static ApiHistorySample Point(long timestamp, double? remaining, double sol, double terra, double luna) =>
        new(timestamp, 2_000, remaining, sol, terra, luna, (ulong)sol, (ulong)terra, (ulong)luna);

    private static ApiHistorySample ConfirmedCumulativeSample(long timestamp, double value) =>
        new(
            timestamp,
            2_000,
            100,
            value,
            value,
            value,
            (ulong)value,
            (ulong)value,
            (ulong)value,
            ApiHistorySample.ConfirmedModelSource);

    private static void AssertFirstObservationModel(
        GraphModelLineProjection lines,
        double firstObserved,
        double terminalObserved)
    {
        Assert.Empty(lines.Flat.X);
        Assert.Equal([1_120d, 1_180d], lines.Rising.X);
        Assert.Equal([firstObserved, terminalObserved], lines.Rising.Y);
        Assert.Equal([1_000d, 1_120d], lines.Dashed.X);
        Assert.Equal([0d, firstObserved], lines.Dashed.Y);
    }

    private static void AssertHistorySample(
        ApiHistorySample actual,
        (long Timestamp, double? Remaining, double SolDollars, double TerraDollars, double LunaDollars,
            ulong SolTokens, ulong TerraTokens, ulong LunaTokens) expected)
    {
        Assert.Equal(expected.Timestamp, actual.Timestamp);
        Assert.Equal(expected.Remaining, actual.RemainingPercent);
        Assert.Equal(expected.SolDollars, actual.SolDollars!.Value, precision: 6);
        Assert.Equal(expected.TerraDollars, actual.TerraDollars!.Value, precision: 6);
        Assert.Equal(expected.LunaDollars, actual.LunaDollars!.Value, precision: 6);
        Assert.Equal(expected.SolTokens, actual.SolTokens);
        Assert.Equal(expected.TerraTokens, actual.TerraTokens);
        Assert.Equal(expected.LunaTokens, actual.LunaTokens);
    }

    private static async Task<ApiDetailsSnapshot> ParseDetailsFixtureAsync(JsonElement response)
    {
        var handler = new DetailsFixtureHandler(Encoding.UTF8.GetBytes(response.GetRawText()));
        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchDetailsAsync(CancellationToken.None);
        Assert.True(result.IsSuccess);
        Assert.Null(result.Failure);
        Assert.Equal(3, handler.RequestCount);
        return Assert.IsType<ApiDetailsSnapshot>(result.Snapshot);
    }

    private static async Task EventuallyAsync(Func<bool> condition)
    {
        var deadline = DateTimeOffset.UtcNow + TimeSpan.FromSeconds(2);
        while (!condition())
        {
            if (DateTimeOffset.UtcNow >= deadline)
            {
                throw new TimeoutException("Expected rollover presentation state was not reached.");
            }

            await Task.Delay(10);
        }
    }

    private sealed class CountingReadyHealthClient : ILoopbackHealthClient
    {
        public int CallCount { get; private set; }

        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default)
        {
            CallCount++;
            return Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version)));
        }
    }

    private sealed class SequenceFixtureDetailsClient(params ApiDetailsSnapshot[] snapshots)
        : ILoopbackDetailsClient
    {
        public int CallCount { get; private set; }

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var snapshot = snapshots[Math.Min(CallCount, snapshots.Length - 1)];
            CallCount++;
            return Task.FromResult(DetailsFetchResult.Success(snapshot));
        }
    }

    private sealed class SingleDetailsClient(DetailsFetchResult result) : ILoopbackDetailsClient
    {
        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(result);
    }

    private sealed class AlwaysReadyConnectionSupervisor : IConnectionSupervisor
    {
        public bool EnsureStarted(ClientSettings settings) => true;

        public ConnectionRestartOutcome RestartExplicit(ClientSettings settings) =>
            ConnectionRestartOutcome.NoChildRequired;

        public void Dispose()
        {
        }
    }

    private sealed class DetailsFixtureHandler(byte[] body) : HttpMessageHandler
    {
        private const string DetailsEndpoint = "http://127.0.0.1:8787/v1/details";
        private readonly byte[] body = body.ToArray();
        private readonly bool servesV2 = Encoding.UTF8.GetString(body).Contains(
            "\"api_version\":\"v2\"",
            StringComparison.Ordinal) || Encoding.UTF8.GetString(body).Contains(
                "\"api_version\": \"v2\"",
                StringComparison.Ordinal);

        public int RequestCount { get; private set; }

        public HttpRequestMessage? LastRequest { get; private set; }

        public HttpStatusCode StatusCode { get; private set; }

        public byte[] ReturnedBytes { get; private set; } = [];

        public long? ContentLength { get; private set; }

        public string? ContentType { get; private set; }

        public bool NoStore { get; private set; }

        public IReadOnlyList<string> PublishedPairValues { get; private set; } = [];

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            RequestCount++;
            Assert.Equal(HttpMethod.Get, request.Method);
            if (request.RequestUri?.AbsolutePath == "/v3/details")
            {
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound));
            }
            if (request.RequestUri?.AbsolutePath == "/v2/details")
            {
                if (!servesV2)
                {
                    return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound));
                }
            }
            else
            {
                Assert.Equal(DetailsEndpoint, request.RequestUri?.AbsoluteUri);
            }
            LastRequest = request;

            var response = new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new ByteArrayContent(body),
            };
            response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json")
            {
                CharSet = "utf-8",
            };
            response.Content.Headers.ContentLength = body.LongLength;
            response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
            response.Headers.TryAddWithoutValidation(
                PublishedPairHeader,
                CanonicalPublishedPair);

            StatusCode = response.StatusCode;
            ReturnedBytes = body;
            ContentLength = response.Content.Headers.ContentLength;
            ContentType = response.Content.Headers.ContentType?.ToString();
            NoStore = response.Headers.CacheControl?.NoStore == true;
            PublishedPairValues = response.Headers
                .GetValues(PublishedPairHeader)
                .ToArray();
            return Task.FromResult(response);
        }
    }

    private static GraphScene Scene(IReadOnlyList<ApiHistorySample> points, long? start = null, long? end = null) =>
        GraphScene.Create(points, GraphMetric.Dollars, start ?? points[0].Timestamp, end ?? points[^1].Timestamp);
}
