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
