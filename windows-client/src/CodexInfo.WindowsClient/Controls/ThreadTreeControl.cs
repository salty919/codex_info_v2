// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using Avalonia.Controls;
using Avalonia.Media;

namespace CodexInfo.WindowsClient.Controls;

internal readonly record struct ThreadTreeSegment(Point Start, Point End);

internal sealed record ThreadTreeGeometry(
    IReadOnlyList<ThreadTreeSegment> Segments,
    Point? JunctionDot);

/// <summary>
/// Draws the parent-to-child connections for the complete, scrolled thread list.
/// Adjacent rows use a short downwards connection. Only a child that is separated
/// from its parent by another subtree gets a left-side routed connection.
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
        var rail = new Pen(new SolidColorBrush(Color.Parse("#52718D")), 1.5);
        var geometry = BuildGeometry(Bounds.Width, Bounds.Height, Connections);
        foreach (var segment in geometry.Segments)
        {
            context.DrawLine(rail, segment.Start, segment.End);
        }
    }

    internal static ThreadTreeGeometry BuildGeometry(
        double width,
        double height,
        IReadOnlyList<ThreadTreeConnection> connections)
    {
        const double rowHeight = 96;
        const double cardLeft = 80;
        const double cardAnchorX = cardLeft + 16;
        const double cardTopInset = 6;
        const double cardBottomInset = 90;
        const double routedInset = 24;
        const double routedStep = 12;
        const double arrowHeight = 5;
        const double arrowWidth = 4;
        var cardX = Math.Min(cardAnchorX, Math.Max(0, width - 1));
        var segments = new List<ThreadTreeSegment>(connections.Count * 5);
        var seen = new HashSet<ThreadTreeSegment>();

        void Add(Point start, Point end)
        {
            if (start != end && seen.Add(new ThreadTreeSegment(start, end)))
            {
                segments.Add(new ThreadTreeSegment(start, end));
            }
        }

        void AddArrow(double childTop)
        {
            Add(new Point(cardX, childTop), new Point(cardX - arrowWidth, childTop - arrowHeight));
            Add(new Point(cardX, childTop), new Point(cardX + arrowWidth, childTop - arrowHeight));
        }

        var validConnections = connections
            .Where(connection => connection.ParentRow >= 0 && connection.ChildRow > connection.ParentRow)
            .ToArray();
        foreach (var connection in validConnections.Where(connection => connection.ChildRow == connection.ParentRow + 1))
        {
            var parentBottom = connection.ParentRow * rowHeight + cardBottomInset;
            var childTop = connection.ChildRow * rowHeight + cardTopInset;
            Add(new Point(cardX, parentBottom), new Point(cardX, childTop));
            AddArrow(childTop);
        }

        foreach (var group in validConnections
                     .Where(connection => connection.ChildRow > connection.ParentRow + 1)
                     .GroupBy(connection => (connection.ParentRow, connection.ParentDepth)))
        {
            var parentBottom = group.Key.ParentRow * rowHeight + cardBottomInset;
            var routeX = Math.Max(8, cardLeft - routedInset - Math.Clamp(group.Key.ParentDepth, 0, 4) * routedStep);
            var lastChildTop = group.Max(connection => connection.ChildRow) * rowHeight + cardTopInset;
            Add(new Point(cardX, parentBottom), new Point(routeX, parentBottom));
            Add(new Point(routeX, parentBottom), new Point(routeX, lastChildTop));
            foreach (var connection in group.OrderBy(connection => connection.ChildRow))
            {
                var childTop = connection.ChildRow * rowHeight + cardTopInset;
                Add(new Point(routeX, childTop), new Point(cardX, childTop));
                AddArrow(childTop);
            }
        }

        return new ThreadTreeGeometry(segments, null);
    }
}

public readonly record struct ThreadTreeConnection(int ParentRow, int ChildRow, int ParentDepth);
