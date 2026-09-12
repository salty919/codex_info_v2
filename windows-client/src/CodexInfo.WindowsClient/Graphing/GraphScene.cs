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
    ResetBoundary,
    Raw,
    ActivitySmoothed,
    Interpolated,
    BoundedNullHold,
    TerminalNullHold,
    SyntheticTailHold,
    MonotonicHold,
}

/// <summary>The provenance of one accepted cumulative model value.</summary>
internal enum GraphModelOrigin
{
    Unknown,
    Direct,
    LegacyUnknown,
    Unverified,
    BoundedFlat,
    Interpolated,
    Held,
    Rejected,
}

/// <summary>
/// Framework-independent graph projection. It is the single owner of graph
/// data semantics; XAML owns layout and the ScottPlot adapter only paints the
/// arrays and fixed axes exposed here.
/// </summary>
public sealed class GraphScene
{
    // A gray band denotes a sustained session-level break. Two adjacent exact
    // flat intervals establish a candidate, but ordinary short publication
    // pauses must remain part of the foreground timeline.
    private const long SustainedUnusedMinimumSeconds = 30 * 60;
    private const long WeeklyQuotaWindowSeconds = 7 * 24 * 60 * 60;
    private const long ResetAtToleranceSeconds = 60;

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
        IReadOnlyDictionary<string, IReadOnlyList<bool>> modelLineReliability,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenWeightability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
        bool[] modelVectorAvailable,
        bool[] modelSynthetic,
        bool[] remainingObserved,
        double[] observedRemainingValues,
        bool[] remainingInterpolated,
        GraphRemainingOrigin[] remainingOrigins,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlyDictionary<string, IReadOnlySet<long>> modelCorrectionStarts,
        IReadOnlySet<long> correctionStarts,
        IReadOnlySet<long> tokenCorrectionStarts,
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
        ModelLineReliability = modelLineReliability;
        TokenWeightability = tokenWeightability;
        PublishedModelNames = publishedModelNames;
        ModelVectorAvailable = modelVectorAvailable;
        ModelSynthetic = modelSynthetic;
        RemainingObserved = remainingObserved;
        ObservedRemainingValues = observedRemainingValues;
        RemainingInterpolated = remainingInterpolated;
        RemainingOrigins = remainingOrigins;
        ConfirmedGaps = confirmedGaps;
        ModelCorrectionStarts = modelCorrectionStarts;
        CorrectionStarts = correctionStarts;
        TokenCorrectionStarts = tokenCorrectionStarts;
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

    internal IReadOnlyDictionary<string, IReadOnlyList<bool>> ModelLineReliability { get; }

    internal IReadOnlyDictionary<string, IReadOnlyList<bool>> TokenWeightability { get; }

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

    internal IReadOnlyDictionary<string, IReadOnlySet<long>> ModelCorrectionStarts { get; }

    internal IReadOnlySet<long> CorrectionStarts { get; }

    internal IReadOnlySet<long> TokenCorrectionStarts { get; }

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
            new Dictionary<string, IReadOnlySet<long>>(StringComparer.Ordinal),
            new HashSet<long>(),
            new HashSet<long>(),
            [],
            1);

    public static GraphScene Create(
        IReadOnlyList<ApiHistorySample> samples,
        GraphMetric metric,
        long periodStartAt,
        long periodEndAt) =>
        Create(samples, metric, periodStartAt, periodEndAt, null, null);

    internal static GraphScene Create(
        IReadOnlyList<ApiHistorySample> samples,
        GraphMetric metric,
        long periodStartAt,
        long periodEndAt,
        IReadOnlyList<GraphConfirmedGap>? confirmedGaps = null) =>
        Create(samples, metric, periodStartAt, periodEndAt, confirmedGaps, null);

    internal static GraphScene Create(
        IReadOnlyList<ApiHistorySample> samples,
        GraphMetric metric,
        long periodStartAt,
        long periodEndAt,
        IReadOnlyList<GraphConfirmedGap>? confirmedGaps,
        IReadOnlySet<string>? hiddenModelNames)
    {
        ArgumentNullException.ThrowIfNull(samples);
        if (samples.Count == 0)
        {
            return Empty(metric);
        }

        var start = periodStartAt >= 0 ? periodStartAt : samples[0].Timestamp;
        var end = periodEndAt > start ? periodEndAt : Math.Max(start + 1, samples[^1].Timestamp);
        var periodStartIsQuotaResetBoundary = IsPeriodStartQuotaResetBoundary(samples, start);
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
        var allModelNames = samples
            .SelectMany(PublishedModels)
            .Select(model => model.Name)
            .Distinct(StringComparer.Ordinal)
            .OrderBy(name => name, Utf8ModelNameComparer.Instance)
            .ToArray();
        var displayModelNames = hiddenModelNames is null
            ? allModelNames
            : allModelNames
                .Where(name => !hiddenModelNames.Contains(name))
                .ToArray();
        // Semantic evidence is derived from the complete published model
        // universe. A visibility toggle is a rendering concern only: hidden
        // models still participate in token activity, quota attribution,
        // idle detection, and completeness/correction evidence.
        var dollarProjection = BuildAcceptedModelProjection(samples, allModelNames, GraphMetric.Dollars);
        var tokenProjection = BuildAcceptedModelProjection(samples, allModelNames, GraphMetric.Tokens);
        var semanticProjection = metric == GraphMetric.Dollars
            ? dollarProjection
            : tokenProjection;
        var displayProjection = semanticProjection.Filter(displayModelNames);
        var activityTokenProjection = ActivityRelevantTokenProjection(tokenProjection);
        var publishedModelNames = samples
            .Select(sample => (IReadOnlySet<string>)(sample.IsSyntheticTail ||
                sample.ModelSource is ApiHistorySample.UnavailableModelSource or
                    ApiHistorySample.ReconstructedFromSessionModelSource
                    ? new HashSet<string>(StringComparer.Ordinal)
                    : PublishedModels(sample)
                        .Where(model => model.TotalTokens is not null)
                        .Select(model => model.Name)
                        .ToHashSet(StringComparer.Ordinal)))
            .ToArray();
        var points = new ScenePoint[samples.Count];
        var correctionStarts = displayProjection.CorrectionStarts;
        var tokenCorrectionStarts = tokenProjection.CorrectionStarts;
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
            var currentModels = displayModelNames
                .Select(name => displayProjection.Values[name][index])
                .ToArray();
            var currentSol = ModelValue(displayModelNames, currentModels, "SOL");
            var currentTerra = ModelValue(displayModelNames, currentModels, "TERRA");
            var currentLuna = ModelValue(displayModelNames, currentModels, "LUNA");
            var currentAstra = ModelValue(displayModelNames, currentModels, "ASTRA");
            var modelDataAvailable = allModelNames.Length > 0 && allModelNames.All(name =>
                double.IsFinite(semanticProjection.Values[name][index]) &&
                semanticProjection.Reliability[name][index]);
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
                sample.IsSyntheticTail,
                ResetBoundary: IsResetBoundaryPoint(sample, start),
                TaskActiveSincePrevious: sample.TaskActiveSincePrevious);
        }

        var remainingProjection = BuildEffectiveRemainingWithOrigins(
            points,
            activityTokenProjection.Values,
            activityTokenProjection.Reliability,
            activityTokenProjection.Weightability,
            publishedModelNames,
            normalizedGaps,
            tokenCorrectionStarts,
            CanReconstructResetBoundary(
                points,
                start,
                activityTokenProjection.Values,
                activityTokenProjection.Reliability,
                normalizedGaps,
                periodStartIsQuotaResetBoundary));
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
            new[] { sol, terra, luna, astra }
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
            activityTokenProjection.Values,
            displayProjection.Reliability,
            activityTokenProjection.Reliability,
            displayProjection.LineReliability,
            activityTokenProjection.Weightability,
            publishedModelNames,
            modelVectorAvailable,
            modelSynthetic,
            remainingObserved,
            observedRemainingValues,
            remainingInterpolated,
            remainingOrigins,
            normalizedGaps,
            displayProjection.CorrectionStartsByModel,
            correctionStarts,
            tokenCorrectionStarts,
            BuildConfirmedIdleIntervals(samples, start, end, normalizedGaps),
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
        return name is null || !ModelLineReliability.TryGetValue(name, out var reliability) ||
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
        var lineReliability = new Dictionary<string, IReadOnlyList<bool>>(StringComparer.Ordinal);
        var weightability = new Dictionary<string, IReadOnlyList<bool>>(StringComparer.Ordinal);
        var origins = new Dictionary<string, IReadOnlyList<GraphModelOrigin>>(StringComparer.Ordinal);
        var correctionStartsByModel = new Dictionary<string, IReadOnlySet<long>>(StringComparer.Ordinal);
        var correctionStarts = new HashSet<long>();
        foreach (var name in modelNames)
        {
            var modelCorrectionStarts = new HashSet<long>();
            var raw = samples
                .Select(sample => RawModelValue(sample, name, metric))
                .ToArray();
            var sourceOrigins = samples
                .Select(sample => sample.ModelSource switch
                {
                    ApiHistorySample.ConfirmedModelSource when sample.ModelsComplete =>
                        GraphModelOrigin.Direct,
                    ApiHistorySample.LegacyUnknownModelSource =>
                        GraphModelOrigin.LegacyUnknown,
                    _ => GraphModelOrigin.Unverified,
                })
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
                    sourceOrigins[index - 1] is GraphModelOrigin.Direct &&
                    sourceOrigins[index] is GraphModelOrigin.Direct &&
                    sourceOrigins[index + 1] is GraphModelOrigin.Direct &&
                    double.IsFinite(left) && left >= 0 &&
                    double.IsFinite(middle) && middle >= 0 &&
                    double.IsFinite(right) && right >= 0 &&
                    left <= right && (middle < left || middle > right);
            }

            var accepted = Enumerable.Repeat(double.NaN, samples.Count).ToArray();
            var acceptedOrigins = Enumerable.Repeat(GraphModelOrigin.Unknown, samples.Count).ToArray();
            double? directBaseline = null;
            for (var index = 0; index < samples.Count; index++)
            {
                var candidate = raw[index];
                if (samples[index].IsSyntheticTail || !double.IsFinite(candidate) || candidate < 0)
                {
                    continue;
                }
                if (sourceOrigins[index] is GraphModelOrigin.LegacyUnknown)
                {
                    accepted[index] = candidate;
                    acceptedOrigins[index] = GraphModelOrigin.LegacyUnknown;
                    continue;
                }
                if (sourceOrigins[index] is not GraphModelOrigin.Direct)
                {
                    continue;
                }
                if (isolated[index] || directBaseline is { } prior && candidate < prior)
                {
                    modelCorrectionStarts.Add(samples[index].Timestamp);
                    correctionStarts.Add(samples[index].Timestamp);
                    accepted[index] = directBaseline ?? candidate;
                    acceptedOrigins[index] = GraphModelOrigin.Rejected;
                    continue;
                }
                accepted[index] = candidate;
                acceptedOrigins[index] = GraphModelOrigin.Direct;
                directBaseline = candidate;
            }

            // Legacy rows are display-only. Keep one only when it fits between
            // the surrounding accepted direct observations; otherwise leave
            // the slot unknown so the UI projection below can visibly infer it.
            // A legacy value never updates directBaseline or correctionStarts.
            var nextDirectValue = Enumerable.Repeat(double.NaN, samples.Count).ToArray();
            var nextDirect = double.NaN;
            for (var index = samples.Count - 1; index >= 0; index--)
            {
                nextDirectValue[index] = nextDirect;
                if (acceptedOrigins[index] is GraphModelOrigin.Direct)
                {
                    nextDirect = accepted[index];
                }
            }
            var displayFloor = double.NaN;
            for (var index = 0; index < samples.Count; index++)
            {
                if (acceptedOrigins[index] is GraphModelOrigin.Direct)
                {
                    displayFloor = accepted[index];
                    continue;
                }
                if (acceptedOrigins[index] is not GraphModelOrigin.LegacyUnknown)
                {
                    continue;
                }
                var belowFloor = double.IsFinite(displayFloor) && accepted[index] < displayFloor;
                var aboveCeiling = double.IsFinite(nextDirectValue[index]) &&
                    accepted[index] > nextDirectValue[index];
                if (belowFloor || aboveCeiling)
                {
                    accepted[index] = double.NaN;
                    acceptedOrigins[index] = GraphModelOrigin.Unknown;
                }
                else
                {
                    displayFloor = accepted[index];
                }
            }

            var anchors = Enumerable.Range(0, samples.Count)
                .Where(index => ModelOriginArithmeticReliable(acceptedOrigins[index]))
                .ToArray();
            for (var anchor = 1; anchor < anchors.Length; anchor++)
            {
                var left = anchors[anchor - 1];
                var right = anchors[anchor];
                var elapsed = samples[right].Timestamp - samples[left].Timestamp;
                if (elapsed <= 0 || !double.IsFinite(accepted[left]) || !double.IsFinite(accepted[right]) ||
                    accepted[left] < 0 || accepted[right] < accepted[left])
                {
                    continue;
                }
                for (var index = left + 1; index < right; index++)
                {
                    if (acceptedOrigins[index] is not GraphModelOrigin.Unknown)
                    {
                        continue;
                    }
                    var fraction = (double)(samples[index].Timestamp - samples[left].Timestamp) / elapsed;
                    accepted[index] = accepted[left] + (accepted[right] - accepted[left]) * fraction;
                    acceptedOrigins[index] = accepted[right] == accepted[left]
                        ? GraphModelOrigin.BoundedFlat
                        : GraphModelOrigin.Interpolated;
                }
            }

            if (anchors.LastOrDefault(-1) is var lastAnchor && lastAnchor >= 0)
            {
                for (var index = lastAnchor + 1; index < samples.Count; index++)
                {
                    if (acceptedOrigins[index] is not GraphModelOrigin.Unknown)
                    {
                        continue;
                    }
                    accepted[index] = accepted[lastAnchor];
                    acceptedOrigins[index] = GraphModelOrigin.Held;
                }
            }

            // A newer valid legacy observation is still the latest saved
            // display value. It may update only the unbounded presentation
            // tail; Held remains non-authoritative for arithmetic and idle.
            var lastDisplayObservation = Enumerable.Range(0, samples.Count)
                .Where(index => acceptedOrigins[index] is
                    GraphModelOrigin.Direct or GraphModelOrigin.LegacyUnknown)
                .LastOrDefault(-1);
            if (lastDisplayObservation >= 0)
            {
                for (var index = lastDisplayObservation + 1; index < samples.Count; index++)
                {
                    if (acceptedOrigins[index] is not
                        (GraphModelOrigin.Unknown or GraphModelOrigin.Held))
                    {
                        continue;
                    }
                    accepted[index] = accepted[lastDisplayObservation];
                    acceptedOrigins[index] = GraphModelOrigin.Held;
                }
            }

            ShapeInferredModelSeriesByTaskActivity(samples, accepted, acceptedOrigins);
            var acceptedReliability = acceptedOrigins
                .Select(ModelOriginArithmeticReliable)
                .ToArray();
            var acceptedLineReliability = acceptedOrigins
                .Select(ModelOriginLineIsExact)
                .ToArray();
            var acceptedWeightability = acceptedOrigins
                .Select(ModelOriginDisplayWeightable)
                .ToArray();
            values[name] = accepted;
            reliability[name] = acceptedReliability;
            lineReliability[name] = acceptedLineReliability;
            weightability[name] = acceptedWeightability;
            origins[name] = acceptedOrigins;
            correctionStartsByModel[name] = modelCorrectionStarts;
        }
        return new ModelProjection(
            values,
            reliability,
            lineReliability,
            weightability,
            origins,
            correctionStartsByModel,
            correctionStarts);
    }

    private static void ShapeInferredModelSeriesByTaskActivity(
        IReadOnlyList<ApiHistorySample> samples,
        double[] values,
        GraphModelOrigin[] origins)
    {
        var anchors = Enumerable.Range(0, samples.Count)
            .Where(index => ModelOriginLineIsExact(origins[index]))
            .ToArray();
        for (var anchor = 1; anchor < anchors.Length; anchor++)
        {
            var left = anchors[anchor - 1];
            var right = anchors[anchor];
            if (samples[right].Timestamp <= samples[left].Timestamp ||
                !double.IsFinite(values[left]) || !double.IsFinite(values[right]) ||
                values[left] < 0 || values[right] < values[left])
            {
                continue;
            }

            var hasIdle = false;
            var activeDuration = 0d;
            var complete = true;
            for (var index = left + 1; index <= right; index++)
            {
                if (samples[index].TaskActiveSincePrevious is not { } active)
                {
                    complete = false;
                    break;
                }
                var elapsed = Math.Max(0, samples[index].Timestamp - samples[index - 1].Timestamp);
                hasIdle |= !active;
                if (active)
                {
                    activeDuration += elapsed;
                }
            }
            if (!complete || !hasIdle || values[right] > values[left] && activeDuration <= double.Epsilon)
            {
                continue;
            }

            var activeElapsed = 0d;
            for (var index = left + 1; index < right; index++)
            {
                var active = samples[index].TaskActiveSincePrevious is true;
                if (active)
                {
                    activeElapsed += Math.Max(0, samples[index].Timestamp - samples[index - 1].Timestamp);
                }
                var fraction = values[right] == values[left]
                    ? 0
                    : Math.Clamp(activeElapsed / activeDuration, 0, 1);
                values[index] = values[left] + (values[right] - values[left]) * fraction;
                origins[index] = active
                    ? GraphModelOrigin.Interpolated
                    : GraphModelOrigin.BoundedFlat;
            }
        }
    }

    private static ModelProjection ActivityRelevantTokenProjection(ModelProjection projection)
    {
        var neutral = projection.Values.Keys
            .Where(name =>
                projection.Values[name].Select((value, index) => (value, index))
                    .Any(point => projection.Reliability[name][point.index] && point.value == 0) &&
                projection.Values[name].Select((value, index) => (value, index))
                    .All(point => double.IsFinite(point.value) && point.value == 0 &&
                        ModelOriginArithmeticReliable(projection.Origins[name][point.index])))
            .ToHashSet(StringComparer.Ordinal);
        var relevant = projection.Values.Keys
            .Where(name => !neutral.Contains(name))
            .ToArray();
        if (relevant.Length == 0)
        {
            return projection;
        }

        return projection.Filter(relevant);
    }

    private static bool ModelOriginArithmeticReliable(GraphModelOrigin origin) =>
        origin is GraphModelOrigin.Direct;

    private static bool ModelOriginLineIsExact(GraphModelOrigin origin) =>
        origin is GraphModelOrigin.Direct;

    private static bool ModelOriginDisplayWeightable(GraphModelOrigin origin) =>
        ModelOriginArithmeticReliable(origin);

    private static double RawModelValue(ApiHistorySample sample, string name, GraphMetric metric)
    {
        if (sample.ModelSource != ApiHistorySample.LegacyUnknownModelSource &&
            (sample.ModelSource != ApiHistorySample.ConfirmedModelSource ||
             !sample.ModelsComplete))
        {
            return double.NaN;
        }
        var model = PublishedModels(sample).FirstOrDefault(candidate => candidate.Name == name);
        if (model is not null)
        {
            return ModelValue(sample, model, metric);
        }
        return double.NaN;
    }

    private static bool IsResetBoundaryPoint(ApiHistorySample sample, long periodStart) =>
        sample.Timestamp == periodStart &&
        (sample.RemainingPercent is not { } raw ||
         !double.IsFinite(raw) || raw is < 0 or > 100);

    private static bool IsPeriodStartQuotaResetBoundary(
        IReadOnlyList<ApiHistorySample> samples,
        long periodStart)
    {
        foreach (var sample in samples)
        {
            if (sample.ResetAt >= long.MinValue + WeeklyQuotaWindowSeconds &&
                WithinResetAtTolerance(
                    sample.ResetAt - WeeklyQuotaWindowSeconds,
                    periodStart))
            {
                return true;
            }

            try
            {
                var monthlyStart = DateTimeOffset
                    .FromUnixTimeSeconds(sample.ResetAt)
                    .AddMonths(-1)
                    .ToUnixTimeSeconds();
                if (WithinResetAtTolerance(monthlyStart, periodStart))
                {
                    return true;
                }
            }
            catch (ArgumentOutOfRangeException)
            {
                // The wire parser normally rejects values outside the Unix
                // range. Keep this projection fail-closed if a direct caller
                // supplies one anyway.
            }
        }

        return false;
    }

    private static bool WithinResetAtTolerance(long left, long right) =>
        left >= right
            ? left - right <= ResetAtToleranceSeconds
            : right - left <= ResetAtToleranceSeconds;

    private static bool CanReconstructResetBoundary(
        IReadOnlyList<ScenePoint> points,
        long periodStart,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        bool periodStartIsQuotaResetBoundary)
    {
        if (!periodStartIsQuotaResetBoundary)
        {
            return false;
        }

        var boundary = Enumerable.Range(0, points.Count)
            .FirstOrDefault(index => points[index].ResetBoundary, -1);
        var firstObserved = Enumerable.Range(0, points.Count)
            .FirstOrDefault(index => points[index].Remaining is { } raw &&
                double.IsFinite(raw) && raw is >= 0 and <= 100, -1);
        if (boundary < 0 || firstObserved <= boundary ||
            points[boundary].Timestamp != periodStart ||
            HasConfirmedGapBetween(
                confirmedGaps,
                periodStart,
                points[firstObserved].Timestamp))
        {
            return false;
        }

        return tokenSeries.Any(pair =>
            tokenReliability.TryGetValue(pair.Key, out var reliability) &&
            pair.Value.Count == points.Count &&
            reliability.Count == points.Count &&
            Enumerable.Range(boundary, firstObserved - boundary + 1).Any(index =>
                reliability[index] && double.IsFinite(pair.Value[index]) &&
                pair.Value[index] >= 0));
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
                reliability,
                published,
                confirmedGaps ?? Array.Empty<GraphConfirmedGap>(),
                new HashSet<long>(),
                resetBoundaryEligible: false)
            .Values;
    }

    private static RemainingProjection BuildEffectiveRemainingWithOrigins(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenWeightability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
        bool resetBoundaryEligible)
    {
        var values = new double?[points.Count];
        var origins = Enumerable.Repeat(GraphRemainingOrigin.Missing, points.Count).ToArray();
        var rawReliable = new bool[points.Count];
        var rawValues = points
            .Select(point => point.ResetBoundary && resetBoundaryEligible
                ? 100d
                : !point.SyntheticTail && point.Remaining is { } raw &&
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
                origins[index] = points[index].ResetBoundary && resetBoundaryEligible
                    ? GraphRemainingOrigin.ResetBoundary
                    : GraphRemainingOrigin.Raw;
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
            for (var index = runStart; index < runEnd; index++)
            {
                if (values[index - 1] is not { } prior)
                {
                    break;
                }
                values[index] = prior;
                origins[index] = points[index].SyntheticTail
                    ? GraphRemainingOrigin.SyntheticTailHold
                    : bounded
                        ? GraphRemainingOrigin.BoundedNullHold
                        : GraphRemainingOrigin.TerminalNullHold;
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
            var activity = new List<ProjectedTokenDelta?>();
            var tokenEvidenceComplete = true;
            var tokenWeight = 0d;
            for (var segment = left; segment < right; segment++)
            {
                var projected = ProjectTokenIntervalDelta(
                    points,
                    tokenSeries,
                    tokenReliability,
                    tokenWeightability,
                    confirmedGaps,
                    correctionStarts,
                    segment,
                    segment + 1);
                if (projected is null)
                {
                    tokenEvidenceComplete = false;
                }
                else
                {
                    tokenWeight += projected.Value.Delta;
                }
                activity.Add(projected);
            }

            var useTokenWeights = tokenEvidenceComplete && tokenWeight > double.Epsilon;
            if (useTokenWeights)
            {
                // Every interval has an exact or bounded-theoretical token
                // delta. Allocate the quota drop by those deltas; zero-delta
                // intervals remain horizontal and sparse theoretical spans
                // retain inferred provenance.
            }
            else
            {
                // A single incomplete/contradictory interval makes the
                // attributable part of the anchor span time-weighted. Exact
                // zero-token intervals remain weightless so a later unknown
                // interval cannot smear quota loss backwards through a proven
                // idle run. If every interval is known but the total token
                // delta is zero, the quota drop is contradictory and the
                // entire span remains an inferred elapsed-time bridge.
                var elapsedWeight = 0d;
                for (var segment = left; segment < right; segment++)
                {
                    var index = segment - left;
                    var exactZero = !tokenEvidenceComplete &&
                        activity[index] is { ExactZero: true };
                    var weight = exactZero
                        ? 0d
                        : points[segment + 1].Timestamp - points[segment].Timestamp;
                    elapsedWeight += weight;
                    activity[index] = new ProjectedTokenDelta(
                        weight,
                        Inferred: !exactZero,
                        ExactZero: exactZero);
                }
                tokenWeight = elapsedWeight;
            }

            if (tokenEvidenceComplete && !useTokenWeights)
            {
                // A quota drop with zero tokens everywhere is contradictory.
                // Do not claim idle; connect the accepted quota anchors only
                // with a wholly inferred elapsed-time bridge.
                tokenWeight = 0d;
                for (var segment = left; segment < right; segment++)
                {
                    var weight = points[segment + 1].Timestamp - points[segment].Timestamp;
                    tokenWeight += weight;
                    activity[segment - left] = new ProjectedTokenDelta(
                        weight,
                        Inferred: true,
                        ExactZero: false);
                }
            }

            var weightedSeconds = tokenWeight;
            if (weightedSeconds <= double.Epsilon)
            {
                continue;
            }
            var weightedElapsed = 0d;
            for (var index = left + 1; index < right; index++)
            {
                var interval = activity[index - left - 1]
                    ?? throw new InvalidOperationException("Quota span weight was not resolved.");
                weightedElapsed += interval.Weight;
                var smoothed = leftValue +
                    (rightValue - leftValue) * (weightedElapsed / weightedSeconds);
                values[index] = smoothed;
                if (interval.Inferred)
                {
                    origins[index] = GraphRemainingOrigin.Interpolated;
                }
                else if (!(rawReliable[index] && rawValues[index] == smoothed))
                {
                    // A changed presentation value is not by itself missing
                    // evidence.  Normal staircase smoothing retains the raw
                    // quota observation at this timestamp and is a measured,
                    // solid line.  Only a raw-null point is interpolation.
                    origins[index] = !interval.Inferred && rawReliable[index] && rawValues[index] is not null
                        ? GraphRemainingOrigin.ActivitySmoothed
                        : GraphRemainingOrigin.Interpolated;
                }
            }
        }

        return new RemainingProjection(values, origins, rawReliable);
    }

    /// <summary>
    /// Computes idle bands from the recorder's direct raw observations. The
    /// renderer may add holds, smoothing, and interpolation, but those values
    /// are intentionally absent from this authority path.
    /// </summary>
    private static IReadOnlyList<GraphIdleInterval> BuildConfirmedIdleIntervals(
        IReadOnlyList<ApiHistorySample> samples,
        long periodStart,
        long periodEnd,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps)
    {
        if (periodEnd <= periodStart || samples.Count < 2)
        {
            return Array.Empty<GraphIdleInterval>();
        }

        var direct = new List<(int Index, IReadOnlyDictionary<string, DirectModelValue> Vector)>();
        for (var index = 0; index < samples.Count; index++)
        {
            if (TryGetDirectModelVector(samples[index], out var vector) &&
                double.IsFinite(samples[index].RemainingPercent ?? double.NaN))
            {
                direct.Add((index, vector));
            }
        }

        var candidates = new List<GraphIdleInterval>();
        for (var observation = 1; observation < direct.Count; observation++)
        {
            var before = direct[observation - 1];
            var after = direct[observation];
            var left = samples[before.Index];
            var right = samples[after.Index];
            if (right.Timestamp <= left.Timestamp ||
                left.ResetAt != right.ResetAt ||
                !RemainingBitsEqual(left.RemainingPercent!.Value, right.RemainingPercent!.Value) ||
                !TokenVectorsEqual(before.Vector, after.Vector) ||
                HasConfirmedGapBetween(
                    confirmedGaps,
                    left.Timestamp,
                    right.Timestamp) ||
                HasDirectIdleContradiction(
                    samples,
                    before.Index,
                    after.Index,
                    left.ResetAt,
                    before.Vector))
            {
                continue;
            }

            var start = Math.Max(periodStart, left.Timestamp);
            var end = Math.Min(periodEnd, right.Timestamp);
            if (end > start)
            {
                candidates.Add(new GraphIdleInterval(start, end, false));
            }
        }

        var merged = new List<GraphIdleInterval>();
        foreach (var candidate in candidates.OrderBy(interval => interval.StartAt))
        {
            if (merged.Count > 0 && candidate.StartAt <= merged[^1].EndAt)
            {
                merged[^1] = merged[^1] with
                {
                    EndAt = Math.Max(merged[^1].EndAt, candidate.EndAt),
                };
            }
            else
            {
                merged.Add(candidate);
            }
        }

        return merged
            .Where(interval => interval.EndAt - interval.StartAt >= SustainedUnusedMinimumSeconds)
            .ToArray();
    }

    private static bool HasDirectIdleContradiction(
        IReadOnlyList<ApiHistorySample> samples,
        int before,
        int after,
        long resetAt,
        IReadOnlyDictionary<string, DirectModelValue> baseline)
    {
        for (var index = before + 1; index <= after; index++)
        {
            var sample = samples[index];
            if (sample.TaskActiveSincePrevious is true)
            {
                return true;
            }

            if (sample.ModelSource != ApiHistorySample.ConfirmedModelSource ||
                !sample.ModelsComplete)
            {
                continue;
            }

            if (sample.ResetAt != resetAt ||
                !TryGetDirectModelVector(sample, out var vector) ||
                !TokenVectorsEqual(baseline, vector))
            {
                return true;
            }
        }

        return false;
    }

    private static bool TryGetDirectModelVector(
        ApiHistorySample sample,
        out IReadOnlyDictionary<string, DirectModelValue> vector)
    {
        vector = new Dictionary<string, DirectModelValue>(StringComparer.Ordinal);
        if (sample.IsSyntheticTail ||
            sample.ModelSource != ApiHistorySample.ConfirmedModelSource ||
            !sample.ModelsComplete)
        {
            return false;
        }

        var models = PublishedModels(sample).ToArray();
        if (models.Length == 0)
        {
            return false;
        }

        var result = new Dictionary<string, DirectModelValue>(StringComparer.Ordinal);
        foreach (var model in models)
        {
            if (model.TotalTokens is not ulong totalTokens ||
                model.TotalDollars is double dollars && !double.IsFinite(dollars))
            {
                vector = new Dictionary<string, DirectModelValue>(StringComparer.Ordinal);
                return false;
            }
            if (!result.TryAdd(model.Name, new DirectModelValue(totalTokens, model.TotalDollars)))
            {
                vector = new Dictionary<string, DirectModelValue>(StringComparer.Ordinal);
                return false;
            }
        }

        vector = result;
        return true;
    }

    private static bool TokenVectorsEqual(
        IReadOnlyDictionary<string, DirectModelValue> left,
        IReadOnlyDictionary<string, DirectModelValue> right) =>
        left.Count == right.Count && left.Keys.All(name =>
            right.TryGetValue(name, out var candidate) &&
            candidate.TotalTokens == left[name].TotalTokens);

    private static bool RemainingBitsEqual(double left, double right) =>
        BitConverter.DoubleToInt64Bits(left) == BitConverter.DoubleToInt64Bits(right);

    private static bool HasTokenIntervalEvidence(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
        int before,
        int after,
        out bool advanced) =>
        HasTokenIntervalEvidence(
            points,
            tokenSeries,
            tokenReliability,
            publishedModelNames,
            confirmedGaps,
            correctionStarts,
            before,
            after,
            out advanced,
            out _);

    private static bool HasTokenIntervalEvidence(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyList<IReadOnlySet<string>> publishedModelNames,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
        int before,
        int after,
        out bool advanced,
        out double tokenDelta)
    {
        advanced = false;
        tokenDelta = 0;
        if (before < 0 || after != before + 1 || after >= points.Count ||
            points[after].Timestamp <= points[before].Timestamp ||
            points[after].Timestamp - points[before].Timestamp > 60 ||
            points[before].SyntheticTail || points[after].SyntheticTail ||
            HasConfirmedGapBetween(confirmedGaps, points[before].Timestamp, points[after].Timestamp) ||
            HasCorrectionBetween(correctionStarts, points[before].Timestamp, points[after].Timestamp))
        {
            return false;
        }
        if (tokenSeries.Count == 0)
        {
            return false;
        }
        foreach (var name in tokenSeries.Keys)
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
            var delta = values[after] - values[before];
            tokenDelta += delta;
            advanced |= delta > 0;
        }
        return double.IsFinite(tokenDelta);
    }

    private static ProjectedTokenDelta? ProjectTokenIntervalDelta(
        IReadOnlyList<ScenePoint> points,
        IReadOnlyDictionary<string, IReadOnlyList<double>> tokenSeries,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenReliability,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> tokenWeightability,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<long> correctionStarts,
        int before,
        int after)
    {
        if (before < 0 || after != before + 1 || after >= points.Count ||
            points[after].Timestamp <= points[before].Timestamp ||
            points[before].SyntheticTail || points[after].SyntheticTail ||
            HasConfirmedGapBetween(confirmedGaps, points[before].Timestamp, points[after].Timestamp) ||
            HasCorrectionBetween(correctionStarts, points[before].Timestamp, points[after].Timestamp) ||
            tokenSeries.Count == 0)
        {
            return null;
        }

        var total = 0d;
        var exact = true;
        var inferred = points[after].Timestamp - points[before].Timestamp > 60;
        foreach (var name in tokenSeries.Keys)
        {
            if (!tokenSeries.TryGetValue(name, out var values) ||
                !tokenReliability.TryGetValue(name, out var reliable) ||
                !tokenWeightability.TryGetValue(name, out var weightable) ||
                before >= values.Count || after >= values.Count ||
                before >= reliable.Count || after >= reliable.Count ||
                before >= weightable.Count || after >= weightable.Count ||
                !weightable[before] || !weightable[after] ||
                !double.IsFinite(values[before]) || !double.IsFinite(values[after]) ||
                values[before] < 0 || values[after] < values[before])
            {
                return null;
            }
            total += values[after] - values[before];
            if (!double.IsFinite(total))
            {
                return null;
            }
            exact &= reliable[before] && reliable[after];
            inferred |= !reliable[before] || !reliable[after];
        }

        return new ProjectedTokenDelta(
            total,
            Inferred: inferred,
            ExactZero: exact && total <= double.Epsilon);
    }

    internal bool TryGetTokenIntervalEvidence(int before, int after, out bool advanced) =>
        TryGetTokenIntervalEvidence(before, after, out advanced, out _);

    internal bool TryGetTokenIntervalEvidence(
        int before,
        int after,
        out bool advanced,
        out double tokenDelta)
    {
        advanced = false;
        tokenDelta = 0;
        if (before < 0 || after != before + 1 || after >= Timestamps.Count ||
            Timestamps[after] <= Timestamps[before] ||
            Timestamps[after] - Timestamps[before] > 60 ||
            ModelSynthetic[before] || ModelSynthetic[after] ||
            HasRemainingHardBreakBetween(Timestamps[before], Timestamps[after]))
        {
            return false;
        }
        if (TokenModelSeries.Count == 0)
        {
            return false;
        }
        foreach (var name in TokenModelSeries.Keys)
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
            var delta = values[after] - values[before];
            tokenDelta += delta;
            advanced |= delta > 0;
        }
        return double.IsFinite(tokenDelta);
    }

    internal bool HasConfirmedGapBetween(double startAt, double endAt) =>
        HasConfirmedGapBetween(ConfirmedGaps, startAt, endAt);

    internal bool HasCorrectionBetween(double startAt, double endAt) =>
        HasCorrectionBetween(CorrectionStarts, startAt, endAt);

    internal bool HasTokenCorrectionBetween(double startAt, double endAt) =>
        HasCorrectionBetween(TokenCorrectionStarts, startAt, endAt);

    internal bool HasModelHardBreakBetween(double startAt, double endAt) =>
        HasConfirmedGapBetween(startAt, endAt) || HasCorrectionBetween(startAt, endAt);

    internal bool HasModelHardBreakBetween(
        IReadOnlyList<double> values,
        double startAt,
        double endAt)
    {
        var name = ModelSeries
            .FirstOrDefault(pair => ReferenceEquals(pair.Value, values))
            .Key;
        if (name is not null && ModelCorrectionStarts.TryGetValue(name, out var correctionStarts))
        {
            return HasConfirmedGapBetween(startAt, endAt) ||
                HasCorrectionBetween(correctionStarts, startAt, endAt);
        }

        // An unregistered series has no model identity. Preserve the conservative
        // union behavior so an unknown correction boundary is never painted solid.
        return HasModelHardBreakBetween(startAt, endAt);
    }

    internal bool HasRemainingHardBreakBetween(double startAt, double endAt) =>
        HasConfirmedGapBetween(startAt, endAt) || HasTokenCorrectionBetween(startAt, endAt);

    internal bool HasHardBreakBetween(double startAt, double endAt) =>
        HasModelHardBreakBetween(startAt, endAt);

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
        ApiHistorySample sample,
        ApiHistoryModelSample model,
        GraphMetric metric)
    {
        if (metric == GraphMetric.Dollars)
        {
            // A missing wire dollar is missing evidence. Token-derived price
            // reconstruction belongs to the recorder, not this renderer.
            return FiniteNonNegative(model.TotalDollars);
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
        if (sample.ModelSource is ApiHistorySample.UnavailableModelSource or
            ApiHistorySample.ReconstructedFromSessionModelSource)
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

    private sealed record ModelProjection(
        IReadOnlyDictionary<string, IReadOnlyList<double>> Values,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> Reliability,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> LineReliability,
        IReadOnlyDictionary<string, IReadOnlyList<bool>> Weightability,
        IReadOnlyDictionary<string, IReadOnlyList<GraphModelOrigin>> Origins,
        IReadOnlyDictionary<string, IReadOnlySet<long>> CorrectionStartsByModel,
        IReadOnlySet<long> CorrectionStarts)
    {
        internal ModelProjection Filter(IReadOnlyList<string> names) => new(
            names.ToDictionary(name => name, name => Values[name], StringComparer.Ordinal),
            names.ToDictionary(name => name, name => Reliability[name], StringComparer.Ordinal),
            names.ToDictionary(name => name, name => LineReliability[name], StringComparer.Ordinal),
            names.ToDictionary(name => name, name => Weightability[name], StringComparer.Ordinal),
            names.ToDictionary(name => name, name => Origins[name], StringComparer.Ordinal),
            names.ToDictionary(name => name, name => CorrectionStartsByModel[name], StringComparer.Ordinal),
            names.SelectMany(name => CorrectionStartsByModel[name]).ToHashSet());
    }

    private readonly record struct ProjectedTokenDelta(
        double Weight,
        bool Inferred,
        bool ExactZero)
    {
        internal double Delta => Weight;
    }

    private readonly record struct DirectModelValue(ulong TotalTokens, double? TotalDollars);

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
        bool SyntheticTail = false,
        bool ResetBoundary = false,
        bool? TaskActiveSincePrevious = null);
}
