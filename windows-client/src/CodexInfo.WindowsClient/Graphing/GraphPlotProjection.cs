// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.Numerics;
using System.Runtime.CompilerServices;
using System.Text;

namespace CodexInfo.WindowsClient.Graphing;

/// <summary>
/// Framework-independent values consumed by the ScottPlot graph adapter.
/// </summary>
internal readonly record struct GraphAxisProjection(
    IReadOnlyList<double> BottomValues,
    IReadOnlyList<long> BottomTimestampValues,
    IReadOnlyList<string> BottomLabels,
    IReadOnlyList<long> MidnightGuideTimestamps,
    IReadOnlyList<double> TopDateValues,
    IReadOnlyList<string> TopDateLabels,
    IReadOnlyList<double> ModelValues,
    IReadOnlyList<string> ModelLabels,
    IReadOnlyList<double> RemainingValues,
    IReadOnlyList<string> RemainingLabels,
    double DisplayEndAt,
    double PlotLimitEndAt,
    double ModelDisplayMinimum,
    double ModelDisplayMaximum,
    double RemainingDisplayMinimum,
    double RemainingDisplayMaximum);

/// <summary>
/// A line path projected without a rendering framework. NaN separators split
/// independent segments so the drawing adapter never joins unrelated lines.
/// </summary>
internal readonly record struct GraphLineProjection(
    IReadOnlyList<double> X,
    IReadOnlyList<double> Y);

/// <summary>Separate X-compatible paths for quiet and changing segments.</summary>
internal readonly record struct GraphModelLineProjection(
    GraphLineProjection Idle,
    GraphLineProjection Flat,
    GraphLineProjection Rising,
    GraphLineProjection Dashed);

/// <summary>Separate solid and reference-only remaining-quota paths.</summary>
internal readonly record struct GraphRemainingLineProjection(
    GraphLineProjection Idle,
    GraphLineProjection Solid,
    GraphLineProjection Dashed);

/// <summary>A finite presentation policy for the leading quota interval.</summary>
internal enum GraphRemainingBaselineMode
{
    None,
    PeriodStartAtFullQuota,
}

/// <summary>
/// A renderer-ready line whose coordinates are quantized through the same
/// 0..100, two-decimal viewbox used by the native Slint graph.
/// </summary>
internal readonly record struct GraphCanonicalLineProjection(
    GraphLineProjection Line,
    string Path);

internal readonly record struct GraphCanonicalModelLineProjection(
    GraphCanonicalLineProjection Idle,
    GraphCanonicalLineProjection Flat,
    GraphCanonicalLineProjection Rising,
    GraphCanonicalLineProjection Dashed);

internal readonly record struct GraphCanonicalRemainingLineProjection(
    GraphCanonicalLineProjection Idle,
    GraphCanonicalLineProjection Solid,
    GraphCanonicalLineProjection Dashed);

internal readonly record struct GraphCanonicalRemainingMarker(
    double X,
    double YTop,
    int Boundary);

/// <summary>Canonical immutable geometry prepared for one accepted period scene.</summary>
internal sealed class GraphPreparedGeometry(
    IReadOnlyDictionary<GraphSeries, GraphCanonicalModelLineProjection> modelLines,
    GraphCanonicalRemainingLineProjection remainingLines,
    IReadOnlyList<GraphCanonicalRemainingMarker> remainingMarkers)
{
    internal IReadOnlyDictionary<GraphSeries, GraphCanonicalModelLineProjection> ModelLines { get; } = modelLines;

    internal GraphCanonicalRemainingLineProjection RemainingLines { get; } = remainingLines;

    internal IReadOnlyList<GraphCanonicalRemainingMarker> RemainingMarkers { get; } = remainingMarkers;
}

internal enum GraphSeries
{
    Remaining,
    Sol,
    Terra,
    Luna,
    Astra,
}

/// <summary>
/// Pure graph presentation semantics.  No Avalonia or ScottPlot types belong
/// here so the boundary can be tested without a windowing environment.
/// </summary>
internal static class GraphPlotProjection
{
    // Y keeps zero/maximum one percent inside the clipped path. These values
    // are the equivalent data-axis expansion: [0, maximum] maps to [1%, 99%].
    private const double AxisPaddingRatio = 1d / 98d;
    private const double CanonicalReferenceDataAreaWidth = 788;
    internal const double PlotStrokeEdgeClearance = 2;
    internal const double MinimumPlotHeight = 204;
    internal const double CanonicalDashLength = 0.45;
    internal const double CanonicalDashGap = 0.30;
    private const double CurveMaximumViewboxStep = 0.1;
    private const double TopDateLabelCharacterWidth = 7;
    private const double TopDateLabelPadding = 4;
    private const double TopDateLabelGap = 6;
    private const double CanonicalGeometryEpsilon = 1e-12;
    private static readonly ConditionalWeakTable<GraphScene, Lazy<GraphPreparedGeometry>> PreparedGeometryCache = new();
    private static readonly object PreparedGeometryCacheLock = new();

    public static GraphAxisProjection BuildAxes(
        GraphScene scene,
        TimeZoneInfo displayTimeZone,
        CultureInfo culture)
    {
        return BuildAxes(
            scene,
            displayTimeZone,
            culture,
            CanonicalReferenceDataAreaWidth);
    }

    /// <summary>
    /// Builds axes across the full data area. The plotted period ends two
    /// pixels before the right frame so a three-pixel stroke remains visible.
    /// </summary>
    public static GraphAxisProjection BuildAxes(
        GraphScene scene,
        TimeZoneInfo displayTimeZone,
        CultureInfo culture,
        double currentDataAreaWidth)
    {
        ArgumentNullException.ThrowIfNull(scene);
        ArgumentNullException.ThrowIfNull(displayTimeZone);
        ArgumentNullException.ThrowIfNull(culture);
        if (!double.IsFinite(currentDataAreaWidth) || currentDataAreaWidth <= 0)
        {
            throw new ArgumentOutOfRangeException(nameof(currentDataAreaWidth));
        }
        var modelPadding = scene.ModelMaximum * AxisPaddingRatio;
        var remainingPadding = 100d * AxisPaddingRatio;
        var modelDisplayRange = scene.ModelMaximum + modelPadding * 2;
        var remainingDisplayRange = 100d + remainingPadding * 2;
        var bottomValues = new double[5];
        var bottomTimestampValues = new long[5];
        var bottomLabels = new string[5];
        var modelValues = new double[5];
        var modelLabels = new string[5];
        var remainingValues = new double[5];
        for (var index = 0; index < 5; index++)
        {
            var ratio = index / 4d;
            var timestamp = scene.PeriodStartAt +
                (long)((scene.PeriodEndAt - scene.PeriodStartAt) * ratio);
            bottomTimestampValues[index] = timestamp;
            bottomValues[index] = scene.PeriodStartAt +
                (scene.PeriodEndAt - scene.PeriodStartAt) * ratio;
            bottomLabels[index] = FormatTimestamp(timestamp, displayTimeZone, culture);
            modelValues[index] = index switch
            {
                0 => -modelPadding,
                4 => scene.ModelMaximum + modelPadding,
                _ => -modelPadding + modelDisplayRange * ratio,
            };
            modelLabels[index] = FormatAxisValue(scene.ModelMaximum * ratio, scene.Metric, culture);
            remainingValues[index] = index switch
            {
                0 => -remainingPadding,
                4 => 100d + remainingPadding,
                _ => -remainingPadding + remainingDisplayRange * ratio,
            };
        }

        var span = Math.Max(1d, scene.PeriodEndAt - scene.PeriodStartAt);
        var currentPlotWidth = currentDataAreaWidth - PlotStrokeEdgeClearance;
        if (currentPlotWidth <= 0)
        {
            throw new ArgumentOutOfRangeException(
                nameof(currentDataAreaWidth),
                "The current data area must leave room for the plotted period and its stroke clearance.");
        }
        var midnightGuideTimestamps = BuildLocalMidnightGuides(scene, displayTimeZone);
        var topDateAxis = BuildTopDateAxis(scene, displayTimeZone, midnightGuideTimestamps, currentPlotWidth);
        var plotLimitEndAt = scene.PeriodStartAt + span * currentDataAreaWidth / currentPlotWidth;

        return new GraphAxisProjection(
            bottomValues,
            bottomTimestampValues,
            bottomLabels,
            midnightGuideTimestamps,
            topDateAxis.Values,
            topDateAxis.Labels,
            modelValues,
            modelLabels,
            remainingValues,
            ["0%", "25%", "50%", "75%", "100%"],
            scene.PeriodEndAt,
            plotLimitEndAt,
            -modelPadding,
            scene.ModelMaximum + modelPadding,
            -remainingPadding,
            100d + remainingPadding);
    }

    /// <summary>
    /// Quantizes measured paths and expands inferred paths into the native
    /// graph's explicit short-dash geometry. ScottPlot line-pattern state is
    /// deliberately not used because its pixel cadence differs by platform.
    /// </summary>
    internal static GraphCanonicalModelLineProjection BuildCanonicalModelLines(
        GraphScene scene,
        IReadOnlyList<double> values)
    {
        var semantic = BuildModelLines(scene, values, smooth: true);
        return new GraphCanonicalModelLineProjection(
            CanonicalizeLine(scene, semantic.Idle, scene.ModelMaximum, remaining: false, dashed: false),
            CanonicalizeLine(scene, semantic.Flat, scene.ModelMaximum, remaining: false, dashed: false),
            CanonicalizeLine(scene, semantic.Rising, scene.ModelMaximum, remaining: false, dashed: false),
            CanonicalizeLine(scene, semantic.Dashed, scene.ModelMaximum, remaining: false, dashed: true));
    }

    internal static GraphCanonicalRemainingLineProjection BuildCanonicalRemainingLines(
        GraphScene scene) =>
        BuildCanonicalRemainingLines(scene, GraphRemainingBaselineMode.None);

    internal static GraphCanonicalRemainingLineProjection BuildCanonicalRemainingLines(
        GraphScene scene,
        GraphRemainingBaselineMode baselineMode)
    {
        var semantic = BuildRemainingLines(scene, smooth: true, baselineMode);
        return new GraphCanonicalRemainingLineProjection(
            CanonicalizeLine(scene, semantic.Idle, 100, remaining: true, dashed: false),
            CanonicalizeLine(scene, semantic.Solid, 100, remaining: true, dashed: false),
            CanonicalizeLine(scene, semantic.Dashed, 100, remaining: true, dashed: true));
    }

    internal static IReadOnlyList<GraphCanonicalRemainingMarker> BuildCanonicalRemainingMarkers(
        GraphScene scene)
    {
        ArgumentNullException.ThrowIfNull(scene);
        // The smooth measured quota path is the only trajectory. Boundary
        // dots derived from unsmoothed integer samples would look like a
        // second line and are intentionally not rendered.
        return Array.Empty<GraphCanonicalRemainingMarker>();
    }

    /// <summary>
    /// Prepares the expensive canonical curves once for this immutable period
    /// scene. Viewport redraws reuse these arrays and only clip their vertices.
    /// </summary>
    internal static GraphPreparedGeometry PrepareGeometry(GraphScene scene)
    {
        ArgumentNullException.ThrowIfNull(scene);
        if (scene.IsViewport)
        {
            throw new ArgumentException("Prepare each reset-period child, not its composite viewport.", nameof(scene));
        }

        Lazy<GraphPreparedGeometry> prepared;
        lock (PreparedGeometryCacheLock)
        {
            if (!PreparedGeometryCache.TryGetValue(scene, out prepared!))
            {
                prepared = new Lazy<GraphPreparedGeometry>(
                    () => CreatePreparedGeometry(scene),
                    LazyThreadSafetyMode.ExecutionAndPublication);
                PreparedGeometryCache.Add(scene, prepared);
            }
        }
        return prepared.Value;
    }

    private static GraphPreparedGeometry CreatePreparedGeometry(GraphScene scene) =>
        new(
            new Dictionary<GraphSeries, GraphCanonicalModelLineProjection>
            {
                [GraphSeries.Astra] = BuildCanonicalModelLines(scene, scene.Astra),
                [GraphSeries.Luna] = BuildCanonicalModelLines(scene, scene.Luna),
                [GraphSeries.Sol] = BuildCanonicalModelLines(scene, scene.Sol),
                [GraphSeries.Terra] = BuildCanonicalModelLines(scene, scene.Terra),
            },
            BuildCanonicalRemainingLines(scene, GraphRemainingBaselineMode.PeriodStartAtFullQuota),
            BuildCanonicalRemainingMarkers(scene));

    internal static GraphCanonicalModelLineProjection BuildViewportModelLines(
        GraphScene viewport,
        GraphSeries series)
    {
        EnsureViewport(viewport);
        if (series is GraphSeries.Remaining)
        {
            throw new ArgumentOutOfRangeException(nameof(series), "Remaining has a separate projection.");
        }

        var periods = new List<GraphCanonicalModelLineProjection>();
        foreach (var child in viewport.PeriodScenes)
        {
            var prepared = PrepareGeometry(child);
            if (!prepared.ModelLines.TryGetValue(series, out var lines))
            {
                continue;
            }
            periods.Add(new GraphCanonicalModelLineProjection(
                ClipPeriodLine(viewport, child, lines.Idle),
                ClipPeriodLine(viewport, child, lines.Flat),
                ClipPeriodLine(viewport, child, lines.Rising),
                ClipPeriodLine(viewport, child, lines.Dashed)));
        }

        return new GraphCanonicalModelLineProjection(
            MergePeriodLines(viewport, periods.Select(lines => lines.Idle)),
            MergePeriodLines(viewport, periods.Select(lines => lines.Flat)),
            MergePeriodLines(viewport, periods.Select(lines => lines.Rising)),
            MergePeriodLines(viewport, periods.Select(lines => lines.Dashed)));
    }

    internal static GraphCanonicalRemainingLineProjection BuildViewportRemainingLines(GraphScene viewport)
    {
        EnsureViewport(viewport);
        var periods = viewport.PeriodScenes
            .Select(child =>
            {
                var lines = PrepareGeometry(child).RemainingLines;
                return new GraphCanonicalRemainingLineProjection(
                    ClipPeriodLine(viewport, child, lines.Idle, remaining: true),
                    ClipPeriodLine(viewport, child, lines.Solid, remaining: true),
                    ClipPeriodLine(viewport, child, lines.Dashed, remaining: true));
            })
            .ToArray();
        return new GraphCanonicalRemainingLineProjection(
            MergePeriodLines(viewport, periods.Select(lines => lines.Idle), remaining: true),
            MergePeriodLines(viewport, periods.Select(lines => lines.Solid), remaining: true),
            MergePeriodLines(viewport, periods.Select(lines => lines.Dashed), remaining: true));
    }

    internal static IReadOnlyList<GraphCanonicalRemainingMarker> BuildViewportRemainingMarkers(GraphScene viewport)
    {
        EnsureViewport(viewport);
        var span = viewport.PeriodEndAt - viewport.PeriodStartAt;
        var markers = new List<GraphCanonicalRemainingMarker>();
        foreach (var child in viewport.PeriodScenes)
        {
            var endAt = VisiblePeriodEnd(viewport, child);
            if (endAt < viewport.PeriodStartAt)
            {
                continue;
            }
            var childSpan = child.PeriodEndAt - child.PeriodStartAt;
            foreach (var marker in PrepareGeometry(child).RemainingMarkers)
            {
                var timestamp = child.PeriodStartAt + marker.X / 100 * childSpan;
                if (timestamp < viewport.PeriodStartAt || timestamp > endAt)
                {
                    continue;
                }
                markers.Add(marker with
                {
                    X = Math.Clamp((timestamp - viewport.PeriodStartAt) / span * 100, 0, 100),
                });
            }
        }
        return markers;
    }

    internal static IReadOnlyList<GraphUnusedInterval> BuildViewportUnusedIntervals(GraphScene viewport)
    {
        EnsureViewport(viewport);
        var intervals = new List<GraphUnusedInterval>();
        foreach (var child in viewport.PeriodScenes)
        {
            foreach (var interval in BuildVisibleUnusedIntervals(child))
            {
                var startAt = Math.Max(viewport.PeriodStartAt, interval.StartAt);
                var endAt = Math.Min(viewport.PeriodEndAt, interval.EndAt);
                if (endAt > startAt)
                {
                    intervals.Add(new GraphUnusedInterval(startAt, endAt));
                }
            }
        }
        return intervals.OrderBy(interval => interval.StartAt).ToArray();
    }

    internal static IReadOnlyList<long> BuildResetGuides(GraphScene scene)
    {
        IReadOnlyList<GraphScene> periods = scene.IsViewport ? scene.PeriodScenes : [scene];
        return periods.Select(period => period.ResetAt)
            .Where(reset => reset.HasValue && reset.Value >= scene.PeriodStartAt && reset.Value <= scene.PeriodEndAt)
            .Select(reset => reset!.Value)
            .Distinct()
            .Order()
            .ToArray();
    }

    internal static IReadOnlyList<long> BuildLocalMidnightGuides(GraphScene scene, TimeZoneInfo displayTimeZone)
    {
        ArgumentNullException.ThrowIfNull(scene);
        ArgumentNullException.ThrowIfNull(displayTimeZone);

        var startAt = scene.PeriodStartAt;
        var endAt = scene.PeriodEndAt;
        DateTimeOffset localStart;
        DateTimeOffset localEnd;
        try
        {
            localStart = TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeSeconds(startAt), displayTimeZone);
            localEnd = TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeSeconds(endAt), displayTimeZone);
        }
        catch (ArgumentOutOfRangeException)
        {
            return Array.Empty<long>();
        }

        var guides = new List<long>();
        for (var date = localStart.Date; date <= localEnd.Date; date = date.AddDays(1))
        {
            var localMidnight = DateTime.SpecifyKind(date, DateTimeKind.Unspecified);
            if (displayTimeZone.IsInvalidTime(localMidnight))
            {
                continue;
            }

            DateTime utcMidnight;
            try
            {
                utcMidnight = TimeZoneInfo.ConvertTimeToUtc(localMidnight, displayTimeZone);
            }
            catch (ArgumentException)
            {
                continue;
            }
            var timestamp = new DateTimeOffset(utcMidnight, TimeSpan.Zero).ToUnixTimeSeconds();
            if (timestamp >= startAt && timestamp <= endAt)
            {
                guides.Add(timestamp);
            }
        }
        return guides;
    }

    private static (IReadOnlyList<double> Values, IReadOnlyList<string> Labels) BuildTopDateAxis(
        GraphScene scene,
        TimeZoneInfo displayTimeZone,
        IReadOnlyList<long> midnightGuideTimestamps,
        double plotWidth)
    {
        var span = Math.Max(1d, scene.PeriodEndAt - scene.PeriodStartAt);
        var selected = new List<(double Position, double HalfWidth, long Timestamp, string Label)>();
        foreach (var timestamp in midnightGuideTimestamps)
        {
            DateTimeOffset localTime;
            try
            {
                localTime = TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeSeconds(timestamp), displayTimeZone);
            }
            catch (ArgumentOutOfRangeException)
            {
                continue;
            }

            var label = localTime.ToString("MM/dd", CultureInfo.InvariantCulture);
            var longestLineLength = label.Split('\n').Max(line => line.Length);
            var halfWidth = (longestLineLength * TopDateLabelCharacterWidth + TopDateLabelPadding) / 2;
            var position = (timestamp - scene.PeriodStartAt) / span * plotWidth;
            if (position < halfWidth || plotWidth - position < halfWidth)
            {
                continue;
            }

            if (selected.Count > 0)
            {
                var previous = selected[^1];
                if (position - previous.Position < previous.HalfWidth + halfWidth + TopDateLabelGap)
                {
                    continue;
                }
            }

            selected.Add((position, halfWidth, timestamp, label));
        }

        return (
            selected.Select(item => (double)item.Timestamp).ToArray(),
            selected.Select(item => item.Label).ToArray());
    }

    internal static bool HasVisibleViewportPoints(
        IReadOnlyList<GraphScene> periodScenes,
        long startAt,
        long endAt)
    {
        foreach (var child in periodScenes)
        {
            if (!child.HasPoints || child.PeriodStartAt > endAt || child.Timestamps[^1] < startAt)
            {
                continue;
            }
            if (HasTimestampInRange(child.Timestamps, startAt, endAt))
            {
                return true;
            }

            var prepared = PrepareGeometry(child);
            var visibleStart = Math.Max(startAt, child.PeriodStartAt);
            var visibleEnd = VisiblePeriodEnd(startAt, endAt, child);
            foreach (var lines in prepared.ModelLines.Values)
            {
                if (visibleEnd >= visibleStart &&
                    (AnyClippedPath(visibleStart, visibleEnd, lines.Idle) ||
                     AnyClippedPath(visibleStart, visibleEnd, lines.Flat) ||
                     AnyClippedPath(visibleStart, visibleEnd, lines.Rising) ||
                     AnyClippedPath(visibleStart, visibleEnd, lines.Dashed)))
                {
                    return true;
                }
            }
            var remaining = prepared.RemainingLines;
            if (visibleEnd >= visibleStart &&
                (AnyClippedPath(visibleStart, visibleEnd, remaining.Idle) ||
                 AnyClippedPath(visibleStart, visibleEnd, remaining.Solid) ||
                 AnyClippedPath(visibleStart, visibleEnd, remaining.Dashed)))
            {
                return true;
            }
        }
        return false;
    }

    internal static double CalculateViewportModelMaximum(
        IReadOnlyList<GraphScene> periodScenes,
        long startAt,
        long endAt)
    {
        var maximum = 0d;
        foreach (var child in periodScenes)
        {
            foreach (var values in child.ModelSeries.Values)
            {
                var firstVisible = FirstTimestampAtOrAfter(child.Timestamps, startAt);
                for (var index = firstVisible; index < values.Count && child.Timestamps[index] <= endAt; index++)
                {
                    if (double.IsFinite(values[index]))
                    {
                        maximum = Math.Max(maximum, values[index]);
                    }
                }
            }

            var visibleEnd = VisiblePeriodEnd(startAt, endAt, child);
            if (visibleEnd < startAt)
            {
                continue;
            }
            foreach (var lines in PrepareGeometry(child).ModelLines.Values)
            {
                maximum = MaxLineY(maximum, ClipLine(lines.Idle.Line, startAt, visibleEnd));
                maximum = MaxLineY(maximum, ClipLine(lines.Flat.Line, startAt, visibleEnd));
                maximum = MaxLineY(maximum, ClipLine(lines.Rising.Line, startAt, visibleEnd));
                maximum = MaxLineY(maximum, ClipLine(lines.Dashed.Line, startAt, visibleEnd));
            }
        }
        return Math.Max(1, maximum);
    }

    private static bool HasTimestampInRange(IReadOnlyList<double> timestamps, long startAt, long endAt)
    {
        var index = FirstTimestampAtOrAfter(timestamps, startAt);
        return index < timestamps.Count && timestamps[index] <= endAt;
    }

    private static int FirstTimestampAtOrAfter(IReadOnlyList<double> timestamps, long timestamp)
    {
        var low = 0;
        var high = timestamps.Count;
        while (low < high)
        {
            var middle = low + (high - low) / 2;
            if (timestamps[middle] < timestamp)
            {
                low = middle + 1;
            }
            else
            {
                high = middle;
            }
        }
        return low;
    }

    private static void EnsureViewport(GraphScene viewport)
    {
        ArgumentNullException.ThrowIfNull(viewport);
        if (!viewport.IsViewport)
        {
            throw new ArgumentException("A viewport projection requires a composite viewport scene.", nameof(viewport));
        }
    }

    private static long VisiblePeriodEnd(GraphScene viewport, GraphScene child) =>
        VisiblePeriodEnd(viewport.PeriodStartAt, viewport.PeriodEndAt, child);

    private static long VisiblePeriodEnd(long startAt, long endAt, GraphScene child)
    {
        if (!child.HasPoints)
        {
            return long.MinValue;
        }
        var observedEnd = (long)Math.Floor(child.Timestamps[^1]);
        if (observedEnd < startAt || child.PeriodStartAt > endAt)
        {
            return long.MinValue;
        }
        return Math.Min(endAt, Math.Min(child.PeriodEndAt, observedEnd));
    }

    private static GraphCanonicalLineProjection ClipPeriodLine(
        GraphScene viewport,
        GraphScene child,
        GraphCanonicalLineProjection source,
        bool remaining = false)
    {
        var startAt = Math.Max(viewport.PeriodStartAt, child.PeriodStartAt);
        var endAt = VisiblePeriodEnd(viewport, child);
        if (endAt < startAt)
        {
            return new GraphCanonicalLineProjection(new GraphLineProjection([], []), string.Empty);
        }
        var line = ClipLine(source.Line, startAt, endAt);
        return new GraphCanonicalLineProjection(
            line,
            BuildViewportPath(line, viewport, remaining));
    }

    private static GraphCanonicalLineProjection MergePeriodLines(
        GraphScene viewport,
        IEnumerable<GraphCanonicalLineProjection> source,
        bool remaining = false)
    {
        var x = new List<double>();
        var y = new List<double>();
        foreach (var projection in source)
        {
            var line = projection.Line;
            if (!line.X.Any(double.IsFinite))
            {
                continue;
            }
            if (x.Count > 0 && double.IsFinite(x[^1]))
            {
                x.Add(double.NaN);
                y.Add(double.NaN);
            }
            x.AddRange(line.X);
            y.AddRange(line.Y);
            while (x.Count > 0 && !double.IsFinite(x[^1]))
            {
                x.RemoveAt(x.Count - 1);
                y.RemoveAt(y.Count - 1);
            }
        }
        var merged = new GraphLineProjection(x, y);
        return new GraphCanonicalLineProjection(merged, BuildViewportPath(merged, viewport, remaining));
    }

    private static GraphLineProjection ClipLine(
        GraphLineProjection source,
        double startAt,
        double endAt)
    {
        if (source.X.Count != source.Y.Count)
        {
            throw new ArgumentException("Line coordinate arrays must have the same length.", nameof(source));
        }

        var runs = new List<List<CanonicalPoint>>();
        List<CanonicalPoint>? current = null;

        void CloseRun()
        {
            if (current is { Count: >= 2 })
            {
                runs.Add(current);
            }
            current = null;
        }

        for (var index = 0; index + 1 < source.X.Count; index++)
        {
            var x0 = source.X[index];
            var y0 = source.Y[index];
            var x1 = source.X[index + 1];
            var y1 = source.Y[index + 1];
            if (!double.IsFinite(x0) || !double.IsFinite(y0) ||
                !double.IsFinite(x1) || !double.IsFinite(y1))
            {
                CloseRun();
                continue;
            }

            var leftX = Math.Max(startAt, Math.Min(x0, x1));
            var rightX = Math.Min(endAt, Math.Max(x0, x1));
            if (rightX < leftX)
            {
                CloseRun();
                continue;
            }

            var deltaX = x1 - x0;
            var leftRatio = Math.Abs(deltaX) <= CanonicalGeometryEpsilon ? 0 : (leftX - x0) / deltaX;
            var rightRatio = Math.Abs(deltaX) <= CanonicalGeometryEpsilon ? 1 : (rightX - x0) / deltaX;
            var leftPoint = new CanonicalPoint(leftX, y0 + (y1 - y0) * leftRatio);
            var rightPoint = new CanonicalPoint(rightX, y0 + (y1 - y0) * rightRatio);

            if (current is null || !SamePoint(current[^1], leftPoint))
            {
                CloseRun();
                current = [leftPoint];
            }
            if (!SamePoint(current[^1], rightPoint))
            {
                current.Add(rightPoint);
            }

            if (rightX < Math.Max(x0, x1))
            {
                CloseRun();
            }
        }
        CloseRun();

        var outputX = new List<double>();
        var outputY = new List<double>();
        foreach (var run in runs)
        {
            if (outputX.Count > 0)
            {
                outputX.Add(double.NaN);
                outputY.Add(double.NaN);
            }
            foreach (var point in run)
            {
                outputX.Add(point.X);
                outputY.Add(point.YTop);
            }
        }
        return new GraphLineProjection(outputX, outputY);
    }

    private static bool SamePoint(CanonicalPoint left, CanonicalPoint right) =>
        Math.Abs(left.X - right.X) <= CanonicalGeometryEpsilon &&
        Math.Abs(left.YTop - right.YTop) <= CanonicalGeometryEpsilon;

    private static bool AnyClippedPath(double startAt, double endAt, GraphCanonicalLineProjection line) =>
        AnyClippedPath(startAt, endAt, line.Line);

    private static bool AnyClippedPath(double startAt, double endAt, GraphLineProjection line) =>
        ClipLine(line, startAt, endAt).X.Count >= 2;

    private static double MaxLineY(double maximum, GraphLineProjection line) =>
        Math.Max(maximum, line.Y.Where(double.IsFinite).DefaultIfEmpty(0).Max());

    private static string BuildViewportPath(GraphLineProjection line, GraphScene viewport, bool remaining)
    {
        var span = viewport.PeriodEndAt - viewport.PeriodStartAt;
        var maximum = remaining ? 100 : Math.Max(viewport.ModelMaximum, 1);
        var path = new StringBuilder();
        var startRun = true;
        for (var index = 0; index < line.X.Count; index++)
        {
            var x = line.X[index];
            var y = line.Y[index];
            if (!double.IsFinite(x) || !double.IsFinite(y))
            {
                startRun = true;
                continue;
            }
            var normalizedX = Math.Clamp((x - viewport.PeriodStartAt) / span * 100, 0, 100);
            var yTop = remaining
                ? Math.Clamp(99 - Math.Clamp(y, 0, 100) * 0.98, 1, 99)
                : Math.Clamp(99 - Math.Max(y, 0) / maximum * 98, 1, 99);
            var roundedX = RoundCanonicalValue(normalizedX);
            var roundedY = RoundCanonicalValue(yTop);
            if (path.Length > 0)
            {
                path.Append(' ');
            }
            path.Append(CultureInfo.InvariantCulture,
                $"{(startRun ? "M" : "L")}{roundedX:0.00} {roundedY:0.00}");
            startRun = false;
        }
        return path.ToString();
    }

    private static bool AnyClippedPath(double startAt, double endAt, GraphCanonicalModelLineProjection lines) =>
        AnyClippedPath(startAt, endAt, lines.Idle) ||
        AnyClippedPath(startAt, endAt, lines.Flat) ||
        AnyClippedPath(startAt, endAt, lines.Rising) ||
        AnyClippedPath(startAt, endAt, lines.Dashed);

    /// <summary>
    /// Builds the quota path while keeping remote observations independent
    /// from model availability. Only explicit missing or anomalous intervals
    /// are emitted into the dashed prediction path.
    /// </summary>
    public static GraphRemainingLineProjection BuildRemainingLines(GraphScene scene) =>
        BuildRemainingLines(scene, smooth: false, GraphRemainingBaselineMode.None);

    private static GraphRemainingLineProjection BuildRemainingLines(
        GraphScene scene,
        bool smooth,
        GraphRemainingBaselineMode baselineMode)
    {
        ArgumentNullException.ThrowIfNull(scene);
        if (baselineMode is not GraphRemainingBaselineMode.None and
            not GraphRemainingBaselineMode.PeriodStartAtFullQuota)
        {
            throw new ArgumentOutOfRangeException(nameof(baselineMode));
        }
        if (!scene.HasPoints)
        {
            return new GraphRemainingLineProjection(
                new GraphLineProjection([], []),
                new GraphLineProjection([], []),
                new GraphLineProjection([], []));
        }

        var firstRenderable = scene.Remaining
            .Select((value, index) => double.IsFinite(value) ? index : -1)
            .FirstOrDefault(index => index >= 0, -1);
        if (firstRenderable < 0 || !scene.RemainingObserved.Any(observed => observed))
        {
            return new GraphRemainingLineProjection(
                new GraphLineProjection([], []),
                new GraphLineProjection([], []),
                new GraphLineProjection([], []));
        }

        var idleX = new List<double>();
        var idleY = new List<double>();
        var solidX = new List<double>();
        var solidY = new List<double>();
        var dashedX = new List<double>();
        var dashedY = new List<double>();
        var anchors = Enumerable.Range(firstRenderable, scene.Timestamps.Count - firstRenderable)
            .Where(index => double.IsFinite(scene.Remaining[index]) &&
                scene.RemainingOrigins[index] is GraphRemainingOrigin.Raw)
            .ToArray();
        var smoothableIntervals = new List<(int Left, int Right, bool Dashed)>();
        if (baselineMode is GraphRemainingBaselineMode.PeriodStartAtFullQuota &&
            anchors.Length > 0 &&
            scene.RemainingObserved[anchors[0]] &&
            scene.Timestamps[anchors[0]] > scene.PeriodStartAt)
        {
            // Full quota at the period boundary is a renderer-only convention.
            // Keep it out of GraphScene's raw/history arrays and visibly infer
            // only the interval leading to the first accepted observation.
            AppendSegment(
                dashedX,
                dashedY,
                scene.PeriodStartAt,
                100,
                scene.Timestamps[anchors[0]],
                scene.Remaining[anchors[0]]);
        }
        for (var anchor = 1; anchor < anchors.Length; anchor++)
        {
            var left = anchors[anchor - 1];
            var right = anchors[anchor];
            if (scene.OverlapsNonOwnedInterval(scene.Timestamps[left], scene.Timestamps[right]))
            {
                AppendSegment(
                    dashedX,
                    dashedY,
                    scene.Timestamps[left],
                    RemainingValue(scene, left),
                    scene.Timestamps[right],
                    RemainingValue(scene, right));
                continue;
            }
            var before = RemainingValue(scene, left);
            var current = RemainingValue(scene, right);
            var crossesPrediction = Enumerable.Range(left + 1, right - left - 1)
                .Any(index => scene.RemainingOrigins[index] is not GraphRemainingOrigin.Raw);
            var measured = RemainingOriginHasMeasuredQuota(scene.RemainingOrigins[left]) &&
                RemainingOriginHasMeasuredQuota(scene.RemainingOrigins[right]);
            if (current > before)
            {
                AppendSegment(
                    dashedX,
                    dashedY,
                    scene.Timestamps[left],
                    before,
                    scene.Timestamps[right],
                    before);
            }
            else
            {
                var dashed = scene.HasRemainingHardBreakBetween(
                        scene.Timestamps[left],
                        scene.Timestamps[right]) ||
                    crossesPrediction ||
                    IsLongUnobservedTokenIncrease(scene, left, right) ||
                    !measured;
                if (!dashed && SameDoubleBits(current, before) && IsConfirmedIdleInterval(
                        scene.IdleIntervals,
                        scene.Timestamps[left],
                        scene.Timestamps[right]))
                {
                    AppendSegment(
                        idleX,
                        idleY,
                        scene.Timestamps[left],
                        before,
                        scene.Timestamps[right],
                        current);
                }
                else
                {
                    smoothableIntervals.Add((left, right, dashed));
                }
            }
        }
        if (smooth)
        {
            AppendSmoothedRemainingRuns(
                scene,
                scene.Remaining,
                smoothableIntervals,
                solidX,
                solidY,
                dashedX,
                dashedY);
        }
        else
        {
            foreach (var interval in smoothableIntervals)
            {
                AppendSegment(
                    interval.Dashed ? dashedX : solidX,
                    interval.Dashed ? dashedY : solidY,
                    scene.Timestamps[interval.Left],
                    scene.Remaining[interval.Left],
                    scene.Timestamps[interval.Right],
                    scene.Remaining[interval.Right]);
            }
        }

        var previous = anchors.LastOrDefault(-1);
        if (previous >= 0 && scene.PeriodEndAt > scene.Timestamps[previous] &&
            !scene.OverlapsNonOwnedInterval(scene.Timestamps[previous], scene.PeriodEndAt))
        {
            // The remote source may stop while the current period continues.
            // Keep only the last measured value, horizontally and dashed;
            // never extend the local model vector to manufacture a current
            // observation or a consumption rate.
            var lastKnown = RemainingValue(scene, previous);
            AppendSegment(
                dashedX,
                dashedY,
                scene.Timestamps[previous],
                lastKnown,
                scene.PeriodEndAt,
                lastKnown);
        }

        return new GraphRemainingLineProjection(
            new GraphLineProjection(idleX, idleY),
            new GraphLineProjection(solidX, solidY),
            new GraphLineProjection(dashedX, dashedY));
    }

    private static bool RemainingOriginHasMeasuredQuota(GraphRemainingOrigin origin) =>
        origin is GraphRemainingOrigin.Raw;

    private static bool SameDoubleBits(double left, double right) =>
        BitConverter.DoubleToInt64Bits(left) == BitConverter.DoubleToInt64Bits(right);

    /// <summary>
    /// Projects the flat, rising, and inferred cumulative-model paths used by
    /// the renderer. Confirmed gaps remain disconnected.
    /// </summary>
    internal static GraphModelLineProjection BuildModelLines(
        GraphScene scene,
        IReadOnlyList<double> values) =>
        BuildModelLines(scene, values, smooth: false);

    private static GraphModelLineProjection BuildModelLines(
        GraphScene scene,
        IReadOnlyList<double> values,
        bool smooth)
    {
        ArgumentNullException.ThrowIfNull(scene);
        ArgumentNullException.ThrowIfNull(values);
        if (values.Count != scene.Timestamps.Count)
        {
            throw new ArgumentException("A model series must match the graph timestamp count.", nameof(values));
        }

        var idleX = new List<double>();
        var idleY = new List<double>();
        var flatX = new List<double>();
        var flatY = new List<double>();
        var risingX = new List<double>();
        var risingY = new List<double>();
        var dashedX = new List<double>();
        var dashedY = new List<double>();
        var idleIntervals = scene.IdleIntervalsForModel(values);
        var anchors = Enumerable.Range(0, values.Count)
            .Where(index => double.IsFinite(values[index]) && values[index] >= 0 &&
                !scene.ModelSynthetic[index] &&
                scene.IsModelIntervalReliable(values, index, index))
            .ToArray();
        var smoothableIntervals = new List<(int Left, int Right, ProjectionStyle Style)>();
        for (var anchor = 1; anchor < anchors.Length; anchor++)
        {
            var left = anchors[anchor - 1];
            var right = anchors[anchor];
            if (scene.OverlapsNonOwnedInterval(scene.Timestamps[left], scene.Timestamps[right]))
            {
                AppendSegment(
                    dashedX,
                    dashedY,
                    scene.Timestamps[left],
                    values[left],
                    scene.Timestamps[right],
                    values[right]);
                continue;
            }
            var before = values[left];
            var current = values[right];
            var confirmedIdle = IsConfirmedIdleInterval(
                idleIntervals,
                scene.Timestamps[left],
                scene.Timestamps[right]);
            if (!confirmedIdle &&
                (!scene.IsModelIntervalReliable(values, left, left) ||
                 !scene.IsModelIntervalReliable(values, right, right)))
            {
                continue;
            }
            var crossesPrediction = Enumerable.Range(left + 1, right - left - 1)
                .Any(index => !scene.IsModelIntervalReliable(values, index, index));
            var crossesCorrection = scene.HasModelCorrectionBetween(
                values,
                scene.Timestamps[left],
                scene.Timestamps[right]);
            if (confirmedIdle && !crossesPrediction)
            {
                if (scene.HasModelTokenCountChange(values, left, right))
                {
                    // Keep the measured endpoints connected, but show a
                    // low-rate change as uncertain rather than active use.
                    AppendSegment(
                        dashedX,
                        dashedY,
                        scene.Timestamps[left],
                        before,
                        scene.Timestamps[right],
                        current);
                }
                else
                {
                    AppendSegment(
                        idleX,
                        idleY,
                        scene.Timestamps[left],
                        before,
                        scene.Timestamps[right],
                        before);
                }
            }
            else if (current < before)
            {
                AppendSegment(
                    dashedX,
                    dashedY,
                    scene.Timestamps[left],
                    before,
                    scene.Timestamps[right],
                    before);
            }
            else if (crossesCorrection)
            {
                AppendSegment(
                    dashedX,
                    dashedY,
                    scene.Timestamps[left],
                    before,
                    scene.Timestamps[right],
                    current);
            }
            else
            {
                var dashed = scene.HasConfirmedGapBetween(
                        scene.Timestamps[left],
                        scene.Timestamps[right]) ||
                    IsLongUnobservedTokenIncrease(scene, left, right) ||
                    crossesPrediction;
                smoothableIntervals.Add((
                    left,
                    right,
                    dashed
                        ? ProjectionStyle.Dashed
                        : current == before
                            ? ProjectionStyle.Flat
                            : ProjectionStyle.Rising));
            }
        }
        if (smooth)
        {
            AppendSmoothedModelRuns(
                scene,
                values,
                smoothableIntervals,
                flatX,
                flatY,
                risingX,
                risingY,
                dashedX,
                dashedY);
        }
        else
        {
            foreach (var interval in smoothableIntervals)
            {
                var (targetX, targetY) = interval.Style switch
                {
                    ProjectionStyle.Flat => (flatX, flatY),
                    ProjectionStyle.Rising => (risingX, risingY),
                    ProjectionStyle.Dashed => (dashedX, dashedY),
                    _ => throw new InvalidOperationException("Unknown graph projection style."),
                };
                AppendSegment(
                    targetX,
                    targetY,
                    scene.Timestamps[interval.Left],
                    values[interval.Left],
                    scene.Timestamps[interval.Right],
                    values[interval.Right]);
            }
        }

        var previous = anchors.LastOrDefault(-1);
        if (previous >= 0 && scene.PeriodEndAt > scene.Timestamps[previous] &&
            !scene.OverlapsNonOwnedInterval(scene.Timestamps[previous], scene.PeriodEndAt))
        {
            AppendSegment(
                dashedX,
                dashedY,
                scene.Timestamps[previous],
                values[previous],
                scene.PeriodEndAt,
                values[previous]);
        }

        return new GraphModelLineProjection(
            new GraphLineProjection(idleX, idleY),
            new GraphLineProjection(flatX, flatY),
            new GraphLineProjection(risingX, risingY),
            new GraphLineProjection(dashedX, dashedY));
    }

    private static bool IsConfirmedIdleInterval(
        IReadOnlyList<GraphIdleInterval> intervals,
        double startAt,
        double endAt) =>
        intervals.Any(interval =>
            startAt >= interval.StartAt && endAt <= interval.EndAt);

    private static bool IsConfirmedIdleTimestamp(
        IReadOnlyList<GraphIdleInterval> intervals,
        double timestamp) =>
        intervals.Any(interval =>
            timestamp >= interval.StartAt && timestamp <= interval.EndAt);

    private static bool IsLongUnobservedTokenIncrease(GraphScene scene, int left, int right) =>
        scene.Metric == GraphMetric.Tokens &&
        scene.Timestamps[right] - scene.Timestamps[left] >= GraphScene.SustainedUnusedMinimumSeconds &&
        scene.TryGetTokenIntervalEvidence(left, right, out var advanced) &&
        advanced;

    /// <summary>Returns every evidence interval without a pixel-width filter.</summary>
    public static IReadOnlyList<GraphIdleInterval> BuildVisibleIdleIntervals(GraphScene scene)
    {
        ArgumentNullException.ThrowIfNull(scene);
        return scene.IdleIntervals.ToArray();
    }

    public static IReadOnlyList<GraphUnusedInterval> BuildVisibleUnusedIntervals(GraphScene scene)
    {
        ArgumentNullException.ThrowIfNull(scene);
        var intervals = scene.IdleIntervals
            .Select(interval => new GraphUnusedInterval(interval.StartAt, interval.EndAt))
            .OrderBy(interval => interval.StartAt);
        var merged = new List<GraphUnusedInterval>();
        foreach (var interval in intervals)
        {
            if (merged.Count > 0 && interval.StartAt <= merged[^1].EndAt)
            {
                merged[^1] = merged[^1] with { EndAt = Math.Max(interval.EndAt, merged[^1].EndAt) };
            }
            else
            {
                merged.Add(interval);
            }
        }
        return merged;
    }

    internal static string FormatAxisValue(double value, GraphMetric metric, CultureInfo culture)
    {
        ArgumentNullException.ThrowIfNull(culture);
        if (metric == GraphMetric.Dollars)
        {
            return "$" + FormatExactBinary(value, 2, culture);
        }

        if (Math.Abs(value) >= 1_000_000_000)
        {
            return FormatExactBinary(value / 1_000_000_000, 1, culture) + "B";
        }

        if (Math.Abs(value) >= 1_000_000)
        {
            return FormatExactBinary(value / 1_000_000, 1, culture) + "M";
        }

        if (Math.Abs(value) >= 1_000)
        {
            return FormatExactBinary(value / 1_000, 1, culture) + "K";
        }

        return RoundUnsignedCount(value).ToString("N0", culture);
    }

    private static ulong RoundUnsignedCount(double value)
    {
        var rounded = Math.Round(Math.Max(0, value), MidpointRounding.AwayFromZero);
        return rounded >= ulong.MaxValue ? ulong.MaxValue : (ulong)rounded;
    }

    private static string FormatExactBinary(double value, int decimalPlaces, CultureInfo culture) =>
        RoundExactBinary(value, decimalPlaces).ToString($"F{decimalPlaces}", culture);

    private static string FormatTimestamp(long timestamp, TimeZoneInfo displayTimeZone, CultureInfo culture) =>
        TimeZoneInfo.ConvertTime(
                DateTimeOffset.FromUnixTimeSeconds(timestamp),
                displayTimeZone)
            .ToString("MM/dd HH:mm", culture);

    private static double RemainingValue(GraphScene scene, int index)
    {
        var effective = scene.Remaining[index];
        var observed = scene.ObservedRemainingValues[index];
        if (!scene.RemainingObserved[index] || !double.IsFinite(observed))
        {
            return effective;
        }
        // GraphScene has already rejected quota pulses and applied bounded
        // smoothing. Reintroducing a lower raw pulse here would draw the exact
        // false valley that the evidence projection rejected.
        return double.IsFinite(effective) ? effective : observed;
    }

    private static GraphCanonicalLineProjection CanonicalizeLine(
        GraphScene scene,
        GraphLineProjection source,
        double maximum,
        bool remaining,
        bool dashed)
    {
        ArgumentNullException.ThrowIfNull(scene);
        if (source.X.Count != source.Y.Count)
        {
            throw new ArgumentException("Line coordinate arrays must have the same length.", nameof(source));
        }
        var x = new List<double>();
        var y = new List<double>();
        var path = new StringBuilder();
        var run = new List<CanonicalPoint>();

        void FlushRun()
        {
            if (run.Count < 2)
            {
                run.Clear();
                return;
            }
            if (dashed)
            {
                AppendCanonicalDashes(scene, x, y, path, run, maximum, remaining);
            }
            else
            {
                for (var index = 1; index < run.Count; index++)
                {
                    AppendCanonicalSegment(
                        scene,
                        x,
                        y,
                        path,
                        run[index - 1],
                        run[index],
                        maximum,
                        remaining,
                        continuePath: index > 1);
                }
            }
            run.Clear();
        }

        for (var index = 0; index < source.X.Count; index++)
        {
            if (!double.IsFinite(source.X[index]) || !double.IsFinite(source.Y[index]))
            {
                FlushRun();
                continue;
            }
            run.Add(CanonicalPointFor(
                scene,
                source.X[index],
                source.Y[index],
                maximum,
                remaining));
        }
        FlushRun();
        return new GraphCanonicalLineProjection(
            new GraphLineProjection(x, y),
            path.ToString());
    }

    private static CanonicalPoint CanonicalPointFor(
        GraphScene scene,
        double timestamp,
        double value,
        double maximum,
        bool remaining)
    {
        var span = Math.Max(1d, scene.PeriodEndAt - scene.PeriodStartAt);
        var x = Math.Clamp((timestamp - scene.PeriodStartAt) / span * 100, 0, 100);
        var yTop = remaining
            ? Math.Clamp(99 - Math.Clamp(value, 0, 100) * 0.98, 1, 99)
            : Math.Clamp(99 - Math.Max(value, 0) / Math.Max(maximum, 1) * 98, 1, 99);
        return new CanonicalPoint(
            Math.Round(x, 12, MidpointRounding.ToEven),
            Math.Round(yTop, 12, MidpointRounding.ToEven));
    }

    private static void AppendCanonicalDashes(
        GraphScene scene,
        List<double> x,
        List<double> y,
        StringBuilder path,
        IReadOnlyList<CanonicalPoint> points,
        double maximum,
        bool remaining)
    {
        var period = CanonicalDashLength + CanonicalDashGap;
        var phase = 0d;
        for (var index = 1; index < points.Count; index++)
        {
            var start = points[index - 1];
            var end = points[index];
            var dx = end.X - start.X;
            var dy = end.YTop - start.YTop;
            var length = Math.Sqrt(dx * dx + dy * dy);
            if (!double.IsFinite(length) || length <= CanonicalGeometryEpsilon)
            {
                continue;
            }
            var offset = 0d;
            while (offset < length)
            {
                var inDash = phase < CanonicalDashLength;
                var phaseEnd = inDash ? CanonicalDashLength : period;
                var advance = Math.Min(phaseEnd - phase, length - offset);
                if (advance <= CanonicalGeometryEpsilon)
                {
                    var residualLength = length - offset;
                    if (residualLength <= CanonicalGeometryEpsilon)
                    {
                        phase += residualLength;
                        if (phase >= period - CanonicalGeometryEpsilon)
                        {
                            phase = 0;
                        }
                        else if (Math.Abs(phase - CanonicalDashLength) <= CanonicalGeometryEpsilon)
                        {
                            phase = CanonicalDashLength;
                        }
                        offset = length;
                        continue;
                    }

                    phase = phaseEnd >= period ? 0 : phaseEnd;
                    continue;
                }
                if (inDash)
                {
                    var from = offset / length;
                    var to = (offset + advance) / length;
                    AppendCanonicalSegment(
                        scene,
                        x,
                        y,
                        path,
                        new CanonicalPoint(start.X + dx * from, start.YTop + dy * from),
                        new CanonicalPoint(start.X + dx * to, start.YTop + dy * to),
                        maximum,
                        remaining,
                        continuePath: false);
                }
                offset += advance;
                phase += advance;
                if (phase >= period - CanonicalGeometryEpsilon)
                {
                    phase = 0;
                }
                else if (Math.Abs(phase - CanonicalDashLength) <= CanonicalGeometryEpsilon)
                {
                    phase = CanonicalDashLength;
                }
            }
        }
    }

    private static void AppendCanonicalSegment(
        GraphScene scene,
        List<double> x,
        List<double> y,
        StringBuilder path,
        CanonicalPoint start,
        CanonicalPoint end,
        double maximum,
        bool remaining,
        bool continuePath)
    {
        var roundedStart = RoundCanonical(start);
        var roundedEnd = RoundCanonical(end);
        var startTimestamp = CanonicalTimestamp(scene, roundedStart.X);
        var startValue = CanonicalAxisValue(roundedStart.YTop, maximum, remaining);
        var endTimestamp = CanonicalTimestamp(scene, roundedEnd.X);
        var endValue = CanonicalAxisValue(roundedEnd.YTop, maximum, remaining);
        var canContinue = continuePath &&
            x.Count > 0 &&
            double.IsFinite(x[^1]) &&
            x[^1] == startTimestamp &&
            y[^1] == startValue;
        if (canContinue)
        {
            path.Append(CultureInfo.InvariantCulture, $" L{roundedEnd.X:0.00} {roundedEnd.YTop:0.00}");
            x.Add(endTimestamp);
            y.Add(endValue);
            return;
        }

        if (path.Length > 0)
        {
            path.Append(' ');
        }
        path.Append(CultureInfo.InvariantCulture, $"M{roundedStart.X:0.00} {roundedStart.YTop:0.00} L{roundedEnd.X:0.00} {roundedEnd.YTop:0.00}");

        if (x.Count > 0)
        {
            x.Add(double.NaN);
            y.Add(double.NaN);
        }
        x.Add(startTimestamp);
        y.Add(startValue);
        x.Add(endTimestamp);
        y.Add(endValue);
    }

    private static CanonicalPoint RoundCanonical(CanonicalPoint point) =>
        new(RoundCanonicalValue(point.X), RoundCanonicalValue(point.YTop));

    private static double RoundCanonicalValue(double value) => RoundExactBinary(value, 2);

    private static double RoundExactBinary(double value, int decimalPlaces)
    {
        if (!double.IsFinite(value))
        {
            throw new ArgumentOutOfRangeException(nameof(value));
        }
        if (decimalPlaces is < 0 or > 9)
        {
            throw new ArgumentOutOfRangeException(nameof(decimalPlaces));
        }

        // Rust formats the exact IEEE-754 value. .NET's fixed-point formatter
        // rounds its decimal rendering instead, which moves values such as
        // 18.39499999999999957 from 18.39 to 18.40. Round the exact binary
        // rational to hundredths so the two renderers choose the same pixel.
        var negative = value < 0;
        var bits = (ulong)BitConverter.DoubleToInt64Bits(Math.Abs(value));
        var exponentBits = (int)((bits >> 52) & 0x7ff);
        var fraction = bits & 0x000f_ffff_ffff_ffff;
        var significand = exponentBits == 0
            ? new BigInteger(fraction)
            : new BigInteger(fraction | (1UL << 52));
        var exponent = exponentBits == 0
            ? -1074
            : exponentBits - 1023 - 52;
        var scale = BigInteger.Pow(10, decimalPlaces);
        var numerator = significand * scale;
        var denominator = BigInteger.One;
        if (exponent >= 0)
        {
            numerator <<= exponent;
        }
        else
        {
            denominator <<= -exponent;
        }

        var rounded = BigInteger.DivRem(numerator, denominator, out var remainder);
        var comparison = (remainder << 1).CompareTo(denominator);
        if (comparison > 0 || (comparison == 0 && !rounded.IsEven))
        {
            rounded++;
        }
        if (negative)
        {
            rounded = -rounded;
        }
        return (double)rounded / (double)scale;
    }

    private static double CanonicalTimestamp(GraphScene scene, double x) =>
        scene.PeriodStartAt + x / 100 * (scene.PeriodEndAt - scene.PeriodStartAt);

    private static double CanonicalAxisValue(double yTop, double maximum, bool remaining) =>
        remaining
            ? Math.Clamp((99 - yTop) / 0.98, 0, 100)
            : Math.Clamp((99 - yTop) / 98 * Math.Max(maximum, 1), 0, Math.Max(maximum, 1));

    internal static IReadOnlyList<double> EvaluateMonotoneCubicInterval(
        IReadOnlyList<double> x,
        IReadOnlyList<double> y,
        int interval,
        IReadOnlyList<double> fractions)
    {
        ArgumentNullException.ThrowIfNull(x);
        ArgumentNullException.ThrowIfNull(y);
        ArgumentNullException.ThrowIfNull(fractions);
        var slopes = BuildMonotoneCubicSlopes(x, y);
        if (interval < 0 || interval + 1 >= x.Count ||
            fractions.Any(fraction => !double.IsFinite(fraction) || fraction is < 0 or > 1))
        {
            throw new ArgumentOutOfRangeException(nameof(interval));
        }

        var width = x[interval + 1] - x[interval];
        return fractions.Select(fraction =>
        {
            var squared = fraction * fraction;
            var cubed = squared * fraction;
            return (2 * cubed - 3 * squared + 1) * y[interval] +
                (cubed - 2 * squared + fraction) * width * slopes[interval] +
                (-2 * cubed + 3 * squared) * y[interval + 1] +
                (cubed - squared) * width * slopes[interval + 1];
        }).ToArray();
    }

    private static double[] BuildMonotoneCubicSlopes(
        IReadOnlyList<double> x,
        IReadOnlyList<double> y)
    {
        if (x.Count != y.Count || x.Count < 2 ||
            x.Any(value => !double.IsFinite(value)) ||
            y.Any(value => !double.IsFinite(value)))
        {
            throw new ArgumentException("Curve coordinates must be finite and have matching lengths.");
        }

        var widths = new double[x.Count - 1];
        var deltas = new double[x.Count - 1];
        for (var index = 0; index < widths.Length; index++)
        {
            widths[index] = x[index + 1] - x[index];
            if (widths[index] <= 0)
            {
                throw new ArgumentException("Curve timestamps must be strictly increasing.", nameof(x));
            }
            deltas[index] = (y[index + 1] - y[index]) / widths[index];
        }
        if (x.Count == 2)
        {
            return [deltas[0], deltas[0]];
        }

        static double Endpoint(double firstWidth, double secondWidth, double firstDelta, double secondDelta)
        {
            var candidate = ((2 * firstWidth + secondWidth) * firstDelta -
                firstWidth * secondDelta) / (firstWidth + secondWidth);
            if (candidate * firstDelta <= 0)
            {
                return 0;
            }
            if (firstDelta * secondDelta < 0 && Math.Abs(candidate) > 3 * Math.Abs(firstDelta))
            {
                return 3 * firstDelta;
            }
            return candidate;
        }

        var slopes = new double[x.Count];
        slopes[0] = Endpoint(widths[0], widths[1], deltas[0], deltas[1]);
        for (var index = 1; index < x.Count - 1; index++)
        {
            var before = deltas[index - 1];
            var after = deltas[index];
            if (before * after <= 0)
            {
                slopes[index] = 0;
                continue;
            }
            var firstWeight = 2 * widths[index] + widths[index - 1];
            var secondWeight = widths[index] + 2 * widths[index - 1];
            slopes[index] = (firstWeight + secondWeight) /
                (firstWeight / before + secondWeight / after);
        }
        var last = x.Count - 1;
        slopes[last] = Endpoint(
            widths[last - 1],
            widths[last - 2],
            deltas[last - 1],
            deltas[last - 2]);
        return slopes;
    }

    private static void AppendSmoothedRemainingRuns(
        GraphScene scene,
        IReadOnlyList<double> values,
        IReadOnlyList<(int Left, int Right, bool Dashed)> intervals,
        List<double> solidX,
        List<double> solidY,
        List<double> dashedX,
        List<double> dashedY)
    {
        var runStart = 0;
        while (runStart < intervals.Count)
        {
            var runEnd = runStart + 1;
            while (runEnd < intervals.Count &&
                intervals[runEnd].Left == intervals[runEnd - 1].Right)
            {
                runEnd++;
            }
            var run = intervals.Skip(runStart).Take(runEnd - runStart).ToArray();
            var indices = new[] { run[0].Left }
                .Concat(run.Select(interval => interval.Right))
                .ToArray();
            var timestamps = indices.Select(index => scene.Timestamps[index]).ToArray();
            var runValues = indices.Select(index => values[index]).ToArray();
            var preservedIndices = run
                .SelectMany((interval, index) => interval.Dashed
                    ? new[] { index, index + 1 }
                    : Array.Empty<int>())
                .ToHashSet();
            runValues = SmoothSamplingPlateaus(
                scene,
                timestamps,
                runValues,
                preservedIndices);
            for (var interval = 0; interval < run.Length; interval++)
            {
                AppendMonotoneCubicInterval(
                    scene,
                    timestamps,
                    runValues,
                    interval,
                    run[interval].Dashed ? dashedX : solidX,
                    run[interval].Dashed ? dashedY : solidY);
            }
            runStart = runEnd;
        }
    }

    private static double[] SmoothSamplingPlateaus(
        GraphScene scene,
        IReadOnlyList<double> timestamps,
        IReadOnlyList<double> values,
        IReadOnlySet<int> preservedIndices)
    {
        if (timestamps.Count != values.Count || values.Count < 3)
        {
            return values.ToArray();
        }

        var last = values.Count - 1;
        var knotIndices = Enumerable.Range(0, values.Count)
            .Where(index =>
                index == 0 ||
                index == last ||
                preservedIndices.Contains(index) ||
                scene.IdleIntervals.Any(interval =>
                    interval.StartAt == timestamps[index] ||
                    interval.EndAt == timestamps[index]) ||
                (values[index] != values[index - 1] &&
                    values[index] != values[index + 1]))
            .ToArray();
        var knotTimestamps = knotIndices.Select(index => timestamps[index]).ToArray();
        var knotValues = knotIndices.Select(index => values[index]).ToArray();
        if (knotTimestamps.Length < 2)
        {
            return values.ToArray();
        }

        return timestamps.Select(timestamp =>
        {
            var knot = Array.IndexOf(knotTimestamps, timestamp);
            if (knot >= 0)
            {
                return knotValues[knot];
            }
            var right = Array.FindIndex(knotTimestamps, candidate => candidate > timestamp);
            if (right <= 0)
            {
                return right == 0 ? knotValues[0] : knotValues[^1];
            }
            var left = right - 1;
            var fraction = (timestamp - knotTimestamps[left]) /
                (knotTimestamps[right] - knotTimestamps[left]);
            return EvaluateMonotoneCubicInterval(
                knotTimestamps,
                knotValues,
                left,
                [fraction])[0];
        }).ToArray();
    }

    private static void AppendSmoothedModelRuns(
        GraphScene scene,
        IReadOnlyList<double> values,
        IReadOnlyList<(int Left, int Right, ProjectionStyle Style)> intervals,
        List<double> flatX,
        List<double> flatY,
        List<double> risingX,
        List<double> risingY,
        List<double> dashedX,
        List<double> dashedY)
    {
        var runStart = 0;
        while (runStart < intervals.Count)
        {
            var runEnd = runStart + 1;
            while (runEnd < intervals.Count &&
                intervals[runEnd].Left == intervals[runEnd - 1].Right)
            {
                runEnd++;
            }
            var run = intervals.Skip(runStart).Take(runEnd - runStart).ToArray();
            var indices = new[] { run[0].Left }
                .Concat(run.Select(interval => interval.Right))
                .ToArray();
            var timestamps = indices.Select(index => scene.Timestamps[index]).ToArray();
            var runValues = indices.Select(index => values[index]).ToArray();
            var preservedIndices = run
                .SelectMany((interval, index) => interval.Style is ProjectionStyle.Dashed
                    ? new[] { index, index + 1 }
                    : Array.Empty<int>())
                .ToHashSet();
            runValues = SmoothSamplingPlateaus(
                scene,
                timestamps,
                runValues,
                preservedIndices);
            for (var interval = 0; interval < run.Length; interval++)
            {
                var (targetX, targetY) = run[interval].Style switch
                {
                    ProjectionStyle.Flat => (flatX, flatY),
                    ProjectionStyle.Rising => (risingX, risingY),
                    ProjectionStyle.Dashed => (dashedX, dashedY),
                    _ => throw new InvalidOperationException("Unknown graph projection style."),
                };
                AppendMonotoneCubicInterval(
                    scene,
                    timestamps,
                    runValues,
                    interval,
                    targetX,
                    targetY);
            }
            runStart = runEnd;
        }
    }

    private static void AppendMonotoneCubicInterval(
        GraphScene scene,
        IReadOnlyList<double> timestamps,
        IReadOnlyList<double> values,
        int interval,
        List<double> x,
        List<double> y)
    {
        var span = Math.Max(1d, scene.PeriodEndAt - scene.PeriodStartAt);
        var viewboxWidth = Math.Abs(timestamps[interval + 1] - timestamps[interval]) / span * 100;
        var steps = Math.Max(1, (int)Math.Ceiling(viewboxWidth / CurveMaximumViewboxStep));
        var fractions = Enumerable.Range(0, steps + 1)
            .Select(step => step / (double)steps)
            .ToArray();
        var projected = EvaluateMonotoneCubicInterval(
            timestamps,
            values,
            interval,
            fractions);
        for (var step = 0; step < steps; step++)
        {
            var startAt = timestamps[interval] +
                (timestamps[interval + 1] - timestamps[interval]) * fractions[step];
            var endAt = timestamps[interval] +
                (timestamps[interval + 1] - timestamps[interval]) * fractions[step + 1];
            AppendSegment(x, y, startAt, projected[step], endAt, projected[step + 1]);
        }
    }

    private static void AppendSegment(
        List<double> x,
        List<double> y,
        double x1,
        double y1,
        double x2,
        double y2)
    {
        // Adjacent segments with the same visual role form one polyline. A
        // NaN is needed only between runs separated by the other line style or
        // invalid data; adding one after every minute triples ScottPlot work.
        if (x.Count > 0 &&
            !double.IsNaN(x[^1]) &&
            x[^1] == x1 &&
            y[^1] == y1)
        {
            x.Add(x2);
            y.Add(y2);
            return;
        }
        if (x.Count > 0)
        {
            x.Add(double.NaN);
            y.Add(double.NaN);
        }
        x.Add(x1);
        y.Add(y1);
        x.Add(x2);
        y.Add(y2);
    }

    private enum ProjectionStyle
    {
        Flat,
        Rising,
        Dashed,
    }

    private readonly record struct CanonicalPoint(double X, double YTop);

}
