// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.ViewModels;

public sealed partial class MainWindowViewModel
{
    public string QuotaHeading => Texts.RemainingQuota;
    public string QuotaResetHeading => Texts.ResetTime;
    public string ModelUsageHeading => Texts.ModelLabel;

    private string DescribeRecordedQuality(string detail) => detailsSnapshot is { ApiVersion: "v5" } recorded
        ? $"{detail} · {RecordedUsageText.PeriodUnknown} · {RecordedUsageText.Coverage(recorded.UsageScope?.Coverage ?? ApiRecordedCoverage.Unknown)}"
        : detail;

    private static string RecordedCostText(IReadOnlyList<ApiRecordedModelUsage> source)
    {
        if (source.Count == 0 || source.Any(model => model.Cost is null)) return RecordedUsageText.PriceUnknown;
        var sum = source.Sum(model => model.Cost!.TotalDollars);
        return double.IsFinite(sum) ? "$" + sum.ToString("N2", CultureInfo.CurrentCulture) : RecordedUsageText.PriceUnknown;
    }

    private void ReplaceRecordedModels(IReadOnlyList<ApiRecordedModelUsage> source)
    {
        static ulong? SumKnown(IEnumerable<ulong?> values)
        {
            ulong sum = 0;
            foreach (var value in values)
            {
                if (value is null || ulong.MaxValue - sum < value.Value) return null;
                sum += value.Value;
            }
            return sum;
        }
        var projected = new List<ApiRecordedModelUsage>();
        foreach (var group in source.GroupBy(model => MainModelIdentityFor(model.Name), StringComparer.Ordinal))
        {
            var rows = group.ToArray();
            if (rows.Length == 1)
            {
                projected.Add(rows[0] with { Name = group.Key });
                continue;
            }
            var total = SumKnown(rows.Select(model => (ulong?)model.TotalTokens));
            if (total is null) { projected.AddRange(rows); continue; }
            ApiRecordedCost? cost = null;
            if (rows.All(model => model.Cost is not null))
            {
                var costs = rows.Select(model => model.Cost!).ToArray();
                var all = costs.Sum(c => c.TotalDollars);
                if (double.IsFinite(all)) cost = new("display-only", costs.Sum(c => c.OrdinaryInputDollars),
                    costs.Sum(c => c.CachedInputDollars), costs.Sum(c => c.CacheWriteInputDollars),
                    costs.Sum(c => c.OutputDollars), all);
            }
            projected.Add(new(group.Key, total.Value, SumKnown(rows.Select(model => model.InputTokens)),
                SumKnown(rows.Select(model => model.CachedInputTokens)), SumKnown(rows.Select(model => model.CacheWriteInputTokens)),
                SumKnown(rows.Select(model => model.OutputTokens)), cost));
        }
        var usages = projected.OrderByDescending(model => model.Cost?.TotalDollars)
            .ThenBy(model => model.Name, StringComparer.Ordinal).ToArray();
        if (usages.Select(model => model.Name).SequenceEqual(models.Select(model => model.Name), StringComparer.Ordinal))
        {
            for (var index = 0; index < usages.Length; index++) models[index].Update(usages[index]);
            return;
        }
        var previous = models.ToArray();
        models.ReplaceAll(usages.Select(model => new ModelUsageViewModel(model, MainModelDisplayName(model.Name),
            model.Name == OtherModelIdentity)).ToArray());
        foreach (var model in previous) model.Dispose();
    }
}
