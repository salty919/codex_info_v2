// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using System.Text;

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

/// <summary>The provenance of one displayed remaining-quota point.</summary>
internal enum GraphRemainingOrigin
{
    Missing,
    Raw,
    Interpolated,
    BoundedNullHold,
    TerminalNullHold,
    MonotonicHold,
}

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
        bool[] modelSynthetic,
        bool[] remainingObserved,
        double[] observedRemainingValues,
        bool[] remainingInterpolated,
        GraphRemainingOrigin[] remainingOrigins,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
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
        ModelSynthetic = modelSynthetic;
        RemainingObserved = remainingObserved;
        ObservedRemainingValues = observedRemainingValues;
        RemainingInterpolated = remainingInterpolated;
        RemainingOrigins = remainingOrigins;
        ConfirmedGaps = confirmedGaps;
        CorrectionStarts = correctionStarts;
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
    /// Indicates whether every model in the selected period's published
    /// universe has an accepted finite value at this sample. Set completeness
    /// alone does not decide this value.
    /// </summary>
    internal IReadOnlyList<bool> ModelVectorAvailable { get; }

    internal IReadOnlyList<bool> ModelSynthetic { get; }

    /// <summary>Indicates which remaining-quota values came from the remote observation.</summary>
    internal IReadOnlyList<bool> RemainingObserved { get; }

    /// <summary>Raw remote quota observations, with missing rows represented by NaN.</summary>
    internal IReadOnlyList<double> ObservedRemainingValues { get; }

    /// <summary>Indicates values filled or adjusted because an observation was unavailable.</summary>
    internal IReadOnlyList<bool> RemainingInterpolated { get; }

    internal IReadOnlyList<GraphRemainingOrigin> RemainingOrigins { get; }

    internal IReadOnlyList<GraphConfirmedGap> ConfirmedGaps { get; }

    internal IReadOnlySet<long> CorrectionStarts { get; }

    public IReadOnlyList<GraphIdleInterval> IdleIntervals { get; }

    public double ModelMaximum { get; }

    public bool HasPoints => Timestamps.Count > 0;

    public static GraphScene Empty(GraphMetric metric = GraphMetric.Dollars) =>
        new(0, 1, metric, [], [], [], [], [], [], new Dictionary<string, IReadOnlyList<double>>(StringComparer.Ordinal), [], [], [], [], [], [], [], new HashSet<long>(), [], 1);

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
            .SelectMany(PublishedModels)
            .Select(model => model.Name)
            .Distinct(StringComparer.Ordinal)
            .OrderBy(name => name, Utf8ModelNameComparer.Instance)
            .ToArray();
        var modelSeriesValues = modelNames.ToDictionary(
            name => name,
            _ => new List<double>(samples.Count),
            StringComparer.Ordinal);
        var points = new ScenePoint[samples.Count];
        var acceptedBaselines = new Dictionary<string, double>(StringComparer.Ordinal);
        var correctionStarts = new HashSet<long>();
        var modelVectorAvailable = new bool[samples.Count];
        var modelSynthetic = new bool[samples.Count];
        var remainingObserved = new bool[samples.Count];
        long? previousTimestamp = null;
        for (var index = 0; index < samples.Count; index++)
        {
            var sample = samples[index];
            modelSynthetic[index] = sample.IsSyntheticTail;
            if (previousTimestamp is { } priorTimestamp && sample.Timestamp <= priorTimestamp)
            {
                throw new ArgumentException("Graph samples must have strictly increasing timestamps.", nameof(samples));
            }

            var crossedGap = previousTimestamp is { } previous &&
                HasConfirmedGapBetween(normalizedGaps, previous, sample.Timestamp);
            if (crossedGap)
            {
                acceptedBaselines.Clear();
            }

            previousTimestamp = sample.Timestamp;
            var modelRows = PublishedModels(sample)
                .ToDictionary(model => model.Name, StringComparer.Ordinal);
            var currentModels = modelNames
                .Select(name => modelRows.TryGetValue(name, out var model)
                    ? ModelValue(model, metric)
                    : sample.ModelSource == ApiHistorySample.ConfirmedModelSource && sample.ModelsComplete
                        ? 0d
                        : double.NaN)
                .ToArray();
            if (sample.ModelSource == ApiHistorySample.UnavailableModelSource)
            {
                Array.Fill(currentModels, double.NaN);
            }
            else
            {
                var trustedCompleteVector = sample.ModelSource == ApiHistorySample.ConfirmedModelSource &&
                    sample.ModelsComplete && currentModels.Length > 0 && currentModels.All(double.IsFinite);
                var correction = !crossedGap && trustedCompleteVector &&
                    modelNames.Select((name, modelIndex) =>
                            acceptedBaselines.TryGetValue(name, out var prior) && currentModels[modelIndex] < prior)
                        .Any(regressed => regressed);
                if (correction)
                {
                    correctionStarts.Add(sample.Timestamp);
                    acceptedBaselines.Clear();
                }

                for (var modelIndex = 0; modelIndex < modelNames.Length; modelIndex++)
                {
                    var value = currentModels[modelIndex];
                    if (!double.IsFinite(value))
                    {
                        continue;
                    }
                    var name = modelNames[modelIndex];
                    if (!correction && acceptedBaselines.TryGetValue(name, out var priorValue) && value < priorValue)
                    {
                        // An incomplete or unconfirmed regression is not an
                        // authoritative correction. Hide only this model until
                        // it recovers to the last accepted baseline.
                        currentModels[modelIndex] = double.NaN;
                        continue;
                    }
                    acceptedBaselines[name] = value;
                }
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
            modelVectorAvailable[index] = modelDataAvailable;
            remainingObserved[index] = sample.RemainingPercent is { } observedQuota &&
                double.IsFinite(observedQuota);
            points[index] = new ScenePoint(
                Math.Clamp(sample.Timestamp, start, end),
                sample.RemainingPercent,
                currentSol,
                currentTerra,
                currentLuna,
                currentAstra,
                modelDataAvailable,
                modelDataAvailable,
                sample.IsSyntheticTail);
        }

        var evidenceSeries = modelSeriesValues.Values
            .Select(values => (IReadOnlyList<double>)values)
            .ToArray();
        var remainingProjection = BuildEffectiveRemainingWithOrigins(
            points,
            evidenceSeries,
            normalizedGaps,
            correctionStarts);
        var effectiveRemaining = remainingProjection.Values;
        var timestamps = points.Select(point => (double)point.Timestamp).ToArray();
        var observedRemainingValues = points
            .Select(point => point.Remaining is { } observed && double.IsFinite(observed)
                ? Math.Clamp(observed, 0, 100)
                : double.NaN)
            .ToArray();
        var remaining = effectiveRemaining
            .Select(value => value is { } finite && double.IsFinite(finite) ? finite : double.NaN)
            .ToArray();
        var remainingOrigins = remainingProjection.Origins;
        var remainingInterpolated = remainingOrigins
            .Select(origin => origin is not GraphRemainingOrigin.Raw)
            .ToArray();
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
            modelSynthetic,
            remainingObserved,
            observedRemainingValues,
            remainingInterpolated,
            remainingOrigins,
            normalizedGaps,
            correctionStarts,
            BuildIdleIntervals(
                points,
                start,
                end,
                evidenceSeries,
                normalizedGaps,
                correctionStarts),
            maximum);
    }

    internal static IReadOnlyList<double?> BuildEffectiveRemaining(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyList<GraphConfirmedGap>? confirmedGaps = null)
    {
        var legacySeries = new List<IReadOnlyList<double>>
        {
            points.Select(point => point.Sol).ToArray(),
            points.Select(point => point.Terra).ToArray(),
            points.Select(point => point.Luna).ToArray(),
        };
        if (points.Any(point => double.IsFinite(point.Astra) && point.Astra >= 0))
        {
            legacySeries.Add(points.Select(point => point.Astra).ToArray());
        }
        return BuildEffectiveRemainingWithOrigins(
                points,
                legacySeries,
                confirmedGaps ?? Array.Empty<GraphConfirmedGap>(),
                new HashSet<long>())
            .Values;
    }

    private static RemainingProjection BuildEffectiveRemainingWithOrigins(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyList<IReadOnlyList<double>> modelSeries,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts)
    {
        var values = new double?[points.Count];
        var origins = Enumerable.Repeat(GraphRemainingOrigin.Missing, points.Count).ToArray();
        var rawValues = points
            .Select(point => point.Remaining is { } raw && double.IsFinite(raw) && raw is >= 0 and <= 100
                ? raw
                : (double?)null)
            .ToArray();

        double? minimum = null;
        for (var index = 0; index < rawValues.Length; index++)
        {
            if (rawValues[index] is not { } raw)
            {
                continue;
            }
            if (minimum is { } prior && raw > prior)
            {
                values[index] = prior;
                origins[index] = GraphRemainingOrigin.MonotonicHold;
            }
            else
            {
                values[index] = raw;
                origins[index] = GraphRemainingOrigin.Raw;
                minimum = raw;
            }
        }

        var runStart = 0;
        while (runStart < rawValues.Length)
        {
            if (rawValues[runStart] is not null)
            {
                runStart++;
                continue;
            }

            var runEnd = runStart;
            while (runEnd < rawValues.Length && rawValues[runEnd] is null)
            {
                runEnd++;
            }
            var left = runStart - 1;
            if (left < 0 || values[left] is null)
            {
                runStart = runEnd;
                continue;
            }

            var bounded = runEnd < rawValues.Length && values[runEnd] is not null;
            var interpolated = false;
            if (bounded && values[left] is { } leftValue && values[runEnd] is { } rightValue && rightValue < leftValue)
            {
                var fullEvidence = true;
                var activeSeconds = 0d;
                for (var segment = left; segment < runEnd; segment++)
                {
                    if (!HasFullModelEvidence(
                            points,
                            modelSeries,
                            confirmedGaps,
                            correctionStarts,
                            segment,
                            segment + 1))
                    {
                        fullEvidence = false;
                        break;
                    }
                    if (ModelAdvanced(modelSeries, segment, segment + 1))
                    {
                        activeSeconds += points[segment + 1].Timestamp - points[segment].Timestamp;
                    }
                }

                if (fullEvidence && activeSeconds > double.Epsilon)
                {
                    var elapsedActive = 0d;
                    for (var index = runStart; index < runEnd; index++)
                    {
                        if (ModelAdvanced(modelSeries, index - 1, index))
                        {
                            elapsedActive += points[index].Timestamp - points[index - 1].Timestamp;
                        }
                        values[index] = leftValue + (rightValue - leftValue) * (elapsedActive / activeSeconds);
                        origins[index] = GraphRemainingOrigin.Interpolated;
                    }
                    interpolated = true;
                }
            }

            if (!interpolated)
            {
                var origin = bounded
                    ? GraphRemainingOrigin.BoundedNullHold
                    : GraphRemainingOrigin.TerminalNullHold;
                for (var index = runStart; index < runEnd; index++)
                {
                    if (values[index - 1] is not { } prior ||
                        !CanCarryQuota(points, confirmedGaps, correctionStarts, index - 1, index))
                    {
                        break;
                    }
                    values[index] = prior;
                    origins[index] = origin;
                }
            }
            runStart = runEnd;
        }

        return new RemainingProjection(values, origins);
    }

    internal static IReadOnlyList<GraphIdleInterval> BuildIdleIntervals(
        IReadOnlyList<ScenePoint> points,
        long periodStart,
        long periodEnd,
        IEnumerable<IReadOnlyList<double>> modelSeries,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts)
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

            if (after.Timestamp - before.Timestamp > 60 ||
                HasConfirmedGapBetween(confirmedGaps, before.Timestamp, after.Timestamp) ||
                HasCorrectionBetween(correctionStarts, before.Timestamp, after.Timestamp) ||
                before.SyntheticTail || after.SyntheticTail ||
                !before.ModelAvailable || !after.ModelAvailable)
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

    internal bool HasConfirmedGapBetween(double startAt, double endAt) =>
        HasConfirmedGapBetween(ConfirmedGaps, startAt, endAt);

    internal bool HasCorrectionBetween(double startAt, double endAt) =>
        HasCorrectionBetween(CorrectionStarts, startAt, endAt);

    internal bool HasHardBreakBetween(double startAt, double endAt) =>
        HasConfirmedGapBetween(startAt, endAt) || HasCorrectionBetween(startAt, endAt);

    private static bool HasConfirmedGapBetween(
        IReadOnlyList<GraphConfirmedGap>? gaps,
        double startAt,
        double endAt) =>
        gaps is not null && gaps.Any(gap => gap.StartAt < endAt && gap.EndAt > startAt);

    private static bool HasCorrectionBetween(
        IReadOnlySet<long> correctionStarts,
        double startAt,
        double endAt) =>
        correctionStarts.Any(timestamp => timestamp > startAt && timestamp <= endAt);

    private static bool CanCarryQuota(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
        int before,
        int after) =>
        after == before + 1 &&
        points[after].Timestamp > points[before].Timestamp &&
        points[after].Timestamp - points[before].Timestamp <= 60 &&
        !HasConfirmedGapBetween(confirmedGaps, points[before].Timestamp, points[after].Timestamp) &&
        !HasCorrectionBetween(correctionStarts, points[before].Timestamp, points[after].Timestamp);

    private static bool HasFullModelEvidence(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyList<IReadOnlyList<double>> modelSeries,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
        int before,
        int after) =>
        modelSeries.Count > 0 &&
        CanCarryQuota(points, confirmedGaps, correctionStarts, before, after) &&
        modelSeries.All(values =>
            before < values.Count && after < values.Count &&
            double.IsFinite(values[before]) && double.IsFinite(values[after]));

    private static bool ModelAdvanced(
        IReadOnlyList<IReadOnlyList<double>> modelSeries,
        int before,
        int after) =>
        modelSeries.Any(values => values[after] > values[before]);

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

    private static IEnumerable<ApiHistoryModelSample> PublishedModels(ApiHistorySample sample)
    {
        if (sample.ModelSource == ApiHistorySample.UnavailableModelSource)
        {
            return Array.Empty<ApiHistoryModelSample>();
        }
        if (sample.ModelSamples is not null)
        {
            return sample.ModelSamples;
        }

        var hasLegacyValues = sample.SolDollars is not null || sample.TerraDollars is not null ||
            sample.LunaDollars is not null || sample.SolTokens is not null ||
            sample.TerraTokens is not null || sample.LunaTokens is not null;
        return hasLegacyValues ? sample.Models : Array.Empty<ApiHistoryModelSample>();
    }

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

    private readonly record struct RemainingProjection(
        double?[] Values,
        GraphRemainingOrigin[] Origins);

    private sealed class Utf8ModelNameComparer : IComparer<string>
    {
        internal static Utf8ModelNameComparer Instance { get; } = new();

        public int Compare(string? left, string? right)
        {
            if (ReferenceEquals(left, right))
            {
                return 0;
            }
            if (left is null)
            {
                return -1;
            }
            if (right is null)
            {
                return 1;
            }
            return Encoding.UTF8.GetBytes(left).AsSpan()
                .SequenceCompareTo(Encoding.UTF8.GetBytes(right));
        }
    }

    internal readonly record struct ScenePoint(
        long Timestamp,
        double? Remaining,
        double Sol,
        double Terra,
        double Luna,
        double Astra,
        bool ModelAvailable,
        bool DataAvailable,
        bool SyntheticTail = false);
}
