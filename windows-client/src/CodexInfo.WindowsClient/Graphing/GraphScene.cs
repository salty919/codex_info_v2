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
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenModelSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> modelReliability,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
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
        TokenModelSeries = tokenModelSeries;
        ModelReliability = modelReliability;
        TokenReliability = tokenReliability;
        PublishedModelNames = publishedModelNames;
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

    /// <summary>Accepted cumulative tokens used for activity and idle evidence.</summary>
    internal IReadOnlyDictionary<string, IReadOnlyList<double>> TokenModelSeries { get; }

    internal IReadOnlyDictionary<string, IReadOnlyList<bool>> ModelReliability { get; }

    internal IReadOnlyDictionary<string, IReadOnlyList<bool>> TokenReliability { get; }

    internal IReadOnlyList<IReadOnlySet<string>> PublishedModelNames { get; }

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
            new Dictionary<string, IReadOnlyList<double>>(StringComparer.Ordinal),
            new Dictionary<string, IReadOnlyList<double>>(StringComparer.Ordinal),
            new Dictionary<string, IReadOnlyList<bool>>(StringComparer.Ordinal),
            new Dictionary<string, IReadOnlyList<bool>>(StringComparer.Ordinal),
            [],
            [],
            [],
            [],
            [],
            [],
            [],
            [],
            new HashSet<long>(),
            [],
            1);

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
        var displayProjection = BuildAcceptedModelProjection(samples, modelNames, metric);
        var tokenProjection = BuildAcceptedModelProjection(samples, modelNames, GraphMetric.Tokens);
        var publishedModelNames = samples
            .Select(sample => (IReadOnlySet<string>)(sample.IsSyntheticTail ||
                sample.ModelSource == ApiHistorySample.UnavailableModelSource
                    ? new HashSet<string>(StringComparer.Ordinal)
                    : PublishedModels(sample)
                        .Where(model => model.TotalTokens is not null)
                        .Select(model => model.Name)
                        .ToHashSet(StringComparer.Ordinal)))
            .ToArray();
        var points = new ScenePoint[samples.Count];
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

            previousTimestamp = sample.Timestamp;
            var currentModels = modelNames
                .Select(name => displayProjection.Values[name][index])
                .ToArray();
            var currentSol = ModelValue(modelNames, currentModels, "SOL");
            var currentTerra = ModelValue(modelNames, currentModels, "TERRA");
            var currentLuna = ModelValue(modelNames, currentModels, "LUNA");
            var currentAstra = ModelValue(modelNames, currentModels, "ASTRA");
            var modelDataAvailable = currentModels.Length > 0 && modelNames.All(name =>
                double.IsFinite(displayProjection.Values[name][index]) &&
                displayProjection.Reliability[name][index]);
            modelVectorAvailable[index] = modelDataAvailable;
            remainingObserved[index] = sample.RemainingPercent is { } observedQuota &&
                double.IsFinite(observedQuota) && observedQuota is >= 0 and <= 100 && !sample.IsSyntheticTail;
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

        var remainingProjection = BuildEffectiveRemainingWithOrigins(
            points,
            tokenProjection.Values,
            tokenProjection.Reliability,
            publishedModelNames,
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
        var modelSeries = displayProjection.Values;
        var sol = SeriesOrMissing(modelSeries, "SOL", samples.Count);
        var terra = SeriesOrMissing(modelSeries, "TERRA", samples.Count);
        var luna = SeriesOrMissing(modelSeries, "LUNA", samples.Count);
        var astra = SeriesOrMissing(modelSeries, "ASTRA", samples.Count);
        var maximum = Math.Max(
            1,
            modelSeries.Values
                .SelectMany(values => values)
                .Where(double.IsFinite)
                .DefaultIfEmpty(0)
                .Max());
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
            tokenProjection.Values,
            displayProjection.Reliability,
            tokenProjection.Reliability,
            publishedModelNames,
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
                tokenProjection.Values,
                tokenProjection.Reliability,
                publishedModelNames,
                remainingProjection.RawReliable,
                normalizedGaps,
                correctionStarts),
            maximum);
    }

    private static double[] SeriesOrMissing(
        IReadOnlyDictionary<string, IReadOnlyList<double>> series,
        string name,
        int count) =>
        series.TryGetValue(name, out var values)
            ? values as double[] ?? values.ToArray()
            : Enumerable.Repeat(double.NaN, count).ToArray();

    internal bool IsModelIntervalReliable(
        IReadOnlyList<double> values,
        int before,
        int after)
    {
        var name = ModelSeries
            .FirstOrDefault(pair => ReferenceEquals(pair.Value, values))
            .Key;
        return name is null || !ModelReliability.TryGetValue(name, out var reliability) ||
            (before >= 0 && after >= 0 && before < reliability.Count && after < reliability.Count &&
                reliability[before] && reliability[after]);
    }

    private static ModelProjection BuildAcceptedModelProjection(
        IReadOnlyList<ApiHistorySample> samples,
        IReadOnlyList<string> modelNames,
        GraphMetric metric)
    {
        var values = new Dictionary<string, IReadOnlyList<double>>(StringComparer.Ordinal);
        var reliability = new Dictionary<string, IReadOnlyList<bool>>(StringComparer.Ordinal);
        foreach (var name in modelNames)
        {
            var raw = samples
                .Select(sample => RawModelValue(sample, name, metric))
                .ToArray();
            var isolated = new bool[samples.Count];
            for (var index = 1; index + 1 < samples.Count; index++)
            {
                var left = raw[index - 1];
                var middle = raw[index];
                var right = raw[index + 1];
                isolated[index] =
                    samples[index].Timestamp - samples[index - 1].Timestamp == 60 &&
                    samples[index + 1].Timestamp - samples[index].Timestamp == 60 &&
                    !samples[index - 1].IsSyntheticTail &&
                    !samples[index].IsSyntheticTail &&
                    !samples[index + 1].IsSyntheticTail &&
                    double.IsFinite(left) && left >= 0 &&
                    double.IsFinite(middle) && middle >= 0 &&
                    double.IsFinite(right) && right >= 0 &&
                    left <= right && (middle < left || middle > right);
            }

            var accepted = new double[samples.Count];
            var acceptedReliability = new bool[samples.Count];
            double? baseline = null;
            for (var index = 0; index < samples.Count; index++)
            {
                var candidate = raw[index];
                if (samples[index].IsSyntheticTail || !double.IsFinite(candidate) || candidate < 0)
                {
                    accepted[index] = samples[index].IsSyntheticTail && baseline is { } tail
                        ? tail
                        : double.NaN;
                    continue;
                }
                if (isolated[index] || baseline is { } prior && candidate < prior)
                {
                    accepted[index] = baseline ?? double.NaN;
                    continue;
                }
                accepted[index] = candidate;
                acceptedReliability[index] = true;
                baseline = candidate;
            }
            values[name] = accepted;
            reliability[name] = acceptedReliability;
        }
        return new ModelProjection(values, reliability);
    }

    private static double RawModelValue(ApiHistorySample sample, string name, GraphMetric metric)
    {
        if (sample.ModelSource == ApiHistorySample.UnavailableModelSource)
        {
            return double.NaN;
        }
        var model = PublishedModels(sample).FirstOrDefault(candidate => candidate.Name == name);
        if (model is not null)
        {
            return ModelValue(model, metric);
        }
        return sample.ModelSource == ApiHistorySample.ConfirmedModelSource && sample.ModelsComplete
            ? 0d
            : double.NaN;
    }

    internal static IReadOnlyList<double?> BuildEffectiveRemaining(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyList<GraphConfirmedGap>? confirmedGaps = null)
    {
        var legacySeries = new Dictionary<string, IReadOnlyList<double>>(StringComparer.Ordinal)
        {
            ["SOL"] = points.Select(point => point.Sol).ToArray(),
            ["TERRA"] = points.Select(point => point.Terra).ToArray(),
            ["LUNA"] = points.Select(point => point.Luna).ToArray(),
        };
        if (points.Any(point => double.IsFinite(point.Astra) && point.Astra >= 0))
        {
            legacySeries["ASTRA"] = points.Select(point => point.Astra).ToArray();
        }
        var reliability = legacySeries.ToDictionary(
            pair => pair.Key,
            pair => (IReadOnlyList<bool>)pair.Value.Select(value => double.IsFinite(value) && value >= 0).ToArray(),
            StringComparer.Ordinal);
        var published = Enumerable.Range(0, points.Count)
            .Select(index => (IReadOnlySet<string>)legacySeries
                .Where(pair => double.IsFinite(pair.Value[index]) && pair.Value[index] >= 0)
                .Select(pair => pair.Key)
                .ToHashSet(StringComparer.Ordinal))
            .ToArray();
        return BuildEffectiveRemainingWithOrigins(
                points,
                legacySeries,
                reliability,
                published,
                confirmedGaps ?? Array.Empty<GraphConfirmedGap>(),
                new HashSet<long>())
            .Values;
    }

    private static RemainingProjection BuildEffectiveRemainingWithOrigins(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts)
    {
        var values = new double?[points.Count];
        var origins = Enumerable.Repeat(GraphRemainingOrigin.Missing, points.Count).ToArray();
        var rawReliable = new bool[points.Count];
        var rawValues = points
            .Select(point => !point.SyntheticTail && point.Remaining is { } raw &&
                double.IsFinite(raw) && raw is >= 0 and <= 100
                ? raw
                : (double?)null)
            .ToArray();

        var isolated = new bool[points.Count];
        for (var index = 1; index + 1 < points.Count; index++)
        {
            if (rawValues[index - 1] is not { } left ||
                rawValues[index] is not { } middle ||
                rawValues[index + 1] is not { } right)
            {
                continue;
            }
            isolated[index] = points[index].Timestamp - points[index - 1].Timestamp == 60 &&
                points[index + 1].Timestamp - points[index].Timestamp == 60 &&
                left >= right && (middle > left || middle < right);
        }

        double? minimum = null;
        for (var index = 0; index < rawValues.Length; index++)
        {
            if (rawValues[index] is not { } raw)
            {
                continue;
            }
            if (isolated[index] || minimum is { } prior && raw > prior)
            {
                values[index] = minimum;
                origins[index] = GraphRemainingOrigin.MonotonicHold;
            }
            else
            {
                values[index] = raw;
                origins[index] = GraphRemainingOrigin.Raw;
                rawReliable[index] = true;
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
            var origin = bounded
                ? GraphRemainingOrigin.BoundedNullHold
                : GraphRemainingOrigin.TerminalNullHold;
            for (var index = runStart; index < runEnd; index++)
            {
                if (values[index - 1] is not { } prior)
                {
                    break;
                }
                values[index] = prior;
                origins[index] = origin;
            }
            runStart = runEnd;
        }

        var changeAnchors = new List<int>();
        for (var index = 0; index < rawValues.Length; index++)
        {
            if (!rawReliable[index] || rawValues[index] is not { } raw)
            {
                continue;
            }
            if (changeAnchors.Count == 0 || rawValues[changeAnchors[^1]] != raw)
            {
                changeAnchors.Add(index);
            }
        }
        for (var anchor = 1; anchor < changeAnchors.Count; anchor++)
        {
            var left = changeAnchors[anchor - 1];
            var right = changeAnchors[anchor];
            if (right - left < 2 || values[left] is not { } leftValue ||
                values[right] is not { } rightValue || rightValue >= leftValue)
            {
                continue;
            }
            var crossesRemainingAnomaly = Enumerable.Range(left + 1, right - left - 1)
                .Any(index => rawValues[index] is not null && !rawReliable[index]);
            if (crossesRemainingAnomaly)
            {
                continue;
            }
            var activity = new List<(bool Active, double Elapsed)>();
            var activeSeconds = 0d;
            var fullEvidence = true;
            for (var segment = left; segment < right; segment++)
            {
                if (!HasTokenIntervalEvidence(
                        points,
                        tokenSeries,
                        tokenReliability,
                        publishedModelNames,
                        confirmedGaps,
                        correctionStarts,
                        segment,
                        segment + 1,
                        out var advanced))
                {
                    fullEvidence = false;
                    break;
                }
                var elapsed = points[segment + 1].Timestamp - points[segment].Timestamp;
                if (advanced)
                {
                    activeSeconds += elapsed;
                }
                activity.Add((advanced, elapsed));
            }
            if (!fullEvidence || activeSeconds <= double.Epsilon)
            {
                continue;
            }
            var activeElapsed = 0d;
            for (var index = left + 1; index < right; index++)
            {
                var interval = activity[index - left - 1];
                if (interval.Active)
                {
                    activeElapsed += interval.Elapsed;
                }
                var smoothed = leftValue +
                    (rightValue - leftValue) * (activeElapsed / activeSeconds);
                values[index] = smoothed;
                if (!(rawReliable[index] && rawValues[index] == smoothed))
                {
                    origins[index] = GraphRemainingOrigin.Interpolated;
                }
            }
        }

        return new RemainingProjection(values, origins, rawReliable);
    }

    internal static IReadOnlyList<GraphIdleInterval> BuildIdleIntervals(
        IReadOnlyList<ScenePoint> points,
        long periodStart,
        long periodEnd,
        IEnumerable<IReadOnlyList<double>> modelSeries,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts)
    {
        var series = modelSeries
            .Select((values, index) => (Name: $"legacy-{index}", Values: values))
            .ToDictionary(pair => pair.Name, pair => pair.Values, StringComparer.Ordinal);
        var reliability = series.ToDictionary(
            pair => pair.Key,
            pair => (IReadOnlyList<bool>)pair.Value.Select(value => double.IsFinite(value) && value >= 0).ToArray(),
            StringComparer.Ordinal);
        var published = Enumerable.Range(0, points.Count)
            .Select(index => (IReadOnlySet<string>)series
                .Where(pair => index < pair.Value.Count && double.IsFinite(pair.Value[index]) && pair.Value[index] >= 0)
                .Select(pair => pair.Key)
                .ToHashSet(StringComparer.Ordinal))
            .ToArray();
        var remainingReliable = points
            .Select(point => point.Remaining is { } raw && double.IsFinite(raw) && raw is >= 0 and <= 100 && !point.SyntheticTail)
            .ToArray();
        return BuildIdleIntervals(
            points,
            periodStart,
            periodEnd,
            series,
            reliability,
            published,
            remainingReliable,
            confirmedGaps,
            correctionStarts);
    }

    internal static IReadOnlyList<GraphIdleInterval> BuildIdleIntervals(
        IReadOnlyList<ScenePoint> points,
        long periodStart,
        long periodEnd,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
        IReadOnlyList<bool> remainingRawReliable,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts)
    {
        var candidates = new List<GraphIdleInterval>();
        if (periodEnd <= periodStart || points.Count < 2 ||
            publishedModelNames.Count != points.Count || remainingRawReliable.Count != points.Count)
        {
            return candidates;
        }

        var basic = new bool[points.Count - 1];
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
                !IdleEvidenceEqual(
                    points,
                    tokenSeries,
                    tokenReliability,
                    publishedModelNames,
                    remainingRawReliable,
                    confirmedGaps,
                    correctionStarts,
                    index - 1,
                    index))
            {
                continue;
            }
            basic[index - 1] = true;
            candidates.Add(new GraphIdleInterval(intervalStart, intervalEnd, false));
        }

        for (var index = 0; index + 3 < points.Count; index++)
        {
            if (points[index + 1].Timestamp - points[index].Timestamp == 60 &&
                points[index + 2].Timestamp - points[index + 1].Timestamp == 120 &&
                points[index + 3].Timestamp - points[index + 2].Timestamp == 60 &&
                basic[index] && basic[index + 2] &&
                IdleEvidenceEqual(
                    points,
                    tokenSeries,
                    tokenReliability,
                    publishedModelNames,
                    remainingRawReliable,
                    confirmedGaps,
                    correctionStarts,
                    index + 1,
                    index + 2))
            {
                candidates.Add(new GraphIdleInterval(
                    points[index + 1].Timestamp,
                    points[index + 2].Timestamp,
                    false));
            }
        }

        var ordered = candidates
            .OrderBy(interval => interval.StartAt)
            .ThenBy(interval => interval.EndAt)
            .ToArray();
        var merged = new List<GraphIdleInterval>();
        foreach (var interval in ordered)
        {
            if (merged.Count > 0 && interval.StartAt <= merged[^1].EndAt)
            {
                merged[^1] = merged[^1] with { EndAt = Math.Max(merged[^1].EndAt, interval.EndAt) };
            }
            else
            {
                merged.Add(interval);
            }
        }
        return merged;
    }

    private static bool IdleEvidenceEqual(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
        IReadOnlyList<bool> remainingRawReliable,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
        int before,
        int after)
    {
        if (before < 0 || after <= before || after >= points.Count ||
            points[before].SyntheticTail || points[after].SyntheticTail ||
            !remainingRawReliable[before] || !remainingRawReliable[after] ||
            points[before].Remaining is not { } beforeRemaining ||
            points[after].Remaining is not { } afterRemaining ||
            beforeRemaining != afterRemaining ||
            HasConfirmedGapBetween(confirmedGaps, points[before].Timestamp, points[after].Timestamp) ||
            HasCorrectionBetween(correctionStarts, points[before].Timestamp, points[after].Timestamp))
        {
            return false;
        }
        var names = publishedModelNames[before];
        if (names.Count == 0 || !names.SetEquals(publishedModelNames[after]))
        {
            return false;
        }
        return names.All(name =>
            tokenSeries.TryGetValue(name, out var values) &&
            tokenReliability.TryGetValue(name, out var reliable) &&
            before < values.Count && after < values.Count &&
            before < reliable.Count && after < reliable.Count &&
            reliable[before] && reliable[after] &&
            double.IsFinite(values[before]) && values[before] >= 0 &&
            values[before] == values[after]);
    }

    private static bool HasTokenIntervalEvidence(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
        int before,
        int after,
        out bool advanced)
    {
        advanced = false;
        if (before < 0 || after != before + 1 || after >= points.Count ||
            points[after].Timestamp <= points[before].Timestamp ||
            points[after].Timestamp - points[before].Timestamp > 60 ||
            points[before].SyntheticTail || points[after].SyntheticTail ||
            HasConfirmedGapBetween(confirmedGaps, points[before].Timestamp, points[after].Timestamp) ||
            HasCorrectionBetween(correctionStarts, points[before].Timestamp, points[after].Timestamp))
        {
            return false;
        }
        var names = publishedModelNames[before];
        if (names.Count == 0 || !names.SetEquals(publishedModelNames[after]))
        {
            return false;
        }
        foreach (var name in names)
        {
            if (!tokenSeries.TryGetValue(name, out var values) ||
                !tokenReliability.TryGetValue(name, out var reliable) ||
                before >= values.Count || after >= values.Count ||
                before >= reliable.Count || after >= reliable.Count ||
                !reliable[before] || !reliable[after] ||
                !double.IsFinite(values[before]) || !double.IsFinite(values[after]) ||
                values[before] < 0 || values[after] < values[before])
            {
                return false;
            }
            advanced |= values[after] > values[before];
        }
        return true;
    }

    internal bool TryGetTokenIntervalEvidence(int before, int after, out bool advanced)
    {
        advanced = false;
        if (before < 0 || after != before + 1 || after >= Timestamps.Count ||
            Timestamps[after] <= Timestamps[before] ||
            Timestamps[after] - Timestamps[before] > 60 ||
            ModelSynthetic[before] || ModelSynthetic[after] ||
            HasHardBreakBetween(Timestamps[before], Timestamps[after]))
        {
            return false;
        }
        var names = PublishedModelNames[before];
        if (names.Count == 0 || !names.SetEquals(PublishedModelNames[after]))
        {
            return false;
        }
        foreach (var name in names)
        {
            if (!TokenModelSeries.TryGetValue(name, out var values) ||
                !TokenReliability.TryGetValue(name, out var reliable) ||
                before >= values.Count || after >= values.Count ||
                before >= reliable.Count || after >= reliable.Count ||
                !reliable[before] || !reliable[after] ||
                !double.IsFinite(values[before]) || !double.IsFinite(values[after]) ||
                values[before] < 0 || values[after] < values[before])
            {
                return false;
            }
            advanced |= values[after] > values[before];
        }
        return true;
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
        GraphRemainingOrigin[] Origins,
        bool[] RawReliable);

    private readonly record struct ModelProjection(
        IReadOnlyDictionary<string, IReadOnlyList<double>> Values,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> Reliability);

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
