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
