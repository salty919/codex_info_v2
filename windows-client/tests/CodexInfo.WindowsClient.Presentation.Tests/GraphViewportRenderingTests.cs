// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.Reflection;
using Avalonia;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using SkiaSharp;
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
                    .Where(line => line.LineColor.ToStringRGB() == "#FFFFFF")
                    .ToArray();
                Assert.Equal(expected.Length, midnightGuides.Length);
                var guidePoints = midnightGuides
                    .Select(line => line.Data.GetScatterPoints())
                    .ToArray();
                Assert.All(midnightGuides, line =>
                {
                    Assert.True(line.IsVisible);
                    Assert.Equal(0.5f, line.LineWidth);
                    AssertOpacity(line.LineColor, 0.30);
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
                        $"Expected the translucent local-midnight guide to change rendered pixels for viewport={GetViewportFlag(scene)} at {timestamp} ({x},{y}).");
                }
            }
        }
        finally
        {
            setter.Invoke(null, [priorZone]);
        }
    }

    [Fact]
    public void TopAxisLabelsLocalMidnightsOutsideTheDataRectangleAcrossDaylightSavingTime()
    {
        var zone = FindEasternTimeZone();
        var start = Unix("2026-03-06T17:00:00Z");
        var end = Unix("2026-03-09T16:00:00Z");
        var expected = new[]
        {
            (Timestamp: Unix("2026-03-07T05:00:00Z"), Label: "03/07"),
            (Timestamp: Unix("2026-03-08T05:00:00Z"), Label: "03/08"),
            (Timestamp: Unix("2026-03-09T04:00:00Z"), Label: "03/09"),
        };
        var scene = GraphScene.Create(
        [
            Sample(expected[1].Timestamp - 600, periodEnd: end, sol: 10, taskActiveSincePrevious: false),
            Sample(expected[1].Timestamp + 600, periodEnd: end, sol: 10, taskActiveSincePrevious: false),
        ],
        GraphMetric.Dollars,
        start,
        end);

        var timeZoneProperty = typeof(LocalizationService).GetProperty(nameof(LocalizationService.DisplayTimeZone));
        Assert.NotNull(timeZoneProperty);
        var setter = timeZoneProperty.GetSetMethod(nonPublic: true);
        Assert.NotNull(setter);
        var priorZone = LocalizationService.DisplayTimeZone;
        try
        {
            setter.Invoke(null, [zone]);
            var control = new GraphPlotControl { Scene = scene };
            using var rendered = control.Plot.GetImage(940, 480);

            // ScottPlot's Top axis is the horizontal panel above its DataRect.
            // Midnight dates remain ticks on that axis, not data labels. The
            // font-independent vector marker is drawn by GraphPlotControl's
            // custom Skia operation and is tested separately below.
            var topAxis = control.Plot.Axes.Top;
            Assert.True(topAxis.IsVisible);
            var ticks = topAxis.TickGenerator.Ticks;
            foreach (var (timestamp, date) in expected)
            {
                var tick = Assert.Single(ticks, item => Math.Abs(item.Position - timestamp) < 1e-8);
                Assert.Equal(date, tick.Label);
            }
        }
        finally
        {
            setter.Invoke(null, [priorZone]);
        }
    }

    [Fact]
    public void TopDateMarkersRenderAsVectorPixelsAboveTheDataRectangle()
    {
        var zone = FindEasternTimeZone();
        var start = Unix("2026-03-06T17:00:00Z");
        var end = Unix("2026-03-09T16:00:00Z");
        var scene = GraphScene.Create(
        [
            Sample(Unix("2026-03-08T04:50:00Z"), periodEnd: end, sol: 10, taskActiveSincePrevious: false),
            Sample(Unix("2026-03-08T05:10:00Z"), periodEnd: end, sol: 10, taskActiveSincePrevious: false),
        ],
        GraphMetric.Dollars,
        start,
        end);

        var timeZoneProperty = typeof(LocalizationService).GetProperty(nameof(LocalizationService.DisplayTimeZone));
        Assert.NotNull(timeZoneProperty);
        var setter = timeZoneProperty.GetSetMethod(nonPublic: true);
        Assert.NotNull(setter);
        var priorZone = LocalizationService.DisplayTimeZone;
        try
        {
            setter.Invoke(null, [zone]);
            var control = new GraphPlotControl { Scene = scene };
            const int width = 940;
            const int height = 480;
            using var bitmap = new SKBitmap(width, height, SKColorType.Rgba8888, SKAlphaType.Premul);
            using var canvas = new SKCanvas(bitmap);
            control.Plot.Render(canvas, new ScottPlot.PixelRect(0, width, height, 0));

            var topAxis = control.Plot.Axes.Top;
            var dataRect = control.Plot.LastRender.DataRect;
            var ticks = topAxis.TickGenerator.Ticks.ToArray();
            Assert.Equal(3, ticks.Length);
            Assert.All(ticks, tick => Assert.DoesNotContain('\n', tick.Label));
            var baseline = new SKColor[width * height];
            for (var y = 0; y < height; y++)
            {
                for (var x = 0; x < width; x++)
                {
                    baseline[y * width + x] = bitmap.GetPixel(x, y);
                }
            }

            GraphPlotControl.DrawTopDateMarkers(control.Plot, canvas);

            foreach (var tick in ticks)
            {
                var centerX = topAxis.GetPixel(tick.Position, dataRect);
                var left = Math.Max(0, (int)Math.Floor(centerX - 6));
                var right = Math.Min(width, (int)Math.Ceiling(centerX + 6));
                var top = Math.Max(0, (int)Math.Floor(dataRect.Top - 7));
                var bottom = Math.Min(height, (int)Math.Ceiling(dataRect.Top));
                var changed = new List<(int X, int Y)>();
                for (var y = top; y < bottom; y++)
                {
                    for (var x = left; x < right; x++)
                    {
                        if (bitmap.GetPixel(x, y) != baseline[y * width + x])
                        {
                            changed.Add((x, y));
                        }
                    }
                }

                Assert.True(changed.Count >= 4, $"Expected a visible vector marker at midnight tick {tick.Position}.");
                Assert.All(changed, pixel => Assert.True(pixel.Y + 0.5f < dataRect.Top));
                Assert.Contains(changed, pixel => pixel.Y + 0.5f < dataRect.Top - 3 && pixel.X + 0.5f < centerX - 1.5f);
                Assert.Contains(changed, pixel => pixel.Y + 0.5f < dataRect.Top - 3 && pixel.X + 0.5f > centerX + 1.5f);
                Assert.Contains(changed, pixel => pixel.Y + 0.5f >= dataRect.Top - 3 && Math.Abs(pixel.X + 0.5f - centerX) <= 1.5f);
            }
        }
        finally
        {
            setter.Invoke(null, [priorZone]);
        }
    }

    [Fact]
    public void TopDateAxisUsesTheFullWidthWhenSelectingDayLabels()
    {
        var start = Unix("2026-01-01T00:00:00Z");
        var end = Unix("2026-01-18T00:00:00Z");
        var scene = GraphScene.Create(
        [
            Sample(start + 60, periodEnd: end, sol: 10, taskActiveSincePrevious: false),
            Sample(end - 60, periodEnd: end, sol: 12, taskActiveSincePrevious: false),
        ],
        GraphMetric.Dollars,
        start,
        end);
        const double dataAreaWidth = 800;
        var plotWidth = dataAreaWidth - 2;
        var axes = GraphPlotProjection.BuildAxes(
            scene,
            TimeZoneInfo.Utc,
            CultureInfo.InvariantCulture,
            dataAreaWidth);
        var labelPositions = axes.TopDateValues
            .Select(timestamp => (timestamp - start) / (end - (double)start) * plotWidth)
            .ToArray();

        Assert.Equal(axes.TopDateValues.Count, axes.TopDateLabels.Count);
        Assert.Equal(16, labelPositions.Length);
        Assert.DoesNotContain((double)start, axes.TopDateValues);
        Assert.DoesNotContain((double)end, axes.TopDateValues);
        Assert.All(labelPositions, position => Assert.InRange(position, 20, plotWidth - 20));
        Assert.All(
            labelPositions.Zip(labelPositions.Skip(1), (left, right) => right - left),
            distance => Assert.True(distance >= 45, $"Adjacent month labels are only {distance:F1} px apart."));
    }

    [Fact]
    public void SameDayPeriodKeepsUpperModelTickInsideFigureWithoutDateHeader()
    {
        var start = Unix("2026-10-07T21:10:00Z");
        var end = Unix("2026-10-07T21:50:00Z");
        var scene = GraphScene.Create(
        [
            Sample(start, end, sol: 100, taskActiveSincePrevious: false),
            Sample(start + 1_200, end, sol: 600, taskActiveSincePrevious: false),
            Sample(end, end, sol: 1_800, taskActiveSincePrevious: false),
        ],
        GraphMetric.Tokens,
        start,
        end);
        var timeZoneProperty = typeof(LocalizationService).GetProperty(nameof(LocalizationService.DisplayTimeZone));
        Assert.NotNull(timeZoneProperty);
        var setter = timeZoneProperty.GetSetMethod(nonPublic: true);
        Assert.NotNull(setter);
        var priorZone = LocalizationService.DisplayTimeZone;
        try
        {
            setter.Invoke(null, [TimeZoneInfo.Utc]);
            var control = new GraphPlotControl { Scene = scene };
            using var rendered = control.Plot.GetImage(940, 480);
            var axes = GraphPlotProjection.BuildAxes(scene, TimeZoneInfo.Utc, CultureInfo.InvariantCulture);
            var topAxis = control.Plot.Axes.Top;
            var leftAxis = control.Plot.Axes.Left;
            var upperTick = Assert.Single(
                leftAxis.TickGenerator.Ticks,
                tick => Math.Abs(tick.Position - axes.ModelValues[^1]) < 1e-8);
            var dataTop = control.Plot.LastRender.DataRect.Top;
            var halfTickFontHeight = Math.Ceiling(leftAxis.TickLabelStyle.FontSize / 2d);
            var oneTickFontHeight = Math.Ceiling(leftAxis.TickLabelStyle.FontSize);
            var pixels = rendered.GetArrayRGB();

            Assert.Empty(axes.TopDateValues);
            Assert.Empty(topAxis.TickGenerator.Ticks);
            Assert.Equal(axes.ModelLabels[^1], upperTick.Label);
            Assert.Equal("1.8K", upperTick.Label);
            Assert.Equal(480, pixels.GetLength(0));
            Assert.Equal(940, pixels.GetLength(1));
            Assert.InRange((double)dataTop, halfTickFontHeight, oneTickFontHeight + 4);
        }
        finally
        {
            setter.Invoke(null, [priorZone]);
        }
    }

    [Fact]
    public void TopDateAxisTracksControlWidthAfterResizeAndExpansion()
    {
        var start = Unix("2026-01-01T00:00:00Z");
        var end = Unix("2026-02-01T00:00:00Z");
        var scene = GraphScene.Create(
        [
            Sample(start + 60, periodEnd: end, sol: 10, taskActiveSincePrevious: false),
            Sample(end - 60, periodEnd: end, sol: 12, taskActiveSincePrevious: false),
        ],
        GraphMetric.Dollars,
        start,
        end);
        const double referenceControlWidth = 940;
        const double referenceDataAreaWidth = 788;
        const double narrowControlWidth = 640;
        const double expandedControlWidth = 1_300;
        const double plotHeight = 480;
        var narrowDataAreaWidth = referenceDataAreaWidth + narrowControlWidth - referenceControlWidth;
        var expandedDataAreaWidth = referenceDataAreaWidth + expandedControlWidth - referenceControlWidth;
        var narrowAxes = GraphPlotProjection.BuildAxes(
            scene,
            TimeZoneInfo.Utc,
            CultureInfo.InvariantCulture,
            narrowDataAreaWidth);
        var expandedAxes = GraphPlotProjection.BuildAxes(
            scene,
            TimeZoneInfo.Utc,
            CultureInfo.InvariantCulture,
            expandedDataAreaWidth);
        Assert.True(narrowAxes.TopDateValues.Count < expandedAxes.TopDateValues.Count);

        var timeZoneProperty = typeof(LocalizationService).GetProperty(nameof(LocalizationService.DisplayTimeZone));
        Assert.NotNull(timeZoneProperty);
        var setter = timeZoneProperty.GetSetMethod(nonPublic: true);
        Assert.NotNull(setter);
        var priorZone = LocalizationService.DisplayTimeZone;
        try
        {
            setter.Invoke(null, [TimeZoneInfo.Utc]);
            var control = new GraphPlotControl { Scene = scene };
            SeedResponsiveReferenceLayout(control, referenceControlWidth, scene.Metric, referenceDataAreaWidth);
            control.Measure(new Size(referenceControlWidth, plotHeight));
            control.Arrange(new Rect(0, 0, referenceControlWidth, plotHeight));
            using var referenceImage = control.Plot.GetImage((int)referenceControlWidth, (int)plotHeight);
            var referenceDataRect = control.Plot.LastRender.DataRect;
            var hoverX = control.Plot.GetPixel(
                new ScottPlot.Coordinates(start + 60, 0),
                control.Plot.Axes.Bottom,
                control.Plot.Axes.Left).X;
            control.UpdateHoverAt(new Point(hoverX, (referenceDataRect.Top + referenceDataRect.Bottom) / 2));
            Assert.True(Avalonia.Controls.ToolTip.GetIsOpen(control));
            Assert.NotNull(Avalonia.Controls.ToolTip.GetTip(control));

            control.Measure(new Size(narrowControlWidth, plotHeight));
            control.Arrange(new Rect(0, 0, narrowControlWidth, plotHeight));
            Assert.False(Avalonia.Controls.ToolTip.GetIsOpen(control));
            Assert.Null(Avalonia.Controls.ToolTip.GetTip(control));
            using var narrowImage = control.Plot.GetImage((int)narrowControlWidth, (int)plotHeight);
            var narrowTicksAfterResize = TopDateTickPositions(control);

            control.Measure(new Size(expandedControlWidth, plotHeight));
            control.Arrange(new Rect(0, 0, expandedControlWidth, plotHeight));
            using var expandedImage = control.Plot.GetImage((int)expandedControlWidth, (int)plotHeight);
            var expandedTicksAfterResize = TopDateTickPositions(control);

            Assert.Equal(narrowAxes.TopDateValues, narrowTicksAfterResize);
            Assert.Equal(expandedAxes.TopDateValues, expandedTicksAfterResize);
        }
        finally
        {
            setter.Invoke(null, [priorZone]);
        }
    }

    [Theory]
    [InlineData(700)]
    [InlineData(940)]
    public void RightmostBottomDateLabelRendersFullyInsideFigure(int width)
    {
        const int height = 480;
        var start = Unix("2026-10-01T00:00:00Z");
        var end = Unix("2026-10-08T00:00:00Z");
        var scene = Scene(
            start,
            end,
            (start + 60, 10),
            (end - 60, 12));
        var priorCulture = CultureInfo.CurrentCulture;
        var timeZoneProperty = typeof(LocalizationService).GetProperty(nameof(LocalizationService.DisplayTimeZone));
        Assert.NotNull(timeZoneProperty);
        var setter = timeZoneProperty.GetSetMethod(nonPublic: true);
        Assert.NotNull(setter);
        var priorZone = LocalizationService.DisplayTimeZone;
        try
        {
            CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
            setter.Invoke(null, [TimeZoneInfo.Utc]);
            var control = new GraphPlotControl { Scene = scene };
            using var rendered = control.Plot.GetImage(width, height);

            var bottomAxis = control.Plot.Axes.Bottom;
            var originalGenerator = bottomAxis.TickGenerator;
            var ticks = originalGenerator.Ticks.ToArray();
            var lastTick = ticks[^1];
            Assert.Equal("10/08 00:00", lastTick.Label);
            Assert.Contains("Center", bottomAxis.TickLabelStyle.Alignment.ToString());

            using var typeface = SKTypeface.FromFamilyName(bottomAxis.TickLabelStyle.FontName);
            using var labelFont = new SKFont(typeface, bottomAxis.TickLabelStyle.FontSize);
            var labelHalfWidth = labelFont.MeasureText(lastTick.Label) / 2;
            var dataRect = control.Plot.LastRender.DataRect;
            var tickPixel = bottomAxis.GetPixel(lastTick.Position, dataRect);
            var rightmostLabelPixel = tickPixel + labelHalfWidth;
            var pixelsWithLabel = rendered.GetArrayRGB();
            var rightAxis = control.Plot.Axes.Right;
            Assert.True(rightAxis.IsVisible);
            Assert.All(rightAxis.TickGenerator.Ticks, tick => Assert.Empty(tick.Label));

            bottomAxis.TickGenerator = new ScottPlot.TickGenerators.NumericManual(
                ticks.Select((tick, index) => new ScottPlot.Tick(
                    tick.Position,
                    index == ticks.Length - 1 ? string.Empty : tick.Label,
                    tick.IsMajor)).ToArray());
            byte[,,] pixelsWithoutLastLabel;
            try
            {
                using var withoutLastLabel = control.Plot.GetImage(width, height);
                pixelsWithoutLastLabel = withoutLastLabel.GetArrayRGB();
                Assert.Equal(dataRect.Right, control.Plot.LastRender.DataRect.Right);
            }
            finally
            {
                bottomAxis.TickGenerator = originalGenerator;
            }

            Assert.True(
                rightmostLabelPixel <= width - GraphPlotProjection.PlotStrokeEdgeClearance,
                $"The final date label ends at x={rightmostLabelPixel:0.##}; it needs {GraphPlotProjection.PlotStrokeEdgeClearance}px of clear canvas space before width={width}.");

            var labelInkToRightOfTick = false;
            var firstTextPixel = (int)Math.Ceiling(tickPixel + 3);
            for (var y = (int)Math.Ceiling(dataRect.Bottom); y < height && !labelInkToRightOfTick; y++)
            {
                for (var x = firstTextPixel; x < width; x++)
                {
                    if (pixelsWithLabel[y, x, 0] != pixelsWithoutLastLabel[y, x, 0] ||
                        pixelsWithLabel[y, x, 1] != pixelsWithoutLastLabel[y, x, 1] ||
                        pixelsWithLabel[y, x, 2] != pixelsWithoutLastLabel[y, x, 2])
                    {
                        labelInkToRightOfTick = true;
                        break;
                    }
                }
            }

            Assert.True(labelInkToRightOfTick, "The right-hand half of the final timestamp label must be present in the rendered bitmap.");
        }
        finally
        {
            CultureInfo.CurrentCulture = priorCulture;
            setter.Invoke(null, [priorZone]);
        }
    }

    [Fact]
    public void ResetGuidesUsePublishedPeriodStartsAndOverrideAnOverlappingMidnight()
    {
        var midnight = Unix("2026-03-08T00:00:00Z");
        var start = midnight - 1_800;
        var end = midnight + 1_800;
        var midnightPeriod = CreateResetPeriodScene(midnight, end, midnight + 300);
        var viewport = CreateViewport(start, end, GraphMetric.Dollars,
        [
            midnightPeriod,
            midnightPeriod,
            CreateResetPeriodScene(start - 1, end, midnight + 900),
            CreateResetPeriodScene(midnight + 600, end, end + 1),
            CreateResetPeriodScene(start, end, null, includePublishedStart: false),
            CreateResetPeriodScene(midnight + 1_200, end + 3_600, end + 3_600, empty: true),
            CreateResetPeriodScene(end + 1, end + 3_600, midnight + 1_500),
        ]);

        Assert.NotEqual(midnightPeriod.PeriodEndAt, midnight);

        var timeZoneProperty = typeof(LocalizationService).GetProperty(nameof(LocalizationService.DisplayTimeZone));
        Assert.NotNull(timeZoneProperty);
        var setter = timeZoneProperty.GetSetMethod(nonPublic: true);
        Assert.NotNull(setter);
        var priorZone = LocalizationService.DisplayTimeZone;
        try
        {
            setter.Invoke(null, [TimeZoneInfo.Utc]);
            var control = new GraphPlotControl { Scene = viewport };
            var lines = control.Plot.GetPlottables()
                .OfType<ScottPlot.Plottables.Scatter>()
                .ToArray();
            var resetGuides = lines.Where(line => line.LineColor.ToStringRGB() == "#E6B85C").ToArray();
            Assert.Equal(new double[] { midnight, midnight + 600, midnight + 1_200 },
                resetGuides.Select(line => line.Data.GetScatterPoints().First().X).Order());
            Assert.All(resetGuides, line =>
            {
                Assert.True(line.IsVisible);
                Assert.Equal(1f, line.LineWidth);
                AssertOpacity(line.LineColor, 0.70);
                var points = line.Data.GetScatterPoints();
                Assert.Equal(2, points.Count);
                Assert.Equal(points[0].X, points[1].X);
            });
            Assert.DoesNotContain(lines, line => line.LineColor.ToStringRGB() == "#FFFFFF");
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

    private static void SeedResponsiveReferenceLayout(
        GraphPlotControl control,
        double referenceControlWidth,
        GraphMetric metric,
        double referenceDataAreaWidth)
    {
        var referenceWidthField = typeof(GraphPlotControl).GetField(
            "referenceControlWidth",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(referenceWidthField);
        referenceWidthField.SetValue(control, referenceControlWidth);

        var referenceAreaWidthsField = typeof(GraphPlotControl).GetField(
            "referenceDataAreaWidths",
            BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(referenceAreaWidthsField);
        var referenceAreaWidths = Assert.IsType<Dictionary<GraphMetric, double>>(
            referenceAreaWidthsField.GetValue(control));
        referenceAreaWidths[metric] = referenceDataAreaWidth;
    }

    private static double[] TopDateTickPositions(GraphPlotControl control) =>
        control.Plot.Axes.Top.TickGenerator.Ticks.Select(tick => tick.Position).ToArray();

    private static GraphScene CreateResetPeriodScene(
        long periodStartAt,
        long periodEndAt,
        long? resetAt,
        bool empty = false,
        bool includePublishedStart = true)
    {
        var samples = empty ? Array.Empty<ApiHistorySample>() : new[]
        {
            Sample(periodStartAt, periodEndAt, 1, taskActiveSincePrevious: false),
            Sample(periodEndAt, periodEndAt, 2, taskActiveSincePrevious: false),
        };
        var method = typeof(GraphScene).GetMethods(StaticMembers).SingleOrDefault(candidate =>
        {
            var parameters = candidate.GetParameters();
            return candidate.Name == nameof(GraphScene.Create) &&
                parameters.Any(parameter =>
                    parameter.Name == "resetAt" && parameter.ParameterType == typeof(long?));
        });
        Assert.NotNull(method);

        var parameters = method.GetParameters();
        var arguments = new object?[parameters.Length];
        arguments[0] = samples;
        arguments[1] = GraphMetric.Dollars;
        arguments[2] = periodStartAt;
        arguments[3] = periodEndAt;
        for (var index = 4; index < parameters.Length; index++)
        {
            var parameter = parameters[index];
            arguments[index] = parameter.Name switch
            {
                "confirmedGaps" => null,
                "hiddenModelNames" => null,
                "accountOwnershipIntervals" => null,
                "resetAt" => resetAt,
                "publishedPeriodStartAt" => includePublishedStart ? periodStartAt : null,
                _ when parameter.HasDefaultValue => parameter.DefaultValue,
                _ => null,
            };
        }

        return Assert.IsType<GraphScene>(method.Invoke(null, arguments));
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

    private static void AssertOpacity(ScottPlot.Color color, double expectedOpacity)
    {
        var hex = color.ToHex();
        Assert.Equal(9, hex.Length);
        var actualAlpha = Convert.ToByte(hex[^2..], 16);
        var expectedAlpha = expectedOpacity * byte.MaxValue;
        Assert.InRange((double)actualAlpha, Math.Floor(expectedAlpha), Math.Ceiling(expectedAlpha));
    }

    private static TimeZoneInfo FindEasternTimeZone() => TimeZoneInfo.FindSystemTimeZoneById(
        OperatingSystem.IsWindows() ? "Eastern Standard Time" : "America/New_York");

    private static long Unix(string value) => DateTimeOffset.Parse(value).ToUnixTimeSeconds();
}
