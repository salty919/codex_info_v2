// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Reflection;
using System.Xml.Linq;
using CodexInfo.WindowsClient.Core;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class MainLayoutParityTests
{
    [Fact]
    public void MainHeaderUsesTheIssue349OrderWithoutAThreadsEntry()
    {
        var source = LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "MainWindow.axaml");
        var document = XDocument.Parse(source);
        var ids = document.Descendants()
            .Select(element => element.Attribute("AutomationProperties.AutomationId")?.Value)
            .Where(value => value is not null)
            .Cast<string>()
            .ToArray();

        Assert.DoesNotContain("Main.OpenThreads", ids);
        Assert.True(IndexOf(source, "Main.UsageStatus") < IndexOf(source, "Main.AccountSelector"));
        Assert.True(IndexOf(source, "Main.AccountSelector") < IndexOf(source, "Main.OpenGraph"));
        Assert.True(IndexOf(source, "Main.OpenGraph") < IndexOf(source, "Main.OpenLegal"));
        Assert.True(IndexOf(source, "Main.OpenLegal") < IndexOf(source, "Main.OpenSettings"));

        AssertCurrentMarker(document, "Main.SelectedAccountCurrentMarker", "{Binding SelectedAccount.IsCurrent}");
        AssertCurrentMarker(document, "Main.AccountCurrentMarker", "{Binding IsCurrent}");
        Assert.Contains("Text=\"{Binding MainDisplayLabel}\"", source, StringComparison.Ordinal);
    }

    [Fact]
    public void MainSectionsUseTheIssue349HybridComposition()
    {
        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "MainWindow.axaml"));

        var quotaBar = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.RemainingQuotaBar");
        Assert.Equal("1", quotaBar.Attribute("Grid.Row")?.Value);
        Assert.Equal("2", quotaBar.Attribute("Grid.ColumnSpan")?.Value);

        var gauge = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.QuotaPeriodGauge");
        Assert.Equal("1", gauge.Attribute("Grid.Row")?.Value);
        var reset = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.QuotaResetAt");
        var observed = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.QuotaObservedAt");
        Assert.Equal("2", reset.Attribute("Grid.Row")?.Value);
        Assert.Equal("2", observed.Attribute("Grid.Row")?.Value);

        var details = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.OpenThreadDetails");
        Assert.Equal("{Binding HasActiveThreads}", details.Attribute("IsVisible")?.Value);

        var modelSection = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.ModelUsageTable");
        Assert.DoesNotContain(modelSection.DescendantsAndSelf(), element =>
            element.Attribute("Text")?.Value is "{Binding Texts.ModelUsageDescription}" or
                "{Binding DetailsStatusText}");

        var status = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.StatusBanner");
        Assert.DoesNotContain(status.DescendantsAndSelf(), element =>
            element.Attribute("Text")?.Value == "{Binding SelectedAccountText}");
    }

    [Fact]
    public void MainUsesTheIssue360FixedQuotaGaugeAndStatusGeometry()
    {
        var source = LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "MainWindow.axaml");
        var document = XDocument.Parse(source);
        var surface = document.Descendants()
            .Single(element => element.Name.LocalName == "Grid" &&
                element.Attribute("RowDefinitions")?.Value == "52,82,78,56,102,42");

        Assert.Equal("22,14", surface.Attribute("Margin")?.Value);
        Assert.Equal("8", surface.Attribute("RowSpacing")?.Value);

        var quota = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.RemainingQuotaBar");
        var quotaGrid = quota.Parent;
        Assert.NotNull(quotaGrid);
        Assert.Equal("604,210", quotaGrid.Attribute("ColumnDefinitions")?.Value);
        Assert.Equal("14", quotaGrid.Attribute("ColumnSpacing")?.Value);
        Assert.Equal("13,5", quotaGrid.Parent?.Attribute("Padding")?.Value);
        Assert.Contains(
            document.Descendants().Where(element => element.Name.LocalName == "TextBlock"),
            element => element.Attribute("Width")?.Value == "604" &&
                element.Attribute("Text")?.Value == "{Binding RemainingPercentText}");
        Assert.Equal("0,4,0,0", quota.Attribute("Margin")?.Value);
        Assert.Equal("Top", quota.Attribute("VerticalAlignment")?.Value);

        var gauge = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.QuotaPeriodGauge");
        var gaugeGrid = gauge.Parent;
        Assert.NotNull(gaugeGrid);
        Assert.Equal("18,20,24", gaugeGrid.Attribute("RowDefinitions")?.Value);
        Assert.Equal("0", gaugeGrid.Attribute("RowSpacing")?.Value);
        var quotaStyle = document.Descendants().Single(element =>
            element.Name.LocalName == "Style" &&
            element.Attribute("Selector")?.Value == "Border.quota-segment");
        Assert.Equal("20", quotaStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" &&
            element.Attribute("Property")?.Value == "Height").Attribute("Value")?.Value);
        foreach (var timestamp in new[] { "Main.QuotaResetAt", "Main.QuotaObservedAt" })
        {
            var element = document.Descendants().Single(candidate =>
                candidate.Attribute("AutomationProperties.AutomationId")?.Value == timestamp);
            Assert.Equal("0,2,0,0", element.Attribute("Margin")?.Value);
        }

        var status = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.StatusBanner");

        var updateButton = status.Descendants().Single(element =>
            element.Name.LocalName == "Button" &&
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.Status.Update");
        var updateParent = updateButton.Parent ?? throw new InvalidOperationException(
            "Update CTA button has no direct parent");
        Assert.Equal("2", updateParent.Attribute("Grid.RowSpan")?.Value);

        Assert.Equal("StackPanel", updateParent.Name.LocalName);
        foreach (var ctaId in new[] { "AuthStart", "AuthCheck", "Retry", "Refreshing" })
        {
            var button = status.Descendants().Single(element =>
                element.Name.LocalName == "Button" &&
                element.Attribute("AutomationProperties.AutomationId")?.Value == $"Main.Status.{ctaId}");
            var cta = button.Parent ?? throw new InvalidOperationException(
                $"CTA button has no direct parent: {ctaId}");
            Assert.Equal("StackPanel", cta.Name.LocalName);
            Assert.Equal("2", cta.Attribute("Grid.RowSpan")?.Value);
        }

        Assert.Equal("9,2,7,2", status.Attribute("Padding")?.Value);

        var statusGrid = status.Descendants().Single(element =>
            element.Name.LocalName == "Grid" &&
            element.Attribute("RowDefinitions")?.Value == "17,18");
        Assert.Equal("10", statusGrid.Attribute("ColumnSpacing")?.Value);

        var statusDot = status.Descendants().Single(element =>
            element.Name.LocalName == "Border" &&
            element.Attribute("Background")?.Value == "{Binding StatusAccent}");
        Assert.Equal("8", statusDot.Attribute("Width")?.Value);
        Assert.Equal("8", statusDot.Attribute("Height")?.Value);
        Assert.Equal("0,14,0,0", statusDot.Attribute("Margin")?.Value);

        var showLastReceived = status.Descendants().Single(element =>
            element.Name.LocalName == "StackPanel" &&
            element.Attribute("IsVisible")?.Value == "{Binding ShowLastReceived}");
        Assert.Equal("0,0,4,0", showLastReceived.Attribute("Margin")?.Value);

        var accountStyle = document.Descendants().Single(element =>
            element.Name.LocalName == "Style" &&
            element.Attribute("Selector")?.Value == "ToggleButton.account-selector");
        Assert.Equal("#18283A", accountStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" &&
            element.Attribute("Property")?.Value == "Background").Attribute("Value")?.Value);
        Assert.Equal("#304A63", accountStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" &&
            element.Attribute("Property")?.Value == "BorderBrush").Attribute("Value")?.Value);
        Assert.Equal("6", accountStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" &&
            element.Attribute("Property")?.Value == "CornerRadius").Attribute("Value")?.Value);

        var legalCommand = document.Descendants().Single(element =>
            element.Name.LocalName == "Button" &&
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.OpenLegal");
        var legalClasses = legalCommand.Attribute("Classes")?.Value
            .Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries) ?? [];
        Assert.Contains("command", legalClasses);
        Assert.Contains("legal", legalClasses);

        var minimize = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.Window.Minimize");
        var close = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.Window.Close");
        var windowControlRow = minimize.Parent;
        Assert.NotNull(windowControlRow);
        Assert.Same(windowControlRow, close.Parent);
        Assert.Same(legalCommand.Parent, windowControlRow.Parent);
        Assert.Equal("StackPanel", windowControlRow.Name.LocalName);
        Assert.Equal("Horizontal", windowControlRow.Attribute("Orientation")?.Value);
        Assert.Equal("0", windowControlRow.Attribute("Spacing")?.Value);
        Assert.Equal("12,0,0,0", windowControlRow.Attribute("Margin")?.Value);
        var windowControlStyle = document.Descendants().Single(element =>
            element.Name.LocalName == "Style" &&
            element.Attribute("Selector")?.Value == "Button.window-control");
        Assert.Equal("Center", windowControlStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" &&
            element.Attribute("Property")?.Value == "HorizontalContentAlignment").Attribute("Value")?.Value);
        Assert.Equal("Center", windowControlStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" &&
            element.Attribute("Property")?.Value == "VerticalContentAlignment").Attribute("Value")?.Value);
    }

    [Fact]
    public void MainAccountLabelOmitsStateWordsWhileGraphLabelKeepsThem()
    {
        var account = new ApiAccount("account-7", true, 1, null)
        {
            DisplayStatusSuffix = "［ログイン中］",
        };
        var property = typeof(ApiAccount).GetProperty(
            "MainDisplayLabel",
            BindingFlags.Instance | BindingFlags.Public);

        Assert.NotNull(property);
        Assert.Equal("アカウント 7 · ID未復元", property!.GetValue(account));
        Assert.Equal("アカウント 7 · ID未復元［ログイン中］", account.DisplayLabel);
    }

    private static void AssertCurrentMarker(XDocument document, string automationId, string visibility)
    {
        var marker = document.Descendants().Single(element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == automationId);
        Assert.Equal("●", marker.Attribute("Text")?.Value);
        Assert.Equal("#5DC98A", marker.Attribute("Foreground")?.Value);
        Assert.Equal(visibility, marker.Attribute("IsVisible")?.Value);
    }

    private static int IndexOf(string source, string marker)
    {
        var index = source.IndexOf(marker, StringComparison.Ordinal);
        Assert.True(index >= 0, $"Missing Main layout marker: {marker}");
        return index;
    }

    private static string LoadRepositoryFile(params string[] segments)
    {
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory);
             directory is not null;
             directory = directory.Parent)
        {
            var candidate = Path.Combine([directory.FullName, .. segments]);
            if (File.Exists(candidate))
            {
                return File.ReadAllText(candidate);
            }
        }

        throw new FileNotFoundException(
            $"Could not locate repository file: {Path.Combine(segments)}");
    }
}
