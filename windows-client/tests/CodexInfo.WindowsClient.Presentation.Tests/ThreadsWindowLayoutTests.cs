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
        Assert.Equal("{Binding TreeSurfaceHeight}", treeControl.Attribute("Height")?.Value);
        Assert.Equal("{Binding TreeConnections}", treeControl.Attribute("Connections")?.Value);
        Assert.Equal("{Binding TreeRootRows}", treeControl.Attribute("RootRows")?.Value);

        var title = BoundText(row, "{Binding Title}");
        Assert.Equal("0", title.Attribute("Grid.Column")?.Value);
        Assert.Null(title.Attribute("Grid.Row"));
        Assert.Equal("Wrap", title.Attribute("TextWrapping")?.Value);
        Assert.Equal("2", title.Attribute("MaxLines")?.Value);
        Assert.Null(title.Attribute("TextTrimming"));
        Assert.Equal("{Binding Title}", title.Attribute("AutomationProperties.Name")?.Value);
        Assert.Equal("{Binding Title}", title.Attribute("AutomationProperties.HelpText")?.Value);
        Assert.Equal("{Binding Title}", title.Attribute("ToolTip.Tip")?.Value);
        Assert.Equal("{Binding Title}", card.Attribute("AutomationProperties.Name")?.Value);
        Assert.DoesNotContain(document.Descendants(Avalonia + "TextBlock"),
            element => element.Attribute("Text")?.Value is "{Binding TreeRelationText}" or "{Binding TreeGuideText}");

        Assert.Null(card.Elements().SingleOrDefault(element => element.Name.LocalName == "Border.RenderTransform"));
        Assert.DoesNotContain(document.Descendants(Avalonia + "TextBlock"),
            element => element.Attribute("Text")?.Value == "{Binding RoleStatusText}");
        Assert.DoesNotContain(document.Descendants(Avalonia + "Border"),
            element => element.Attribute("Classes")?.Value == "thread-root-marker");
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
    public void ThreadTreeConnectionsUseCenteredPortsSharedRailsAndJunctions()
    {
        const double treeSurfaceWidth = 860;
        const double cardLeft = 80;
        const double railX = 10;
        const double nestedRailX = 26;
        const double row0CenterY = 48;
        const double row1CenterY = 144;
        const double row2CenterY = 240;
        const double row3CenterY = 336;
        const double row4CenterY = 432;
        const double row7CenterY = 720;
        var direct = ThreadTreeControl.BuildGeometry(
            treeSurfaceWidth,
            192,
            [new ThreadTreeConnection(0, 1, 0)]);
        Assert.Contains(direct.Segments, segment =>
            segment.Start == new Point(cardLeft, row0CenterY) &&
            segment.End == new Point(railX, row0CenterY));
        Assert.Contains(direct.Segments, segment =>
            segment.Start == new Point(railX, row0CenterY) &&
            segment.End == new Point(railX, row1CenterY));
        Assert.Contains(direct.Segments, segment =>
            segment.Start == new Point(railX, row1CenterY) &&
            segment.End == new Point(cardLeft, row1CenterY));
        Assert.Contains(direct.Segments, segment =>
            segment.Start == new Point(cardLeft, row1CenterY) &&
            segment.End == new Point(cardLeft - 7, row1CenterY - 5));
        Assert.Contains(direct.Segments, segment =>
            segment.Start == new Point(cardLeft, row1CenterY) &&
            segment.End == new Point(cardLeft - 7, row1CenterY + 5));
        Assert.Contains(new Point(railX, row0CenterY), direct.Junctions);
        Assert.Contains(new Point(railX, row1CenterY), direct.Junctions);

        var root = ThreadTreeControl.BuildGeometry(treeSurfaceWidth, 96, [], [0]);
        Assert.Contains(root.Segments, segment =>
            segment.Start == new Point(railX, row0CenterY) &&
            segment.End == new Point(cardLeft, row0CenterY));
        Assert.Contains(root.Segments, segment =>
            segment.Start == new Point(cardLeft, row0CenterY) &&
            segment.End == new Point(cardLeft - 7, row0CenterY - 5));
        Assert.Contains(root.Segments, segment =>
            segment.Start == new Point(cardLeft, row0CenterY) &&
            segment.End == new Point(cardLeft - 7, row0CenterY + 5));

        var nested = ThreadTreeControl.BuildGeometry(
            treeSurfaceWidth,
            288,
            [new ThreadTreeConnection(0, 1, 0), new ThreadTreeConnection(1, 2, 1)]);
        Assert.Contains(nested.Segments, segment =>
            segment.Start == new Point(cardLeft, row1CenterY) &&
            segment.End == new Point(nestedRailX, row1CenterY));
        Assert.Contains(nested.Segments, segment =>
            segment.Start == new Point(nestedRailX, row1CenterY) &&
            segment.End == new Point(nestedRailX, row2CenterY));
        Assert.Contains(nested.Segments, segment =>
            segment.Start == new Point(nestedRailX, row2CenterY) &&
            segment.End == new Point(cardLeft, row2CenterY));
        Assert.Contains(new Point(nestedRailX, row1CenterY), nested.Junctions);
        Assert.Contains(new Point(nestedRailX, row2CenterY), nested.Junctions);

        var distant = ThreadTreeControl.BuildGeometry(
            treeSurfaceWidth,
            384,
            [new ThreadTreeConnection(0, 3, 0)]);
        Assert.Contains(distant.Segments, segment =>
            segment.Start == new Point(cardLeft, row0CenterY) &&
            segment.End == new Point(railX, row0CenterY));
        Assert.Contains(distant.Segments, segment =>
            segment.Start == new Point(railX, row0CenterY) &&
            segment.End == new Point(railX, row3CenterY));
        Assert.Contains(distant.Segments, segment =>
            segment.Start == new Point(railX, row3CenterY) &&
            segment.End == new Point(cardLeft, row3CenterY));
        Assert.Contains(distant.Segments, segment =>
            segment.Start == new Point(cardLeft, row3CenterY) &&
            segment.End == new Point(cardLeft - 7, row3CenterY - 5));

        var threeChildren = ThreadTreeControl.BuildGeometry(
            treeSurfaceWidth,
            384,
            [
                new ThreadTreeConnection(0, 1, 0),
                new ThreadTreeConnection(0, 2, 0),
                new ThreadTreeConnection(0, 3, 0),
            ]);
        Assert.Contains(threeChildren.Segments, segment =>
            segment.Start == new Point(cardLeft, row0CenterY) && segment.End == new Point(railX, row0CenterY));
        Assert.Contains(threeChildren.Segments, segment =>
            segment.Start == new Point(railX, row0CenterY) && segment.End == new Point(railX, row3CenterY));
        Assert.Contains(threeChildren.Segments, segment =>
            segment.Start == new Point(railX, row2CenterY) && segment.End == new Point(cardLeft, row2CenterY));
        Assert.Contains(threeChildren.Segments, segment =>
            segment.Start == new Point(railX, row3CenterY) && segment.End == new Point(cardLeft, row3CenterY));
        Assert.Contains(threeChildren.Segments, segment =>
            segment.Start == new Point(cardLeft, row2CenterY) && segment.End == new Point(cardLeft - 7, row2CenterY - 5));
        Assert.Equal(
            1,
            threeChildren.Segments.Count(segment =>
                segment.Start == new Point(railX, row0CenterY) && segment.End == new Point(railX, row3CenterY)));
        Assert.Equal(4, threeChildren.Junctions.Count);

        var mixedBranching = ThreadTreeControl.BuildGeometry(
            treeSurfaceWidth,
            8 * 96,
            [
                new ThreadTreeConnection(0, 1, 0),
                new ThreadTreeConnection(1, 2, 1),
                new ThreadTreeConnection(1, 3, 1),
                new ThreadTreeConnection(0, 4, 0),
                new ThreadTreeConnection(4, 5, 1),
                new ThreadTreeConnection(4, 6, 1),
                new ThreadTreeConnection(0, 7, 0),
            ]);
        Assert.Contains(mixedBranching.Segments, segment =>
            segment.Start == new Point(cardLeft, row1CenterY) && segment.End == new Point(nestedRailX, row1CenterY));
        Assert.Contains(mixedBranching.Segments, segment =>
            segment.Start == new Point(nestedRailX, row1CenterY) && segment.End == new Point(nestedRailX, row3CenterY));
        Assert.Contains(mixedBranching.Segments, segment =>
            segment.Start == new Point(cardLeft, row0CenterY) && segment.End == new Point(railX, row0CenterY));
        Assert.Contains(mixedBranching.Segments, segment =>
            segment.Start == new Point(railX, row0CenterY) && segment.End == new Point(railX, row7CenterY));
        Assert.Contains(mixedBranching.Segments, segment =>
            segment.Start == new Point(railX, row4CenterY) && segment.End == new Point(cardLeft, row4CenterY));

        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "ThreadsWindow.axaml"));
        Assert.Equal("{Binding TreeConnections}",
            Assert.Single(document.Descendants(
                XName.Get("ThreadTreeControl", "using:CodexInfo.WindowsClient.Controls")))
            .Attribute("Connections")?.Value);
        Assert.Equal("{Binding TreeRootRows}",
            Assert.Single(document.Descendants(
                XName.Get("ThreadTreeControl", "using:CodexInfo.WindowsClient.Controls")))
            .Attribute("RootRows")?.Value);

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
