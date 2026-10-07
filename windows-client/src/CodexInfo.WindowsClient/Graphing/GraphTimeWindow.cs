// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Graphing;

/// <summary>The time range shown by the Windows usage graph.</summary>
public enum GraphTimeRange
{
    ResetPeriod,
    Last24Hours,
    Last7Days,
}

/// <summary>The exact, half-open Unix-time viewport for a fixed graph range.</summary>
public readonly record struct GraphTimeBounds(long StartAt, long EndAt);

/// <summary>Pure range-width and navigation rules shared by the graph view model.</summary>
public static class GraphTimeWindow
{
    public const long DaySeconds = 86_400;
    public const long WeekSeconds = 7 * DaySeconds;

    public static GraphTimeBounds GetBounds(
        GraphTimeRange range,
        long now,
        long? pinnedEndAt)
    {
        var duration = GetDurationSeconds(range);
        var endAt = pinnedEndAt is { } pinned && pinned < now ? pinned : now;
        return new GraphTimeBounds(endAt - duration, endAt);
    }

    /// <summary>Moves a fixed range into history by exactly one current range width.</summary>
    public static long StepBack(GraphTimeRange range, long now, long? pinnedEndAt)
    {
        var duration = GetDurationSeconds(range);
        var endAt = pinnedEndAt is { } pinned && pinned < now ? pinned : now;
        return endAt - duration;
    }

    /// <summary>
    /// Moves one fixed width toward the present. A null result means the new
    /// viewport is live and ends at now, avoiding a future blank axis.
    /// </summary>
    public static long? StepForward(GraphTimeRange range, long now, long? pinnedEndAt)
        => StepForward(range, now, pinnedEndAt, navigationOriginAt: null);

    /// <summary>
    /// Moves one fixed width toward the present. Once the original live edge
    /// is reached, or the clock has moved behind it, a null result returns to
    /// the current live viewport.
    /// </summary>
    public static long? StepForward(
        GraphTimeRange range,
        long now,
        long? pinnedEndAt,
        long? navigationOriginAt)
    {
        var duration = GetDurationSeconds(range);
        if (pinnedEndAt is not { } pinned)
        {
            return null;
        }

        var nextEndAt = pinned + duration;
        var liveEdge = navigationOriginAt is { } origin ? Math.Min(origin, now) : now;
        return nextEndAt >= liveEdge ? null : nextEndAt;
    }

    public static long GetDurationSeconds(GraphTimeRange range) => range switch
    {
        GraphTimeRange.Last24Hours => DaySeconds,
        GraphTimeRange.Last7Days => WeekSeconds,
        _ => throw new ArgumentOutOfRangeException(nameof(range), range, "A fixed graph range is required."),
    };
}
