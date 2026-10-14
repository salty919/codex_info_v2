// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections;
using System.Globalization;
using System.Reflection;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphMonthCalendarTests
{
    [Theory]
    [InlineData(GraphMetric.Tokens)]
    [InlineData(GraphMetric.Dollars)]
    public void DaysWithoutRecordedUseAreQuietWithoutCallingMissingDataUnused(GraphMetric metric)
    {
        var start = DateTimeOffset.Parse("2026-10-01T00:00:00Z").ToUnixTimeSeconds();
        var reset = start + 604_800;
        var period = GraphDailyUsageProjectionTests.Period("quiet", start, reset,
            GraphDailyUsageProjectionTests.Sample(start + 60, reset,
                GraphDailyUsageProjectionTests.Model("gpt-6-sol", 100, 1)),
            GraphDailyUsageProjectionTests.Sample(start + 86_400, reset,
                GraphDailyUsageProjectionTests.Model("gpt-6-sol", 100, 1)),
            GraphDailyUsageProjectionTests.Sample(start + 86_460, reset,
                GraphDailyUsageProjectionTests.Model("gpt-6-sol", 100, 1)));
        var scene = GraphDailyUsageProjectionTests.DailyScene(start, start + 4 * 86_400, metric, period);
        var days = Days(scene, start + 3 * 86_400);
        var quiet = days[4].GetType().GetProperty("IsQuiet");
        Assert.NotNull(quiet);
        Assert.Equal(false, quiet.GetValue(days[4]));
        Assert.Equal(true, quiet.GetValue(days[5]));
        Assert.Equal(true, quiet.GetValue(days[6]));
        Assert.NotEqual("—", Property<string>(days[5], "ValueText"));
        Assert.Equal("—", Property<string>(days[6], "ValueText"));
        Assert.NotEqual(Property<string>(days[5], "StatusText"), Property<string>(days[6], "StatusText"));
    }

    [Theory]
    [InlineData(GraphMetric.Tokens, "3 M")]
    [InlineData(GraphMetric.Dollars, "$7.50")]
    public void MonthHasSevenColumnCalendarAndDailyAllModelTotals(GraphMetric metric, string expected)
    {
        var start = DateTimeOffset.Parse("2026-10-01T00:00:00Z").ToUnixTimeSeconds();
        var reset = start + 604_800;
        var period = GraphDailyUsageProjectionTests.Period("month", start, reset,
            GraphDailyUsageProjectionTests.Sample(start + 60, reset,
                GraphDailyUsageProjectionTests.Model("gpt-6-sol", 1_000_000, 2.5),
                GraphDailyUsageProjectionTests.Model("unlisted-model", 2_000_000, 5)));
        var scene = GraphDailyUsageProjectionTests.DailyScene(start, start + 86_400, metric, period);
        var days = Days(scene, start + 43_200);
        Assert.Equal(35, days.Length);
        Assert.Equal("1", Property<string>(days[4], "DayText"));
        Assert.Equal(expected, Property<string>(days[4], "ValueText"));
        Assert.True(Property<bool>(days[4], "IsToday"));
        Assert.True(Property<bool>(days[5], "IsFuture"));
        Assert.Equal("", Property<string>(days[5], "ValueText"));
        Assert.Equal("31", Property<string>(days[34], "DayText"));
        Assert.Equal("", Property<string>(days[0], "DayText"));
    }

    [Fact]
    public void MissingPriceIsNotAZeroOrKnownModelOnlyTotal()
    {
        var start = DateTimeOffset.Parse("2026-10-01T00:00:00Z").ToUnixTimeSeconds();
        var reset = start + 604_800;
        var period = GraphDailyUsageProjectionTests.Period("month", start, reset,
            GraphDailyUsageProjectionTests.Sample(start + 60, reset,
                GraphDailyUsageProjectionTests.Model("gpt-6-sol", 1_000_000, 2.5),
                GraphDailyUsageProjectionTests.Model("gpt-6-astra", 2_000_000, null)));
        var scene = GraphDailyUsageProjectionTests.DailyScene(start, start + 3 * 86_400, GraphMetric.Dollars, period);
        var days = Days(scene, start + 2 * 86_400);
        Assert.Equal("—", Property<string>(days[4], "ValueText"));
        Assert.Equal("—", Property<string>(days[5], "ValueText"));
        Assert.False(Property<bool>(days[5], "IsFuture"));
    }

    [Theory]
    [InlineData(999999, "999,999")]
    [InlineData(1000000, "1 M")]
    [InlineData(1250000, "1 M")]
    [InlineData(1000000000, "1,000 M")]
    public void TokenUnitsUseMillionsWithoutChangingSmallCounts(long tokens, string expected)
    {
        var type = typeof(GraphScene).Assembly.GetType("CodexInfo.WindowsClient.Graphing.GraphUsageSummary");
        Assert.NotNull(type);
        var format = type.GetMethod("FormatTokens", BindingFlags.Static | BindingFlags.NonPublic);
        Assert.NotNull(format);
        Assert.Equal(expected, format.Invoke(null, [(decimal)tokens, CultureInfo.InvariantCulture]));
    }

    [Theory]
    [InlineData(89_543_950, "90 M", "89,543,950")]
    [InlineData(303_150_800, "303 M", "303,150,800")]
    [InlineData(1_500_000, "2 M", "1,500,000")]
    [InlineData(1_000_000, "1 M", "1,000,000")]
    [InlineData(999_999, "999,999", "999,999")]
    [InlineData(0, "0", "0")]
    public void CalendarPrioritizesDailyComparisonAndRetainsExactCount(
        ulong tokens, string expected, string exact)
    {
        var start = DateTimeOffset.Parse("2026-10-01T00:00:00Z").ToUnixTimeSeconds();
        var reset = start + 604_800;
        var period = GraphDailyUsageProjectionTests.Period("month", start, reset,
            GraphDailyUsageProjectionTests.Sample(start + 60, reset,
                GraphDailyUsageProjectionTests.Model("gpt-6-sol", tokens, 1)));
        var scene = GraphDailyUsageProjectionTests.DailyScene(
            start, start + 86_400, GraphMetric.Tokens, period);
        var day = Days(scene, start + 43_200)[4];
        Assert.Equal(expected, Property<string>(day, "ValueText"));
        Assert.Contains(exact, Property<string>(day, "AccessibleText"));
        Assert.Equal(tokens, scene.DailyUsage.Single().Tokens);
    }

    private static object[] Days(GraphScene scene, long now)
    {
        var type = typeof(GraphScene).Assembly.GetType("CodexInfo.WindowsClient.Graphing.GraphCalendarProjection");
        Assert.NotNull(type);
        var create = type.GetMethod("Create", BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic);
        Assert.NotNull(create);
        var result = Assert.IsAssignableFrom<IEnumerable>(create.Invoke(null,
            [scene, TimeZoneInfo.Utc, CultureInfo.InvariantCulture, now]));
        return result.Cast<object>().ToArray();
    }

    private static T Property<T>(object value, string name) =>
        (T)(value.GetType().GetProperty(name)!.GetValue(value)!);
}
