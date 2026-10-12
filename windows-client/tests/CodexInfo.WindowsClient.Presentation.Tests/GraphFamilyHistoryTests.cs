// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphFamilyHistoryTests
{
    [Theory]
    [InlineData(GraphMetric.Tokens, 599, false)]
    [InlineData(GraphMetric.Tokens, 600, false)]
    [InlineData(GraphMetric.Tokens, 1_200, false)]
    [InlineData(GraphMetric.Dollars, 1_200, false)]
    [InlineData(GraphMetric.Tokens, 1_200, true)]
    [InlineData(GraphMetric.Dollars, 1_200, true)]
    public void ConfirmedObservationSpacingDoesNotInventMissingData(GraphMetric metric, long spacing, bool hasGap)
    {
        var samples = new[]
        {
            Sample(1_000, Model("gpt-6-luna", 100, 1)) with
            { ModelSource = ApiHistorySample.ConfirmedModelSource, ModelsComplete = true, RemainingPercent = 90 },
            Sample(1_000 + spacing, Model("gpt-6-luna", 200, 2)) with
            { ModelSource = ApiHistorySample.ConfirmedModelSource, ModelsComplete = true, RemainingPercent = 80 },
        };
        var scene = GraphScene.Create(samples, metric, 1_000, 1_000 + spacing,
            hasGap ? [new GraphConfirmedGap(1_010, 990 + spacing)] : []);
        var model = GraphPlotProjection.BuildModelLines(scene, scene.Luna);
        var remaining = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.Equal(hasGap, model.Dashed.X.Count > 0);
        Assert.Equal(hasGap, remaining.Dashed.X.Count > 0);
        var viewport = GraphScene.CreateViewport(1_000, 1_000 + spacing, metric, [scene]);
        Assert.Equal(hasGap, GraphPlotProjection.BuildViewportModelLines(viewport, GraphSeries.Luna).Dashed.Line.X.Count > 0);
        Assert.Equal(hasGap, GraphPlotProjection.BuildViewportRemainingLines(viewport).Dashed.Line.X.Count > 0);
        Assert.Equal(metric == GraphMetric.Tokens ? new[] { 100d, 200d } : [1d, 2d], scene.Luna);
        Assert.Equal(new[] { 90d, 80d }, scene.Remaining);
        Assert.Equal(200UL, samples[1].Models[0].TotalTokens);
        Assert.Empty(scene.IdleIntervals);
        SaveLineImage(scene, $"confirmed-spacing-{metric}-{spacing}-gap-{hasGap}");
    }

    [Theory]
    [InlineData(GraphMetric.Tokens)]
    [InlineData(GraphMetric.Dollars)]
    public void FamilyNameChangeKeepsMetricEvidenceIndependent(GraphMetric knownMetric)
    {
        var oldFamily = new ApiHistoryModelSample("LUNA", null, null, null,
            knownMetric == GraphMetric.Dollars ? 0d : null)
        {
            TotalTokens = knownMetric == GraphMetric.Tokens ? 0UL : null,
        };
        var samples = new[]
        {
            Sample(1_000, oldFamily),
            Sample(1_060, Model("gpt-6-luna", 200, 20)),
            Sample(1_120, Model("gpt-6-luna", 300, 30)),
        };
        var known = GraphScene.Create(samples, knownMetric, 1_000, 1_120);
        Assert.Equal(knownMetric == GraphMetric.Tokens ? new[] { 0d, 200d, 300d } : [0d, 20d, 30d], known.Luna);
        Assert.Empty(GraphPlotProjection.BuildModelLines(known, known.Luna).Dashed.X);
        var hover = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            known, 1_060, new HashSet<GraphSeries> { GraphSeries.Luna }));
        if (knownMetric == GraphMetric.Tokens)
            Assert.Equal(200UL, Assert.Single(hover.Rows).TokenValue);
        else
            Assert.Equal(20d, Assert.Single(hover.Rows).NumericValue);

        var unknownMetric = knownMetric == GraphMetric.Tokens ? GraphMetric.Dollars : GraphMetric.Tokens;
        var unknown = GraphScene.Create(samples, unknownMetric, 1_000, 1_120);
        Assert.All(unknown.Luna, value => Assert.True(double.IsNaN(value)));
        var unknownHover = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            unknown, 1_060, new HashSet<GraphSeries> { GraphSeries.Luna }));
        Assert.Null(Assert.Single(unknownHover.Rows).TokenValue);
        Assert.Null(Assert.Single(unknownHover.Rows).NumericValue);
        Assert.Same(oldFamily, Assert.Single(samples[0].Models));
    }

    [Theory]
    [InlineData("LUNA", "gpt-6-luna", GraphMetric.Tokens)]
    [InlineData("LUNA", "gpt-6-luna", GraphMetric.Dollars)]
    [InlineData("SOL", "gpt-6.1-sol", GraphMetric.Tokens)]
    [InlineData("SOL", "gpt-6.1-sol", GraphMetric.Dollars)]
    [InlineData("TERRA", "gpt-5.6-terra", GraphMetric.Tokens)]
    [InlineData("TERRA", "gpt-5.6-terra", GraphMetric.Dollars)]
    [InlineData("ASTRA", "gpt-6-astra", GraphMetric.Tokens)]
    [InlineData("ASTRA", "gpt-6-astra", GraphMetric.Dollars)]
    public void PublishedFamilyNameChangeKeepsRecordedLineAndHover(
        string family, string model, GraphMetric metric)
    {
        // The history endpoint supplies its old family column until exact model
        // rows for that family are available. Both names belong to one curve.
        var samples = new[]
        {
            Sample(1_000, Model(family, 0, 0)),
            Sample(1_060, Model(model, 200, 20)),
            Sample(1_120, Model(model, 300, 30)),
        };
        var scene = GraphScene.Create(samples, metric, 1_000, 1_120);
        var series = Enum.Parse<GraphSeries>(family, ignoreCase: true);
        var values = scene.ModelSeries[family];
        Assert.Equal(metric == GraphMetric.Tokens ? new[] { 0d, 200d, 300d } : [0d, 20d, 30d], values);
        var lines = GraphPlotProjection.BuildModelLines(scene, values);
        Assert.Empty(lines.Dashed.X);
        Assert.Contains((1_060d, metric == GraphMetric.Tokens ? 200d : 20d), lines.Rising.X.Zip(lines.Rising.Y));
        Assert.Contains((1_120d, metric == GraphMetric.Tokens ? 300d : 30d), lines.Rising.X.Zip(lines.Rising.Y));

        var viewport = GraphScene.CreateViewport(1_000, 1_120, metric, [scene]);
        foreach (var view in new[] { scene, viewport })
        {
            var hover = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
                view, 1_060, new HashSet<GraphSeries> { series }));
            var row = Assert.Single(hover.Rows);
            if (metric == GraphMetric.Tokens)
                Assert.Equal(200UL, row.TokenValue);
            else
                Assert.Equal(20d, row.NumericValue);
        }

        // Display grouping must not manufacture raw model rows or idle evidence.
        Assert.Single(samples[0].Models);
        Assert.Equal(family, samples[0].Models[0].Name);
        Assert.Equal(model, Assert.Single(samples[1].Models).Name);
        Assert.True(double.IsNaN(scene.TokenModelSeries[model][0]));
        Assert.All(scene.ModelReliability[family], Assert.False);
        Assert.Empty(scene.IdleIntervals);
        if (family == "LUNA")
            SaveLineImage(scene, $"recorded-{metric}");
    }

    [Theory]
    [InlineData(GraphMetric.Tokens)]
    [InlineData(GraphMetric.Dollars)]
    public void FamilyNameChangeDoesNotHideARealMissingInterval(GraphMetric metric)
    {
        var samples = new[]
        {
            Sample(1_000, Model("LUNA", 0, 0)),
            Sample(1_060, Model("gpt-6-luna", 200, 20)),
            Sample(1_120) with { ModelSource = ApiHistorySample.UnavailableModelSource, ModelSamples = null },
            Sample(1_180, Model("gpt-6-luna", 300, 30)),
            Sample(1_240, Model("gpt-6-luna", 400, 40)),
        };
        var scene = GraphScene.Create(samples, metric, 1_000, 1_240);
        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Luna);
        var observed = lines.Rising.X.Zip(lines.Rising.Y).ToArray();
        Assert.Contains((1_180d, metric == GraphMetric.Tokens ? 300d : 30d), observed);
        Assert.Contains((1_240d, metric == GraphMetric.Tokens ? 400d : 40d), observed);
        Assert.Contains(1_060d, lines.Dashed.X);
        Assert.Contains(1_180d, lines.Dashed.X);
        Assert.DoesNotContain(lines.Dashed.X, time => double.IsFinite(time) && (time < 1_060 || time > 1_180));
        var hover = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            scene, 1_120, new HashSet<GraphSeries> { GraphSeries.Luna }));
        Assert.Null(Assert.Single(hover.Rows).TokenValue);
        Assert.Null(Assert.Single(hover.Rows).NumericValue);
        SaveLineImage(scene, $"missing-interval-{metric}");
    }

    [Theory]
    [InlineData(GraphMetric.Tokens, 24)]
    [InlineData(GraphMetric.Tokens, 168)]
    [InlineData(GraphMetric.Dollars, 24)]
    [InlineData(GraphMetric.Dollars, 168)]
    public void LaterExactModelDoesNotEraseSavedFamilyHistoryOrHover(GraphMetric metric, int hours)
    {
        // Old-period observations from the reported incident. The exact model
        // first appears later; it is not an unknown value in earlier rows.
        var samples = new[]
        {
            Sample(1_791_360_780, Model("ASTRA", 45_868_393, 58.438066), Model("LUNA", 71_033_330, 2.45230816)),
            Sample(1_791_424_800, Model("ASTRA", 171_920_142, 219.875732), Model("LUNA", 256_185_255, 8.99136188)),
            Sample(1_791_438_000, Model("ASTRA", 201_416_126, 258.04086), Model("LUNA", 365_101_523, 12.5057224)),
            Sample(1_791_439_680, Model("ASTRA", 201_416_126, 258.04086), Model("LUNA", 365_101_523, 12.5057224),
                Model("gpt-6-astra", 8_885_062, 11.816564), Model("gpt-6-luna", 22_573_062, 0.41763788)),
        };
        var scene = GraphScene.Create(samples, metric, 1_791_343_860, 1_791_439_740);
        var end = 1_791_447_180L;
        var viewport = GraphScene.CreateViewport(end - hours * 3_600, end, metric, [scene]);

        Assert.Equal(metric == GraphMetric.Tokens ? 45_868_393d : 58.438066, scene.Astra[0], 8);
        Assert.Equal(metric == GraphMetric.Tokens ? 171_920_142d : 219.875732, scene.Astra[1], 8);
        Assert.Equal(metric == GraphMetric.Tokens ? 210_301_188d : 269.857424, scene.Astra[3], 8);
        Assert.Equal(metric == GraphMetric.Tokens ? 71_033_330d : 2.45230816, scene.Luna[0], 8);
        Assert.All(scene.ModelLineReliability["ASTRA"], Assert.True);
        Assert.All(scene.ModelReliability["ASTRA"], value => Assert.False(value));
        Assert.True(double.IsNaN(scene.TokenModelSeries["gpt-6-astra"][0]));
        Assert.Empty(scene.IdleIntervals);

        var line = GraphPlotProjection.BuildViewportModelLines(viewport, GraphSeries.Astra).Rising.Line;
        Assert.Contains(line.X, timestamp => timestamp >= samples[0].Timestamp && timestamp < samples[1].Timestamp);
        Assert.Contains(line.X, timestamp => timestamp >= samples[1].Timestamp && timestamp < samples[2].Timestamp);
        var hover = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            viewport, samples[1].Timestamp, new HashSet<GraphSeries> { GraphSeries.Astra, GraphSeries.Luna }));
        var astra = Assert.Single(hover.Rows, row => row.Series == GraphSeries.Astra);
        if (metric == GraphMetric.Tokens)
        {
            Assert.Equal(171_920_142UL, astra.TokenValue);
        }
        else
        {
            Assert.Equal(219.875732, Assert.IsType<double>(astra.NumericValue), 8);
        }
        Assert.Equal(2, samples[0].Models.Count);
        Assert.Equal(4, samples[3].Models.Count);
    }

    [Fact]
    public void LaterVersionWithUnknownPriceDoesNotInvalidateEarlierVersion()
    {
        var samples = new[]
        {
            Sample(1_000, Model("gpt-6-sol", 10, 1)),
            Sample(1_060, Model("gpt-6-sol", 20, 2)),
            Sample(1_120, Model("gpt-6-sol", 30, 3), Model("gpt-6.1-sol", 5, null)),
        };
        var dollars = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_120);
        var tokens = GraphScene.Create(samples, GraphMetric.Tokens, 1_000, 1_120);

        Assert.Equal(1d, dollars.Sol[0]);
        Assert.Equal(2d, dollars.Sol[1]);
        Assert.True(double.IsNaN(dollars.Sol[2]));
        Assert.Equal([10d, 20d, 35d], tokens.Sol);
        Assert.Equal(10UL, tokens.HoverObservations[0].TokensFor(GraphSeries.Sol));
        Assert.Equal(35UL, tokens.HoverObservations[2].TokensFor(GraphSeries.Sol));
    }

    [Fact]
    public void PreviouslyPublishedMemberOmissionDoesNotBecomeZeroOrAHoverValue()
    {
        var samples = new[]
        {
            Sample(1_000, Model("ASTRA", 10, 1)),
            Sample(1_060, Model("ASTRA", 20, 2), Model("gpt-6-astra", 5, 0.5)),
            Sample(1_120, Model("ASTRA", 30, 3)),
        };
        var scene = GraphScene.Create(samples, GraphMetric.Tokens, 1_000, 1_120);

        Assert.Equal(10d, scene.Astra[0]);
        Assert.Equal(25d, scene.Astra[1]);
        Assert.Equal(35d, scene.Astra[2]); // Existing non-authoritative held presentation.
        Assert.False(scene.ModelLineReliability["ASTRA"][2]);
        Assert.Null(scene.HoverObservations[2].TokensFor(GraphSeries.Astra));
        var hover = Assert.IsType<GraphHoverSnapshot>(GraphHoverProjection.Find(
            scene, 1_120, new HashSet<GraphSeries> { GraphSeries.Astra }));
        Assert.Null(Assert.Single(hover.Rows).TokenValue);
        Assert.Empty(scene.IdleIntervals);
    }

    [Fact]
    public void UnavailableOrSyntheticModelDoesNotEstablishEarlierFamilyMembership()
    {
        var samples = new[]
        {
            Sample(940, Model("gpt-6-astra", 100, 10)) with { ModelSource = ApiHistorySample.UnavailableModelSource },
            Sample(970, Model("gpt-6-astra", 100, 10)) with { IsSyntheticTail = true },
            Sample(1_000, Model("ASTRA", 10, 1)),
            Sample(1_060, Model("ASTRA", 20, 2), Model("gpt-6-astra", 0, 0)),
        };
        var scene = GraphScene.Create(samples, GraphMetric.Tokens, 940, 1_060);

        Assert.True(double.IsNaN(scene.Astra[0]));
        Assert.True(double.IsNaN(scene.Astra[1]));
        Assert.Equal(10d, scene.Astra[2]);
        Assert.Equal(20d, scene.Astra[3]);
        Assert.Equal(10UL, scene.HoverObservations.Single(row => row.Timestamp == 1_000).TokensFor(GraphSeries.Astra));
    }

    private static ApiHistorySample Sample(long timestamp, params ApiHistoryModelSample[] models) =>
        new(timestamp, 1_791_948_558, 50, null, null, null, null, null, null, ApiHistorySample.LegacyUnknownModelSource)
        {
            ModelsComplete = false,
            ModelSamples = models,
        };

    private static void SaveLineImage(GraphScene scene, string name)
    {
        var directory = Environment.GetEnvironmentVariable("CODEX_INFO_FAMILY_LINE_RENDER_DIR");
        if (string.IsNullOrWhiteSpace(directory))
            return;
        Directory.CreateDirectory(directory);
        new GraphPlotControl { Scene = scene }.Plot.SavePng(Path.Combine(directory, $"{name}.png"), 940, 480);
    }

    private static ApiHistoryModelSample Model(string name, ulong tokens, double? dollars) =>
        new(name, null, null, null, dollars) { TotalTokens = tokens };
}
