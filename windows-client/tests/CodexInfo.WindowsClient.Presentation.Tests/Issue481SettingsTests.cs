// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.Net;
using System.Reflection;
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
    public void FixtureDefaultSettingsPathIsIsolated()
    {
        const string fixturePortVariable = "CODEX_INFO_WINDOWS_E2E_FIXTURE_PORT";
        const string fixtureSettingsPathVariable = "CODEX_INFO_WINDOWS_E2E_SETTINGS_PATH";
        var originalFixturePort = Environment.GetEnvironmentVariable(fixturePortVariable);
        var originalFixtureSettingsPath = Environment.GetEnvironmentVariable(fixtureSettingsPathVariable);
        var temporaryDirectory = Path.Combine(
            Path.GetTempPath(),
            "codex-info-e2e-settings-" + Guid.NewGuid().ToString("N"));
        var expectedFixturePath = Path.Combine(temporaryDirectory, "settings.json");

        try
        {
            Environment.SetEnvironmentVariable(fixturePortVariable, "12345");
            Environment.SetEnvironmentVariable(fixtureSettingsPathVariable, expectedFixturePath);

            var fixtureStore = new ClientSettingsStore();
            var settingsPathField = typeof(ClientSettingsStore).GetField(
                "path",
                BindingFlags.Instance | BindingFlags.NonPublic);
            Assert.NotNull(settingsPathField);
            Assert.Equal(Path.GetFullPath(expectedFixturePath), settingsPathField.GetValue(fixtureStore));

            // The RED path stops above, before any write to the real profile.
            fixtureStore.Save(ClientSettings.Default);
            Assert.True(File.Exists(expectedFixturePath));

            Environment.SetEnvironmentVariable(fixturePortVariable, null);
            var normalStore = new ClientSettingsStore();
            var expectedNormalPath = Path.GetFullPath(Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "CodexInfo",
                "settings.json"));
            Assert.Equal(expectedNormalPath, settingsPathField.GetValue(normalStore));

            Environment.SetEnvironmentVariable(fixturePortVariable, "12345");
            Environment.SetEnvironmentVariable(fixtureSettingsPathVariable, null);
            Assert.Throws<InvalidOperationException>(() => new ClientSettingsStore());

            Environment.SetEnvironmentVariable(fixtureSettingsPathVariable, "relative/settings.json");
            Assert.Throws<InvalidOperationException>(() => new ClientSettingsStore());
        }
        finally
        {
            Environment.SetEnvironmentVariable(fixturePortVariable, originalFixturePort);
            Environment.SetEnvironmentVariable(fixtureSettingsPathVariable, originalFixtureSettingsPath);
            if (Directory.Exists(temporaryDirectory))
            {
                Directory.Delete(temporaryDirectory, recursive: true);
            }
        }
    }

    [Fact]
    public void SettingsTabContentTemplateSupportsNativeSelection()
    {
        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "SettingsWindow.axaml"));
        var style = Assert.Single(document.Descendants(), element =>
            element.Name.LocalName == "Style" &&
            element.Attribute("Selector")?.Value == "TabControl.settings-content");
        var template = Assert.Single(style.Descendants(), element => element.Name.LocalName == "ControlTemplate");

        var itemsPresenter = Assert.Single(template.Descendants(), element =>
            element.Name.LocalName == "ItemsPresenter" &&
            element.Attribute("Name")?.Value == "PART_ItemsPresenter");
        Assert.Equal("False", itemsPresenter.Attribute("IsVisible")?.Value);
        Assert.Single(template.Descendants(), element =>
            element.Name.LocalName == "ContentPresenter" &&
            element.Attribute("Name")?.Value == "PART_SelectedContentHost");
    }

    [Fact]
    public void SettingsTabsContainAllExistingControlsAndKeepFooterFixed()
    {
        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "SettingsWindow.axaml"));
        var window = document.Root!;
        Assert.Equal("900", window.Attribute("Width")?.Value);
        Assert.Equal("480", window.Attribute("Height")?.Value);

        var grid = Assert.Single(window.Elements(), element => element.Name.LocalName == "Grid");
        Assert.Equal("24", grid.Attribute("Margin")?.Value);
        Assert.Equal("12", grid.Attribute("RowSpacing")?.Value);
        Assert.Equal("Auto", grid.Attribute("RowDefinitions")?.Value.Split(',').Last());
        var header = Assert.Single(grid.Elements(), element =>
            element.Name.LocalName == "Grid" && element.Attribute("ColumnDefinitions")?.Value == "*,Auto" &&
            element.Attribute("Grid.Row") is null);
        Assert.DoesNotContain(header.DescendantsAndSelf(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.AccountSelector");
        Assert.Contains(header.Elements(), element =>
            element.Name.LocalName == "Button" && element.Attribute("Grid.Column")?.Value == "1" &&
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.Window.Close");

        var navigationBand = Assert.Single(grid.Elements(), element =>
            element.Attribute("Grid.Row")?.Value == "1" &&
            element.Descendants().Any(child => child.Name.LocalName == "TabStrip"));
        var tabStrip = Assert.Single(navigationBand.Descendants(), element => element.Name.LocalName == "TabStrip");
        Assert.Equal("{Binding #SettingsTabContent.SelectedIndex, Mode=TwoWay}", tabStrip.Attribute("SelectedIndex")?.Value);
        var stripItems = tabStrip.Elements().Where(element => element.Name.LocalName == "TabStripItem").ToArray();
        Assert.Equal(2, stripItems.Length);
        Assert.All(stripItems, item => Assert.Contains(
            "settings-tab",
            item.Attribute("Classes")?.Value.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries) ?? []));

        var contentRegion = Assert.Single(grid.Elements(), element => element.Attribute("Grid.Row")?.Value == "2");
        var tabs = Assert.Single(contentRegion.DescendantsAndSelf(), element => element.Name.LocalName == "TabControl");
        Assert.Equal("settings-content", tabs.Attribute("Classes")?.Value);
        Assert.Equal("0", tabs.Attribute("Grid.Row")?.Value);
        Assert.Equal("Top", tabs.Attribute("TabStripPlacement")?.Value);
        var items = tabs.Elements().Where(element => element.Name.LocalName == "TabItem").ToArray();
        Assert.Equal(2, items.Length);
        Assert.All(items, item => Assert.Contains(
            "settings-content-item",
            item.Attribute("Classes")?.Value.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries) ?? []));
        Assert.DoesNotContain(document.Descendants(), element => element.Name.LocalName == "ScrollViewer");

        var tabStyle = Assert.Single(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value == "TabStripItem.settings-tab");
        Assert.Contains(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value == "TabStripItem.settings-tab:selected");
        Assert.Equal("{DynamicResource Theme151F2D}", tabStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Background").Attribute("Value")?.Value);
        Assert.Equal("{DynamicResource Theme36516B}", tabStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "BorderBrush").Attribute("Value")?.Value);
        var contentTemplate = Assert.Single(tabStyle.Elements(), element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "ContentTemplate");
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
        var selectedTabSurfaceStyle = Assert.Single(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value ==
                "TabStripItem.settings-tab:selected /template/ Border#PART_Border");
        Assert.Equal("{DynamicResource Theme18283A}", selectedTabSurfaceStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Background").Attribute("Value")?.Value);
        Assert.Contains(document.Descendants(), element =>
            element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value ==
                "TabStripItem.settings-tab:selected /template/ Border#SelectionIndicator");

        var headers = new[]
        {
            "{Binding Texts.SettingsDisplayTab}",
            "{Binding Texts.SettingsConnectionStatus}",
        };
        var automationIds = new[]
        {
            "Settings.Tab.Display",
            "Settings.Tab.ConnectionStatus",
        };
        Assert.Equal(headers, stripItems.Select(item => item.Attribute("Content")?.Value));
        Assert.Equal(automationIds, stripItems.Select(item => item.Attribute("AutomationProperties.AutomationId")?.Value));
        Assert.Equal(headers, stripItems.Select(item => item.Attribute("AutomationProperties.Name")?.Value));
        Assert.DoesNotContain(items, item => item.Attribute("AutomationProperties.AutomationId") is not null);

        var displayTab = items[0];
        var connectionTab = items[1];
        var displayFields = new[]
        {
            (Id: "Settings.LanguageSelector", Items: "{Binding LanguageOptions}", Value: "{Binding SelectedLanguageCode}", ValueBinding: "{Binding LanguageCode}", Label: "{Binding Texts.Language}"),
            (Id: "Settings.TimeZoneSelector", Items: "{Binding TimeZoneOptions}", Value: "{Binding SelectedTimeZoneId}", ValueBinding: "{Binding Id}", Label: "{Binding Texts.TimeZone}"),
            (Id: "Settings.ThemeSelector", Items: "{Binding ThemeOptions}", Value: "{Binding SelectedThemeId}", ValueBinding: "{Binding Id}", Label: "{Binding Texts.Appearance}"),
        };
        foreach (var field in displayFields)
        {
            var selector = Assert.Single(displayTab.Descendants(), element =>
                element.Attribute("AutomationProperties.AutomationId")?.Value == field.Id);
            Assert.Equal("ComboBox", selector.Name.LocalName);
            Assert.Equal("488", selector.Attribute("Width")?.Value);
            Assert.Equal("36", selector.Attribute("Height")?.Value);
            AssertExactlyOneBinding(selector, "ItemsSource", field.Items);
            AssertExactlyOneBinding(selector, "SelectedValue", field.Value);
            AssertExactlyOneBinding(selector, "SelectedValueBinding", field.ValueBinding);
            Assert.Contains(displayTab.Descendants(), element =>
                element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == field.Label);
        }

        var accountSelector = Assert.Single(displayTab.Descendants(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.AccountSelector");
        Assert.Equal("488", accountSelector.Attribute("Width")?.Value);
        Assert.Equal("36", accountSelector.Attribute("Height")?.Value);

        var tabText = contentTemplate.Descendants().Single(element => element.Name.LocalName == "TextBlock");
        Assert.Equal("NoWrap", tabText.Attribute("TextWrapping")?.Value);
        Assert.Equal("None", tabText.Attribute("TextTrimming")?.Value);
        Assert.Null(tabStyle.Descendants().SingleOrDefault(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Width"));
        Assert.Equal("88", tabStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "MinWidth")
            .Attribute("Value")?.Value);
        Assert.Equal("36", tabStyle.Descendants().Single(element =>
            element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Height")
            .Attribute("Value")?.Value);
        Assert.Equal(new[] { "44", "36", "*", "Auto" }, grid.Attribute("RowDefinitions")?.Value.Split(','));
        Assert.Equal("660", navigationBand.Attribute("Width")?.Value);
        Assert.Equal("Center", navigationBand.Attribute("HorizontalAlignment")?.Value);
        Assert.Equal("660", tabStrip.Attribute("Width")?.Value);
        Assert.Equal("36", tabStrip.Attribute("Height")?.Value);
        Assert.Equal("660", contentRegion.Attribute("Width")?.Value);
        Assert.Equal("Center", contentRegion.Attribute("HorizontalAlignment")?.Value);
        Assert.Contains(displayTab.Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding Texts.SettingsDisplayTarget}");
        Assert.Contains(displayTab.Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding SelectedAccountText}");
        Assert.DoesNotContain(accountSelector.Ancestors(), element => element.Name.LocalName == "Border");
        var displayFieldIds = displayFields.Select(field => field.Id).Append("Settings.AccountSelector").ToArray();
        var displayForm = Assert.Single(displayTab.DescendantsAndSelf(), form =>
            form.Name.LocalName == "Grid" &&
            displayFieldIds.All(id => form.DescendantsAndSelf().Any(element =>
                element.Attribute("AutomationProperties.AutomationId")?.Value == id)) &&
            form.Attribute("RowSpacing")?.Value == "12");
        Assert.Equal("660", displayForm.Attribute("Width")?.Value);
        Assert.Equal("160,12,488", displayForm.Attribute("ColumnDefinitions")?.Value);
        var displayFormLeftMargin = displayForm.Attribute("Margin")?.Value.Split(',')[0] ?? "0";
        var tabStripLeftMargin = tabStrip.Attribute("Margin")?.Value?.Split(',')[0] ?? "0";
        Assert.Equal("0", displayFormLeftMargin);
        Assert.Equal(displayFormLeftMargin, tabStripLeftMargin);
        Assert.Equal("Left", tabStrip.Attribute("HorizontalAlignment")?.Value);
        Assert.Equal(new[] { "36", "36", "36", "36" },
            displayForm.Attribute("RowDefinitions")?.Value.Split(','));
        Assert.Equal(new[] { "0", "1", "2", "3" }, displayForm.Elements()
            .Where(element => element.Attribute("Grid.Row") is not null)
            .Select(element => element.Attribute("Grid.Row")!.Value)
            .Distinct()
            .OrderBy(row => row, StringComparer.Ordinal));

        var accountAnchor = Assert.Single(displayTab.DescendantsAndSelf(), element =>
            element.Attribute(XNamespace.Get("http://schemas.microsoft.com/winfx/2006/xaml") + "Name")?.Value ==
            "SettingsAccountSelectorAnchor");
        Assert.Equal("3", accountAnchor.Attribute("Grid.Row")?.Value);
        Assert.Contains(accountAnchor.Elements(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.AccountSelector");
        Assert.DoesNotContain(accountAnchor.Descendants(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.AccountMenuHost");

        var accountMenuHost = Assert.Single(displayForm.Elements(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.AccountMenuHost");
        Assert.Equal("Border", accountMenuHost.Name.LocalName);
        Assert.Equal("2", accountMenuHost.Attribute("Grid.Column")?.Value);
        Assert.Equal("0", accountMenuHost.Attribute("Grid.Row")?.Value);
        Assert.Equal("4", accountMenuHost.Attribute("Grid.RowSpan")?.Value);
        Assert.Equal("488", accountMenuHost.Attribute("Width")?.Value);
        Assert.Equal("488", accountMenuHost.Attribute("MaxWidth")?.Value);
        Assert.Equal("144", accountMenuHost.Attribute("MaxHeight")?.Value);
        Assert.Equal("Bottom", accountMenuHost.Attribute("VerticalAlignment")?.Value);
        Assert.Equal("0,0,0,36", accountMenuHost.Attribute("Margin")?.Value);
        Assert.Single(accountMenuHost.Descendants(), element =>
            element.Name.LocalName == "ListBox" &&
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.AccountMenu");

        Assert.DoesNotContain(document.Descendants(), element =>
            element.Attribute("Text")?.Value == "{Binding Texts.AppearanceDescription}");
        Assert.DoesNotContain(displayTab.Descendants(), element =>
            element.Attribute("Text")?.Value == "{Binding Texts.ConnectionEndpoint}" ||
            element.Attribute("Text")?.Value == "{Binding CurrentEndpoint}" ||
            element.Attribute("Text")?.Value == "{Binding StatusTitle}" ||
            element.Attribute("Text")?.Value == "{Binding RecorderVersion}" ||
            element.Attribute("Text")?.Value == "{Binding RestVersion}" ||
            element.Attribute("Text")?.Value == "{Binding RuntimeVersionStatus}");
        AssertExactlyOneBinding(connectionTab, "Text", "{Binding CurrentEndpoint}");
        Assert.Contains(connectionTab.Descendants(), element =>
            element.Name.LocalName == "TextBlock" &&
            element.Attribute("Text")?.Value == "{Binding Texts.SettingsConnectionStatus}");
        Assert.Contains(connectionTab.Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding StatusTitle}" &&
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.ConnectionStatus.Title");
        AssertExactlyOneBinding(connectionTab, "Click", "OnRefresh");
        AssertExactlyOneBinding(connectionTab, "AutomationProperties.AutomationId", "Settings.AuthCheck");
        AssertExactlyOneBinding(connectionTab, "Click", "OnAuth");
        AssertExactlyOneBinding(connectionTab, "Text", "{Binding RuntimeVersionStatus}");
        var recorderVersion = Assert.Single(connectionTab.Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding RecorderVersion}" &&
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.RuntimeVersion.Recorder");
        Assert.Equal("Grid", recorderVersion.Parent?.Name.LocalName);
        Assert.Contains(recorderVersion.Parent!.Elements(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "Recorder");
        var restVersion = Assert.Single(connectionTab.Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding RestVersion}" &&
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.RuntimeVersion.Rest");
        Assert.Equal("Grid", restVersion.Parent?.Name.LocalName);
        Assert.Contains(restVersion.Parent!.Elements(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "REST");

        var saveError = Assert.Single(contentRegion.Elements(), element => element.Attribute("Grid.Row")?.Value == "1");
        Assert.Equal("{Binding SaveFailed}", saveError.Attribute("IsVisible")?.Value);
        var saveErrorText = Assert.Single(saveError.Descendants(), element =>
            element.Name.LocalName == "TextBlock" && element.Attribute("Text")?.Value == "{Binding StatusDetail}");
        Assert.Equal("Settings.SaveError", saveErrorText.Attribute("AutomationProperties.AutomationId")?.Value);
        Assert.DoesNotContain(tabs.Descendants(), element =>
            element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.SaveError");

        var footer = Assert.Single(grid.Elements(), element =>
            element.Name.LocalName == "Grid" && element.Attribute("Grid.Row")?.Value == "3");
        Assert.Equal("660", footer.Attribute("Width")?.Value);
        Assert.Equal("Center", footer.Attribute("HorizontalAlignment")?.Value);
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
