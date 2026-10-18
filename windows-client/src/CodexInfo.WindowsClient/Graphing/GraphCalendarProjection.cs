// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.Graphing;

public sealed record GraphCalendarDay(
    string DayText, string ValueText, string StatusText, string AccessibleText,
    bool IsInMonth, bool IsToday, bool IsFuture)
{
    public bool IsQuiet { get; init; }
    public bool IsInScope { get; init; }
    public bool IsVisible => IsInMonth || IsInScope;
}

/// <summary>Calendar layout over the existing recorded daily attribution.</summary>
internal static class GraphCalendarProjection
{
    internal static IReadOnlyList<GraphCalendarDay> Create(
        GraphScene scene, TimeZoneInfo zone, CultureInfo culture, long now)
    {
        if (!scene.IsDaily) return Array.Empty<GraphCalendarDay>();
        var first = scene.IsRecordedScope
            ? DateTimeOffset.FromUnixTimeSeconds(scene.PeriodStartAt).UtcDateTime.Date
            : GraphTimeWindow.LocalDate(scene.PeriodStartAt, zone);
        first = new DateTime(first.Year, first.Month, 1);
        var today = GraphTimeWindow.LocalDate(now, zone);
        var offset = ((int)first.DayOfWeek - (int)culture.DateTimeFormat.FirstDayOfWeek + 7) % 7;
        var count = DateTime.DaysInMonth(first.Year, first.Month);
        var cells = new List<GraphCalendarDay>();
        var rows = scene.DailyUsage.GroupBy(day => GraphTimeWindow.LocalDate(day.StartAt, zone))
            .ToDictionary(group => group.Key, group => group.ToArray());
        var cellCount = ((offset + count + 6) / 7) * 7;
        if (scene.IsRecordedScope && rows.Count > 0)
            cellCount = Math.Max(cellCount, (((int)(rows.Keys.Max() - first).TotalDays + offset + 7) / 7) * 7);
        for (var index = 0; index < cellCount; index++)
        {
            var number = index - offset + 1;
            var inMonth = number >= 1 && number <= count;
            DateTime date;
            try { date = first.AddDays(number - 1); }
            catch (ArgumentOutOfRangeException)
            {
                cells.Add(new("", "", "", "", false, false, false));
                continue;
            }
            rows.TryGetValue(date, out var day);
            var contributions = day ?? [];
            var inScope = scene.IsRecordedScope && contributions.Length > 0;
            if (!inMonth && !inScope)
            {
                cells.Add(new("", "", "", "", false, false, false));
                continue;
            }
            var future = date > today;
            var value = "";
            string? exactTokens = null;
            var status = "";
            var hasRecordedUse = contributions.Any(item => item.Tokens > 0 || item.Dollars > 0);
            if (!future)
            {
                if (scene.Metric == GraphMetric.Tokens)
                {
                    var known = contributions.Where(item => item.Tokens is not null).ToArray();
                    var tokens = known.Sum(item => (decimal)item.Tokens!.Value);
                    value = known.Length == 0 ? "—" : GraphUsageSummary.FormatTokens(tokens, culture);
                    if (known.Length > 0)
                        exactTokens = tokens.ToString("N0", culture) + " " + LocalizationService.Current.GraphTokenMetric;
                }
                else
                {
                    var known = contributions.Where(item => item.Dollars is not null).ToArray();
                    var unknownPrice = contributions.Any(item => item.Tokens is not null && item.Dollars is null);
                    value = known.Length == 0 || unknownPrice ? "—" : "$" +
                        known.Sum(item => item.Dollars!.Value).ToString("N2", culture);
                }
                status = value == "—" ? LocalizationService.Current.GraphUnmeasured :
                    contributions.Any(item => item.IsPartial) ? LocalizationService.Current.GraphRecordedPartial : "";
                if (scene.IsRecordedScope)
                    status = hasRecordedUse ? contributions.Any(item => item.IsPartial) ? RecordedUsageText.Partial : "" :
                        contributions.Length > 0 && contributions.All(item => item.Coverage == Core.ApiRecordedCoverage.Complete)
                            ? RecordedUsageText.RecordedZero : RecordedUsageText.NoRecord;
            }
            cells.Add(new(inMonth ? number.ToString(culture) : date.ToString("M/d", culture), value, status,
                $"{date:yyyy/MM/dd} {exactTokens ?? value} {status}".Trim(), inMonth, date == today, future)
            {
                IsInScope = inScope,
                IsQuiet = scene.IsRecordedScope
                    ? !hasRecordedUse && contributions.Length > 0 && contributions.All(item => item.Coverage == Core.ApiRecordedCoverage.Complete)
                    : !hasRecordedUse,
            });
        }
        return cells;
    }
}
