// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphSceneLinuxParityTests
{
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
    public void HiddenModelsRemainQuotaSmoothingAuthorityAcrossUnequalIntervals()
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

        Assert.Equal([100d, 90d, 90d, 70d], full.Remaining);
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
    public void LegacyUnknownRowsAreDisplayOnlyDashedAndCannotAttributeQuota()
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
        Assert.Empty(modelLines.Rising.X);
        Assert.Equal([1_000d, 1_060d], modelLines.Dashed.X);
        var remainingLines = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Empty(remainingLines.Solid.X);
        Assert.Equal([1_000d, 1_060d], remainingLines.Dashed.X);
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
    public void IdleUsesRawDirectVectorsAndIgnoresDisplayEstimatedRows()
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
            expected,
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
}
