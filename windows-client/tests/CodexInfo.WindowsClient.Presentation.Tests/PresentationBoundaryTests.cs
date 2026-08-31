// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Diagnostics;
using System.Xml.Linq;
using CodexInfo.WindowsClient;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

[assembly: CollectionBehavior(DisableTestParallelization = true)]

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class PresentationBoundaryTests
{
    [Fact]
    public void UiTextRemainingBoundariesHaveExplicitJapaneseMeaning()
    {
        var text = LocalizationService.Languages.Single(language => language.LanguageCode == "ja");

        Assert.Equal("まもなくリセット", text.FormatRemaining(0, 0, 0));
        Assert.Equal("まもなくリセット", text.FormatRemaining(0, 0, 0, immediate: true));
        Assert.Equal("残り 1分未満", text.FormatRemaining(0, 0, 0, lessThanMinute: true));
        Assert.Equal("残り 2日 3時間 4分", text.FormatRemaining(2, 3, 4));
        Assert.Equal("残り 2日", text.FormatRemaining(2, 0, 0));
    }

    [Fact]
    public void UiTextStatusDetailsDistinguishLaunchFailureAndRetainedSnapshot()
    {
        var text = LocalizationService.Languages.Single(language => language.LanguageCode == "en");

        Assert.Contains("Could not start authentication", text.StatusDetailFor("AuthRequired", true, false), StringComparison.Ordinal);
        Assert.Contains("Start Linux authentication", text.StatusDetailFor("AuthRequired", false, false), StringComparison.Ordinal);
        Assert.DoesNotContain("last received", text.StatusDetailFor("TransportError", false, false), StringComparison.OrdinalIgnoreCase);
        Assert.Contains("last received", text.StatusDetailFor("TransportError", false, true), StringComparison.OrdinalIgnoreCase);
        Assert.Contains("valid response", text.StatusDetailFor("ResponseError", false, false), StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void UiTextCatalogIsUniqueAndEveryLocaleHasCoreLabels()
    {
        var languages = LocalizationService.Languages;

        Assert.Equal(languages.Count, languages.Select(language => language.LanguageCode).Distinct(StringComparer.Ordinal).Count());
        Assert.All(languages, language =>
        {
            Assert.False(string.IsNullOrWhiteSpace(language.LanguageName));
            Assert.False(string.IsNullOrWhiteSpace(language.UsageStatus));
            Assert.False(string.IsNullOrWhiteSpace(language.Refresh));
            Assert.False(string.IsNullOrWhiteSpace(language.UnavailableValue));
            Assert.False(string.IsNullOrWhiteSpace(language.ConnectionEndpoint));
        });
    }

    [Fact]
    public void UiTextPeriodLabelsNameResetAndDistinguishWeeklyAndMonthly()
    {
        var japanese = LocalizationService.Languages.Single(language => language.LanguageCode == "ja");

        Assert.Equal("7日周期：リセットまで", japanese.WeeklyQuota);
        Assert.Equal("月間：リセットまで", japanese.MonthlyQuota);
        Assert.Contains("リセット", japanese.WeeklyQuota, StringComparison.Ordinal);
        Assert.Contains("リセット", japanese.MonthlyQuota, StringComparison.Ordinal);
        Assert.NotEqual(japanese.WeeklyQuota, japanese.MonthlyQuota);
        Assert.DoesNotContain("利用枠", japanese.WeeklyQuota, StringComparison.Ordinal);
        Assert.DoesNotContain("利用枠", japanese.MonthlyQuota, StringComparison.Ordinal);
    }

    [Fact]
    public void SettingsViewModelSaveNormalizesLocaleTimezoneAndRaisesSaved()
    {
        var originalSettings = App.CurrentSettings;
        var originalLanguage = LocalizationService.Current.LanguageCode;
        var originalTimeZone = LocalizationService.DisplayTimeZone.Id;
        var root = Directory.CreateTempSubdirectory("codex-info-settings-vm-test");
        SettingsViewModel? viewModel = null;
        try
        {
            var path = Path.Combine(root.FullName, "settings.json");
            var store = new ClientSettingsStore(path);
            store.Save(new ClientSettings("ja", true));
            App.CurrentSettings = store.Load();
            LocalizationService.SetLanguage("ja");
            LocalizationService.SetTimeZone("local");

            viewModel = new SettingsViewModel(store);
            var savedCount = 0;
            viewModel.Saved += (_, _) => savedCount++;
            viewModel.SelectedLanguageCode = "en-US";
            viewModel.SelectedTimeZoneId = "utc";

            viewModel.Save();

            var loaded = store.Load();
            Assert.Equal("en", loaded.Language);
            Assert.Equal("UTC", loaded.TimeZoneId);
            Assert.Equal(loaded, App.CurrentSettings);
            Assert.Equal(1, savedCount);
            Assert.Equal("en", LocalizationService.Current.LanguageCode);
            Assert.Equal("UTC", LocalizationService.DisplayTimeZone.Id);
        }
        finally
        {
            viewModel?.Dispose();
            App.CurrentSettings = originalSettings;
            LocalizationService.SetLanguage(originalLanguage);
            LocalizationService.SetTimeZone(string.Equals(originalTimeZone, "UTC", StringComparison.OrdinalIgnoreCase) ? "UTC" : "local");
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public void SettingsSaveFailureKeepsRecoveryOpenAndDoesNotPublishMemoryState()
    {
        var originalSettings = App.CurrentSettings;
        var originalLanguage = LocalizationService.Current.LanguageCode;
        var originalTimeZone = LocalizationService.DisplayTimeZone.Id;
        var root = Directory.CreateTempSubdirectory("codex-info-settings-vm-failure-test");
        SettingsViewModel? viewModel = null;
        try
        {
            var blocker = Path.Combine(root.FullName, "not-a-directory");
            File.WriteAllText(blocker, "blocked");
            var store = new ClientSettingsStore(Path.Combine(blocker, "settings.json"));
            App.CurrentSettings = new ClientSettings("ja", true);
            LocalizationService.SetLanguage("ja");
            LocalizationService.SetTimeZone("local");
            viewModel = new SettingsViewModel(store);

            Assert.False(viewModel.Save());
            Assert.True(viewModel.SaveFailed);
            Assert.Contains("設定", viewModel.StatusDetail, StringComparison.Ordinal);
            Assert.Equal(new ClientSettings("ja", true), App.CurrentSettings);
            Assert.Equal("ja", LocalizationService.Current.LanguageCode);
            Assert.Equal(TimeZoneInfo.Local.Id, LocalizationService.DisplayTimeZone.Id);
        }
        finally
        {
            viewModel?.Dispose();
            App.CurrentSettings = originalSettings;
            LocalizationService.SetLanguage(originalLanguage);
            LocalizationService.SetTimeZone(string.Equals(originalTimeZone, "UTC", StringComparison.OrdinalIgnoreCase) ? "UTC" : "local");
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public void SuccessfulSettingsRecoveryClearsTheInMemoryCorruptMarker()
    {
        var originalSettings = App.CurrentSettings;
        var originalLanguage = LocalizationService.Current.LanguageCode;
        var originalTimeZone = LocalizationService.DisplayTimeZone.Id;
        var root = Directory.CreateTempSubdirectory("codex-info-settings-recovery-test");
        SettingsViewModel? viewModel = null;
        try
        {
            var path = Path.Combine(root.FullName, "settings.json");
            File.WriteAllText(path, "{not-json}");
            var store = new ClientSettingsStore(path);
            App.CurrentSettings = store.Load();
            Assert.True(App.CurrentSettings.SettingsCorrupt);
            LocalizationService.SetLanguage("ja");
            LocalizationService.SetTimeZone("local");
            viewModel = new SettingsViewModel(store);

            Assert.True(viewModel.Save());
            Assert.False(App.CurrentSettings.SettingsCorrupt);
            Assert.False(store.Load().SettingsCorrupt);
        }
        finally
        {
            viewModel?.Dispose();
            App.CurrentSettings = originalSettings;
            LocalizationService.SetLanguage(originalLanguage);
            LocalizationService.SetTimeZone(string.Equals(originalTimeZone, "UTC", StringComparison.OrdinalIgnoreCase) ? "UTC" : "local");
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public async Task SettingsViewModelMirrorsMainAuthenticationCapability()
    {
        var auth = PublishedPairTestFixtures.DetailsGeneration(ApiState.AuthRequired, 1, false, null, null, [], 0);
        var ready = ValidDetails(DateTimeOffset.UtcNow.AddHours(2).ToUnixTimeSeconds());
        var client = new SequenceCombinedClient(DetailsFetchResult.Success(auth), DetailsFetchResult.Success(ready));
        using var main = new MainWindowViewModel(client);
        var root = Directory.CreateTempSubdirectory("codex-info-settings-status-test");
        SettingsViewModel? settings = null;
        try
        {
            settings = new SettingsViewModel(new ClientSettingsStore(Path.Combine(root.FullName, "settings.json")), main);
            main.Start();
            await EventuallyAsync(() => main.IsAuthRequired);

            Assert.True(settings.CanAuthenticate);
            Assert.Equal(main.StatusTitle, settings.StatusTitle);
            Assert.Equal(main.StatusDetail, settings.StatusDetail);

            main.RefreshCommand.Execute(null);
            await EventuallyAsync(() => main.IsAuthenticated);
            Assert.False(settings.CanAuthenticate);
            Assert.Equal(main.StatusTitle, settings.StatusTitle);
        }
        finally
        {
            settings?.Dispose();
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public async Task SetupNoneProfileRequiresObservedConnectionAndBuildsFailClosedSettings()
    {
        var auth = PublishedPairTestFixtures.DetailsGeneration(ApiState.AuthRequired, 1, false, null, null, [], 0);
        using var main = new MainWindowViewModel(new SequenceCombinedClient(DetailsFetchResult.Success(auth)));
        var originalSettings = App.CurrentSettings;
        try
        {
            App.CurrentSettings = ClientSettings.Default;
            main.Start();
            await EventuallyAsync(() => main.IsAuthRequired);

            using var setup = new SetupViewModel(main);
            Assert.Equal(ConnectionProfiles.None, setup.SelectedConnectionProfile);
            Assert.Equal([ConnectionSelectors.None], setup.ConnectionSelectorOptions);
            Assert.True(setup.IsConnectionSelectionValid);
            Assert.True(setup.CanContinue);

            var configured = setup.BuildSettings(ClientSettings.Default);
            Assert.True(configured.ConnectionConfigured);
            Assert.Equal(ConnectionProfiles.None, configured.ConnectionProfile);
            Assert.Equal(ConnectionSelectors.None, configured.ConnectionSelector);

            // Transient SSH input must not silently become a durable selector
            // for the none profile.
            setup.SshHost = "linux.example";
            setup.SshUser = "salty";
            Assert.False(setup.IsConnectionSelectionValid);
            var rejected = setup.BuildSettings(ClientSettings.Default);
            Assert.False(rejected.ConnectionConfigured);
            Assert.Equal(ConnectionProfiles.None, rejected.ConnectionProfile);
            Assert.Equal(ConnectionSelectors.None, rejected.ConnectionSelector);
        }
        finally
        {
            App.CurrentSettings = originalSettings;
        }
    }

    [Fact]
    public async Task SetupSshInputUsesBoundedSafeHostAndUserGrammar()
    {
        var auth = PublishedPairTestFixtures.DetailsGeneration(ApiState.AuthRequired, 1, false, null, null, [], 0);
        using var main = new MainWindowViewModel(new SequenceCombinedClient(DetailsFetchResult.Success(auth)));
        main.Start();
        await EventuallyAsync(() => main.IsAuthRequired);

        using var setup = new SetupViewModel(main);
        setup.SshHost = new string('h', 255);
        Assert.True(setup.CanStartSsh);
        setup.SshHost = new string('h', 256);
        Assert.False(setup.CanStartSsh);

        setup.SshHost = "linux.example";
        setup.SshUser = new string('u', 128);
        Assert.True(setup.CanStartSsh);
        setup.SshUser = new string('u', 129);
        Assert.False(setup.CanStartSsh);

        setup.SshUser = "salty";
        setup.SshHost = "host;whoami";
        Assert.False(setup.CanStartSsh);
        Assert.Contains("user@linux-host", setup.SshCommand, StringComparison.Ordinal);
    }

    [Fact]
    public async Task MainQuotaSegmentsExposeSevenBoundedCellsAndCurrentPeriod()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var quota = new ApiQuota(40, now + 500, 1_000, false);
        using var viewModel = new MainWindowViewModel(new SequenceCombinedClient(
            DetailsFetchResult.Success(PublishedPairTestFixtures.DetailsGeneration(ApiState.Ready, now, true, "Pro", quota, [], 0))));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.IsAuthenticated);

        Assert.Equal(7, viewModel.QuotaSegments.Count);
        Assert.All(viewModel.QuotaSegments, segment => Assert.InRange(segment.Fill, 0, 1));
        Assert.True(viewModel.QuotaSegments[0].Fill > 0.95);
        Assert.True(viewModel.QuotaSegments[3].Fill is > 0 and < 1);
        Assert.Equal(0, viewModel.QuotaSegments[4].Fill);
        Assert.InRange(viewModel.QuotaRemainingPeriodValue, 45, 55);
        Assert.Equal(LocalizationService.Current.WeeklyQuota, viewModel.QuotaWindowText);
    }

    [Fact]
    public async Task MainMonthlyQuotaLabelNamesResetAndKeepsSevenCellBoundary()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            LocalizationService.SetLanguage("ja");
            var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
            var quota = new ApiQuota(40, now + 500, 2_592_000, true);
            using var viewModel = new MainWindowViewModel(new SequenceCombinedClient(
                DetailsFetchResult.Success(PublishedPairTestFixtures.DetailsGeneration(
                    ApiState.Ready, now, true, "エンタープライズ", quota, [], 0))));

            viewModel.Start();
            await EventuallyAsync(() => viewModel.IsAuthenticated);

            Assert.Equal("月間：リセットまで", viewModel.QuotaWindowText);
            Assert.Equal(7, viewModel.QuotaSegments.Count);
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void MainQuotaGaugeUsesSevenCellsAndOnlyItsTwoDeclaredSurfaceColors()
    {
        var source = LoadRepositoryFile("windows-client", "src", "CodexInfo.WindowsClient", "MainWindow.axaml");
        var document = XDocument.Parse(source);
        var gaugeStyle = document.Descendants()
            .Single(element => element.Name.LocalName == "Style" && element.Attribute("Selector")?.Value == "Border.quota-segment");

        var gauge = document.Descendants()
            .Single(element => element.Name.LocalName == "TextBlock" && element.Attribute("AutomationProperties.AutomationId")?.Value == "Main.QuotaPeriodGauge");
        var gaugeItems = document.Descendants()
            .Single(element => element.Name.LocalName == "ItemsControl" && element.Attribute("ItemsSource")?.Value == "{Binding QuotaSegments}");
        var colors = gaugeStyle.Descendants()
            .Attributes("Value")
            .Select(attribute => attribute.Value)
            .Where(value => value.StartsWith("#", StringComparison.Ordinal))
            .Concat(gaugeItems.Descendants()
                .Attributes("Background")
                .Select(attribute => attribute.Value)
                .Where(value => value.StartsWith("#", StringComparison.Ordinal)))
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .ToArray();
        var xAccentPrimary = ReadSlintColor("accent-primary");
        var xAccentMuted = ReadSlintColor("accent-muted");
        Assert.Equal(["#56B2F5", "#326799"], [xAccentPrimary, xAccentMuted]);
        Assert.Equal(
            [xAccentMuted, xAccentPrimary],
            colors.Select(color => color.ToUpperInvariant()).ToArray());
        Assert.DoesNotContain(gaugeStyle.Descendants(), element => element.Name.LocalName is "Animation" or "Transitions");
        Assert.DoesNotContain(gaugeItems.Descendants(), element => element.Name.LocalName == "ProgressBar");

        Assert.Equal("{Binding QuotaWindowText}", gauge.Attribute("AutomationProperties.Name")?.Value);
        Assert.Equal("{Binding QuotaRemainingText}", gauge.Attribute("AutomationProperties.HelpText")?.Value);
        Assert.Equal("7", gaugeItems.Descendants().Single(element => element.Name.LocalName == "UniformGrid").Attribute("Columns")?.Value);
        Assert.Equal("0,0,4,0", gaugeStyle.Descendants()
            .Single(element => element.Name.LocalName == "Setter" && element.Attribute("Property")?.Value == "Margin")
            .Attribute("Value")?.Value);
        var scale = gaugeItems.Descendants().Single(element => element.Name.LocalName == "ScaleTransform");
        Assert.Equal("False", scale.Attribute("{http://schemas.microsoft.com/winfx/2006/xaml}CompileBindings")?.Value);
        Assert.Equal("{Binding DataContext.Fill, ElementName=QuotaSegmentCell}", scale.Attribute("ScaleX")?.Value);
    }

    [Fact]
    public void MainRefreshKeepsDetailsNotificationsOnTheUiContext()
    {
        var source = LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "ViewModels", "MainWindowViewModel.cs");
        var start = source.IndexOf("private async Task FetchCycleAsync", StringComparison.Ordinal);
        var end = source.IndexOf("private ExplicitOperation? BeginExplicitOperation", start, StringComparison.Ordinal);
        Assert.True(start >= 0 && end > start, "Main refresh method boundaries are missing.");
        var refresh = source[start..end];
        Assert.DoesNotContain("ConfigureAwait(false)", refresh, StringComparison.Ordinal);
    }

    [Fact]
    public void MainStatusBannerHasOneStablePrimaryCtaPerRecoveryRow()
    {
        var document = XDocument.Parse(LoadRepositoryFile(
            "windows-client", "src", "CodexInfo.WindowsClient", "MainWindow.axaml"));
        var buttons = document.Descendants()
            .Where(element => element.Name.LocalName == "Button")
            .ToArray();
        var statusButtons = buttons
            .Where(button => (button.Attribute("AutomationProperties.AutomationId")?.Value ?? "")
                .StartsWith("Main.Status.", StringComparison.Ordinal))
            .ToArray();

        Assert.Equal(
            ["Main.Status.AuthCheck", "Main.Status.AuthStart", "Main.Status.Refreshing", "Main.Status.Retry", "Main.Status.Update"],
            statusButtons
                .Select(button => button.Attribute("AutomationProperties.AutomationId")!.Value)
                .OrderBy(value => value, StringComparer.Ordinal)
                .ToArray());
        Assert.All(statusButtons, button =>
        {
            Assert.NotEqual("{Binding IsAuthRequired}", button.Attribute("IsVisible")?.Value);
            Assert.False(string.IsNullOrWhiteSpace(button.Attribute("AutomationProperties.Name")?.Value));
        });

        var visibleBindings = document.Descendants()
            .Where(element => element.Name.LocalName == "StackPanel")
            .Select(element => element.Attribute("IsVisible")?.Value)
            .Where(value => value is "{Binding IsAuthStartVisible}" or
                "{Binding IsAuthCheckVisible}" or
                "{Binding IsRetryVisible}" or
                "{Binding IsRefreshingVisible}" or
                "{Binding IsUpdateNotificationVisible}")
            .ToArray();
        Assert.Contains("{Binding IsAuthStartVisible}", visibleBindings);
        Assert.Contains("{Binding IsAuthCheckVisible}", visibleBindings);
        Assert.Contains("{Binding IsRetryVisible}", visibleBindings);
        Assert.Contains("{Binding IsRefreshingVisible}", visibleBindings);
        Assert.Contains("{Binding IsUpdateNotificationVisible}", visibleBindings);
    }

    [Fact]
    public void BorderlessWindowCloseControlsExposeStableAutomationIds()
    {
        var expected = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["MainWindow.axaml"] = "Main.Window.Close",
            ["GraphWindow.axaml"] = "Graph.Window.Close",
            ["ThreadsWindow.axaml"] = "Threads.Window.Close",
            ["LegalNoticesWindow.axaml"] = "Legal.Window.Close",
            ["SettingsWindow.axaml"] = "Settings.Window.Close",
            ["SetupWindow.axaml"] = "Setup.Window.Close",
        };

        foreach (var (fileName, automationId) in expected)
        {
            var document = XDocument.Parse(LoadRepositoryFile(
                "windows-client", "src", "CodexInfo.WindowsClient", fileName));
            Assert.Contains(
                document.Descendants().Where(element => element.Name.LocalName == "Button"),
                button => (button.Attribute("Click")?.Value is "OnCloseWindow" or "OnClose") &&
                    button.Attribute("AutomationProperties.AutomationId")?.Value == automationId);
        }
    }

    [Fact]
    public async Task MainDetailsFailureDoesNotPublishAPartialAccountGeneration()
    {
        var status = ValidDetails(DateTimeOffset.UtcNow.AddHours(2).ToUnixTimeSeconds());
        using var viewModel = new MainWindowViewModel(
            new SequenceCombinedClient(DetailsFetchResult.Success(status)),
            new SequenceDetailsClient(DetailsFetchResult.FromFailure(DetailsFetchFailure.Response)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.StatusTitle == LocalizationService.Current.Unavailable);

        Assert.False(viewModel.IsAuthenticated);
        Assert.False(viewModel.HasDetails);
        Assert.True(viewModel.HasNoModels);
        Assert.Equal(LocalizationService.Current.UnavailableValue, viewModel.RemainingPercentText);
    }

    [Fact]
    public async Task MainActivitySummaryClassifiesExactlyOneKnownModelToken()
    {
        var threads = new ApiThreadDetails[]
        {
            new("sol", "sol", null, "gpt-sol", "SOL", null, null, null, null, null, false, null, false),
            new("terra", "terra", null, "gpt-terra", "TERRA", null, null, null, null, null, false, null, false),
            new("luna", "luna", null, "gpt-luna", "LUNA", null, null, null, null, null, false, null, false),
            new("other", "other", null, "gpt-sol-terra", "SOL TERRA", null, null, null, null, null, false, null, false),
        };
        var details = new ApiDetailsSnapshot(ApiState.Ready, 1, true, "Pro", null, [], 4, [], [], threads, "概算 —")
        {
            PublishedPair = PublishedPairTestFixtures.Canonical,
        };
        using var viewModel = new MainWindowViewModel(
            new SequenceCombinedClient(DetailsFetchResult.Success(details)),
            new SequenceDetailsClient(DetailsFetchResult.Success(details)));

        viewModel.Start();
        await EventuallyAsync(() => viewModel.HasDetails);

        Assert.Equal(4UL, viewModel.ActiveThreadCount);
        Assert.Equal(1, viewModel.ActiveSolCount);
        Assert.Equal(1, viewModel.ActiveTerraCount);
        Assert.Equal(1, viewModel.ActiveLunaCount);
        Assert.Equal(1, viewModel.ActiveOtherCount);
    }

    [Fact]
    public void GraphPointFormatsMissingQuotaAndBothMetricFamilies()
    {
        var sample = new ApiHistorySample(100, 200, null, 1.25, 2, 3, 100, 200, 300);

        var dollars = new GraphPointViewModel(sample, GraphMetric.Dollars);
        var tokens = new GraphPointViewModel(sample, GraphMetric.Tokens);

        Assert.Null(dollars.RemainingPercent);
        Assert.Contains("—", dollars.RemainingText, StringComparison.Ordinal);
        Assert.Equal(1.25, dollars.SolValue);
        Assert.Contains("$1.25", dollars.ModelsText, StringComparison.Ordinal);
        Assert.Equal(100, tokens.SolValue);
        Assert.Contains("SOL 100", tokens.ModelsText, StringComparison.Ordinal);
    }

    [Fact]
    public void GraphReductionPreservesEndpointsAndRejectsAnUnrenderableBudget()
    {
        var samples = Enumerable.Range(0, 5)
            .Select(index => new ApiHistorySample(index + 1, 10, 100 - index, index, index * 2, index * 3, (ulong)index, (ulong)(index * 2), (ulong)(index * 3)))
            .ToArray();

        var reduced = GraphWindowViewModel.ReduceGraphSamples(samples, 3);

        Assert.Equal(3, reduced.Count);
        Assert.Equal(samples[0], reduced[0]);
        Assert.Equal(samples[^1], reduced[^1]);
        Assert.Throws<ArgumentOutOfRangeException>(() => GraphWindowViewModel.ReduceGraphSamples(samples, 1));
    }

    [Fact]
    public async Task ThreadPresentationUsesExplicitFallbacksForMissingContextTokensAndParent()
    {
        var thread = new ApiThreadDetails(
            "orphan",
            "orphan task",
            null,
            "gpt-5.6-luna",
            "",
            null,
            null,
            null,
            null,
            null,
            true,
            null,
            true);
        var details = new ApiDetailsSnapshot(ApiState.Ready, 1, true, "Pro", null, [], 1, [], [], [thread], "概算 —")
        {
            PublishedPair = PublishedPairTestFixtures.Canonical,
        };
        using var main = new MainWindowViewModel(
            new SequenceCombinedClient(DetailsFetchResult.Success(details)),
            new SequenceDetailsClient(DetailsFetchResult.Success(details)));
        main.Start();
        await EventuallyAsync(() => main.HasDetails);

        using var viewModel = new ThreadsWindowViewModel(main);
        var item = Assert.Single(viewModel.Threads);

        Assert.Contains(LocalizationService.Current.SubThread, item.RoleText, StringComparison.Ordinal);
        Assert.Contains(LocalizationService.Current.ParentUnavailable, item.ParentText, StringComparison.Ordinal);
        Assert.Equal("gpt-5.6-luna", item.ModelText);
        Assert.EndsWith("—", item.ContextText, StringComparison.Ordinal);
        Assert.EndsWith("—", item.TokenText, StringComparison.Ordinal);
        Assert.EndsWith("—", item.AgeText, StringComparison.Ordinal);
        Assert.EndsWith("—", item.InstructionAgeText, StringComparison.Ordinal);
    }

    private static ApiDetailsSnapshot ValidDetails(long observedAt) =>
        PublishedPairTestFixtures.DetailsGeneration(
            ApiState.Ready,
            observedAt,
            true,
            "Pro",
            new ApiQuota(75, observedAt + 604_800, 604_800, false),
            [],
            0);

    private static async Task EventuallyAsync(Func<bool> condition)
    {
        var stopwatch = Stopwatch.StartNew();
        while (!condition())
        {
            if (stopwatch.Elapsed > TimeSpan.FromSeconds(2))
            {
                throw new TimeoutException("Expected presentation state was not reached.");
            }

            await Task.Delay(10);
        }
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

    private static string ReadSlintColor(string token)
    {
        var source = LoadRepositoryFile("ui", "theme.slint");
        var line = source
            .Split('\n', StringSplitOptions.RemoveEmptyEntries)
            .Single(line => line.Contains($"out property <color> {token}:", StringComparison.Ordinal));
        var colon = line.IndexOf(':');
        return line[(colon + 1)..].Trim().TrimEnd(';').ToUpperInvariant();
    }

    private sealed class SequenceCombinedClient(params DetailsFetchResult[] results) : HealthyDetailsClientBase
    {
        private int index;

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            var result = results[Math.Min(index, results.Length - 1)];
            index++;
            return Task.FromResult(result);
        }
    }

    private sealed class SequenceDetailsClient(params DetailsFetchResult[] results) : ILoopbackDetailsClient
    {
        private int index;

        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default)
        {
            var result = results[Math.Min(index, results.Length - 1)];
            index++;
            return Task.FromResult(result);
        }
    }
}
