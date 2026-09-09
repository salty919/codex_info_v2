// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.Numerics;
using System.Text;

namespace CodexInfo.WindowsClient.Graphing;

/// <summary>
/// Framework-independent values consumed by the ScottPlot graph adapter.
/// </summary>
internal readonly record struct GraphAxisProjection(
    IReadOnlyList<double> BottomValues,
    IReadOnlyList<long> BottomTimestampValues,
    IReadOnlyList<string> BottomLabels,
    IReadOnlyList<double> ModelValues,
    IReadOnlyList<string> ModelLabels,
    IReadOnlyList<double> RemainingValues,
    IReadOnlyList<string> RemainingLabels,
    double DisplayEndAt,
    double ModelDisplayMinimum,
    double ModelDisplayMaximum,
    double RemainingDisplayMinimum,
    double RemainingDisplayMaximum,
    double EndpointLabelAt);

/// <summary>
/// A line path projected without a rendering framework. NaN separators split
/// independent segments so the drawing adapter never joins unrelated lines.
/// </summary>
internal readonly record struct GraphLineProjection(
    IReadOnlyList<double> X,
    IReadOnlyList<double> Y);

/// <summary>Separate X-compatible paths for quiet and changing segments.</summary>
internal readonly record struct GraphModelLineProjection(
    GraphLineProjection Flat,
    GraphLineProjection Rising,
    GraphLineProjection Dashed);

/// <summary>Separate solid and reference-only remaining-quota paths.</summary>
internal readonly record struct GraphRemainingLineProjection(
    GraphLineProjection Solid,
    GraphLineProjection Dashed);

/// <summary>
/// A renderer-ready line whose coordinates are quantized through the same
/// 0..100, two-decimal viewbox used by the native Slint graph.
/// </summary>
internal readonly record struct GraphCanonicalLineProjection(
    GraphLineProjection Line,
    string Path);

internal readonly record struct GraphCanonicalModelLineProjection(
    GraphCanonicalLineProjection Flat,
    GraphCanonicalLineProjection Rising,
    GraphCanonicalLineProjection Dashed);

internal readonly record struct GraphCanonicalRemainingLineProjection(
    GraphCanonicalLineProjection Solid,
    GraphCanonicalLineProjection Dashed);

internal readonly record struct GraphCanonicalRemainingMarker(
    double X,
    double YTop,
    int Boundary);

/// <summary>
/// A final endpoint label projection.  <see cref="NormalizedTop"/> is the
/// collision-free semantic position and <see cref="AxisValue"/> is the value
/// the rendering adapter should pass to its selected y-axis.
/// </summary>
internal readonly record struct GraphEndpointLabel(
    GraphSeries Series,
    string Text,
    double NormalizedTop,
    double ArrangedTop,
    double AxisValue,
    double PointAxisValue);

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
    // X keeps zero/maximum one percent inside the clipped path. These values
    // are the equivalent data-axis expansion: [0, maximum] maps to [1%, 99%].
    private const double AxisPaddingRatio = 1d / 98d;
    private const double CanonicalReferenceDataAreaWidth = 788;
    internal const double DollarLabelGutterWidth = 94;
    internal const double TokenLabelGutterWidth = 126;
    internal const double EndpointLabelGapWidth = 10;
    internal const double EndpointLabelHeight = 16;
    internal const double MinimumPlotHeight = 204;
    private const long ModelContiguousSampleMaxGapSeconds = 60;
    internal const double CanonicalDashLength = 0.45;
    internal const double CanonicalDashGap = 0.30;

    public static GraphAxisProjection BuildAxes(
        GraphScene scene,
        TimeZoneInfo displayTimeZone,
        CultureInfo culture)
    {
        return BuildAxes(
            scene,
            displayTimeZone,
            culture,
            CanonicalReferenceDataAreaWidth,
            CanonicalReferenceDataAreaWidth);
    }

    /// <summary>
    /// Builds axes whose endpoint-label gutter keeps the physical width it
    /// has at <paramref name="referenceDataAreaWidth"/> while the current
    /// data area grows or shrinks horizontally.
    /// </summary>
    public static GraphAxisProjection BuildAxes(
        GraphScene scene,
        TimeZoneInfo displayTimeZone,
        CultureInfo culture,
        double currentDataAreaWidth,
        double referenceDataAreaWidth)
    {
        ArgumentNullException.ThrowIfNull(scene);
        ArgumentNullException.ThrowIfNull(displayTimeZone);
        ArgumentNullException.ThrowIfNull(culture);
        if (!double.IsFinite(currentDataAreaWidth) || currentDataAreaWidth <= 0)
        {
            throw new ArgumentOutOfRangeException(nameof(currentDataAreaWidth));
        }
        if (!double.IsFinite(referenceDataAreaWidth) || referenceDataAreaWidth <= 0)
        {
            throw new ArgumentOutOfRangeException(nameof(referenceDataAreaWidth));
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
        var gutterWidth = scene.Metric == GraphMetric.Tokens
            ? TokenLabelGutterWidth
            : DollarLabelGutterWidth;
        var currentPlotWidth = currentDataAreaWidth - gutterWidth;
        if (currentPlotWidth <= 0)
        {
            throw new ArgumentOutOfRangeException(
                nameof(currentDataAreaWidth),
                "The current data area must be wider than the fixed endpoint-label gutter.");
        }
        var currentGutterRatio = gutterWidth / currentPlotWidth;
        var currentLabelGapRatio = EndpointLabelGapWidth / currentPlotWidth;

        return new GraphAxisProjection(
            bottomValues,
            bottomTimestampValues,
            bottomLabels,
            modelValues,
            modelLabels,
            remainingValues,
            ["0%", "25%", "50%", "75%", "100%"],
            scene.PeriodEndAt + span * currentGutterRatio,
            -modelPadding,
            scene.ModelMaximum + modelPadding,
            -remainingPadding,
            100d + remainingPadding,
            scene.PeriodEndAt + span * currentLabelGapRatio);
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
        var semantic = BuildModelLines(scene, values);
        return new GraphCanonicalModelLineProjection(
            CanonicalizeLine(scene, semantic.Flat, scene.ModelMaximum, remaining: false, dashed: false),
            CanonicalizeLine(scene, semantic.Rising, scene.ModelMaximum, remaining: false, dashed: false),
            CanonicalizeLine(scene, semantic.Dashed, scene.ModelMaximum, remaining: false, dashed: true));
    }

    internal static GraphCanonicalRemainingLineProjection BuildCanonicalRemainingLines(
        GraphScene scene)
    {
        var semantic = BuildRemainingLines(scene);
        return new GraphCanonicalRemainingLineProjection(
            CanonicalizeLine(scene, semantic.Solid, 100, remaining: true, dashed: false),
            CanonicalizeLine(scene, semantic.Dashed, 100, remaining: true, dashed: true));
    }

    internal static IReadOnlyList<GraphCanonicalRemainingMarker> BuildCanonicalRemainingMarkers(
        GraphScene scene)
    {
        ArgumentNullException.ThrowIfNull(scene);
        if (!scene.HasPoints || scene.PeriodEndAt <= scene.PeriodStartAt)
        {
            return Array.Empty<GraphCanonicalRemainingMarker>();
        }

        var markers = new List<GraphCanonicalRemainingMarker>();
        var seen = new HashSet<int>();
        var previous = -1;
        for (var index = 0; index < scene.Timestamps.Count; index++)
        {
            if (!double.IsFinite(scene.Remaining[index]))
            {
                continue;
            }
            if (previous < 0)
            {
                previous = index;
                continue;
            }

            var before = scene.Remaining[previous];
            var current = scene.Remaining[index];
            if (scene.Timestamps[index] >= scene.Timestamps[previous] && current < before)
            {
                var boundary = (int)Math.Floor(before);
                if (Math.Abs(before - boundary) <= double.Epsilon)
                {
                    boundary--;
                }
                var lowest = (int)Math.Ceiling(current);
                while (boundary >= lowest)
                {
                    if (boundary < before && boundary >= current && seen.Add(boundary))
                    {
                        var fraction = Math.Clamp(
                            (boundary - before) / (current - before),
                            0,
                            1);
                        var timestamp = scene.Timestamps[previous] +
                            (scene.Timestamps[index] - scene.Timestamps[previous]) * fraction;
                        markers.Add(new GraphCanonicalRemainingMarker(
                            (timestamp - scene.PeriodStartAt) /
                                (scene.PeriodEndAt - scene.PeriodStartAt) * 100,
                            99 - boundary * 0.98,
                            boundary));
                    }
                    boundary--;
                }
            }
            previous = index;
        }
        return markers;
    }

    /// <summary>
    /// Builds the quota path while keeping remote observations independent
    /// from model availability. Any missing or unattributed interval is
    /// emitted only into the dashed reference path.
    /// </summary>
    public static GraphRemainingLineProjection BuildRemainingLines(GraphScene scene)
    {
        ArgumentNullException.ThrowIfNull(scene);
        if (!scene.HasPoints)
        {
            return new GraphRemainingLineProjection(
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
                new GraphLineProjection([], []));
        }

        var solidX = new List<double>();
        var solidY = new List<double>();
        var dashedX = new List<double>();
        var dashedY = new List<double>();
        var previous = -1;
        for (var index = firstRenderable; index < scene.Timestamps.Count; index++)
        {
            if (!double.IsFinite(scene.Remaining[index]))
            {
                continue;
            }
            if (previous < 0)
            {
                previous = index;
                continue;
            }

            var before = RemainingValue(scene, previous);
            var current = RemainingValue(scene, index);
            var elapsed = scene.Timestamps[index] - scene.Timestamps[previous];
            var contiguous = index == previous + 1;
            if (scene.HasHardBreakBetween(
                    scene.Timestamps[previous],
                    scene.Timestamps[index]))
            {
                AppendSegment(
                    dashedX,
                    dashedY,
                    scene.Timestamps[previous],
                    before,
                    scene.Timestamps[index],
                    current);
                previous = index;
                continue;
            }
            var observed = RemainingOriginHasMeasuredQuota(scene.RemainingOrigins[previous]) &&
                RemainingOriginHasMeasuredQuota(scene.RemainingOrigins[index]);
            var modelAvailable = scene.TryGetTokenIntervalEvidence(
                previous,
                index,
                out var modelAdvanced);
            var quotaDropped = current < before;
            var unattributed = quotaDropped && (!modelAvailable || !modelAdvanced);
            var dashed = !contiguous || elapsed > ModelContiguousSampleMaxGapSeconds ||
                !observed || unattributed;
            if (dashed)
            {
                AppendSegment(dashedX, dashedY, scene.Timestamps[previous], before, scene.Timestamps[index], current);
            }
            else
            {
                AppendSegment(solidX, solidY, scene.Timestamps[previous], before, scene.Timestamps[index], current);
            }
            previous = index;
        }

        if (previous >= 0 && scene.PeriodEndAt > scene.Timestamps[previous])
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
            new GraphLineProjection(solidX, solidY),
            new GraphLineProjection(dashedX, dashedY));
    }

    private static bool RemainingOriginHasMeasuredQuota(GraphRemainingOrigin origin) =>
        origin is GraphRemainingOrigin.Raw or GraphRemainingOrigin.ActivitySmoothed;

    /// <summary>
    /// Projects the flat, rising, and inferred cumulative-model paths used by
    /// the renderer. Confirmed gaps remain disconnected.
    /// </summary>
    internal static GraphModelLineProjection BuildModelLines(
        GraphScene scene,
        IReadOnlyList<double> values)
    {
        ArgumentNullException.ThrowIfNull(scene);
        ArgumentNullException.ThrowIfNull(values);
        if (values.Count != scene.Timestamps.Count)
        {
            throw new ArgumentException("A model series must match the graph timestamp count.", nameof(values));
        }

        var flatX = new List<double>();
        var flatY = new List<double>();
        var risingX = new List<double>();
        var risingY = new List<double>();
        var dashedX = new List<double>();
        var dashedY = new List<double>();
        var previous = -1;
        for (var index = 0; index < values.Count; index++)
        {
            var value = values[index];
            if (!double.IsFinite(value))
            {
                continue;
            }
            if (previous < 0)
            {
                previous = index;
                continue;
            }

            var before = values[previous];
            var startAt = scene.Timestamps[previous];
            var endAt = scene.Timestamps[index];
            var elapsed = endAt - startAt;
            if (scene.HasHardBreakBetween(startAt, endAt))
            {
                AppendSegment(dashedX, dashedY, startAt, before, endAt, value);
                previous = index;
                continue;
            }
            if (value < before)
            {
                AppendSegment(dashedX, dashedY, startAt, before, endAt, before);
                previous = index;
                continue;
            }
            if (index != previous + 1 || elapsed > ModelContiguousSampleMaxGapSeconds ||
                scene.ModelSynthetic[previous] || scene.ModelSynthetic[index] ||
                !scene.IsModelIntervalReliable(values, previous, index))
            {
                AppendSegment(dashedX, dashedY, startAt, before, endAt, value);
            }
            else if (value == before)
            {
                AppendSegment(flatX, flatY, startAt, before, endAt, value);
            }
            else
            {
                AppendSegment(risingX, risingY, startAt, before, endAt, value);
            }
            previous = index;
        }

        if (previous >= 0 && scene.PeriodEndAt > scene.Timestamps[previous])
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
            new GraphLineProjection(flatX, flatY),
            new GraphLineProjection(risingX, risingY),
            new GraphLineProjection(dashedX, dashedY));
    }

    /// <summary>Returns every evidence interval without a pixel-width filter.</summary>
    public static IReadOnlyList<GraphIdleInterval> BuildVisibleIdleIntervals(GraphScene scene)
    {
        ArgumentNullException.ThrowIfNull(scene);
        return scene.IdleIntervals.ToArray();
    }

    public static IReadOnlyList<GraphEndpointLabel> BuildEndpointLabels(
        GraphScene scene,
        CultureInfo culture)
    {
        ArgumentNullException.ThrowIfNull(scene);
        ArgumentNullException.ThrowIfNull(culture);
        if (!scene.HasPoints)
        {
            return Array.Empty<GraphEndpointLabel>();
        }

        var candidates = new List<EndpointCandidate>();
        AddLatestModelCandidate(scene, scene.Astra, GraphSeries.Astra, culture, candidates);
        AddLatestModelCandidate(scene, scene.Luna, GraphSeries.Luna, culture, candidates);
        AddLatestModelCandidate(scene, scene.Terra, GraphSeries.Terra, culture, candidates);
        AddLatestModelCandidate(scene, scene.Sol, GraphSeries.Sol, culture, candidates);
        var lastRemaining = scene.Remaining
            .Select((value, index) => double.IsFinite(value) ? index : -1)
            .LastOrDefault(index => index >= 0, -1);
        if (lastRemaining >= 0 && scene.RemainingObserved.Any(observed => observed))
        {
            var remainingAtEndpoint = RemainingValue(scene, lastRemaining);
            candidates.Add(new EndpointCandidate(
                GraphSeries.Remaining,
                FormatRemaining(remainingAtEndpoint, culture),
                NativeGraphY(remainingAtEndpoint, 100),
                remainingAtEndpoint));
        }

        var ordered = candidates
            .OrderBy(candidate => candidate.NormalizedTop)
            .ThenBy(candidate => EndpointSortRank(candidate.Series))
            .ToArray();
        var tops = GraphScene.ArrangeEndpointLabelTops(
            ordered.Select(candidate => candidate.NormalizedTop - EndpointLabelHeight / MinimumPlotHeight / 2).ToArray(),
            0,
            1,
            EndpointLabelHeight / MinimumPlotHeight,
            0);
        var labels = new GraphEndpointLabel[ordered.Length];
        for (var index = 0; index < ordered.Length; index++)
        {
            var candidate = ordered[index];
            var maximum = candidate.Series == GraphSeries.Remaining ? 100 : scene.ModelMaximum;
            var arrangedCenter = (double)(float)(tops[index] + EndpointLabelHeight / MinimumPlotHeight / 2);
            labels[index] = new GraphEndpointLabel(
                candidate.Series,
                candidate.Text,
                candidate.NormalizedTop,
                arrangedCenter,
                NormalizedTopToAxisValue(arrangedCenter, maximum),
                candidate.PointAxisValue);
        }

        return labels;
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

    private static double NativeGraphY(double value, double maximum) =>
        (double)(float)Math.Clamp(
            (99 - value / Math.Max(maximum, 1) * 98) / 100,
            0.01,
            0.99);

    private static double NormalizedTopToAxisValue(double normalizedTop, double maximum) =>
        Math.Clamp((0.99 - normalizedTop) / 0.98 * maximum, 0, maximum);

    private static int EndpointSortRank(GraphSeries series) => series switch
    {
        GraphSeries.Remaining => 0,
        GraphSeries.Luna => 1,
        GraphSeries.Terra => 2,
        GraphSeries.Sol => 3,
        GraphSeries.Astra => 4,
        _ => int.MaxValue,
    };

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

    private static string FormatRemaining(double value, CultureInfo culture) =>
        Math.Abs(value - Math.Truncate(value)) < 0.0001
            ? FormatExactBinary(value, 0, culture) + "%"
            : FormatExactBinary(value, 1, culture) + "%";

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

    private static void AddModelCandidate(
        double value,
        double maximum,
        GraphMetric metric,
        GraphSeries series,
        CultureInfo culture,
        ICollection<EndpointCandidate> candidates)
    {
        if (!double.IsFinite(value) || value < 0)
        {
            return;
        }

        candidates.Add(new EndpointCandidate(
            series,
            metric == GraphMetric.Tokens
                ? RoundUnsignedCount(value).ToString("N0", culture)
                : "$" + FormatExactBinary(value, 2, culture),
            NativeGraphY(value, maximum),
            value));
    }

    private static void AddLatestModelCandidate(
        GraphScene scene,
        IReadOnlyList<double> values,
        GraphSeries series,
        CultureInfo culture,
        ICollection<EndpointCandidate> candidates)
    {
        var last = values
            .Select((value, index) => double.IsFinite(value) ? index : -1)
            .LastOrDefault(index => index >= 0, -1);
        if (last < 0)
        {
            return;
        }
        AddModelCandidate(values[last], scene.ModelMaximum, scene.Metric, series, culture, candidates);
    }

    private static GraphCanonicalLineProjection CanonicalizeLine(
        GraphScene scene,
        GraphLineProjection source,
        double maximum,
        bool remaining,
        bool dashed)
    {
        ArgumentNullException.ThrowIfNull(scene);
        var x = new List<double>();
        var y = new List<double>();
        var path = new StringBuilder();
        foreach (var segment in EnumerateLineSegments(source))
        {
            var start = CanonicalPointFor(
                scene,
                segment.X1,
                segment.Y1,
                maximum,
                remaining);
            var end = CanonicalPointFor(
                scene,
                segment.X2,
                segment.Y2,
                maximum,
                remaining);
            if (dashed)
            {
                AppendCanonicalDashes(scene, x, y, path, start, end, maximum, remaining);
            }
            else
            {
                AppendCanonicalSegment(scene, x, y, path, start, end, maximum, remaining);
            }
        }
        return new GraphCanonicalLineProjection(
            new GraphLineProjection(x, y),
            path.ToString());
    }

    private static IEnumerable<LineSegment> EnumerateLineSegments(GraphLineProjection line)
    {
        if (line.X.Count != line.Y.Count)
        {
            throw new ArgumentException("Line coordinate arrays must have the same length.", nameof(line));
        }
        var previous = -1;
        for (var index = 0; index < line.X.Count; index++)
        {
            if (!double.IsFinite(line.X[index]) || !double.IsFinite(line.Y[index]))
            {
                previous = -1;
                continue;
            }
            if (previous >= 0)
            {
                yield return new LineSegment(
                    line.X[previous],
                    line.Y[previous],
                    line.X[index],
                    line.Y[index]);
            }
            previous = index;
        }
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
        CanonicalPoint start,
        CanonicalPoint end,
        double maximum,
        bool remaining)
    {
        var dx = end.X - start.X;
        var dy = end.YTop - start.YTop;
        var length = Math.Sqrt(dx * dx + dy * dy);
        if (!double.IsFinite(length) || length <= double.Epsilon)
        {
            return;
        }
        var offset = 0d;
        while (offset < length)
        {
            var dashEnd = Math.Min(offset + CanonicalDashLength, length);
            var from = offset / length;
            var to = dashEnd / length;
            AppendCanonicalSegment(
                scene,
                x,
                y,
                path,
                new CanonicalPoint(start.X + dx * from, start.YTop + dy * from),
                new CanonicalPoint(start.X + dx * to, start.YTop + dy * to),
                maximum,
                remaining);
            offset += CanonicalDashLength + CanonicalDashGap;
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
        bool remaining)
    {
        var roundedStart = RoundCanonical(start);
        var roundedEnd = RoundCanonical(end);
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
        x.Add(CanonicalTimestamp(scene, roundedStart.X));
        y.Add(CanonicalAxisValue(roundedStart.YTop, maximum, remaining));
        x.Add(CanonicalTimestamp(scene, roundedEnd.X));
        y.Add(CanonicalAxisValue(roundedEnd.YTop, maximum, remaining));
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
        if (value == 0)
        {
            return 0;
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

    private readonly record struct EndpointCandidate(
        GraphSeries Series,
        string Text,
        double NormalizedTop,
        double PointAxisValue);

    private readonly record struct CanonicalPoint(double X, double YTop);

    private readonly record struct LineSegment(
        double X1,
        double Y1,
        double X2,
        double Y2);
}
