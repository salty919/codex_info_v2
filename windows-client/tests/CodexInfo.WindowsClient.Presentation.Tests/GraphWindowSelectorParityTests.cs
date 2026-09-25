// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Xml.Linq;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphWindowSelectorParityTests
{
    [Fact]
    public void GraphSelectorsUseOneAnchoredPopupComponent()
    {
        var graph = XDocument.Parse(Load("GraphWindow.axaml"));
        var selectors = new[]
        {
            (Name: "AccountSelector", Id: "Graph.AccountSelector"),
            (Name: "PeriodSelector", Id: "Graph.PeriodSelector"),
            (Name: "MetricSelector", Id: "Graph.MetricSelector")
        };
        var xamlName = XName.Get("Name", "http://schemas.microsoft.com/winfx/2006/xaml");
        foreach (var (name, id) in selectors)
        {
            var selector = graph.Descendants().Single(element =>
                element.Name.LocalName == "GraphSelect" && element.Attribute(xamlName)?.Value == name);
            Assert.Equal(id, selector.Attribute("AutomationProperties.AutomationId")?.Value);
            Assert.Null(selector.Attribute("SelectorAutomationId"));
        }

        var fieldStyle = graph.Descendants().Single(element =>
            element.Name.LocalName == "Style" &&
            element.Attribute("Selector")?.Value == "controls|GraphSelect.graph-select-field");
        Assert.Equal("#111B2C", fieldStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Background")
            .Attribute("Value")?.Value);
        Assert.Equal("#405779", fieldStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "BorderBrush")
            .Attribute("Value")?.Value);

        var openFieldStyle = graph.Descendants().Single(element =>
            element.Name.LocalName == "Style" &&
            element.Attribute("Selector")?.Value == "controls|GraphSelect.graph-select-field:checked");
        Assert.Equal("#244D74", openFieldStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Background")
            .Attribute("Value")?.Value);
        Assert.Equal("#56B2F5", openFieldStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "BorderBrush")
            .Attribute("Value")?.Value);

        var accountSelector = graph.Descendants().Single(element =>
            element.Name.LocalName == "GraphSelect" && element.Attribute(xamlName)?.Value == "AccountSelector");
        var accountOption = accountSelector.Descendants().Single(element =>
            element.Name.LocalName == "TextBlock" &&
            element.Attribute("AutomationProperties.Name") is not null);
        foreach (var attribute in new[] { "Text", "ToolTip.Tip", "AutomationProperties.Name", "AutomationProperties.HelpText" })
        {
            Assert.Equal("{Binding MainDisplayLabel}", accountOption.Attribute(attribute)?.Value);
        }

        Assert.DoesNotContain(graph.Descendants(), element =>
            element.Attribute(xamlName)?.Value
                is "AccountMenu" or "PeriodMenu" or "MetricMenu");

        var field = XDocument.Parse(Load("Controls/GraphSelect.axaml"));
        Assert.Equal("ToggleButton", field.Root?.Name.LocalName);
        Assert.Equal("CodexInfo.WindowsClient.Controls.GraphSelect",
            field.Root?.Attribute(XName.Get("Class", xamlName.NamespaceName))?.Value);
        Assert.DoesNotContain(field.Root!.Descendants(), element => element.Name.LocalName == "ToggleButton");

        var presenter = field.Descendants().Single(element => element.Name.LocalName == "ContentPresenter");
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
