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

/// <summary>A recorder-confirmed interval in which no complete observation exists.</summary>
internal readonly record struct GraphConfirmedGap(long StartAt, long EndAt);

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
        double[] astra,
        IReadOnlyDictionary<string, IReadOnlyList<double>> modelSeries,
        bool[] modelVectorAvailable,
        bool[] remainingObserved,
        double[] observedRemainingValues,
        bool[] remainingInterpolated,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
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
        Astra = astra;
        ModelSeries = modelSeries;
        ModelVectorAvailable = modelVectorAvailable;
        RemainingObserved = remainingObserved;
        ObservedRemainingValues = observedRemainingValues;
        RemainingInterpolated = remainingInterpolated;
        ConfirmedGaps = confirmedGaps;
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

    /// <summary>The explicit ASTRA series from the accepted v3 model rows.</summary>
    public IReadOnlyList<double> Astra { get; }

    /// <summary>
    /// Generic model series keyed by the server-provided model identifier.
    /// Missing values are represented by NaN and are never replaced with zero.
    /// </summary>
    public IReadOnlyDictionary<string, IReadOnlyList<double>> ModelSeries { get; }

    /// <summary>
    /// Indicates whether the complete cumulative model vector at each sample
    /// is trusted. A false row is intentionally unavailable for every model,
    /// even when one or two raw components were still monotonic.
    /// </summary>
    internal IReadOnlyList<bool> ModelVectorAvailable { get; }

    /// <summary>Indicates which remaining-quota values came from the remote observation.</summary>
    internal IReadOnlyList<bool> RemainingObserved { get; }

    /// <summary>Raw remote quota observations, with missing rows represented by NaN.</summary>
    internal IReadOnlyList<double> ObservedRemainingValues { get; }

    /// <summary>Indicates values filled or adjusted because an observation was unavailable.</summary>
    internal IReadOnlyList<bool> RemainingInterpolated { get; }

    internal IReadOnlyList<GraphConfirmedGap> ConfirmedGaps { get; }

    public IReadOnlyList<GraphIdleInterval> IdleIntervals { get; }

    public double ModelMaximum { get; }

    public bool HasPoints => Timestamps.Count > 0;

    public static GraphScene Empty(GraphMetric metric = GraphMetric.Dollars) =>
        new(0, 1, metric, [], [], [], [], [], [], new Dictionary<string, IReadOnlyList<double>>(StringComparer.Ordinal), [], [], [], [], [], [], 1);

    public static GraphScene Create(
        IReadOnlyList<ApiHistorySample> samples,
        GraphMetric metric,
        long periodStartAt,
        long periodEndAt) =>
        Create(samples, metric, periodStartAt, periodEndAt, null);

    internal static GraphScene Create(
        IReadOnlyList<ApiHistorySample> samples,
        GraphMetric metric,
        long periodStartAt,
        long periodEndAt,
        IReadOnlyList<GraphConfirmedGap>? confirmedGaps = null)
    {
        ArgumentNullException.ThrowIfNull(samples);
        if (samples.Count == 0)
        {
            return Empty(metric);
        }

        var start = periodStartAt > 0 ? periodStartAt : samples[0].Timestamp;
        var end = periodEndAt > start ? periodEndAt : Math.Max(start + 1, samples[^1].Timestamp);
        var normalizedGaps = confirmedGaps is null
            ? Array.Empty<GraphConfirmedGap>()
            : confirmedGaps
                .Where(gap => gap.EndAt > gap.StartAt && gap.EndAt > start && gap.StartAt < end)
                .Select(gap => new GraphConfirmedGap(
                    Math.Max(start, gap.StartAt),
                    Math.Min(end, gap.EndAt)))
                .Where(gap => gap.EndAt > gap.StartAt)
                .OrderBy(gap => gap.StartAt)
                .ToArray();
        var modelNames = samples
            .SelectMany(sample => sample.Models.Select(model => model.Name))
            .Distinct(StringComparer.Ordinal)
            .ToArray();
        var modelSeriesValues = modelNames.ToDictionary(
            name => name,
            _ => new List<double>(samples.Count),
            StringComparer.Ordinal);
        var points = new ScenePoint[samples.Count];
        var hasTrustedVector = false;
        var hasObservedVector = false;
        var previousTrustedModels = new Dictionary<string, double>(StringComparer.Ordinal);
        var previousObservedModels = new Dictionary<string, double>(StringComparer.Ordinal);
        var modelVectorAvailable = new bool[samples.Count];
        var remainingObserved = new bool[samples.Count];
        long? previousTimestamp = null;
        for (var index = 0; index < samples.Count; index++)
        {
            var sample = samples[index];
            if (previousTimestamp is { } previous && sample.Timestamp <= previous)
            {
                throw new ArgumentException("Graph samples must have strictly increasing timestamps.", nameof(samples));
            }

            previousTimestamp = sample.Timestamp;
            var modelRows = sample.Models.ToDictionary(model => model.Name, StringComparer.Ordinal);
            var currentModels = modelNames
                .Select(name => modelRows.TryGetValue(name, out var model)
                    ? ModelValue(model, metric)
                    : double.NaN)
                .ToArray();
            var completeVector = currentModels.Length > 0 && currentModels.All(double.IsFinite);
            var sourceConfirmed = sample.ModelSource == ApiHistorySample.ConfirmedModelSource &&
                sample.ModelsComplete;
            var vectorRegressed = hasObservedVector && completeVector &&
                modelNames.Zip(currentModels, (name, value) =>
                    previousObservedModels.TryGetValue(name, out var previous) && value < previous)
                .Any(regressed => regressed);
            var vectorRecovered = sourceConfirmed && completeVector &&
                (!hasTrustedVector || modelNames.All(name =>
                    !previousTrustedModels.TryGetValue(name, out var previous) ||
                    currentModels[Array.IndexOf(modelNames, name)] >= previous));
            if (vectorRecovered)
            {
                previousTrustedModels.Clear();
                for (var modelIndex = 0; modelIndex < modelNames.Length; modelIndex++)
                {
                    previousTrustedModels[modelNames[modelIndex]] = currentModels[modelIndex];
                }
                hasTrustedVector = true;
                modelVectorAvailable[index] = true;
            }
            else if (!sourceConfirmed &&
                     sample.ModelSource != ApiHistorySample.UnavailableModelSource &&
                     currentModels.Any(double.IsFinite) &&
                     !vectorRegressed)
            {
                // The set is incomplete, but each present model row is still
                // an exact observation. Missing models remain NaN rather than
                // becoming synthetic zeroes. Set completeness is retained
                // separately for quota attribution.
            }
            if (sample.ModelSource == ApiHistorySample.UnavailableModelSource)
            {
                Array.Fill(currentModels, double.NaN);
            }
            else
            {
                for (var modelIndex = 0; modelIndex < modelNames.Length; modelIndex++)
                {
                    var value = currentModels[modelIndex];
                    if (!double.IsFinite(value))
                    {
                        continue;
                    }
                    var name = modelNames[modelIndex];
                    if (previousObservedModels.TryGetValue(name, out var priorValue) && value < priorValue)
                    {
                        // A cumulative model can never fall within one period.
                        // Hide only the regressed model; a peer model's exact
                        // row must not disappear with the whole vector.
                        currentModels[modelIndex] = double.NaN;
                        continue;
                    }
                    previousObservedModels[name] = value;
                }
                hasObservedVector = previousObservedModels.Count > 0;
            }
            for (var modelIndex = 0; modelIndex < modelNames.Length; modelIndex++)
            {
                modelSeriesValues[modelNames[modelIndex]].Add(currentModels[modelIndex]);
            }
            var currentSol = ModelValue(modelNames, currentModels, "SOL");
            var currentTerra = ModelValue(modelNames, currentModels, "TERRA");
            var currentLuna = ModelValue(modelNames, currentModels, "LUNA");
            var currentAstra = ModelValue(modelNames, currentModels, "ASTRA");
            var modelDataAvailable = currentModels.Length > 0 && currentModels.All(double.IsFinite);
            remainingObserved[index] = sample.RemainingPercent is { } observedQuota &&
                double.IsFinite(observedQuota);
            points[index] = new ScenePoint(
                Math.Clamp(sample.Timestamp, start, end),
                sample.RemainingPercent,
                currentSol,
                currentTerra,
                currentLuna,
                currentAstra,
                modelVectorAvailable[index],
                modelDataAvailable);
        }

        var effectiveRemaining = BuildEffectiveRemaining(points, normalizedGaps);
        var timestamps = points.Select(point => (double)point.Timestamp).ToArray();
        var observedRemainingValues = points
            .Select(point => point.Remaining is { } observed && double.IsFinite(observed)
                ? Math.Clamp(observed, 0, 100)
                : double.NaN)
            .ToArray();
        var remaining = effectiveRemaining
            .Select(value => value is { } finite && double.IsFinite(finite) ? finite : double.NaN)
            .ToArray();
        var remainingInterpolated = new bool[points.Length];
        for (var index = 0; index < points.Length; index++)
        {
            var raw = points[index].Remaining;
            var rawFinite = raw is { } observed && double.IsFinite(observed);
            var rawValue = rawFinite ? Math.Clamp(raw!.Value, 0, 100) : double.NaN;
            remainingInterpolated[index] = !rawFinite ||
                !double.IsFinite(remaining[index]) ||
                remaining[index] != rawValue;
        }
        var sol = points.Select(point => point.Sol).ToArray();
        var terra = points.Select(point => point.Terra).ToArray();
        var luna = points.Select(point => point.Luna).ToArray();
        var astra = points.Select(point => point.Astra).ToArray();
        var maximum = Math.Max(
            1,
            modelSeriesValues.Values
                .SelectMany(values => values)
                .Where(double.IsFinite)
                .DefaultIfEmpty(0)
                .Max());
        var modelSeries = modelSeriesValues.ToDictionary(
            pair => pair.Key,
            pair => (IReadOnlyList<double>)pair.Value.ToArray(),
            StringComparer.Ordinal);
        return new GraphScene(
            start,
            end,
            metric,
            timestamps,
            remaining,
            sol,
            terra,
            luna,
            astra,
            modelSeries,
            modelVectorAvailable,
            remainingObserved,
            observedRemainingValues,
            remainingInterpolated,
            normalizedGaps,
            BuildIdleIntervals(points, start, end, modelSeriesValues.Values),
            maximum);
    }

    internal static IReadOnlyList<double?> BuildEffectiveRemaining(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyList<GraphConfirmedGap>? confirmedGaps = null)
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
        var interpolated = new bool[points.Count];
        values[0] = rawValues[0] is { } first && double.IsFinite(first)
            ? Math.Clamp(first, 0, 100)
            : 100;
        var quotaAvailable = values[0] is not null;
        var quotaObservedSinceModelChange = true;
        for (var index = 1; index < points.Count; index++)
        {
            var previous = values[index - 1] ?? 100;
            var modelAdvanced = ModelAdvanced(points[index - 1], points[index]);
            var syntheticGap = IsSyntheticRemainingGap(points, index, points[0].Timestamp);
            var confirmedGap = HasConfirmedGapBetween(
                confirmedGaps,
                points[index - 1].Timestamp,
                points[index].Timestamp);
            activeSegments[index - 1] = modelAdvanced && !syntheticGap && !confirmedGap;
            var observed = rawValues[index] is { } raw && double.IsFinite(raw)
                ? Math.Clamp(raw, 0, 100)
                : (double?)null;
            if (confirmedGap)
            {
                // A recorder-confirmed gap is not an inferred interval. Do
                // not carry a previous quota into it or synthesize a value
                // at its far edge; only a remote observation after the gap
                // can restart the quota path.
                values[index] = observed;
                quotaAvailable = observed is not null;
                quotaObservedSinceModelChange = observed is not null;
            }
            else if (!quotaAvailable)
            {
                // Once a confirmed gap has cut the quota path, no inferred
                // value may restart it. Wait for a fresh remote endpoint.
                values[index] = observed;
                quotaAvailable = observed is not null;
                quotaObservedSinceModelChange = observed is not null;
            }
            else if (!points[index].DataAvailable && observed is { } unavailableRaw)
            {
                // Preserve a remote quota reading even when the local model
                // vector is unavailable. The renderer classifies this drop
                // as unattributed and paints it dashed.
                values[index] = Math.Min(previous, unavailableRaw);
                quotaObservedSinceModelChange = true;
            }
            else if (modelAdvanced)
            {
                values[index] = observed is { } activeRaw
                    ? Math.Min(previous, activeRaw)
                    : previous;
                quotaObservedSinceModelChange = observed is not null;
            }
            else if (!quotaObservedSinceModelChange && observed is { } delayedRaw && delayedRaw < previous)
            {
                // The quota poll can lag behind session usage. Accept the
                // first lower endpoint after that unobserved active interval,
                // but keep a genuinely idle period horizontal.
                values[index] = Math.Min(previous, delayedRaw);
                quotaObservedSinceModelChange = true;
            }
            else if (observed is { } independentRaw)
            {
                // A remote quota observation is evidence in its own right.
                // Preserve its timestamp even when the local vector is flat;
                // projection will mark an unattributed decrease as dashed.
                values[index] = Math.Min(previous, independentRaw);
                quotaObservedSinceModelChange = true;
            }
            else
            {
                values[index] = previous;
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
            if (HasConfirmedGapBetween(
                    confirmedGaps,
                    points[index - 1].Timestamp,
                    points[index].Timestamp))
            {
                continue;
            }
            if (!activeSegments[index - 1] &&
                points[index].Timestamp != points[index - 1].Timestamp &&
                !IsSyntheticRemainingGap(points, index, points[0].Timestamp))
            {
                values[index] = rawValues[index] is { } observed && double.IsFinite(observed) &&
                    values[index - 1] is { } before
                    ? Math.Min(before, Math.Clamp(observed, 0, 100))
                    : values[index - 1];
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
        long periodEnd,
        IEnumerable<IReadOnlyList<double>> modelSeries)
    {
        var intervals = new List<GraphIdleInterval>();
        if (periodEnd <= periodStart)
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

            if (!ModelsEqualAt(modelSeries, index - 1, index))
            {
                continue;
            }

            // This background has one meaning: every measured cumulative
            // model value stayed unchanged. Missing evidence and recorder
            // gaps are rendered by thin dashed paths, never as idle bands.
            const bool preserveBoundary = false;

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

        return intervals
            .OrderBy(interval => interval.StartAt)
            .ThenBy(interval => interval.EndAt)
            .ToArray();
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
            before.Sol <= 0 && before.Terra <= 0 && before.Luna <= 0 && before.Astra <= 0 &&
            ModelAdvanced(before, after);
    }

    internal bool HasConfirmedGapBetween(double startAt, double endAt) =>
        HasConfirmedGapBetween(ConfirmedGaps, startAt, endAt);

    private static bool HasConfirmedGapBetween(
        IReadOnlyList<GraphConfirmedGap>? gaps,
        double startAt,
        double endAt) =>
        gaps is not null && gaps.Any(gap => gap.StartAt < endAt && gap.EndAt > startAt);

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

    private static double FiniteNonNegative(double? value) =>
        value is { } finite && double.IsFinite(finite) ? Math.Max(0, finite) : double.NaN;

    private static double ModelValue(
        ApiHistoryModelSample model,
        GraphMetric metric)
    {
        if (metric == GraphMetric.Dollars)
        {
            return FiniteNonNegative(model.Dollars);
        }

        return model.TotalTokens is ulong tokens ? tokens : double.NaN;
    }

    private static double ModelValue(
        IReadOnlyList<string> names,
        IReadOnlyList<double> values,
        string name)
    {
        var index = -1;
        for (var candidate = 0; candidate < names.Count; candidate++)
        {
            if (names[candidate] == name)
            {
                index = candidate;
                break;
            }
        }
        return index >= 0 && index < values.Count ? values[index] : double.NaN;
    }

    private static bool ModelAdvanced(ScenePoint before, ScenePoint after) =>
        before.DataAvailable && after.DataAvailable &&
        (after.Sol > before.Sol || after.Terra > before.Terra || after.Luna > before.Luna ||
         after.Astra > before.Astra);

    private static bool ModelsEqualAt(
        IEnumerable<IReadOnlyList<double>> modelSeries,
        int beforeIndex,
        int afterIndex)
    {
        var anyModel = false;
        foreach (var values in modelSeries)
        {
            if (beforeIndex >= values.Count || afterIndex >= values.Count ||
                !double.IsFinite(values[beforeIndex]) || !double.IsFinite(values[afterIndex]))
            {
                return false;
            }
            anyModel = true;
            if (values[beforeIndex] != values[afterIndex])
            {
                return false;
            }
        }
        return anyModel;
    }

    internal readonly record struct ScenePoint(
        long Timestamp,
        double? Remaining,
        double Sol,
        double Terra,
        double Luna,
        double Astra,
        bool ModelAvailable,
        bool DataAvailable);
}
