// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections;
using System.Reflection;
using System.Text.Json;
using System.Xml.Linq;
using CodexInfo.WindowsClient;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class Issue422ThemeTests
{
    [Fact]
    public void LegacySixKeySettingsLoadAndSaveAsSevenKeyClassicDark()
    {
        var root = Directory.CreateTempSubdirectory("codex-info-issue-422-legacy-settings-test");
        try
        {
            var path = Path.Combine(root.FullName, "settings.json");
            File.WriteAllText(
                path,
                "{\"language\":\"ja\",\"setupCompleted\":true,\"connectionConfigured\":true,\"timeZoneId\":\"UTC\",\"connectionProfile\":\"sshConfigAlias\",\"connectionSelector\":\"work.example\"}");
            var store = new ClientSettingsStore(path);

            var loaded = store.Load();
            Assert.Equal("ja", loaded.Language);
            Assert.True(loaded.SetupCompleted);
            Assert.True(loaded.ConnectionConfigured);
            Assert.Equal("UTC", loaded.TimeZoneId);
            Assert.Equal("sshConfigAlias", loaded.ConnectionProfile);
            Assert.Equal("work.example", loaded.ConnectionSelector);
            Assert.False(loaded.SettingsCorrupt);

            store.Save(loaded);

            using var document = JsonDocument.Parse(File.ReadAllText(path));
            Assert.Equal(
                ["connectionConfigured", "connectionProfile", "connectionSelector", "language", "setupCompleted", "themeId", "timeZoneId"],
                document.RootElement.EnumerateObject()
                    .Select(property => property.Name)
                    .OrderBy(name => name, StringComparer.Ordinal));
            Assert.Equal("classic-dark", document.RootElement.GetProperty("themeId").GetString());

            var reloaded = store.Load();
            Assert.Equal("classic-dark", ReadStringProperty(reloaded, "ThemeId"));
        }
        finally
        {
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public void SettingsExposeExactlyThreeThemePresets()
    {
        var root = Directory.CreateTempSubdirectory("codex-info-issue-422-theme-options-test");
        SettingsViewModel? viewModel = null;
        try
        {
            viewModel = new SettingsViewModel(new ClientSettingsStore(Path.Combine(root.FullName, "settings.json")));

            var themeOptionsProperty = typeof(SettingsViewModel).GetProperty(
                "ThemeOptions",
                BindingFlags.Instance | BindingFlags.Public);
            Assert.NotNull(themeOptionsProperty);
            var themeOptionsValue = themeOptionsProperty.GetValue(viewModel);
            Assert.NotNull(themeOptionsValue);
            var themeOptions = Assert.IsAssignableFrom<IEnumerable>(themeOptionsValue);
            var themeIds = themeOptions.Cast<object>().Select(ReadThemeOptionId).ToArray();
            Assert.Equal(["classic-dark", "graphite-dark", "light"], themeIds);

            var settingsWindow = XDocument.Parse(LoadRepositoryFile(
                "windows-client", "src", "CodexInfo.WindowsClient", "SettingsWindow.axaml"));
            var themeSelector = settingsWindow.Descendants().SingleOrDefault(element =>
                element.Name.LocalName == "ComboBox" &&
                element.Attribute("AutomationProperties.AutomationId")?.Value == "Settings.ThemeSelector");
            Assert.NotNull(themeSelector);
            Assert.Equal("{Binding ThemeOptions}", themeSelector.Attribute("ItemsSource")?.Value);
            Assert.Contains(themeSelector.Attributes(), attribute =>
                attribute.Name.LocalName is "SelectedItem" or "SelectedValue" &&
                attribute.Value == "{Binding SelectedThemeId}");
        }
        finally
        {
            viewModel?.Dispose();
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public void ThemeSelectionPublishesOnlyAfterSuccessfulSave()
    {
        var originalSettings = App.CurrentSettings;
        var originalLanguage = LocalizationService.Current.LanguageCode;
        var originalTimeZone = LocalizationService.DisplayTimeZone.Id;
        var root = Directory.CreateTempSubdirectory("codex-info-issue-422-theme-save-test");
        SettingsViewModel? cancelledViewModel = null;
        SettingsViewModel? successfulViewModel = null;
        SettingsViewModel? failedViewModel = null;
        try
        {
            var path = Path.Combine(root.FullName, "settings.json");
            var store = new ClientSettingsStore(path);
            store.Save(new ClientSettings("ja", true));
            App.CurrentSettings = store.Load();
            LocalizationService.SetLanguage("ja");
            LocalizationService.SetTimeZone("local");

            cancelledViewModel = new SettingsViewModel(store);
            var selectedThemeProperty = typeof(SettingsViewModel).GetProperty(
                "SelectedThemeId",
                BindingFlags.Instance | BindingFlags.Public);
            Assert.NotNull(selectedThemeProperty);
            Assert.Equal("classic-dark", ReadStringProperty(cancelledViewModel, "SelectedThemeId"));
            selectedThemeProperty.SetValue(cancelledViewModel, "graphite-dark");
            Assert.Equal("classic-dark", ReadStringProperty(App.CurrentSettings, "ThemeId"));
            cancelledViewModel.Dispose();
            cancelledViewModel = null;
            Assert.Equal("classic-dark", ReadStringProperty(App.CurrentSettings, "ThemeId"));
            Assert.Equal("classic-dark", ReadStringProperty(store.Load(), "ThemeId"));

            successfulViewModel = new SettingsViewModel(store);
            var successfulSelectedThemeProperty = typeof(SettingsViewModel).GetProperty(
                "SelectedThemeId",
                BindingFlags.Instance | BindingFlags.Public);
            Assert.NotNull(successfulSelectedThemeProperty);
            successfulSelectedThemeProperty.SetValue(successfulViewModel, "light");
            Assert.Equal("classic-dark", ReadStringProperty(App.CurrentSettings, "ThemeId"));
            Assert.True(successfulViewModel.Save());
            Assert.Equal("light", ReadStringProperty(App.CurrentSettings, "ThemeId"));
            Assert.Equal("light", ReadStringProperty(store.Load(), "ThemeId"));
            using (var savedDocument = JsonDocument.Parse(File.ReadAllText(path)))
            {
                Assert.Equal("light", savedDocument.RootElement.GetProperty("themeId").GetString());
            }

            successfulViewModel.Dispose();
            successfulViewModel = null;

            var blocker = Path.Combine(root.FullName, "not-a-directory");
            File.WriteAllText(blocker, "blocked");
            var blockedStore = new ClientSettingsStore(Path.Combine(blocker, "settings.json"));
            failedViewModel = new SettingsViewModel(blockedStore);
            var failedSelectedThemeProperty = typeof(SettingsViewModel).GetProperty(
                "SelectedThemeId",
                BindingFlags.Instance | BindingFlags.Public);
            Assert.NotNull(failedSelectedThemeProperty);
            failedSelectedThemeProperty.SetValue(failedViewModel, "light");

            Assert.False(failedViewModel.Save());
            Assert.True(failedViewModel.SaveFailed);
            Assert.Equal("light", ReadStringProperty(App.CurrentSettings, "ThemeId"));
            Assert.Equal("light", ReadStringProperty(store.Load(), "ThemeId"));
        }
        finally
        {
            cancelledViewModel?.Dispose();
            successfulViewModel?.Dispose();
            failedViewModel?.Dispose();
            App.CurrentSettings = originalSettings;
            LocalizationService.SetLanguage(originalLanguage);
            LocalizationService.SetTimeZone(string.Equals(originalTimeZone, "UTC", StringComparison.OrdinalIgnoreCase) ? "UTC" : "local");
            root.Delete(recursive: true);
        }
    }

    private static string ReadThemeOptionId(object option)
    {
        if (option is string id)
        {
            return id;
        }

        var idProperty = option.GetType().GetProperty("Id", BindingFlags.Instance | BindingFlags.Public)
            ?? option.GetType().GetProperty("ThemeId", BindingFlags.Instance | BindingFlags.Public);
        Assert.NotNull(idProperty);
        return Assert.IsType<string>(idProperty.GetValue(option));
    }

    private static string ReadStringProperty(object instance, string propertyName)
    {
        var property = instance.GetType().GetProperty(propertyName, BindingFlags.Instance | BindingFlags.Public);
        Assert.NotNull(property);
        return Assert.IsType<string>(property.GetValue(instance));
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
