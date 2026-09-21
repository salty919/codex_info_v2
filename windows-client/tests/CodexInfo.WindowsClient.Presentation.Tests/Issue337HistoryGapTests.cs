// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.Reflection;
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
            periodEnd);

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
        Assert.Equal((double)periodStart, displayRemaining.Dashed.Line.X[0]);
        Assert.Equal(100d, displayRemaining.Dashed.Line.Y[0]);
        Assert.StartsWith("M0.00 1.00", displayRemaining.Dashed.Path);

        var sol = GraphPlotProjection.BuildModelLines(scene, scene.ModelSeries["SOL"]);
        Assert.DoesNotContain(
            sol.Idle.X
                .Concat(sol.Flat.X)
                .Concat(sol.Rising.X)
                .Concat(sol.Dashed.X)
                .Where(double.IsFinite),
            timestamp => timestamp < firstObservation);

        var control = new GraphPlotControl { Scene = scene };
        var renderedLeadingQuota = typeof(GraphPlotControl)
            .GetField("remainingDashedSeries", BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new MissingFieldException(typeof(GraphPlotControl).FullName, "remainingDashedSeries");
        Assert.NotNull(renderedLeadingQuota.GetValue(control));
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
