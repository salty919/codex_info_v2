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
        Assert.Equal("24", grid.Attribute("Margin")?.Value);
        Assert.Equal("12", grid.Attribute("RowSpacing")?.Value);
        var contentRegion = Assert.Single(grid.Elements(), element => element.Attribute("Grid.Row")?.Value == "2");
        var tabs = Assert.Single(contentRegion.DescendantsAndSelf(), element => element.Name.LocalName == "TabControl");
        Assert.Equal("2", contentRegion.Attribute("Grid.Row")?.Value);
        Assert.Equal("Left", tabs.Attribute("TabStripPlacement")?.Value);
        Assert.Contains("settings-tabs", tabs.Attribute("Classes")?.Value);
        var items = tabs.Elements().Where(element => element.Name.LocalName == "TabItem").ToArray();
        Assert.Equal(4, items.Length);
        Assert.All(items, item => Assert.Contains(
            "settings-tab",
            item.Attribute("Classes")?.Value.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries) ?? []));
        Assert.DoesNotContain(document.Descendants(), element => element.Name.LocalName == "ScrollViewer");

        Assert.Contains(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value == "TabItem.settings-tab");
        Assert.Contains(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value == "TabItem.settings-tab:selected");
        var tabStyle = Assert.Single(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value == "TabItem.settings-tab");
        var focusAdornerPropertyType = typeof(Avalonia.Controls.Control)
            .GetProperty(nameof(Avalonia.Controls.Control.FocusAdorner))?.PropertyType;
        Assert.NotNull(focusAdornerPropertyType);
        Assert.True(focusAdornerPropertyType.IsAssignableFrom(typeof(Avalonia.Markup.Xaml.Templates.Template)));
        Assert.Equal("176", tabStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Width").Attribute("Value")?.Value);
        Assert.Equal("40", tabStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "MinHeight").Attribute("Value")?.Value);
        Assert.Equal("12,8", tabStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Padding").Attribute("Value")?.Value);
        Assert.Contains(tabStyle.Descendants(), element =>
            element.Name.LocalName == "Border" &&
            element.Attribute(XNamespace.Get("http://schemas.microsoft.com/winfx/2006/xaml") + "Name")?.Value == "SelectionIndicator");
        var focusAdornerSetter = Assert.Single(tabStyle.Elements(), element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "FocusAdorner");
        var focusAdornerTemplate = Assert.Single(focusAdornerSetter.Elements(), element =>
            element.Name.LocalName == "Template");
        var focusAdornerBorder = Assert.Single(focusAdornerTemplate.Elements(), element =>
            element.Name.LocalName == "Border");
        Assert.Equal("{DynamicResource Theme5EA7E5}", focusAdornerBorder.Attribute("BorderBrush")?.Value);
        Assert.Equal("2", focusAdornerBorder.Attribute("BorderThickness")?.Value);
        Assert.DoesNotContain(tabStyle.Descendants(), element =>
            element.Attribute(XNamespace.Get("http://schemas.microsoft.com/winfx/2006/xaml") + "Name")?.Value == "FocusOutline");
        var selectedTabStyle = Assert.Single(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value == "TabItem.settings-tab:selected");
        Assert.DoesNotContain(selectedTabStyle.Descendants(), element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "FocusAdorner");
        Assert.Contains(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value ==
                "TabItem.settings-tab:selected /template/ Border#SelectionIndicator");

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

        var languageSection = Assert.Single(items[0].Elements(), element => element.Name.LocalName == "StackPanel");
        Assert.Equal("Top", languageSection.Attribute("VerticalAlignment")?.Value);
        Assert.Equal("16", languageSection.Attribute("Margin")?.Value);
        Assert.Equal("8", languageSection.Attribute("Spacing")?.Value);
        Assert.DoesNotContain(languageSection.DescendantsAndSelf(), element => element.Name.LocalName == "Border");
        Assert.Contains(languageSection.Elements(), element =>
            element.Name.LocalName == "TextBlock" &&
            element.Attribute("Text")?.Value == "{Binding Texts.Language}" &&
            element.Attribute("FontSize")?.Value == "18");
        var languageSelector = Assert.Single(languageSection.Elements(), element => element.Name.LocalName == "ComboBox");
        Assert.Equal("420", languageSelector.Attribute("Width")?.Value);
        Assert.Equal("36", languageSelector.Attribute("Height")?.Value);
        AssertExactlyOneBinding(items[0], "ItemsSource", "{Binding LanguageOptions}");
        AssertExactlyOneBinding(items[0], "SelectedValue", "{Binding SelectedLanguageCode}");
        AssertExactlyOneBinding(items[0], "SelectedValueBinding", "{Binding LanguageCode}");
        AssertExactlyOneBinding(items[0], "AutomationProperties.AutomationId", "Settings.LanguageSelector");
        var timeZoneSection = Assert.Single(items[1].Elements(), element => element.Name.LocalName == "StackPanel");
        Assert.Equal("Top", timeZoneSection.Attribute("VerticalAlignment")?.Value);
        Assert.Equal("16", timeZoneSection.Attribute("Margin")?.Value);
        Assert.Equal("8", timeZoneSection.Attribute("Spacing")?.Value);
        Assert.DoesNotContain(timeZoneSection.DescendantsAndSelf(), element => element.Name.LocalName == "Border");
        Assert.Contains(timeZoneSection.Elements(), element =>
            element.Name.LocalName == "TextBlock" &&
            element.Attribute("Text")?.Value == "{Binding Texts.TimeZone}" &&
            element.Attribute("FontSize")?.Value == "18");
        var timeZoneSelector = Assert.Single(timeZoneSection.Elements(), element => element.Name.LocalName == "ComboBox");
        Assert.Equal("420", timeZoneSelector.Attribute("Width")?.Value);
        Assert.Equal("36", timeZoneSelector.Attribute("Height")?.Value);
        AssertExactlyOneBinding(items[1], "ItemsSource", "{Binding TimeZoneOptions}");
        AssertExactlyOneBinding(items[1], "SelectedValue", "{Binding SelectedTimeZoneId}");
        AssertExactlyOneBinding(items[1], "SelectedValueBinding", "{Binding Id}");
        AssertExactlyOneBinding(items[1], "AutomationProperties.AutomationId", "Settings.TimeZoneSelector");
        var appearanceSection = Assert.Single(items[2].Elements(), element => element.Name.LocalName == "StackPanel");
        Assert.Equal("Top", appearanceSection.Attribute("VerticalAlignment")?.Value);
        Assert.Equal("16", appearanceSection.Attribute("Margin")?.Value);
        Assert.Equal("8", appearanceSection.Attribute("Spacing")?.Value);
        Assert.DoesNotContain(appearanceSection.DescendantsAndSelf(), element => element.Name.LocalName == "Border");
        Assert.Contains(appearanceSection.Elements(), element =>
            element.Name.LocalName == "TextBlock" &&
            element.Attribute("Text")?.Value == "{Binding Texts.Appearance}" &&
            element.Attribute("FontSize")?.Value == "18");
        var appearanceSelector = Assert.Single(appearanceSection.Elements(), element => element.Name.LocalName == "ComboBox");
        Assert.Equal("420", appearanceSelector.Attribute("Width")?.Value);
        Assert.Equal("36", appearanceSelector.Attribute("Height")?.Value);
        Assert.DoesNotContain(document.Descendants(), element =>
            element.Attribute("Text")?.Value == "{Binding Texts.AppearanceDescription}");
        AssertExactlyOneBinding(items[2], "Text", "{Binding Texts.Appearance}");
        AssertExactlyOneBinding(items[2], "ItemsSource", "{Binding ThemeOptions}");
        AssertExactlyOneBinding(items[2], "SelectedValue", "{Binding SelectedThemeId}");
        AssertExactlyOneBinding(items[2], "AutomationProperties.AutomationId", "Settings.ThemeSelector");
        Assert.DoesNotContain(document.Descendants(), element =>
            element.Attribute("Text")?.Value == "{Binding Texts.ConnectionEndpoint}");
        AssertExactlyOneBinding(items[3], "Text", "{Binding CurrentEndpoint}");
        var connectionSection = Assert.Single(items[3].Elements(), element => element.Name.LocalName == "StackPanel");
        Assert.Equal("Top", connectionSection.Attribute("VerticalAlignment")?.Value);
        Assert.Equal("16", connectionSection.Attribute("Margin")?.Value);
        Assert.DoesNotContain(connectionSection.DescendantsAndSelf(), element => element.Name.LocalName == "Border");
        Assert.Contains(connectionSection.Elements(), element =>
            element.Name.LocalName == "TextBlock" &&
            element.Attribute("Text")?.Value == "{Binding Texts.SettingsConnectionStatus}" &&
            element.Attribute("FontSize")?.Value == "18");
        var connectionStatusTitle = Assert.Single(items[3].Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding StatusTitle}");
        Assert.Equal("Settings.ConnectionStatus.Title", connectionStatusTitle.Attribute("AutomationProperties.AutomationId")?.Value);
        Assert.DoesNotContain(document.Descendants(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.ConnectionStatus.Detail");
        var saveError = Assert.Single(contentRegion.Elements(), element => element.Attribute("Grid.Row")?.Value == "1");
        Assert.Equal("{Binding SaveFailed}", saveError.Attribute("IsVisible")?.Value);
        var saveErrorText = Assert.Single(saveError.Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding StatusDetail}");
        Assert.Equal("Settings.SaveError", saveErrorText.Attribute("AutomationProperties.AutomationId")?.Value);
        AssertExactlyOneBinding(items[3], "Click", "OnRefresh");
        AssertExactlyOneBinding(items[3], "AutomationProperties.AutomationId", "Settings.AuthCheck");
        AssertExactlyOneBinding(items[3], "Click", "OnAuth");
        AssertExactlyOneBinding(items[3], "Text", "{Binding RuntimeVersionStatus}");
        var recorderVersion = Assert.Single(items[3].Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding RecorderVersion}");
        Assert.Equal("Grid", recorderVersion.Parent?.Name.LocalName);
        Assert.Contains(recorderVersion.Parent!.Elements(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "Recorder");
        var restVersion = Assert.Single(items[3].Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding RestVersion}");
        Assert.Equal("Grid", restVersion.Parent?.Name.LocalName);
        Assert.Contains(restVersion.Parent!.Elements(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "REST");
        foreach (var binding in new[]
        {
            "{Binding CurrentEndpoint}",
            "{Binding StatusTitle}",
            "{Binding RecorderVersion}",
            "{Binding RestVersion}",
            "{Binding RuntimeVersionStatus}",
        })
        {
            Assert.DoesNotContain(items.Take(3).SelectMany(item => item.Descendants()), element =>
                element.Attribute("Text")?.Value == binding);
        }

        var accountSelector = Assert.Single(grid.Elements(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.AccountSelector");
        Assert.Equal("1", accountSelector.Attribute("Grid.Row")?.Value);
        var footer = Assert.Single(grid.Elements(), element =>
            element.Name.LocalName == "Grid" && element.Attribute("Grid.Row")?.Value == "3");
        Assert.Equal("*,Auto", footer.Attribute("ColumnDefinitions")?.Value);
        var secondaryActions = Assert.Single(footer.Elements(), element =>
            element.Name.LocalName == "StackPanel" && element.Attribute("Grid.Column")?.Value == "0");
        var saveButton = Assert.Single(footer.Elements(), element =>
            element.Name.LocalName == "Button" && element.Attribute("Grid.Column")?.Value == "1");
        Assert.Contains(secondaryActions.Descendants(), element => element.Attribute("Text")?.Value == "{Binding Texts.ConnectionSettings}");
        Assert.Contains(secondaryActions.Descendants(), element => element.Attribute("Text")?.Value == "{Binding Texts.LicenseInformation}");
        Assert.Equal("secondary", Assert.Single(secondaryActions.Elements(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.Footer.Setup").Attribute("Classes")?.Value);
        Assert.Equal("secondary", Assert.Single(secondaryActions.Elements(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.Footer.Legal").Attribute("Classes")?.Value);
        Assert.Contains(saveButton.DescendantsAndSelf(), element => element.Attribute("Text")?.Value == "{Binding Texts.Save}");
        Assert.Equal("primary", saveButton.Attribute("Classes")?.Value);
        AssertExactlyOneBinding(footer, "AutomationProperties.AutomationId", "Settings.Footer.Setup");
        AssertExactlyOneBinding(footer, "AutomationProperties.AutomationId", "Settings.Footer.Legal");
        AssertExactlyOneBinding(footer, "AutomationProperties.AutomationId", "Settings.Footer.Save");
        AssertExactlyOneBinding(footer, "Click", "OnOpenSetup");
        AssertExactlyOneBinding(footer, "Click", "OnOpenLegal");
        AssertExactlyOneBinding(footer, "Click", "OnSave");

        var localizedSettingsNames = new (string Locale, string ConnectionSettings, string LicenseInformation)[]
        {
            ("ja", "接続設定", "ライセンス情報"),
            ("en", "Connection settings", "License information"),
            ("zh-Hans", "连接设置", "许可证信息"),
            ("ko", "연결 설정", "라이선스 정보"),
            ("es", "Configuración de conexión", "Información de licencia"),
            ("fr", "Paramètres de connexion", "Informations sur la licence"),
            ("de", "Verbindungseinstellungen", "Lizenzinformationen"),
            ("pt", "Configurações de conexão", "Informações da licença"),
            ("it", "Impostazioni di connessione", "Informazioni sulla licenza"),
            ("ru", "Настройки подключения", "Информация о лицензии"),
        };
        foreach (var (locale, connectionSettings, licenseInformation) in localizedSettingsNames)
        {
            var text = Assert.Single(
                CodexInfo.WindowsClient.Localization.LocalizationService.Languages,
                language => language.LanguageCode == locale);
            Assert.Equal(connectionSettings, text.ConnectionSettings);
            Assert.Equal(licenseInformation, text.LicenseInformation);
        }

        var licenseWindow = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "LegalNoticesWindow.axaml"));
        Assert.Equal("Codex Info License", licenseWindow.Root?.Attribute("Title")?.Value);
        AssertExactlyOneBinding(licenseWindow.Root!, "Text", "{Binding Texts.LicenseInformation}");
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
