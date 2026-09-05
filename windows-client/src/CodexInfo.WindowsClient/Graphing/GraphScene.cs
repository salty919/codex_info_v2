// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;

namespace CodexInfo.WindowsClient.Graphing;

/// <summary>The model-unit selected for a graph scene.</summary>
public enum GraphMetric
{
    Dollars,
    Tokens,
}

/// <summary>A period in which every cumulative model value is unchanged.</summary>
public readonly record struct GraphIdleInterval(long StartAt, long EndAt, bool PreserveBoundary);

/// <summary>
/// A source observation whose selected cumulative vector corrects a prior
/// observation. The two observations are separate graph segments; this is
/// not a recorder gap and must not be inferred as one by the adapter.
/// </summary>
public readonly record struct GraphCorrectionBoundary(
    int PointIndex,
    long BeforeAt,
    long AfterAt);

/// <summary>
/// Framework-independent graph projection. It is the single owner of graph
/// data semantics; XAML owns layout and the ScottPlot adapter only paints the
/// arrays and fixed axes exposed here.
/// </summary>
public sealed class GraphScene
{
    private GraphScene(
        long periodStartAt,
        long periodEndAt,
        GraphMetric metric,
        double[] timestamps,
        double[] remaining,
        double[] sol,
        double[] terra,
        double[] luna,
        IReadOnlyList<GraphCorrectionBoundary> correctionBoundaries,
        IReadOnlyList<GraphIdleInterval> idleIntervals,
        double modelMaximum)
    {
        PeriodStartAt = periodStartAt;
        PeriodEndAt = periodEndAt;
        Metric = metric;
        Timestamps = timestamps;
        Remaining = remaining;
        Sol = sol;
        Terra = terra;
        Luna = luna;
        CorrectionBoundaries = correctionBoundaries;
        IdleIntervals = idleIntervals;
        ModelMaximum = modelMaximum;
    }

    public long PeriodStartAt { get; }

    public long PeriodEndAt { get; }

    public GraphMetric Metric { get; }

    public IReadOnlyList<double> Timestamps { get; }

    public IReadOnlyList<double> Remaining { get; }

    public IReadOnlyList<double> Sol { get; }

    public IReadOnlyList<double> Terra { get; }

    public IReadOnlyList<double> Luna { get; }

    /// <summary>
    /// Source-backed cumulative corrections. A boundary is a renderer
    /// discontinuity, not a confirmed recorder gap.
    /// </summary>
    public IReadOnlyList<GraphCorrectionBoundary> CorrectionBoundaries { get; }

    public IReadOnlyList<GraphIdleInterval> IdleIntervals { get; }

    public double ModelMaximum { get; }

    public bool HasPoints => Timestamps.Count > 0;

    public static GraphScene Empty(GraphMetric metric = GraphMetric.Dollars) =>
        new(
            0,
            1,
            metric,
            [],
            [],
            [],
            [],
            [],
            [],
            [],
            1);

    public static GraphScene Create(
        IReadOnlyList<ApiHistorySample> samples,
        GraphMetric metric,
        long periodStartAt,
        long periodEndAt)
    {
        ArgumentNullException.ThrowIfNull(samples);
        if (samples.Count == 0)
        {
            return Empty(metric);
        }

        var start = periodStartAt > 0 ? periodStartAt : samples[0].Timestamp;
        var end = periodEndAt > start ? periodEndAt : Math.Max(start + 1, samples[^1].Timestamp);
        var points = new ScenePoint[samples.Count];
        var correctionBoundaries = new List<GraphCorrectionBoundary>();
        long? previousTimestamp = null;
        for (var index = 0; index < samples.Count; index++)
        {
            var sample = samples[index];
            if (previousTimestamp is { } previous && sample.Timestamp <= previous)
            {
                throw new ArgumentException("Graph samples must have strictly increasing timestamps.", nameof(samples));
            }

            previousTimestamp = sample.Timestamp;
            var rawSol = metric == GraphMetric.Dollars ? sample.SolDollars : sample.SolTokens;
            var rawTerra = metric == GraphMetric.Dollars ? sample.TerraDollars : sample.TerraTokens;
            var rawLuna = metric == GraphMetric.Dollars ? sample.LunaDollars : sample.LunaTokens;
            var point = new ScenePoint(
                Math.Clamp(sample.Timestamp, start, end),
                sample.RemainingPercent,
                FiniteNonNegative(rawSol),
                FiniteNonNegative(rawTerra),
                FiniteNonNegative(rawLuna));
            if (index > 0 && IsCorrectionBoundary(points[index - 1], point))
            {
                correctionBoundaries.Add(new(index, points[index - 1].Timestamp, point.Timestamp));
            }

            points[index] = point;
        }

        var effectiveRemaining = BuildEffectiveRemaining(points);
        var timestamps = points.Select(point => (double)point.Timestamp).ToArray();
        var remaining = effectiveRemaining
            .Select(value => value is { } finite && double.IsFinite(finite) ? finite : double.NaN)
            .ToArray();
        var sol = points.Select(point => point.Sol).ToArray();
        var terra = points.Select(point => point.Terra).ToArray();
        var luna = points.Select(point => point.Luna).ToArray();
        var maximum = Math.Max(1, points.SelectMany(point => new[] { point.Sol, point.Terra, point.Luna }).Max());
        return new GraphScene(
            start,
            end,
            metric,
            timestamps,
            remaining,
            sol,
            terra,
            luna,
            correctionBoundaries,
            BuildIdleIntervals(points, start, end),
            maximum);
    }

    internal static IReadOnlyList<double?> BuildEffectiveRemaining(IReadOnlyList<ScenePoint> points)
    {
        if (points.Count == 0)
        {
            return Array.Empty<double?>();
        }

        var rawValues = points.Select(point => point.Remaining).ToArray();
        if (!rawValues.Any(value => value is { } observed && double.IsFinite(observed)))
        {
            return rawValues;
        }

        var values = new double?[points.Count];
        var activeSegments = new bool[Math.Max(0, points.Count - 1)];
        var correctionSegments = new bool[Math.Max(0, points.Count - 1)];
        var interpolated = new bool[points.Count];
        values[0] = rawValues[0] is { } first && double.IsFinite(first)
            ? Math.Clamp(first, 0, 100)
            : 100;
        var quotaObservedSinceModelChange = true;
        for (var index = 1; index < points.Count; index++)
        {
            var previousObserved = values[index - 1];
            var previous = previousObserved ?? 100;
            var correctionBoundary = IsCorrectionBoundary(points[index - 1], points[index]);
            correctionSegments[index - 1] = correctionBoundary;
            var modelAdvanced = ModelAdvanced(points[index - 1], points[index]);
            var syntheticGap = IsSyntheticRemainingGap(points, index, points[0].Timestamp);
            activeSegments[index - 1] = modelAdvanced && !syntheticGap && !correctionBoundary;
            var observed = rawValues[index] is { } raw && double.IsFinite(raw)
                ? Math.Clamp(raw, 0, 100)
                : (double?)null;
            if (correctionBoundary)
            {
                // A correction starts a new source segment. Never carry the
                // old quota into it; a missing new observation remains
                // unknown until the source supplies one.
                values[index] = observed;
                quotaObservedSinceModelChange = observed is not null;
            }
            else if (modelAdvanced)
            {
                values[index] = observed is { } activeRaw
                    ? previousObserved is { } prior ? Math.Min(prior, activeRaw) : activeRaw
                    : previousObserved;
                quotaObservedSinceModelChange = observed is not null;
            }
            else if (!quotaObservedSinceModelChange && previousObserved is { } prior &&
                     observed is { } delayedRaw && delayedRaw < prior)
            {
                // The quota poll can lag behind session usage. Accept the
                // first lower endpoint after that unobserved active interval,
                // but keep a genuinely idle period horizontal.
                values[index] = Math.Min(prior, delayedRaw);
                quotaObservedSinceModelChange = true;
            }
            else
            {
                values[index] = previousObserved;
            }
        }

        var source = values.ToArray();
        for (var index = 1; index < points.Count; index++)
        {
            if (interpolated[index])
            {
                continue;
            }

            if (source[index - 1] is not { } previous || source[index] is not { } current ||
                previous != current || points[index - 1].Timestamp == points[index].Timestamp)
            {
                continue;
            }

            var left = index - 1;
            var right = index + 1;
            while (right < points.Count && source[right] is { } candidate && candidate >= previous)
            {
                right++;
            }

            if (right >= points.Count || source[right] is not { } next || next >= previous)
            {
                continue;
            }

            var crossesCorrectionBoundary = false;
            for (var segment = left; segment < right; segment++)
            {
                if (correctionSegments[segment])
                {
                    crossesCorrectionBoundary = true;
                    break;
                }
            }

            if (crossesCorrectionBoundary)
            {
                continue;
            }

            var totalActive = 0d;
            for (var segment = left; segment < right; segment++)
            {
                var duration = Math.Max(0, points[segment + 1].Timestamp - points[segment].Timestamp);
                if (activeSegments[segment])
                {
                    totalActive += duration;
                }
            }

            if (totalActive <= double.Epsilon)
            {
                continue;
            }

            // Match the native graph's sampling-gap completion exactly: once
            // a later lower quota observation bounds a run of repeated active
            // samples, distribute that change across every active interval in
            // the run.  Updating only the first repeated point leaves the
            // remainder folded into an almost-vertical drop at the right edge.
            var elapsedActive = 0d;
            for (var pointIndex = index; pointIndex < right; pointIndex++)
            {
                if (pointIndex > left && activeSegments[pointIndex - 1])
                {
                    elapsedActive += Math.Max(
                        0,
                        points[pointIndex].Timestamp - points[pointIndex - 1].Timestamp);
                }

                var fraction = Math.Clamp(elapsedActive / totalActive, 0, 1);
                values[pointIndex] = previous + (next - previous) * fraction;
                interpolated[pointIndex] = true;
            }
        }

        for (var index = 1; index + 1 < values.Length; index++)
        {
            if (!activeSegments[index - 1] || !activeSegments[index] ||
                interpolated[index - 1] || interpolated[index] || interpolated[index + 1] ||
                values[index - 1] is not { } before || values[index] is not { } current ||
                values[index + 1] is not { } after)
            {
                continue;
            }

            values[index] = Math.Min(before, (before + 2 * current + after) / 4);
        }

        for (var index = 1; index < values.Length; index++)
        {
            if (correctionSegments[index - 1])
            {
                continue;
            }

            if (!activeSegments[index - 1] &&
                points[index].Timestamp != points[index - 1].Timestamp &&
                !IsSyntheticRemainingGap(points, index, points[0].Timestamp))
            {
                values[index] = values[index - 1];
            }
            else if (values[index] is { } current && values[index - 1] is { } before)
            {
                values[index] = Math.Min(current, before);
            }
        }

        return values;
    }

    internal static IReadOnlyList<GraphIdleInterval> BuildIdleIntervals(
        IReadOnlyList<ScenePoint> points,
        long periodStart,
        long periodEnd)
    {
        var intervals = new List<GraphIdleInterval>();
        if (points.Count < 2 || periodEnd <= periodStart)
        {
            return intervals;
        }

        for (var index = 1; index < points.Count; index++)
        {
            var before = points[index - 1];
            var after = points[index];
            if (after.Timestamp <= before.Timestamp)
            {
                continue;
            }

            var intervalStart = Math.Max(before.Timestamp, periodStart);
            var intervalEnd = Math.Min(after.Timestamp, periodEnd);
            if (intervalEnd <= intervalStart)
            {
                continue;
            }

            var syntheticGap = IsSyntheticRemainingGap(points, index, periodStart);
            var unobservedGap = after.Timestamp - before.Timestamp > 60;
            var modelChanged = !ModelsEqual(before, after);
            if (modelChanged && !syntheticGap && !unobservedGap)
            {
                continue;
            }

            // A long interval between observations is not evidence of a
            // continuous spend rate.  The native graph marks that interval
            // as unused/unobserved and draws any cumulative increase at the
            // observed endpoint.  Preserve the boundary so the remaining
            // line does not turn the unknown interval into a diagonal.
            var correctionBoundary = IsCorrectionBoundary(before, after);
            var preserveBoundary = !correctionBoundary &&
                (syntheticGap || (unobservedGap && modelChanged));

            if (intervals.Count > 0)
            {
                var previous = intervals[^1];
                if (!previous.PreserveBoundary && !preserveBoundary && previous.EndAt == intervalStart)
                {
                    intervals[^1] = previous with { EndAt = intervalEnd };
                    continue;
                }
            }

            intervals.Add(new GraphIdleInterval(intervalStart, intervalEnd, preserveBoundary));
        }

        return intervals;
    }

    internal static bool IsSyntheticRemainingGap(
        IReadOnlyList<ScenePoint> points,
        int index,
        long periodStart)
    {
        if (index <= 0 || index >= points.Count)
        {
            return false;
        }

        var before = points[index - 1];
        var after = points[index];
        return before.Timestamp == periodStart &&
            after.Timestamp - before.Timestamp > 60 &&
            before.Sol <= 0 && before.Terra <= 0 && before.Luna <= 0 &&
            ModelAdvanced(before, after);
    }

    internal static IReadOnlyList<double> ArrangeEndpointLabelTops(
        IReadOnlyList<double> idealTops,
        double top,
        double bottom,
        double labelHeight,
        double gap)
    {
        ArgumentNullException.ThrowIfNull(idealTops);
        if (idealTops.Count == 0)
        {
            return Array.Empty<double>();
        }
        if (labelHeight <= 0 || gap < 0 || bottom < top)
        {
            throw new ArgumentOutOfRangeException(nameof(labelHeight));
        }

        var maximumTop = Math.Max(top, bottom - labelHeight);
        var step = labelHeight + gap;
        var result = idealTops.Select(ideal => Math.Clamp(ideal, top, maximumTop)).ToArray();
        for (var index = 1; index < result.Length; index++)
        {
            result[index] = Math.Max(result[index], result[index - 1] + step);
        }
        if (result[^1] > maximumTop)
        {
            result[^1] = maximumTop;
            for (var index = result.Length - 2; index >= 0; index--)
            {
                result[index] = Math.Min(result[index], result[index + 1] - step);
            }
        }
        if (result[0] < top)
        {
            var shift = top - result[0];
            for (var index = 0; index < result.Length; index++)
            {
                result[index] += shift;
            }
        }
        return result;
    }

    private static double FiniteNonNegative(double value) =>
        double.IsFinite(value) ? Math.Max(0, value) : 0;

    private static bool ModelAdvanced(ScenePoint before, ScenePoint after) =>
        after.Sol > before.Sol || after.Terra > before.Terra || after.Luna > before.Luna;

    private static bool IsCorrectionBoundary(ScenePoint before, ScenePoint after) =>
        after.Sol < before.Sol || after.Terra < before.Terra || after.Luna < before.Luna;

    private static bool ModelsEqual(ScenePoint before, ScenePoint after) =>
        before.Sol == after.Sol && before.Terra == after.Terra && before.Luna == after.Luna;

    internal readonly record struct ScenePoint(
        long Timestamp,
        double? Remaining,
        double Sol,
        double Terra,
        double Luna);
}
