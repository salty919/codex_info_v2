// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.Graphing;

public sealed record GraphCalendarDay(
    string DayText, string ValueText, string StatusText, string AccessibleText,
    bool IsInMonth, bool IsToday, bool IsFuture);

/// <summary>Calendar layout over the existing recorded daily attribution.</summary>
internal static class GraphCalendarProjection
{
    internal static IReadOnlyList<GraphCalendarDay> Create(
        GraphScene scene, TimeZoneInfo zone, CultureInfo culture, long now)
    {
        if (!scene.IsDaily) return Array.Empty<GraphCalendarDay>();
        var first = GraphTimeWindow.LocalDate(scene.PeriodStartAt, zone);
        first = new DateTime(first.Year, first.Month, 1);
        var today = GraphTimeWindow.LocalDate(now, zone);
        var offset = ((int)first.DayOfWeek - (int)culture.DateTimeFormat.FirstDayOfWeek + 7) % 7;
        var count = DateTime.DaysInMonth(first.Year, first.Month);
        var cells = new List<GraphCalendarDay>();
        var rows = scene.DailyUsage.GroupBy(day => GraphTimeWindow.LocalDate(day.StartAt, zone))
            .ToDictionary(group => group.Key, group => group.ToArray());
        for (var index = 0; index < ((offset + count + 6) / 7) * 7; index++)
        {
            var number = index - offset + 1;
            if (number < 1 || number > count)
            {
                cells.Add(new("", "", "", "", false, false, false));
                continue;
            }
            var date = first.AddDays(number - 1);
            var future = date > today;
            var value = "";
            string? exactTokens = null;
            var status = "";
            if (!future)
            {
                rows.TryGetValue(date, out var day);
                var contributions = day ?? [];
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
            }
            cells.Add(new(number.ToString(culture), value, status,
                $"{date:yyyy/MM/dd} {exactTokens ?? value} {status}".Trim(), true, date == today, future));
        }
        return cells;
    }
}
