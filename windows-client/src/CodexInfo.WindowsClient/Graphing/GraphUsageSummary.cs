// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.Graphing;

internal static class GraphUsageSummary
{
    internal static string FormatTokens(decimal tokens, CultureInfo culture) =>
        tokens < 1_000_000m ? tokens.ToString("N0", culture)
            : (tokens / 1_000_000m).ToString("#,0.######", culture) + " M";

    internal static string PeriodText(GraphScene scene)
    {
        if (scene.Metric == GraphMetric.Dollars)
            return LocalizationService.Current.FormatGraphPeriodCost(scene.PeriodCost.RecordedDollars);

        return scene.RecordedPeriodTokens is { } tokens
            ? FormatTokens(tokens, CultureInfo.InvariantCulture) : "—";
    }
}
