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
    GraphLineProjection Rising,
    GraphLineProjection Dashed);

/// <summary>Separate solid and reference-only remaining-quota paths.</summary>
internal readonly record struct GraphRemainingLineProjection(
    GraphLineProjection Solid,
    GraphLineProjection Dashed);

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
            x.Add(scene.Timestamps[index]);
            y.Add(scene.Remaining[index]);
        }
        return new GraphLineProjection(x, y);
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
            if (scene.HasConfirmedGapBetween(
                    scene.Timestamps[previous],
                    scene.Timestamps[index]))
            {
                // Confirmed recorder gaps terminate the old subpath. The
                // marker is the only visual evidence for the interval.
                previous = index;
                continue;
            }
            var observed = scene.RemainingObserved[previous] && scene.RemainingObserved[index];
            // A filled value makes only the interval ending at that point
            // reference-only. Do not taint the following segment once a
            // fresh remote observation and a trusted model increment arrive.
            var interpolated = scene.RemainingInterpolated[index];
            var modelAvailable = scene.ModelVectorAvailable[previous] &&
                scene.ModelVectorAvailable[index];
            var modelAdvanced = ModelAdvanced(scene, previous, index);
            var quotaDropped = current < before;
            var unattributed = quotaDropped && (!modelAvailable || !modelAdvanced);
            var dashed = !contiguous || elapsed > ModelContiguousSampleMaxGapSeconds ||
                !observed || interpolated || !modelAvailable || unattributed ||
                scene.HasConfirmedGapBetween(scene.Timestamps[previous], scene.Timestamps[index]);
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

        if (previous >= 0 && scene.PeriodEndAt > scene.Timestamps[previous] &&
            !scene.HasConfirmedGapBetween(scene.Timestamps[previous], scene.PeriodEndAt))
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
        var dashedX = new List<double>();
        var dashedY = new List<double>();
        for (var index = 1; index < values.Count; index++)
        {
            var before = values[index - 1];
            var after = values[index];
            if (!double.IsFinite(before) || !double.IsFinite(after) || after < before)
            {
                continue;
            }

            var startAt = scene.Timestamps[index - 1];
            var endAt = scene.Timestamps[index];
            if (scene.HasConfirmedGapBetween(startAt, endAt))
            {
                continue;
            }
            if (!scene.ModelVectorAvailable[index - 1] ||
                !scene.ModelVectorAvailable[index])
            {
                AppendSegment(dashedX, dashedY, startAt, before, endAt, after);
                continue;
            }
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

        AppendDashedModelGaps(scene, values, dashedX, dashedY);

        return new GraphModelLineProjection(
            new GraphLineProjection(flatX, flatY),
            new GraphLineProjection(risingX, risingY),
            new GraphLineProjection(dashedX, dashedY));
    }

    /// <summary>
    /// Projects the paths used by the renderer. This keeps the compatibility
    /// output of <see cref="BuildModelLines"/> while ensuring gaps are never
    /// painted as solid connections.
    /// </summary>
    internal static GraphModelLineProjection BuildRenderableModelLines(
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
            if (previous >= 0 && value < values[previous])
            {
                if (!scene.ModelVectorAvailable[previous] ||
                    !scene.ModelVectorAvailable[index])
                {
                    AppendSegment(
                        dashedX,
                        dashedY,
                        scene.Timestamps[previous],
                        values[previous],
                        scene.Timestamps[index],
                        value);
                }
                previous = -1;
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
            if (scene.HasConfirmedGapBetween(startAt, endAt))
            {
                previous = index;
                continue;
            }
            if (!scene.ModelVectorAvailable[previous] ||
                !scene.ModelVectorAvailable[index])
            {
                AppendSegment(dashedX, dashedY, startAt, before, endAt, value);
                previous = index;
                continue;
            }
            if (index != previous + 1 || elapsed > ModelContiguousSampleMaxGapSeconds)
            {
                AppendSegment(dashedX, dashedY, startAt, before, endAt, value);
            }
            else if (IsSyntheticFirstObservation(scene, values, index))
            {
                AppendSegment(flatX, flatY, startAt, before, endAt, before);
                AppendSegment(risingX, risingY, endAt, before, endAt, value);
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

        return new GraphModelLineProjection(
            new GraphLineProjection(flatX, flatY),
            new GraphLineProjection(risingX, risingY),
            new GraphLineProjection(dashedX, dashedY));
    }

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
        if (scene.PeriodEndAt - scene.Timestamps[last] <= ModelContiguousSampleMaxGapSeconds)
        {
            AddModelCandidate("LUNA", scene.Luna[last], scene.ModelMaximum, scene.Metric, GraphSeries.Luna, culture, candidates);
            AddModelCandidate("TERRA", scene.Terra[last], scene.ModelMaximum, scene.Metric, GraphSeries.Terra, culture, candidates);
            AddModelCandidate("SOL", scene.Sol[last], scene.ModelMaximum, scene.Metric, GraphSeries.Sol, culture, candidates);
        }
        var lastRemainingObservation = scene.RemainingObserved
            .Select((observed, index) => observed ? index : -1)
            .LastOrDefault(index => index >= 0, -1);
        if (lastRemainingObservation >= 0 &&
            !scene.HasConfirmedGapBetween(
                scene.Timestamps[lastRemainingObservation],
                scene.PeriodEndAt))
        {
            var remainingAtEndpoint = RemainingValue(scene, lastRemainingObservation);
            candidates.Add(new EndpointCandidate(
                GraphSeries.Remaining,
                FormatRemaining(remainingAtEndpoint, culture),
                1 - Math.Clamp(remainingAtEndpoint / 100, 0, 1),
                remainingAtEndpoint));
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

    private static bool ModelAdvanced(GraphScene scene, int before, int after) =>
        scene.ModelVectorAvailable[before] && scene.ModelVectorAvailable[after] &&
        (scene.Sol[after] > scene.Sol[before] ||
         scene.Terra[after] > scene.Terra[before] ||
         scene.Luna[after] > scene.Luna[before]);

    private static double RemainingValue(GraphScene scene, int index)
    {
        var effective = scene.Remaining[index];
        var observed = scene.ObservedRemainingValues[index];
        if (!scene.RemainingObserved[index] || !double.IsFinite(observed))
        {
            return effective;
        }
        return double.IsFinite(effective) ? Math.Min(effective, observed) : observed;
    }

    private static void AppendDashedModelGaps(
        GraphScene scene,
        IReadOnlyList<double> values,
        List<double> dashedX,
        List<double> dashedY)
    {
        var previous = -1;
        for (var index = 0; index < values.Count; index++)
        {
            if (!double.IsFinite(values[index]))
            {
                continue;
            }
            if (previous >= 0 && values[index] >= values[previous])
            {
                var elapsed = scene.Timestamps[index] - scene.Timestamps[previous];
                if (scene.HasConfirmedGapBetween(
                        scene.Timestamps[previous],
                        scene.Timestamps[index]))
                {
                    previous = index;
                    continue;
                }
                if (index != previous + 1 || elapsed > ModelContiguousSampleMaxGapSeconds)
                {
                    AppendSegment(
                        dashedX,
                        dashedY,
                        scene.Timestamps[previous],
                        values[previous],
                        scene.Timestamps[index],
                        values[index]);
                }
            }
            previous = index;
        }
    }

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
