// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Theme;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphPeriodCostRenderingTests
{
    [Fact]
    public void EmbeddedPeriodCostFontAliasesResolveAmountGlyphs()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        GraphPeriodCostFontResolver.EnsureRegistered();
        try
        {
            Assert.IsType<GraphPeriodCostFontResolver>(ScottPlot.Fonts.FontResolvers[0]);
            Assert.Equal(GraphPeriodCostFontResolver.JapaneseAlias, GraphPeriodCostFontResolver.AliasForLanguage("ja"));
            Assert.Equal(GraphPeriodCostFontResolver.KoreanAlias, GraphPeriodCostFontResolver.AliasForLanguage("ko"));

            var japanese = ScottPlot.Fonts.GetTypeface(GraphPeriodCostFontResolver.JapaneseAlias, false, false);
            var korean = ScottPlot.Fonts.GetTypeface(GraphPeriodCostFontResolver.KoreanAlias, false, false);
            AssertGlyphsAvailable(japanese, "$1");
            AssertGlyphsAvailable(korean, "$1");

            var scene = GraphScene.Create(
                [Sample(1_000, Model("gpt-6-sol", 1.0))],
                GraphMetric.Dollars,
                1_000,
                1_060);

            LocalizationService.SetLanguage("ja");
            var japaneseControl = new GraphPlotControl { Scene = scene };
            var japanesePanel = Panel(japaneseControl);
            using var japaneseImage = japaneseControl.Plot.GetImage(900, 542);
            var japaneseAmount = Assert.Single(japanesePanel.Amounts);
            Assert.Equal("$1", japaneseAmount.LabelStyle.Text);
            Assert.Equal(GraphPeriodCostFontResolver.JapaneseAlias, japaneseAmount.LabelStyle.FontName);
            AssertLabelPaintUsesFontWithGlyphs(japaneseAmount, japanese, "$1");

            LocalizationService.SetLanguage("ko");
            var koreanControl = new GraphPlotControl { Scene = scene };
            var koreanPanel = Panel(koreanControl);
            using var koreanImage = koreanControl.Plot.GetImage(900, 542);
            var koreanAmount = Assert.Single(koreanPanel.Amounts);
            Assert.Equal("$1", koreanAmount.LabelStyle.Text);
            Assert.Equal(GraphPeriodCostFontResolver.KoreanAlias, koreanAmount.LabelStyle.FontName);
            AssertLabelPaintUsesFontWithGlyphs(koreanAmount, korean, "$1");
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void CurrentPeriodAmountIsRoundedAndCenteredInTheBottomFooter()
    {
        var scene = GraphScene.Create(
        [
            Sample(1_000, Model("gpt-6-sol", 30.10), Model("gpt-5.6-sol", 44.52)),
            Sample(1_060, Model("gpt-6-sol", 30.10), Model("gpt-5.6-sol", 44.52)),
        ], GraphMetric.Dollars, 1_000, 2_000);
        var control = new GraphPlotControl { Scene = scene };
        var panel = Panel(control);

        using var rendered = control.Plot.GetImage(900, 542);
        var amount = Assert.Single(panel.Amounts);
        var dataRect = control.Plot.LastRender.DataRect;
        var footerRect = AssertRenderedRect(panel.LastRenderBounds);
        var amountRect = AssertRenderedRect(amount.LastRenderBounds);

        Assert.Equal("$75", amount.LabelStyle.Text);
        Assert.Equal(GraphPeriodCostPanel.BaseFontSize, amount.LabelStyle.FontSize);
        Assert.Equal(ScottPlot.Edge.Bottom, panel.Edge);
        Assert.InRange(Math.Abs(footerRect.Height - GraphPeriodCostPanel.FooterHeight), 0, 0.5f);
        Assert.True(footerRect.Top >= dataRect.Bottom);
        Assert.True(footerRect.Bottom <= 542);
        Assert.True(amountRect.Left >= footerRect.Left + 8);
        Assert.True(amountRect.Right <= footerRect.Right - 8);
        Assert.True(amountRect.Top >= footerRect.Top);
        Assert.True(amountRect.Bottom <= footerRect.Bottom);
        Assert.InRange(
            Math.Abs(amount.LastRenderCenterX!.Value - control.Plot.Axes.Bottom.GetPixel(1_500, dataRect)),
            0,
            0.1f);
        Assert.Empty(control.Plot.GetPlottables<ScottPlot.Plottables.Text>());

        // Inspect actual painted glyph pixels: ScottPlot label layout bounds
        // do not establish the visible distance from the footer divider.
        using var bitmap = SkiaSharp.SKBitmap.Decode(rendered.GetImageBytes());
        var amountColor = SkiaSharp.SKColor.Parse(ThemePalette.Resolve("#E6B85C"));
        var inkRows = new List<int>();
        for (var y = (int)Math.Ceiling(dataRect.Bottom); y < bitmap.Height; y++)
        {
            for (var x = (int)Math.Ceiling(dataRect.Left); x < (int)dataRect.Right; x++)
            {
                if (bitmap.GetPixel(x, y) == amountColor)
                {
                    inkRows.Add(y);
                    break;
                }
            }
        }
        Assert.NotEmpty(inkRows);
        Assert.True(inkRows.Min() >= footerRect.Top + 8,
            $"Amount ink starts at {inkRows.Min()}, footer divider is at {footerRect.Top}");
        Assert.True(inkRows.Max() <= footerRect.Bottom - 8);

    }

    [Fact]
    public void AmountsScaleToTheirVisiblePeriodAndKeepThePeriodMidpoint()
    {
        var periods = new[]
        {
            Period(900, 1_200, 274, 0),
            Period(1_200, 1_500, 300, 0),
            Period(1_500, 1_800, 106, 0),
        };
        var viewport = GraphScene.CreateViewport(0, 3_000, GraphMetric.Dollars, periods);
        var control = new GraphPlotControl { Scene = viewport };
        var panel = Panel(control);

        using var rendered = control.Plot.GetImage(900, 640);
        var dataRect = control.Plot.LastRender.DataRect;
        var amounts = panel.Amounts.OrderBy(amount => amount.CenterAt).ToArray();

        Assert.Equal(["$274", "$300", "$106"], amounts.Select(amount => amount.LabelStyle.Text));
        for (var index = 0; index < amounts.Length; index++)
        {
            var amount = amounts[index];
            var period = periods[index];
            var intervalLeft = control.Plot.Axes.Bottom.GetPixel(period.PeriodStartAt, dataRect);
            var intervalRight = control.Plot.Axes.Bottom.GetPixel(period.PeriodEndAt, dataRect);
            var renderedBounds = AssertRenderedRect(amount.LastRenderBounds);
            var expectedCenter = control.Plot.Axes.Bottom.GetPixel(amount.CenterAt, dataRect);

            Assert.InRange(intervalRight - intervalLeft, 70, 100);
            Assert.InRange(amount.LabelStyle.FontSize, 14, 22);
            Assert.True(renderedBounds.Left >= intervalLeft + 8,
                $"{amount.LabelStyle.Text} starts at {renderedBounds.Left} in [{intervalLeft}, {intervalRight}]");
            Assert.True(renderedBounds.Right <= intervalRight - 8,
                $"{amount.LabelStyle.Text} ends at {renderedBounds.Right} in [{intervalLeft}, {intervalRight}]");
            Assert.InRange(Math.Abs(amount.LastRenderCenterX!.Value - expectedCenter), 0, 0.1f);
        }
    }

    [Fact]
    public void ResetSeparatorUsesPublishedStartInPlotButNotInFooter()
    {
        var scene = GraphScene.CreateViewport(
            1_000,
            1_200,
            GraphMetric.Dollars,
            [
                Period(1_000, 1_100, 1, 0, resetAt: 1_125),
                Period(1_100, 1_200, 1, 0, resetAt: 1_300),
            ]);
        var control = new GraphPlotControl { Scene = scene };
        var panel = Panel(control);

        using var rendered = control.Plot.GetImage(900, 640);
        var expectedResetColor = new ScottPlot.Color(
            ThemePalette.Resolve("#E6B85C")).WithOpacity(0.70);
        var dataRect = control.Plot.LastRender.DataRect;
        var footerRect = AssertRenderedRect(panel.LastRenderBounds);
        var publishedStartX = (int)Math.Round(control.Plot.Axes.Bottom.GetPixel(1_100, dataRect));
        var deadlineX = (int)Math.Round(control.Plot.Axes.Bottom.GetPixel(1_125, dataRect));
        using var bitmap = SkiaSharp.SKBitmap.Decode(rendered.GetImageBytes());
        var footerBackground = SkiaSharp.SKColor.Parse(ThemePalette.Resolve(GraphPlotControl.PlotColorHex));
        var footerBoundaryPixels =
            from x in Enumerable.Range(Math.Max(0, publishedStartX - 1), 3)
            from y in Enumerable.Range((int)Math.Ceiling(footerRect.Top) + 2, Math.Max(0, (int)footerRect.Bottom - (int)footerRect.Top - 4))
            let color = bitmap.GetPixel(x, y)
            where color.Red > footerBackground.Red + 30 && color.Green > footerBackground.Green + 30
            select color;
        var footerDeadlinePixels =
            from x in Enumerable.Range(Math.Max(0, deadlineX - 1), 3)
            from y in Enumerable.Range((int)Math.Ceiling(footerRect.Top) + 2, Math.Max(0, (int)footerRect.Bottom - (int)footerRect.Top - 4))
            let color = bitmap.GetPixel(x, y)
            where color.Red > footerBackground.Red + 30 && color.Green > footerBackground.Green + 30
            select color;

        Assert.Empty(footerBoundaryPixels);
        Assert.Empty(footerDeadlinePixels);

        var plotResetGuide = control.Plot.GetPlottables<ScottPlot.Plottables.Scatter>().Any(scatter =>
        {
            var points = scatter.Data.GetScatterPoints().ToArray();
            return points.Length == 2 && points.All(point => point.X == 1_100) && scatter.Color == expectedResetColor;
        });
        Assert.True(plotResetGuide, "The plot separator must use the published period start and amount theme color.");
    }

    [Fact]
    public void PartialUnavailableAndConfirmedZeroRenderOnlyTheirAmounts()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        LocalizationService.SetLanguage("ja");
        try
        {
            var partial = GraphScene.Create(
            [
                Sample(1_000, Model("gpt-6-sol", 1.25)) with { ModelsComplete = false },
            ], GraphMetric.Tokens, 1_000, 1_060);
            var unavailable = GraphScene.Create(
            [
                Sample(1_000, Model("gpt-6-sol", null)) with
                {
                    ModelSource = ApiHistorySample.UnavailableModelSource,
                    ModelsComplete = false,
                    ModelSamples = null,
                },
            ], GraphMetric.Dollars, 1_000, 1_060);
            var zero = GraphScene.Create(
            [
                Sample(1_000),
            ], GraphMetric.Tokens, 1_000, 1_060);

            Assert.Equal("$1", Assert.Single(Panel(new GraphPlotControl { Scene = partial }).Amounts).LabelStyle.Text);
            Assert.Equal("—", Assert.Single(Panel(new GraphPlotControl { Scene = unavailable }).Amounts).LabelStyle.Text);
            Assert.Equal("$0", Assert.Single(Panel(new GraphPlotControl { Scene = zero }).Amounts).LabelStyle.Text);
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void SummaryUsesPersistedDollarsRegardlessOfMetricOrModelVisibility()
    {
        var samples = new[]
        {
            Sample(1_000, Model("gpt-6-sol", 0.75), Model("gpt-5.6-sol", 0.25)),
            Sample(1_060, Model("gpt-6-sol", 1.5), Model("gpt-5.6-sol", 0.5)),
        };
        var dollarScene = GraphScene.Create(samples, GraphMetric.Dollars, 1_000, 1_120);
        var tokenScene = GraphScene.Create(samples, GraphMetric.Tokens, 1_000, 1_120);
        var visible = new GraphPlotControl { Scene = dollarScene };
        var hidden = new GraphPlotControl
        {
            Scene = tokenScene,
            ShowRemaining = false,
            ShowModels = false,
            ShowSol = false,
            ShowTerra = false,
            ShowLuna = false,
            ShowAstra = false,
        };

        using var visibleImage = visible.Plot.GetImage(900, 542);
        using var hiddenImage = hidden.Plot.GetImage(900, 542);
        Assert.Equal("$2", Assert.Single(Panel(visible).Amounts).LabelStyle.Text);
        Assert.Equal("$2", Assert.Single(Panel(hidden).Amounts).LabelStyle.Text);
        var hiddenModelLines = hidden.Plot.GetPlottables<ScottPlot.Plottables.Scatter>()
            .Where(series => series.LineWidth >= GraphPlotControl.MeasuredModelLineWidth);
        Assert.NotEmpty(hiddenModelLines);
        Assert.All(hiddenModelLines, series => Assert.False(series.IsVisible));
    }

    [Fact]
    public void SummaryUsesThemeAmountColorAndUnchangedPlotBackground()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        var originalTheme = ThemePalette.CurrentId;
        LocalizationService.SetLanguage("en");
        ThemePalette.Apply(ThemePalette.PaperLight);
        try
        {
            var scene = GraphScene.Create(
                [Sample(1_000, Model("gpt-6-sol", 1.0))],
                GraphMetric.Dollars,
                1_000,
                1_060);
            var control = new GraphPlotControl { Scene = scene };
            var panel = Panel(control);
            using var rendered = control.Plot.GetImage(900, 542);
            var amount = Assert.Single(panel.Amounts);

            Assert.Equal("$1", amount.LabelStyle.Text);
            Assert.Equal(new ScottPlot.Color(ThemePalette.Resolve("#E6B85C")), amount.LabelStyle.ForeColor);
            Assert.Equal(
                new ScottPlot.Color(ThemePalette.Resolve(GraphPlotControl.PlotColorHex)),
                panel.BackgroundColor);
        }
        finally
        {
            ThemePalette.Apply(originalTheme);
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void ViewportIncludesLeftClippedPeriodAmountAndKeepsTheRightPartialPeriod()
    {
        var first = Period(0, 100, 0.10, 0.20);
        var second = Period(100, 200, 0.40, 0.60);
        var third = Period(200, 300, 0.75, 1.25);
        var viewport = GraphScene.CreateViewport(50, 280, GraphMetric.Dollars, [first, second, third]);
        var control = new GraphPlotControl { Scene = viewport };
        var panel = Panel(control);

        using var rendered = control.Plot.GetImage(900, 542);
        var amounts = panel.Amounts;
        var dataRect = control.Plot.LastRender.DataRect;

        Assert.Equal(["$0", "$1", "$2"], amounts.Select(amount => amount.LabelStyle.Text));
        Assert.Equal([75d, 150d, 240d], amounts.Select(amount => amount.CenterAt));
        Assert.InRange(
            Math.Abs(amounts[0].LastRenderCenterX!.Value - control.Plot.Axes.Bottom.GetPixel(75, dataRect)),
            0,
            1);
        Assert.InRange(
            Math.Abs(amounts[1].LastRenderCenterX!.Value - control.Plot.Axes.Bottom.GetPixel(150, dataRect)),
            0,
            1);
        Assert.InRange(
            Math.Abs(amounts[2].LastRenderCenterX!.Value - control.Plot.Axes.Bottom.GetPixel(240, dataRect)),
            0,
            1);
        Assert.Contains(control.Plot.GetPlottables<ScottPlot.Plottables.Scatter>()
            .Where(series => series.LineWidth >= GraphPlotControl.MeasuredModelLineWidth)
            .SelectMany(series => series.Data.GetScatterPoints()),
            point => point.X == 50);
    }

    [Fact]
    public void FirstAndResizedNarrowRendersKeepAmountsInsideOneFooterRow()
    {
        var periods = new[]
        {
            Period(0, 100, 0.10, 0.20),
            Period(100, 200, 0.40, 0.60),
            Period(200, 300, 0.75, 1.25),
        };
        var viewport = GraphScene.CreateViewport(0, 300, GraphMetric.Dollars, periods);
        var control = new GraphPlotControl { Scene = viewport };

        AssertAmountsFitFooter(control, width: 360, height: 260);
        AssertAmountsFitFooter(control, width: 480, height: 320);

        var clusteredPeriods = new[]
        {
            Period(0, 100, 1_234.5, 2_345.5),
            Period(100, 990, 2_345.5, 3_456.5),
            Period(990, 1_000, 3_456.5, 4_567.5),
        };
        var clusteredViewport = GraphScene.CreateViewport(0, 1_000, GraphMetric.Dollars, clusteredPeriods);
        var clusteredControl = new GraphPlotControl { Scene = clusteredViewport };
        AssertAmountsFitFooter(clusteredControl, width: 360, height: 260);
    }

    private static void AssertAmountsFitFooter(GraphPlotControl control, int width, int height)
    {
        using var rendered = control.Plot.GetImage(width, height);
        var panel = Panel(control);
        var footerRect = AssertRenderedRect(panel.LastRenderBounds);
        var amounts = panel.Amounts.OrderBy(amount => amount.CenterAt).ToArray();
        var periods = control.Scene.PeriodScenes.OrderBy(period => period.PeriodStartAt).ToArray();
        var previousRight = float.NegativeInfinity;

        Assert.Equal(3, amounts.Length);
        Assert.Equal(amounts.Length, periods.Length);
        foreach (var (amount, period) in amounts.Zip(periods))
        {
            var periodLeft = control.Plot.Axes.Bottom.GetPixel(period.PeriodStartAt, control.Plot.LastRender.DataRect);
            var periodRight = control.Plot.Axes.Bottom.GetPixel(period.PeriodEndAt, control.Plot.LastRender.DataRect);
            var inset = Math.Min(8, (periodRight - periodLeft) * 0.1f);
            var rect = AssertRenderedRect(amount.LastRenderBounds);
            var expectedCenter = control.Plot.Axes.Bottom.GetPixel(amount.CenterAt, control.Plot.LastRender.DataRect);

            Assert.InRange(amount.LabelStyle.FontSize, 0.001f, GraphPeriodCostPanel.BaseFontSize);
            Assert.True(rect.Left >= periodLeft + inset - 0.1f,
                $"left={rect.Left}, period=[{periodLeft}, {periodRight}], inset={inset}");
            Assert.True(rect.Right <= periodRight - inset + 0.1f,
                $"right={rect.Right}, period=[{periodLeft}, {periodRight}], inset={inset}");
            Assert.True(rect.Top >= footerRect.Top, $"top={rect.Top}, footer-top={footerRect.Top}");
            Assert.True(rect.Bottom <= footerRect.Bottom, $"bottom={rect.Bottom}, footer-bottom={footerRect.Bottom}");
            Assert.InRange(Math.Abs(amount.LastRenderCenterX!.Value - expectedCenter), 0, 0.1f);
            Assert.True(rect.Left >= previousRight, $"period amounts overlap at {rect.Left}");
            previousRight = rect.Right;
        }
    }

    private static GraphPeriodCostPanel Panel(GraphPlotControl control) =>
        Assert.Single(control.Plot.Axes.GetPanels().OfType<GraphPeriodCostPanel>());

    private static ScottPlot.PixelRect AssertRenderedRect(ScottPlot.PixelRect? rect) =>
        rect ?? throw new InvalidOperationException("The amount footer was not rendered.");

    private static void AssertGlyphsAvailable(SkiaSharp.SKTypeface typeface, string text)
    {
        var glyphs = typeface.GetGlyphs(text);
        Assert.Equal(text.Length, glyphs.Length);
        Assert.All(glyphs, glyph => Assert.NotEqual((ushort)0, glyph));
    }

    private static void AssertLabelPaintUsesFontWithGlyphs(
        GraphPeriodCostAmount amount,
        SkiaSharp.SKTypeface expectedTypeface,
        string text)
    {
        using var paint = ScottPlot.Paint.NewDisposablePaint();
        amount.LabelStyle.ApplyToPaint(paint);

        var renderedTypeface = paint.SKTypeface ??
            throw new InvalidOperationException("LabelStyle.ApplyToPaint did not choose a typeface.");
        Assert.Equal(expectedTypeface.FamilyName, renderedTypeface.FamilyName);
        AssertGlyphsAvailable(renderedTypeface, text);
    }

    private static GraphScene Period(
        long startAt,
        long endAt,
        double firstDollars,
        double secondDollars,
        long? resetAt = null)
    {
        var duration = endAt - startAt;
        var firstOffset = duration / 2;
        var secondOffset = Math.Min(duration - 1, Math.Max(firstOffset + 1, (duration * 4) / 5));
        return GraphScene.Create(
        [
            Sample(startAt + firstOffset, Model("gpt-6-sol", firstDollars / 2), Model("gpt-5.6-sol", secondDollars / 2)),
            Sample(startAt + secondOffset, Model("gpt-6-sol", firstDollars), Model("gpt-5.6-sol", secondDollars)),
        ], GraphMetric.Dollars, startAt, endAt,
            confirmedGaps: null,
            hiddenModelNames: null,
            accountOwnershipIntervals: null,
            resetAt: resetAt,
            publishedPeriodStartAt: startAt);
    }

    private static ApiHistorySample Sample(long timestamp, params ApiHistoryModelSample[] models) =>
        new(timestamp, 1_000, 99, null, null, null, null, null, null)
        {
            ModelsComplete = true,
            ModelSamples = models,
        };

    private static ApiHistoryModelSample Model(string name, double? dollars) =>
        new(name, 10, 2, 3, dollars)
        {
            CacheWriteInputTokens = 1,
            TotalTokens = 13,
        };
}
