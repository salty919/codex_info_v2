// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using Avalonia.Controls;
using Avalonia.Media;

namespace CodexInfo.WindowsClient.Controls;

internal readonly record struct ThreadTreeSegment(Point Start, Point End);

internal static class ThreadTreeLayout
{
    public const double RowHeight = 96;
    public const double CardLeft = 80;
    public const double RailBaseX = 24;
    public const double RailStep = 12;
    public const int MaximumDepth = 4;
}

internal sealed record ThreadTreeGeometry(
    IReadOnlyList<ThreadTreeSegment> Segments,
    Point? JunctionDot,
    IReadOnlyList<IReadOnlyList<Point>> Paths);

/// <summary>
/// Draws a conventional tree in the fixed left gutter of the complete, scrolled
/// thread list. Cards never move horizontally; each child receives a short
/// right-facing arrow from the rail for its parent's depth.
/// </summary>
public sealed class ThreadTreeControl : Control
{
    public static readonly StyledProperty<IReadOnlyList<ThreadTreeConnection>> ConnectionsProperty =
        AvaloniaProperty.Register<ThreadTreeControl, IReadOnlyList<ThreadTreeConnection>>(
            nameof(Connections), Array.Empty<ThreadTreeConnection>());

    public IReadOnlyList<ThreadTreeConnection> Connections
    {
        get => GetValue(ConnectionsProperty);
        set => SetValue(ConnectionsProperty, value);
    }

    public override void Render(DrawingContext context)
    {
        base.Render(context);
        var rail = new Pen(new SolidColorBrush(Color.Parse("#76A7CC")), 2)
        {
            LineCap = PenLineCap.Round,
            LineJoin = PenLineJoin.Round,
        };
        var geometry = BuildGeometry(Bounds.Width, Bounds.Height, Connections);
        foreach (var path in geometry.Paths)
        {
            if (path.Count < 2)
            {
                continue;
            }

            var stream = new StreamGeometry();
            using (var builder = stream.Open())
            {
                builder.BeginFigure(path[0], false);
                foreach (var point in path.Skip(1))
                {
                    builder.LineTo(point);
                }
            }
            context.DrawGeometry(null, rail, stream);
        }

        foreach (var segment in geometry.Segments.Where(segment => segment.Start.X != segment.End.X && segment.Start.Y != segment.End.Y))
        {
            context.DrawLine(rail, segment.Start, segment.End);
        }
    }

    internal static ThreadTreeGeometry BuildGeometry(
        double width,
        double height,
        IReadOnlyList<ThreadTreeConnection> connections)
    {
        const double arrowHeight = 5;
        const double arrowLength = 7;
        const double incomingPortOffset = 20;
        const double outgoingPortOffset = 76;
        var cardX = Math.Min(ThreadTreeLayout.CardLeft, Math.Max(0, width - 1));
        var segments = new List<ThreadTreeSegment>(connections.Count * 5);
        var paths = new List<IReadOnlyList<Point>>(connections.Count * 2);
        var seen = new HashSet<ThreadTreeSegment>();

        void Add(Point start, Point end)
        {
            if (start != end && seen.Add(new ThreadTreeSegment(start, end)))
            {
                segments.Add(new ThreadTreeSegment(start, end));
            }
        }

        void AddPath(params Point[] points)
        {
            var filtered = points
                .Where((point, index) => index == 0 || point != points[index - 1])
                .ToArray();
            if (filtered.Length < 2)
            {
                return;
            }

            paths.Add(filtered);
            for (var index = 1; index < filtered.Length; index++)
            {
                Add(filtered[index - 1], filtered[index]);
            }
        }

        void AddArrow(double childIncomingY)
        {
            var tip = new Point(cardX, childIncomingY);
            Add(tip, new Point(tip.X - arrowLength, childIncomingY - arrowHeight));
            Add(tip, new Point(tip.X - arrowLength, childIncomingY + arrowHeight));
        }

        var validConnections = connections
            .Where(connection => connection.ParentRow >= 0 && connection.ChildRow > connection.ParentRow)
            .ToArray();
        foreach (var group in validConnections.GroupBy(connection =>
                     (connection.ParentRow, ParentDepth: Math.Clamp(connection.ParentDepth, 0, ThreadTreeLayout.MaximumDepth))))
        {
            var parentDepth = group.Key.ParentDepth;
            var railX = ThreadTreeLayout.RailBaseX + parentDepth * ThreadTreeLayout.RailStep;
            var parentOutgoingY = group.Key.ParentRow * ThreadTreeLayout.RowHeight + outgoingPortOffset;
            var lastChildIncomingY = group.Max(connection =>
                connection.ChildRow * ThreadTreeLayout.RowHeight + incomingPortOffset);
            AddPath(new Point(cardX, parentOutgoingY), new Point(railX, parentOutgoingY),
                new Point(railX, lastChildIncomingY));
            foreach (var connection in group.OrderBy(connection => connection.ChildRow))
            {
                var childIncomingY = connection.ChildRow * ThreadTreeLayout.RowHeight + incomingPortOffset;
                AddPath(new Point(railX, childIncomingY), new Point(cardX, childIncomingY));
                AddArrow(childIncomingY);
            }
        }

        return new ThreadTreeGeometry(segments, null, paths);
    }
}

public readonly record struct ThreadTreeConnection(int ParentRow, int ChildRow, int ParentDepth);
