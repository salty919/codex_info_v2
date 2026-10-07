// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections;
using System.Reflection;
using System.Text.Json;
using System.Xml.Linq;
using Avalonia.Media;
using CodexInfo.WindowsClient;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.Theme;
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
    public void SettingsExposeSixteenThemePresets()
    {
        var root = Directory.CreateTempSubdirectory("codex-info-issue-422-theme-options-test");
        var originalLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            var expectedIds = new[]
            {
                "classic-dark", "graphite-dark", "light", "paper-light", "sand-light",
                "steel-light", "ocean-dark", "teal-dark", "ember-dark", "ink-dark",
                "neon-dark", "lavender-light", "mint-light", "forest-dark", "tangerine-dark", "rose-dark",
            };
            var store = new ClientSettingsStore(Path.Combine(root.FullName, "settings.json"));
            foreach (var language in LocalizationService.Languages)
            {
                LocalizationService.SetLanguage(language.LanguageCode);
                var viewModel = new SettingsViewModel(store);
                try
                {
                    var themeOptionsProperty = typeof(SettingsViewModel).GetProperty(
                        "ThemeOptions",
                        BindingFlags.Instance | BindingFlags.Public);
                    Assert.NotNull(themeOptionsProperty);
                    var themeOptionsValue = themeOptionsProperty.GetValue(viewModel);
                    Assert.NotNull(themeOptionsValue);
                    var themeOptions = Assert.IsAssignableFrom<IEnumerable>(themeOptionsValue)
                        .Cast<object>()
                        .ToArray();
                    Assert.Equal(expectedIds, themeOptions.Select(ReadThemeOptionId));
                    var labels = themeOptions.Select(ReadThemeOptionLabel).ToArray();
                    Assert.All(labels, label => Assert.False(string.IsNullOrWhiteSpace(label)));
                    Assert.Equal(labels.Length, labels.Distinct(StringComparer.Ordinal).Count());
                }
                finally
                {
                    viewModel.Dispose();
                }
            }

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
            LocalizationService.SetLanguage(originalLanguage);
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public void EveryThemeMapsAllClassicColorKeys()
    {
        var originalTheme = ThemePalette.CurrentId;
        try
        {
            Assert.Equal(72, ClassicColors.Length);
            Assert.Equal(72, ClassicColors.Select(color => color.Classic).Distinct(StringComparer.OrdinalIgnoreCase).Count());

            ThemePalette.Apply("classic-dark");
            Assert.Equal("#0E141E", ThemePalette.Resolve("#0E141E"));
            Assert.Equal("#1A2C40", ThemePalette.Resolve("#243E5A"));
            ThemePalette.Apply("graphite-dark");
            Assert.Equal("#181A1F", ThemePalette.Resolve("#0E141E"));
            ThemePalette.Apply("light");
            Assert.Equal("#F4F7FB", ThemePalette.Resolve("#0E141E"));

            foreach (var theme in NewThemeFixtures)
            {
                Assert.True(ThemePalette.IsValid(theme.Id), $"ThemePalette does not recognize {theme.Id}.");
                ThemePalette.Apply(theme.Id);
                var overrides = RoleKeys
                    .Select((key, index) => (key, value: theme.RoleColors[index]))
                    .ToDictionary(pair => pair.key, pair => pair.value, StringComparer.OrdinalIgnoreCase);
                foreach (var color in ClassicColors)
                {
                    var expected = overrides.TryGetValue(color.Classic, out var exact)
                        ? exact
                        : TransformColor(theme, color);
                    Assert.Equal(expected, ThemePalette.Resolve(color.Classic));
                }
            }

            Assert.Throws<ArgumentOutOfRangeException>(() => ThemePalette.Resolve("#010203"));
        }
        finally
        {
            ThemePalette.Apply(originalTheme);
        }
    }

    [Fact]
    public void GraphRoleResourcesUseFixedDarkAndLightColorsAndContrastWithPlotAndIdleAcrossAllThemes()
    {
        var originalTheme = ThemePalette.CurrentId;
        var lightThemes = new HashSet<string>(StringComparer.Ordinal)
        {
            "light", "paper-light", "sand-light", "steel-light", "lavender-light", "mint-light",
        };
        var darkRoles = new[]
        {
            (Key: "GraphRemaining", Color: "#56B2F5"),
            (Key: "GraphLuna", Color: "#E6A23C"),
            (Key: "GraphTerra", Color: "#5DC98A"),
            (Key: "GraphSol", Color: "#A88CF5"),
            (Key: "GraphAstra", Color: "#EF6A6A"),
        };
        var lightRoles = new[]
        {
            (Key: "GraphRemaining", Color: "#176AAB"),
            (Key: "GraphLuna", Color: "#985F08"),
            (Key: "GraphTerra", Color: "#16794B"),
            (Key: "GraphSol", Color: "#6A4BCC"),
            (Key: "GraphAstra", Color: "#B23553"),
        };

        try
        {
            Assert.Equal(16, ThemePalette.PresetIds.Count);
            foreach (var theme in ThemePalette.PresetIds)
            {
                ThemePalette.Apply(theme);
                var roles = lightThemes.Contains(theme) ? lightRoles : darkRoles;
                var actualRoles = roles
                    .Select(role => (role.Key, Expected: role.Color, Actual: ReadGraphRoleResource(role.Key)))
                    .ToArray();
                Assert.Equal(5, actualRoles.Select(role => role.Actual).Distinct(StringComparer.OrdinalIgnoreCase).Count());

                var plot = ThemePalette.Resolve(GraphPlotControl.PlotColorHex);
                var idleBaseline = ThemePalette.Resolve(GraphPlotControl.IdleBandColorHex);
                var idle = BlendHalfUp(plot, idleBaseline);
                foreach (var role in actualRoles)
                {
                    Assert.Equal(role.Expected, role.Actual);
                    AssertGraphContrast(theme, role.Key, role.Actual, plot, "plot background");
                    AssertGraphContrast(theme, role.Key, role.Actual, idle, "idle background");
                }
            }
        }
        finally
        {
            ThemePalette.Apply(originalTheme);
        }
    }

    [Fact]
    public void NewThemeIdsSaveAndReloadWithoutChangingSevenKeySchema()
    {
        var root = Directory.CreateTempSubdirectory("codex-info-issue-422-theme-persistence-test");
        try
        {
            var ids = new[]
            {
                "paper-light", "sand-light", "steel-light", "ocean-dark", "teal-dark", "ember-dark", "ink-dark",
                "neon-dark", "lavender-light", "mint-light", "forest-dark", "tangerine-dark", "rose-dark",
            };
            foreach (var id in ids)
            {
                Assert.True(ThemePalette.IsValid(id), $"Theme id {id} is not accepted.");
                var path = Path.Combine(root.FullName, $"{id}.json");
                var store = new ClientSettingsStore(path);
                store.Save(new ClientSettings("ja", true) { ThemeId = id });
                using var document = JsonDocument.Parse(File.ReadAllText(path));
                Assert.Equal(
                    ["connectionConfigured", "connectionProfile", "connectionSelector", "language", "setupCompleted", "themeId", "timeZoneId"],
                    document.RootElement.EnumerateObject()
                        .Select(property => property.Name)
                        .OrderBy(name => name, StringComparer.Ordinal));
                Assert.Equal(id, document.RootElement.GetProperty("themeId").GetString());
                Assert.Equal(id, ReadStringProperty(store.Load(), "ThemeId"));
            }
        }
        finally
        {
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
            Assert.Equal("classic-dark", ThemePalette.CurrentId);
            selectedThemeProperty.SetValue(cancelledViewModel, "paper-light");
            Assert.Equal("classic-dark", ReadStringProperty(App.CurrentSettings, "ThemeId"));
            Assert.Equal("classic-dark", ThemePalette.CurrentId);
            cancelledViewModel.Dispose();
            cancelledViewModel = null;
            Assert.Equal("classic-dark", ReadStringProperty(App.CurrentSettings, "ThemeId"));
            Assert.Equal("classic-dark", ReadStringProperty(store.Load(), "ThemeId"));
            Assert.Equal("classic-dark", ThemePalette.CurrentId);

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
            Assert.Equal("light", ThemePalette.CurrentId);
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
            failedSelectedThemeProperty.SetValue(failedViewModel, "ink-dark");
            var savedBytesBeforeFailure = File.ReadAllBytes(path);

            Assert.False(failedViewModel.Save());
            Assert.True(failedViewModel.SaveFailed);
            Assert.Equal("light", ReadStringProperty(App.CurrentSettings, "ThemeId"));
            Assert.Equal("light", ReadStringProperty(store.Load(), "ThemeId"));
            Assert.Equal("light", ThemePalette.CurrentId);
            Assert.Equal(savedBytesBeforeFailure, File.ReadAllBytes(path));
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

    [Fact]
    public void ColorfulThemesKeepTextAndGraphContrast()
    {
        var original = ThemePalette.CurrentId;
        try
        {
            foreach (var id in new[] { "neon-dark", "lavender-light", "mint-light", "forest-dark", "tangerine-dark", "rose-dark" })
            {
                Assert.True(ThemePalette.IsValid(id), $"Missing requested theme: {id}");
                ThemePalette.Apply(id);
                foreach (var surface in new[] { "#0E141E", "#151F2D", "#243E5A", "#111B2C", "#244D74" })
                {
                    AssertContrast(id, "#E9EFF8", surface, 4.5);
                    AssertContrast(id, "#A8B7CA", surface, 4.5);
                    AssertContrast(id, "#8BD4FF", surface, 3);
                }
                AssertContrast(id, "#4FB878", "#143426", 4.5);
                AssertContrast(id, "#E6B85C", "#3A2A13", 4.5);
                AssertContrast(id, "#E06B7A", "#3A1D24", 4.5);
                AssertContrast(id, "#EF6A6A", "#151F2D", 4.5);
                AssertContrast(id, "#EF6A6A", "#243E5A", 4.5);
                AssertContrast(id, "#F5FAFF", "#236B9E", 4.5);
                AssertContrast(id, "#78879C", "#121C2C", 4.5);
                foreach (var series in new[] { "#56B2F5", "#A88CF5", "#5DC98A", "#E6A23C", "#EF6A6A" })
                {
                    AssertContrast(id, series, "#121C2C", 3);
                }
            }
        }
        finally
        {
            ThemePalette.Apply(original);
        }
    }

    [Theory]
    [InlineData("classic-dark", "#EF6A6A", "#243E5A")]
    [InlineData("graphite-dark", "#EF6A6A", "#243E5A")]
    [InlineData("sand-light", "#A8B7CA", "#243E5A")]
    [InlineData("teal-dark", "#EF6A6A", "#243E5A")]
    [InlineData("teal-dark", "#F5FAFF", "#236B9E")]
    public void ExistingThemeTextContrastRegressions(string id, string foreground, string background)
    {
        var original = ThemePalette.CurrentId;
        try
        {
            ThemePalette.Apply(id);
            AssertContrast(id, foreground, background, 4.5);
        }
        finally
        {
            ThemePalette.Apply(original);
        }
    }

    private static void AssertContrast(string id, string foreground, string background, double minimum)
    {
        static double Luminance(string hex)
        {
            static double Linear(int channel)
            {
                var value = channel / 255.0;
                return value <= 0.04045 ? value / 12.92 : Math.Pow((value + 0.055) / 1.055, 2.4);
            }
            return 0.2126 * Linear(Convert.ToInt32(hex.Substring(1, 2), 16))
                + 0.7152 * Linear(Convert.ToInt32(hex.Substring(3, 2), 16))
                + 0.0722 * Linear(Convert.ToInt32(hex.Substring(5, 2), 16));
        }
        var a = Luminance(ThemePalette.Resolve(foreground));
        var b = Luminance(ThemePalette.Resolve(background));
        var contrast = (Math.Max(a, b) + 0.05) / (Math.Min(a, b) + 0.05);
        Assert.True(contrast >= minimum, $"{id}: {foreground}/{background} contrast {contrast:F2} < {minimum}");
    }

    private static string ReadGraphRoleResource(string key)
    {
        var brush = Assert.IsType<SolidColorBrush>(ThemePalette.Brush(key));
        return $"#{brush.Color.R:X2}{brush.Color.G:X2}{brush.Color.B:X2}";
    }

    private static void AssertGraphContrast(string theme, string role, string foreground, string background, string surface)
    {
        var contrast = ContrastRatio(foreground, background);
        Assert.True(contrast >= 3,
            $"{theme}: {role} {foreground} has {contrast:F2}:1 contrast against {surface} {background}; expected >= 3:1.");
    }

    private static double ContrastRatio(string first, string second)
    {
        static double Luminance(string hex)
        {
            static double Linear(int channel)
            {
                var value = channel / 255.0;
                return value <= 0.04045 ? value / 12.92 : Math.Pow((value + 0.055) / 1.055, 2.4);
            }

            return 0.2126 * Linear(Convert.ToInt32(hex.Substring(1, 2), 16))
                + 0.7152 * Linear(Convert.ToInt32(hex.Substring(3, 2), 16))
                + 0.0722 * Linear(Convert.ToInt32(hex.Substring(5, 2), 16));
        }

        var a = Luminance(first);
        var b = Luminance(second);
        return (Math.Max(a, b) + 0.05) / (Math.Min(a, b) + 0.05);
    }

    private static string BlendHalfUp(string first, string second)
    {
        static int Channel(string color, int offset) => Convert.ToInt32(color.Substring(offset, 2), 16);
        static int Average(int left, int right) => (left + right + 1) / 2;

        return $"#{Average(Channel(first, 1), Channel(second, 1)):X2}" +
            $"{Average(Channel(first, 3), Channel(second, 3)):X2}" +
            $"{Average(Channel(first, 5), Channel(second, 5)):X2}";
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

    private static string ReadThemeOptionLabel(object option)
    {
        var labelProperty = option.GetType().GetProperty("Label", BindingFlags.Instance | BindingFlags.Public);
        Assert.NotNull(labelProperty);
        return Assert.IsType<string>(labelProperty.GetValue(option));
    }

    private static string TransformColor(ThemeFixture theme, ClassicColor color)
    {
        if (theme.Id == "teal-dark" && color.Classic == "#236B9E") return "#0D759F";
        if (theme.Id == "ink-dark")
        {
            var channels = ParseChannels(color.Graphite)
                .Select(channel => channel < 128 ? 3 * channel / 4 : Math.Min(255, 5 * channel / 4));
            return FormatColor(channels);
        }

        var source = ParseChannels(theme.UsesLightBase ? color.Light : color.Graphite);
        var deltas = new[] { theme.RedDelta, theme.GreenDelta, theme.BlueDelta };
        return FormatColor(source.Zip(deltas, (channel, delta) => Math.Clamp(channel + delta, 0, 255)));
    }

    private static int[] ParseChannels(string color) =>
    [
        Convert.ToInt32(color.Substring(1, 2), 16),
        Convert.ToInt32(color.Substring(3, 2), 16),
        Convert.ToInt32(color.Substring(5, 2), 16),
    ];

    private static string FormatColor(IEnumerable<int> channels) =>
        "#" + string.Concat(channels.Select(channel => channel.ToString("X2", System.Globalization.CultureInfo.InvariantCulture)));

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

    private sealed record ClassicColor(string Classic, string Graphite, string Light);

    private sealed record ThemeFixture(
        string Id,
        bool UsesLightBase,
        int RedDelta,
        int GreenDelta,
        int BlueDelta,
        string[] RoleColors);

    private static readonly string[] RoleKeys =
    [
        "#0E141E", "#151F2D", "#E9EFF8", "#A8B7CA", "#56B2F5", "#121C2C", "#263850", "#1A2838",
        "#326799", "#143426", "#276C49", "#4FB878", "#243E5A", "#2B425B", "#76A7CC", "#EF6A6A",
        "#111B2C", "#244D74", "#8BD4FF",
    ];

    // Fixed oracle for all 72 existing classic keys and their two documented transform bases.
    // Values are captured here as test data; this oracle does not read ThemePalette.Colors.
    private static readonly ClassicColor[] ClassicColors =
    [
        new("#0A111A", "#15171B", "#F1F4F8"),
        new("#0E141E", "#181A1F", "#F4F7FB"),
        new("#101925", "#1D2026", "#F0F4F8"),
        new("#101F2D", "#20242B", "#EAF0F6"),
        new("#111A27", "#20232A", "#E9EFF6"),
        new("#111B2C", "#222730", "#EEF3F8"),
        new("#121C2C", "#20242B", "#FFFFFF"),
        new("#143426", "#18362A", "#E5F5EC"),
        new("#151F2D", "#242830", "#FFFFFF"),
        new("#172C42", "#1D354A", "#E7F1FA"),
        new("#18283A", "#292F39", "#EAF0F6"),
        new("#1A2838", "#303944", "#E4EDF5"),
        new("#1B2735", "#292D34", "#EBEFF3"),
        new("#1B2838", "#292F39", "#EAF0F6"),
        new("#1B2A3A", "#2B313A", "#E8EEF5"),
        new("#1C5D91", "#285A80", "#D7EAF7"),
        new("#1D2A38", "#292D34", "#EDF1F5"),
        new("#236B9E", "#2676A8", "#176AAB"),
        new("#243E5A", "#303844", "#DDEAF5"),
        new("#24415E", "#30485D", "#D7EAF7"),
        new("#244D74", "#344D63", "#D9EBF8"),
        new("#263548", "#3A444F", "#D5DFE9"),
        new("#263850", "#3C4652", "#CFD9E4"),
        new("#276C49", "#327653", "#4A9469"),
        new("#294968", "#35506A", "#D9E9F7"),
        new("#2A3A4B", "#454C55", "#C7D0DA"),
        new("#2B425B", "#4B5A6B", "#B6C5D4"),
        new("#2C4258", "#424B57", "#C5D0DC"),
        new("#2D3D56", "#444F5C", "#C0CDDA"),
        new("#2D6193", "#3972A0", "#5A91C2"),
        new("#304052", "#454C55", "#C7D0DA"),
        new("#304A63", "#4B596B", "#B6C5D4"),
        new("#326799", "#4A6B89", "#A9CDE8"),
        new("#356087", "#3D617F", "#C7E0F4"),
        new("#36516B", "#506071", "#B8C8D7"),
        new("#3A1D24", "#41262B", "#FDECEF"),
        new("#3A2A13", "#42331B", "#FFF3DC"),
        new("#3B506F", "#536073", "#B6C5D4"),
        new("#405779", "#54677C", "#AEC1D2"),
        new("#47769F", "#5B7E9E", "#82A9CB"),
        new("#4FB878", "#5CC88A", "#176E42"),
        new("#56B2F5", "#69B5F7", "#176AAB"),
        new("#5DC98A", "#70CF99", "#16794B"),
        new("#5EA7E5", "#6EB5E9", "#267DB7"),
        new("#61B4EB", "#72BDEA", "#267DB7"),
        new("#718196", "#9AA8B8", "#5F7183"),
        new("#71C7FF", "#84C7F3", "#176AAB"),
        new("#71D39A", "#83D4A7", "#16794B"),
        new("#76A7CC", "#8CACBF", "#6B839A"),
        new("#78879C", "#A2ADB8", "#526579"),
        new("#8A651F", "#A98037", "#B8892D"),
        new("#8BD4FF", "#9AD7F8", "#176AAB"),
        new("#8E3D4D", "#AB5362", "#C77382"),
        new("#8EA1B5", "#9DAAB6", "#687A8A"),
        new("#9FB0C5", "#B5BEC9", "#526579"),
        new("#A88CF5", "#B69CF1", "#6A4BCC"),
        new("#A8B7CA", "#B5BEC9", "#526579"),
        new("#B79BFF", "#C5AFF5", "#6A4BCC"),
        new("#B8C6D7", "#C9D0DA", "#526579"),
        new("#D5A43A", "#E0B255", "#805800"),
        new("#E06B7A", "#E5808C", "#A32746"),
        new("#E6A23C", "#EBB460", "#985F08"),
        new("#E6B85C", "#ECC779", "#805800"),
        new("#E7C26A", "#EBCF89", "#805800"),
        new("#E86E9F", "#EC87AC", "#B23553"),
        new("#E9EFF8", "#F1F3F5", "#1C2834"),
        new("#EAF2FB", "#F2F4F7", "#1C2834"),
        new("#EF6A6A", "#EF8585", "#B23553"),
        new("#F0A35B", "#F2BA82", "#985F08"),
        new("#F1B35A", "#EFC176", "#985F08"),
        new("#F2F6FC", "#F7F8FA", "#1C2834"),
        new("#F5FAFF", "#F8FBFF", "#FFFFFF"),
    ];

    // WIN-THEME-422's 19 exact role overrides, in RoleKeys order.
    private static readonly ThemeFixture[] NewThemeFixtures =
    [
        new("paper-light", true, 3, 1, -5,
            ["#F7F6F2", "#FFFFFC", "#252B31", "#59636B", "#356C91", "#FFFFFC", "#D8E0E5", "#ECEFEB", "#B7CDD8", "#E6F3EB", "#5C9976", "#216543", "#E2EDF2", "#B7C7D0", "#708D9E", "#B23553", "#F1F4F1", "#DAE9EE", "#276A91"]),
        new("sand-light", true, 10, 3, -22,
            ["#FDF6E3", "#FFFBEF", "#334650", "#566367", "#1B748A", "#FFFBEF", "#C9D6D2", "#EBE4D2", "#B3C9C5", "#E3F0E2", "#6D9B72", "#2B714A", "#E1E9D9", "#BCCBBC", "#728D84", "#A83D48", "#F5EDDA", "#DCE8DB", "#126A7F"]),
        new("steel-light", true, -3, -1, 3,
            ["#F3F5F8", "#FFFFFF", "#202B38", "#586978", "#275FA8", "#FFFFFF", "#D5DEE9", "#E8EEF5", "#A9C4E1", "#E5F2EA", "#6EAA83", "#1E7047", "#DDE9F6", "#BACBDD", "#728BA9", "#B42F49", "#EDF2F8", "#D7E5F7", "#275FA8"]),
        new("ocean-dark", false, -8, 0, 14,
            ["#10182A", "#18263D", "#EAF3FF", "#ACBED3", "#56B8F2", "#111D33", "#304968", "#1E304B", "#3D668B", "#16372F", "#3A8266", "#72CDA3", "#213958", "#45617F", "#82A9C5", "#F28B9B", "#192B45", "#284B70", "#7CD2FF"]),
        new("teal-dark", false, -22, 14, 13,
            ["#002B36", "#073642", "#E6F0E9", "#A8C0BC", "#4FB3C3", "#073642", "#3C6570", "#174550", "#3C7583", "#124B40", "#47866A", "#79CAA3", "#123A46", "#3D6C75", "#76AEB3", "#F47D88", "#0B3D49", "#1E5B67", "#74D2DB"]),
        new("ember-dark", false, 11, 5, -6,
            ["#202126", "#2B2D32", "#F4F0E9", "#BCBDB7", "#E7BC62", "#26272C", "#505258", "#35373D", "#77725F", "#244437", "#5D9974", "#9FDC9E", "#3D3D47", "#686973", "#A5A3A0", "#F58A94", "#303238", "#555147", "#F2CB78"]),
        new("ink-dark", false, 0, 0, 0,
            ["#000000", "#121212", "#FFFFFF", "#D8D8D8", "#6DD3FF", "#050505", "#787878", "#242424", "#808080", "#002B17", "#78E8A4", "#78E8A4", "#202A34", "#FFFFFF", "#FFFFFF", "#FF8BA1", "#101010", "#174A66", "#FFFFFF"]),
        new("neon-dark", false, 0, 0, 0,
            ["#16122A", "#241C3B", "#F2ECFF", "#C4B8E2", "#70CBFF", "#1B1530", "#493B68", "#302448", "#37436A", "#163D33", "#4D8B70", "#8BDEB6", "#33264F", "#63517D", "#AF9AD0", "#FF929F", "#2C2144", "#493369", "#B19BFF"]),
        new("lavender-light", true, 0, 0, 0,
            ["#F2EAFB", "#FFFAFF", "#29233C", "#615570", "#7046AE", "#FFFAFF", "#D8CBE3", "#EBE0F4", "#DDD4EF", "#E3F3E9", "#5C9571", "#216C44", "#E7DDF5", "#BCAACE", "#78658F", "#AC2853", "#EFE7F8", "#DDD0EF", "#7046AE"]),
        new("mint-light", true, 0, 0, 0,
            ["#E8F7EE", "#F7FFFA", "#19382F", "#3F5F50", "#14765F", "#F7FFFA", "#C8DECF", "#DDEDE3", "#BEDCCD", "#D9F2E1", "#579772", "#207343", "#D7EDE0", "#A8C8B5", "#4D806C", "#AF2D4C", "#E8F6EC", "#C8E8D6", "#11735B"]),
        new("forest-dark", false, 0, 0, 0,
            ["#11231B", "#1B3427", "#EDF8EA", "#ADC9B5", "#94DB75", "#14291E", "#355642", "#263F30", "#40623A", "#193E29", "#508264", "#8CDCAC", "#284833", "#526F5B", "#8BAF90", "#FF949B", "#203F2E", "#31533A", "#B5EA94"]),
        new("tangerine-dark", false, 0, 0, 0,
            ["#29180F", "#3B261A", "#FFF3E5", "#DFC1A5", "#FFB46E", "#2C1D13", "#614533", "#453022", "#735035", "#213D28", "#567D59", "#A3D892", "#4D3424", "#876346", "#D3A37A", "#FF9A96", "#3D2B1D", "#68462C", "#FFCA86"]),
        new("rose-dark", false, 0, 0, 0,
            ["#281523", "#3A2233", "#FCECF5", "#DAB9CD", "#F49DC7", "#2D1B29", "#604258", "#482D3F", "#704762", "#1D3D32", "#507E68", "#9CDBBC", "#4C2F44", "#835A75", "#CC94B5", "#FF969F", "#412838", "#67425B", "#FFBDDF"]),
    ];
}
