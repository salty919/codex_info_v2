// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections;
using System.Reflection;
using System.Xml.Linq;
using Avalonia;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class ThreadsWindowLayoutTests
{
    private static readonly XNamespace Avalonia = "https://github.com/avaloniaui";

    [Fact]
    public void ThreadsInformationRowsUseTheIssue362Contract()
    {
        const int expectedWindowWidth = 900;
        const int expectedWindowHeight = 480;
        const int expectedRowHeight = 96;
        const int expectedVisibleRows = 4;
        const int expectedListWidth = 860;
        const int expectedCardLeft = 80;
        const int expectedCardRight = 16;
        const int expectedCardWidth = expectedListWidth - expectedCardLeft - expectedCardRight;
        const int expectedColumnGap = 12;
        const int expectedTitleWidth = 350;
        const int expectedModelWidth = 190;
        const int expectedTimeWidth = 198;
        Assert.Equal(expectedCardWidth - 2,
            expectedTitleWidth + expectedModelWidth + expectedTimeWidth + (2 * expectedColumnGap));

        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client",
            "src",
            "CodexInfo.WindowsClient",
            "ThreadsWindow.axaml"));
        var window = Assert.IsType<XElement>(document.Root);
        Assert.Equal(expectedWindowWidth.ToString(), window.Attribute("Width")?.Value);
        Assert.Equal(expectedWindowHeight.ToString(), window.Attribute("Height")?.Value);
        Assert.Equal(expectedWindowWidth.ToString(), window.Attribute("MinWidth")?.Value);
        Assert.Equal(expectedWindowHeight.ToString(), window.Attribute("MinHeight")?.Value);
        Assert.Equal(expectedWindowWidth.ToString(), window.Attribute("MaxWidth")?.Value);
        Assert.Equal(expectedWindowHeight.ToString(), window.Attribute("MaxHeight")?.Value);

        var surface = Assert.Single(window.Elements(Avalonia + "Grid"));
        Assert.Equal("30,14,384", surface.Attribute("RowDefinitions")?.Value);
        Assert.Equal("6", surface.Attribute("RowSpacing")?.Value);
        var viewport = Assert.Single(document.Descendants(Avalonia + "ScrollViewer"));
        var viewportHeight = int.Parse(Assert.IsType<XAttribute>(viewport.Attribute("Height")).Value);
        Assert.Equal(expectedRowHeight * expectedVisibleRows, viewportHeight);

        var cardStyle = Assert.Single(document.Descendants(Avalonia + "Style"),
            element => element.Attribute("Selector")?.Value == "Border.thread-card");
        var cardHeight = int.Parse(StyleSetter(cardStyle, "Height"));
        var card = Assert.Single(document.Descendants(Avalonia + "Border"),
            element => element.Attribute("Classes")?.Value == "thread-card");
        var cardMargins = Assert.IsType<XAttribute>(card.Attribute("Margin")).Value
            .Split(',')
            .Select(int.Parse)
            .ToArray();
        Assert.Equal([expectedCardLeft, 6, expectedCardRight, 6], cardMargins);
        Assert.Equal(expectedRowHeight, cardHeight + cardMargins[1] + cardMargins[3]);
        Assert.Equal(expectedVisibleRows, viewportHeight / (cardHeight + cardMargins[1] + cardMargins[3]));
        Assert.Equal("14,8", StyleSetter(cardStyle, "Padding"));

        var row = Assert.Single(card.Elements(Avalonia + "Grid"));
        Assert.Equal("*,180,208", row.Attribute("ColumnDefinitions")?.Value);
        Assert.Equal("0", row.Attribute("Margin")?.Value);
        Assert.Equal(expectedColumnGap.ToString(), row.Attribute("ColumnSpacing")?.Value);
        var treeControl = Assert.Single(document.Descendants(
            XName.Get("ThreadTreeControl", "using:CodexInfo.WindowsClient.Controls")));
        Assert.Equal(expectedListWidth.ToString(), treeControl.Attribute("Width")?.Value);
        Assert.Equal("Left", treeControl.Attribute("HorizontalAlignment")?.Value);

        var title = BoundText(row, "{Binding Title}");
        Assert.Equal("0", title.Attribute("Grid.Column")?.Value);
        Assert.Equal("Wrap", title.Attribute("TextWrapping")?.Value);
        Assert.Equal("2", title.Attribute("MaxLines")?.Value);
        Assert.Null(title.Attribute("TextTrimming"));
        Assert.Equal("{Binding Title}", title.Attribute("AutomationProperties.Name")?.Value);
        Assert.Equal("{Binding Title}", title.Attribute("AutomationProperties.HelpText")?.Value);
        Assert.Equal("{Binding Title}", title.Attribute("ToolTip.Tip")?.Value);
        Assert.Equal("{Binding Title}", card.Attribute("AutomationProperties.Name")?.Value);

        Assert.Null(card.Elements().SingleOrDefault(element => element.Name.LocalName == "Border.RenderTransform"));
        Assert.DoesNotContain(document.Descendants(Avalonia + "TextBlock"),
            element => element.Attribute("Text")?.Value == "{Binding RoleStatusText}");
        var rootMarker = Assert.Single(row.Descendants(Avalonia + "Border"),
            element => element.Attribute("Classes")?.Value == "thread-root-marker");
        Assert.Equal("{Binding IsRootThread}", rootMarker.Attribute("IsVisible")?.Value);
        var model = BoundText(row, "{Binding ModelText}");
        Assert.Equal("{Binding ModelAccentHex}", model.Attribute("Foreground")?.Value);
        var modelAccent = Assert.Single(row.Descendants(Avalonia + "Border"),
            element => element.Attribute("Classes")?.Value == "model-accent");
        Assert.Equal("{Binding ModelAccentHex}", modelAccent.Attribute("Background")?.Value);
        var context = BoundText(row, "{Binding ContextUsageText}");
        Assert.Equal("1", model.Parent?.Attribute("Grid.Column")?.Value);
        Assert.Same(model.Parent, context.Parent);
        Assert.Equal("{Binding HasContextUsage}", context.Attribute("IsVisible")?.Value);
        Assert.Equal("Wrap", context.Attribute("TextWrapping")?.Value);
        Assert.Equal("2", context.Attribute("MaxLines")?.Value);
        Assert.Null(context.Attribute("TextTrimming"));
        Assert.Equal("{Binding ContextUsageText}", context.Attribute("AutomationProperties.Name")?.Value);
        Assert.Equal("{Binding ContextUsageText}", context.Attribute("ToolTip.Tip")?.Value);
        var elapsed = BoundText(row, "{Binding ElapsedMinutesText}");
        Assert.Equal("2", elapsed.Parent?.Attribute("Grid.Column")?.Value);
        Assert.Equal("{Binding HasElapsedMinutes}", elapsed.Attribute("IsVisible")?.Value);
        Assert.Equal("{Binding HasInstructionMinutes}",
            BoundText(row, "{Binding InstructionMinutesText}").Attribute("IsVisible")?.Value);
        Assert.Equal("{Binding HasDisplayToken}",
            BoundText(row, "{Binding DisplayTokenText}").Attribute("IsVisible")?.Value);

        var forbiddenBindings = new[]
        {
            "{Binding ParentText}",
            "{Binding DepthText}",
            "{Binding AgeText}",
            "{Binding InstructionAgeText}",
        };
        Assert.DoesNotContain(document.Descendants(Avalonia + "TextBlock"), element =>
            forbiddenBindings.Contains(element.Attribute("Text")?.Value, StringComparer.Ordinal));

        var now = 2_000_000L;
        var child = new ApiThreadDetails(
            "child",
            "A complete title that remains available to UI Automation",
            "parent",
            "gpt-luna",
            "LUNA",
            600,
            50,
            200,
            now - (125 * 60),
            now - (7 * 60),
            true,
            2,
            false);
        var texts = LocalizationService.Current;
        var roleText = InvokeFormatter("FormatRoleStatus", texts, child);
        Assert.Contains(texts.SubThread, roleText, StringComparison.Ordinal);
        Assert.Contains(ActiveLabel(texts), roleText, StringComparison.Ordinal);
        Assert.DoesNotContain("D2", roleText, StringComparison.Ordinal);

        var contextText = InvokeFormatter("FormatContextUsage", texts, child);
        var contextLines = contextText.Split('\n');
        Assert.Equal(2, contextLines.Length);
        Assert.Contains(texts.Context, contextLines[0], StringComparison.Ordinal);
        Assert.Contains("25", contextLines[0], StringComparison.Ordinal);
        Assert.Contains("%", contextLines[0], StringComparison.Ordinal);
        Assert.Contains("50", contextLines[1], StringComparison.Ordinal);
        Assert.Contains("200", contextLines[1], StringComparison.Ordinal);
        Assert.Contains(texts.Tokens, contextLines[1], StringComparison.Ordinal);

        var elapsedText = InvokeFormatter("FormatMinuteAge", texts, child.CreatedAt, texts.Elapsed, now);
        var instruction = InvokeFormatter("FormatMinuteAge", texts, child.LastUserMessageAt, texts.Instruction, now);
        Assert.Contains("125", elapsedText, StringComparison.Ordinal);
        Assert.Contains("7", instruction, StringComparison.Ordinal);
        Assert.Equal(string.Empty, InvokeFormatter("FormatMinuteAge", texts, null, texts.Instruction, now));

        var missing = child with
        {
            CumulativeTokens = null,
            ContextTokens = null,
            ContextLimit = null,
            LastUserMessageAt = null,
        };
        Assert.Equal(string.Empty, InvokeFormatter("FormatContextUsage", texts, missing));
        Assert.Equal(string.Empty, InvokeFormatter("FormatDisplayToken", texts, missing));
    }

    [Fact]
    public void ThreadTreeConnectionsUseShortArrowsAndRouteOnlyDistantChildren()
    {
        const double treeSurfaceWidth = 860;
        const double cardLeft = 80;
        const double cardAnchorX = cardLeft + 16;
        var direct = ThreadTreeControl.BuildGeometry(
            treeSurfaceWidth,
            192,
            [new ThreadTreeConnection(0, 1, 0)]);
        Assert.Contains(direct.Segments, segment =>
            segment.Start == new Point(cardAnchorX, 90) &&
            segment.End == new Point(cardAnchorX, 102));
        Assert.Contains(direct.Segments, segment =>
            segment.Start == new Point(cardAnchorX, 102) &&
            segment.End == new Point(cardAnchorX - 4, 97));
        Assert.Contains(direct.Segments, segment =>
            segment.Start == new Point(cardAnchorX, 102) &&
            segment.End == new Point(cardAnchorX + 4, 97));
        Assert.Null(direct.JunctionDot);

        var nested = ThreadTreeControl.BuildGeometry(
            treeSurfaceWidth,
            288,
            [new ThreadTreeConnection(0, 1, 0), new ThreadTreeConnection(1, 2, 1)]);
        Assert.Contains(nested.Segments, segment =>
            segment.Start == new Point(cardAnchorX, 186) &&
            segment.End == new Point(cardAnchorX, 198));

        var distant = ThreadTreeControl.BuildGeometry(
            treeSurfaceWidth,
            384,
            [new ThreadTreeConnection(0, 3, 0)]);
        Assert.Contains(distant.Segments, segment =>
            segment.Start == new Point(cardAnchorX, 90) &&
            segment.End == new Point(56, 90));
        Assert.Contains(distant.Segments, segment =>
            segment.Start == new Point(56, 90) &&
            segment.End == new Point(56, 294));
        Assert.Contains(distant.Segments, segment =>
            segment.Start == new Point(56, 294) &&
            segment.End == new Point(cardAnchorX, 294));
        Assert.DoesNotContain(distant.Segments, segment =>
            segment.Start == new Point(cardAnchorX, 90) && segment.End == new Point(cardAnchorX, 294));

        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "ThreadsWindow.axaml"));
        var treeHost = Assert.Single(document.Descendants(
            XName.Get("ThreadTreeControl", "using:CodexInfo.WindowsClient.Controls")));
        Assert.Equal(treeSurfaceWidth.ToString(), treeHost.Attribute("Width")?.Value);
        Assert.Equal("{Binding TreeSurfaceHeight}", treeHost.Attribute("Height")?.Value);
        Assert.Equal("{Binding TreeConnections}", treeHost.Attribute("Connections")?.Value);

        var linuxThreads = LoadRepositoryFile("ui", "components.slint")
            .Split("export component ThreadsWindow inherits Window {")[1]
            .Split("export component ModelUsage inherits Rectangle {")[0];
        foreach (var marker in new[]
        {
            "property <length> tree-gutter-width: 64px;",
            "property <length> tree-base-x: 8px;",
            "property <length> tree-depth-step: 12px;",
            "property <length> tree-junction-y: 48px;",
            "property <length> tree-junction-end-x: self.tree-gutter-width - 5px;",
            "width: parent.tree-junction-end-x - self.x;",
            "x: parent.tree-junction-end-x - 3px;",
            "width: 6px;",
        })
        {
            Assert.Contains(marker, linuxThreads, StringComparison.Ordinal);
        }
    }

    private static XElement BoundText(XElement scope, string binding) =>
        Assert.Single(scope.Descendants(Avalonia + "TextBlock"),
            element => string.Equals(element.Attribute("Text")?.Value, binding, StringComparison.Ordinal));

    private static string StyleSetter(XElement style, string property) =>
        Assert.IsType<XAttribute>(Assert.Single(style.Elements(Avalonia + "Setter"),
            element => element.Attribute("Property")?.Value == property).Attribute("Value")).Value;

    private static string InvokeFormatter(string name, params object?[] arguments)
    {
        var formatter = typeof(ThreadItemViewModel).GetMethod(
            name,
            BindingFlags.Static | BindingFlags.NonPublic);
        Assert.NotNull(formatter);
        return Assert.IsType<string>(formatter!.Invoke(null, arguments));
    }

    private static string ActiveLabel(UiText texts) => texts.LanguageCode switch
    {
        "ja" => "実行中",
        "zh-Hans" => "活跃",
        "ko" => "활성",
        "es" => "Activo",
        "fr" => "Actif",
        "de" => "Aktiv",
        "pt" => "Ativo",
        "it" => "Attivo",
        "ru" => "Активен",
        _ => "Active",
    };

    private static string LoadRepositoryFile(params string[] segments)
    {
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory); directory is not null; directory = directory.Parent)
        {
            var candidate = Path.Combine([directory.FullName, .. segments]);
            if (File.Exists(candidate))
            {
                return File.ReadAllText(candidate);
            }
        }

        throw new FileNotFoundException($"Could not locate repository file: {Path.Combine(segments)}");
    }

}
