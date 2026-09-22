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

/// <summary>Draws the parent/child rails in the dedicated thread gutter.</summary>
public sealed class ThreadTreeControl : Control
{
    public static readonly StyledProperty<int> TreeDepthProperty = AvaloniaProperty.Register<ThreadTreeControl, int>(nameof(TreeDepth));
    public static readonly StyledProperty<bool> ConnectedToParentProperty = AvaloniaProperty.Register<ThreadTreeControl, bool>(nameof(ConnectedToParent));
    public static readonly StyledProperty<bool> HasChildrenProperty = AvaloniaProperty.Register<ThreadTreeControl, bool>(nameof(HasChildren));
    public static readonly StyledProperty<bool> HasNextSiblingProperty = AvaloniaProperty.Register<ThreadTreeControl, bool>(nameof(HasNextSibling));
    public static readonly StyledProperty<bool> AncestorGuide1Property = AvaloniaProperty.Register<ThreadTreeControl, bool>(nameof(AncestorGuide1));
    public static readonly StyledProperty<bool> AncestorGuide2Property = AvaloniaProperty.Register<ThreadTreeControl, bool>(nameof(AncestorGuide2));
    public static readonly StyledProperty<bool> AncestorGuide3Property = AvaloniaProperty.Register<ThreadTreeControl, bool>(nameof(AncestorGuide3));

    public int TreeDepth { get => GetValue(TreeDepthProperty); set => SetValue(TreeDepthProperty, value); }
    public bool ConnectedToParent { get => GetValue(ConnectedToParentProperty); set => SetValue(ConnectedToParentProperty, value); }
    public bool HasChildren { get => GetValue(HasChildrenProperty); set => SetValue(HasChildrenProperty, value); }
    public bool HasNextSibling { get => GetValue(HasNextSiblingProperty); set => SetValue(HasNextSiblingProperty, value); }
    public bool AncestorGuide1 { get => GetValue(AncestorGuide1Property); set => SetValue(AncestorGuide1Property, value); }
    public bool AncestorGuide2 { get => GetValue(AncestorGuide2Property); set => SetValue(AncestorGuide2Property, value); }
    public bool AncestorGuide3 { get => GetValue(AncestorGuide3Property); set => SetValue(AncestorGuide3Property, value); }

    public override void Render(DrawingContext context)
    {
        base.Render(context);
        var rail = new Pen(new SolidColorBrush(Color.Parse("#D5A43A")), 2);
        var geometry = BuildGeometry(
            Bounds.Width,
            Bounds.Height,
            TreeDepth,
            ConnectedToParent,
            HasChildren,
            HasNextSibling,
            AncestorGuide1,
            AncestorGuide2,
            AncestorGuide3);
        foreach (var segment in geometry.Segments)
        {
            context.DrawLine(rail, segment.Start, segment.End);
        }
        if (geometry.JunctionDot is { } junctionDot)
        {
            context.DrawEllipse(rail.Brush, null, junctionDot, 3, 3);
        }
    }

    internal static ThreadTreeGeometry BuildGeometry(
        double width,
        double height,
        int treeDepth,
        bool connectedToParent,
        bool hasChildren,
        bool hasNextSibling,
        bool ancestorGuide1,
        bool ancestorGuide2,
        bool ancestorGuide3)
    {
        const double baseX = 8;
        const double step = 12;
        var junctionY = height / 2;
        var junctionEndX = Math.Max(baseX + step, width - 5);
        var depth = Math.Clamp(treeDepth, 0, 3);
        var segments = new List<ThreadTreeSegment>(6);
        if (ancestorGuide1) segments.Add(new(new Point(baseX, 0), new Point(baseX, height)));
        if (ancestorGuide2) segments.Add(new(new Point(baseX + step, 0), new Point(baseX + step, height)));
        if (ancestorGuide3) segments.Add(new(new Point(baseX + step * 2, 0), new Point(baseX + step * 2, height)));
        Point? junctionDot = null;
        if (connectedToParent)
        {
            var x = baseX + Math.Max(0, depth - 1) * step;
            segments.Add(new(new Point(x, 0), new Point(x, hasNextSibling ? height : junctionY)));
            segments.Add(new(new Point(x, junctionY), new Point(junctionEndX, junctionY)));
            junctionDot = new Point(junctionEndX, junctionY);
        }
        if (hasChildren)
        {
            var x = baseX + depth * step;
            segments.Add(new(new Point(x, junctionY), new Point(x, height)));
        }

        return new ThreadTreeGeometry(segments, junctionDot);
    }
}
