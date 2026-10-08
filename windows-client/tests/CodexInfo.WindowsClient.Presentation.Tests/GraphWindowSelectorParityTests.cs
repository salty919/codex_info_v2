// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Xml.Linq;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphWindowSelectorParityTests
{
    [Fact]
    public void GraphTimeWindowsExposeModesAndBoundedNavigation()
    {
        var graph = XDocument.Parse(Load("GraphWindow.axaml"));
        foreach (var id in new[] { "Graph.Range.Period", "Graph.Range.Day", "Graph.Range.Week",
                     "Graph.Range.Previous", "Graph.Range.Next", "Graph.Range.Label" })
        {
            Assert.Single(graph.Descendants(), element =>
                element.Attribute("AutomationProperties.AutomationId")?.Value == id);
        }
        foreach (var (id, enabled) in new[] { ("Previous", "CanGoBack"), ("Next", "CanGoForward") })
        {
            var button = graph.Descendants().Single(element =>
                element.Attribute("AutomationProperties.AutomationId")?.Value == $"Graph.Range.{id}");
            Assert.Equal($"{{Binding {enabled}}}", button.Attribute("IsEnabled")?.Value);
        }
        var plot = graph.Descendants().Single(element => element.Name.LocalName == "GraphPlotControl");
        Assert.Equal("{Binding HasPlot}", plot.Attribute("IsVisible")?.Value);
    }

    [Fact]
    public void GraphHeaderUsesEveryFixedRowForVisibleControls()
    {
        var graph = XDocument.Parse(Load("GraphWindow.axaml"));
        var content = graph.Descendants().Single(element =>
            element.Attribute(XName.Get("Name", "http://schemas.microsoft.com/winfx/2006/xaml"))?.Value == "GraphContent");
        var rows = content.Attribute("RowDefinitions")!.Value.Split(',');
        Assert.Equal(4, rows.Length);
        for (var row = 0; row < rows.Length - 1; row++)
        {
            Assert.Contains(content.Elements(), element =>
                (element.Attribute("Grid.Row")?.Value ?? "0") == row.ToString() &&
                (element.Name.LocalName != "Border" || element.HasElements));
        }
    }

    [Fact]
    public void GraphPlotFrameDrawsRoundedOutlineAfterClippedContent()
    {
        var graph = XDocument.Parse(Load("GraphWindow.axaml"));
        var xamlName = XName.Get("Name", "http://schemas.microsoft.com/winfx/2006/xaml");
        var frame = graph.Descendants().Single(element =>
            element.Attribute(xamlName)?.Value == "GraphFrame");
        Assert.Equal("Grid", frame.Name.LocalName);
        Assert.Equal("3", frame.Attribute("Grid.Row")?.Value);

        var children = frame.Elements().ToArray();
        Assert.Equal(2, children.Length);
        var content = children[0];
        Assert.Equal("Border", content.Name.LocalName);
        Assert.Equal("1", content.Attribute("BorderThickness")?.Value);
        Assert.Equal("10", content.Attribute("CornerRadius")?.Value);
        Assert.Equal("True", content.Attribute("ClipToBounds")?.Value);

        var outline = graph.Descendants().Single(element =>
            element.Attribute(xamlName)?.Value == "GraphFrameOutline");
        Assert.Same(children[^1], outline);
        Assert.Equal("Border", outline.Name.LocalName);
        Assert.Equal("{DynamicResource Theme3B506F}", outline.Attribute("BorderBrush")?.Value);
        Assert.Equal("1", outline.Attribute("BorderThickness")?.Value);
        Assert.Equal("10", outline.Attribute("CornerRadius")?.Value);
        Assert.Equal("False", outline.Attribute("IsHitTestVisible")?.Value);
    }

    [Fact]
    public void GraphPeriodSelectorUsesOneAnchoredPopupComponent()
    {
        var graph = XDocument.Parse(Load("GraphWindow.axaml"));
        var xamlName = XName.Get("Name", "http://schemas.microsoft.com/winfx/2006/xaml");
        var selector = graph.Descendants().Single(element =>
            element.Name.LocalName == "GraphSelect" &&
            element.Attribute(xamlName)?.Value == "PeriodSelector");
        Assert.Equal("Graph.PeriodSelector", selector.Attribute("AutomationProperties.AutomationId")?.Value);
        Assert.Null(selector.Attribute("SelectorAutomationId"));
        Assert.DoesNotContain(graph.Descendants(), element =>
            element.Name.LocalName == "GraphSelect" &&
            element.Attribute(xamlName)?.Value == "MetricSelector");
        Assert.DoesNotContain(graph.Descendants(), element =>
            element.Attribute(xamlName)?.Value == "MetricMenu");

        var fieldStyle = graph.Descendants().Single(element =>
            element.Name.LocalName == "Style" &&
            element.Attribute("Selector")?.Value == "controls|GraphSelect.graph-select-field");
        Assert.Equal("{DynamicResource Theme111B2C}", fieldStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Background")
            .Attribute("Value")?.Value);
        Assert.Equal("{DynamicResource Theme405779}", fieldStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "BorderBrush")
            .Attribute("Value")?.Value);

        var openFieldStyle = graph.Descendants().Single(element =>
            element.Name.LocalName == "Style" &&
            element.Attribute("Selector")?.Value == "controls|GraphSelect.graph-select-field:checked");
        Assert.Equal("{DynamicResource Theme244D74}", openFieldStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Background")
            .Attribute("Value")?.Value);
        Assert.Equal("{DynamicResource Theme56B2F5}", openFieldStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "BorderBrush")
            .Attribute("Value")?.Value);

        Assert.DoesNotContain(graph.Descendants(), element =>
            element.Attribute(xamlName)?.Value == "AccountSelector");

        Assert.DoesNotContain(graph.Descendants(), element =>
            element.Attribute(xamlName)?.Value
                is "AccountMenu" or "PeriodMenu" or "MetricMenu");

        var field = XDocument.Parse(Load("Controls/GraphSelect.axaml"));
        Assert.Equal("ToggleButton", field.Root?.Name.LocalName);
        Assert.Equal("CodexInfo.WindowsClient.Controls.GraphSelect",
            field.Root?.Attribute(XName.Get("Class", xamlName.NamespaceName))?.Value);
        Assert.DoesNotContain(field.Root!.Descendants(), element => element.Name.LocalName == "ToggleButton");

        var presenter = field.Root!.Elements().Single(element => element.Name.LocalName == "ToggleButton.Template").Descendants().Single(element => element.Name.LocalName == "ContentPresenter");
        Assert.Equal("Stretch", presenter.Attribute("HorizontalAlignment")?.Value);
        Assert.Equal("Stretch", presenter.Attribute("VerticalAlignment")?.Value);

        var popup = field.Descendants().Single(element => element.Name.LocalName == "Popup");
        Assert.Single(field.Descendants(), element => element.Name.LocalName == "Popup");
        Assert.Equal("True", popup.Attribute("ShouldUseOverlayLayer")?.Value);
        Assert.Equal("True", popup.Attribute("IsLightDismissEnabled")?.Value);
        Assert.Contains(field.Descendants(), element =>
            element.Name.LocalName == "Setter" &&
            element.Attribute("Property")?.Value == "Height" &&
            element.Attribute("Value")?.Value == "32");
    }

    private static string Load(string relativePath)
    {
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory);
             directory is not null;
             directory = directory.Parent)
        {
            var candidate = Path.Combine(directory.FullName,
                "windows-client", "src", "CodexInfo.WindowsClient", relativePath);
            if (File.Exists(candidate))
            {
                return File.ReadAllText(candidate);
            }
        }

        throw new FileNotFoundException(relativePath);
    }
}
