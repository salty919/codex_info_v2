// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.ViewModels;

namespace CodexInfo.WindowsClient.Graphing;

/// <summary>Attributable usage for one local date, never a cumulative snapshot.</summary>
internal readonly record struct GraphDailyUsageValue(
    long StartAt,
    long EndAt,
    string ModelName,
    ulong? Tokens,
    double? Dollars,
    bool IsPartial);

/// <summary>Read-only daily attribution from accepted canonical-period raw observations.</summary>
internal static class GraphDailyUsageProjection
{
    internal static IReadOnlyList<GraphDailyUsageValue> Create(
        IReadOnlyList<ApiHistoryPeriod> periods,
        long startAt,
        long endAt,
        TimeZoneInfo displayTimeZone,
        IReadOnlyList<ApiHistoryGap>? gaps,
        IReadOnlyList<GraphAccountOwnershipInterval>? ownershipIntervals)
    {
        var contributions = new List<(string Family, Dictionary<DateTime, DayUsage> Days)>();
        foreach (var period in periods)
        {
            var samples = period.Samples
                .Where(sample => !sample.IsSyntheticTail && sample.ResetAt == period.ResetAt)
                .OrderBy(sample => sample.Timestamp)
                .ToArray();
            var names = samples.SelectMany(Models).Select(model => model.Name)
                .Distinct(StringComparer.Ordinal).ToArray();
            foreach (var name in names)
            {
                var days = ProjectModel(period, samples, name, endAt, displayTimeZone, gaps, ownershipIntervals);
                contributions.Add((ModelUsageViewModel.DisplayFamilyName(name) ?? name, days));
            }
        }

        var result = new List<GraphDailyUsageValue>();
        foreach (var day in GraphTimeWindow.LocalDays(startAt, endAt, displayTimeZone))
        {
            var date = GraphTimeWindow.LocalDate(day.StartAt, displayTimeZone);
            foreach (var family in contributions.GroupBy(item => item.Family, StringComparer.Ordinal))
            {
                ulong? tokens = null;
                double? dollars = null;
                var priceUnavailable = false;
                foreach (var member in family)
                {
                    if (!member.Days.TryGetValue(date, out var usage)) continue;
                    if (usage.Tokens is { } recordedTokens)
                    {
                        tokens = checked((tokens ?? 0) + recordedTokens);
                    }
                    if (usage.Dollars is { } recordedDollars)
                    {
                        dollars = (dollars ?? 0) + recordedDollars;
                    }
                    priceUnavailable |= usage.PriceUnavailable;
                }
                if (priceUnavailable || dollars is { } total && !double.IsFinite(total)) dollars = null;
                result.Add(new GraphDailyUsageValue(day.StartAt, day.EndAt, family.Key,
                    tokens, dollars, tokens is not null || dollars is not null));
            }
        }
        return result.ToArray();
    }

    private static Dictionary<DateTime, DayUsage> ProjectModel(
        ApiHistoryPeriod period,
        IReadOnlyList<ApiHistorySample> samples,
        string name,
        long endAt,
        TimeZoneInfo displayTimeZone,
        IReadOnlyList<ApiHistoryGap>? gaps,
        IReadOnlyList<GraphAccountOwnershipInterval>? ownershipIntervals)
    {
        var days = new Dictionary<DateTime, DayUsage>();
        ulong? priorTokens = null;
        double? priorDollars = null;
        ulong? lastKnownTokens = null;
        double? lastKnownDollars = null;
        long? priorTimestamp = null;
        var startDate = GraphTimeWindow.LocalDate(period.StartAt, displayTimeZone);
        foreach (var sample in samples)
        {
            var models = Models(sample).Where(model => model.Name == name).ToArray();
            var authoritative = sample.ModelSource == ApiHistorySample.ConfirmedModelSource && sample.ModelsComplete &&
                models.Length == 1 && OwnsSpan(sample.Timestamp, sample.Timestamp, ownershipIntervals);
            var model = authoritative ? models[0] : null;
            var tokens = model?.TotalTokens;
            var dollars = model?.Dollars is { } amount && double.IsFinite(amount) && amount >= 0
                ? amount : (double?)null;
            var date = GraphTimeWindow.LocalDate(sample.Timestamp, displayTimeZone);
            if (!days.TryGetValue(date, out var day))
            {
                day = new DayUsage();
                days.Add(date, day);
            }
            if (model is not null && dollars is null) day.PriceUnavailable = true;

            var tokenCorrection = tokens is { } currentTokens && lastKnownTokens is { } knownTokens && currentTokens < knownTokens;
            var dollarCorrection = dollars is { } currentDollars && lastKnownDollars is { } knownDollars && currentDollars < knownDollars;
            if (tokenCorrection)
            {
                foreach (var affected in days.Values) affected.Tokens = null;
            }
            if (dollarCorrection)
            {
                foreach (var affected in days.Values) affected.Dollars = null;
            }

            var sameDay = priorTimestamp is { } previous && GraphTimeWindow.LocalDate(previous, displayTimeZone) == date &&
                OwnsSpan(previous, sample.Timestamp, ownershipIntervals) &&
                !CrossesGap(previous, sample.Timestamp, period.ResetAt, gaps);
            var initialDay = date == startDate && OwnsSpan(period.StartAt, sample.Timestamp, ownershipIntervals);
            if (sample.Timestamp <= endAt && tokens is { } tokenValue && !tokenCorrection)
            {
                if (sameDay && priorTokens is { } tokenBaseline)
                {
                    day.Tokens = checked((day.Tokens ?? 0) + (tokenValue - tokenBaseline));
                }
                else if (lastKnownTokens is null && initialDay)
                {
                    day.Tokens = tokenValue;
                }
            }
            if (sample.Timestamp <= endAt && dollars is { } dollarValue && !dollarCorrection)
            {
                if (sameDay && priorDollars is { } dollarBaseline)
                {
                    day.Dollars = (day.Dollars ?? 0) + (dollarValue - dollarBaseline);
                }
                else if (lastKnownDollars is null && initialDay)
                {
                    day.Dollars = dollarValue;
                }
            }

            priorTokens = tokens;
            priorDollars = dollars;
            priorTimestamp = sample.Timestamp;
            if (tokens is not null) lastKnownTokens = tokens;
            if (dollars is not null) lastKnownDollars = dollars;
        }
        return days;
    }

    private static IEnumerable<ApiHistoryModelSample> Models(ApiHistorySample sample) =>
        sample.ModelSamples ?? (sample.ModelSource is ApiHistorySample.ConfirmedModelSource or ApiHistorySample.LegacyUnknownModelSource
            ? sample.Models : Array.Empty<ApiHistoryModelSample>());

    private static bool OwnsSpan(long startAt, long endAt, IReadOnlyList<GraphAccountOwnershipInterval>? intervals) =>
        intervals is null || intervals.Any(interval =>
            (interval.StartAt is null || startAt >= interval.StartAt) &&
            (interval.EndAt is null || endAt < interval.EndAt));

    private static bool CrossesGap(long startAt, long endAt, long resetAt, IReadOnlyList<ApiHistoryGap>? gaps) =>
        gaps is not null && gaps.Any(gap => gap.ResetAt == resetAt && gap.StartAt < endAt && gap.EndAt > startAt);

    private sealed class DayUsage
    {
        internal ulong? Tokens;
        internal double? Dollars;
        internal bool PriceUnavailable;
    }
}
