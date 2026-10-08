// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Controls;

/// <summary>
/// Positions period-cost labels inside the final data rectangle immediately
/// before ScottPlot draws its plottables. The action changes only label font
/// size and pixel offsets; it does not alter graph data or axes.
/// </summary>
internal sealed class GraphPeriodCostLabelsLayoutAction(
    IReadOnlyList<ScottPlot.Plottables.Text> labels) : ScottPlot.IRenderAction
{
    internal const float LabelPadding = 5;
    internal const float EdgeInset = 4;
    private const float BaseFontSize = 11;
    private const float LaneGap = 3;

    public void Render(ScottPlot.RenderPack renderPack)
    {
        if (labels.Count == 0 || renderPack.DataRect.Width <= 0 || renderPack.DataRect.Height <= 0)
        {
            return;
        }

        var measuredAtBase = labels.Select(label => Measure(label, BaseFontSize)).ToArray();
        var padding = labels[0].LabelPixelPadding;
        var maximumTextWidth = measuredAtBase.Max(measurement => measurement.Width);
        var maximumTextHeight = measuredAtBase.Max(measurement => measurement.Height);
        var availableTextWidth = Math.Max(
            1,
            renderPack.DataRect.Width - (2 * EdgeInset) - padding.Horizontal);
        var availableTextHeight = Math.Max(
            1,
            renderPack.DataRect.Height -
            (2 * EdgeInset) -
            (labels.Count * padding.Vertical) -
            ((labels.Count - 1) * LaneGap));
        var widthScale = availableTextWidth / maximumTextWidth;
        var heightScale = availableTextHeight / (labels.Count * maximumTextHeight);
        var fontSize = (float)(BaseFontSize * Math.Min(1, Math.Min(widthScale, heightScale)));

        var measurements = new LabelMeasurement[labels.Count];
        for (var index = 0; index < labels.Count; index++)
        {
            labels[index].LabelFontSize = fontSize;
            measurements[index] = Measure(labels[index]);
        }

        var laneHeight = measurements.Max(measurement => measurement.Height) + padding.Vertical;
        for (var index = 0; index < labels.Count; index++)
        {
            var label = labels[index];
            var anchorX = renderPack.Plot.Axes.Bottom.GetPixel(label.Location.X, renderPack.DataRect);
            var anchorY = renderPack.Plot.Axes.Left.GetPixel(label.Location.Y, renderPack.DataRect);
            var minimumRight = renderPack.DataRect.Left + EdgeInset + measurements[index].Width + padding.Left;
            var maximumRight = renderPack.DataRect.Right - EdgeInset - padding.Right;
            var right = Math.Clamp(anchorX - EdgeInset, minimumRight, maximumRight);
            var top = renderPack.DataRect.Top + EdgeInset + padding.Top + (index * (laneHeight + LaneGap));

            label.OffsetX = right - anchorX;
            label.OffsetY = top - anchorY;
        }
    }

    private static LabelMeasurement Measure(ScottPlot.Plottables.Text label, float? fontSize = null)
    {
        if (fontSize is { } size)
        {
            label.LabelFontSize = size;
        }

        using var paint = ScottPlot.Paint.NewDisposablePaint();
        var measured = label.LabelStyle.Measure(label.LabelText, paint);
        return new LabelMeasurement(measured.Width, measured.Height);
    }

    private readonly record struct LabelMeasurement(float Width, float Height);
}
