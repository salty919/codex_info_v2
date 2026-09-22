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
        const int expectedContentWidth = 860;
        const int expectedGutterWidth = 72;
        const int expectedColumnGap = 10;
        const int expectedRoleWidth = 110;
        const int expectedModelWidth = 190;
        const int expectedTimeWidth = 180;
        const int expectedRightInset = 36;
        var expectedTitleWidth = expectedContentWidth - expectedGutterWidth -
            (3 * expectedColumnGap) - expectedRoleWidth - expectedModelWidth -
            expectedTimeWidth - expectedRightInset;
        Assert.Equal(242, expectedTitleWidth);

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
        var cardGap = int.Parse(Assert.IsType<XAttribute>(card.Attribute("Margin")).Value.Split(',')[3]);
        Assert.Equal(expectedRowHeight, cardHeight + cardGap);
        Assert.Equal(expectedVisibleRows, viewportHeight / (cardHeight + cardGap));
        Assert.Equal("0", StyleSetter(cardStyle, "Padding"));
        Assert.Equal($"{expectedGutterWidth},0,0,4", card.Attribute("Margin")?.Value);

        var row = Assert.Single(card.Descendants(Avalonia + "Grid"),
            element => element.Attribute("ColumnDefinitions") is not null);
        var laneWidths = Assert.IsType<XAttribute>(row.Attribute("ColumnDefinitions")).Value
            .Split(',')
            .Select(int.Parse)
            .ToArray();
        Assert.Equal(
            [expectedRoleWidth, expectedTitleWidth, expectedModelWidth, expectedTimeWidth],
            laneWidths);
        Assert.Equal("0", row.Attribute("Margin")?.Value);
        Assert.Equal(expectedColumnGap.ToString(), row.Attribute("ColumnSpacing")?.Value);
        var treeControl = Assert.Single(document.Descendants(
            XName.Get("ThreadTreeControl", "using:CodexInfo.WindowsClient.Controls")));
        Assert.Equal("64", treeControl.Attribute("Width")?.Value);

        var title = BoundText(row, "{Binding Title}");
        Assert.Equal("1", title.Attribute("Grid.Column")?.Value);
        Assert.Equal("Wrap", title.Attribute("TextWrapping")?.Value);
        Assert.Equal("2", title.Attribute("MaxLines")?.Value);
        Assert.Equal("CharacterEllipsis", title.Attribute("TextTrimming")?.Value);
        Assert.Equal("{Binding Title}", title.Attribute("AutomationProperties.Name")?.Value);
        Assert.Equal("{Binding Title}", title.Attribute("AutomationProperties.HelpText")?.Value);
        Assert.Equal("{Binding Title}", title.Attribute("ToolTip.Tip")?.Value);
        Assert.Equal("{Binding Title}", card.Attribute("AutomationProperties.Name")?.Value);

        var role = BoundText(row, "{Binding RoleStatusText}");
        Assert.Equal("0", role.Attribute("Grid.Column")?.Value ?? "0");
        var model = BoundText(row, "{Binding ModelText}");
        var context = BoundText(row, "{Binding ContextUsageText}");
        Assert.Equal("2", model.Parent?.Attribute("Grid.Column")?.Value);
        Assert.Same(model.Parent, context.Parent);
        Assert.Equal("{Binding HasContextUsage}", context.Attribute("IsVisible")?.Value);
        Assert.Equal("Wrap", context.Attribute("TextWrapping")?.Value);
        Assert.Equal("2", context.Attribute("MaxLines")?.Value);
        Assert.Null(context.Attribute("TextTrimming"));
        Assert.Equal("{Binding ContextUsageText}", context.Attribute("AutomationProperties.Name")?.Value);
        Assert.Equal("{Binding ContextUsageText}", context.Attribute("ToolTip.Tip")?.Value);
        var elapsed = BoundText(row, "{Binding ElapsedMinutesText}");
        Assert.Equal("3", elapsed.Parent?.Attribute("Grid.Column")?.Value);
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
    public void ThreadTreeRailGeometryConnectsRowsAndStopsAtTheFinalBranch()
    {
        const double rowHeight = 96;
        const double treeGutterWidth = 64;
        const double junctionRightInset = 5;
        const double rootRailX = 8;
        const double nestedRailX = 20;
        const double junctionEndX = treeGutterWidth - junctionRightInset;

        var parent = InvokeRailGeometry(rowHeight, 0, connectedToParent: false, hasChildren: true, hasNextSibling: false);
        var parentDown = Assert.Single(parent.Segments, segment =>
            segment.Start == new Point(rootRailX, rowHeight / 2) &&
            segment.End == new Point(rootRailX, rowHeight));

        var onlyChild = InvokeRailGeometry(rowHeight, 1, connectedToParent: true, hasChildren: false, hasNextSibling: false);
        var childUp = Assert.Single(onlyChild.Segments, segment =>
            segment.Start == new Point(rootRailX, 0) &&
            segment.End == new Point(rootRailX, rowHeight / 2));
        Assert.Equal(parentDown.End.X, childUp.Start.X);
        Assert.Equal(parentDown.End.Y, rowHeight + childUp.Start.Y);
        Assert.Contains(onlyChild.Segments, segment =>
            segment.Start == new Point(rootRailX, rowHeight / 2) &&
            segment.End == new Point(junctionEndX, rowHeight / 2));
        Assert.Equal(new Point(junctionEndX, rowHeight / 2), onlyChild.JunctionDot);
        Assert.DoesNotContain(onlyChild.Segments, segment =>
            segment.Start.X == rootRailX && segment.End == new Point(rootRailX, rowHeight));

        var firstSibling = InvokeRailGeometry(rowHeight, 1, connectedToParent: true, hasChildren: false, hasNextSibling: true);
        Assert.Contains(firstSibling.Segments, segment =>
            segment.Start == new Point(rootRailX, 0) &&
            segment.End == new Point(rootRailX, rowHeight));

        var nestedParent = InvokeRailGeometry(rowHeight, 1, connectedToParent: true, hasChildren: true, hasNextSibling: false);
        var nestedChild = InvokeRailGeometry(rowHeight, 2, connectedToParent: true, hasChildren: false, hasNextSibling: false);
        var nestedParentDown = Assert.Single(nestedParent.Segments, segment =>
            segment.Start == new Point(nestedRailX, rowHeight / 2) &&
            segment.End == new Point(nestedRailX, rowHeight));
        var nestedChildUp = Assert.Single(nestedChild.Segments, segment =>
            segment.Start == new Point(nestedRailX, 0) &&
            segment.End == new Point(nestedRailX, rowHeight / 2));
        Assert.Equal(nestedParentDown.End.X, nestedChildUp.Start.X);
        Assert.Equal(nestedParentDown.End.Y, rowHeight + nestedChildUp.Start.Y);

        var ancestor = InvokeRailGeometry(
            rowHeight,
            2,
            connectedToParent: true,
            hasChildren: false,
            hasNextSibling: false,
            ancestorGuide1: true);
        Assert.Contains(ancestor.Segments, segment =>
            segment.Start == new Point(rootRailX, 0) &&
            segment.End == new Point(rootRailX, rowHeight));

        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "ThreadsWindow.axaml"));
        var treeHost = Assert.Single(document.Descendants(
            XName.Get("ThreadTreeControl", "using:CodexInfo.WindowsClient.Controls")));
        Assert.Equal(treeGutterWidth.ToString(), treeHost.Attribute("Width")?.Value);
        Assert.Equal(rowHeight.ToString(), treeHost.Attribute("Height")?.Value);

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

    private static RailGeometryFixture InvokeRailGeometry(
        double height,
        int depth,
        bool connectedToParent,
        bool hasChildren,
        bool hasNextSibling,
        bool ancestorGuide1 = false,
        bool ancestorGuide2 = false,
        bool ancestorGuide3 = false)
    {
        var factory = typeof(ThreadTreeControl).GetMethod(
            "BuildGeometry",
            BindingFlags.Static | BindingFlags.NonPublic);
        Assert.NotNull(factory);
        var value = factory!.Invoke(null,
        [
            64d,
            height,
            depth,
            connectedToParent,
            hasChildren,
            hasNextSibling,
            ancestorGuide1,
            ancestorGuide2,
            ancestorGuide3,
        ]);
        Assert.NotNull(value);
        var type = value.GetType();
        var segments = Assert.IsAssignableFrom<IEnumerable>(
                Assert.IsAssignableFrom<PropertyInfo>(type.GetProperty("Segments")).GetValue(value))
            .Cast<object>()
            .Select(segment =>
            {
                var segmentType = segment.GetType();
                var start = Assert.IsType<Point>(Assert.IsAssignableFrom<PropertyInfo>(segmentType.GetProperty("Start")).GetValue(segment));
                var end = Assert.IsType<Point>(Assert.IsAssignableFrom<PropertyInfo>(segmentType.GetProperty("End")).GetValue(segment));
                return new RailSegmentFixture(start, end);
            })
            .ToArray();
        var junctionValue = Assert.IsAssignableFrom<PropertyInfo>(type.GetProperty("JunctionDot")).GetValue(value);
        var junction = junctionValue is Point point ? point : (Point?)null;
        return new RailGeometryFixture(segments, junction);
    }

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

    private sealed record RailGeometryFixture(IReadOnlyList<RailSegmentFixture> Segments, Point? JunctionDot);

    private sealed record RailSegmentFixture(Point Start, Point End);
}
