// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.ViewModels;

namespace CodexInfo.WindowsClient.Graphing;

public sealed partial class GraphScene
{
    internal bool IsRecordedScope { get; private init; }
    internal bool IsSavedObservation { get; private init; }
    internal ApiRecordedGraphBundle? RecordedBundle { get; private init; }
    internal TimeZoneInfo? RecordedDisplayTimeZone { get; private init; }

    internal static GraphScene CreateRecorded(ApiRecordedGraphBundle bundle, GraphMetric metric, bool saved,
        bool daily, TimeZoneInfo zone, IReadOnlySet<string>? hiddenFamilies = null,
        IReadOnlyList<ApiHistorySample>? legacySavedQuota = null)
    {
        var scope = bundle.Scope ?? throw new ArgumentException("A recording scene requires an explicit scope.", nameof(bundle));
        var modelRows = saved
            ? bundle.Observations.Select(observation => (observation.Timestamp,
                Models: observation.Models.Select(model => (Name: model.StoredModelKey, Tokens: model.TotalTokens,
                    Dollars: model.TotalDollars)).ToArray())).ToArray()
            : bundle.Samples.Select(sample => (sample.Timestamp,
                Models: sample.Models.Select(model => (Name: model.Name, Tokens: (ulong?)model.TotalTokens,
                    Dollars: model.Cost?.TotalDollars)).ToArray())).ToArray();
        var quotaRows = bundle.QuotaObservations.Select(row => (Timestamp: row.ObservedAt, Remaining: row.RemainingPercent))
            .Concat((legacySavedQuota ?? []).Where(row => row.RemainingPercent is not null)
                .Select(row => (row.Timestamp, Remaining: row.RemainingPercent!.Value))).ToArray();
        var rows = modelRows.Select((row, index) => (At: row.Timestamp, Model: index, Quota: -1))
            .Concat(quotaRows.Select((row, index) => (At: row.Timestamp, Model: -1, Quota: index)))
            .OrderBy(row => row.At).ThenBy(row => row.Model < 0 ? 1 : 0).ToArray();
        var names = modelRows.SelectMany(row => row.Models).Select(model => model.Name).Distinct(StringComparer.Ordinal)
            .Where(name => hiddenFamilies is null || !hiddenFamilies.Contains(ModelUsageViewModel.DisplayFamilyName(name) ?? name)).ToArray();
        var values = names.ToDictionary(name => name, _ => (IReadOnlyList<double>)Enumerable.Repeat(double.NaN, rows.Length).ToArray(), StringComparer.Ordinal);
        var tokens = names.ToDictionary(name => name, _ => (IReadOnlyList<double>)Enumerable.Repeat(double.NaN, rows.Length).ToArray(), StringComparer.Ordinal);
        var reliability = names.ToDictionary(name => name, _ => (IReadOnlyList<bool>)new bool[rows.Length], StringComparer.Ordinal);
        var remaining = Enumerable.Repeat(double.NaN, rows.Length).ToArray();
        var vector = new bool[rows.Length];
        var remainingObserved = new bool[rows.Length];
        var origins = new GraphRemainingOrigin[rows.Length];
        var published = new IReadOnlySet<string>[rows.Length];
        for (var index = 0; index < rows.Length; index++)
        {
            var row = rows[index];
            published[index] = new HashSet<string>(StringComparer.Ordinal);
            if (row.Quota >= 0)
            {
                remaining[index] = quotaRows[row.Quota].Remaining;
                remainingObserved[index] = true;
                origins[index] = GraphRemainingOrigin.Raw;
            }
            if (row.Model < 0) continue;
            vector[index] = true;
            var models = modelRows[row.Model].Models.Where(model => values.ContainsKey(model.Name)).ToArray();
            published[index] = models.Select(model => model.Name).ToHashSet(StringComparer.Ordinal);
            foreach (var model in models)
            {
                ((double[])tokens[model.Name])[index] = model.Tokens is { } total ? total : double.NaN;
                ((double[])values[model.Name])[index] = metric == GraphMetric.Tokens
                    ? model.Tokens is { } token ? token : double.NaN : model.Dollars ?? double.NaN;
                ((bool[])reliability[model.Name])[index] = true;
            }
        }
        double[] Family(string family) => Enumerable.Range(0, rows.Length).Select(index =>
        {
            var members = values.Where(member => ModelUsageViewModel.DisplayFamilyName(member.Key) == family)
                .Select(member => member.Value[index]).ToArray();
            var known = members.Where(double.IsFinite).ToArray();
            return known.Length == 0 ? double.NaN : known.Sum();
        }).ToArray();
        var sol = Family("SOL");
        var terra = Family("TERRA");
        var luna = Family("LUNA");
        var astra = Family("ASTRA");
        var finiteValues = values.SelectMany(pair => pair.Value).Concat(sol).Concat(terra).Concat(luna).Concat(astra)
            .Where(double.IsFinite).ToArray();
        var latest = bundle.Samples.LastOrDefault();
        var cost = latest is not null && latest.Models.Count > 0 && latest.Models.All(model => model.Cost is not null)
            ? latest.Models.Sum(model => model.Cost!.TotalDollars) : (double?)null;
        var scene = new GraphScene(scope.StartAt, scope.EndAt - 1, metric,
            rows.Select(row => (double)row.At).ToArray(), remaining,
            sol, terra, luna, astra,
            values, tokens, reliability, reliability, reliability, published, vector,
            new bool[rows.Length], remainingObserved, remaining.ToArray(), new bool[rows.Length], origins,
            [], [], new Dictionary<string, IReadOnlySet<long>>(), new HashSet<long>(), new HashSet<long>(), [],
            finiteValues.Length == 0 ? 1 : Math.Max(1, finiteValues.Max()),
            periodCost: new(saved ? null : cost, !saved && scope.Coverage == ApiRecordedCoverage.Complete))
        {
            IsRecordedScope = true,
            IsSavedObservation = saved,
            RecordedBundle = bundle,
            RecordedDisplayTimeZone = zone,
            RecordedPeriodTokens = saved ? null : latest is not null
                ? latest.Models.Sum(model => (decimal)model.TotalTokens)
                : scope.Coverage == ApiRecordedCoverage.Complete ? 0m : null,
        };
        if (daily && !saved)
        {
            scene.IsDaily = true;
            scene.DailyUsage = RecordedDays(bundle, zone);
        }
        return scene;
    }

    internal static GraphScene LegacySaved(GraphScene original, TimeZoneInfo zone)
    {
        static IEnumerable<ApiHistorySample> Samples(GraphScene scene) => scene.RawSamples.Count > 0
            ? scene.RawSamples : scene.PeriodScenes.SelectMany(Samples);
        var samples = Samples(original).Where(sample => !sample.IsSyntheticTail &&
            sample.Timestamp >= original.PeriodStartAt && sample.Timestamp <= original.PeriodEndAt).ToArray();
        var saved = samples.Select((sample, index) => new ApiSavedObservation(
            "legacy-saved-" + index.ToString(System.Globalization.CultureInfo.InvariantCulture), sample.Timestamp,
            sample.ResetAt, sample.ModelSamples is null ? "legacy-fixed" : "model-sidecar",
            sample.Models.Select(model => new ApiSavedModelObservation(model.Name, model.TotalTokens,
                model.InputTokens, model.CachedInputTokens, model.CacheWriteInputTokens, model.OutputTokens, model.Dollars)).ToArray(),
            ["period-identity-unknown"])).ToArray();
        var start = Math.Max(1, original.PeriodStartAt);
        var end = Math.Max(start + 1, original.PeriodEndAt + 1);
        var scope = new ApiRecordedUsageScope(start, end, ApiRecordedCoverage.Unknown);
        // The old route has saved quota points, without durable receipt sequence authority.
        var bundle = new ApiRecordedGraphBundle(null, default, [], null, scope,
            [new(start, end, ApiRecordedCoverage.Unknown)], [], [], saved);
        return CreateRecorded(bundle, original.Metric, true, false, zone, legacySavedQuota: samples);
    }

    private static IReadOnlyList<GraphDailyUsageValue> RecordedDays(ApiRecordedGraphBundle bundle, TimeZoneInfo zone)
    {
        var increments = new List<(long At, string Name, ulong Tokens, double? Dollars)>();
        var prior = new Dictionary<string, ApiRecordedModelUsage>(StringComparer.Ordinal);
        foreach (var sample in bundle.Samples)
        {
            foreach (var model in sample.Models)
            {
                prior.TryGetValue(model.Name, out var before);
                var tokens = checked(model.TotalTokens - (before?.TotalTokens ?? 0));
                double? dollars = model.Cost is null || before is not null && before.Cost is null ? null :
                    model.Cost.TotalDollars - (before?.Cost?.TotalDollars ?? 0);
                increments.Add((sample.Timestamp, model.Name, tokens, dollars));
                prior[model.Name] = model;
            }
        }
        var result = new List<GraphDailyUsageValue>();
        var scope = bundle.Scope ?? throw new ArgumentException("Recording days require an explicit scope.", nameof(bundle));
        foreach (var day in GraphTimeWindow.LocalDays(scope.StartAt, scope.EndAt, zone))
        {
            var date = GraphTimeWindow.LocalDate(day.StartAt, zone);
            var wholeStart = GraphTimeWindow.LocalMidnight(date, zone);
            var wholeEnd = GraphTimeWindow.LocalMidnight(date.AddDays(1), zone);
            var intervals = bundle.CoverageIntervals.Where(interval => interval.StartAt < wholeEnd && interval.EndAt > wholeStart).ToArray();
            var complete = wholeStart >= scope.StartAt && wholeEnd <= scope.EndAt &&
                intervals.Length > 0 && intervals.All(interval => interval.Coverage == ApiRecordedCoverage.Complete);
            var coverage = complete ? ApiRecordedCoverage.Complete : intervals.Any(i => i.Coverage == ApiRecordedCoverage.Partial)
                ? ApiRecordedCoverage.Partial : ApiRecordedCoverage.Unknown;
            var observed = increments.Where(row => row.At >= day.StartAt && row.At < day.EndAt && row.Tokens > 0).ToArray();
            foreach (var model in observed.GroupBy(row => row.Name, StringComparer.Ordinal))
            {
                var rows = model.ToArray();
                // Separate model rows retain UInt64 arithmetic; calendar totals use decimal.
                ulong tokens = 0;
                foreach (var row in rows) tokens = checked(tokens + row.Tokens);
                result.Add(new(day.StartAt, day.EndAt, model.Key, tokens,
                    rows.All(row => row.Dollars is not null) ? rows.Sum(row => row.Dollars!.Value) : null,
                    !complete)
                { Coverage = coverage });
            }
            if (observed.Length == 0)
                result.Add(new(day.StartAt, day.EndAt, "", complete ? 0UL : null, complete ? 0d : null, !complete)
                { Coverage = coverage });
        }
        return result.AsReadOnly();
    }
}
