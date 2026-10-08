// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Theme;
using SkiaSharp;

namespace CodexInfo.WindowsClient.Controls;

/// <summary>
/// Reserves a native ScottPlot footer below the time axis and renders each
/// visible reset-period amount at that period's horizontal midpoint.
/// </summary>
internal sealed class GraphPeriodCostPanel(IReadOnlyList<GraphPeriodCostAmount> amounts) : ScottPlot.IPanel
{
    internal const float FooterHeight = 48;
    internal const float BaseFontSize = 30;

    private const float HorizontalInset = 8;
    private const float LabelGap = 8;
    private const float MinimumFontSize = 5;
    private const string AmountColorHex = "#E6B85C";

    internal IReadOnlyList<GraphPeriodCostAmount> Amounts { get; } = amounts;
    internal ScottPlot.PixelRect? LastRenderBounds { get; private set; }
    internal ScottPlot.Color BackgroundColor => new(ThemePalette.Resolve(GraphPlotControl.PlotColorHex));
    internal ScottPlot.Color AmountColor => new(ThemePalette.Resolve(AmountColorHex));

    public bool IsVisible { get; set; } = true;
    public float MinimumSize { get; set; } = FooterHeight;
    public float MaximumSize { get; set; } = FooterHeight;
    public ScottPlot.Edge Edge => ScottPlot.Edge.Bottom;
    public bool ShowDebugInformation { get; set; }

    public float Measure(ScottPlot.Paint paint) => FooterHeight;

    public ScottPlot.PixelRect GetPanelRect(
        ScottPlot.PixelRect dataRect,
        float size,
        float offset,
        ScottPlot.Paint paint)
    {
        var bottom = dataRect.Bottom + offset;
        return new ScottPlot.PixelRect(dataRect.Left, dataRect.Right, bottom + size, bottom);
    }

    public void Render(ScottPlot.RenderPack renderPack, float size, float offset)
    {
        var panelRect = GetPanelRect(renderPack.DataRect, size, offset, renderPack.Paint);
        LastRenderBounds = panelRect;

        using (var background = new SKPaint
        {
            Color = SKColor.Parse(ThemePalette.Resolve(GraphPlotControl.PlotColorHex)),
            Style = SKPaintStyle.Fill,
            IsAntialias = false,
        })
        {
            renderPack.Canvas.DrawRect(ToSKRect(panelRect), background);
        }

        using (var divider = new SKPaint
        {
            Color = SKColor.Parse(ThemePalette.Resolve(GraphPlotControl.GridColorHex)),
            Style = SKPaintStyle.Stroke,
            StrokeWidth = 1,
            IsAntialias = false,
        })
        {
            renderPack.Canvas.DrawLine(panelRect.Left, panelRect.Top, panelRect.Right, panelRect.Top, divider);
        }

        if (Amounts.Count == 0 || panelRect.Width <= 0 || panelRect.Height <= 0)
        {
            return;
        }

        var fontSize = SelectFontSize(panelRect.Width);
        var measurements = MeasureAmounts(fontSize);
        var centers = PositionAmounts(renderPack, panelRect, measurements);
        for (var index = 0; index < Amounts.Count; index++)
        {
            var amount = Amounts[index];
            amount.LabelStyle.FontSize = fontSize;

            using var paint = ScottPlot.Paint.NewDisposablePaint();
            amount.LabelStyle.ApplyToPaint(paint);
            using var font = new SKFont(paint.SKTypeface, fontSize);
            font.MeasureText(amount.LabelStyle.Text, out var ink, paint.SKPaint);
            // Center the painted glyphs, not ScottPlot's text layout box. Its
            // baseline adjustment can place embedded-font ink above that box.
            var x = centers[index] - ink.MidX;
            var baseline = panelRect.VerticalCenter - ink.MidY;
            renderPack.Canvas.DrawText(
                amount.LabelStyle.Text, x, baseline, SKTextAlign.Left, font, paint.SKPaint);
            amount.RecordRender(centers[index], new ScottPlot.PixelRect(
                x + ink.Left, x + ink.Right, baseline + ink.Bottom, baseline + ink.Top));
        }
    }

    private float SelectFontSize(float panelWidth)
    {
        var fontSize = BaseFontSize;
        while (fontSize > MinimumFontSize)
        {
            var measurements = MeasureAmounts(fontSize);
            var totalWidth = measurements.Sum(measurement => measurement.Width) +
                ((Amounts.Count - 1) * LabelGap);
            if (totalWidth <= panelWidth - (2 * HorizontalInset))
            {
                break;
            }

            fontSize = Math.Max(MinimumFontSize, fontSize - 1);
        }

        return fontSize;
    }

    private AmountMeasurement[] MeasureAmounts(float fontSize)
    {
        var measurements = new AmountMeasurement[Amounts.Count];
        for (var index = 0; index < Amounts.Count; index++)
        {
            var style = Amounts[index].LabelStyle;
            style.FontSize = fontSize;
            using var paint = ScottPlot.Paint.NewDisposablePaint();
            style.ApplyToPaint(paint);
            using var font = new SKFont(paint.SKTypeface, fontSize);
            font.MeasureText(style.Text, out var ink, paint.SKPaint);
            measurements[index] = new AmountMeasurement(ink.Width);
        }

        return measurements;
    }

    private float[] PositionAmounts(
        ScottPlot.RenderPack renderPack,
        ScottPlot.PixelRect panelRect,
        IReadOnlyList<AmountMeasurement> measurements)
    {
        var centers = Amounts
            .Select(amount => renderPack.Plot.Axes.Bottom.GetPixel(amount.CenterAt, renderPack.DataRect))
            .ToArray();
        var minimumCenters = new float[centers.Length];
        var maximumCenters = new float[centers.Length];

        for (var index = 0; index < centers.Length; index++)
        {
            var halfWidth = measurements[index].Width / 2;
            minimumCenters[index] = panelRect.Left + HorizontalInset + halfWidth;
            maximumCenters[index] = panelRect.Right - HorizontalInset - halfWidth;
            centers[index] = Math.Clamp(
                centers[index],
                minimumCenters[index],
                maximumCenters[index]);
        }

        for (var index = 1; index < centers.Length; index++)
        {
            var minimum = centers[index - 1] + Separation(index - 1, measurements);
            centers[index] = Math.Max(centers[index], minimum);
        }

        for (var index = centers.Length - 1; index >= 0; index--)
        {
            var maximum = maximumCenters[index];
            if (index < centers.Length - 1)
            {
                maximum = Math.Min(maximum, centers[index + 1] - Separation(index, measurements));
            }

            centers[index] = Math.Min(centers[index], maximum);
        }

        return centers;
    }

    private static float Separation(int leftIndex, IReadOnlyList<AmountMeasurement> measurements) =>
        (measurements[leftIndex].Width / 2) +
        (measurements[leftIndex + 1].Width / 2) +
        LabelGap;

    private static SKRect ToSKRect(ScottPlot.PixelRect rect) =>
        new(rect.Left, rect.Top, rect.Right, rect.Bottom);

    private readonly record struct AmountMeasurement(float Width);
}

internal sealed class GraphPeriodCostAmount(double centerAt, ScottPlot.LabelStyle labelStyle)
{
    internal double CenterAt { get; } = centerAt;
    internal ScottPlot.LabelStyle LabelStyle { get; } = labelStyle;
    internal ScottPlot.PixelRect? LastRenderBounds { get; private set; }
    internal float? LastRenderCenterX { get; private set; }

    internal void RecordRender(float centerX, ScottPlot.PixelRect bounds)
    {
        LastRenderCenterX = centerX;
        LastRenderBounds = bounds;
    }
}
