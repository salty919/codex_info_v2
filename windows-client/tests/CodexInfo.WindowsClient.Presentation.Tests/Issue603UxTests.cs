// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Xml.Linq;
using CodexInfo.WindowsClient.Theme;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class Issue603UxTests
{
    [Theory]
    [InlineData("Main")]
    [InlineData("Graph")]
    [InlineData("Threads")]
    [InlineData("Settings")]
    [InlineData("Setup")]
    [InlineData("LegalNotices")]
    public void WindowsKeepAVisibleNonInteractiveFrame(string window)
    {
        var frame = Assert.Single(Read(window + "Window.axaml").Descendants(), e =>
            e.Attribute("Classes")?.Value == "window-frame");
        Assert.Equal("False", frame.Attribute("IsHitTestVisible")?.Value);
        Assert.Equal("{Binding $parent[Window].IsActive}", frame.Attribute("Classes.active")?.Value);
    }

    [Fact]
    public void SettingsGroupsFieldsAndOffersCancelAndVisibleRefreshResult()
    {
        var document = Read("SettingsWindow.axaml");
        Assert.Contains(document.Descendants(), e => e.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.Display.Surface");
        var cancel = ById(document, "Settings.Footer.Cancel");
        Assert.Equal("OnClose", cancel.Attribute("Click")?.Value);
        Assert.Equal("{Binding Texts.Cancel}", cancel.Attribute("Content")?.Value);
        var refresh = ById(document, "Settings.AuthCheck");
        Assert.Equal("{Binding RefreshButtonText}", refresh.Attribute("Content")?.Value);
        Assert.Equal("{Binding CanRefresh}", refresh.Attribute("IsEnabled")?.Value);
        Assert.Equal("{Binding ConnectionStatusDetail}", ById(document, "Settings.ConnectionStatus.Detail").Attribute("Text")?.Value);
        Assert.Equal("{Binding StatusDetail}", ById(document, "Settings.SaveError").Attribute("Text")?.Value);
    }

    [Fact]
    public void SetupShowsContextAndKeepsManualInstructionsInDetails()
    {
        var document = Read("SetupWindow.axaml");
        Assert.Equal("{Binding WindowTitle}", document.Root!.Attribute("Title")?.Value);
        Assert.Contains(document.Descendants(), e => e.Attribute("Text")?.Value == "{Binding PageTitle}");
        var guide = Assert.Single(document.Descendants(), e => e.Attribute("Text")?.Value == "{Binding Texts.ConnectionGuideBody}");
        Assert.Contains(guide.Ancestors(), e => e.Name.LocalName == "Expander");
        Assert.Equal("{Binding RefreshButtonText}", ById(document, "Setup.Refresh").Attribute("Content")?.Value);
        var authenticate = Assert.Single(document.Descendants(), e => e.Attribute("Click")?.Value == "OnAuth");
        Assert.Equal("{Binding CanAuthenticate}", authenticate.Attribute("IsVisible")?.Value);
        Assert.Equal("{Binding !IsDoneStep}", ById(document, "Setup.Refresh").Attribute("IsVisible")?.Value);
        var dismiss = Assert.Single(document.Descendants(), e => e.Attribute("Click")?.Value == "OnClose" && e.Attribute("Content")?.Value == "{Binding Texts.Close}");
        Assert.Equal("{Binding !IsDoneStep}", dismiss.Attribute("IsVisible")?.Value);
    }

    [Fact]
    public void FramesContrastWithEveryWindowBackground()
    {
        var original = ThemePalette.CurrentId;
        try
        {
            foreach (var preset in ThemePalette.PresetIds)
            {
                ThemePalette.Apply(preset);
                foreach (var role in new[] { "WindowBorder", "WindowBorderActive" })
                {
                    var a = Luminance(ThemePalette.Resolve(role));
                    var b = Luminance(ThemePalette.Resolve("#0E141E"));
                    Assert.True((Math.Max(a, b) + .05) / (Math.Min(a, b) + .05) >= 3, preset + ": " + role);
                }
            }
        }
        finally { ThemePalette.Apply(original); }
    }

    [Fact]
    public void AllWindowsUseSharedTitlesControlsAndVectorIcons()
    {
        var app = Read("App.axaml");
        var frameStyle = Assert.Single(app.Descendants(), e => e.Attribute("Selector")?.Value == "Border.window-frame");
        Assert.Contains(frameStyle.Descendants(), e => e.Attribute("Property")?.Value == "Height" && e.Attribute("Value")?.Value == "{Binding $parent[Window].Bounds.Height}");
        Assert.Contains(frameStyle.Descendants(), e => e.Attribute("Property")?.Value == "Width" && e.Attribute("Value")?.Value == "{Binding $parent[Window].Bounds.Width}");
        foreach (var selector in new[] { "TextBlock.window-title", "Button.window-control", "Button.primary", "Button.secondary" })
            Assert.Single(app.Descendants(), e => e.Attribute("Selector")?.Value == selector);
        var command = Assert.Single(Read("MainWindow.axaml").Descendants(), e => e.Attribute("Selector")?.Value == "Button.command");
        Assert.DoesNotContain(command.Descendants(), e => e.Attribute("Property")?.Value == "Width");
        foreach (var window in new[] { "Main", "Graph", "Threads", "Settings", "Setup", "LegalNotices" })
        {
            var d = Read(window + "Window.axaml");
            Assert.Contains(d.Descendants(), e => e.Attribute("Classes")?.Value == "window-title");
            Assert.Contains(d.Descendants(), e => e.Name.LocalName == "PathIcon");
            Assert.DoesNotContain(d.Descendants(), e => e.Attribute("Selector")?.Value is
                "Button.window-control" or "Button.primary" or "Button.secondary");
        }
    }

    [Fact]
    public void SetupHidesTheInternalNoneSelector()
    {
        var d = Read("SetupWindow.axaml");
        var selector = Assert.Single(d.Descendants(), e => e.Attribute("SelectedItem")?.Value == "{Binding SelectedConnectionSelector, Mode=TwoWay}");
        Assert.Contains(selector.Ancestors(), e => e.Attribute("IsVisible")?.Value == "{Binding HasConnectionSelector}");
        var ja = CodexInfo.WindowsClient.Localization.LocalizationService.Languages.Single(t => t.LanguageCode == "ja");
        Assert.Equal("既存の接続を使う", ja.ConnectionProfileNone);
    }

    [Theory]
    [InlineData("de", "Abbrechen")]
    [InlineData("zh-Hans", "取消")]
    [InlineData("ko", "취소")]
    [InlineData("es", "Cancelar")]
    [InlineData("fr", "Annuler")]
    [InlineData("pt", "Cancelar")]
    [InlineData("it", "Annulla")]
    [InlineData("ru", "Отмена")]
    public void CancelUsesTheSelectedLanguage(string locale, string label)
    {
        Assert.Equal(label, CodexInfo.WindowsClient.Localization.LocalizationService.Languages.Single(t => t.LanguageCode == locale).Cancel);
    }

    private static double Luminance(string value)
    {
        var channels = new[] { 1, 3, 5 }.Select(i => Convert.ToInt32(value.Substring(i, 2), 16) / 255.0)
            .Select(c => c <= .04045 ? c / 12.92 : Math.Pow((c + .055) / 1.055, 2.4)).ToArray();
        return channels[0] * .2126 + channels[1] * .7152 + channels[2] * .0722;
    }

    private static XElement ById(XDocument d, string id) => Assert.Single(d.Descendants(),
        e => e.Attribute("AutomationProperties.AutomationId")?.Value == id);
    private static XDocument Read(string file)
    {
        for (var d = new DirectoryInfo(AppContext.BaseDirectory); d is not null; d = d.Parent)
        {
            var p = Path.Combine(d.FullName, "windows-client/src/CodexInfo.WindowsClient", file);
            if (File.Exists(p)) return XDocument.Load(p);
        }
        throw new FileNotFoundException(file);
    }
}
