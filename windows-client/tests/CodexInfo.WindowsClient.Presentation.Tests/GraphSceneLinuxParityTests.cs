// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphSceneLinuxParityTests
{
    [Theory]
    [InlineData(false, GraphMetric.Tokens)]
    [InlineData(false, GraphMetric.Dollars)]
    [InlineData(true, GraphMetric.Tokens)]
    [InlineData(true, GraphMetric.Dollars)]
    public void Issue621TaskStopBoundaryPreventsSmoothedUsageBeforeResume(bool exactModelIds, GraphMetric metric)
    {
        // Saved 10/13 samples, relative to 06:08. The SOL event after the stop
        // is at 06:24:12, and the next minute observation is 06:25.
        var samples = Enumerable.Range(0, 19).Select(minute =>
        {
            var tokens = minute switch { 0 => 2_097_645ul, 17 => 2_896_985ul, 18 => 3_923_057ul, _ => 2_411_449ul };
            var dollars = minute switch { 0 => 2.206878, 17 => 2.760806, 18 => 3.448645, _ => 2.443581 };
            var exactDollars = minute switch { 0 => 0.6156964, 17 => 0.752226, 18 => 0.9193188, _ => 0.676122 };
            var missing = minute == 13; // 06:21: task stop is still known.
            return new ApiHistorySample(1_000 + minute * 60, 10_000, 100,
                missing ? null : dollars, missing ? null : 0, missing ? null : 0.05897728,
                missing ? null : tokens, missing ? null : 0, missing ? null : 2_924_216ul,
                missing ? ApiHistorySample.UnavailableModelSource : ApiHistorySample.LegacyUnknownModelSource)
            {
                ModelsComplete = false,
                TaskActiveSincePrevious = minute <= 1 || minute >= 17,
                ModelSamples = exactModelIds && !missing ? [
                    new ApiHistoryModelSample("gpt-6.1-sol", null, null, null, exactDollars) { TotalTokens = tokens - 28_924 },
                    new ApiHistoryModelSample("gpt-6-sol", null, null, null, 0.0457584) { TotalTokens = 28_924 },
                    new ApiHistoryModelSample("gpt-6-luna", null, null, null, 0.05897728) { TotalTokens = 2_924_216 },
                ] : null,
            };
        }).ToArray();
        var scene = GraphScene.Create(samples, metric, 1_000, 2_080);
        Assert.Equal([new GraphUnusedInterval(1_060, 1_960)],
            GraphPlotProjection.BuildVisibleUnusedIntervals(scene));
        var expected = metric == GraphMetric.Tokens ? 2_411_449 : exactModelIds ? 0.7218804 : 2.443581;
        var lines = GraphPlotProjection.BuildCanonicalModelLines(scene, scene.Sol);
        var stoppedPoints = new[] { lines.Idle.Line, lines.Flat.Line, lines.Rising.Line, lines.Dashed.Line }
            .SelectMany(line => line.X.Zip(line.Y))
            .Where(point => point.First > 1_061 && point.First < 1_959)
            .ToArray();
        Assert.NotEmpty(stoppedPoints);
        // Canonical rendering rounds normalized coordinates to two decimals.
        // With these fixed maxima (<4M tokens / $4), that is at most 200
        // tokens / $0.0002. The plateau itself must remain exactly horizontal.
        var quantization = metric == GraphMetric.Tokens ? 200 : 0.0002;
        Assert.All(stoppedPoints, point => Assert.InRange(point.Second,
            expected - quantization, expected + quantization));
        Assert.All(stoppedPoints, point => Assert.Equal(stoppedPoints[0].Second, point.Second, 9));
        Assert.Equal(expected, scene.Sol[16], 9);
        Assert.Equal(metric == GraphMetric.Tokens ? 2_896_985 : exactModelIds ? 0.7979844 : 2.760806,
            scene.Sol[17], 9);
        Assert.Equal(2_411_449ul, samples[16].SolTokens);
        Assert.Null(samples[13].SolTokens);
        Assert.Equal(2_896_985ul, samples[17].SolTokens);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void Issue621SlowerRecordedUsageDoesNotExtendTaskIdle(bool exactModelIds)
    {
        // Minutes are relative to 06:03. Actual cumulative SOL values from
        // 10/13: the increase at 06:09 precedes the 06:09–06:24 stop.
        var values = new Dictionary<int, ulong>
        {
            [0] = 847_165,
            [1] = 1_361_486,
            [3] = 1_943_197,
            [5] = 2_097_645,
            [6] = 2_411_449,
            [22] = 2_896_985,
        };
        var samples = Enumerable.Range(0, 23)
            .Where(minute => minute is not 2)
            .Select(minute => minute is 4 or 18
                ? new ApiHistorySample(1_000 + minute * 60, 10_000, minute == 4 ? null : 100,
                    null, null, null, null, null, null, ApiHistorySample.UnavailableModelSource)
                { ModelsComplete = false, TaskActiveSincePrevious = minute < 7 }
                : new ApiHistorySample(1_000 + minute * 60, 10_000, 100,
                    values.GetValueOrDefault(minute, 2_411_449ul), 0, 2_924_216,
                    values.GetValueOrDefault(minute, 2_411_449ul), 0, 2_924_216,
                    ApiHistorySample.LegacyUnknownModelSource)
                {
                    ModelsComplete = false,
                    TaskActiveSincePrevious = minute < 7 || minute == 22,
                    ModelSamples = exactModelIds ? [
                        new ApiHistoryModelSample("gpt-6.1-sol", null, null, null, null) { TotalTokens = values.GetValueOrDefault(minute, 2_411_449ul) - 28_924 },
                        new ApiHistoryModelSample("gpt-6-sol", null, null, null, null) { TotalTokens = 28_924 },
                        new ApiHistoryModelSample("gpt-6-luna", null, null, null, null) { TotalTokens = 2_924_216 },
                    ] : null,
                })
            .ToArray();
        var scene = GraphScene.Create(samples, GraphMetric.Tokens, 1_000, 2_320);

        Assert.Equal([new GraphUnusedInterval(1_360, 2_260)],
            GraphPlotProjection.BuildVisibleUnusedIntervals(scene));
        Assert.Equal(2_097_645d, scene.Sol[Array.IndexOf(samples.Select(s => s.Timestamp).ToArray(), 1_300)]);
        Assert.Equal(2_411_449d, scene.Sol[Array.IndexOf(samples.Select(s => s.Timestamp).ToArray(), 1_360)]);
        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        Assert.Contains(1_360d, lines.Idle.X);
        Assert.All(lines.Idle.Y.Where(double.IsFinite), y => Assert.Equal(2_411_449d, y));
    }

    [Theory]
    [InlineData(false, true, false)]
    [InlineData(false, true, true)]
    [InlineData(false, false, false)]
    [InlineData(true, true, false)]
    [InlineData(null, true, false)]
    public void Issue621StoppedTasksBoundMissingModelHistory(bool? active, bool resumed, bool legacyBoundary)
    {
        var samples = Enumerable.Range(0, 29).Select(minute =>
            (minute == 0 || resumed && minute == 28
                ? V3Sample(1_000 + minute * 60, minute == 0 ? 93 : 92,
                    Model("SOL", minute == 0 ? 100ul : 200ul, minute == 0 ? 1ul : 2ul))
                : legacyBoundary && minute == 27
                    ? V3Sample(2_620, 93, Model("SOL", 150, 1)) with
                    { ModelSource = ApiHistorySample.LegacyUnknownModelSource, ModelsComplete = false }
                : new ApiHistorySample(1_000 + minute * 60, 10_000, null,
                    null, null, null, null, null, null, ApiHistorySample.UnavailableModelSource)
                { ModelsComplete = false }) with
            { TaskActiveSincePrevious = minute is >= 2 and <= 27 ? active : true })
            .ToArray();
        foreach (var metric in new[] { GraphMetric.Tokens, GraphMetric.Dollars })
        {
            var scene = GraphScene.Create(samples, metric, 1_000, 2_680);
            // Both measured endpoints are authoritative. A missing-model span
            // containing their positive token delta cannot establish inactivity.
            if (active != false || resumed)
            {
                Assert.Empty(GraphPlotProjection.BuildVisibleUnusedIntervals(scene));
                continue;
            }
            Assert.Equal([new GraphUnusedInterval(1_060, 2_620)], GraphPlotProjection.BuildVisibleUnusedIntervals(scene));
            var line = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
            Assert.Contains(1_060d, line.Idle.X);
            Assert.Contains(2_620d, line.Idle.X);
            Assert.All(line.Idle.Y.Where(double.IsFinite), y => Assert.Equal(metric == GraphMetric.Tokens ? 100 : 1, y));
            Assert.DoesNotContain(line.Dashed.X, x => x > 1_060 && x < 2_620);
            var quota = GraphPlotProjection.BuildRemainingLines(scene);
            Assert.Contains(1_060d, quota.Idle.X);
            Assert.Contains(2_620d, quota.Idle.X);
            Assert.All(quota.Idle.Y.Where(double.IsFinite), y => Assert.Equal(93, y));
            Assert.Null(samples[10].RemainingPercent);
            Assert.False(samples[10].ModelsComplete);
        }
    }

    [Fact]
    public void HiddenModelsRemainSemanticAuthoritiesButOnlyAffectDisplayedSeriesAndAxis()
    {
        var samples = Enumerable.Range(0, 61)
            .Select(minute => V3Sample(
                1_000 + minute * 60,
                100,
                Model("SOL", 0, 0),
                Model("TERRA", (ulong)minute * 100, (ulong)minute)))
            .ToArray();
        var full = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 4_600);
        var hidden = GraphScene.Create(
            samples,
            GraphMetric.Dollars,
            1_000,
            4_600,
            confirmedGaps: null,
            hiddenModelNames: new HashSet<string>(StringComparer.Ordinal) { "TERRA" });

        Assert.Empty(full.IdleIntervals);
        Assert.Equal(full.IdleIntervals, hidden.IdleIntervals);
        Assert.Equal(full.ModelVectorAvailable, hidden.ModelVectorAvailable);
        Assert.Contains("TERRA", hidden.TokenModelSeries.Keys);
        Assert.All(hidden.PublishedModelNames, names => Assert.Contains("TERRA", names));
        Assert.DoesNotContain("TERRA", hidden.ModelSeries.Keys);
        Assert.True(full.ModelMaximum > hidden.ModelMaximum);

        var fullAxes = GraphPlotProjection.BuildAxes(
            full,
            TimeZoneInfo.Utc,
            System.Globalization.CultureInfo.InvariantCulture);
        var hiddenAxes = GraphPlotProjection.BuildAxes(
            hidden,
            TimeZoneInfo.Utc,
            System.Globalization.CultureInfo.InvariantCulture);
        Assert.True(fullAxes.ModelDisplayMaximum > hiddenAxes.ModelDisplayMaximum);
        Assert.True(hidden.TryGetTokenIntervalEvidence(0, 1, out var advanced));
        Assert.True(advanced);
    }

    [Fact]
    public void RemainingNullInterpolationIsIndependentOfHiddenModelActivity()
    {
        var samples = new[]
        {
            V3Sample(1_000, 100, Model("SOL", 0, 0), Model("TERRA", 0, 0)),
            V3Sample(1_060, null, Model("SOL", 0, 0), Model("TERRA", 100, 1)),
            V3Sample(1_120, null, Model("SOL", 0, 0), Model("TERRA", 100, 1)),
            V3Sample(1_300, 70, Model("SOL", 0, 0), Model("TERRA", 300, 3)),
        };
        var full = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_300);
        var hidden = GraphScene.Create(
            samples,
            GraphMetric.Dollars,
            1_000,
            1_300,
            confirmedGaps: null,
            hiddenModelNames: new HashSet<string>(StringComparer.Ordinal) { "TERRA" });

        Assert.Equal([100d, 94d, 88d, 70d], full.Remaining);
        Assert.Equal(full.Remaining, hidden.Remaining);
    }

    [Fact]
    public void SessionDerivedModelRowsAreDiscardedBeforeGraphProjection()
    {
        var sample = V3Sample(
            1_000,
            100,
            ApiHistorySample.ReconstructedFromSessionModelSource,
            Model("SOL", 1_100_000, 1_000_000, 100_000, 100_000, 0),
            Model("TERRA", 2_200_000, 2_000_000, 200_000, 200_000, 0),
            Model("LUNA", 3_300_000, 3_000_000, 300_000, 300_000, 0),
            Model("ASTRA", 1_100_000, 1_000_000, 100_000, 100_000, 100_000),
            Model("unknown-model", 1_000_000, 1_000_000, 0, 0, 0));
        var scene = GraphScene.Create([sample], GraphMetric.Dollars, 1_000, 1_000);

        Assert.All(scene.Sol, value => Assert.True(double.IsNaN(value)));
        Assert.DoesNotContain("SOL", scene.ModelSeries.Keys);
        Assert.DoesNotContain("TERRA", scene.ModelSeries.Keys);
        Assert.DoesNotContain("LUNA", scene.ModelSeries.Keys);
        Assert.DoesNotContain("ASTRA", scene.ModelSeries.Keys);
        Assert.DoesNotContain("unknown-model", scene.ModelSeries.Keys);
        Assert.DoesNotContain("SOL", scene.ModelReliability.Keys);
        Assert.DoesNotContain("SOL", scene.TokenReliability.Keys);
    }

    [Fact]
    public void NullDollarsRemainUnknownForNonSessionOrInvalidComponents()
    {
        var components = Model("SOL", 1_100_000, 1_000_000, 100_000, 100_000, 0);
        var confirmed = V3Sample(
            1_000,
            100,
            ApiHistorySample.ConfirmedModelSource,
            components);
        var legacy = V3Sample(
            1_000,
            100,
            ApiHistorySample.LegacyUnknownModelSource,
            components);
        var invalid = V3Sample(
            1_000,
            100,
            ApiHistorySample.ReconstructedFromSessionModelSource,
            Model("SOL", 1_000_000, 999_999, 100_000, 100_000, 1));

        Assert.True(double.IsNaN(GraphScene.Create([confirmed], GraphMetric.Dollars, 1_000, 1_000).Sol[0]));
        Assert.True(double.IsNaN(GraphScene.Create([legacy], GraphMetric.Dollars, 1_000, 1_000).Sol[0]));
        Assert.True(double.IsNaN(GraphScene.Create([invalid], GraphMetric.Dollars, 1_000, 1_000).Sol[0]));
    }

    [Fact]
    public void ExactModelHistoryUsesPersistedDollarsThenGroupsToFamily()
    {
        var sample = V3Sample(
            1_000,
            100,
            DollarModel("gpt-6-sol", 10, 0.2),
            DollarModel("gpt-5.6-sol", 20, 0.4));

        var scene = GraphScene.Create([sample], GraphMetric.Dollars, 1_000, 1_000);

        Assert.Equal(0.6, scene.Sol[0], 12);
        Assert.Equal(0.6, scene.ModelSeries["SOL"][0], 12);
        Assert.DoesNotContain("gpt-6-sol", scene.ModelSeries.Keys);
        Assert.DoesNotContain("gpt-5.6-sol", scene.ModelSeries.Keys);
        Assert.Equal(30UL, Assert.Single(scene.HoverObservations).TokensFor(GraphSeries.Sol));
    }

    [Fact]
    public void LegacyUnknownRowsRenderMeasuredLinesWithoutBecomingQuotaAuthorities()
    {
        var samples = new[]
        {
            V3Sample(
                1_000,
                100,
                ApiHistorySample.LegacyUnknownModelSource,
                Model("SOL", 0, 0)),
            V3Sample(
                1_060,
                90,
                ApiHistorySample.LegacyUnknownModelSource,
                Model("SOL", 10, 1)),
        };

        var scene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_060);

        Assert.Equal([0d, 1d], scene.Sol);
        Assert.False(scene.ModelReliability["SOL"][0]);
        Assert.False(scene.TokenReliability["SOL"][0]);
        Assert.False(scene.TryGetTokenIntervalEvidence(0, 1, out _));
        var modelLines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        Assert.Empty(modelLines.Flat.X);
        Assert.Equal([1_000d, 1_060d], modelLines.Rising.X);
        Assert.Empty(modelLines.Dashed.X);
        var remainingLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Equal([1_000d, 1_060d], remainingLines.Solid.X);
        Assert.Empty(remainingLines.Dashed.X);
    }

    [Theory]
    [InlineData(GraphMetric.Dollars)]
    [InlineData(GraphMetric.Tokens)]
    public void LegacyOutlierCannotRejectOrMoveLaterDirectObservation(GraphMetric metric)
    {
        var samples = new[]
        {
            V3Sample(1_000, 90, Model("SOL", 100, 100)),
            V3Sample(
                1_060,
                90,
                ApiHistorySample.LegacyUnknownModelSource,
                Model("SOL", 1_000, 1_000)),
            V3Sample(1_120, 90, Model("SOL", 110, 110)),
        };

        var scene = GraphScene.Create(samples, metric, 1_000, 1_120);

        Assert.Equal([100d, 105d, 110d], scene.Sol);
        Assert.True(scene.ModelReliability["SOL"][2]);
        Assert.True(scene.TokenReliability["SOL"][2]);
        Assert.False(scene.ModelReliability["SOL"][1]);
        Assert.Empty(scene.ModelCorrectionStarts["SOL"]);
        Assert.Empty(scene.TokenCorrectionStarts);
    }

    [Fact]
    public void IdleUsesRawTokenVectorsAndSplitsOnlyAtNonTokenAuthorityRows()
    {
        var direct = Enumerable.Range(0, 31)
            .Select(minute => V3Sample(
                1_000 + minute * 60,
                90,
                Model("SOL", 10, 1),
                Model("TERRA", 0, 0)))
            .ToArray();
        var expected = new[] { new GraphIdleInterval(1_000, 2_800, false) };
        Assert.Equal(expected, GraphScene.Create(direct, GraphMetric.Dollars, 1_000, 2_800).IdleIntervals);

        var withEstimatedRow = direct
            .Take(15)
            .Append(direct[14] with
            {
                Timestamp = direct[14].Timestamp + 30,
                ModelSource = ApiHistorySample.ReconstructedFromSessionModelSource,
                ModelsComplete = false,
            })
            .Concat(direct.Skip(15))
            .ToArray();
        Assert.Equal(
            [
                new GraphIdleInterval(1_000, 1_840, false),
                new GraphIdleInterval(1_900, 2_800, false),
            ],
            GraphScene.Create(withEstimatedRow, GraphMetric.Dollars, 1_000, 2_800).IdleIntervals);

        var dollarOnlyChange = direct
            .Select((sample, index) => index == 15
                ? sample with
                {
                    ModelSamples =
                    [
                        Model("SOL", 10, 2),
                        Model("TERRA", 0, 0),
                    ],
                }
                : sample)
            .ToArray();
        Assert.Equal(
            expected,
            GraphScene.Create(dollarOnlyChange, GraphMetric.Dollars, 1_000, 2_800).IdleIntervals);
    }

    [Fact]
    public void IdleRejectsNumericIncompleteRowsBetweenDirectEndpoints()
    {
        var samples = new[]
        {
            V3Sample(1_000, 90, Model("SOL", 10, 1), Model("TERRA", 0, 0)),
            new ApiHistorySample(
                1_900,
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
                ModelsComplete = false,
                ModelSamples = [Model("SOL", 10, 1)],
            },
            V3Sample(2_800, 90, Model("SOL", 10, 1), Model("TERRA", 0, 0)),
        };

        Assert.Empty(GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 2_800).IdleIntervals);
    }

    [Fact]
    public void IdleRecoversOneUnavailableSamplingMinuteBetweenEqualDirectEndpoints()
    {
        var samples = Enumerable.Range(0, 11)
            .Select(minute => minute == 5
                ? new ApiHistorySample(
                    1_000 + minute * 60,
                    10_000,
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
                }
                : V3Sample(1_000 + minute * 60, 90, Model("SOL", 10, 1), Model("TERRA", 0, 0)))
            .ToArray();

        Assert.Equal(
            [new GraphIdleInterval(1_000, 1_600, false)],
            GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_600).IdleIntervals);
    }

    [Fact]
    public void IdleRejectsConsecutiveInactiveUnavailableMinutes()
    {
        var samples = Enumerable.Range(0, 11)
            .Select(minute => minute is 5 or 6
                ? new ApiHistorySample(
                    1_000 + minute * 60,
                    10_000,
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
                }
                : V3Sample(1_000 + minute * 60, 90, Model("SOL", 10, 1), Model("TERRA", 0, 0)))
            .ToArray();

        Assert.Empty(GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_600).IdleIntervals);
    }

    [Fact]
    public void IdleRecoversSeparatedSingleUnavailableSamplingMinutes()
    {
        var samples = Enumerable.Range(0, 35)
            .Select(minute => minute is 5 or 21
                ? new ApiHistorySample(
                    1_000 + minute * 60,
                    10_000,
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
                }
                : V3Sample(1_000 + minute * 60, 90, Model("SOL", 10, 1), Model("TERRA", 0, 0)))
            .ToArray();

        Assert.Equal(
            [new GraphIdleInterval(1_000, 3_040, false)],
            GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 3_040).IdleIntervals);
    }

    [Fact]
    public void ActiveMetadataAloneDoesNotSplitExactIdle()
    {
        var samples = Enumerable.Range(0, 22)
            .Select(minute => V3Sample(1_000 + minute * 60, 90, Model("SOL", 10, 1)) with
            {
                TaskActiveSincePrevious = minute == 11,
            })
            .ToArray();

        Assert.Equal(
            [new GraphIdleInterval(1_000, 2_260, false)],
            GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 2_260).IdleIntervals);
    }

    [Fact]
    public void IdleRequiresTheSameCompleteModelUniverseAtBothEndpoints()
    {
        var samples = new[]
        {
            V3Sample(1_000, 90, Model("SOL", 10, 1)),
            new ApiHistorySample(
                1_900,
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
                ModelSamples = [Model("SOL", 10, 1), Model("TERRA", 0, 0)],
            },
            V3Sample(2_800, 90, Model("SOL", 10, 1)),
        };

        Assert.Empty(GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 2_800).IdleIntervals);
    }

    [Fact]
    public void ValidRawQuotaRemainsAMeasuredSolidSegment()
    {
        var samples = new[]
        {
            V3Sample(1_000, 100, Model("SOL", 0, 0)),
            V3Sample(1_060, 100, Model("SOL", 10, 0)),
            V3Sample(1_120, 100, Model("SOL", 20, 0)),
            V3Sample(1_180, 70, Model("SOL", 100, 0)),
        };

        var scene = GraphScene.Create(samples, GraphMetric.Tokens, 1_000, 1_180);
        var lines = GraphPlotProjection.BuildRemainingLines(scene);

        Assert.Equal([1_000d, 1_060d, 1_120d, 1_180d], lines.Solid.X);
        Assert.Empty(lines.Dashed.X);
    }

    private static ApiHistorySample V3Sample(
        long timestamp,
        double? remaining,
        params ApiHistoryModelSample[] models) =>
        V3Sample(timestamp, remaining, ApiHistorySample.ConfirmedModelSource, models);

    private static ApiHistorySample V3Sample(
        long timestamp,
        double? remaining,
        string source,
        params ApiHistoryModelSample[] models) =>
        new(
            timestamp,
            10_000,
            remaining,
            null,
            null,
            null,
            null,
            null,
            null,
            source)
        {
            ModelsComplete = true,
            ModelSamples = models,
        };

    private static ApiHistoryModelSample Model(
        string name,
        ulong tokens,
        ulong input,
        ulong cached,
        ulong output,
        ulong cacheWrite) =>
        new(name, input, cached, output, null)
        {
            TotalTokens = tokens,
            CacheWriteInputTokens = cacheWrite,
        };

    private static ApiHistoryModelSample Model(
        string name,
        ulong tokens,
        ulong dollars) =>
        new(name, null, null, null, (double)dollars)
        {
            TotalTokens = tokens,
        };

    private static ApiHistoryModelSample DollarModel(
        string name,
        ulong tokens,
        double dollars) =>
        new(name, null, null, null, dollars)
        {
            TotalTokens = tokens,
        };
}
