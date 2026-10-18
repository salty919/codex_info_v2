// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Graphing;

internal sealed record GraphHoverSnapshot(long Timestamp, IReadOnlyList<GraphHoverRow> Rows)
{
    internal IReadOnlyList<string> Details { get; init; } = [];
}

internal readonly record struct GraphHoverRow(
    GraphSeries Series,
    ulong? TokenValue,
    double? NumericValue);

internal static class GraphHoverProjection
{
    private static readonly GraphSeries[] SeriesOrder =
    [
        GraphSeries.Remaining,
        GraphSeries.Luna,
        GraphSeries.Terra,
        GraphSeries.Sol,
        GraphSeries.Astra,
    ];

    internal static GraphHoverSnapshot? Find(
        GraphScene scene,
        double timestamp,
        IReadOnlySet<GraphSeries> visibleSeries)
    {
        ArgumentNullException.ThrowIfNull(scene);
        ArgumentNullException.ThrowIfNull(visibleSeries);
        if (scene.IsRecordedScope) return FindRecorded(scene, timestamp, visibleSeries);
        if (!double.IsFinite(timestamp) ||
            timestamp < scene.PeriodStartAt ||
            timestamp > scene.PeriodEndAt)
        {
            return null;
        }

        if (scene.IsViewport)
        {
            var period = FindOwningPeriod(scene.PeriodScenes, timestamp);
            if (period is null)
            {
                return null;
            }

            return FindInPeriod(
                period,
                timestamp,
                Math.Max(scene.PeriodStartAt, period.PeriodStartAt),
                Math.Min(scene.PeriodEndAt, period.PeriodEndAt),
                visibleSeries);
        }

        return FindInPeriod(
            scene,
            timestamp,
            scene.PeriodStartAt,
            scene.PeriodEndAt,
            visibleSeries);
    }

    private static GraphHoverSnapshot? FindRecorded(GraphScene scene, double timestamp, IReadOnlySet<GraphSeries> visibleSeries)
    {
        if (!double.IsFinite(timestamp) || timestamp < scene.PeriodStartAt || timestamp > scene.PeriodEndAt || scene.RecordedBundle is not { } bundle)
            return null;
        var culture = System.Globalization.CultureInfo.CurrentCulture;
        string Tokens(ulong? value) => value?.ToString("N0", culture) ?? Localization.RecordedUsageText.Unknown;
        bool Visible(string name) => ViewModels.ModelUsageViewModel.DisplayFamilyName(name) switch
        {
            "SOL" => visibleSeries.Contains(GraphSeries.Sol),
            "TERRA" => visibleSeries.Contains(GraphSeries.Terra),
            "LUNA" => visibleSeries.Contains(GraphSeries.Luna),
            "ASTRA" => visibleSeries.Contains(GraphSeries.Astra),
            _ => visibleSeries.Any(series => series != GraphSeries.Remaining),
        };
        var quotaIndices = visibleSeries.Contains(GraphSeries.Remaining)
            ? Enumerable.Range(0, scene.Timestamps.Count).Where(index => scene.RemainingObserved[index] &&
                double.IsFinite(scene.ObservedRemainingValues[index])).ToArray() : [];
        var modelTimes = scene.IsSavedObservation
            ? bundle.Observations.Where(row => row.Models.Any(model => Visible(model.StoredModelKey))).Select(row => row.Timestamp)
            : bundle.Samples.Where(row => row.Models.Any(model => Visible(model.Name))).Select(row => row.Timestamp);
        var times = modelTimes.Concat(quotaIndices.Select(index => (long)scene.Timestamps[index])).Distinct().ToArray();
        if (times.Length == 0) return null;
        var at = times.OrderBy(time => Math.Abs(time - timestamp)).ThenBy(time => time).First();
        var details = new List<string> { Localization.RecordedUsageText.PeriodUnknown };
        var rows = quotaIndices.Where(index => scene.Timestamps[index] == at)
            .Select(index => new GraphHoverRow(GraphSeries.Remaining, null, scene.ObservedRemainingValues[index])).ToArray();
        foreach (var observation in bundle.QuotaObservations.Where(row => row.ObservedAt == at))
            details.Add($"R: {observation.ResetAt.ToString(culture)}");
        if (scene.IsSavedObservation)
        {
            foreach (var saved in bundle.Observations.Where(row => row.Timestamp == at))
            {
                var models = saved.Models.Where(model => Visible(model.StoredModelKey)).ToArray();
                if (models.Length == 0) continue;
                details.Add("R: " + (saved.RawResetAt?.ToString(culture) ?? Localization.RecordedUsageText.Unknown));
                foreach (var model in models)
                    details.Add($"{model.StoredModelKey}: T {Tokens(model.TotalTokens)} / I {Tokens(model.InputTokens)} / C {Tokens(model.CachedInputTokens)} / W {Tokens(model.CacheWriteInputTokens)} / O {Tokens(model.OutputTokens)} / USD {model.TotalDollars?.ToString("R", culture) ?? Localization.RecordedUsageText.PriceUnknown}");
                details.AddRange(saved.UnresolvedReasons.Select(Localization.RecordedUsageText.Reason));
            }
        }
        else
        {
            foreach (var model in bundle.Samples.Where(sample => sample.Timestamp == at).SelectMany(sample => sample.Models)
                .Where(model => Visible(model.Name)))
                details.Add($"{model.Name}: T {Tokens(model.TotalTokens)} / I {Tokens(model.InputTokens)} / C {Tokens(model.CachedInputTokens)} / W {Tokens(model.CacheWriteInputTokens)} / O {Tokens(model.OutputTokens)} / USD {model.Cost?.TotalDollars.ToString("R", culture) ?? Localization.RecordedUsageText.PriceUnknown}");
        }
        return new(at, rows) { Details = details.AsReadOnly() };
    }

    private static GraphScene? FindOwningPeriod(
        IReadOnlyList<GraphScene> periods,
        double timestamp)
    {
        GraphScene? owner = null;
        foreach (var period in periods)
        {
            if (timestamp < period.PeriodStartAt || timestamp > period.PeriodEndAt)
            {
                continue;
            }
            if (owner is not null)
            {
                // Adjacent/overlapping reset periods do not have a unique owner at
                // this cursor coordinate. Hide the tooltip instead of borrowing.
                return null;
            }
            owner = period;
        }
        return owner;
    }

    private static GraphHoverSnapshot? FindInPeriod(
        GraphScene scene,
        double timestamp,
        long allowedStart,
        long allowedEnd,
        IReadOnlySet<GraphSeries> visibleSeries)
    {
        if (scene.IsViewport ||
            !scene.HasUnambiguousHoverResetAt ||
            allowedEnd < allowedStart ||
            timestamp < allowedStart ||
            timestamp > allowedEnd ||
            IsBlockedAt(scene, timestamp))
        {
            return null;
        }

        var observations = scene.HoverObservations;
        if (observations.Count == 0)
        {
            return null;
        }

        var first = LowerBound(observations, allowedStart);
        var afterLast = UpperBound(observations, allowedEnd);
        if (first >= afterLast)
        {
            return null;
        }

        var insertion = LowerBound(observations, timestamp);
        insertion = Math.Clamp(insertion, first, afterLast);
        var leftIndex = insertion > first ? insertion - 1 : -1;
        var rightIndex = insertion < afterLast ? insertion : -1;
        var left = leftIndex >= 0 && CanReach(scene, observations[leftIndex], timestamp)
            ? observations[leftIndex]
            : (GraphObservedSample?)null;
        var right = rightIndex >= 0 && CanReach(scene, observations[rightIndex], timestamp)
            ? observations[rightIndex]
            : (GraphObservedSample?)null;

        GraphObservedSample? selected = (left, right) switch
        {
            ({ } earlier, { } later) => timestamp - earlier.Timestamp <= later.Timestamp - timestamp
                ? earlier
                : later,
            ({ } earlier, null) => earlier,
            (null, { } later) => later,
            _ => null,
        };
        if (selected is not { } observation)
        {
            return null;
        }

        if (observation.SceneIndex < 0 ||
            observation.SceneIndex >= scene.Timestamps.Count ||
            scene.Timestamps[observation.SceneIndex] != observation.Timestamp ||
            scene.ModelSynthetic[observation.SceneIndex])
        {
            return null;
        }

        var rows = new List<GraphHoverRow>(SeriesOrder.Length);
        foreach (var series in SeriesOrder)
        {
            if (!visibleSeries.Contains(series))
            {
                continue;
            }
            rows.Add(BuildRow(scene, observation, series));
        }

        return new GraphHoverSnapshot(observation.Timestamp, Array.AsReadOnly(rows.ToArray()));
    }

    private static bool CanReach(GraphScene scene, GraphObservedSample observation, double timestamp)
    {
        if (observation.ResetAt != scene.HoverResetAt ||
            IsBlockedAt(scene, observation.Timestamp) ||
            scene.IsNonOwnedAt(timestamp) ||
            scene.IsNonOwnedAt(observation.Timestamp))
        {
            return false;
        }

        var start = Math.Min(timestamp, observation.Timestamp);
        var end = Math.Max(timestamp, observation.Timestamp);
        if (scene.OverlapsNonOwnedInterval(start, end))
        {
            return false;
        }

        return !scene.ConfirmedGaps.Any(gap => gap.StartAt <= end && gap.EndAt >= start);
    }

    private static bool IsBlockedAt(GraphScene scene, double timestamp) =>
        scene.IsNonOwnedAt(timestamp) ||
        scene.ConfirmedGaps.Any(gap => timestamp >= gap.StartAt && timestamp <= gap.EndAt);

    private static GraphHoverRow BuildRow(
        GraphScene scene,
        GraphObservedSample observation,
        GraphSeries series)
    {
        var index = observation.SceneIndex;
        if (series == GraphSeries.Remaining)
        {
            var remaining = index < scene.RemainingObserved.Count &&
                scene.RemainingObserved[index] &&
                index < scene.ObservedRemainingValues.Count &&
                double.IsFinite(scene.ObservedRemainingValues[index])
                ? scene.ObservedRemainingValues[index]
                : (double?)null;
            return new GraphHoverRow(GraphSeries.Remaining, null, remaining);
        }

        var name = series switch
        {
            GraphSeries.Sol => "SOL",
            GraphSeries.Terra => "TERRA",
            GraphSeries.Luna => "LUNA",
            GraphSeries.Astra => "ASTRA",
            _ => null,
        };
        if (name is null ||
            !scene.ModelSeries.TryGetValue(name, out var values) ||
            !scene.ModelLineReliability.TryGetValue(name, out var lineReliability) ||
            index >= values.Count ||
            index >= lineReliability.Count ||
            !lineReliability[index] ||
            !double.IsFinite(values[index]))
        {
            return new GraphHoverRow(series, null, null);
        }

        if (scene.Metric == GraphMetric.Tokens)
        {
            return new GraphHoverRow(series, observation.TokensFor(series), null);
        }

        return new GraphHoverRow(series, null, values[index]);
    }

    private static int LowerBound(IReadOnlyList<GraphObservedSample> observations, double timestamp)
    {
        var low = 0;
        var high = observations.Count;
        while (low < high)
        {
            var middle = low + (high - low) / 2;
            if (observations[middle].Timestamp < timestamp)
            {
                low = middle + 1;
            }
            else
            {
                high = middle;
            }
        }
        return low;
    }

    private static int LowerBound(IReadOnlyList<GraphObservedSample> observations, long timestamp)
    {
        var low = 0;
        var high = observations.Count;
        while (low < high)
        {
            var middle = low + (high - low) / 2;
            if (observations[middle].Timestamp < timestamp)
            {
                low = middle + 1;
            }
            else
            {
                high = middle;
            }
        }
        return low;
    }

    private static int UpperBound(IReadOnlyList<GraphObservedSample> observations, long timestamp)
    {
        var low = 0;
        var high = observations.Count;
        while (low < high)
        {
            var middle = low + (high - low) / 2;
            if (observations[middle].Timestamp <= timestamp)
            {
                low = middle + 1;
            }
            else
            {
                high = middle;
            }
        }
        return low;
    }
}
