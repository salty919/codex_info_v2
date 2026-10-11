// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Reflection;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphTimeWindowTests
{
    private const string RangeTypeName = "CodexInfo.WindowsClient.Graphing.GraphTimeRange";
    private const string WindowTypeName = "CodexInfo.WindowsClient.Graphing.GraphTimeWindow";

    [Fact]
    public void FixedBoundsUseExactDurationAndNowAtRightEdge()
    {
        var rangeType = RequiredType(RangeTypeName);
        var windowType = RequiredType(WindowTypeName);
        var getBounds = RequiredMethod(
            windowType,
            "GetBounds",
            rangeType,
            typeof(long),
            typeof(long?));
        const long now = 1_800_000_000;

        foreach (var (rangeName, seconds) in new[]
        {
            ("Last24Hours", 86_400L),
            ("Last7Days", 604_800L),
        })
        {
            var range = Enum.Parse(rangeType, rangeName);
            var bounds = getBounds.Invoke(null, [range, now, null]);
            Assert.NotNull(bounds);
            Assert.Equal(now - seconds, ReadLong(bounds!, "StartAt"));
            Assert.Equal(now, ReadLong(bounds!, "EndAt"));
        }
    }

    [Fact]
    public void BackAndForwardMoveOneFullWidthAndNeverMoveIntoTheFuture()
    {
        var rangeType = RequiredType(RangeTypeName);
        var windowType = RequiredType(WindowTypeName);
        var stepBack = RequiredMethod(windowType, "StepBack", rangeType, typeof(long), typeof(long?));
        var stepForward = RequiredMethod(windowType, "StepForward", rangeType, typeof(long), typeof(long?));
        var range = Enum.Parse(rangeType, "Last24Hours");
        const long now = 1_800_000_000;
        const long width = 86_400;

        var firstBack = InvokeNullableLong(stepBack, range, now, null);
        Assert.Equal(now - width, firstBack);
        var olderEnd = InvokeNullableLong(stepBack, range, now, firstBack);
        Assert.Equal(now - 2 * width, olderEnd);

        Assert.Equal(now - width, InvokeNullableLong(stepForward, range, now, olderEnd));
        Assert.Null(InvokeNullableLong(stepForward, range, now, firstBack));

        // A pinned historical end remains fixed while the clock advances.
        // Moving forward by one viewport can still be historical, but never future.
        const long laterNow = now + 1_200;
        var laterForward = InvokeNullableLong(stepForward, range, laterNow, firstBack);
        Assert.Equal(now, laterForward);
        Assert.True(laterForward <= laterNow);
    }

    private static Type RequiredType(string name)
    {
        var type = typeof(GraphWindowViewModel).Assembly.GetType(name);
        Assert.True(type is not null, $"Missing required range API type: {name}.");
        return type!;
    }

    [Fact]
    public void CalendarMonthUsesDisplayTimezoneAndStopsAtCurrentNow()
    {
        var rangeType = RequiredType(RangeTypeName);
        Assert.Contains("CalendarMonth", Enum.GetNames(rangeType));
        var range = Enum.Parse(rangeType, "CalendarMonth");
        var getBounds = RequiredMethod(RequiredType(WindowTypeName), "GetBounds",
            rangeType, typeof(long), typeof(long?), typeof(TimeZoneInfo));
        var tokyo = TimeZoneInfo.FindSystemTimeZoneById("Asia/Tokyo");
        var now = Unix("2026-10-11T03:00:00Z");
        var current = getBounds.Invoke(null, [range, now, null, tokyo]);
        Assert.NotNull(current);
        Assert.Equal(Unix("2026-09-30T15:00:00Z"), ReadLong(current, "StartAt"));
        Assert.Equal(now, ReadLong(current, "EndAt"));

        var newYork = TimeZoneInfo.FindSystemTimeZoneById("America/New_York");
        var historicalEnd = Unix("2024-04-01T04:00:00Z");
        var historical = getBounds.Invoke(null, [range, now, historicalEnd, newYork]);
        Assert.NotNull(historical);
        Assert.Equal(Unix("2024-03-01T05:00:00Z"), ReadLong(historical, "StartAt"));
        Assert.Equal(historicalEnd, ReadLong(historical, "EndAt"));
        Assert.Equal(31 * 86_400L - 3_600, ReadLong(historical, "EndAt") - ReadLong(historical, "StartAt"));
    }

    [Fact]
    public void CalendarMonthNavigationMovesByCalendarMonthAndNeverEntersFuture()
    {
        var rangeType = RequiredType(RangeTypeName);
        Assert.Contains("CalendarMonth", Enum.GetNames(rangeType));
        var range = Enum.Parse(rangeType, "CalendarMonth");
        var windowType = RequiredType(WindowTypeName);
        var back = RequiredMethod(windowType, "StepBack", rangeType, typeof(long), typeof(long?), typeof(TimeZoneInfo));
        var forward = RequiredMethod(windowType, "StepForward", rangeType, typeof(long), typeof(long?), typeof(long?), typeof(TimeZoneInfo));
        var getBounds = RequiredMethod(windowType, "GetBounds", rangeType, typeof(long), typeof(long?), typeof(TimeZoneInfo));
        foreach (var (nowText, expectedDays) in new[]
        {
            ("2024-03-15T12:00:00Z", 29),
            ("2025-03-15T12:00:00Z", 28),
            ("2026-05-15T12:00:00Z", 30),
            ("2026-02-15T12:00:00Z", 31),
        })
        {
            var now = Unix(nowText);
            var firstOfCurrent = new DateTimeOffset(DateTimeOffset.FromUnixTimeSeconds(now).Year,
                DateTimeOffset.FromUnixTimeSeconds(now).Month, 1, 0, 0, 0, TimeSpan.Zero).ToUnixTimeSeconds();
            var pinned = Assert.IsType<long>(back.Invoke(null, [range, now, null, TimeZoneInfo.Utc]));
            Assert.Equal(firstOfCurrent, pinned);
            var bounds = getBounds.Invoke(null, [range, now, pinned, TimeZoneInfo.Utc]);
            Assert.NotNull(bounds);
            Assert.Equal(expectedDays * 86_400L, ReadLong(bounds, "EndAt") - ReadLong(bounds, "StartAt"));
            Assert.Null(forward.Invoke(null, [range, now, pinned, null, TimeZoneInfo.Utc]));
            Assert.Null(forward.Invoke(null, [range, now, null, null, TimeZoneInfo.Utc]));
        }
    }

    private static long Unix(string value) => DateTimeOffset.Parse(value).ToUnixTimeSeconds();

    private static MethodInfo RequiredMethod(Type type, string name, params Type[] parameterTypes)
    {
        var method = type.GetMethod(
            name,
            BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static,
            binder: null,
            types: parameterTypes,
            modifiers: null);
        Assert.True(method is not null, $"Missing required range API method: {type.FullName}.{name}.");
        return method!;
    }

    private static long ReadLong(object value, string propertyName)
    {
        var property = value.GetType().GetProperty(propertyName, BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance);
        Assert.True(property is not null, $"Missing bounds property: {propertyName}.");
        return Assert.IsType<long>(property!.GetValue(value));
    }

    private static long? InvokeNullableLong(MethodInfo method, object range, long now, long? pinnedEndAt)
    {
        var value = method.Invoke(null, [range, now, pinnedEndAt]);
        return value is null ? null : Assert.IsType<long>(value);
    }
}
