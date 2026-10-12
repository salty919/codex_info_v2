// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.Xml.Linq;
using Avalonia;
using Avalonia.Controls;
using SkiaSharp;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class MainQuotaLayoutTests
{
    [Theory]
    [InlineData("Estimated dollars $7.20")]
    [InlineData("Estimated dollars $151.00")]
    [InlineData("概算 $151.00")]
    public void RecordedQuotaAndCostFitWithoutClipping(string cost)
    {
        var root = new DirectoryInfo(AppContext.BaseDirectory);
        while (root is not null && !File.Exists(Path.Combine(root.FullName,
                   "windows-client/src/CodexInfo.WindowsClient/MainWindow.axaml")))
            root = root.Parent;
        Assert.NotNull(root);
        var document = XDocument.Load(Path.Combine(root.FullName,
            "windows-client/src/CodexInfo.WindowsClient/MainWindow.axaml"));
        var amount = Assert.Single(document.Descendants(), node => node.Attribute("Text")?.Value == "{Binding EstimatedCostText}");
        var quota = Assert.Single(document.Descendants(), node => node.Attribute("Text")?.Value == "{Binding RemainingPercentText}");
        var layout = amount.Parent!.Parent!;
        using var typeface = SKTypeface.FromFile(Path.Combine(root.FullName,
            "windows-client/src/CodexInfo.WindowsClient/Assets/NotoSansJP-Medium.ttf"));
        Assert.NotNull(typeface);
        using var costFont = new SKFont(typeface, float.Parse(amount.Attribute("FontSize")!.Value, CultureInfo.InvariantCulture));
        using var quotaFont = new SKFont(typeface, float.Parse(quota.Attribute("FontSize")!.Value, CultureInfo.InvariantCulture));
        var costWidth = Math.Ceiling(costFont.MeasureText(cost));
        var quotaWidth = Math.Ceiling(quotaFont.MeasureText("100%"));
        var grid = new Grid
        {
            ColumnDefinitions = ColumnDefinitions.Parse(layout.Attribute("ColumnDefinitions")!.Value),
            ColumnSpacing = double.Parse(layout.Attribute("ColumnSpacing")!.Value, CultureInfo.InvariantCulture),
        };
        var quotaControl = new MeasuredText(quotaWidth);
        if (quota.Attribute("Width") is { } fixedWidth)
            quotaControl.Width = double.Parse(fixedWidth.Value, CultureInfo.InvariantCulture);
        var costControl = new MeasuredText(costWidth);
        Grid.SetColumn(costControl, 1);
        grid.Children.Add(quotaControl);
        grid.Children.Add(costControl);
        grid.Measure(new Size(828, 60));
        grid.Arrange(new Rect(0, 0, 828, 60));

        Assert.True(grid.ColumnDefinitions[1].ActualWidth >= costWidth,
            $"'{cost}' requires {costWidth} DIP; cost column has {grid.ColumnDefinitions[1].ActualWidth} DIP.");
        Assert.True(grid.ColumnDefinitions[0].ActualWidth >= quotaWidth);
        Assert.True(costControl.Bounds.Right <= 828);
        Assert.True(quotaControl.Bounds.Right <= costControl.Bounds.Left);
    }

    private sealed class MeasuredText(double width) : Control
    {
        protected override Size MeasureOverride(Size availableSize) => new(width, 44);
    }
}
