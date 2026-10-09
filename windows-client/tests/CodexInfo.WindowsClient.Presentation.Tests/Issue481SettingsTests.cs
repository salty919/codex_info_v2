// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Text;
using System.Xml.Linq;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class Issue590SettingsTests
{
    [Fact]
    public void SettingsTabsContainAllExistingControlsAndKeepFooterFixed()
    {
        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "SettingsWindow.axaml"));
        var window = document.Root!;
        Assert.Equal("900", window.Attribute("Width")?.Value);
        Assert.Equal("480", window.Attribute("Height")?.Value);

        var grid = Assert.Single(document.Descendants(), element =>
            element.Name.LocalName == "Grid" && element.Attribute("RowDefinitions")?.Value == "Auto,44,*,Auto");
        var tabs = Assert.Single(grid.Elements(), element => element.Name.LocalName == "TabControl");
        Assert.Equal("2", tabs.Attribute("Grid.Row")?.Value);
        var items = tabs.Elements().Where(element => element.Name.LocalName == "TabItem").ToArray();
        Assert.Equal(4, items.Length);
        Assert.DoesNotContain(document.Descendants(), element => element.Name.LocalName == "ScrollViewer");

        var headers = new[]
        {
            "{Binding Texts.Language}",
            "{Binding Texts.TimeZone}",
            "{Binding Texts.Appearance}",
            "{Binding Texts.SettingsConnectionStatus}",
        };
        var automationIds = new[]
        {
            "Settings.Tab.Language",
            "Settings.Tab.TimeZone",
            "Settings.Tab.Appearance",
            "Settings.Tab.ConnectionStatus",
        };
        Assert.Equal(headers, items.Select(item => item.Attribute("Header")?.Value));
        Assert.Equal(automationIds, items.Select(item => item.Attribute("AutomationProperties.AutomationId")?.Value));
        Assert.Equal(headers, items.Select(item => item.Attribute("AutomationProperties.Name")?.Value));
        var expectedConnectionTabNames = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["ja"] = "接続状態",
            ["en"] = "Connection status",
            ["zh-Hans"] = "连接状态",
            ["ko"] = "연결 상태",
            ["es"] = "Estado de conexión",
            ["fr"] = "État de la connexion",
            ["de"] = "Verbindungsstatus",
            ["pt"] = "Estado da conexão",
            ["it"] = "Stato della connessione",
            ["ru"] = "Состояние подключения",
        };
        Assert.Equal(expectedConnectionTabNames.Count, CodexInfo.WindowsClient.Localization.LocalizationService.Languages.Count);
        foreach (var (locale, expectedName) in expectedConnectionTabNames)
        {
            var text = Assert.Single(
                CodexInfo.WindowsClient.Localization.LocalizationService.Languages,
                language => language.LanguageCode == locale);
            Assert.Equal(expectedName, text.SettingsConnectionStatus);
        }

        AssertExactlyOneBinding(items[0], "ItemsSource", "{Binding LanguageOptions}");
        AssertExactlyOneBinding(items[0], "SelectedValue", "{Binding SelectedLanguageCode}");
        AssertExactlyOneBinding(items[0], "SelectedValueBinding", "{Binding LanguageCode}");
        AssertExactlyOneBinding(items[0], "AutomationProperties.AutomationId", "Settings.LanguageSelector");
        AssertExactlyOneBinding(items[1], "ItemsSource", "{Binding TimeZoneOptions}");
        AssertExactlyOneBinding(items[1], "SelectedValue", "{Binding SelectedTimeZoneId}");
        AssertExactlyOneBinding(items[1], "SelectedValueBinding", "{Binding Id}");
        AssertExactlyOneBinding(items[1], "AutomationProperties.AutomationId", "Settings.TimeZoneSelector");
        AssertExactlyOneBinding(items[2], "ItemsSource", "{Binding ThemeOptions}");
        AssertExactlyOneBinding(items[2], "SelectedValue", "{Binding SelectedThemeId}");
        AssertExactlyOneBinding(items[2], "AutomationProperties.AutomationId", "Settings.ThemeSelector");
        AssertExactlyOneBinding(items[2], "Text", "{Binding Texts.AppearanceDescription}");
        AssertExactlyOneBinding(items[3], "Text", "{Binding StatusTitle}");
        AssertExactlyOneBinding(items[3], "Text", "{Binding StatusDetail}");
        AssertExactlyOneBinding(items[3], "Click", "OnRefresh");
        AssertExactlyOneBinding(items[3], "AutomationProperties.AutomationId", "Settings.AuthCheck");
        AssertExactlyOneBinding(items[3], "Click", "OnAuth");
        AssertExactlyOneBinding(items[3], "Text", "{Binding RecorderVersion}");
        AssertExactlyOneBinding(items[3], "Text", "{Binding RestVersion}");
        AssertExactlyOneBinding(items[3], "Text", "{Binding RuntimeVersionStatus}");

        var accountSelector = Assert.Single(grid.Elements(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.AccountSelector");
        Assert.Equal("1", accountSelector.Attribute("Grid.Row")?.Value);
        var footer = Assert.Single(grid.Elements(), element =>
            element.Name.LocalName == "StackPanel" && element.Attribute("Grid.Row")?.Value == "3");
        Assert.Contains(footer.Descendants(), element => element.Attribute("Text")?.Value == "{Binding Texts.Setup}");
        Assert.Contains(footer.Descendants(), element => element.Attribute("Text")?.Value == "{Binding Texts.Legal}");
        Assert.Contains(footer.Descendants(), element => element.Attribute("Text")?.Value == "{Binding Texts.Save}");
        AssertExactlyOneBinding(footer, "AutomationProperties.AutomationId", "Settings.Footer.Setup");
        AssertExactlyOneBinding(footer, "AutomationProperties.AutomationId", "Settings.Footer.Legal");
        AssertExactlyOneBinding(footer, "AutomationProperties.AutomationId", "Settings.Footer.Save");
    }

    private static void AssertExactlyOneBinding(XElement root, string attribute, string value)
    {
        Assert.Single(root.DescendantsAndSelf(), element => element.Attribute(attribute)?.Value == value);
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
}

public sealed class Issue481SettingsTests
{
    [Fact]
    public async Task ExistingSettingsShowsBothRuntimeVersionsAndClearsUnavailableValues()
    {
        using var handler = new RuntimeHandler();
        using var client = new LoopbackStatusClient(handler);
        using var main = new MainWindowViewModel(client);
        var root = Directory.CreateTempSubdirectory("codex-info-runtime-settings-");
        var settings = new SettingsViewModel(new ClientSettingsStore(Path.Combine(root.FullName, "settings.json")), main);
        try
        {
            var refresh = typeof(SettingsViewModel).GetMethod("RefreshRuntimeVersionsAsync");
            Assert.NotNull(refresh);
            await (Task)refresh.Invoke(settings, null)!;
            Assert.Equal("1.2.3", typeof(SettingsViewModel).GetProperty("RestVersion")!.GetValue(settings));
            Assert.Equal("1.2.2", typeof(SettingsViewModel).GetProperty("RecorderVersion")!.GetValue(settings));
            Assert.Equal("mismatch", typeof(SettingsViewModel).GetProperty("RuntimeVersionState")!.GetValue(settings));
            handler.Unavailable = true;
            await (Task)refresh.Invoke(settings, null)!;
            Assert.Equal(settings.Texts.UnavailableValue, typeof(SettingsViewModel).GetProperty("RestVersion")!.GetValue(settings));
            Assert.Equal(settings.Texts.UnavailableValue, typeof(SettingsViewModel).GetProperty("RecorderVersion")!.GetValue(settings));
            Assert.Equal("unavailable", typeof(SettingsViewModel).GetProperty("RuntimeVersionState")!.GetValue(settings));
        }
        finally
        {
            settings.Dispose();
            root.Delete(recursive: true);
        }
    }

    private sealed class RuntimeHandler : HttpMessageHandler
    {
        public bool Unavailable { get; set; }
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            Assert.Equal("/v1/runtime", request.RequestUri!.AbsolutePath);
            var response = new HttpResponseMessage(Unavailable ? HttpStatusCode.ServiceUnavailable : HttpStatusCode.OK)
            {
                Content = new StringContent("{\"api_version\":\"v1\",\"rest_version\":\"1.2.3\",\"recorder_version\":\"1.2.2\",\"recorder_status\":\"mismatch\"}", Encoding.UTF8, "application/json"),
            };
            response.Headers.CacheControl = new System.Net.Http.Headers.CacheControlHeaderValue { NoStore = true };
            return Task.FromResult(response);
        }
    }
}
