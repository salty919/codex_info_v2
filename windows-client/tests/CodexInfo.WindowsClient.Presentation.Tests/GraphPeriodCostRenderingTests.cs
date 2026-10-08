// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
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
    public void EmbeddedPeriodCostFontAliasesResolveJapaneseAndKoreanGlyphs()
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

            AssertGlyphsAvailable(japanese, "期間合計 未取得");
            AssertGlyphsAvailable(korean, "기간 합계 가져오지 못함");

            var scene = GraphScene.Create(
                [Sample(1_000, Model("gpt-6-sol", 1.0))],
                GraphMetric.Dollars,
                1_000,
                1_060);
            LocalizationService.SetLanguage("ja");
            var japaneseLabel = Labels(new GraphPlotControl { Scene = scene }).Single();
            Assert.Equal(GraphPeriodCostFontResolver.JapaneseAlias, japaneseLabel.LabelFontName);
            AssertLabelPaintUsesFontWithGlyphs(japaneseLabel, japanese, "期間合計");

            LocalizationService.SetLanguage("ko");
            var koreanLabel = Labels(new GraphPlotControl { Scene = scene }).Single();
            Assert.Equal(GraphPeriodCostFontResolver.KoreanAlias, koreanLabel.LabelFontName);
            AssertLabelPaintUsesFontWithGlyphs(koreanLabel, korean, "기간 합계");
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void CurrentPeriodAddsItsPersistedDollarSummaryToThePlot()
    {
        var scene = GraphScene.Create(
        [
            Sample(1_000, Model("gpt-6-sol", 0.10), Model("gpt-5.6-sol", 0.20)),
            Sample(1_060, Model("gpt-6-sol", 0.375), Model("gpt-5.6-sol", 0.625)),
        ], GraphMetric.Dollars, 1_000, 2_000);
        var control = new GraphPlotControl { Scene = scene };

        Assert.Equal("期間合計 $1.00", Labels(control).Single().LabelText);
    }

    [Fact]
    public void PartialUnavailableAndConfirmedZeroHaveDistinctSummaryLabels()
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

            Assert.Equal("記録分 $1.25（未確定）", Labels(new GraphPlotControl { Scene = partial }).Single().LabelText);
            Assert.Equal("期間合計 未取得", Labels(new GraphPlotControl { Scene = unavailable }).Single().LabelText);
            Assert.Equal("期間合計 $0.00", Labels(new GraphPlotControl { Scene = zero }).Single().LabelText);
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

        Assert.Equal("期間合計 $2.00", Labels(visible).Single().LabelText);
        Assert.Equal("期間合計 $2.00", Labels(hidden).Single().LabelText);
        var hiddenModelLines = hidden.Plot.GetPlottables<ScottPlot.Plottables.Scatter>()
            .Where(series => series.LineWidth >= GraphPlotControl.MeasuredModelLineWidth);
        Assert.NotEmpty(hiddenModelLines);
        Assert.All(hiddenModelLines, series => Assert.False(series.IsVisible));
    }

    [Fact]
    public void SummaryUsesSelectedLocaleAndResolvedThemeColors()
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
            var label = Labels(new GraphPlotControl { Scene = scene }).Single();

            Assert.Equal("Period total $1.00", label.LabelText);
            Assert.Equal(
                new ScottPlot.Color(ThemePalette.Resolve(GraphPlotControl.PlotColorHex)),
                label.LabelBackgroundColor);
            Assert.Equal(
                new ScottPlot.Color(ThemePalette.Resolve(GraphPlotControl.AxisTextColorHex)),
                label.LabelFontColor);
        }
        finally
        {
            ThemePalette.Apply(originalTheme);
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void ViewportOmitsOnlyTheLeftClippedPeriodLabelAndKeepsTheRightPartialAnchor()
    {
        var first = Period(0, 100, 0.10, 0.20);
        var second = Period(100, 200, 0.40, 0.60);
        var third = Period(200, 300, 0.75, 1.25);
        var viewport = GraphScene.CreateViewport(50, 280, GraphMetric.Dollars, [first, second, third]);
        var control = new GraphPlotControl { Scene = viewport };

        var labels = Labels(control);
        Assert.Equal(2, labels.Length);
        Assert.Equal(
            [
                StartLabel(second.PeriodStartAt) + "\n期間合計 $1.00",
                StartLabel(third.PeriodStartAt) + "\n期間合計 $2.00",
            ],
            labels.Select(label => label.LabelText));
        Assert.Equal(280, labels[^1].Location.X);
        Assert.Contains(control.Plot.GetPlottables<ScottPlot.Plottables.Scatter>()
            .Where(series => series.LineWidth >= GraphPlotControl.MeasuredModelLineWidth)
            .SelectMany(series => series.Data.GetScatterPoints()),
            point => point.X == 50);
    }

    [Fact]
    public void FirstAndResizedNarrowRendersKeepThreeDatedPeriodLabelsInsideSeparateLanes()
    {
        var periods = new[]
        {
            Period(0, 100, 0.10, 0.20),
            Period(100, 200, 0.40, 0.60),
            Period(200, 300, 0.75, 1.25),
        };
        var viewport = GraphScene.CreateViewport(0, 300, GraphMetric.Dollars, periods);
        var control = new GraphPlotControl { Scene = viewport };
        var labels = Labels(control);

        Assert.Equal(3, labels.Length);
        Assert.Equal(
            periods.Select(period => StartLabel(period.PeriodStartAt) + "\n"),
            labels.Select(label => label.LabelText[..(label.LabelText.IndexOf('\n') + 1)]));

        AssertLabelsFitAndUseSeparateLanes(control, width: 360, height: 260);
        AssertLabelsFitAndUseSeparateLanes(control, width: 480, height: 320);
    }

    private static void AssertLabelsFitAndUseSeparateLanes(GraphPlotControl control, int width, int height)
    {
        using var rendered = control.Plot.GetImage(width, height);
        var dataRect = control.Plot.LastRender.DataRect;
        var labels = Labels(control).OrderBy(label => label.Location.X).ToArray();
        var previousBottom = float.NegativeInfinity;

        foreach (var label in labels)
        {
            // LabelLastRenderPixelRect is written inside Text.Render after
            // layout runs, so this checks the box actually painted on the
            // first raster rather than offsets mutated later in the frame.
            var drawnRect = label.LabelLastRenderPixelRect;
            Assert.True(drawnRect.Left >= dataRect.Left + 3,
                $"left={drawnRect.Left}, data-left={dataRect.Left}, text={label.LabelText}");
            Assert.True(drawnRect.Right <= dataRect.Right - 3,
                $"right={drawnRect.Right}, data-right={dataRect.Right}, text={label.LabelText}");
            Assert.True(drawnRect.Top >= dataRect.Top + 3,
                $"top={drawnRect.Top}, data-top={dataRect.Top}, text={label.LabelText}");
            Assert.True(drawnRect.Bottom <= dataRect.Bottom - 2,
                $"bottom={drawnRect.Bottom}, data-bottom={dataRect.Bottom}, text={label.LabelText}");
            Assert.True(drawnRect.Top >= previousBottom + 2,
                $"period labels overlap vertically at {drawnRect.Top}");
            previousBottom = drawnRect.Bottom;
        }
    }

    private static ScottPlot.Plottables.Text[] Labels(GraphPlotControl control) =>
        control.Plot.GetPlottables<ScottPlot.Plottables.Text>().ToArray();

    private static void AssertGlyphsAvailable(SkiaSharp.SKTypeface typeface, string text)
    {
        var glyphs = typeface.GetGlyphs(text);
        Assert.Equal(text.Length, glyphs.Length);
        Assert.All(glyphs, glyph => Assert.NotEqual((ushort)0, glyph));
    }

    private static void AssertLabelPaintUsesFontWithGlyphs(
        ScottPlot.Plottables.Text label,
        SkiaSharp.SKTypeface expectedTypeface,
        string text)
    {
        using var paint = ScottPlot.Paint.NewDisposablePaint();
        label.LabelStyle.ApplyToPaint(paint);

        var renderedTypeface = paint.SKTypeface ??
            throw new InvalidOperationException("LabelStyle.ApplyToPaint did not choose a typeface.");
        Assert.Equal(expectedTypeface.FamilyName, renderedTypeface.FamilyName);
        AssertGlyphsAvailable(renderedTypeface, text);
    }

    private static string StartLabel(long periodStartAt) =>
        TimeZoneInfo.ConvertTime(
                DateTimeOffset.FromUnixTimeSeconds(periodStartAt),
                LocalizationService.DisplayTimeZone)
            .ToString("MM/dd HH:mm", CultureInfo.CurrentCulture) + "～";

    private static GraphScene Period(long startAt, long endAt, double firstDollars, double secondDollars) =>
        GraphScene.Create(
        [
            Sample(startAt + 50, Model("gpt-6-sol", firstDollars / 2), Model("gpt-5.6-sol", secondDollars / 2)),
            Sample(startAt + 80, Model("gpt-6-sol", firstDollars), Model("gpt-5.6-sol", secondDollars)),
        ], GraphMetric.Dollars, startAt, endAt);

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
