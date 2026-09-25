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
        foreach (var id in new[] { "Graph.AccountSelector", "Graph.PeriodSelector", "Graph.MetricSelector" })
        {
            var selector = graph.Descendants().Single(element =>
                element.Attribute("AutomationProperties.AutomationId")?.Value == id);
            Assert.Equal("GraphSelect", selector.Name.LocalName);
        }

        Assert.DoesNotContain(graph.Descendants(), element =>
            element.Attribute(XName.Get("Name", "http://schemas.microsoft.com/winfx/2006/xaml"))?.Value
                is "AccountMenu" or "PeriodMenu" or "MetricMenu");

        var field = XDocument.Parse(Load("Controls/GraphSelect.axaml"));
        Assert.Contains(field.Descendants(), element => element.Name.LocalName == "Popup");
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
