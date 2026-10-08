// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class Issue337HistoryGapTests
{
    [Fact]
    public void Delayed_first_observation_renders_quota_baseline_without_model_backfill()
    {
        var jst = TimeSpan.FromHours(9);
        var periodStart = new DateTimeOffset(2026, 9, 20, 7, 24, 36, jst).ToUnixTimeSeconds();
        var firstObservation = new DateTimeOffset(2026, 9, 21, 9, 45, 0, jst).ToUnixTimeSeconds();
        var periodEnd = new DateTimeOffset(2026, 9, 21, 14, 5, 0, jst).ToUnixTimeSeconds();
        var resetAt = new DateTimeOffset(2026, 9, 27, 7, 24, 36, jst).ToUnixTimeSeconds();
        Assert.Equal(periodStart, resetAt - 604_800);

        var scene = GraphScene.Create(
            [
                Sample(firstObservation, resetAt, 89, 0),
                Sample(periodEnd, resetAt, 75, 2.06),
            ],
            GraphMetric.Dollars,
            periodStart,
            periodEnd,
            confirmedGaps: null,
            hiddenModelNames: null,
            accountOwnershipIntervals: null,
            resetAt: resetAt,
            isVerifiedCurrentResetStart: true);

        var axes = GraphPlotProjection.BuildAxes(
            scene,
            TimeZoneInfo.Utc,
            CultureInfo.InvariantCulture);
        Assert.Equal(periodStart, axes.BottomTimestampValues[0]);
        Assert.Equal([firstObservation, periodEnd], scene.Timestamps.Select(value => (long)value));
        Assert.Equal(89, scene.Remaining[0]);
        Assert.Equal(0, scene.ModelSeries["SOL"][0]);

        var remaining = GraphPlotProjection.BuildRemainingLines(scene);
        Assert.DoesNotContain(
            remaining.Idle.X
                .Concat(remaining.Solid.X)
                .Concat(remaining.Dashed.X)
                .Where(double.IsFinite),
            timestamp => timestamp < firstObservation);

        var displayRemaining = GraphPlotProjection.BuildCanonicalRemainingLines(
            scene,
            GraphRemainingBaselineMode.PeriodStartAtFullQuota);
        Assert.Equal((double)periodStart, displayRemaining.Solid.Line.X[0]);
        Assert.Equal(100d, displayRemaining.Solid.Line.Y[0]);
        Assert.Contains(89d, displayRemaining.Solid.Line.Y);
        Assert.Contains(displayRemaining.Solid.Line.X, timestamp =>
            timestamp >= firstObservation - 4d && timestamp <= firstObservation + 4d);
        Assert.Empty(displayRemaining.Dashed.Line.X);

        Assert.DoesNotContain((double)periodStart, scene.Timestamps);
        Assert.Equal(0d, scene.ModelSeries["SOL"][0]);

        var displaySol = GraphPlotProjection.BuildCanonicalModelLines(scene, scene.ModelSeries["SOL"]);
        Assert.Equal((double)periodStart, displaySol.Flat.Line.X[0]);
        Assert.Equal(0d, displaySol.Flat.Line.Y[0]);
        Assert.InRange(displaySol.Flat.Line.X[1], firstObservation - 4d, firstObservation + 4d);
        Assert.Equal(0d, displaySol.Flat.Line.Y[1]);

        var control = new GraphPlotControl { Scene = scene };
        Assert.Contains(
            control.Plot.GetPlottables<ScottPlot.Plottables.Scatter>(),
            line => line.Axes.YAxis == control.Plot.Axes.Right &&
                line.LineWidth == GraphPlotControl.MeasuredRemainingLineWidth &&
                line.Data.GetScatterPoints().Any(point => point.X == periodStart && point.Y == 100));
    }

    private static ApiHistorySample Sample(
        long timestamp,
        long resetAt,
        double remaining,
        double solDollars) =>
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
            TaskActiveSincePrevious = false,
            ModelSamples =
            [
                new ApiHistoryModelSample("SOL", null, null, null, solDollars)
                {
                    TotalTokens = (ulong)(solDollars * 100),
                },
                new ApiHistoryModelSample("LUNA", null, null, null, 0) { TotalTokens = 0 },
                new ApiHistoryModelSample("TERRA", null, null, null, 0) { TotalTokens = 0 },
            ],
        };
}
