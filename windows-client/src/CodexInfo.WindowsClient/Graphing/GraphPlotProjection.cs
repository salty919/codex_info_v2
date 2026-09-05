// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;

namespace CodexInfo.WindowsClient.Graphing;

/// <summary>
/// Framework-independent values consumed by the ScottPlot graph adapter.
/// </summary>
internal readonly record struct GraphAxisProjection(
    IReadOnlyList<double> BottomValues,
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
    GraphLineProjection Rising);

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
    private const double DollarLabelGutterRatio = 0.20;
    private const double TokenLabelGutterRatio = 0.27;
    private const double LabelGapRatio = 0.018;
    private const long ModelContiguousSampleMaxGapSeconds = 60;

    public static GraphAxisProjection BuildAxes(
        GraphScene scene,
        TimeZoneInfo displayTimeZone,
        CultureInfo culture)
    {
        return BuildAxes(scene, displayTimeZone, culture, 1, 1);
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

        var bottomValues = new double[5];
        var bottomLabels = new string[5];
        var modelValues = new double[5];
        var modelLabels = new string[5];
        for (var index = 0; index < 5; index++)
        {
            var ratio = index / 4d;
            var timestamp = scene.PeriodStartAt +
                (long)Math.Round((scene.PeriodEndAt - scene.PeriodStartAt) * ratio);
            bottomValues[index] = scene.PeriodStartAt +
                (scene.PeriodEndAt - scene.PeriodStartAt) * ratio;
            bottomLabels[index] = FormatTimestamp(timestamp, displayTimeZone, culture);
            modelValues[index] = scene.ModelMaximum * ratio;
            modelLabels[index] = FormatAxisValue(modelValues[index], scene.Metric, culture);
        }

        var span = Math.Max(1d, scene.PeriodEndAt - scene.PeriodStartAt);
        var gutterRatio = scene.Metric == GraphMetric.Tokens
            ? TokenLabelGutterRatio
            : DollarLabelGutterRatio;
        var referenceGutterWidth = referenceDataAreaWidth * gutterRatio / (1 + gutterRatio);
        var referenceLabelGapWidth = referenceDataAreaWidth * LabelGapRatio / (1 + gutterRatio);
        var currentPlotWidth = currentDataAreaWidth - referenceGutterWidth;
        if (currentPlotWidth <= 0)
        {
            throw new ArgumentOutOfRangeException(
                nameof(currentDataAreaWidth),
                "The current data area must be wider than the fixed endpoint-label gutter.");
        }
        var currentGutterRatio = referenceGutterWidth / currentPlotWidth;
        var currentLabelGapRatio = referenceLabelGapWidth / currentPlotWidth;
        var modelPadding = scene.ModelMaximum * AxisPaddingRatio;
        var remainingPadding = 100d * AxisPaddingRatio;

        return new GraphAxisProjection(
            bottomValues,
            bottomLabels,
            modelValues,
            modelLabels,
            [0, 25, 50, 75, 100],
            ["0%", "25%", "50%", "75%", "100%"],
            scene.PeriodEndAt + span * currentGutterRatio,
            -modelPadding,
            scene.ModelMaximum + modelPadding,
            -remainingPadding,
            100d + remainingPadding,
            scene.PeriodEndAt + span * currentLabelGapRatio);
    }

    /// <summary>
    /// Builds the remaining-quota path, retaining the synthetic reset anchor
    /// so an unknown interval stays horizontal until the first observation.
    /// </summary>
    public static GraphLineProjection BuildRemainingLine(GraphScene scene)
    {
        ArgumentNullException.ThrowIfNull(scene);
        if (!scene.HasPoints)
        {
            return new GraphLineProjection([], []);
        }

        var x = new List<double>(scene.Timestamps.Count + 1) { scene.Timestamps[0] };
        var y = new List<double>(scene.Remaining.Count + 1) { scene.Remaining[0] };
        for (var index = 1; index < scene.Timestamps.Count; index++)
        {
            if (scene.IdleIntervals.Any(interval =>
                    interval.PreserveBoundary && interval.EndAt == (long)scene.Timestamps[index]))
            {
                x.Add(scene.Timestamps[index]);
                y.Add(scene.Remaining[index - 1]);
            }
            if (IsCorrectionBoundary(scene, index))
            {
                // A correction is an observed source boundary, not a
                // confirmed recorder gap. Keep both endpoint observations,
                // but make the adapter start a new polyline at the latter.
                x.Add(double.NaN);
                y.Add(double.NaN);
            }
            x.Add(scene.Timestamps[index]);
            y.Add(scene.Remaining[index]);
        }
        return new GraphLineProjection(x, y);
    }

    /// <summary>
    /// Splits cumulative model data into the thin/quiet flat path and the
    /// thicker rising path used by the native X graph.
    /// </summary>
    public static GraphModelLineProjection BuildModelLines(
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
        for (var index = 1; index < values.Count; index++)
        {
            var before = values[index - 1];
            var after = values[index];
            if (!double.IsFinite(before) || !double.IsFinite(after) ||
                IsCorrectionBoundary(scene, index) || after < before)
            {
                continue;
            }

            var startAt = scene.Timestamps[index - 1];
            var endAt = scene.Timestamps[index];
            if (IsSyntheticFirstObservation(scene, values, index))
            {
                // The interval between the synthetic reset anchor and the
                // first real observation is unknown. Keep the selected model
                // flat at zero, then place its known cumulative increase at
                // the actual observation timestamp.
                AppendSegment(flatX, flatY, startAt, before, endAt, before);
                AppendSegment(risingX, risingY, endAt, before, endAt, after);
            }
            else if (after == before)
            {
                AppendSegment(flatX, flatY, startAt, before, endAt, after);
            }
            else if (endAt - startAt > ModelContiguousSampleMaxGapSeconds)
            {
                // The elapsed interval is not observed. Keep the cumulative
                // value horizontal during that gap, then show the increase at
                // the actual next observation instead of inventing a spend
                // rate across idle time.
                AppendSegment(flatX, flatY, startAt, before, endAt, before);
                AppendSegment(risingX, risingY, endAt, before, endAt, after);
            }
            else
            {
                AppendSegment(risingX, risingY, startAt, before, endAt, after);
            }
        }

        return new GraphModelLineProjection(
            new GraphLineProjection(flatX, flatY),
            new GraphLineProjection(risingX, risingY));
    }

    private static bool IsCorrectionBoundary(GraphScene scene, int pointIndex) =>
        scene.CorrectionBoundaries.Any(boundary => boundary.PointIndex == pointIndex);

    private static bool IsSyntheticFirstObservation(
        GraphScene scene,
        IReadOnlyList<double> values,
        int index)
    {
        if (index <= 0 || index >= scene.Timestamps.Count)
        {
            return false;
        }

        var startAt = scene.Timestamps[index - 1];
        var endAt = scene.Timestamps[index];
        return startAt == scene.PeriodStartAt &&
            endAt - startAt > 60 &&
            scene.Sol[index - 1] <= 0 &&
            scene.Terra[index - 1] <= 0 &&
            scene.Luna[index - 1] <= 0 &&
            values[index - 1] <= 0 &&
            values[index] > values[index - 1];
    }

    /// <summary>
    /// Sub-pixel rectangles produce the barcode artefact seen in ScottPlot.
    /// X effectively drops those rectangles during rasterization, so apply
    /// the same finite minimum at the smallest supported plot width.
    /// </summary>
    public static IReadOnlyList<GraphIdleInterval> BuildVisibleIdleIntervals(
        GraphScene scene,
        double minimumNormalizedWidth = 1d / 480d)
    {
        ArgumentNullException.ThrowIfNull(scene);
        if (!double.IsFinite(minimumNormalizedWidth) || minimumNormalizedWidth < 0)
        {
            throw new ArgumentOutOfRangeException(nameof(minimumNormalizedWidth));
        }

        var span = Math.Max(1d, scene.PeriodEndAt - scene.PeriodStartAt);
        return scene.IdleIntervals
            .Where(interval => interval.PreserveBoundary ||
                (interval.EndAt - interval.StartAt) / span >= minimumNormalizedWidth)
            .ToArray();
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

        var last = scene.Timestamps.Count - 1;
        var candidates = new List<EndpointCandidate>();
        AddModelCandidate("LUNA", scene.Luna[last], scene.ModelMaximum, scene.Metric, GraphSeries.Luna, culture, candidates);
        AddModelCandidate("TERRA", scene.Terra[last], scene.ModelMaximum, scene.Metric, GraphSeries.Terra, culture, candidates);
        AddModelCandidate("SOL", scene.Sol[last], scene.ModelMaximum, scene.Metric, GraphSeries.Sol, culture, candidates);
        if (double.IsFinite(scene.Remaining[last]))
        {
            candidates.Add(new EndpointCandidate(
                GraphSeries.Remaining,
                FormatRemaining(scene.Remaining[last], culture),
                1 - Math.Clamp(scene.Remaining[last] / 100, 0, 1),
                scene.Remaining[last]));
        }

        var ordered = candidates.OrderBy(candidate => candidate.NormalizedTop).ToArray();
        var tops = GraphScene.ArrangeEndpointLabelTops(
            ordered.Select(candidate => candidate.NormalizedTop - 0.025).ToArray(),
            0,
            1,
            0.05,
            0.012);
        var labels = new GraphEndpointLabel[ordered.Length];
        for (var index = 0; index < ordered.Length; index++)
        {
            var candidate = ordered[index];
            var maximum = candidate.Series == GraphSeries.Remaining ? 100 : scene.ModelMaximum;
            labels[index] = new GraphEndpointLabel(
                candidate.Series,
                candidate.Text,
                candidate.NormalizedTop,
                tops[index],
                (1 - (tops[index] + 0.025)) * maximum,
                candidate.PointAxisValue);
        }

        return labels;
    }

    internal static string FormatAxisValue(double value, GraphMetric metric, CultureInfo culture)
    {
        ArgumentNullException.ThrowIfNull(culture);
        if (metric == GraphMetric.Dollars)
        {
            return "$" + value.ToString("0.00", culture);
        }

        if (Math.Abs(value) >= 1_000_000_000)
        {
            return (value / 1_000_000_000).ToString("0.0", culture) + "B";
        }

        if (Math.Abs(value) >= 1_000_000)
        {
            return (value / 1_000_000).ToString("0.0", culture) + "M";
        }

        if (Math.Abs(value) >= 1_000)
        {
            return (value / 1_000).ToString("0.0", culture) + "K";
        }

        return value.ToString("N0", culture);
    }

    private static string FormatTimestamp(long timestamp, TimeZoneInfo displayTimeZone, CultureInfo culture) =>
        TimeZoneInfo.ConvertTime(
                DateTimeOffset.FromUnixTimeSeconds(timestamp),
                displayTimeZone)
            .ToString("MM/dd HH:mm", culture);

    private static string FormatRemaining(double value, CultureInfo culture) =>
        value.ToString("0.#", culture) + "%";

    private static void AddModelCandidate(
        string name,
        double value,
        double maximum,
        GraphMetric metric,
        GraphSeries series,
        CultureInfo culture,
        ICollection<EndpointCandidate> candidates)
    {
        if (!double.IsFinite(value) || value <= 0)
        {
            return;
        }

        candidates.Add(new EndpointCandidate(
            series,
            $"{name} {FormatAxisValue(value, metric, culture)}",
            1 - Math.Clamp(value / maximum, 0, 1),
            value));
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
}
