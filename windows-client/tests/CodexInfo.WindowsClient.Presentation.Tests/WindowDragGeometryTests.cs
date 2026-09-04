// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using CodexInfo.WindowsClient;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class WindowDragGeometryTests
{
    [Fact]
    public void Clips_current_graph_period_to_observation_time()
    {
        var period = new ApiHistoryPeriod("2000", 1_000, 2_000, true, "current");

        Assert.Equal(1_500, GraphWindowViewModel.EffectiveGraphEnd(period, 1_500));
        Assert.Equal(2_000, GraphWindowViewModel.EffectiveGraphEnd(period, 2_500));
    }

    [Theory]
    [InlineData(999L, 1_000L)]
    [InlineData(1_000L, 1_000L)]
    [InlineData(2_000L, 2_000L)]
    [InlineData(2_001L, 2_000L)]
    public void Clips_current_graph_period_at_start_and_reset_boundaries(long now, long expectedEnd)
    {
        var period = new ApiHistoryPeriod("2000", 1_000, 2_000, true, "current");

        Assert.Equal(expectedEnd, GraphWindowViewModel.EffectiveGraphEnd(period, now));
    }

    [Fact]
    public void Keeps_historical_graph_period_boundary_intact()
    {
        var period = new ApiHistoryPeriod("2000", 1_000, 2_000, false, "historical");

        Assert.Equal(2_000, GraphWindowViewModel.EffectiveGraphEnd(period, 1_500));
    }

    [Fact]
    public void Graph_samples_anchor_at_period_start_and_current_right_edge()
    {
        var period = new ApiHistoryPeriod("2040", 1_020, 2_040, true, "current")
        {
            Samples =
            [
                new ApiHistorySample(1_380, 2_040, 80, 1, 2, 3, 10, 20, 30),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_620);

        // The admitted row is already a UTC minute-start. A 240-second open
        // local-log gap must not be turned into a current model observation.
        Assert.Equal([1_020L, 1_380L], samples.Select(sample => sample.Timestamp));
        Assert.Equal(100, samples[0].RemainingPercent);
        Assert.Equal(0UL, samples[0].SolTokens);
        Assert.Equal(1, samples[^1].SolDollars);
    }

    [Theory]
    [InlineData(1_019L)]
    [InlineData(1_020L)]
    public void Graph_samples_preserve_admitted_exact_start_at_clamped_current_end(long now)
    {
        var period = new ApiHistoryPeriod("2040", 1_020, 2_040, true, "current")
        {
            Samples =
            [
                new ApiHistorySample(1_020, 2_040, 80, 1, 0, 0, 10, 0, 0),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, now);

        Assert.Equal([1_020L], samples.Select(sample => sample.Timestamp));
        Assert.Equal(1, samples[0].SolDollars);
        Assert.Equal(80, samples[0].RemainingPercent);
    }

    [Fact]
    public void Graph_samples_are_empty_when_period_has_no_history()
    {
        var period = new ApiHistoryPeriod("2000", 1_000, 2_000, false, "historical");

        Assert.Empty(GraphWindowViewModel.BuildGraphSamples(period, 1_500));
    }

    [Fact]
    public void Graph_samples_keep_historical_rows_and_filter_outside_minute_rows()
    {
        var period = new ApiHistoryPeriod("2040", 1_020, 2_040, false, "historical")
        {
            Samples =
            [
                // Adapter-only defensive injections: these minute-aligned
                // rows are outside the admitted period and never reach the
                // graph through Core strict admission.
                new ApiHistorySample(960, 2_040, 1, 1, 0, 0, 1, 0, 0),
                new ApiHistorySample(1_020, 2_040, 80, 2, 0, 0, 2, 0, 0),
                new ApiHistorySample(1_500, 2_040, 70, 3, 0, 0, 3, 0, 0),
                new ApiHistorySample(2_040, 2_040, 60, 99, 0, 0, 99, 0, 0),
                new ApiHistorySample(2_100, 2_040, 50, 100, 0, 0, 100, 0, 0),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_500);

        Assert.Equal([1_020L, 1_500L, 2_040L], samples.Select(sample => sample.Timestamp));
        Assert.Equal(2, samples[0].SolDollars);
        Assert.Equal(3, samples[1].SolDollars);
        Assert.Equal(99, samples[^1].SolDollars);
    }

    [Fact]
    public void Graph_samples_include_the_historical_reset_boundary_observation()
    {
        var period = new ApiHistoryPeriod("2040", 1_020, 2_040, false, "historical")
        {
            Samples =
            [
                new ApiHistorySample(1_020, 2_040, 80, 1, 0, 0, 10, 0, 0),
                new ApiHistorySample(1_980, 2_040, 70, 2, 0, 0, 20, 0, 0),
                new ApiHistorySample(2_040, 2_040, 60, 99, 0, 0, 99, 0, 0),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 2_040);

        Assert.Equal([1_020L, 1_980L, 2_040L], samples.Select(sample => sample.Timestamp));
        Assert.Equal(99, samples[^1].SolDollars);
        Assert.Equal(99UL, samples[^1].SolTokens);
        Assert.Equal(60, samples[^1].RemainingPercent);
    }

    [Fact]
    public void Historical_period_with_only_reset_boundary_sample_keeps_sol_series()
    {
        var period = new ApiHistoryPeriod("2040", 1_020, 2_040, false, "historical")
        {
            Samples =
            [
                // One admitted source row at the canonical period end; the
                // graph may add only its documented synthetic start anchor.
                new ApiHistorySample(2_040, 2_040, 60, 9, 0, 0, 90, 0, 0),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 2_500);

        Assert.Equal([1_020L, 2_040L], samples.Select(sample => sample.Timestamp));
        Assert.Equal(0, samples[0].SolDollars);
        Assert.Equal(9, samples[^1].SolDollars);
        Assert.Equal(90UL, samples[^1].SolTokens);
    }

    [Fact]
    public void Historical_period_keeps_canonicalized_moving_reset_samples()
    {
        var period = new ApiHistoryPeriod("2040", 1_020, 1_260, false, "historical")
        {
            ResetAt = 1_260,
            Samples =
            [
                new ApiHistorySample(1_020, 1_260, 80, 3, 0, 0, 30, 0, 0),
                new ApiHistorySample(1_080, 1_260, 70, 9, 0, 0, 90, 0, 0),
                new ApiHistorySample(1_260, 1_260, 60, 12, 0, 0, 120, 0, 0),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 2_000);

        Assert.Equal([1_020L, 1_080L, 1_260L], samples.Select(sample => sample.Timestamp));
        Assert.Equal(3, samples[0].SolDollars);
        Assert.Equal(9, samples[1].SolDollars);
        Assert.Equal(12, samples[^1].SolDollars);
        Assert.Equal(120UL, samples[^1].SolTokens);
    }

    [Fact]
    public void Graph_samples_keep_each_literal_details_vector_independent()
    {
        var period = new ApiHistoryPeriod("2040", 1_020, 1_200, false, "historical")
        {
            Samples =
            [
                new ApiHistorySample(1_080, 2_000, 90, 2, 1, 0, 20, 10, 0),
                new ApiHistorySample(1_140, 2_000, 80, 1, 3, 0, 10, 30, 0),
                new ApiHistorySample(1_200, 2_000, 70, 4, 2, 1, 40, 20, 10),
            ],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_200);

        Assert.Equal([1_020L, 1_080L, 1_140L, 1_200L], samples.Select(sample => sample.Timestamp));
        Assert.Equal([0d, 2d, 1d, 4d], samples.Select(sample => sample.SolDollars));
        Assert.Equal([0d, 1d, 3d, 2d], samples.Select(sample => sample.TerraDollars));
        Assert.Equal([0UL, 20UL, 10UL, 40UL], samples.Select(sample => sample.SolTokens));
        Assert.Equal([0UL, 10UL, 30UL, 20UL], samples.Select(sample => sample.TerraTokens));
        Assert.Equal([100d, 90d, 80d, 70d], samples.Select(sample => sample.RemainingPercent!.Value));
    }

    [Fact]
    public void Graph_samples_do_not_fabricate_quota_when_all_observations_are_missing()
    {
        var period = new ApiHistoryPeriod("2000", 1_000, 2_000, false, "historical")
        {
            Samples =
            [new ApiHistorySample(1_200, 2_000, null, 1, 0, 0, 10, 0, 0)],
        };

        var samples = GraphWindowViewModel.BuildGraphSamples(period, 1_500);

        Assert.All(samples, sample => Assert.Null(sample.RemainingPercent));
    }

    [Fact]
    public void Configured_connection_skips_intrusive_first_run_setup()
    {
        Assert.True(SetupLaunchPolicy.ShouldOpen(new ClientSettings("ja", false)));
        Assert.False(SetupLaunchPolicy.ShouldOpen(new ClientSettings("ja", false) { ConnectionConfigured = true }));
        Assert.False(SetupLaunchPolicy.ShouldOpen(new ClientSettings("ja", true)));
        Assert.False(SetupLaunchPolicy.ShouldOpen(new ClientSettings("ja", false) { SettingsCorrupt = true }));
    }

    [Fact]
    public void Keeps_position_stable_when_pointer_stays_at_title_point_after_window_moves()
    {
        var start = new PixelPoint(100, 200);
        var startCursor = new PixelPoint(500, 400);
        var movedCursor = new PixelPoint(560, 440);

        var result = WindowDragGeometry.CalculatePosition(start, startCursor, movedCursor);

        Assert.Equal(new PixelPoint(160, 240), result);
    }

    [Fact]
    public void Applies_positive_and_negative_screen_deltas_without_oscillation()
    {
        var start = new PixelPoint(100, 200);
        var startCursor = new PixelPoint(500, 400);

        var first = WindowDragGeometry.CalculatePosition(start, startCursor, new PixelPoint(518, 411));
        var second = WindowDragGeometry.CalculatePosition(start, startCursor, new PixelPoint(530, 420));
        var negative = WindowDragGeometry.CalculatePosition(start, startCursor, new PixelPoint(494, 395));

        Assert.Equal(new PixelPoint(118, 211), first);
        Assert.Equal(new PixelPoint(130, 220), second);
        Assert.Equal(new PixelPoint(94, 195), negative);
    }

    [Fact]
    public void Rounds_high_dpi_coordinates_once_and_preserves_monotonic_drag()
    {
        // GetCursorPos already reports physical screen pixels, so no render-scale
        // conversion or repeated rounding is allowed in the drag calculation.
        var start = new PixelPoint(10, 10);
        var startCursor = new PixelPoint(1000, 1000);
        var first = WindowDragGeometry.CalculatePosition(start, startCursor, new PixelPoint(1001, 1001));
        var stable = WindowDragGeometry.CalculatePosition(start, startCursor, new PixelPoint(1001, 1001));
        var later = WindowDragGeometry.CalculatePosition(start, startCursor, new PixelPoint(1004, 1003));

        Assert.Equal(new PixelPoint(11, 11), first);
        Assert.Equal(first, stable);
        Assert.True(later.X >= stable.X);
        Assert.True(later.Y >= stable.Y);
    }
}
