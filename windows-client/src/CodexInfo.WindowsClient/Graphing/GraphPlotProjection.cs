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
    GraphLineProjection Idle,
    GraphLineProjection Flat,
    GraphLineProjection Rising,
    GraphLineProjection Dashed);

/// <summary>Separate solid and reference-only remaining-quota paths.</summary>
internal readonly record struct GraphRemainingLineProjection(
    GraphLineProjection Idle,
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
    internal const double CanonicalDashLength = 0.45;
    internal const double CanonicalDashGap = 0.30;
    private const double CurveMaximumViewboxStep = 0.25;
    private const double CanonicalGeometryEpsilon = 1e-12;

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
        var semantic = BuildModelLines(scene, values, smooth: true);
        return new GraphCanonicalModelLineProjection(
            CanonicalizeLine(scene, semantic.Idle, scene.ModelMaximum, remaining: false, dashed: false),
            CanonicalizeLine(scene, semantic.Flat, scene.ModelMaximum, remaining: false, dashed: false),
            CanonicalizeLine(scene, semantic.Rising, scene.ModelMaximum, remaining: false, dashed: false),
            CanonicalizeLine(scene, semantic.Dashed, scene.ModelMaximum, remaining: false, dashed: true));
    }

    internal static GraphCanonicalRemainingLineProjection BuildCanonicalRemainingLines(
        GraphScene scene)
    {
        var semantic = BuildRemainingLines(scene, smooth: true);
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
    /// Builds the quota path while keeping remote observations independent
    /// from model availability. Only explicit missing or anomalous intervals
    /// are emitted into the dashed prediction path.
    /// </summary>
    public static GraphRemainingLineProjection BuildRemainingLines(GraphScene scene) =>
        BuildRemainingLines(scene, smooth: false);

    private static GraphRemainingLineProjection BuildRemainingLines(GraphScene scene, bool smooth)
    {
        ArgumentNullException.ThrowIfNull(scene);
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
        for (var anchor = 1; anchor < anchors.Length; anchor++)
        {
            var left = anchors[anchor - 1];
            var right = anchors[anchor];
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
                    !measured;
                if (!dashed && current == before && IsConfirmedIdleInterval(
                        scene,
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
            new GraphLineProjection(idleX, idleY),
            new GraphLineProjection(solidX, solidY),
            new GraphLineProjection(dashedX, dashedY));
    }

    private static bool RemainingOriginHasMeasuredQuota(GraphRemainingOrigin origin) =>
        origin is GraphRemainingOrigin.Raw;

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
            var before = values[left];
            var current = values[right];
            var crossesPrediction = Enumerable.Range(left + 1, right - left - 1)
                .Any(index => !scene.IsModelIntervalReliable(values, index, index));
            var crossesCorrection = scene.HasModelCorrectionBetween(
                values,
                scene.Timestamps[left],
                scene.Timestamps[right]);
            if (current < before)
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
                    crossesPrediction;
                if (!dashed && current == before && IsConfirmedIdleInterval(
                        scene,
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
            new GraphLineProjection(idleX, idleY),
            new GraphLineProjection(flatX, flatY),
            new GraphLineProjection(risingX, risingY),
            new GraphLineProjection(dashedX, dashedY));
    }

    private static bool IsConfirmedIdleInterval(GraphScene scene, double startAt, double endAt) =>
        scene.IdleIntervals.Any(interval =>
            startAt >= interval.StartAt && endAt <= interval.EndAt);

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

    private readonly record struct EndpointCandidate(
        GraphSeries Series,
        string Text,
        double NormalizedTop,
        double PointAxisValue);

    private enum ProjectionStyle
    {
        Flat,
        Rising,
        Dashed,
    }

    private readonly record struct CanonicalPoint(double X, double YTop);

}
