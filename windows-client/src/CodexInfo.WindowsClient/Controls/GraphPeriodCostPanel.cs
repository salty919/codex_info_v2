// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Theme;
using SkiaSharp;

namespace CodexInfo.WindowsClient.Controls;

/// <summary>
/// Reserves a native ScottPlot footer below the time axis and renders each
/// visible reset-period amount at that period's horizontal midpoint.
/// </summary>
internal sealed class GraphPeriodCostPanel(
    IReadOnlyList<GraphPeriodCostAmount> amounts,
    IReadOnlyList<long> resetGuideTimestamps) : ScottPlot.IPanel
{
    internal const float FooterHeight = 48;
    internal const float BaseFontSize = 22;

    private const float HorizontalInset = 8;

    internal IReadOnlyList<GraphPeriodCostAmount> Amounts { get; } = amounts;
    internal IReadOnlyList<long> ResetGuideTimestamps { get; } = resetGuideTimestamps;
    internal ScottPlot.PixelRect? LastRenderBounds { get; private set; }
    internal ScottPlot.Color BackgroundColor => new(ThemePalette.Resolve(GraphPlotControl.PlotColorHex));
    internal ScottPlot.Color AmountColor =>
        new ScottPlot.Color(ThemePalette.Resolve(GraphPlotControl.PeriodAmountColorHex));
    internal ScottPlot.Color ResetSeparatorColor => AmountColor.WithOpacity(0.70);

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

        if (panelRect.Width <= 0 || panelRect.Height <= 0)
        {
            return;
        }

        using (var resetGuide = new SKPaint
        {
            Color = SKColor.Parse(ThemePalette.Resolve(GraphPlotControl.PeriodAmountColorHex)).WithAlpha(178),
            Style = SKPaintStyle.Stroke,
            StrokeWidth = 1,
            IsAntialias = false,
        })
        {
            foreach (var timestamp in ResetGuideTimestamps)
            {
                var x = renderPack.Plot.Axes.Bottom.GetPixel(timestamp, renderPack.DataRect);
                if (x >= panelRect.Left && x <= panelRect.Right)
                {
                    renderPack.Canvas.DrawLine(x, panelRect.Top, x, panelRect.Bottom, resetGuide);
                }
            }
        }

        foreach (var amount in Amounts)
        {
            var periodLeft = renderPack.Plot.Axes.Bottom.GetPixel(amount.VisibleStartAt, renderPack.DataRect);
            var periodRight = renderPack.Plot.Axes.Bottom.GetPixel(amount.VisibleEndAt, renderPack.DataRect);
            var periodWidth = Math.Max(0, periodRight - periodLeft);
            if (periodWidth <= 0)
            {
                continue;
            }

            var inset = Math.Min(HorizontalInset, periodWidth * 0.1f);
            var availableWidth = periodWidth - (2 * inset);
            var fontSize = SelectFontSize(amount, availableWidth);
            amount.LabelStyle.FontSize = fontSize;

            using var paint = ScottPlot.Paint.NewDisposablePaint();
            amount.LabelStyle.ApplyToPaint(paint);
            using var font = new SKFont(paint.SKTypeface, fontSize);
            font.MeasureText(amount.LabelStyle.Text, out var ink, paint.SKPaint);
            var center = renderPack.Plot.Axes.Bottom.GetPixel(amount.CenterAt, renderPack.DataRect);
            var x = center - ink.MidX;
            var baseline = panelRect.VerticalCenter - ink.MidY;
            renderPack.Canvas.DrawText(
                amount.LabelStyle.Text, x, baseline, SKTextAlign.Left, font, paint.SKPaint);
            amount.RecordRender(center, new ScottPlot.PixelRect(
                x + ink.Left, x + ink.Right, baseline + ink.Bottom, baseline + ink.Top));
        }
    }

    private static float SelectFontSize(GraphPeriodCostAmount amount, float availableWidth)
    {
        var fontSize = BaseFontSize;
        for (var attempt = 0; attempt < 32; attempt++)
        {
            var width = MeasureAmountWidth(amount, fontSize);
            if (width <= availableWidth || width <= 0)
            {
                return fontSize;
            }

            var scale = Math.Clamp((availableWidth / width) * 0.99f, 0.01f, 0.99f);
            var nextFontSize = fontSize * scale;
            if (nextFontSize >= fontSize || nextFontSize <= 0)
            {
                return fontSize;
            }

            fontSize = nextFontSize;
        }

        return fontSize;
    }

    private static float MeasureAmountWidth(GraphPeriodCostAmount amount, float fontSize)
    {
        amount.LabelStyle.FontSize = fontSize;
        using var paint = ScottPlot.Paint.NewDisposablePaint();
        amount.LabelStyle.ApplyToPaint(paint);
        using var font = new SKFont(paint.SKTypeface, fontSize);
        font.MeasureText(amount.LabelStyle.Text, out var ink, paint.SKPaint);
        return ink.Width;
    }

    private static SKRect ToSKRect(ScottPlot.PixelRect rect) =>
        new(rect.Left, rect.Top, rect.Right, rect.Bottom);
}

internal sealed class GraphPeriodCostAmount(
    double visibleStartAt,
    double visibleEndAt,
    double centerAt,
    ScottPlot.LabelStyle labelStyle)
{
    internal double VisibleStartAt { get; } = visibleStartAt;
    internal double VisibleEndAt { get; } = visibleEndAt;
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
