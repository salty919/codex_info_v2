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

    private static bool IsRenderableModel(string model) =>
        model is "ASTRA" or "LUNA" or "SOL" or "TERRA";

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

        var directQuotaDrop = monotonic
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
            directQuotaDrop,
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
        var expectedModelValues = new[]
            { -1d / 98d, 24d / 98d, 49d / 98d, 74d / 98d, 99d / 98d };
        var expectedRemainingValues = new[]
            { -100d / 98d, 2_400d / 98d, 4_900d / 98d, 7_400d / 98d, 9_900d / 98d };
        Assert.Equal(expectedModelValues.Length, projection.ModelValues.Count);
        Assert.Equal(expectedRemainingValues.Length, projection.RemainingValues.Count);
        for (var index = 0; index < expectedModelValues.Length; index++)
        {
            Assert.Equal(expectedModelValues[index], projection.ModelValues[index], precision: 12);
            Assert.Equal(expectedRemainingValues[index], projection.RemainingValues[index], precision: 12);
        }
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
    public void Hidden_models_do_not_change_axis_scale_idle_or_endpoint_candidates()
    {
        var samples = Enumerable.Range(0, 31)
            .Select(minute =>
            {
                var terra = minute < 30 ? 10_000d : 20_000d;
                var terraTokens = minute < 30 ? 10_000UL : 20_000UL;
                return new ApiHistorySample(
                    1_000 + minute * 60,
                    1_000_000,
                    100,
                    10,
                    terra,
                    20,
                    10,
                    terraTokens,
                    20);
            })
            .ToArray();
        var hidden = new HashSet<string>(StringComparer.Ordinal) { "TERRA" };
        var dollars = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 2_800);
        var hiddenDollars = GraphScene.Create(
            samples,
            GraphMetric.Dollars,
            1_000,
            2_800,
            confirmedGaps: null,
            hiddenModelNames: hidden);
        var tokens = GraphScene.Create(samples, GraphMetric.Tokens, 1_000, 2_800);
        var hiddenTokens = GraphScene.Create(
            samples,
            GraphMetric.Tokens,
            1_000,
            2_800,
            confirmedGaps: null,
            hiddenModelNames: hidden);

        Assert.True(dollars.ModelMaximum > hiddenDollars.ModelMaximum);
        Assert.True(tokens.ModelMaximum > hiddenTokens.ModelMaximum);
        Assert.True(
            GraphPlotProjection.BuildAxes(dollars, TimeZoneInfo.Utc, CultureInfo.InvariantCulture)
                .ModelDisplayMaximum >
            GraphPlotProjection.BuildAxes(hiddenDollars, TimeZoneInfo.Utc, CultureInfo.InvariantCulture)
                .ModelDisplayMaximum);
        Assert.True(
            GraphPlotProjection.BuildAxes(tokens, TimeZoneInfo.Utc, CultureInfo.InvariantCulture)
                .ModelDisplayMaximum >
            GraphPlotProjection.BuildAxes(hiddenTokens, TimeZoneInfo.Utc, CultureInfo.InvariantCulture)
                .ModelDisplayMaximum);

        var expectedIdle = new[] { new GraphIdleInterval(1_000, 2_740, false) };
        Assert.Equal(expectedIdle, dollars.IdleIntervals);
        Assert.Equal(expectedIdle, hiddenDollars.IdleIntervals);
        Assert.Contains("TERRA", dollars.ModelSeries.Keys);
        Assert.DoesNotContain("TERRA", hiddenDollars.ModelSeries.Keys);
        Assert.Contains(
            GraphPlotProjection.BuildEndpointLabels(dollars, CultureInfo.InvariantCulture),
            label => label.Series == GraphSeries.Terra);
        Assert.DoesNotContain(
            GraphPlotProjection.BuildEndpointLabels(hiddenDollars, CultureInfo.InvariantCulture),
            label => label.Series == GraphSeries.Terra);
    }

    [Fact]
    public void PlotProjectionKeepsNativePixelEndpointGutterFixedAcrossUnboundedWidths()
    {
        const double referenceWidth = 800;
        double[] currentWidths = [320, 800, 1_200, 10_000];
        var points = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(2_000, 75, 2, 4, 6),
        };

        foreach (var (metric, expectedGutter) in new[]
        {
            (GraphMetric.Dollars, 94d),
            (GraphMetric.Tokens, 126d),
        })
        {
            var scene = GraphScene.Create(points, metric, points[0].Timestamp, points[^1].Timestamp);

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
                Assert.Equal(10d, labelGap, precision: 9);
            }
        }
    }

    [Fact]
    public void PlotProjectionFallbackUsesTheNativeCanonicalDataAreaWidth()
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
                788,
                788);

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
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                ModelSamples =
                [
                    new ApiHistoryModelSample("ASTRA", 1_000_000, 200_000, 100_000, null) { CacheWriteInputTokens = 100_000, TotalTokens = 1_100_000 },
                    new ApiHistoryModelSample("LUNA", 1, 0, 0, 1) { CacheWriteInputTokens = 0, TotalTokens = 1 },
                ],
            },
            new ApiHistorySample(
                1_060, 2_000, 89, null, null, null, null, null, null,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                ModelSamples =
                [
                    new ApiHistoryModelSample("ASTRA", 2_000_000, 400_000, 200_000, null) { CacheWriteInputTokens = 200_000, TotalTokens = 2_200_000 },
                    new ApiHistoryModelSample("LUNA", 2, 0, 0, 2) { CacheWriteInputTokens = 0, TotalTokens = 2 },
                    new ApiHistoryModelSample("SOL", 3, 0, 0, 3) { CacheWriteInputTokens = 0, TotalTokens = 3 },
                ],
            },
            new ApiHistorySample(
                1_120, 2_000, 88, null, null, null, null, null, null,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                ModelSamples =
                [
                    new ApiHistoryModelSample("ASTRA", 3_000_000, 600_000, 300_000, null) { CacheWriteInputTokens = 300_000, TotalTokens = 3_300_000 },
                    new ApiHistoryModelSample("LUNA", 2, 0, 0, 2) { CacheWriteInputTokens = 0, TotalTokens = 2 },
                    new ApiHistoryModelSample("SOL", 4, 0, 0, 4) { CacheWriteInputTokens = 0, TotalTokens = 4 },
                ],
            },
        };

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_120);
        var astra = GraphPlotProjection.BuildModelLines(scene, scene.Astra);
        var luna = GraphPlotProjection.BuildModelLines(scene, scene.Luna);
        var sol = GraphPlotProjection.BuildModelLines(scene, scene.Sol);

        Assert.Equal([13.45d, 26.9d, 40.35d], scene.Astra);
        Assert.Equal([13.45d, 26.9d, 40.35d], scene.ModelSeries["ASTRA"]);
        Assert.Equal([1d, 2d, 2d], scene.Luna);
        Assert.True(double.IsNaN(scene.Sol[0]));
        Assert.Equal([3d, 4d], scene.Sol.Skip(1));
        Assert.Equal([1_000d, 1_060d, 1_120d], astra.Rising.X);
        Assert.Equal([13.45d, 26.9d, 40.35d], astra.Rising.Y);
        Assert.Empty(astra.Dashed.X);
        Assert.Equal([1_000d, 1_060d], luna.Rising.X);
        Assert.Equal([1_060d, 1_120d], luna.Flat.X);
        Assert.Empty(luna.Dashed.X);
        Assert.Equal([1_060d, 1_120d], sol.Rising.X);
        Assert.Empty(sol.Dashed.X);
        Assert.Empty(scene.IdleIntervals);

        var labels = GraphPlotProjection.BuildEndpointLabels(scene, CultureInfo.InvariantCulture);
        foreach (var series in new[] { GraphSeries.Remaining, GraphSeries.Sol, GraphSeries.Luna, GraphSeries.Astra })
            Assert.Contains(labels, label => label.Series == series);

        var control = new GraphPlotControl { Scene = scene };
        var rendered = control.Plot.GetImage(940, 480);
        var pixels = rendered.GetArrayRGB();
        var gutterStart = (int)Math.Ceiling(control.Plot.GetPixel(new ScottPlot.Coordinates(1_120, 0)).X);
        foreach (var color in new[] { (86, 178, 245), (168, 140, 245), (230, 162, 60), (239, 106, 106) })
        {
            var found = false;
            for (var x = gutterStart + 1; x < pixels.GetLength(1) && !found; x++)
                for (var y = 0; y < pixels.GetLength(0) && !found; y++)
                    found = Math.Abs(pixels[y, x, 0] - color.Item1) <= 24 &&
                            Math.Abs(pixels[y, x, 1] - color.Item2) <= 24 &&
                            Math.Abs(pixels[y, x, 2] - color.Item3) <= 24;
            Assert.True(found, $"Missing endpoint gutter pixels for {color}; start={gutterStart}, dimensions={pixels.GetLength(0)}x{pixels.GetLength(1)}");
        }
    }

    [Fact]
    public void Scene_rejects_only_the_anomalous_metric_and_keeps_other_series_measured()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 1, 2, 3),
                Point(1_060, 90, 2, 1, 4),
                Point(1_120, 80, 2, 2, 4),
                Point(1_180, 70, 75, 3, 5),
            ]);

        Assert.Equal([true, false, true, true], scene.ModelVectorAvailable);
        Assert.Equal([1_060L], scene.CorrectionStarts.Order());
        Assert.Equal([1d, 2d, 2d, 75d], scene.Sol);
        Assert.Equal([2d, 2d, 2d, 3d], scene.Terra);
        Assert.Equal([3d, 4d, 4d, 5d], scene.Luna);

        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        Assert.Equal([1_000d, 1_060d, double.NaN, 1_120d, 1_180d], lines.Rising.X);
        Assert.Equal([1d, 2d, double.NaN, 2d, 75d], lines.Rising.Y);
        Assert.Equal([1_060d, 1_120d], lines.Flat.X);
        Assert.Empty(lines.Dashed.X);

        var quotaLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Equal([1_000d, 1_060d, 1_120d, 1_180d], quotaLines.Solid.X);
        Assert.Equal([100d, 90d, 80d, 70d], quotaLines.Solid.Y);
        Assert.Empty(quotaLines.Dashed.X);
    }

    [Fact]
    public void Live_incident_regressions_are_held_and_dashed_without_vertical_drops()
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
        Assert.Equal([60L, 120L, 180L], scene.CorrectionStarts.Order());
        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Empty(model.Flat.X);
        Assert.Empty(model.Rising.X);
        Assert.Equal([0d, 240d, 300d], model.Dashed.X);
        Assert.Equal([176.04d, 184.14d, 184.14d], model.Dashed.Y);
        Assert.Equal([0d, 60d, 120d, 180d, 240d], remaining.Solid.X);
        Assert.Equal([85d, 84d, 83d, 82d, 81d], remaining.Solid.Y);
        Assert.Equal([240d, 300d], remaining.Dashed.X);
        Assert.Equal([81d, 81d], remaining.Dashed.Y);
    }

    [Fact]
    public void Model_lines_keep_jittered_direct_intervals_measured()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 90, 1, 0, 0),
                Point(1_121, 80, 2, 0, 0),
                Point(1_242, 70, 2, 0, 0),
            ]);

        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);

        Assert.Equal([1_000d, 1_060d, 1_121d], lines.Rising.X);
        Assert.Equal([0d, 1d, 2d], lines.Rising.Y);
        Assert.Equal([1_121d, 1_242d], lines.Flat.X);
        Assert.Equal([2d, 2d], lines.Flat.Y);
        Assert.Empty(lines.Dashed.X);
    }

    [Fact]
    public void Isolated_anomalies_are_detected_across_sampling_jitter()
    {
        var scene = GraphScene.Create(
            [
                Point(0, 90, 10, 0, 0),
                Point(61, 80, 20, 0, 0),
                Point(183, 89, 11, 0, 0),
            ],
            GraphMetric.Tokens,
            0,
            183);

        Assert.Equal([10d, 10d, 11d], scene.Sol);
        Assert.Equal([true, false, true], scene.ModelVectorAvailable);
        Assert.Equal([90d, 90d, 89d], scene.Remaining);
        Assert.Equal(GraphRemainingOrigin.MonotonicHold, scene.RemainingOrigins[1]);
        Assert.Equal(GraphRemainingOrigin.Raw, scene.RemainingOrigins[2]);
    }

    [Fact]
    public void Remaining_quota_observations_stay_solid_when_model_rows_are_flat()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 90, 1, 0, 0),
                Point(1_120, 70, 1, 0, 0),
            ]);

        var lines = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Equal([100d, 90d, 70d], scene.ObservedRemainingValues);
        Assert.Equal([1_000d, 1_060d, 1_120d], lines.Solid.X);
        Assert.Equal([100d, 90d, 70d], lines.Solid.Y);
        Assert.Empty(lines.Dashed.X);
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
        Assert.Equal([1_000d, 1_120d], lines.Dashed.X);
        Assert.Equal([100d, 80d], lines.Dashed.Y);
    }

    [Fact]
    public void One_old_local_observation_is_held_dashed_to_the_exact_period_end()
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
            2_000);

        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);
        var labels = GraphPlotProjection.BuildEndpointLabels(scene, CultureInfo.InvariantCulture);

        Assert.Equal(2, samples.Count);
        Assert.Equal(1_000, samples[0].Timestamp);
        Assert.Equal(2_000, samples[^1].Timestamp);
        Assert.True(samples[^1].IsSyntheticTail);
        Assert.Empty(model.Flat.X);
        Assert.Empty(model.Rising.X);
        Assert.Equal([1_000d, 2_000d], model.Dashed.X);
        Assert.Equal([5d, 5d], model.Dashed.Y);
        Assert.Empty(remaining.Solid.X);
        Assert.Equal([1_000d, 2_000d], remaining.Dashed.X);
        Assert.Equal([80d, 80d], remaining.Dashed.Y);
        Assert.Contains(labels, label => label.Series == GraphSeries.Sol && label.Text == "$5.00");
        Assert.Contains(labels, label => label.Series == GraphSeries.Remaining && label.Text == "80%");
    }

    [Fact]
    public void Confirmed_history_gap_is_continuous_and_dashed_across_the_gap()
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
        Assert.Equal([1_060d, 1_120d], model.Dashed.X);
        Assert.Equal([1d, 2d], model.Dashed.Y);
        Assert.Equal([1_000d, 1_060d, double.NaN, 1_120d, 1_180d], remaining.Solid.X);
        Assert.Equal([1_060d, 1_120d], remaining.Dashed.X);
        Assert.Equal([90d, 80d], remaining.Dashed.Y);
        Assert.Empty(scene.IdleIntervals);
    }

    [Fact]
    public void PlotProjectionKeepsSparseEqualDirectEndpointsFlat()
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
    }

    [Fact]
    public void PlotProjectionKeepsSparseDirectIncreaseMeasured()
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
        Assert.Equal([1_000d, 1_120d, 1_180d], model.Rising.X);
        Assert.Equal([0d, 1d, 2d], model.Rising.Y);
        Assert.Empty(model.Dashed.X);
        Assert.Equal([1_000d, 1_120d, 1_180d], remaining.Solid.X);
        Assert.Equal([100d, 90d, 80d], remaining.Solid.Y);
        Assert.Empty(remaining.Dashed.X);
        Assert.Empty(scene.IdleIntervals);
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
    public void PlotProjectionKeepsSingleMeasuredFlatLineAndAcceptsSparseCompleteIdleEndpoints()
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

        Assert.Equal([new GraphIdleInterval(2_000, 7_000, false)], visible);
        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        Assert.Equal([1_000d, 1_060d], model.Flat.X);
        Assert.Equal([2_000d, 7_000d], model.Idle.X);
        Assert.Equal([1_060d, 2_000d], model.Rising.X);
        Assert.Equal([7_000d, 87_400d], model.Dashed.X);

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
        Assert.Equal("#1A2838", GraphPlotControl.IdleBandColorHex);
        Assert.Equal(1.0, GraphPlotControl.IdleBandOpacity);
    }

    [Fact]
    public void IdleBandRasterIsOpaqueAboveTheMajorGridAndBelowDataSeries()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 1, 0, 0),
                Point(2_800, 100, 1, 0, 0),
                Point(2_860, 99, 2, 0, 0),
            ]);
        Assert.Equal(
            [new GraphIdleInterval(1_000, 2_800, PreserveBoundary: false)],
            scene.IdleIntervals);

        var control = new GraphPlotControl { Scene = scene };
        var rendered = control.Plot.GetImage(940, 480);
        var pixels = rendered.GetArrayRGB();
        var expected = (Red: (byte)0x1A, Green: (byte)0x28, Blue: (byte)0x38);
        foreach (var timestamp in new[] { 1_045d, 1_050d })
        {
            var pixel = control.Plot.GetPixel(new ScottPlot.Coordinates(timestamp, 0.75));
            var x = (int)Math.Round(pixel.X);
            var y = (int)Math.Round(pixel.Y);
            Assert.InRange(x, 0, pixels.GetLength(1) - 1);
            Assert.InRange(y, 0, pixels.GetLength(0) - 1);
            Assert.Equal(expected.Red, pixels[y, x, 0]);
            Assert.Equal(expected.Green, pixels[y, x, 1]);
            Assert.Equal(expected.Blue, pixels[y, x, 2]);
        }
    }

    [Fact]
    public void InferredLinesAreThinnerThanMeasuredModelLines()
    {
        Assert.Equal(3f, GraphPlotControl.MeasuredModelLineWidth);
        Assert.Equal(3f, GraphPlotControl.MeasuredFlatModelLineWidth);
        Assert.Equal(3f, GraphPlotControl.MeasuredRemainingLineWidth);
        Assert.Equal(1f, GraphPlotControl.InferredLineWidth);
        Assert.Equal(
            GraphPlotControl.MeasuredModelLineWidth,
            GraphPlotControl.MeasuredFlatModelLineWidth);
        Assert.True(GraphPlotControl.InferredLineWidth < GraphPlotControl.MeasuredModelLineWidth);
        Assert.True(GraphPlotControl.InferredLineWidth < GraphPlotControl.MeasuredRemainingLineWidth);
    }

    [Fact]
    public void IdleRenderContractSeparatesSustainedThinSolidsFromShortFlatsAndMissing()
    {
        static ApiHistorySample Complete(long timestamp, double? remaining, double value) =>
            new(
                timestamp,
                2_000,
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
                TaskActiveSincePrevious = false,
                ModelSamples = new[] { "SOL", "TERRA", "LUNA", "ASTRA" }
                    .Select((name, index) => new ApiHistoryModelSample(
                        name,
                        null,
                        null,
                        null,
                        value + index)
                    {
                        TotalTokens = (ulong)(value * 100) + (ulong)index,
                    })
                    .ToArray(),
            };

        var samples = new[]
        {
            Complete(0, 90, 1),
            Complete(600, 90, 1),
            Complete(660, 80, 2),
            Complete(959, 80, 2),
            Complete(1_200, 70, 3),
            Complete(1_260, null, 3) with
            {
                ModelSource = ApiHistorySample.UnavailableModelSource,
                ModelsComplete = false,
                ModelSamples = [],
            },
            Complete(1_320, 60, 4),
        };

        foreach (var metric in new[] { GraphMetric.Dollars, GraphMetric.Tokens })
        {
            var scene = GraphScene.Create(samples, metric, 0, 1_320);
            Assert.Equal([new GraphIdleInterval(0, 600, false)], scene.IdleIntervals);
            foreach (var values in scene.ModelSeries.Values)
            {
                var lines = GraphPlotProjection.BuildModelLines(scene, values);
                Assert.Equal([(0L, 600L)], SegmentPairs(lines.Idle));
                Assert.Contains((660L, 959L), SegmentPairs(lines.Flat));
                Assert.NotEmpty(lines.Rising.X);
                Assert.NotEmpty(lines.Dashed.X);
                Assert.All(lines.Idle.Y, value => Assert.Equal(lines.Idle.Y[0], value));
            }
            var remaining = GraphPlotProjection.BuildRemainingLines(scene);
            Assert.Equal([(0L, 600L)], SegmentPairs(remaining.Idle));
            Assert.Contains((660L, 959L), SegmentPairs(remaining.Solid));
            Assert.NotEmpty(remaining.Dashed.X);
            Assert.All(remaining.Idle.Y, value => Assert.Equal(remaining.Idle.Y[0], value));
        }

        var dollarChange = new[]
        {
            Complete(0, 90, 1),
            Complete(600, 90, 1) with
            {
                ModelSamples = Complete(600, 90, 1).ModelSamples!
                    .Select(model => model.Name == "ASTRA"
                        ? model with { Dollars = model.Dollars + 0.01 }
                        : model)
                    .ToArray(),
            },
        };
        var repricedDollarScene = GraphScene.Create(dollarChange, GraphMetric.Dollars, 0, 600);
        Assert.Equal(
            [new GraphIdleInterval(0, 600, false)],
            repricedDollarScene.IdleIntervals);
        Assert.Equal(repricedDollarScene.Astra[0], repricedDollarScene.Astra[1]);
        var repricedDollarLines = GraphPlotProjection.BuildModelLines(
            repricedDollarScene,
            repricedDollarScene.Astra);
        Assert.Equal([(0L, 600L)], SegmentPairs(repricedDollarLines.Idle));
        Assert.Empty(repricedDollarLines.Rising.X);
        Assert.All(
            repricedDollarLines.Idle.Y,
            value => Assert.Equal(repricedDollarLines.Idle.Y[0], value));

        var repricedTokenScene = GraphScene.Create(dollarChange, GraphMetric.Tokens, 0, 600);
        Assert.Equal(
            [new GraphIdleInterval(0, 600, false)],
            repricedTokenScene.IdleIntervals);

        Assert.Equal(1f, GraphPlotControl.IdleLineWidth);
        Assert.Equal(1f, GraphPlotControl.InferredLineWidth);
        Assert.Equal(3f, GraphPlotControl.MeasuredModelLineWidth);
        Assert.Equal(3f, GraphPlotControl.MeasuredRemainingLineWidth);
    }

    [Fact]
    public void MonotoneCubicProjectionMatchesFixedNoOvershootOracle()
    {
        var increasing = GraphPlotProjection.EvaluateMonotoneCubicInterval(
            [0d, 1d, 2d],
            [0d, 1d, 1d],
            0,
            [0.25d, 0.5d, 0.75d]);
        Assert.Equal([0.3671875d, 0.6875d, 0.9140625d], increasing);

        var decreasing = GraphPlotProjection.EvaluateMonotoneCubicInterval(
            [0d, 1d, 2d],
            [100d, 90d, 70d],
            0,
            [0.5d]);
        Assert.Equal(96.04166666666667d, decreasing[0], 12);
        var second = GraphPlotProjection.EvaluateMonotoneCubicInterval(
            [0d, 1d, 2d],
            [100d, 90d, 70d],
            1,
            [0.5d]);
        Assert.Equal(81.45833333333333d, second[0], 12);
    }

    [Fact]
    public void Continuous_measured_run_uses_one_joined_canonical_path()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 90, 1, 0, 0),
                Point(1_120, 80, 2, 0, 0),
            ],
            1_000,
            1_120);

        var model = GraphPlotProjection.BuildCanonicalModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildCanonicalRemainingLines(scene);

        Assert.Equal(1, model.Rising.Path.Count(character => character == 'M'));
        Assert.Equal(1, remaining.Solid.Path.Count(character => character == 'M'));
        Assert.DoesNotContain(model.Rising.Line.X, double.IsNaN);
        Assert.DoesNotContain(remaining.Solid.Line.X, double.IsNaN);
    }

    [Fact]
    public void BoundedMissingIntervalUsesTheSameSmoothedAnchorGeometry()
    {
        var scene = GraphScene.Create(
            [
                Point(0, 100, 0, 0, 0),
                Point(120, 90, 1, 0, 0),
                Point(180, 70, 3, 0, 0),
            ],
            GraphMetric.Tokens,
            0,
            180,
            [new GraphConfirmedGap(0, 120)]);

        var model = GraphPlotProjection.BuildCanonicalModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildCanonicalRemainingLines(scene);

        Assert.NotEmpty(model.Rising.Path);
        Assert.NotEmpty(model.Dashed.Path);
        Assert.False(
            model.Dashed.Path.StartsWith(
                "M0.00 99.00 L0.40 98.80 M0.67 98.67 L1.08 98.47",
                StringComparison.Ordinal),
            "a bounded missing interval must retain the PCHIP curve defined by valid anchors");
        Assert.NotEmpty(remaining.Solid.Path);
        Assert.StartsWith("M0.00 1.00 L0.25 1.00", remaining.Dashed.Path);
    }

    [Fact]
    public void CanonicalRenderProjectionUsesTheNativeNormalizedDashCadence()
    {
        var scene = GraphScene.Create(
            [
                Point(1_000, 100, 10, 0, 0),
                Point(1_600, 90, 10, 0, 0),
            ],
            GraphMetric.Dollars,
            1_000,
            1_600,
            [new GraphConfirmedGap(1_000, 1_600)]);

        var model = GraphPlotProjection.BuildCanonicalModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildCanonicalRemainingLines(scene);

        Assert.StartsWith("M0.00 1.00 L0.25 1.00", model.Dashed.Path);
        Assert.Contains("L0.45 1.00 M0.75", model.Dashed.Path, StringComparison.Ordinal);
        Assert.StartsWith("M0.00 1.00 L0.25", remaining.Dashed.Path);
        Assert.Contains("M0.75", remaining.Dashed.Path, StringComparison.Ordinal);
        Assert.True(model.Dashed.Line.X.Count > 100);
        Assert.True(remaining.Dashed.Line.X.Count > 100);
        Assert.All(
            model.Dashed.Line.X.Where(double.IsFinite),
            value => Assert.InRange(value, scene.PeriodStartAt, scene.PeriodEndAt));
    }

    [Fact]
    public void CanonicalRenderProjectionUsesNativeBinaryFixedTwoRounding()
    {
        var lowScene = Scene(
            [
                Point(1_000, 97.30, 0, 0, 0),
                Point(1_060, 97.25, 1, 0, 0),
            ],
            1_000,
            1_060);
        var highScene = Scene(
            [
                Point(1_000, 82.30, 0, 0, 0),
                Point(1_060, 82.25, 1, 0, 0),
            ],
            1_000,
            1_060);

        var lowRemaining = GraphPlotProjection.BuildCanonicalRemainingLines(lowScene);
        var highRemaining = GraphPlotProjection.BuildCanonicalRemainingLines(highScene);

        Assert.StartsWith("M0.00 3.65 L0.25 3.65", lowRemaining.Solid.Path);
        Assert.EndsWith("L100.00 3.69", lowRemaining.Solid.Path);
        Assert.StartsWith("M0.00 18.35 L0.25 18.35", highRemaining.Solid.Path);
        Assert.EndsWith("L100.00 18.39", highRemaining.Solid.Path);
    }

    [Fact]
    public void AxisQuarterTimestampsUseTheNativeTruncationRule()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_103, 99, 1, 0, 0),
            ],
            1_000,
            1_103);

        var axes = GraphPlotProjection.BuildAxes(
            scene,
            TimeZoneInfo.Utc,
            CultureInfo.InvariantCulture);

        Assert.Equal(
            [1_000L, 1_025L, 1_051L, 1_077L, 1_103L],
            axes.BottomTimestampValues);
    }

    [Fact]
    public void RemainingDoesNotCreateASecondBoundaryMarkerTrajectory()
    {
        var scene = Scene(
            [
                Point(1_000, 100, 0, 0, 0),
                Point(1_060, 97.5, 10, 0, 0),
            ],
            1_000,
            1_060);

        var markers = GraphPlotProjection.BuildCanonicalRemainingMarkers(scene);

        Assert.Empty(markers);
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
    public void IncompleteModelSetIsNeverLabelledIdleOrForcedToPredictedRemaining()
    {
        var samples = new[]
        {
            new ApiHistorySample(
                1_000, 2_000, 90, 10, 0, 2, 10, 0, 2,
                ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
            },
            new ApiHistorySample(
                1_060, 2_000, 89, 11, 0, 2, 11, 0, 2,
                ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
            },
        };

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_060);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Empty(scene.IdleIntervals);
        Assert.Equal([1_000d, 1_060d], remaining.Solid.X);
        Assert.Equal([90d, 89d], remaining.Solid.Y);
        Assert.Empty(remaining.Dashed.X);
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
        Assert.Equal(["$6.00", "75%", "$4.00", "$2.00"], labels.Select(label => label.Text));
        Assert.Equal(0.01d, labels[0].NormalizedTop, precision: 7);
        Assert.Equal(0.255d, labels[1].NormalizedTop, precision: 7);
        Assert.Equal((double)(float)(0.99d - 0.98d * 4d / 6d), labels[2].NormalizedTop);
        Assert.Equal((double)(float)(0.99d - 0.98d * 2d / 6d), labels[3].NormalizedTop);
        Assert.Equal((0.99d - labels[0].ArrangedTop) / 0.98d * 6d, labels[0].AxisValue, precision: 7);
        Assert.Equal((0.99d - labels[1].ArrangedTop) / 0.98d * 100d, labels[1].AxisValue, precision: 7);
        Assert.Equal((0.99d - labels[2].ArrangedTop) / 0.98d * 6d, labels[2].AxisValue, precision: 7);
        Assert.Equal((0.99d - labels[3].ArrangedTop) / 0.98d * 6d, labels[3].AxisValue, precision: 7);
        Assert.True(labels.Zip(labels.Skip(1)).All(pair =>
            pair.Second.ArrangedTop - pair.First.ArrangedTop >= 16d / 204d - 1e-7));
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

        Assert.Equal([1_020L, 1_080L, 1_200L], samples.Select(sample => sample.Timestamp));
        Assert.Null(samples[0].RemainingPercent);
        Assert.Equal(1, samples[0].SolDollars);
        Assert.Equal(10UL, samples[0].SolTokens);
        Assert.Equal(90, samples[1].RemainingPercent);
        Assert.True(samples[^1].IsSyntheticTail);
        var scene = Scene(samples, period.StartAt, period.EndAt);
        Assert.True(double.IsNaN(scene.Remaining[0]));
        Assert.Equal(GraphRemainingOrigin.Missing, scene.RemainingOrigins[0]);
        Assert.Equal(90, scene.Remaining[1]);
        Assert.Equal([false, true, false], scene.RemainingObserved);
        Assert.True(double.IsNaN(scene.ObservedRemainingValues[0]));
        Assert.Equal(90, scene.ObservedRemainingValues[1]);
        Assert.True(double.IsNaN(scene.ObservedRemainingValues[2]));
        var rawLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Empty(rawLines.Solid.X);
        Assert.Equal([1_080d, 1_200d], rawLines.Dashed.X);
        Assert.Equal([90d, 90d], rawLines.Dashed.Y);

        var displayLines = GraphPlotProjection.BuildCanonicalRemainingLines(
            scene,
            GraphRemainingBaselineMode.PeriodStartAtFullQuota);
        Assert.StartsWith("M0.00 1.00", displayLines.Dashed.Path);
        Assert.Equal((double)period.StartAt, displayLines.Dashed.Line.X[0]);
        Assert.Equal(100d, displayLines.Dashed.Line.Y[0]);
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
                // Adapter-only invalid injection beyond the accepted period;
                // this row tests defensive end filtering.
                new ApiHistorySample(1_201, 2_000, 89, 99, 0, 0, 990, 0, 0),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_140);

        Assert.Equal([1_020L, 1_080L, 1_200L], samples.Select(sample => sample.Timestamp));
        Assert.Equal(7, samples[^2].SolDollars);
        Assert.Equal(70UL, samples[^2].SolTokens);
        Assert.Equal(90, samples[^2].RemainingPercent);
        Assert.True(samples[^1].IsSyntheticTail);
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
            [1_080L, 1_200L],
            unavailableSamples.Select(sample => sample.Timestamp));
        Assert.True(unavailableSamples[^1].IsSyntheticTail);
        var unavailableScene = GraphScene.Create(
            unavailableSamples,
            GraphMetric.Dollars,
            1_020,
            1_200);
        var unavailableModel = GraphPlotProjection.BuildModelLines(
            unavailableScene,
            unavailableScene.Sol);
        var unavailableRemaining = GraphPlotProjection.BuildRemainingLines(unavailableScene);
        Assert.Empty(unavailableModel.Flat.X);
        Assert.Empty(unavailableModel.Rising.X);
        Assert.Empty(unavailableModel.Dashed.X);
        Assert.Equal([1_080d, 1_200d], unavailableRemaining.Dashed.X);
        Assert.Equal([90d, 90d], unavailableRemaining.Dashed.Y);
    }

    [Fact]
    public void Idle_intervals_require_ten_minutes_between_equal_direct_endpoints()
    {
        var points = Enumerable.Range(0, 11)
            .Select(minute => Point(1_000 + minute * 60, 100, 0, 0, 0))
            .ToArray();

        var intervals = Scene(points, 1_000, 1_600).IdleIntervals;

        Assert.Single(intervals);
        Assert.Equal((1_000L, 1_600L, false), (intervals[0].StartAt, intervals[0].EndAt, intervals[0].PreserveBoundary));

        var sparse = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_600, 100, 0, 0, 0),
        };
        var sparseIntervals = Scene(sparse, 1_000, 1_600).IdleIntervals;

        Assert.Equal(
            [new GraphIdleInterval(1_000, 1_600, false)],
            sparseIntervals);

        var nineMinutes = Enumerable.Range(0, 10)
            .Select(minute => Point(1_000 + minute * 60, 100, 0, 0, 0))
            .ToArray();
        Assert.Empty(Scene(nineMinutes, 1_000, 1_540).IdleIntervals);
    }

    [Fact]
    public void Remaining_interpolates_a_sparse_delayed_quota_interval_as_prediction()
    {
        var points = new[]
        {
            Point(1_000, 87, 0, 0, 0),
            Point(1_060, null, 140, 0, 0),
            Point(1_120, null, 420, 0, 0),
            Point(1_240, 1, 420, 0, 0),
        };

        var effective = Scene(points).Remaining;

        Assert.Equal([87d, 65.5d, 44d, 1d], effective);
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
        Assert.Equal(
            expectedRawTimestamps.Zip(expectedRawTimestamps.Skip(1)),
            SegmentPairs(terraLines.Flat));
        Assert.Empty(terraLines.Rising.X);
        Assert.Empty(terraLines.Dashed.X);
        Assert.NotEmpty(remainingLines.Dashed.X);
        Assert.Equal(firstObservation, remainingLines.Dashed.X[0]);
        Assert.Equal(87d, remainingLines.Dashed.Y[0]);
        Assert.DoesNotContain(remainingLines.Solid.X, timestamp => timestamp < firstObservation);
        Assert.DoesNotContain(remainingLines.Dashed.X, timestamp => timestamp < firstObservation);
        Assert.Equal(
            expectedRawTimestamps.Skip(2).Zip(expectedRawTimestamps.Skip(3)),
            SegmentPairs(solLines.Flat));
        Assert.Equal(
            expectedRawTimestamps.Take(2).Zip(expectedRawTimestamps.Skip(1).Take(2)),
            SegmentPairs(solLines.Rising));
        Assert.Empty(solLines.Dashed.X);
        Assert.Equal(
            expectedRawTimestamps.Zip(expectedRawTimestamps.Skip(1)),
            SegmentPairs(lunaLines.Flat));
        Assert.Empty(lunaLines.Rising.X);
        Assert.Empty(lunaLines.Dashed.X);
        Assert.Equal([2_000_000_220d, 2_000_000_280d], remainingLines.Solid.X);
        Assert.Equal([1d, 1d], remainingLines.Solid.Y);
    }

    [Fact]
    public async Task Issue137_shared_v3_oracle_matches_values_roles_idle_and_pair_through_http_parser()
    {
        var fixturePath = Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_evidence_oracle.json");
        using var document = JsonDocument.Parse(File.ReadAllText(fixturePath));
        var root = document.RootElement.GetProperty("parity_v3");
        var expected = root.GetProperty("expected");
        var pair = root.GetProperty("published_pair").GetString()!;
        var periodBody = Encoding.UTF8.GetBytes(
            "{\"api_version\":\"v3\",\"history_periods\":[" +
            root.GetProperty("period").GetRawText() +
            "]}");
        var historyBody = Encoding.UTF8.GetBytes(root.GetProperty("history_page").GetRawText());
        var periodId = root.GetProperty("period").GetProperty("id").GetString()!;

        var handler = new SplitHistoryFixtureHandler(periodBody, historyBody, pair, periodId);
        using var client = new LoopbackStatusClient(handler);
        var periodsResult = await client.FetchHistoryPeriodsAsync(CancellationToken.None);
        var pageResult = await client.FetchHistoryPageAsync(periodId, cancellationToken: CancellationToken.None);

        Assert.True(periodsResult.IsSuccess);
        Assert.True(pageResult.IsSuccess);
        Assert.Equal(2, handler.RequestCount);
        var periods = Assert.IsType<ApiHistoryPeriodsSnapshot>(periodsResult.Snapshot);
        var page = Assert.IsType<ApiHistoryPage>(pageResult.Page);
        Assert.Equal(pair, periods.PublishedPair.ToString());
        Assert.Equal(periods.PublishedPair, page.PublishedPair);
        var parsedPeriod = Assert.Single(periods.Periods);
        Assert.Equal(periodId, parsedPeriod.Id);
        Assert.Equal(parsedPeriod.ResetAt, page.Samples.Select(sample => sample.ResetAt).Distinct().Single());

        var period = parsedPeriod with { Samples = page.Samples };
        var samples = GraphWindowViewModel.BuildGraphSamples(period, period.EndAt);
        var gaps = page.HistoryGaps
            .Select(gap => new GraphConfirmedGap(gap.StartAt, gap.EndAt))
            .ToArray();
        var scene = GraphScene.Create(
            samples,
            GraphMetric.Dollars,
            period.StartAt,
            period.EndAt,
            gaps);

        Assert.Equal(
            expected.GetProperty("model_universe").EnumerateArray().Select(value => value.GetString()),
            scene.ModelSeries.Keys);
        Assert.DoesNotContain("ASTRA", scene.ModelSeries.Keys);
        foreach (var model in expected.GetProperty("accepted_models").EnumerateObject())
        {
            var expectedValues = model.Value
                .EnumerateArray()
                .Select(value => value.ValueKind == JsonValueKind.Null ? double.NaN : value.GetDouble())
                .ToArray();
            Assert.Equal(expectedValues, scene.ModelSeries[model.Name].Take(expectedValues.Length));
        }

        var expectedRemaining = expected.GetProperty("remaining_points").EnumerateArray().ToArray();
        Assert.Equal(
            expectedRemaining.Select(point => (double)point.GetProperty("timestamp").GetInt64()),
            scene.Timestamps);
        Assert.Equal(expectedRemaining.Length, scene.Remaining.Count);
        for (var index = 0; index < expectedRemaining.Length; index++)
        {
            Assert.Equal(
                expectedRemaining[index].GetProperty("effective").GetDouble(),
                scene.Remaining[index],
                precision: 12);
        }
        Assert.Equal(
            expectedRemaining.Select(point => point.GetProperty("origin").GetString()),
            scene.RemainingOrigins.Select((origin, index) => RemainingOriginName(scene, origin, index)));
        Assert.Equal(
            expected.GetProperty("idle_intervals").EnumerateArray().Select(interval =>
                new GraphIdleInterval(interval[0].GetInt64(), interval[1].GetInt64(), PreserveBoundary: false)),
            scene.IdleIntervals);

        var modelSegments = expected.GetProperty("model_segments");
        foreach (var model in scene.ModelSeries)
        {
            var lines = GraphPlotProjection.BuildModelLines(scene, model.Value);
            var expectedSegments = modelSegments.GetProperty(model.Key);
            Assert.Equal(
                SegmentPairs(expectedSegments.GetProperty("solid")),
                SegmentPairs(lines.Idle, lines.Flat, lines.Rising));
            Assert.Equal(
                SegmentPairs(expectedSegments.GetProperty("dashed")),
                SegmentPairs(lines.Dashed));
        }

        var remainingLines = GraphPlotProjection.BuildRemainingLines(scene);
        var expectedRemainingSegments = expected.GetProperty("remaining_segments");
        Assert.Equal(
            SegmentPairs(expectedRemainingSegments.GetProperty("solid")),
            SegmentPairs(remainingLines.Idle, remainingLines.Solid));
        Assert.Equal(
            SegmentPairs(expectedRemainingSegments.GetProperty("dashed")),
            SegmentPairs(remainingLines.Dashed));
        var labels = GraphPlotProjection.BuildEndpointLabels(scene, CultureInfo.InvariantCulture);
        Assert.Contains(labels, label =>
            label.Series == GraphSeries.Sol &&
            label.Text == expected.GetProperty("latest_labels").GetProperty("SOL").GetString());
        Assert.Contains(labels, label =>
            label.Series == GraphSeries.Remaining &&
            label.Text == expected.GetProperty("latest_labels").GetProperty("remaining").GetString());
    }

    [Fact]
    public async Task Issue258_recovered_v3_history_keeps_exact_components_and_graph_endpoints()
    {
        const string periodId = "issue-258-current";
        var periodsBody = Encoding.UTF8.GetBytes(
            """
            {"api_version":"v3","history_periods":[{"id":"issue-258-current","start_at":1788832680,"end_at":1788996000,"reset_at":1789437490,"label":"Current period","current":true}]}
            """);
        var historyBody = Encoding.UTF8.GetBytes(
            """
            {"api_version":"v3","history_samples":[{"timestamp":1788975600,"reset_at":1789437490,"remaining_percent":29.0,"models":[{"model":"LUNA","total_tokens":22907995,"input_tokens":22428188,"cached_input_tokens":20141824,"cache_write_input_tokens":0,"output_tokens":479807,"total_dollars":1.43587768},{"model":"SOL","total_tokens":555312427,"input_tokens":553537987,"cached_input_tokens":544468480,"cache_write_input_tokens":0,"output_tokens":1774440,"total_dollars":370.814975}],"models_complete":false,"model_source":"legacy-unknown"},{"timestamp":1788996000,"reset_at":1789437490,"remaining_percent":9.0,"models":[{"model":"LUNA","total_tokens":25262756,"input_tokens":24726033,"cached_input_tokens":22103552,"cache_write_input_tokens":0,"output_tokens":536723,"total_dollars":1.61063484},{"model":"SOL","total_tokens":555312427,"input_tokens":553537987,"cached_input_tokens":544468480,"cache_write_input_tokens":0,"output_tokens":1774440,"total_dollars":370.814975}],"models_complete":false,"model_source":"legacy-unknown"}],"history_gaps":[],"next_cursor":null,"resume_cursor":"issue-258-end"}
            """);
        var handler = new SplitHistoryFixtureHandler(
            periodsBody,
            historyBody,
            CanonicalPublishedPair,
            periodId);
        using var client = new LoopbackStatusClient(handler);

        var periodsResult = await client.FetchHistoryPeriodsAsync(CancellationToken.None);
        var pageResult = await client.FetchHistoryPageAsync(
            periodId,
            cancellationToken: CancellationToken.None);

        Assert.True(periodsResult.IsSuccess);
        Assert.True(pageResult.IsSuccess);
        var periods = Assert.IsType<ApiHistoryPeriodsSnapshot>(periodsResult.Snapshot);
        var page = Assert.IsType<ApiHistoryPage>(pageResult.Page);
        Assert.Equal(periods.PublishedPair, page.PublishedPair);
        var period = Assert.Single(periods.Periods) with { Samples = page.Samples };
        var endpoint = period.Samples[^1];
        var sol = endpoint.Models.Single(model => model.Name == "SOL");
        var luna = endpoint.Models.Single(model => model.Name == "LUNA");
        Assert.Equal(555_312_427UL, sol.TotalTokens);
        Assert.Equal(553_537_987UL, sol.InputTokens);
        Assert.Equal(544_468_480UL, sol.CachedInputTokens);
        Assert.Equal(1_774_440UL, sol.OutputTokens);
        Assert.Equal(370.814975, sol.TotalDollars!.Value);
        Assert.Equal(25_262_756UL, luna.TotalTokens);
        Assert.Equal(24_726_033UL, luna.InputTokens);
        Assert.Equal(22_103_552UL, luna.CachedInputTokens);
        Assert.Equal(536_723UL, luna.OutputTokens);
        Assert.Equal(1.61063484, luna.TotalDollars!.Value);
        Assert.Equal(
            372.42560984,
            sol.TotalDollars.Value + luna.TotalDollars.Value,
            precision: 8);

        var graphSamples = GraphWindowViewModel.BuildGraphSamples(period, period.EndAt);
        var dollars = GraphScene.Create(
            graphSamples,
            GraphMetric.Dollars,
            period.StartAt,
            period.EndAt);
        var dollarLabels = GraphPlotProjection.BuildEndpointLabels(
            dollars,
            CultureInfo.InvariantCulture);
        Assert.Contains(dollarLabels, label =>
            label.Series == GraphSeries.Sol && label.Text == "$370.81");
        Assert.Contains(dollarLabels, label =>
            label.Series == GraphSeries.Luna && label.Text == "$1.61");
        Assert.Equal(370.814975, dollars.ModelSeries["SOL"][^1], precision: 8);
        Assert.Equal(1.61063484, dollars.ModelSeries["LUNA"][^1], precision: 8);

        var tokens = GraphScene.Create(
            graphSamples,
            GraphMetric.Tokens,
            period.StartAt,
            period.EndAt);
        Assert.Equal(555_312_427d, tokens.ModelSeries["SOL"][^1]);
        Assert.Equal(25_262_756d, tokens.ModelSeries["LUNA"][^1]);
        Assert.NotEqual(0d, tokens.ModelSeries["SOL"][^1]);
    }

    [Fact]
    public async Task Issue137_live_evidence_exports_windows_production_projection()
    {
        var evidencePath = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_LIVE_EVIDENCE");
        var outputPath = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_ACTUAL_OUTPUT");
        var imagePath = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_IMAGE_OUTPUT");
        var sourceSha = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_SOURCE_SHA");
        var repositoryRoot = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_REPOSITORY_ROOT");
        if (evidencePath is null && outputPath is null && imagePath is null && sourceSha is null && repositoryRoot is null)
        {
            return;
        }

        Assert.False(string.IsNullOrWhiteSpace(evidencePath));
        Assert.False(string.IsNullOrWhiteSpace(outputPath));
        Assert.False(string.IsNullOrWhiteSpace(sourceSha));
        Assert.False(string.IsNullOrWhiteSpace(repositoryRoot));
        var fullEvidencePath = Path.GetFullPath(evidencePath!);
        var fullOutputPath = Path.GetFullPath(outputPath!);
        var fullImagePath = imagePath is null ? null : Path.GetFullPath(imagePath);
        var fullRepositoryRoot = Path.GetFullPath(repositoryRoot!);
        Assert.True(Path.IsPathFullyQualified(evidencePath));
        Assert.True(Path.IsPathFullyQualified(outputPath));
        Assert.True(imagePath is null || Path.IsPathFullyQualified(imagePath));
        Assert.True(Path.IsPathFullyQualified(repositoryRoot));
        Assert.True(File.Exists(fullEvidencePath));
        Assert.False(File.Exists(fullOutputPath));
        Assert.True(fullImagePath is null || !File.Exists(fullImagePath));
        Assert.True(Directory.Exists(Path.GetDirectoryName(fullOutputPath)));
        Assert.True(fullImagePath is null || Directory.Exists(Path.GetDirectoryName(fullImagePath)));
        Assert.False(PathIsInside(fullEvidencePath, fullRepositoryRoot));
        Assert.False(PathIsInside(fullOutputPath, fullRepositoryRoot));
        Assert.True(fullImagePath is null || !PathIsInside(fullImagePath, fullRepositoryRoot));

        using var artifact = JsonDocument.Parse(File.ReadAllText(fullEvidencePath));
        var root = artifact.RootElement;
        Assert.Equal("graph-evidence-v1", root.GetProperty("schema_version").GetString());
        Assert.Equal(sourceSha, root.GetProperty("source_sha").GetString());
        var fixture = root.GetProperty("fixture");
        var pair = root.GetProperty("published_pair").GetString()!;
        Assert.Equal(pair, fixture.GetProperty("published_pair").GetString());
        var periodElement = fixture.GetProperty("period");
        var periodBody = Encoding.UTF8.GetBytes(
            "{\"api_version\":\"v3\",\"history_periods\":[" +
            periodElement.GetRawText() +
            "]}");
        var historyBody = Encoding.UTF8.GetBytes(fixture.GetProperty("history_page").GetRawText());
        var periodId = periodElement.GetProperty("id").GetString()!;
        var handler = new SplitHistoryFixtureHandler(periodBody, historyBody, pair, periodId);
        using var client = new LoopbackStatusClient(handler);
        var periodsResult = await client.FetchHistoryPeriodsAsync(CancellationToken.None);
        var pageResult = await client.FetchHistoryPageAsync(periodId, cancellationToken: CancellationToken.None);
        Assert.True(periodsResult.IsSuccess);
        Assert.True(pageResult.IsSuccess);
        Assert.Equal(2, handler.RequestCount);
        var periods = Assert.IsType<ApiHistoryPeriodsSnapshot>(periodsResult.Snapshot);
        var page = Assert.IsType<ApiHistoryPage>(pageResult.Page);
        Assert.Equal(periods.PublishedPair, page.PublishedPair);
        Assert.Equal(pair, periods.PublishedPair.ToString());
        var parsedPeriod = Assert.Single(periods.Periods, candidate => candidate.Id == periodId);
        var period = parsedPeriod with { Samples = page.Samples };
        var samples = GraphWindowViewModel.BuildGraphSamples(period, period.EndAt);
        var gaps = page.HistoryGaps
            .Select(gap => new GraphConfirmedGap(gap.StartAt, gap.EndAt))
            .ToArray();

        var actualSegments = new List<LiveGraphSegment>();
        GraphScene? dollarScene = null;
        GraphScene? tokenScene = null;
        foreach (var (metric, name) in new[]
        {
            (GraphMetric.Dollars, "dollars"),
            (GraphMetric.Tokens, "tokens"),
        })
        {
            var scene = GraphScene.Create(samples, metric, period.StartAt, period.EndAt, gaps);
            if (metric == GraphMetric.Dollars)
            {
                dollarScene = scene;
            }
            else
            {
                tokenScene = scene;
            }

            foreach (var model in scene.ModelSeries.Where(pair => IsRenderableModel(pair.Key)))
            {
                var lines = GraphPlotProjection.BuildModelLines(scene, model.Value);
                AddLiveSegments(actualSegments, name, model.Key, "idle", lines.Idle);
                AddLiveSegments(actualSegments, name, model.Key, "flat", lines.Flat);
                AddLiveSegments(actualSegments, name, model.Key, "rising", lines.Rising);
                AddLiveSegments(actualSegments, name, model.Key, "dashed", lines.Dashed);
            }
        }

        Assert.NotNull(dollarScene);
        Assert.NotNull(tokenScene);
        Assert.Equal(dollarScene.IdleIntervals, tokenScene.IdleIntervals);
        var remaining = GraphPlotProjection.BuildRemainingLines(tokenScene);
        AddLiveSegments(actualSegments, "remaining", "remaining", "idle", remaining.Idle);
        AddLiveSegments(actualSegments, "remaining", "remaining", "solid", remaining.Solid);
        AddLiveSegments(actualSegments, "remaining", "remaining", "dashed", remaining.Dashed);
        var orderedActual = OrderLiveSegments(actualSegments);
        var expectedSegments = OrderLiveSegments(
            root.GetProperty("expected_segments")
                .EnumerateArray()
                .Select(segment => new LiveGraphSegment(
                    segment.GetProperty("metric").GetString()!,
                    segment.GetProperty("series").GetString()!,
                    segment.GetProperty("start_at").GetInt64(),
                    segment.GetProperty("end_at").GetInt64(),
                    segment.GetProperty("style").GetString()!)));
        Assert.Equal(expectedSegments, orderedActual);

        var actualIdle = tokenScene.IdleIntervals
            .Select(interval => new LiveIdleInterval(interval.StartAt, interval.EndAt))
            .OrderBy(interval => interval.StartAt)
            .ThenBy(interval => interval.EndAt)
            .ToArray();
        var expectedIdle = root.GetProperty("expected_idle_intervals")
            .EnumerateArray()
            .Select(interval => new LiveIdleInterval(
                interval.GetProperty("start_at").GetInt64(),
                interval.GetProperty("end_at").GetInt64()))
            .OrderBy(interval => interval.StartAt)
            .ThenBy(interval => interval.EndAt)
            .ToArray();
        Assert.Equal(expectedIdle, actualIdle);

        var actual = new LiveActualDocument(
            "graph-actual-v1",
            sourceSha!,
            root.GetProperty("input_sha256").GetString()!,
            pair,
            "windows",
            orderedActual,
            actualIdle,
            new Dictionary<string, LiveRenderContract>(StringComparer.Ordinal)
            {
                ["dollars"] = BuildLiveRenderContract(dollarScene),
                ["tokens"] = BuildLiveRenderContract(tokenScene),
            });
        using var output = new FileStream(fullOutputPath, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        JsonSerializer.Serialize(output, actual, new JsonSerializerOptions
        {
            PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        });
        if (fullImagePath is not null)
        {
            var imageControl = new GraphPlotControl { Scene = dollarScene };
            imageControl.Plot.SavePng(fullImagePath, 940, 480);
        }
    }

    [Fact]
    public async Task Issue137_live_evidence_exports_windows_idle_projection()
    {
        var evidencePath = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_IDLE_LIVE_EVIDENCE");
        var outputPath = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_IDLE_ACTUAL_OUTPUT");
        var sourceSha = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_SOURCE_SHA");
        var repositoryRoot = Environment.GetEnvironmentVariable("CODEX_INFO_GRAPH_REPOSITORY_ROOT");
        if (evidencePath is null && outputPath is null && sourceSha is null && repositoryRoot is null)
        {
            return;
        }

        Assert.False(string.IsNullOrWhiteSpace(evidencePath));
        Assert.False(string.IsNullOrWhiteSpace(outputPath));
        Assert.False(string.IsNullOrWhiteSpace(sourceSha));
        Assert.False(string.IsNullOrWhiteSpace(repositoryRoot));
        var fullEvidencePath = Path.GetFullPath(evidencePath!);
        var fullOutputPath = Path.GetFullPath(outputPath!);
        var fullRepositoryRoot = Path.GetFullPath(repositoryRoot!);
        Assert.True(Path.IsPathFullyQualified(evidencePath));
        Assert.True(Path.IsPathFullyQualified(outputPath));
        Assert.True(Path.IsPathFullyQualified(repositoryRoot));
        Assert.True(File.Exists(fullEvidencePath));
        Assert.False(File.Exists(fullOutputPath));
        Assert.True(Directory.Exists(Path.GetDirectoryName(fullOutputPath)));
        Assert.False(PathIsInside(fullEvidencePath, fullRepositoryRoot));
        Assert.False(PathIsInside(fullOutputPath, fullRepositoryRoot));

        using var artifact = JsonDocument.Parse(File.ReadAllText(fullEvidencePath));
        var root = artifact.RootElement;
        Assert.Equal("graph-evidence-v1", root.GetProperty("schema_version").GetString());
        var fixture = root.GetProperty("fixture");
        var pair = root.GetProperty("published_pair").GetString()!;
        Assert.Equal(pair, fixture.GetProperty("published_pair").GetString());
        var periodElement = fixture.GetProperty("period");
        var periodBody = Encoding.UTF8.GetBytes(
            "{\"api_version\":\"v3\",\"history_periods\":[" +
            periodElement.GetRawText() +
            "]}");
        var historyBody = Encoding.UTF8.GetBytes(fixture.GetProperty("history_page").GetRawText());
        var periodId = periodElement.GetProperty("id").GetString()!;
        var handler = new SplitHistoryFixtureHandler(periodBody, historyBody, pair, periodId);
        using var client = new LoopbackStatusClient(handler);
        var periodsResult = await client.FetchHistoryPeriodsAsync(CancellationToken.None);
        var pageResult = await client.FetchHistoryPageAsync(periodId, cancellationToken: CancellationToken.None);
        Assert.True(periodsResult.IsSuccess);
        Assert.True(pageResult.IsSuccess);
        var periods = Assert.IsType<ApiHistoryPeriodsSnapshot>(periodsResult.Snapshot);
        var page = Assert.IsType<ApiHistoryPage>(pageResult.Page);
        Assert.Equal(periods.PublishedPair, page.PublishedPair);
        var parsedPeriod = Assert.Single(periods.Periods, candidate => candidate.Id == periodId);
        var period = parsedPeriod with { Samples = page.Samples };
        var samples = GraphWindowViewModel.BuildGraphSamples(period, period.EndAt);
        var gaps = page.HistoryGaps
            .Select(gap => new GraphConfirmedGap(gap.StartAt, gap.EndAt))
            .ToArray();
        var dollarScene = GraphScene.Create(samples, GraphMetric.Dollars, period.StartAt, period.EndAt, gaps);
        var tokenScene = GraphScene.Create(samples, GraphMetric.Tokens, period.StartAt, period.EndAt, gaps);
        Assert.Equal(dollarScene.IdleIntervals, tokenScene.IdleIntervals);

        var actualIdle = tokenScene.IdleIntervals
            .Select(interval => new LiveIdleInterval(interval.StartAt, interval.EndAt))
            .OrderBy(interval => interval.StartAt)
            .ThenBy(interval => interval.EndAt)
            .ToArray();
        var expectedIdle = root.GetProperty("expected_idle_intervals")
            .EnumerateArray()
            .Select(interval => new LiveIdleInterval(
                interval.GetProperty("start_at").GetInt64(),
                interval.GetProperty("end_at").GetInt64()))
            .OrderBy(interval => interval.StartAt)
            .ThenBy(interval => interval.EndAt)
            .ToArray();
        Assert.Equal(expectedIdle, actualIdle);

        var document = new Dictionary<string, object?>
        {
            ["schema_version"] = "graph-idle-actual-v1",
            ["source_sha"] = sourceSha,
            ["input_source_sha"] = root.GetProperty("source_sha").GetString(),
            ["input_sha256"] = root.GetProperty("input_sha256").GetString(),
            ["account_id"] = root.GetProperty("account_id").GetString(),
            ["period"] = periodId,
            ["published_pair"] = pair,
            ["platform"] = "windows",
            ["idle_intervals"] = actualIdle,
        };
        File.WriteAllText(fullOutputPath, JsonSerializer.Serialize(document, new JsonSerializerOptions
        {
            PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
            WriteIndented = true,
        }));
    }

    [Fact]
    public async Task Issue137_continuity_v4_oracle_is_identical_for_dollars_tokens_and_exact_period_end()
    {
        using var document = JsonDocument.Parse(File.ReadAllText(Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_evidence_oracle.json")));
        var fixture = document.RootElement.GetProperty("continuous_idle_v4");
        var expected = fixture.GetProperty("expected");
        var pair = fixture.GetProperty("published_pair").GetString()!;
        var periodElement = fixture.GetProperty("period");
        var periodId = periodElement.GetProperty("id").GetString()!;
        var periodsBody = Encoding.UTF8.GetBytes(
            "{\"api_version\":\"v3\",\"history_periods\":[" +
            periodElement.GetRawText() +
            "]}");
        var historyBody = Encoding.UTF8.GetBytes(
            fixture.GetProperty("history_page").GetRawText());
        var handler = new SplitHistoryFixtureHandler(periodsBody, historyBody, pair, periodId);
        using var client = new LoopbackStatusClient(handler);

        var periodsResult = await client.FetchHistoryPeriodsAsync(CancellationToken.None);
        var pageResult = await client.FetchHistoryPageAsync(
            periodId,
            cancellationToken: CancellationToken.None);

        Assert.True(periodsResult.IsSuccess);
        Assert.True(pageResult.IsSuccess);
        Assert.Equal(2, handler.RequestCount);
        var periods = Assert.IsType<ApiHistoryPeriodsSnapshot>(periodsResult.Snapshot);
        var page = Assert.IsType<ApiHistoryPage>(pageResult.Page);
        Assert.Equal(pair, periods.PublishedPair.ToString());
        Assert.Equal(periods.PublishedPair, page.PublishedPair);
        var parsedPeriod = Assert.Single(periods.Periods);
        var period = parsedPeriod with { Samples = page.Samples };
        var origin = period.StartAt;
        var samples = GraphWindowViewModel.BuildGraphSamples(period, period.EndAt);
        var expectedTimestamps = expected.GetProperty("timestamps")
            .EnumerateArray()
            .Select(value => value.GetInt64())
            .ToArray();
        Assert.Equal(expectedTimestamps, samples.Select(sample => sample.Timestamp - origin));
        Assert.Equal(period.EndAt, samples[^1].Timestamp);
        Assert.True(samples[^1].IsSyntheticTail);
        var expectedUniverse = expected.GetProperty("model_universe")
            .EnumerateArray()
            .Select(value => value.GetString())
            .ToArray();
        var expectedIdle = expected.GetProperty("idle_intervals")
            .EnumerateArray()
            .Select(interval => new GraphIdleInterval(
                interval[0].GetInt64(),
                interval[1].GetInt64(),
                PreserveBoundary: false))
            .ToArray();

        GraphScene? dollarScene = null;
        GraphScene? tokenScene = null;
        foreach (var metric in new[] { GraphMetric.Dollars, GraphMetric.Tokens })
        {
            var scene = GraphScene.Create(
                samples,
                metric,
                period.StartAt,
                period.EndAt,
                Array.Empty<GraphConfirmedGap>());
            if (metric == GraphMetric.Dollars)
            {
                dollarScene = scene;
            }
            else
            {
                tokenScene = scene;
            }

            Assert.Equal(period.StartAt, scene.PeriodStartAt);
            Assert.Equal(period.EndAt, scene.PeriodEndAt);
            Assert.Equal(expectedTimestamps, scene.Timestamps.Select(value => (long)value - origin));
            Assert.Equal(expectedUniverse, scene.ModelSeries.Keys);
            Assert.Equal(
                expected.GetProperty("correction_starts").EnumerateArray().Select(value => value.GetInt64()),
                scene.CorrectionStarts.Order().Select(timestamp => timestamp - origin));
            var expectedSeries = expected.GetProperty(
                metric == GraphMetric.Dollars ? "dollar_series" : "token_series");
            foreach (var model in expectedUniverse)
            {
                Assert.Equal(
                    expectedSeries.GetProperty(model!).EnumerateArray().Select(value =>
                        value.ValueKind == JsonValueKind.Null ? double.NaN : value.GetDouble()),
                    scene.ModelSeries[model!]);
                Assert.True(scene.ModelSeries[model!]
                    .Where(double.IsFinite)
                    .Zip(scene.ModelSeries[model!].Where(double.IsFinite).Skip(1))
                    .All(pair => pair.Second >= pair.First));

                var lines = GraphPlotProjection.BuildModelLines(scene, scene.ModelSeries[model!]);
                var expectedSegments = expected.GetProperty("model_segments").GetProperty(model!);
                Assert.Equal(
                    SegmentPairs(expectedSegments.GetProperty("flat")),
                    RelativeSegmentPairs(origin, lines.Idle, lines.Flat));
                Assert.Equal(
                    SegmentPairs(expectedSegments.GetProperty("rising")),
                    RelativeSegmentPairs(origin, lines.Rising));
                Assert.Equal(
                    SegmentPairs(expectedSegments.GetProperty("dashed")),
                    RelativeSegmentPairs(origin, lines.Dashed));
            }

            Assert.Equal(
                expectedIdle,
                scene.IdleIntervals.Select(interval => new GraphIdleInterval(
                    interval.StartAt - origin,
                    interval.EndAt - origin,
                    interval.PreserveBoundary)));
            Assert.Equal(
                expected.GetProperty("remaining_values").EnumerateArray().Select(value => value.GetDouble()),
                scene.Remaining);
            Assert.Equal(
                expected.GetProperty("remaining_origins").EnumerateArray().Select(value => value.GetString()),
                scene.RemainingOrigins.Select((value, index) => RemainingOriginName(scene, value, index)));
            var remaining = GraphPlotProjection.BuildRemainingLines(scene);
            Assert.Equal(
                SegmentPairs(expected.GetProperty("remaining_segments").GetProperty("solid")),
                RelativeSegmentPairs(origin, remaining.Idle, remaining.Solid));
            Assert.Equal(
                SegmentPairs(expected.GetProperty("remaining_segments").GetProperty("dashed")),
                RelativeSegmentPairs(origin, remaining.Dashed));
        }

        Assert.NotNull(dollarScene);
        Assert.NotNull(tokenScene);
        Assert.Equal(dollarScene.IdleIntervals, tokenScene.IdleIntervals);
        Assert.Equal(dollarScene.Remaining, tokenScene.Remaining);
        foreach (var (labelScene, labelProperty) in new[]
                 {
                     (dollarScene, "latest_labels"),
                     (tokenScene, "latest_token_labels"),
                 })
        {
            var labels = GraphPlotProjection.BuildEndpointLabels(labelScene, CultureInfo.InvariantCulture);
            foreach (var (name, series) in new[]
                     {
                         ("ASTRA", GraphSeries.Astra),
                         ("LUNA", GraphSeries.Luna),
                         ("SOL", GraphSeries.Sol),
                         ("TERRA", GraphSeries.Terra),
                     })
            {
                Assert.Contains(labels, label =>
                    label.Series == series &&
                    label.Text == expected.GetProperty(labelProperty).GetProperty(name).GetString());
            }
        }
    }

    [Fact]
    public void Issue137_idle_and_anomaly_counterexamples_match_the_shared_oracle()
    {
        using var document = JsonDocument.Parse(File.ReadAllText(Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_evidence_oracle.json")));
        foreach (var property in document.RootElement
                     .GetProperty("idle_counterexamples_v4")
                     .EnumerateObject())
        {
            var fixture = property.Value;
            var samples = SimpleOracleSamples(fixture);
            var start = samples[0].Timestamp;
            var end = samples[^1].Timestamp;
            var gaps = fixture.TryGetProperty("confirmed_gaps", out var gapElement)
                ? gapElement.EnumerateArray()
                    .Select(interval => new GraphConfirmedGap(
                        interval[0].GetInt64(),
                        interval[1].GetInt64()))
                    .ToArray()
                : Array.Empty<GraphConfirmedGap>();
            var expectedIdle = fixture.GetProperty("idle_intervals")
                .EnumerateArray()
                .Select(interval => new GraphIdleInterval(
                    interval[0].GetInt64(),
                    interval[1].GetInt64(),
                    PreserveBoundary: false))
                .ToArray();

            foreach (var metric in new[] { GraphMetric.Dollars, GraphMetric.Tokens })
            {
                var scene = GraphScene.Create(samples, metric, start, end, gaps);
                Assert.Equal(expectedIdle, scene.IdleIntervals);
                var maximumProperty = metric == GraphMetric.Dollars
                    ? "dollar_maximum"
                    : "token_maximum";
                if (fixture.TryGetProperty(maximumProperty, out var expectedMaximum))
                {
                    Assert.Equal(expectedMaximum.GetDouble(), scene.ModelMaximum, precision: 12);
                }
                var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
                var prefix = metric == GraphMetric.Dollars ? "dollar" : "token";
                if (fixture.TryGetProperty(prefix + "_flat", out var expectedFlat))
                {
                    Assert.Equal(SegmentPairs(expectedFlat), SegmentPairs(model.Idle, model.Flat));
                }
                if (fixture.TryGetProperty(prefix + "_rising", out var expectedRising))
                {
                    Assert.Equal(SegmentPairs(expectedRising), SegmentPairs(model.Rising));
                }
                if (fixture.TryGetProperty(prefix + "_dashed", out var expectedDashed))
                {
                    Assert.Equal(SegmentPairs(expectedDashed), SegmentPairs(model.Dashed));
                }
                if (fixture.TryGetProperty("remaining_dashed", out var expectedRemaining))
                {
                    Assert.Equal(
                        SegmentPairs(expectedRemaining),
                        SegmentPairs(GraphPlotProjection.BuildRemainingLines(scene).Dashed));
                }
                if (fixture.TryGetProperty("remaining_values", out var expectedRemainingValues))
                {
                    Assert.Equal(
                        expectedRemainingValues.EnumerateArray().Select(value => value.GetDouble()),
                        scene.Remaining);
                }
            }
        }
    }

    [Fact]
    public void Issue134_valid_anchor_projection_is_token_and_activity_independent()
    {
        using var document = JsonDocument.Parse(File.ReadAllText(Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_evidence_oracle.json")));
        foreach (var property in document.RootElement
                     .GetProperty("valid_anchor_projection_v5")
                     .EnumerateObject())
        {
            var fixture = property.Value;
            var samples = SimpleOracleSamples(fixture);
            var gaps = fixture.TryGetProperty("confirmed_gaps", out var gapElement)
                ? gapElement.EnumerateArray()
                    .Select(interval => new GraphConfirmedGap(
                        interval[0].GetInt64(),
                        interval[1].GetInt64()))
                    .ToArray()
                : Array.Empty<GraphConfirmedGap>();
            var scene = GraphScene.Create(
                samples,
                GraphMetric.Dollars,
                samples[0].Timestamp,
                samples[^1].Timestamp,
                gaps);
            var expectedValues = fixture.GetProperty("effective")
                .EnumerateArray()
                .Select(value => value.GetDouble())
                .ToArray();
            Assert.Equal(expectedValues.Length, scene.Remaining.Count);
            for (var index = 0; index < expectedValues.Length; index++)
            {
                Assert.Equal(expectedValues[index], scene.Remaining[index], precision: 12);
            }
            Assert.Equal(
                fixture.GetProperty("remaining_origins")
                    .EnumerateArray()
                    .Select(value => value.GetString()),
                scene.RemainingOrigins.Select((origin, index) =>
                    RemainingOriginName(scene, origin, index)));
            Assert.Equal(
                fixture.GetProperty("idle_intervals").EnumerateArray().Select(interval =>
                    new GraphIdleInterval(
                        interval[0].GetInt64(),
                        interval[1].GetInt64(),
                        PreserveBoundary: false)),
                scene.IdleIntervals);
            var remainingLines = GraphPlotProjection.BuildRemainingLines(scene);
            Assert.Equal(
                SegmentPairs(fixture.GetProperty("remaining_solid")),
                SegmentPairs(remainingLines.Idle, remainingLines.Solid));
            Assert.Equal(
                SegmentPairs(fixture.GetProperty("remaining_dashed")),
                SegmentPairs(remainingLines.Dashed));
            Assert.All(
                SegmentPairs(remainingLines.Idle, remainingLines.Solid, remainingLines.Dashed),
                segment => Assert.True(segment.StartAt < segment.EndAt));
            foreach (var interval in scene.IdleIntervals)
            {
                var startIndex = Array.IndexOf(scene.Timestamps.ToArray(), (double)interval.StartAt);
                var endIndex = Array.IndexOf(scene.Timestamps.ToArray(), (double)interval.EndAt);
                Assert.True(startIndex >= 0 && endIndex >= 0);
                Assert.Equal(scene.Remaining[startIndex], scene.Remaining[endIndex], precision: 12);
            }
        }
    }

    [Fact]
    public async Task Issue137_cumulative_anomaly_fixture_never_paints_a_vertical_drop()
    {
        var fixturePath = Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_evidence_oracle.json");
        using var document = JsonDocument.Parse(File.ReadAllText(fixturePath));
        var root = document.RootElement.GetProperty("correction_v2");
        var expected = root.GetProperty("expected");
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
            sample =>
            {
                Assert.Equal(ApiHistorySample.LegacyUnknownModelSource, sample.ModelSource);
                Assert.False(sample.ModelsComplete);
            });

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
        // not manufacture quota or turn the rejected regression into a drop.
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
        Assert.Contains(false, scene.ModelVectorAvailable);
        Assert.Equal(
            expected.GetProperty("remaining_values").EnumerateArray().Select(value => value.GetDouble()),
            scene.Remaining);
        Assert.Equal(
            expected.GetProperty("accepted_sol").EnumerateArray().Select(value => value.GetDouble()),
            scene.Sol);
        Assert.Equal(expectedCorrectionStarts, scene.CorrectionStarts.Order());
        Assert.Equal(
            expected.GetProperty("idle_intervals").EnumerateArray().Select(interval =>
                new GraphIdleInterval(
                    interval[0].GetInt64(),
                    interval[1].GetInt64(),
                    PreserveBoundary: false)),
            scene.IdleIntervals);

        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Equal(
            SegmentPairs(expected.GetProperty("model_segments").GetProperty("SOL").GetProperty("solid")),
            SegmentPairs(model.Idle, model.Flat, model.Rising));
        Assert.Equal(
            SegmentPairs(expected.GetProperty("model_segments").GetProperty("SOL").GetProperty("dashed")),
            SegmentPairs(model.Dashed));
        Assert.Equal(
            SegmentPairs(expected.GetProperty("remaining_segments").GetProperty("solid")),
            SegmentPairs(remaining.Idle, remaining.Solid));
        Assert.Equal(
            SegmentPairs(expected.GetProperty("remaining_segments").GetProperty("dashed")),
            SegmentPairs(remaining.Dashed));
        Assert.NotEmpty(remaining.Dashed.X);
        var acceptedLatestSol = expected.GetProperty("accepted_sol").EnumerateArray().Last().GetDouble();
        Assert.Equal(acceptedLatestSol, scene.Sol[^2], precision: 6);
        Assert.Equal(acceptedLatestSol, scene.Sol[^1], precision: 6);
        var labels = GraphPlotProjection.BuildEndpointLabels(scene, CultureInfo.InvariantCulture);
        Assert.Contains(labels, label =>
            label.Series == GraphSeries.Sol &&
            label.Text == expected.GetProperty("latest_labels").GetProperty("SOL").GetString());
        Assert.Contains(labels, label =>
            label.Series == GraphSeries.Remaining &&
            label.Text == expected.GetProperty("latest_labels").GetProperty("remaining").GetString());
    }

    [Fact]
    public void Issue137_shared_oracle_matches_regression_and_quota_anomaly_rules()
    {
        static ApiHistorySample[] ParseSamples(JsonElement fixture, long resetAt) =>
            fixture.GetProperty("samples").EnumerateArray().Select(sample =>
            {
                var models = sample.GetProperty("models").EnumerateArray().Select(model =>
                {
                    static ulong? OptionalUInt64(JsonElement value, string name) =>
                        value.TryGetProperty(name, out var property) ? property.GetUInt64() : null;

                    return new ApiHistoryModelSample(
                        model.GetProperty("model").GetString()!,
                        OptionalUInt64(model, "input_tokens"),
                        OptionalUInt64(model, "cached_input_tokens"),
                        OptionalUInt64(model, "output_tokens"),
                        model.GetProperty("total_dollars").GetDouble())
                    {
                        CacheWriteInputTokens = OptionalUInt64(model, "cache_write_input_tokens"),
                        TotalTokens = model.GetProperty("total_tokens").GetUInt64(),
                    };
                }).ToArray();
                return new ApiHistorySample(
                    sample.GetProperty("timestamp").GetInt64(),
                    resetAt,
                    sample.GetProperty("remaining_percent").GetDouble(),
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    sample.GetProperty("model_source").GetString()!)
                {
                    ModelsComplete = sample.GetProperty("models_complete").GetBoolean(),
                    ModelSamples = models,
                };
            }).ToArray();

        using var document = JsonDocument.Parse(File.ReadAllText(Path.Combine(
            AppContext.BaseDirectory,
            "Fixtures",
            "graph_evidence_oracle.json")));
        foreach (var fixtureName in new[] { "mixed_unconfirmed_v3", "mixed_confirmed_v3" })
        {
            var fixture = document.RootElement.GetProperty(fixtureName);
            var expected = fixture.GetProperty("expected");
            var samples = ParseSamples(fixture, 10_000);
            var scene = GraphScene.Create(
                samples,
                GraphMetric.Dollars,
                samples[0].Timestamp,
                samples[^1].Timestamp);
            Assert.Equal(
                expected.GetProperty("accepted_sol").EnumerateArray().Select(value =>
                    value.ValueKind == JsonValueKind.Null ? double.NaN : value.GetDouble()),
                scene.Sol);
            Assert.Equal(
                expected.GetProperty("corrections").EnumerateArray().Select(value => value.GetInt64()),
                scene.CorrectionStarts.Order());
            var lines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
            Assert.Equal(
                SegmentPairs(expected.GetProperty("sol_solid")),
                SegmentPairs(lines.Flat, lines.Rising));
            Assert.Equal(
                SegmentPairs(expected.GetProperty("sol_dashed")),
                SegmentPairs(lines.Dashed));
            Assert.Empty(scene.IdleIntervals);
        }

        var quotaFixture = document.RootElement.GetProperty("quota_raw_increase");
        var quotaExpected = quotaFixture.GetProperty("expected");
        var quotaSamples = ParseSamples(quotaFixture, 10_000);
        var periodStart = quotaFixture.GetProperty("period_start").GetInt64();
        var periodEnd = quotaFixture.GetProperty("period_end").GetInt64();
        var period = new ApiHistoryPeriod("quota-raw-increase", periodStart, periodEnd, true, "quota")
        {
            ResetAt = 10_000,
            Samples = quotaSamples,
        };
        var graphSamples = GraphWindowViewModel.BuildGraphSamples(period, periodEnd);
        var quotaScene = GraphScene.Create(
            graphSamples,
            GraphMetric.Dollars,
            periodStart,
            periodEnd);
        Assert.Equal(
            quotaExpected.GetProperty("raw").EnumerateArray().Select(value => value.GetDouble()),
            quotaScene.ObservedRemainingValues.Take(2));
        Assert.Equal(
            quotaExpected.GetProperty("effective").EnumerateArray().Select(value => value.GetDouble()),
            quotaScene.Remaining.Take(2));
        Assert.Equal(
            quotaExpected.GetProperty("origins").EnumerateArray().Select(value => value.GetString()),
            quotaScene.RemainingOrigins.Take(2).Select((origin, index) =>
                RemainingOriginName(quotaScene, origin, index)));
        Assert.Equal(
            quotaExpected.GetProperty("latest_raw").GetDouble(),
            quotaScene.ObservedRemainingValues[^2]);
        Assert.Equal(
            quotaExpected.GetProperty("latest_effective").GetDouble(),
            quotaScene.Remaining[^1]);
        Assert.Equal(
            "synthetic_tail_hold",
            RemainingOriginName(quotaScene, quotaScene.RemainingOrigins[^1], quotaScene.RemainingOrigins.Count - 1));
        Assert.Equal(
            SegmentPairs(quotaExpected.GetProperty("remaining_dashed")),
            SegmentPairs(GraphPlotProjection.BuildRemainingLines(quotaScene).Dashed));
        Assert.Empty(quotaScene.IdleIntervals);
        Assert.Contains(
            GraphPlotProjection.BuildEndpointLabels(quotaScene, CultureInfo.InvariantCulture),
            label => label.Series == GraphSeries.Remaining &&
                     label.Text == quotaExpected.GetProperty("latest_label").GetString());
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
        Assert.Equal(firstCurrent.Id, graph.SelectedPeriod?.Id);
        Assert.Equal(firstCurrent.EndAt, graph.Scene.PeriodEndAt);
        Assert.Equal(firstCurrent.EndAt, graph.Points[^1].Timestamp);
        Assert.Null(graph.Points[^1].RemainingPercent);
        Assert.Equal(100, graph.Scene.Remaining[^1]);
        Assert.Equal("thread-a", Assert.Single(threads.Threads).Id);
        var firstEndpoint = graph.Scene.PeriodEndAt;
        var firstMaximum = graph.Scene.ModelMaximum;

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => ReferenceEquals(main.DetailsSnapshot, second));

        Assert.Equal(41, main.RemainingPercentValue);
        Assert.Equal("概算 $323.674247", main.EstimatedCostText);
        Assert.Equal(323.674247, main.DetailsSnapshot!.Models.Sum(model => model.TotalDollars), precision: 6);
        Assert.Equal(secondCurrent.Id, graph.SelectedPeriod?.Id);
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
    public void Remaining_sampling_plateaus_are_smoothed_only_in_renderer_geometry()
    {
        var points = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_600, 100, 1, 0, 0),
            Point(2_200, 99, 2, 0, 0),
            Point(2_800, 99, 3, 0, 0),
            Point(3_400, 98, 4, 0, 0),
            Point(4_000, 98, 4, 0, 0),
        };

        var scene = Scene(points);
        var effective = scene.Remaining;
        var rawLines = GraphPlotProjection.BuildRemainingLines(scene);
        var canonicalRemaining = GraphPlotProjection.BuildCanonicalRemainingLines(scene);
        var rendered = canonicalRemaining.Solid.Line;
        var renderedIdle = canonicalRemaining.Idle.Line;

        static double RenderedAt(GraphLineProjection line, double timestamp) =>
            line.X.Zip(line.Y)
                .Where(pair => double.IsFinite(pair.First) && Math.Abs(pair.First - timestamp) < 0.1)
                .Select(pair => pair.Second)
                .First();

        Assert.Equal(100d, effective[0]);
        Assert.Equal(100d, effective[1]);
        Assert.Equal(99d, effective[2]);
        Assert.Equal(99d, effective[3]);
        Assert.Equal(98d, effective[4]);
        Assert.Equal(98d, effective[5]);
        Assert.Equal([1_000d, 1_600d, 2_200d, 2_800d, 3_400d], rawLines.Solid.X);
        Assert.Equal([100d, 100d, 99d, 99d, 98d], rawLines.Solid.Y);
        Assert.Equal([3_400d, 4_000d], rawLines.Idle.X);
        Assert.Equal([98d, 98d], rawLines.Idle.Y);
        Assert.InRange(RenderedAt(rendered, 1_600), 99.000_001, 99.999_999);
        Assert.InRange(RenderedAt(rendered, 2_800), 98.000_001, 98.999_999);
        Assert.Equal(98d, RenderedAt(renderedIdle, 3_400), precision: 6);
        Assert.Equal(98d, RenderedAt(renderedIdle, 4_000), precision: 6);
        var idle = Assert.Single(scene.IdleIntervals);
        Assert.Equal(3_400, idle.StartAt);
        Assert.Equal(4_000, idle.EndAt);
        Assert.Equal(3f, GraphPlotControl.MeasuredRemainingLineWidth);
        Assert.Equal(1f, GraphPlotControl.IdleLineWidth);
        Assert.Equal(1f, GraphPlotControl.InferredLineWidth);

        var activeTailPoints = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_600, 100, 1, 0, 0),
            Point(2_200, 60, 2, 0, 0),
            Point(2_800, 60, 3, 0, 0),
            Point(3_400, 20, 4, 0, 0),
            Point(4_000, 20, 5, 0, 0),
        };
        var activeTailScene = Scene(activeTailPoints);
        var activeTailRendered = GraphPlotProjection
            .BuildCanonicalRemainingLines(activeTailScene)
            .Solid.Line;
        Assert.Empty(activeTailScene.IdleIntervals);
        Assert.InRange(RenderedAt(activeTailRendered, 3_400), 20.000_001, 59.999_999);
        Assert.Equal(20d, RenderedAt(activeTailRendered, 4_000), precision: 6);

        var activeModelPoints = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_600, 99, 0, 1, 0),
            Point(2_200, 98, 40, 2, 0),
            Point(2_800, 97, 40, 3, 0),
            Point(3_400, 96, 80, 4, 0),
            Point(4_000, 95, 80, 5, 0),
        };
        var activeModelScene = Scene(activeModelPoints);
        var activeModel = GraphPlotProjection.BuildCanonicalModelLines(
            activeModelScene,
            activeModelScene.Sol);
        Assert.Empty(activeModelScene.IdleIntervals);
        Assert.InRange(RenderedAt(activeModel.Flat.Line, 1_600), 0.000_001, 39.999_999);
        Assert.InRange(RenderedAt(activeModel.Flat.Line, 2_800), 40.000_001, 79.999_999);
        Assert.InRange(RenderedAt(activeModel.Flat.Line, 3_400), 40.000_001, 79.999_999);
        Assert.Equal(80d, RenderedAt(activeModel.Flat.Line, 4_000), precision: 6);

        var idleModelPoints = new[]
        {
            Point(1_000, 100, 0, 0, 0),
            Point(1_600, 99, 0, 1, 0),
            Point(2_200, 98, 40, 2, 0),
            Point(2_800, 97, 40, 3, 0),
            Point(3_400, 96, 80, 4, 0),
            Point(4_000, 96, 80, 4, 0),
        };
        var idleModelScene = Scene(idleModelPoints);
        var idleModel = GraphPlotProjection.BuildCanonicalModelLines(
            idleModelScene,
            idleModelScene.Sol);
        Assert.Single(idleModelScene.IdleIntervals);
        Assert.Equal(80d, RenderedAt(idleModel.Idle.Line, 3_400), precision: 6);
        Assert.Equal(80d, RenderedAt(idleModel.Idle.Line, 4_000), precision: 6);
    }

    [Fact]
    public void Reconstructed_model_points_are_discarded_and_cannot_confirm_idle()
    {
        var samples = Enumerable.Range(0, 31)
            .Select(minute => CompleteModelSample(
                1_000 + minute * 60,
                90,
                1,
                10,
                ApiHistorySample.ReconstructedFromSessionModelSource))
            .ToArray();

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 2_800);

        Assert.All(scene.Sol, value => Assert.True(double.IsNaN(value)));
        Assert.DoesNotContain("SOL", scene.ModelReliability.Keys);
        Assert.DoesNotContain("SOL", scene.TokenReliability.Keys);
        Assert.All(scene.ModelVectorAvailable, Assert.False);
        Assert.Empty(scene.IdleIntervals);
        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        Assert.Empty(model.Flat.X);
        Assert.Empty(model.Rising.X);
        Assert.Empty(model.Dashed.X);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Equal(31, remaining.Solid.X.Count);
        Assert.Equal(1_000d, remaining.Solid.X[0]);
        Assert.Equal(2_800d, remaining.Solid.X[^1]);
        Assert.Empty(remaining.Dashed.X);
    }

    [Fact]
    public void Complete_model_omission_is_not_replaced_with_zero_evidence()
    {
        static ApiHistorySample WithoutSol(long timestamp) =>
            CompleteModelSample(timestamp, 90, 0, 0) with
            {
                ModelSamples =
                [
                    new ApiHistoryModelSample("LUNA", null, null, null, 0)
                    {
                        TotalTokens = 0,
                    },
                    new ApiHistoryModelSample("TERRA", null, null, null, 0)
                    {
                        TotalTokens = 0,
                    },
                ],
            };

        var samples = new[]
        {
            WithoutSol(940),
            CompleteModelSample(1_000, 90, 0, 0),
            WithoutSol(1_060),
            CompleteModelSample(1_120, 90, 10, 10),
            WithoutSol(1_180),
            CompleteModelSample(1_240, 90, 11, 11),
        };

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 940, 1_240);

        Assert.True(double.IsNaN(scene.Sol[0]));
        Assert.Equal([0d, 5d, 10d], scene.Sol.Skip(1).Take(3));
        Assert.Equal(10.5d, scene.Sol[4]);
        Assert.Equal(11d, scene.Sol[5]);
        Assert.Equal([false, true, false, true, false, true], scene.ModelReliability["SOL"]);
        Assert.Empty(scene.IdleIntervals);
    }

    [Fact]
    public void Idle_ignores_and_normalizes_a_dollar_only_anomaly()
    {
        var samples = Enumerable.Range(0, 31)
            .Select(minute => CompleteModelSample(
                1_000 + minute * 60,
                90,
                minute == 15 ? 2 : 1,
                10))
            .ToArray();

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 2_800);
        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);

        Assert.Equal(
            [new GraphIdleInterval(1_000, 2_800, false)],
            scene.IdleIntervals);
        Assert.NotEmpty(model.Idle.X);
        Assert.Empty(model.Dashed.X);
        Assert.Empty(model.Rising.X);
        Assert.All(model.Idle.Y, value => Assert.Equal(model.Idle.Y[0], value));
    }

    [Fact]
    public void Idle_never_overlaps_a_dashed_remaining_interval()
    {
        var samples = new[]
        {
            CompleteModelSample(1_000, 90, 1, 10),
            CompleteModelSample(1_060, 95, 1, 10),
            CompleteModelSample(1_120, 90, 1, 10),
        };

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_120);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Equal([1_000d, 1_120d], remaining.Dashed.X);
        Assert.Empty(scene.IdleIntervals);
    }

    [Fact]
    public void Idle_recovers_only_a_single_bounded_unavailable_sampling_slot()
    {
        static ApiHistorySample Unavailable(long timestamp) =>
            new(
                timestamp,
                2_000,
                null,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.UnavailableModelSource)
            {
                ModelsComplete = false,
                TaskActiveSincePrevious = false,
                ModelSamples = null,
            };

        static ApiHistorySample Direct(long timestamp) =>
            CompleteModelSample(timestamp, 90, 100, 1);

        var singleSlot = Enumerable.Range(0, 11)
            .Select(minute => minute == 5 ? Unavailable(300) : Direct(minute * 60))
            .ToArray();
        var recovered = GraphScene.Create(singleSlot, GraphMetric.Dollars, 0, 600);
        var recoveredModel = GraphPlotProjection.BuildModelLines(recovered, recovered.Sol);
        var recoveredRemaining = GraphPlotProjection.BuildRemainingLines(recovered);

        Assert.Equal([new GraphIdleInterval(0, 600, false)], recovered.IdleIntervals);
        Assert.NotEmpty(recoveredModel.Idle.X);
        Assert.Empty(recoveredModel.Flat.X);
        Assert.Empty(recoveredModel.Dashed.X);
        Assert.NotEmpty(recoveredRemaining.Idle.X);
        Assert.Empty(recoveredRemaining.Solid.X);
        Assert.Empty(recoveredRemaining.Dashed.X);

        var repriced = Enumerable.Range(0, 11)
            .Select(minute => minute switch
            {
                5 => Unavailable(300),
                >= 6 => CompleteModelSample(minute * 60, 90, 101, 1),
                _ => Direct(minute * 60),
            })
            .ToArray();
        var repricedScene = GraphScene.Create(repriced, GraphMetric.Dollars, 0, 600);
        Assert.Equal([new GraphIdleInterval(0, 600, false)], repricedScene.IdleIntervals);
        var repricedModel = GraphPlotProjection.BuildModelLines(repricedScene, repricedScene.Sol);
        Assert.NotEmpty(repricedModel.Idle.X);
        Assert.Empty(repricedModel.Rising.X);
        Assert.Empty(repricedModel.Dashed.X);
        Assert.All(repricedModel.Idle.Y, value => Assert.Equal(repricedModel.Idle.Y[0], value));

        var changed = Enumerable.Range(0, 11)
            .Select(minute => minute switch
            {
                5 => Unavailable(300),
                >= 6 => CompleteModelSample(minute * 60, 90, 101, 2),
                _ => Direct(minute * 60),
            })
            .ToArray();
        Assert.Empty(GraphScene.Create(changed, GraphMetric.Dollars, 0, 600).IdleIntervals);

        var longGap = GraphScene.Create(
            [Direct(0), Unavailable(900), Direct(1_800)],
            GraphMetric.Dollars,
            0,
            1_800);
        Assert.Empty(longGap.IdleIntervals);
    }

    [Fact]
    public void Only_complete_measured_flat_points_form_an_idle_interval()
    {
        var samples = Enumerable.Range(0, 31)
            .Select(minute => CompleteModelSample(1_000 + minute * 60, 90, 1, 10))
            .ToArray();

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 2_800);
        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Equal([new GraphIdleInterval(1_000, 2_800, false)], scene.IdleIntervals);
        Assert.Equal(31, model.Idle.X.Count);
        Assert.Empty(model.Flat.X);
        Assert.Equal(31, remaining.Idle.X.Count);
        Assert.Empty(remaining.Solid.X);
        Assert.Empty(model.Dashed.X);
        Assert.Empty(remaining.Dashed.X);
    }

    [Fact]
    public void Saved_legacy_raw_flat_run_across_midnight_is_idle()
    {
        const long start = 1_788_879_300; // 2026-09-08 23:55 JST
        static ApiHistoryModelSample Model(string name, ulong tokens, double dollars) =>
            new(name, null, null, null, dollars) { TotalTokens = tokens };

        var samples = Enumerable.Range(0, 31)
            .Select(minute => new ApiHistorySample(
                start + minute * 60,
                1_789_437_492,
                79,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
                ModelSamples =
                [
                    Model("LUNA", 8_364_408, 0.54),
                    Model("SOL", 172_318_074, minute == 15 ? 120.86 : 119.86),
                    Model("TERRA", 0, 0),
                ],
            })
            .ToArray();

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, start, start + 1_800);
        var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Equal(
            [new GraphIdleInterval(start, start + 1_800, false)],
            scene.IdleIntervals);
        Assert.Equal(31, model.Idle.X.Count);
        Assert.Empty(model.Flat.X);
        Assert.Empty(model.Rising.X);
        Assert.Empty(model.Dashed.X);
        Assert.All(model.Idle.Y, value => Assert.Equal(model.Idle.Y[0], value));
        Assert.All(scene.ModelReliability["SOL"], Assert.False);
        Assert.Equal(31, remaining.Idle.X.Count);
        Assert.Empty(remaining.Solid.X);
        Assert.Empty(remaining.Dashed.X);
    }

    [Fact]
    public void Lossless_source_transitions_do_not_split_idle_or_dollar_hold()
    {
        static ApiHistoryModelSample Model(double dollars) =>
            new("SOL", null, null, null, dollars) { TotalTokens = 100 };

        foreach (var legacyFirst in new[] { false, true })
        {
            var samples = Enumerable.Range(0, 11)
                .Select(minute =>
                {
                    var beforeTransition = minute <= 5;
                    var legacy = beforeTransition == legacyFirst;
                    return new ApiHistorySample(
                        minute * 60,
                        1_000,
                        90,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        legacy
                            ? ApiHistorySample.LegacyUnknownModelSource
                            : ApiHistorySample.ConfirmedModelSource)
                    {
                        ModelsComplete = !legacy,
                        TaskActiveSincePrevious = false,
                        ModelSamples = [Model(beforeTransition ? 1 : 2)],
                    };
                })
                .ToArray();

            var scene = GraphScene.Create(samples, GraphMetric.Dollars, 0, 600);
            var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
            var remaining = GraphPlotProjection.BuildRemainingLines(scene);

            Assert.Equal([new GraphIdleInterval(0, 600, false)], scene.IdleIntervals);
            Assert.NotEmpty(model.Idle.X);
            Assert.All(model.Idle.Y, value => Assert.Equal(model.Idle.Y[0], value));
            Assert.Empty(model.Flat.X);
            Assert.Empty(model.Rising.X);
            Assert.Empty(model.Dashed.X);
            Assert.NotEmpty(remaining.Idle.X);
            Assert.Empty(remaining.Solid.X);
            Assert.Empty(remaining.Dashed.X);
            for (var minute = 0; minute <= 10; minute++)
            {
                var expectedReliable = (minute <= 5) != legacyFirst;
                Assert.Equal(expectedReliable, scene.ModelReliability["SOL"][minute]);
            }
        }
    }

    [Fact]
    public void Legacy_only_model_does_not_veto_later_common_direct_idle()
    {
        static ApiHistoryModelSample Model(string name, ulong tokens, double dollars) =>
            new(name, null, null, null, dollars)
            {
                TotalTokens = tokens,
            };

        static ApiHistorySample LegacyTransition() =>
            new(
                0,
                10_000,
                90,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
                TaskActiveSincePrevious = false,
                ModelSamples =
                [
                    Model("SOL", 100, 1),
                    Model("LUNA", 200, 2),
                    Model("TERRA", 0, 0),
                ],
            };

        static ApiHistorySample Direct(long timestamp) =>
            new(
                timestamp,
                10_000,
                90,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                TaskActiveSincePrevious = false,
                ModelSamples =
                [
                    Model("SOL", 100, 1),
                    Model("LUNA", 200, 2),
                ],
            };

        var samples = new[] { LegacyTransition() }
            .Concat(Enumerable.Range(1, 31).Select(minute => Direct(minute * 60)))
            .ToArray();

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 0, 1_860);

        Assert.Equal([new GraphIdleInterval(60, 1_860, false)], scene.IdleIntervals);
    }

    [Fact]
    public void Idle_requires_identical_endpoint_model_sets()
    {
        static ApiHistoryModelSample Model(string name, ulong tokens) =>
            new(name, null, null, null, (double)tokens)
            {
                TotalTokens = tokens,
            };

        static ApiHistorySample Direct(long timestamp, params ApiHistoryModelSample[] models) =>
            new(
                timestamp,
                10_000,
                90,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                TaskActiveSincePrevious = false,
                ModelSamples = models,
            };

        var oneSided = Enumerable.Range(0, 31)
            .Select(minute => minute == 0
                ? Direct(0, Model("SOL", 100), Model("LUNA", 200))
                : Direct(minute * 60, Model("SOL", 100), Model("LUNA", 200), Model("TERRA", 300)))
            .ToArray();
        var scene = GraphScene.Create(oneSided, GraphMetric.Dollars, 0, 1_800);
        Assert.Equal([new GraphIdleInterval(60, 1_800, false)], scene.IdleIntervals);

        var disjoint = Enumerable.Range(0, 31)
            .Select(minute => Direct(
                minute * 60,
                Model(minute % 2 == 0 ? "SOL" : "LUNA", 100)))
            .ToArray();
        scene = GraphScene.Create(disjoint, GraphMetric.Dollars, 0, 1_800);
        Assert.Empty(scene.IdleIntervals);
    }

    [Fact]
    public void Idle_merge_rechecks_endpoint_common_models_at_intermediate_direct_rows()
    {
        static ApiHistoryModelSample Model(string name, ulong tokens) =>
            new(name, null, null, null, (double)tokens)
            {
                TotalTokens = tokens,
            };

        static ApiHistorySample Direct(long timestamp, params ApiHistoryModelSample[] models) =>
            new(
                timestamp,
                10_000,
                90,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                TaskActiveSincePrevious = false,
                ModelSamples = models,
            };

        var samples = Enumerable.Range(0, 59)
            .Select(minute => minute == 29
                ? Direct(minute * 60, Model("SOL", 100))
                : Direct(minute * 60, Model("SOL", 100), Model("LUNA", 200)))
            .ToArray();

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 0, 3_480);

        Assert.Equal(
            [
                new GraphIdleInterval(0, 1_680, false),
                new GraphIdleInterval(1_800, 3_480, false),
            ],
            scene.IdleIntervals);
    }

    [Fact]
    public void Missing_leading_quota_starts_at_period_start_and_keeps_first_direct_anchor()
    {
        const long periodStart = 0;
        const long resetAt = 604_800;
        var samples = new[]
        {
            new ApiHistorySample(0, resetAt, null, 1, 1, 1, 10, 10, 10),
            new ApiHistorySample(60, resetAt, null, 1, 1, 1, 20, 20, 20),
            new ApiHistorySample(120, resetAt, null, 1, 1, 1, 60, 60, 60),
            new ApiHistorySample(180, resetAt, 98, 1, 1, 1, 100, 100, 100),
        };

        var scene = GraphScene.Create(samples, GraphMetric.Tokens, periodStart, 180);

        // GraphScene retains the raw observation authority. The period-start
        // 100% point is renderer-only and must not be written into raw arrays.
        Assert.All(scene.Remaining.Take(3), value => Assert.True(double.IsNaN(value)));
        Assert.Equal(98d, scene.Remaining[^1]);
        Assert.All(scene.ObservedRemainingValues.Take(3), value => Assert.True(double.IsNaN(value)));
        Assert.Equal(98d, scene.ObservedRemainingValues[^1]);
        Assert.Equal([false, false, false, true], scene.RemainingObserved);
        Assert.Equal(
            [
                GraphRemainingOrigin.Missing,
                GraphRemainingOrigin.Missing,
                GraphRemainingOrigin.Missing,
                GraphRemainingOrigin.Raw,
            ],
            scene.RemainingOrigins);
        Assert.Equal(GraphRemainingOrigin.Raw, scene.RemainingOrigins[^1]);
        Assert.Equal(98d, scene.Remaining[^1]);
        Assert.Equal([0d, 60d, 120d, 180d], scene.Timestamps);

        // Default projection is the Issue 137/raw oracle: one raw anchor cannot
        // manufacture a segment or a leading baseline.
        var rawLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Empty(rawLines.Idle.X);
        Assert.Empty(rawLines.Solid.X);
        Assert.Empty(rawLines.Dashed.X);

        // The user-facing renderer opts into the period-start convention. Its
        // explicit mode creates only the inferred dashed T0 -> first-raw path.
        var displayLines = GraphPlotProjection.BuildCanonicalRemainingLines(
            scene,
            GraphRemainingBaselineMode.PeriodStartAtFullQuota);
        Assert.StartsWith("M0.00 1.00", displayLines.Dashed.Path);
        Assert.Equal((double)periodStart, displayLines.Dashed.Line.X[0]);
        Assert.Equal(100d, displayLines.Dashed.Line.Y[0]);
        Assert.Contains(displayLines.Dashed.Line.X, value => value > 90);
        Assert.Empty(displayLines.Solid.Line.X);
    }

    [Fact]
    public void Period_start_display_begins_at_100_percent_until_first_raw_remaining_observation()
    {
        const long periodStart = 1_000;
        const long firstObservation = 1_060;
        const long secondObservation = 1_120;
        var samples = new[]
        {
            CompleteModelSample(firstObservation, 90, 1, 10),
            CompleteModelSample(secondObservation, 80, 1, 20),
        };

        var scene = GraphScene.Create(
            samples,
            GraphMetric.Dollars,
            periodStart,
            secondObservation);

        // GraphScene retains raw timestamps and quota observations. The period
        // start is a presentation-only 100% baseline in explicit display mode.
        Assert.Equal([firstObservation, secondObservation],
            scene.Timestamps.Select(timestamp => (long)timestamp));
        Assert.Equal([90d, 80d], scene.Remaining);
        Assert.All(scene.RemainingOrigins, origin =>
            Assert.Equal(GraphRemainingOrigin.Raw, origin));

        var rawLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Equal([firstObservation, secondObservation], rawLines.Solid.X);
        Assert.Equal([90d, 80d], rawLines.Solid.Y);
        Assert.Empty(rawLines.Dashed.X);

        var displayLines = GraphPlotProjection.BuildCanonicalRemainingLines(
            scene,
            GraphRemainingBaselineMode.PeriodStartAtFullQuota);
        Assert.StartsWith("M0.00 1.00", displayLines.Dashed.Path);
        Assert.NotEmpty(displayLines.Solid.Line.X);
        Assert.Equal((double)periodStart, displayLines.Dashed.Line.X[0]);
        Assert.Equal(100d, displayLines.Dashed.Line.Y[0]);
    }

    [Fact]
    public void Period_start_equal_to_first_raw_observation_keeps_raw_point_without_synthetic_segment()
    {
        const long periodStart = 1_000;
        const long secondObservation = 1_060;
        var samples = new[]
        {
            CompleteModelSample(periodStart, 90, 1, 10),
            CompleteModelSample(secondObservation, 80, 1, 20),
        };

        var scene = GraphScene.Create(
            samples,
            GraphMetric.Dollars,
            periodStart,
            secondObservation);

        // Independent raw-observation oracle: T1 == T0 is already the first
        // accepted point, so no presentation-only 100% point is inserted.
        Assert.Equal(
            [periodStart, secondObservation],
            scene.Timestamps.Select(timestamp => (long)timestamp));
        Assert.Equal([90d, 80d], scene.Remaining);
        Assert.Equal([true, true], scene.RemainingObserved);
        Assert.Equal([90d, 80d], scene.ObservedRemainingValues);
        Assert.All(scene.RemainingOrigins, origin =>
            Assert.Equal(GraphRemainingOrigin.Raw, origin));

        var rawLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Contains(
            (periodStart, secondObservation),
            SegmentPairs(rawLines.Idle, rawLines.Solid));
        Assert.Empty(rawLines.Dashed.X);
        Assert.Empty(rawLines.Dashed.Y);

        var displayLines = GraphPlotProjection.BuildCanonicalRemainingLines(
            scene,
            GraphRemainingBaselineMode.PeriodStartAtFullQuota);
        Assert.Empty(displayLines.Dashed.Line.X);
        Assert.Empty(displayLines.Dashed.Line.Y);
        Assert.Empty(displayLines.Dashed.Path);
    }

    [Fact]
    public void Missing_leading_quota_without_accepted_raw_has_no_baseline_or_lines()
    {
        const long periodStart = 1_000;
        const long secondObservation = 1_060;
        var samples = new[]
        {
            CompleteModelSample(periodStart, null, 1, 10),
            CompleteModelSample(secondObservation, null, 1, 20),
        };

        var scene = GraphScene.Create(
            samples,
            GraphMetric.Dollars,
            periodStart,
            secondObservation);

        // Missing observations must remain missing; a display baseline or
        // inferred/raw line would invent quota data without an accepted raw.
        Assert.Equal(
            [periodStart, secondObservation],
            scene.Timestamps.Select(timestamp => (long)timestamp));
        Assert.All(scene.Remaining, value => Assert.True(double.IsNaN(value)));
        Assert.All(scene.RemainingObserved, observed => Assert.False(observed));
        Assert.All(scene.ObservedRemainingValues, value => Assert.True(double.IsNaN(value)));
        Assert.All(scene.RemainingOrigins, origin =>
            Assert.Equal(GraphRemainingOrigin.Missing, origin));

        var rawLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Empty(rawLines.Idle.X);
        Assert.Empty(rawLines.Idle.Y);
        Assert.Empty(rawLines.Solid.X);
        Assert.Empty(rawLines.Solid.Y);
        Assert.Empty(rawLines.Dashed.X);
        Assert.Empty(rawLines.Dashed.Y);

        var displayLines = GraphPlotProjection.BuildCanonicalRemainingLines(
            scene,
            GraphRemainingBaselineMode.PeriodStartAtFullQuota);
        Assert.Empty(displayLines.Idle.Line.X);
        Assert.Empty(displayLines.Idle.Line.Y);
        Assert.Empty(displayLines.Solid.Line.X);
        Assert.Empty(displayLines.Solid.Line.Y);
        Assert.Empty(displayLines.Dashed.Line.X);
        Assert.Empty(displayLines.Dashed.Line.Y);
        Assert.Empty(displayLines.Dashed.Path);
    }

    [Fact]
    public void One_adjacent_interval_is_never_a_sustained_idle_band()
    {
        static GraphScene SceneWithMarker(bool? marker) => GraphScene.Create(
            [
                CompleteModelSample(1_000, 90, 1, 10, taskActiveSincePrevious: false),
                CompleteModelSample(1_060, 90, 1, 10, taskActiveSincePrevious: marker),
            ],
            GraphMetric.Dollars,
            1_000,
            1_060);

        Assert.Empty(SceneWithMarker(false).IdleIntervals);
        Assert.Empty(SceneWithMarker(null).IdleIntervals);
        Assert.Empty(SceneWithMarker(true).IdleIntervals);
    }

    [Fact]
    public void Remaining_projection_preserves_every_valid_raw_anchor()
    {
        var samples = new[]
        {
            CompleteModelSample(1_000, 100, 1, 0),
            CompleteModelSample(1_060, 90, 1, 1),
            CompleteModelSample(1_120, 90, 1, 4),
            CompleteModelSample(1_180, 90, 1, 4),
            CompleteModelSample(1_240, 80, 1, 6),
        };

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_240);

        Assert.Equal([100d, 90d, 90d, 90d, 80d], scene.Remaining);
        Assert.All(scene.RemainingOrigins, origin => Assert.Equal(GraphRemainingOrigin.Raw, origin));
        Assert.Empty(scene.IdleIntervals);
    }

    [Fact]
    public void Remaining_projection_is_independent_of_nonuniform_and_zero_first_token_weights()
    {
        var nonuniform = new[]
        {
            CompleteModelSample(1_000, 100, 1, 0),
            CompleteModelSample(1_060, 100, 1, 10),
            CompleteModelSample(1_120, 100, 1, 20),
            CompleteModelSample(1_180, 90, 1, 100),
        };
        var nonuniformScene = GraphScene.Create(nonuniform, GraphMetric.Dollars, 1_000, 1_180);

        Assert.Equal([100d, 100d, 100d, 90d], nonuniformScene.Remaining);
        Assert.All(nonuniformScene.RemainingOrigins, origin => Assert.Equal(GraphRemainingOrigin.Raw, origin));

        var zeroFirst = new[]
        {
            CompleteModelSample(2_000, 100, 1, 0),
            CompleteModelSample(2_060, 100, 1, 0),
            CompleteModelSample(2_120, 100, 1, 50),
            CompleteModelSample(2_180, 90, 1, 100),
        };
        var zeroFirstScene = GraphScene.Create(zeroFirst, GraphMetric.Dollars, 2_000, 2_180);

        Assert.Equal([100d, 100d, 100d, 90d], zeroFirstScene.Remaining);
        Assert.All(zeroFirstScene.RemainingOrigins, origin => Assert.Equal(GraphRemainingOrigin.Raw, origin));
        Assert.Empty(zeroFirstScene.IdleIntervals);
    }

    [Fact]
    public void Exact_unchanged_values_define_idle_independent_of_task_lifecycle()
    {
        var unknown = Enumerable.Range(0, 31)
            .Select(minute => CompleteModelSample(
                1_000 + minute * 60,
                80,
                1,
                7,
                taskActiveSincePrevious: null))
            .ToArray();
        var active = unknown
            .Select((sample, index) => index == 15
                ? sample with { TaskActiveSincePrevious = true }
                : sample)
            .ToArray();

        var unknownScene = GraphScene.Create(unknown, GraphMetric.Dollars, 1_000, 2_800);
        var activeScene = GraphScene.Create(active, GraphMetric.Dollars, 1_000, 2_800);

        Assert.Equal([new GraphIdleInterval(1_000, 2_800, false)], unknownScene.IdleIntervals);
        Assert.Equal([new GraphIdleInterval(1_000, 2_800, false)], activeScene.IdleIntervals);
    }

    [Fact]
    public void Source_regression_with_a_partial_zero_vector_is_not_idle_or_a_quota_staircase()
    {
        static ApiHistorySample Complete(long timestamp, double remaining) =>
            new(
                timestamp,
                2_000,
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
                    new ApiHistoryModelSample("SOL", null, null, null, 1) { TotalTokens = 10 },
                    new ApiHistoryModelSample("LUNA", null, null, null, 2) { TotalTokens = 20 },
                    new ApiHistoryModelSample("TERRA", null, null, null, 0) { TotalTokens = 0 },
                ],
            };

        static ApiHistorySample TerraOnly(long timestamp, double remaining) =>
            new(
                timestamp,
                2_000,
                remaining,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = false,
                ModelSamples =
                [new ApiHistoryModelSample("TERRA", null, null, null, 0) { TotalTokens = 0 }],
            };

        var samples = new[]
        {
            Complete(1_000, 17),
            Complete(1_060, 17),
            TerraOnly(1_120, 17),
            TerraOnly(1_180, 17),
            TerraOnly(1_240, 17),
            Complete(1_300, 16),
        };

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_300);

        Assert.True(scene.TryGetTokenIntervalEvidence(0, 1, out var completeZeroAdvanced));
        Assert.False(completeZeroAdvanced);
        Assert.False(scene.TryGetTokenIntervalEvidence(2, 3, out _));
        Assert.Equal([true, true, false, false, false, true], scene.ModelVectorAvailable);
        Assert.Empty(scene.IdleIntervals);
        Assert.Equal(17d, scene.Remaining[0]);
        Assert.Equal(17d, scene.Remaining[1]);
        Assert.Equal(17d, scene.Remaining[2]);
        Assert.Equal(17d, scene.Remaining[3]);
        Assert.Equal(17d, scene.Remaining[4]);
        Assert.Equal(16d, scene.Remaining[5]);
        Assert.Equal(
            [
                GraphRemainingOrigin.Raw,
                GraphRemainingOrigin.Raw,
                GraphRemainingOrigin.Raw,
                GraphRemainingOrigin.Raw,
                GraphRemainingOrigin.Raw,
                GraphRemainingOrigin.Raw,
            ],
            scene.RemainingOrigins);
        var remainingLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Equal([1_000d, 1_060d, 1_120d, 1_180d, 1_240d, 1_300d], remainingLines.Solid.X);
        Assert.Empty(remainingLines.Dashed.X);
    }

    [Fact]
    public void Falling_quota_holds_a_missing_model_tail_only_as_inferred_presentation()
    {
        static ApiHistorySample MissingModel(long timestamp, double remaining) =>
            new(
                timestamp,
                2_000,
                remaining,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
                ModelSamples = [],
            };

        var samples = new[]
        {
            Point(0, 100, 0, 0, 0),
            Point(60, 75, 50, 0, 0),
            Point(120, 50, 100, 0, 0),
            MissingModel(180, 25),
            MissingModel(240, 0),
        };
        var scene = Scene(samples, 0, 240);
        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);

        Assert.Equal([0d, 50d, 100d, 100d, 100d], scene.Sol);
        Assert.False(scene.ModelReliability["SOL"][3]);
        Assert.False(scene.ModelReliability["SOL"][4]);
        Assert.Empty(scene.IdleIntervals);
        Assert.Equal([0d, 60d, 120d], lines.Rising.X);
        Assert.Equal([0d, 50d, 100d], lines.Rising.Y);
        Assert.Equal([120d, 240d], lines.Dashed.X);
        Assert.Equal([100d, 100d], lines.Dashed.Y);
    }

    [Fact]
    public void Remaining_preserves_direct_remote_changes_and_does_not_fabricate_terminal_consumption()
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
            // A lower direct quota observation is authoritative independently
            // of unchanged model totals.
            Point(1_120, 70, 1, 0, 0),
            Point(1_180, 60, 2, 0, 0),
        };
        var idleRereadScene = Scene(idleReread);
        var idleRereadEffective = idleRereadScene.Remaining;
        var idleRereadLines = GraphPlotProjection.BuildRemainingLines(idleRereadScene);
        Assert.Equal(70d, idleRereadEffective[2]);
        Assert.Equal(60d, idleRereadEffective[3]);
        Assert.Equal([1_000d, 1_060d, 1_120d, 1_180d], idleRereadLines.Solid.X);
        Assert.Empty(idleRereadLines.Dashed.X);

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
        Assert.Equal([1_000d, 1_060d, 1_120d], terminalLines.Solid.X);
        Assert.Equal([100d, 90d, 90d], terminalLines.Solid.Y);
        Assert.Equal([1_120d, 1_180d], terminalLines.Dashed.X);
        Assert.Equal([90d, 90d], terminalLines.Dashed.Y);
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
        new(
            timestamp,
            2_000,
            remaining,
            sol,
            terra,
            luna,
            (ulong)sol,
            (ulong)terra,
            (ulong)luna,
            ApiHistorySample.ConfirmedModelSource)
        {
            ModelsComplete = true,
            // Synthetic unit-test points explicitly model a confirmed idle
            // marker; production samples pass the nullable wire value through.
            TaskActiveSincePrevious = false,
        };

    private static ApiHistorySample CompleteModelSample(
        long timestamp,
        double? remaining,
        double solDollars,
        ulong solTokens,
        string modelSource = ApiHistorySample.ConfirmedModelSource,
        bool? taskActiveSincePrevious = false) =>
        new(
            timestamp,
            2_000,
            remaining,
            null,
            null,
            null,
            null,
            null,
            null,
            modelSource)
        {
            ModelsComplete = true,
            TaskActiveSincePrevious = taskActiveSincePrevious,
            ModelSamples =
            [
                new ApiHistoryModelSample("SOL", null, null, null, solDollars)
                {
                    TotalTokens = solTokens,
                },
                new ApiHistoryModelSample("LUNA", null, null, null, 0)
                {
                    TotalTokens = 0,
                },
                new ApiHistoryModelSample("TERRA", null, null, null, 0)
                {
                    TotalTokens = 0,
                },
            ],
        };

    private static ApiHistorySample[] SimpleOracleSamples(JsonElement fixture) =>
        fixture.GetProperty("samples")
            .EnumerateArray()
            .Select(sample => new ApiHistorySample(
                sample.GetProperty("timestamp").GetInt64(),
                10_000,
                sample.GetProperty("remaining_percent").ValueKind == JsonValueKind.Null
                    ? null
                    : sample.GetProperty("remaining_percent").GetDouble(),
                sample.GetProperty("dollars").GetDouble(),
                0,
                0,
                sample.GetProperty("tokens").GetUInt64(),
                0,
                0,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                TaskActiveSincePrevious = sample.TryGetProperty(
                    "task_active_since_previous",
                    out var taskActive) && taskActive.ValueKind != JsonValueKind.Null
                        ? taskActive.GetBoolean()
                        : null,
            })
            .ToArray();

    private static (long StartAt, long EndAt)[] RelativeSegmentPairs(
        long origin,
        params GraphLineProjection[] projections) =>
        SegmentPairs(projections)
            .Select(segment => (segment.StartAt - origin, segment.EndAt - origin))
            .ToArray();

    private static (long StartAt, long EndAt)[] SegmentPairs(JsonElement segments) =>
        segments
            .EnumerateArray()
            .Select(segment => (segment[0].GetInt64(), segment[1].GetInt64()))
            .OrderBy(segment => segment.Item1)
            .ThenBy(segment => segment.Item2)
            .ToArray();

    private static (long StartAt, long EndAt)[] SegmentPairs(
        params GraphLineProjection[] projections)
    {
        var result = new List<(long StartAt, long EndAt)>();
        foreach (var projection in projections)
        {
            for (var index = 1; index < projection.X.Count; index++)
            {
                var start = projection.X[index - 1];
                var end = projection.X[index];
                if (double.IsFinite(start) && double.IsFinite(end))
                {
                    result.Add(((long)start, (long)end));
                }
            }
        }

        return result
            .OrderBy(segment => segment.StartAt)
            .ThenBy(segment => segment.EndAt)
            .ToArray();
    }

    private static void AddLiveSegments(
        ICollection<LiveGraphSegment> target,
        string metric,
        string series,
        string style,
        GraphLineProjection projection)
    {
        foreach (var (startAt, endAt) in SegmentPairs(projection))
        {
            target.Add(new LiveGraphSegment(metric, series, startAt, endAt, style));
        }
    }

    private static LiveGraphSegment[] OrderLiveSegments(IEnumerable<LiveGraphSegment> segments) =>
        segments
            .OrderBy(segment => segment.Metric switch
            {
                "remaining" => 0,
                "tokens" => 1,
                "dollars" => 2,
                _ => throw new InvalidOperationException($"Unknown live graph metric: {segment.Metric}"),
            })
            .ThenBy(segment => segment.Series, StringComparer.Ordinal)
            .ThenBy(segment => segment.StartAt)
            .ThenBy(segment => segment.EndAt)
            .ThenBy(segment => segment.Style, StringComparer.Ordinal)
            .ToArray();

    private static bool PathIsInside(string candidate, string root)
    {
        var comparison = OperatingSystem.IsWindows()
            ? StringComparison.OrdinalIgnoreCase
            : StringComparison.Ordinal;
        var normalizedRoot = root.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        return candidate.Equals(normalizedRoot, comparison) ||
            candidate.StartsWith(normalizedRoot + Path.DirectorySeparatorChar, comparison);
    }

    private static LiveRenderContract BuildLiveRenderContract(GraphScene scene)
    {
        var span = Math.Max(1d, scene.PeriodEndAt - scene.PeriodStartAt);
        var models = scene.ModelSeries
            .Where(pair => IsRenderableModel(pair.Key))
            .OrderBy(pair => pair.Key, StringComparer.Ordinal)
            .Select(pair =>
            {
                var lines = GraphPlotProjection.BuildCanonicalModelLines(scene, pair.Value);
                return new LiveModelRenderPaths(
                    pair.Key,
                    lines.Idle.Path,
                    lines.Flat.Path,
                    lines.Rising.Path,
                    lines.Dashed.Path);
            })
            .ToArray();
        var remaining = GraphPlotProjection.BuildCanonicalRemainingLines(scene);
        var markers = GraphPlotProjection.BuildCanonicalRemainingMarkers(scene)
            .Select(marker => new LiveRemainingMarker(
                marker.X.ToString("F12", CultureInfo.InvariantCulture),
                marker.YTop.ToString("F12", CultureInfo.InvariantCulture),
                marker.Boundary))
            .ToArray();
        var idle = scene.IdleIntervals
            .Select(interval => new LiveIdleGeometry(
                ((interval.StartAt - scene.PeriodStartAt) / span * 100)
                    .ToString("F12", CultureInfo.InvariantCulture),
                ((interval.EndAt - interval.StartAt) / span * 100)
                    .ToString("F12", CultureInfo.InvariantCulture)))
            .ToArray();
        var axes = GraphPlotProjection.BuildAxes(
            scene,
            TimeZoneInfo.Utc,
            CultureInfo.InvariantCulture);
        var endpointLabels = GraphPlotProjection.BuildEndpointLabels(
                scene,
                CultureInfo.InvariantCulture)
            .Select(label => new LiveEndpointLabel(
                label.Series == GraphSeries.Remaining ? "remaining" : label.Series.ToString().ToUpperInvariant(),
                label.Text,
                label.NormalizedTop.ToString("F9", CultureInfo.InvariantCulture),
                label.ArrangedTop.ToString("F9", CultureInfo.InvariantCulture)))
            .ToArray();
        var endpointValues = scene.ModelSeries
            .Where(pair => IsRenderableModel(pair.Key))
            .OrderBy(pair => pair.Key, StringComparer.Ordinal)
            .Select(pair => new LiveEndpointValue(
                pair.Key,
                scene.PeriodEndAt,
                double.IsFinite(pair.Value[^1]) ? pair.Value[^1] : null))
            .Append(new LiveEndpointValue(
                "remaining",
                scene.PeriodEndAt,
                double.IsFinite(scene.Remaining[^1]) ? scene.Remaining[^1] : null))
            .ToArray();
        var axisGridY = axes.ModelValues
            .Reverse()
            .Select(value =>
                ((axes.ModelDisplayMaximum - value) /
                 (axes.ModelDisplayMaximum - axes.ModelDisplayMinimum))
                .ToString("F12", CultureInfo.InvariantCulture))
            .ToArray();
        var gutterWidth = scene.Metric == GraphMetric.Tokens
            ? GraphPlotProjection.TokenLabelGutterWidth
            : GraphPlotProjection.DollarLabelGutterWidth;
        var remainingPoints = scene.Timestamps
            .Select((timestamp, index) => new LiveRemainingPoint(
                (long)timestamp,
                scene.Remaining[index].ToString("F12", CultureInfo.InvariantCulture),
                RemainingOriginName(scene, scene.RemainingOrigins[index], index)))
            .Where(point => point.Value != "NaN")
            .ToList();
        if (remainingPoints.Count > 0 &&
            scene.RemainingObserved.Any(observed => observed) &&
            scene.PeriodEndAt > remainingPoints[^1].Timestamp)
        {
            // BuildRemainingLines renders this same horizontal dashed hold.
            // Include it in the renderer contract without inventing a source
            // history observation in BuildGraphSamples.
            remainingPoints.Add(new LiveRemainingPoint(
                scene.PeriodEndAt,
                remainingPoints[^1].Value,
                "synthetic_tail_hold"));
        }
        return new LiveRenderContract(
            [100, 100],
            scene.ModelMaximum.ToString("F12", CultureInfo.InvariantCulture),
            axes.BottomTimestampValues,
            axes.ModelLabels.Reverse().ToArray(),
            axisGridY,
            endpointLabels,
            endpointValues,
            scene.PeriodEndAt,
            new LiveGraphLayout(
                788,
                788 - gutterWidth,
                gutterWidth,
                GraphPlotProjection.EndpointLabelGapWidth,
                gutterWidth - GraphPlotProjection.EndpointLabelGapWidth - 4,
                4,
                GraphPlotProjection.MinimumPlotHeight),
            new LiveGraphStyles(
                GraphPlotControl.PlotColorHex,
                GraphPlotControl.GridColorHex,
                GraphPlotControl.AxisTextColorHex,
                GraphPlotControl.IdleBandColorHex.ToLowerInvariant(),
                GraphPlotControl.RemainingColorHex,
                GraphPlotControl.SolColorHex,
                GraphPlotControl.TerraColorHex,
                GraphPlotControl.LunaColorHex,
                GraphPlotControl.AstraColorHex,
                GraphPlotControl.IdleLineWidth,
                GraphPlotControl.MeasuredFlatModelLineWidth,
                GraphPlotControl.MeasuredModelLineWidth,
                GraphPlotControl.InferredLineWidth,
                GraphPlotControl.MeasuredRemainingLineWidth,
                2,
                "0.95",
                "0.95",
                "0.72"),
            idle,
            models,
            new LiveRemainingRenderPaths(
                remaining.Idle.Path,
                remaining.Solid.Path,
                remaining.Dashed.Path),
            markers,
            remainingPoints);
    }

    private sealed record LiveGraphSegment(
        string Metric,
        string Series,
        long StartAt,
        long EndAt,
        string Style);

    private sealed record LiveIdleInterval(long StartAt, long EndAt);

    private sealed record LiveIdleGeometry(string Start, string Width);

    private sealed record LiveModelRenderPaths(
        string Series,
        string Idle,
        string Flat,
        string Rising,
        string Dashed);

    private sealed record LiveRemainingRenderPaths(string Idle, string Solid, string Dashed);

    private sealed record LiveRemainingMarker(string X, string YTop, int Boundary);

    private sealed record LiveRemainingPoint(long Timestamp, string Value, string Origin);

    private sealed record LiveEndpointLabel(
        string Series,
        string Text,
        string PointY,
        string LabelY);

    private sealed record LiveEndpointValue(
        string Series,
        long Timestamp,
        double? Value);

    private sealed record LiveGraphLayout(
        double ReferenceDataWidth,
        double PlotWidth,
        double GutterWidth,
        double LabelGap,
        double LabelWidth,
        double RightPadding,
        double MinimumPlotHeight);

    private sealed record LiveGraphStyles(
        string PlotSurface,
        string Grid,
        string AxisText,
        string IdleBand,
        string Remaining,
        string Sol,
        string Terra,
        string Luna,
        string Astra,
        float IdleWidth,
        float FlatWidth,
        float RisingWidth,
        float InferredWidth,
        float RemainingWidth,
        int MarkerSize,
        string FlatOpacity,
        string RisingOpacity,
        string InferredOpacity);

    private sealed record LiveRenderContract(
        IReadOnlyList<int> Viewbox,
        string ModelMaximum,
        IReadOnlyList<long> TimeTicks,
        IReadOnlyList<string> AxisLabels,
        IReadOnlyList<string> AxisGridY,
        IReadOnlyList<LiveEndpointLabel> EndpointLabels,
        IReadOnlyList<LiveEndpointValue> EndpointValues,
        long LatestTimestamp,
        LiveGraphLayout Layout,
        LiveGraphStyles Styles,
        IReadOnlyList<LiveIdleGeometry> IdleGeometry,
        IReadOnlyList<LiveModelRenderPaths> Models,
        LiveRemainingRenderPaths Remaining,
        IReadOnlyList<LiveRemainingMarker> RemainingMarkers,
        IReadOnlyList<LiveRemainingPoint> RemainingPoints);

    private sealed record LiveActualDocument(
        string SchemaVersion,
        string SourceSha,
        string InputSha256,
        string PublishedPair,
        string Platform,
        IReadOnlyList<LiveGraphSegment> Segments,
        IReadOnlyList<LiveIdleInterval> IdleIntervals,
        IReadOnlyDictionary<string, LiveRenderContract> RenderContracts);

    private static string RemainingOriginName(
        GraphScene scene,
        GraphRemainingOrigin origin,
        int index) => origin switch
        {
            GraphRemainingOrigin.Raw => "raw",
            GraphRemainingOrigin.Interpolated => "interpolated",
            GraphRemainingOrigin.BoundedNullHold => "bounded_null_hold",
            GraphRemainingOrigin.TerminalNullHold => "terminal_null_hold",
            GraphRemainingOrigin.SyntheticTailHold => "synthetic_tail_hold",
            GraphRemainingOrigin.MonotonicHold => "monotonic_hold",
            _ => "missing",
        };

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
            ApiHistorySample.ConfirmedModelSource)
        {
            ModelsComplete = true,
        };

    private static void AssertFirstObservationModel(
        GraphModelLineProjection lines,
        double firstObserved,
        double terminalObserved)
    {
        Assert.Empty(lines.Flat.X);
        Assert.Equal([1_000d, 1_120d, 1_180d], lines.Rising.X);
        Assert.Equal([0d, firstObserved, terminalObserved], lines.Rising.Y);
        Assert.Empty(lines.Dashed.X);
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

    private sealed class SplitHistoryFixtureHandler(
        byte[] periodsBody,
        byte[] historyBody,
        string pair,
        string periodId) : HttpMessageHandler
    {
        private readonly byte[] periodsBody = periodsBody.ToArray();
        private readonly byte[] historyBody = historyBody.ToArray();

        public int RequestCount { get; private set; }

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            RequestCount++;
            Assert.Equal(HttpMethod.Get, request.Method);
            var path = request.RequestUri?.AbsolutePath;
            var body = path switch
            {
                "/v3/history/periods" => periodsBody,
                "/v3/history" => historyBody,
                _ => throw new Xunit.Sdk.XunitException($"Unexpected fixture request: {request.RequestUri}"),
            };
            if (path == "/v3/history")
            {
                Assert.Equal($"?period={Uri.EscapeDataString(periodId)}", request.RequestUri?.Query);
            }

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
            response.Headers.TryAddWithoutValidation(PublishedPairHeader, pair);
            return Task.FromResult(response);
        }
    }

    private static GraphScene Scene(IReadOnlyList<ApiHistorySample> points, long? start = null, long? end = null) =>
        GraphScene.Create(points, GraphMetric.Dollars, start ?? points[0].Timestamp, end ?? points[^1].Timestamp);
}
