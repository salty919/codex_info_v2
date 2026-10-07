// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Reflection;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

[CollectionDefinition("Graph viewport rendering", DisableParallelization = true)]
public sealed class GraphViewportRenderingCollection
{
}

[Collection("Graph viewport rendering")]
public sealed class GraphViewportRenderingTests
{
    private const BindingFlags StaticMembers = BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic;

    [Fact]
    public void CreateViewportRetainsResetScenesAndDrawsAxesWhenNoObservationsExist()
    {
        var first = Scene(
            1_000,
            1_600,
            (1_000, 10),
            (1_300, 12),
            (1_600, 14));
        var second = Scene(
            5_000,
            5_600,
            (5_000, 1),
            (5_300, 3),
            (5_600, 5));

        var viewport = CreateViewport(
            1_000,
            5_600,
            GraphMetric.Dollars,
            [first, second]);

        Assert.True(GetViewportFlag(viewport));
        Assert.Equal(1_000, viewport.PeriodStartAt);
        Assert.Equal(5_600, viewport.PeriodEndAt);
        var periods = GetPeriodScenes(viewport);
        Assert.Collection(periods,
            period => Assert.Same(first, period),
            period => Assert.Same(second, period));
        Assert.Equal(1_000, first.PeriodStartAt);
        Assert.Equal(1_600, first.PeriodEndAt);
        Assert.Equal(5_000, second.PeriodStartAt);
        Assert.Equal(5_600, second.PeriodEndAt);

        var emptyViewport = CreateViewport(
            10_000,
            13_600,
            GraphMetric.Tokens,
            Array.Empty<GraphScene>());
        Assert.True(GetViewportFlag(emptyViewport));
        Assert.False(emptyViewport.HasPoints);
        Assert.Empty(GetPeriodScenes(emptyViewport));

        var control = new GraphPlotControl { Scene = emptyViewport };
        Assert.NotEmpty(control.Plot.GetPlottables());
    }

    [Fact]
    public void PreparedCanonicalGeometryIsReusedAndViewportClipsTheActualCurveVertices()
    {
        var scene = Scene(
            1_000,
            10_000,
            (1_000, 2),
            (2_000, 4),
            (3_000, 5),
            (4_000, 9),
            (5_000, 11),
            (6_000, 14),
            (7_000, 18),
            (8_000, 20),
            (9_000, 23),
            (10_000, 25));
        var viewportStart = 2_750L;
        var viewportEnd = 2_900L;
        var viewport = CreateViewport(
            viewportStart,
            viewportEnd,
            GraphMetric.Dollars,
            [scene]);

        var prepared = PrepareGeometry(scene);
        Assert.Same(prepared, PrepareGeometry(scene));
        Assert.True(viewport.HasPoints, "A viewport between raw timestamps still contains the visible canonical curve.");
        Assert.DoesNotContain(scene.Timestamps, timestamp => timestamp >= viewportStart && timestamp <= viewportEnd);

        var fullCanonicalCurve = GraphPlotProjection.BuildCanonicalModelLines(scene, scene.Sol).Rising.Line;
        var expected = ClipCanonicalCurve(
            fullCanonicalCurve,
            viewportStart,
            viewportEnd);
        var actual = BuildViewportModelLines(viewport, GraphSeries.Sol).Rising.Line;

        Assert.Equal(expected.X.Count, actual.X.Count);
        Assert.Equal(expected.Y.Count, actual.Y.Count);
        for (var index = 0; index < expected.X.Count; index++)
        {
            Assert.Equal(expected.X[index], actual.X[index], precision: 8);
            Assert.Equal(expected.Y[index], actual.Y[index], precision: 8);
        }
        Assert.Equal(viewportStart, actual.X[0], precision: 8);
        Assert.Equal(viewportEnd, actual.X[^1], precision: 8);
        Assert.True(viewport.ModelMaximum + 1e-8 >= actual.Y.Where(double.IsFinite).Max(),
            "The viewport axis maximum must contain the canonical curve after clipping at a sparse-data boundary.");
    }

    [Fact]
    public void ResetBoundaryHasNoModelConnectorOrIdleBridge()
    {
        var second = Scene(
            5_000,
            6_200,
            (5_000, 1),
            (5_300, 3),
            (5_600, 5),
            (6_200, 5));
        var firstWithIdleTail = Scene(
            1_000,
            2_200,
            (1_000, 10),
            (1_300, 12),
            (1_600, 14),
            (2_200, 14));
        var viewport = CreateViewport(
            1_000,
            6_200,
            GraphMetric.Dollars,
            [firstWithIdleTail, second]);

        Assert.NotEmpty(firstWithIdleTail.IdleIntervals);
        Assert.NotEmpty(second.IdleIntervals);

        var rising = BuildViewportModelLines(viewport, GraphSeries.Sol).Rising.Line;
        var separators = rising.X
            .Select((value, index) => (value, index))
            .Where(point => !double.IsFinite(point.value))
            .Select(point => point.index)
            .ToArray();
        Assert.NotEmpty(separators);
        Assert.Contains(separators, index => rising.X.Take(index).Any(double.IsFinite) &&
                                               rising.X.Skip(index + 1).Any(double.IsFinite));

        Assert.DoesNotContain(viewport.IdleIntervals, interval =>
            interval.StartAt < second.PeriodStartAt && interval.EndAt > firstWithIdleTail.PeriodEndAt);
    }

    [Fact]
    public void NowRightViewportLeavesTheTailAfterTheLastObservationBlank()
    {
        var observed = Scene(
            0,
            100_000,
            (0, 2),
            (15_000, 6),
            (30_000, 11),
            (45_000, 17),
            (60_000, 24));
        var originalTimestamps = observed.Timestamps.ToArray();
        var viewportStart = 13_600L;
        var now = 100_000L;
        var viewport = CreateViewport(
            viewportStart,
            now,
            GraphMetric.Dollars,
            [observed]);

        Assert.Equal(now, viewport.PeriodEndAt);
        Assert.Equal(originalTimestamps, observed.Timestamps);
        Assert.Equal(0, observed.PeriodStartAt);
        Assert.Equal(now, observed.PeriodEndAt);

        var maxObservedX = observed.Timestamps[^1];
        var projected = BuildViewportModelLines(viewport, GraphSeries.Sol);
        foreach (var path in new[] { projected.Idle.Line, projected.Flat.Line, projected.Rising.Line, projected.Dashed.Line })
        {
            Assert.All(path.X.Where(double.IsFinite), x => Assert.InRange(x, viewportStart, maxObservedX + 1e-8));
        }
        Assert.All(viewport.Timestamps, timestamp => Assert.InRange(timestamp, viewportStart, maxObservedX));

        var remaining = BuildViewportRemainingLines(viewport);
        foreach (var path in new[] { remaining.Idle.Line, remaining.Solid.Line, remaining.Dashed.Line })
        {
            Assert.All(path.X.Where(double.IsFinite), x => Assert.InRange(x, viewportStart, maxObservedX + 1e-8));
        }
    }

    [Fact]
    public void WhiteMidnightGuidesFollowLocalCalendarAcrossDaylightSavingTime()
    {
        var zone = FindEasternTimeZone();
        var start = Unix("2026-03-07T04:00:00Z");
        var end = Unix("2026-03-09T08:00:00Z");
        var expected = new[]
        {
            Unix("2026-03-07T05:00:00Z"),
            Unix("2026-03-08T05:00:00Z"),
            Unix("2026-03-09T04:00:00Z"),
        };

        Assert.Equal(24 * 60 * 60, expected[1] - expected[0]);
        Assert.Equal(23 * 60 * 60, expected[2] - expected[1]);

        var idlePeriod = GraphScene.Create(
        [
            Sample(expected[1] - 600, periodEnd: end, sol: 10, taskActiveSincePrevious: false),
            Sample(expected[1] + 600, periodEnd: end, sol: 10, taskActiveSincePrevious: false),
        ],
        GraphMetric.Dollars,
        start,
        end);
        Assert.Contains(idlePeriod.IdleIntervals, interval =>
            interval.StartAt <= expected[1] && interval.EndAt >= expected[1]);

        var viewport = CreateViewport(start, end, GraphMetric.Dollars, [idlePeriod]);
        Assert.Equal(expected, BuildLocalMidnightGuides(viewport, zone));
        Assert.Equal(expected, BuildLocalMidnightGuides(idlePeriod, zone));

        var timeZoneProperty = typeof(LocalizationService).GetProperty(nameof(LocalizationService.DisplayTimeZone));
        Assert.NotNull(timeZoneProperty);
        var setter = timeZoneProperty.GetSetMethod(nonPublic: true);
        Assert.NotNull(setter);
        var priorZone = LocalizationService.DisplayTimeZone;
        try
        {
            setter.Invoke(null, [zone]);
            foreach (var scene in new[] { idlePeriod, viewport })
            {
                var control = new GraphPlotControl { Scene = scene };
                var midnightGuides = control.Plot.GetPlottables()
                    .OfType<ScottPlot.Plottables.Scatter>()
                    .Where(line => line.LineWidth == 1.5f)
                    .ToArray();
                Assert.Equal(expected.Length, midnightGuides.Length);
                var guidePoints = midnightGuides
                    .Select(line => line.Data.GetScatterPoints())
                    .ToArray();
                Assert.All(midnightGuides, line =>
                {
                    Assert.True(line.IsVisible);
                    Assert.Equal("#FFFFFF", line.LineColor.ToHex());
                });
                Assert.All(guidePoints, points =>
                {
                    Assert.Equal(2, points.Count);
                    Assert.Equal(points[0].X, points[1].X);
                });
                Assert.Equal(
                    expected.Select(timestamp => (double)timestamp).OrderBy(timestamp => timestamp),
                    guidePoints.Select(points => points[0].X).OrderBy(timestamp => timestamp));

                using var rendered = control.Plot.GetImage(940, 480);
                var withGuides = rendered.GetArrayRGB();
                var mappedGuides = expected
                    .Select(timestamp => control.Plot.GetPixel(new ScottPlot.Coordinates(timestamp, 5)))
                    .Select(pixel => ((int)Math.Round(pixel.X), (int)Math.Round(pixel.Y)))
                    .ToArray();
                var previousVisibility = midnightGuides.Select(line => line.IsVisible).ToArray();
                byte[,,] withoutGuides;
                try
                {
                    foreach (var guide in midnightGuides)
                    {
                        guide.IsVisible = false;
                    }
                    using var baseline = control.Plot.GetImage(940, 480);
                    withoutGuides = baseline.GetArrayRGB();
                }
                finally
                {
                    for (var index = 0; index < midnightGuides.Length; index++)
                    {
                        midnightGuides[index].IsVisible = previousVisibility[index];
                    }
                }

                foreach (var timestamp in expected)
                {
                    var index = Array.IndexOf(expected, timestamp);
                    var (x, y) = mappedGuides[index];
                    Assert.True(HasWhiteGuideDifference(withGuides, withoutGuides, x, y),
                        $"Expected the opaque white local-midnight guide to change rendered pixels for viewport={GetViewportFlag(scene)} at {timestamp} ({x},{y}).");
                }
            }
        }
        finally
        {
            setter.Invoke(null, [priorZone]);
        }
    }

    private static GraphScene CreateViewport(
        long startAt,
        long endAt,
        GraphMetric metric,
        IReadOnlyList<GraphScene> periods)
    {
        var method = typeof(GraphScene).GetMethod(
            "CreateViewport",
            BindingFlags.Public | BindingFlags.Static,
            binder: null,
            types: [typeof(long), typeof(long), typeof(GraphMetric), typeof(IReadOnlyList<GraphScene>)],
            modifiers: null);
        Assert.NotNull(method);
        return Assert.IsType<GraphScene>(method.Invoke(null, [startAt, endAt, metric, periods]));
    }

    private static object PrepareGeometry(GraphScene scene)
    {
        var method = typeof(GraphPlotProjection).GetMethod("PrepareGeometry", StaticMembers);
        Assert.NotNull(method);
        return Assert.IsAssignableFrom<object>(method.Invoke(null, [scene]));
    }

    private static GraphCanonicalModelLineProjection BuildViewportModelLines(
        GraphScene viewport,
        GraphSeries series)
    {
        var method = typeof(GraphPlotProjection).GetMethod(
            "BuildViewportModelLines",
            BindingFlags.Static | BindingFlags.NonPublic,
            binder: null,
            types: [typeof(GraphScene), typeof(GraphSeries)],
            modifiers: null);
        Assert.NotNull(method);
        return Assert.IsType<GraphCanonicalModelLineProjection>(method.Invoke(null, [viewport, series]));
    }

    private static GraphCanonicalRemainingLineProjection BuildViewportRemainingLines(GraphScene viewport)
    {
        var method = typeof(GraphPlotProjection).GetMethod(
            "BuildViewportRemainingLines",
            BindingFlags.Static | BindingFlags.NonPublic,
            binder: null,
            types: [typeof(GraphScene)],
            modifiers: null);
        Assert.NotNull(method);
        return Assert.IsType<GraphCanonicalRemainingLineProjection>(method.Invoke(null, [viewport]));
    }

    private static IReadOnlyList<long> BuildLocalMidnightGuides(GraphScene scene, TimeZoneInfo zone)
    {
        var method = typeof(GraphPlotProjection).GetMethod(
            "BuildLocalMidnightGuides",
            BindingFlags.Static | BindingFlags.NonPublic,
            binder: null,
            types: [typeof(GraphScene), typeof(TimeZoneInfo)],
            modifiers: null);
        Assert.NotNull(method);
        return Assert.IsAssignableFrom<IReadOnlyList<long>>(method.Invoke(null, [scene, zone]));
    }

    private static bool GetViewportFlag(GraphScene scene)
    {
        var property = typeof(GraphScene).GetProperty("IsViewport", BindingFlags.Public | BindingFlags.Instance);
        Assert.NotNull(property);
        return Assert.IsType<bool>(property.GetValue(scene));
    }

    private static IReadOnlyList<GraphScene> GetPeriodScenes(GraphScene scene)
    {
        var property = typeof(GraphScene).GetProperty("PeriodScenes", BindingFlags.Public | BindingFlags.Instance);
        Assert.NotNull(property);
        return Assert.IsAssignableFrom<IReadOnlyList<GraphScene>>(property.GetValue(scene));
    }

    private static GraphScene Scene(
        long periodStart,
        long periodEnd,
        params (long Timestamp, double Sol)[] points)
    {
        var samples = points.Select((point, index) => new ApiHistorySample(
            point.Timestamp,
            periodEnd,
            100 - index,
            point.Sol,
            0,
            0,
            (ulong)point.Sol,
            0,
            0,
            ApiHistorySample.ConfirmedModelSource)
        {
            ModelsComplete = true,
            TaskActiveSincePrevious = false,
        }).ToArray();
        return GraphScene.Create(samples, GraphMetric.Dollars, periodStart, periodEnd);
    }

    private static ApiHistorySample Sample(long timestamp, long periodEnd, double sol, bool? taskActiveSincePrevious) =>
        new(
            timestamp,
            periodEnd,
            100,
            sol,
            0,
            0,
            (ulong)sol,
            0,
            0,
            ApiHistorySample.ConfirmedModelSource)
        {
            ModelsComplete = true,
            TaskActiveSincePrevious = taskActiveSincePrevious,
        };

    private static GraphLineProjection ClipCanonicalCurve(
        GraphLineProjection canonical,
        long viewportStart,
        long viewportEnd)
    {
        var clippedX = new List<double>();
        var clippedY = new List<double>();

        void FlushRun()
        {
            if (clippedX.Count > 0 && double.IsFinite(clippedX[^1]))
            {
                clippedX.Add(double.NaN);
                clippedY.Add(double.NaN);
            }
        }

        for (var index = 0; index + 1 < canonical.X.Count; index++)
        {
            var x0 = canonical.X[index];
            var y0 = canonical.Y[index];
            var x1 = canonical.X[index + 1];
            var y1 = canonical.Y[index + 1];
            if (!double.IsFinite(x0) || !double.IsFinite(y0) || !double.IsFinite(x1) || !double.IsFinite(y1))
            {
                FlushRun();
                continue;
            }

            var absoluteX0 = x0;
            var absoluteX1 = x1;
            var left = Math.Max(absoluteX0, viewportStart);
            var right = Math.Min(absoluteX1, viewportEnd);
            if (right < left)
            {
                FlushRun();
                continue;
            }

            var leftRatio = absoluteX1 == absoluteX0 ? 0 : (left - absoluteX0) / (absoluteX1 - absoluteX0);
            var rightRatio = absoluteX1 == absoluteX0 ? 1 : (right - absoluteX0) / (absoluteX1 - absoluteX0);
            AppendPoint(left, y0 + (y1 - y0) * leftRatio);
            AppendPoint(right, y0 + (y1 - y0) * rightRatio);
            if (right < absoluteX1)
            {
                FlushRun();
            }
        }

        while (clippedX.Count > 0 && !double.IsFinite(clippedX[^1]))
        {
            clippedX.RemoveAt(clippedX.Count - 1);
            clippedY.RemoveAt(clippedY.Count - 1);
        }
        return new GraphLineProjection(clippedX, clippedY);

        void AppendPoint(double absoluteX, double y)
        {
            var x = absoluteX;
            if (clippedX.Count > 0 && double.IsFinite(clippedX[^1]) &&
                Math.Abs(clippedX[^1] - x) < 1e-9 && Math.Abs(clippedY[^1] - y) < 1e-9)
            {
                return;
            }
            clippedX.Add(x);
            clippedY.Add(y);
        }
    }

    private static bool HasWhiteGuideDifference(
        byte[,,] withGuides,
        byte[,,] withoutGuides,
        int centerX,
        int centerY)
    {
        for (var y = Math.Max(0, centerY - 1); y <= Math.Min(withGuides.GetLength(0) - 1, centerY + 1); y++)
        {
            for (var x = Math.Max(0, centerX - 1); x <= Math.Min(withGuides.GetLength(1) - 1, centerX + 1); x++)
            {
                if (withGuides[y, x, 0] > withoutGuides[y, x, 0] &&
                    withGuides[y, x, 1] > withoutGuides[y, x, 1] &&
                    withGuides[y, x, 2] > withoutGuides[y, x, 2])
                {
                    return true;
                }
            }
        }
        return false;
    }

    private static TimeZoneInfo FindEasternTimeZone() => TimeZoneInfo.FindSystemTimeZoneById(
        OperatingSystem.IsWindows() ? "Eastern Standard Time" : "America/New_York");

    private static long Unix(string value) => DateTimeOffset.Parse(value).ToUnixTimeSeconds();
}
