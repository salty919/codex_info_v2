// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Xml.Linq;
using Avalonia.Automation;
using Avalonia.Automation.Peers;
using Avalonia.Controls;
using SkiaSharp;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphCalendarAutomationTests
{
    [Theory]
    [InlineData("AutomationProperties.AutomationId", "Graph.Calendar")]
    [InlineData("AutomationProperties.Name", "{Binding AccessibleText}")]
    public void CalendarAutomationMetadataBelongsToAnExposedControl(string attribute, string value)
    {
        var document = ReadGraph();
        var element = Assert.Single(document.Descendants(), node => node.Attribute(attribute)?.Value == value);
        var type = typeof(Control).Assembly.GetType("Avalonia.Controls." + element.Name.LocalName);
        Assert.NotNull(type);
        var control = Assert.IsAssignableFrom<Control>(Activator.CreateInstance(type));
        if (element.Attribute("AutomationProperties.AccessibilityView") is { } view)
            AutomationProperties.SetAccessibilityView(control, Enum.Parse<AccessibilityView>(view.Value));
        const string exact = "2026/10/01 3,000,000 Tokens Recorded part";
        AutomationProperties.SetName(control, exact);
        var peer = ControlAutomationPeer.CreatePeerForElement(control);
        Assert.NotNull(peer);
        Assert.True(peer.IsControlElement(), "Calendar metadata must be in the UIA control view.");
        Assert.Equal(exact, peer.GetName());
    }

    [Fact]
    public void CalendarStatusWrapsWithinNarrowCellsWithoutShrinkingText()
    {
        var status = Assert.Single(ReadGraph().Descendants(), node =>
            node.Attribute("Text")?.Value == "{Binding StatusText}" && node.Name.LocalName == "TextBlock");
        Assert.Equal("Wrap", status.Attribute("TextWrapping")?.Value);
        Assert.Equal("12", status.Attribute("FontSize")?.Value);
    }

    [Fact]
    public void CalendarStatusWordsFitTheMinimumWidthCell()
    {
        var document = ReadGraph();
        var cell = Assert.Single(document.Descendants(), node => node.Attribute("Classes")?.Value == "calendar-day");
        var padding = Avalonia.Thickness.Parse(cell.Attribute("Padding")!.Value);
        using var typeface = SKTypeface.FromFile(Path.Combine(RepositoryRoot(),
            "windows-client/src/CodexInfo.WindowsClient/Assets/NotoSansJP-Medium.ttf"));
        Assert.NotNull(typeface);
        using var font = new SKFont(typeface, 12);
        // 700-DIP window: 20-DIP outer margins, 10-DIP calendar margins,
        // frame border, seven columns, cell margins, and the today border.
        var textWidth = (700d - 40d - 20d - 2d) / 7d - 6d - 4d - padding.Left - padding.Right;
        foreach (var word in new[] { "Unmeasured", "Recorded", "part", "未計測", "記録分" })
            Assert.True(font.MeasureText(word) <= textWidth,
                $"Calendar word '{word}' needs {font.MeasureText(word):F2} DIP; cell has {textWidth:F2} DIP.");
    }

    private static XDocument ReadGraph() => XDocument.Load(Path.Combine(RepositoryRoot(),
        "windows-client/src/CodexInfo.WindowsClient/GraphWindow.axaml"));

    private static string RepositoryRoot()
    {
        var root = new DirectoryInfo(AppContext.BaseDirectory);
        while (root is not null && !File.Exists(Path.Combine(root.FullName,
                   "windows-client/src/CodexInfo.WindowsClient/GraphWindow.axaml")))
            root = root.Parent;
        Assert.NotNull(root);
        return root.FullName;
    }
}
