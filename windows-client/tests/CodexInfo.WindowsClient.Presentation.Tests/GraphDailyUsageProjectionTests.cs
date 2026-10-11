// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections;
using System.Reflection;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphDailyUsageProjectionTests
{
    private static readonly long Day = Unix("2026-10-01T00:00:00Z");
    private const long SecondsPerDay = 86_400;

    [Fact]
    public void DailyBarsUseStandaloneDeltasWithinEachLocalDate()
    {
        var reset = Day + 7 * SecondsPerDay;
        var period = Period("p", Day, reset,
            Sample(Day + 10 * 3_600, reset, Model("gpt-6-sol", 100, 1)),
            Sample(Day + 11 * 3_600, reset, Model("gpt-6-sol", 140, 1.4)),
            Sample(Day + SecondsPerDay, reset, Model("gpt-6-sol", 180, 1.8)),
            Sample(Day + SecondsPerDay + 3_600, reset, Model("gpt-6-sol", 200, 2)));
        var scene = DailyScene(Day, Day + 3 * SecondsPerDay, GraphMetric.Tokens, period);

        Assert.Equal(140UL, Value<ulong?>(Row(scene, Day, "SOL"), "Tokens"));
        Assert.Equal(20UL, Value<ulong?>(Row(scene, Day + SecondsPerDay, "SOL"), "Tokens"));
        Assert.Equal(1.4, Value<double?>(Row(scene, Day, "SOL"), "Dollars")!.Value, precision: 10);
        Assert.Equal(0.2, Value<double?>(Row(scene, Day + SecondsPerDay, "SOL"), "Dollars")!.Value, precision: 10);
        Assert.True(Value<bool>(Row(scene, Day + SecondsPerDay, "SOL"), "IsPartial"));
        Assert.Null(Value<ulong?>(Row(scene, Day + 2 * SecondsPerDay, "SOL"), "Tokens"));

        var tokyoStart = Unix("2026-09-30T15:00:00Z");
        var tokyoPeriod = Period("tokyo", tokyoStart, reset,
            Sample(Unix("2026-10-01T14:00:00Z"), reset, Model("gpt-6-sol", 100, 1)),
            Sample(Unix("2026-10-01T15:00:00Z"), reset, Model("gpt-6-sol", 180, 1.8)),
            Sample(Unix("2026-10-01T16:00:00Z"), reset, Model("gpt-6-sol", 200, 2)));
        var tokyoScene = DailySceneInZone(tokyoStart, tokyoStart + 2 * SecondsPerDay,
            GraphMetric.Tokens, TimeZoneInfo.FindSystemTimeZoneById("Asia/Tokyo"), tokyoPeriod);
        Assert.Equal(100UL, Value<ulong?>(Row(tokyoScene, tokyoStart, "SOL"), "Tokens"));
        Assert.Equal(20UL, Value<ulong?>(Row(tokyoScene, tokyoStart + SecondsPerDay, "SOL"), "Tokens"));
    }

    [Fact]
    public void DailyBarsKeepResetAndModelGroupsSeparateAndExcludeLateInitialTotals()
    {
        var reset1 = Day + 12 * 3_600;
        var reset2 = Day + 2 * SecondsPerDay;
        var first = Period("first", Day, reset1,
            Sample(Day + 10 * 3_600, reset1, Model("gpt-6-sol", 100, 10), Model("gpt-6-luna", 40, 4)),
            Sample(Day + 11 * 3_600, reset1, Model("gpt-6-sol", 140, 14), Model("gpt-6-luna", 50, 5)));
        var second = Period("second", reset1, reset2,
            Sample(Day + 13 * 3_600, reset2, Model("gpt-6.1-sol", 20, 2), Model("gpt-6-luna", 5, 0.5)),
            Sample(Day + 14 * 3_600, reset2, Model("gpt-6.1-sol", 30, 3), Model("gpt-6-luna", 5, 0.5)));
        var scene = DailyScene(Day, Day + SecondsPerDay, GraphMetric.Tokens, first, second);
        Assert.Equal(170UL, Value<ulong?>(Row(scene, Day, "SOL"), "Tokens"));
        Assert.Equal(55UL, Value<ulong?>(Row(scene, Day, "LUNA"), "Tokens"));
        Assert.Equal(17d, Value<double?>(Row(scene, Day, "SOL"), "Dollars"));

        var late = Period("late", Day - SecondsPerDay, reset2,
            Sample(Day + 10 * 3_600, reset2, Model("gpt-6-sol", 1_000, 100)),
            Sample(Day + 11 * 3_600, reset2, Model("gpt-6-sol", 1_010, 101)));
        var lateScene = DailyScene(Day, Day + SecondsPerDay, GraphMetric.Tokens, late);
        Assert.Equal(10UL, Value<ulong?>(Row(lateScene, Day, "SOL"), "Tokens"));
        Assert.Equal(1d, Value<double?>(Row(lateScene, Day, "SOL"), "Dollars"));
    }

    [Fact]
    public void DailyBarsKeepMissingAndUnknownPriceValuesUnmeasured()
    {
        var reset = Day + 3 * SecondsPerDay;
        var known = Period("known", Day - SecondsPerDay, reset,
            Sample(Day + 10 * 3_600, reset, Model("gpt-6-sol", 100, null)),
            Sample(Day + 11 * 3_600, reset, Model("gpt-6-sol", 120, null)),
            new ApiHistorySample(Day + SecondsPerDay, reset, null, null, null, null, null, null, null,
                ApiHistorySample.UnavailableModelSource)
            { ModelsComplete = false, ModelSamples = null });
        var scene = DailyScene(Day, Day + 2 * SecondsPerDay, GraphMetric.Dollars, known);
        Assert.Equal(20UL, Value<ulong?>(Row(scene, Day, "SOL"), "Tokens"));
        Assert.Null(Value<double?>(Row(scene, Day, "SOL"), "Dollars"));
        Assert.Null(Value<ulong?>(Row(scene, Day + SecondsPerDay, "SOL"), "Tokens"));
        Assert.Null(Value<double?>(Row(scene, Day + SecondsPerDay, "SOL"), "Dollars"));

        var mixed = Period("mixed", Day - SecondsPerDay, reset,
            Sample(Day + 10 * 3_600, reset, Model("gpt-6-sol", 100, null), Model("gpt-6.1-sol", 10, 1)),
            Sample(Day + 11 * 3_600, reset, Model("gpt-6-sol", 120, null), Model("gpt-6.1-sol", 15, 1.5)));
        var mixedScene = DailyScene(Day, Day + SecondsPerDay, GraphMetric.Dollars, mixed);
        Assert.Equal(25UL, Value<ulong?>(Row(mixedScene, Day, "SOL"), "Tokens"));
        Assert.Null(Value<double?>(Row(mixedScene, Day, "SOL"), "Dollars"));

        var zero = Period("zero", Day, reset,
            Sample(Day + 3_600, reset, Model("gpt-6-sol", 0, 0)));
        var zeroScene = DailyScene(Day, Day + SecondsPerDay, GraphMetric.Tokens, zero);
        Assert.Equal(0UL, Value<ulong?>(Row(zeroScene, Day, "SOL"), "Tokens"));
        Assert.Equal(0d, Value<double?>(Row(zeroScene, Day, "SOL"), "Dollars"));
    }

    [Fact]
    public void DailyBarsDiscardCorrectionAffectedAllocationsAndUseCorrectedBaseline()
    {
        var reset = Day + 7 * SecondsPerDay;
        var period = Period("correction", Day, reset,
            Sample(Day + 10 * 3_600, reset, Model("gpt-6-sol", 100, 10)),
            Sample(Day + SecondsPerDay + 10 * 3_600, reset, Model("gpt-6-sol", 80, 8)),
            Sample(Day + SecondsPerDay + 11 * 3_600, reset, Model("gpt-6-sol", 90, 9)));
        var scene = DailyScene(Day, Day + 2 * SecondsPerDay, GraphMetric.Tokens, period);
        Assert.Null(Value<ulong?>(Row(scene, Day, "SOL"), "Tokens"));
        Assert.Null(Value<double?>(Row(scene, Day, "SOL"), "Dollars"));
        Assert.Equal(10UL, Value<ulong?>(Row(scene, Day + SecondsPerDay, "SOL"), "Tokens"));
        Assert.Equal(1d, Value<double?>(Row(scene, Day + SecondsPerDay, "SOL"), "Dollars"));
        Assert.True(Value<bool>(Row(scene, Day + SecondsPerDay, "SOL"), "IsPartial"));
    }

    internal static GraphScene DailyScene(long start, long end, GraphMetric metric, params ApiHistoryPeriod[] periods) =>
        DailySceneInZone(start, end, metric, TimeZoneInfo.Utc, periods);

    private static GraphScene DailySceneInZone(long start, long end, GraphMetric metric, TimeZoneInfo zone,
        params ApiHistoryPeriod[] periods)
    {
        var method = typeof(GraphScene).GetMethod("CreateDailyViewport", BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic);
        Assert.True(method is not null, "Calendar-month standalone daily projection has not been implemented.");
        return Assert.IsType<GraphScene>(method.Invoke(null, [start, end, metric, periods, zone, null, null, null]));
    }

    internal static ApiHistoryPeriod Period(string id, long start, long reset, params ApiHistorySample[] samples) =>
        new(id, start, reset, false, id) { ResetAt = reset, Samples = samples };

    internal static ApiHistorySample Sample(long timestamp, long reset, params ApiHistoryModelSample[] models) =>
        new(timestamp, reset, null, null, null, null, null, null, null) { ModelSamples = models, ModelsComplete = true };

    internal static ApiHistoryModelSample Model(string name, ulong? tokens, double? dollars) =>
        new(name, null, null, null, dollars) { TotalTokens = tokens };

    private static object Row(GraphScene scene, long day, string model)
    {
        var property = typeof(GraphScene).GetProperty("DailyUsage", BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
        Assert.NotNull(property);
        var rows = Assert.IsAssignableFrom<IEnumerable>(property.GetValue(scene)).Cast<object>();
        return Assert.Single(rows, row => Value<long>(row, "StartAt") == day && Value<string>(row, "ModelName") == model);
    }

    private static T Value<T>(object value, string property)
    {
        var member = value.GetType().GetProperty(property);
        Assert.NotNull(member);
        return (T)member.GetValue(value)!;
    }

    private static long Unix(string value) => DateTimeOffset.Parse(value).ToUnixTimeSeconds();
}
