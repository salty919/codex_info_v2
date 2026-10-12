// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using Avalonia.Automation;
using Avalonia.Controls;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphUsageFormattingTests
{
    [Theory]
    [InlineData(731_821_429, 225_301_669, "732 M", "957 M")]
    [InlineData(999_999, 0, "999,999", "999,999")]
    [InlineData(1_500_000, 0, "2 M", "2 M")]
    [InlineData(1_000_000_000, 0, "1,000 M", "1,000 M")]
    public void RenderedAxisAndPeriodSummaryUseReadableWholeCounts(
        ulong sol, ulong astra, string topLabel, string totalLabel)
    {
        var previous = CultureInfo.CurrentCulture;
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
        try
        {
            var scene = Scene(sol, astra);
            var control = new GraphPlotControl { Scene = scene };
            using var image = control.Plot.GetImage(700, 480);
            var ticks = control.Plot.Axes.Left.TickGenerator.Ticks;
            Assert.Equal(topLabel, ticks.MaxBy(tick => tick.Position).Label);
            var labels = ticks.Select(tick => tick.Label).Where(label => !string.IsNullOrEmpty(label)).ToArray();
            Assert.Equal(labels.Length, labels.Distinct().Count());
            Assert.All(labels, label => Assert.DoesNotContain(".", label));
            var panel = Assert.Single(control.Plot.Axes.GetPanels().OfType<GraphPeriodCostPanel>());
            var amount = Assert.Single(panel.Amounts);
            Assert.Equal(totalLabel, amount.LabelStyle.Text);
            Assert.Equal(GraphPeriodCostPanel.BaseFontSize, amount.LabelStyle.FontSize);
            Assert.Equal((decimal)sol + astra, scene.RecordedPeriodTokens);
        }
        finally
        {
            CultureInfo.CurrentCulture = previous;
        }
    }

    [Fact]
    public void RoundedAxisLabelsDoNotDescribeDifferentTicksWithTheSameNumber()
    {
        var control = new GraphPlotControl { Scene = Scene(2_000_000, 0) };
        using var image = control.Plot.GetImage(700, 480);
        var labels = control.Plot.Axes.Left.TickGenerator.Ticks
            .OrderBy(tick => tick.Position).Select(tick => tick.Label).ToArray();
        Assert.Equal(["0", 500_000.ToString("N0", CultureInfo.CurrentCulture), "1 M", "", "2 M"], labels);
    }

    [Fact]
    public void HoverRetainsExactLargeTokenCountsAndAccessibleText()
    {
        var control = new GraphPlotControl { Scene = Scene(731_821_429, 225_301_669) };
        using var image = control.Plot.GetImage(940, 480);
        var data = control.Plot.LastRender.DataRect;
        var x = control.Plot.GetPixel(new ScottPlot.Coordinates(1_000, 0),
            control.Plot.Axes.Bottom, control.Plot.Axes.Left).X;
        control.UpdateHoverAt(new Avalonia.Point(x, (data.Top + data.Bottom) / 2));
        var tooltip = Assert.IsType<StackPanel>(ToolTip.GetTip(control));
        var row = Assert.Single(tooltip.Children.OfType<TextBlock>(),
            text => text.GetValue(AutomationProperties.AutomationIdProperty) == "Graph.Hover.SOL");
        Assert.Equal($"SOL: {731_821_429.ToString("N0", CultureInfo.CurrentCulture)} {LocalizationService.Current.Tokens}", row.Text);
        Assert.Equal(row.Text, row.GetValue(AutomationProperties.NameProperty));
    }

    private static GraphScene Scene(ulong sol, ulong astra) => GraphScene.Create(
    [
        Sample(1_000, sol, astra),
        Sample(1_060, sol, astra),
    ], GraphMetric.Tokens, 1_000, 1_060);

    private static ApiHistorySample Sample(long timestamp, ulong sol, ulong astra) =>
        new(timestamp, 2_000, null, null, null, null, null, null, null)
        {
            ModelsComplete = true,
            ModelSamples =
            [
                new("gpt-6-sol", sol, 0, 0, 1) { TotalTokens = sol },
                new("gpt-6-astra", astra, 0, 0, 1) { TotalTokens = astra },
            ],
        };
}
