// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.Graphing;

internal static class GraphUsageSummary
{
    internal static string FormatTokens(decimal tokens, CultureInfo culture) =>
        tokens < 1_000_000m ? tokens.ToString("N0", culture)
            : decimal.Round(tokens / 1_000_000m, 0, MidpointRounding.AwayFromZero)
                .ToString("N0", culture) + " M";

    internal static string[] TokenAxisLabels(double maximum, int count, CultureInfo culture)
    {
        var labels = Enumerable.Range(0, count)
            .Select(index => FormatTokens((decimal)(maximum * index / (count - 1)), culture))
            .ToArray();
        // Keep the upper label when rounding would repeat it at different heights.
        for (var index = 0; index < labels.Length - 1; index++)
        {
            if (labels[index] == labels[index + 1])
                labels[index] = string.Empty;
        }
        return labels;
    }

    internal static string PeriodText(GraphScene scene)
    {
        if (scene.Metric == GraphMetric.Dollars)
            return LocalizationService.Current.FormatGraphPeriodCost(scene.PeriodCost.RecordedDollars);

        return scene.RecordedPeriodTokens is { } tokens
            ? FormatTokens(tokens, CultureInfo.InvariantCulture) : "—";
    }
}
