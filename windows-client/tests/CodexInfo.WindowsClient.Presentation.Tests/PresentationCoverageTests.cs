// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Diagnostics;
using System.Globalization;
using System.Reflection;
using CodexInfo.WindowsClient;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class PresentationCoverageTests
{
    [Fact]
    public void LocalizationCatalogExercisesEveryLocalePropertyAndFallback()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        var originalTimeZone = LocalizationService.DisplayTimeZone.Id;
        var stringProperties = typeof(UiText)
            .GetProperties(BindingFlags.Public | BindingFlags.Instance)
            .Where(property => property.PropertyType == typeof(string)
                && property.GetMethod is not null
                && property.GetMethod.GetParameters().Length == 0)
            .ToArray();

        try
        {
            Assert.Equal("ja", LocalizationService.NormalizeLanguageCode(null));
            Assert.Equal("ja", LocalizationService.NormalizeLanguageCode("  "));
            Assert.Equal("en", LocalizationService.NormalizeLanguageCode("en-US"));
            Assert.Equal("zh-Hans", LocalizationService.NormalizeLanguageCode("zh_CN"));
            Assert.Equal("en", LocalizationService.NormalizeLanguageCode("xx-YY"));

            foreach (var language in LocalizationService.Languages)
            {
                LocalizationService.SetLanguage(language.LanguageCode);
                var text = LocalizationService.Current;
                foreach (var property in stringProperties)
                {
                    var value = (string?)property.GetValue(text);
                    Assert.NotNull(value);
                    if (property.Name != nameof(UiText.CountUnit))
                    {
                        Assert.False(string.IsNullOrWhiteSpace(value), $"{language.LanguageCode}.{property.Name}");
                    }
                }

                Assert.False(string.IsNullOrWhiteSpace(text.Format("{0}", text.LanguageName)));
                Assert.False(string.IsNullOrWhiteSpace(text.FormatRemaining(2, 3, 4)));
                Assert.False(string.IsNullOrWhiteSpace(text.FormatRemaining(0, 0, 0, lessThanMinute: true)));
                Assert.False(string.IsNullOrWhiteSpace(text.FormatRemaining(0, 0, 0, immediate: true)));
                Assert.False(string.IsNullOrWhiteSpace(text.FormatElapsed(null, text.Elapsed)));
                Assert.False(string.IsNullOrWhiteSpace(text.FormatElapsed(DateTimeOffset.UtcNow.AddMinutes(-2).ToUnixTimeSeconds(), text.Elapsed)));
                Assert.False(string.IsNullOrWhiteSpace(text.FormatElapsed(DateTimeOffset.UtcNow.AddHours(-2).ToUnixTimeSeconds(), text.Elapsed)));
                Assert.False(string.IsNullOrWhiteSpace(text.FormatElapsed(DateTimeOffset.UtcNow.AddDays(-2).ToUnixTimeSeconds(), text.Elapsed)));

                foreach (var state in new[]
                {
                    "Connecting", "Ready", "QuotaDanger", "QuotaWarning", "ResetWarning",
                    "Initializing", "AuthRequired", "ApiError", "TransportError", "ResponseError", "Other",
                })
                {
                    Assert.False(string.IsNullOrWhiteSpace(text.StatusDetailFor(state, authLaunchFailed: true, hasSnapshot: false)));
                    Assert.False(string.IsNullOrWhiteSpace(text.StatusDetailFor(state, authLaunchFailed: false, hasSnapshot: true)));
                }
            }

            LocalizationService.SetTimeZone("UTC");
            Assert.Equal("UTC", LocalizationService.DisplayTimeZone.Id);
            LocalizationService.SetTimeZone("invalid");
            Assert.NotNull(LocalizationService.DisplayTimeZone);
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
            LocalizationService.SetTimeZone(string.Equals(originalTimeZone, "UTC", StringComparison.OrdinalIgnoreCase) ? "UTC" : "local");
        }
    }

    [Fact]
    public void PreviewEnvironmentReadsScenarioAndSizeBoundaries()
    {
        Assert.False(PreviewEnvironment.TryParseSize("700X480", out _, out _));
        Assert.True(PreviewEnvironment.TryParseSize(" 640x480 ", out var width, out var height));
        Assert.Equal(640, width);
        Assert.Equal(480, height);

        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW_SIZE", "800x600", () =>
        {
            Assert.True(PreviewEnvironment.TryGetSize(out var actualWidth, out var actualHeight));
            Assert.Equal(800, actualWidth);
            Assert.Equal(600, actualHeight);
        });
        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW_SIZE", "bad", () =>
        {
            Assert.False(PreviewEnvironment.TryGetSize(out _, out _));
        });

        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS", "1", () => Assert.Equal(3, PreviewEnvironment.GraphPointCount));
        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS", "44640", () => Assert.Equal(44_640, PreviewEnvironment.GraphPointCount));
        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS", "bad", () => Assert.Equal(3, PreviewEnvironment.GraphPointCount));
        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS", "5000", () => Assert.Equal(5_000, PreviewEnvironment.GraphBuildDelayMilliseconds));
        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS", "-1", () => Assert.Equal(0, PreviewEnvironment.GraphBuildDelayMilliseconds));
    }

    [Theory]
    [InlineData("auth", ApiState.AuthRequired, false, 48)]
    [InlineData("error", ApiState.Error, false, 48)]
    [InlineData("zero", ApiState.Ready, true, 0)]
    [InlineData("full", ApiState.Ready, true, 100)]
    [InlineData("warning", ApiState.Ready, true, 10)]
    [InlineData("danger", ApiState.Ready, true, 2)]
    [InlineData("setup", ApiState.Ready, true, 48)]
    public async Task PreviewLoopbackScenariosExposeTheirDeclaredState(
        string scenario,
        ApiState expectedState,
        bool expectedAuthenticated,
        double expectedRemaining)
    {
        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW", scenario, () =>
        {
            Assert.True(PreviewEnvironment.Enabled);
            Assert.Equal(scenario == "setup", PreviewEnvironment.IsSetup);
            Assert.True(PreviewEnvironment.IsChild(scenario.ToUpperInvariant()));
        });

        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW", scenario, async () =>
        {
            using var client = new PreviewLoopbackClient();
            var details = await client.FetchDetailsAsync(CancellationToken.None);

            Assert.Equal(expectedState, details.Snapshot!.State);
            Assert.Equal(expectedAuthenticated, details.Snapshot.Authenticated);
            Assert.Equal(expectedRemaining, details.Snapshot.Quota!.RemainingPercent);
            client.Dispose();
        });
    }

    [Fact]
    public void PreviewLoopbackUsesGeneratedPointBranchAndIgnoresCancellation()
    {
        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW", "full", () =>
        WithEnvironment("CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS", "2", () =>
        {
            using var client = new PreviewLoopbackClient();
            var details = client.FetchDetailsAsync(new CancellationToken(canceled: true)).GetAwaiter().GetResult();
            Assert.Equal(2, details.Snapshot!.HistoryPeriods.Single(period => period.Current).Samples.Count);
            Assert.Equal(4, details.Snapshot.HistorySamples.Count);
            Assert.Equal(100, details.Snapshot.Quota!.RemainingPercent);
        }));
    }

    [Fact]
    public void SettingsAndSelectorsRejectUnsafeOrUnknownDurableValues()
    {
        Assert.Throws<ArgumentException>(() => new ClientSettingsStore("  "));
        Assert.True(ConnectionSelectors.IsSshAlias("work.example"));
        Assert.False(ConnectionSelectors.IsSshAlias("user@work.example"));
        Assert.False(ConnectionSelectors.IsSshAlias(new string('a', 256)));
        Assert.True(ConnectionSelectors.IsWslToken("Ubuntu-24.04"));
        Assert.False(ConnectionSelectors.IsWslToken(null));
        Assert.False(ConnectionSelectors.IsWslToken(ConnectionSelectors.None));
        Assert.False(ConnectionSelectors.IsWslToken("Ubuntu 24"));
        Assert.False(ConnectionSelectors.IsWslToken(new string('u', 256)));

        Assert.False(ConnectionSelectors.IsValid(new ClientSettings("xx", true)));
        Assert.False(ConnectionSelectors.IsValid(new ClientSettings("ja", true) { TimeZoneId = "remote" }));
        Assert.False(ConnectionSelectors.IsValid(new ClientSettings("ja", true)
        {
            ConnectionProfile = ConnectionProfiles.Wsl,
            ConnectionSelector = "none",
        }));
        Assert.False(ConnectionSelectors.IsValid(new ClientSettings("ja", true)
        {
            ConnectionProfile = "unsupported",
            ConnectionSelector = ConnectionSelectors.None,
        }));
        Assert.True(ConnectionSelectors.IsValid(ClientSettings.Default));
    }

    [Fact]
    public void SettingsRejectDuplicateKeysInsteadOfApplyingSerializerLastValue()
    {
        var root = Directory.CreateTempSubdirectory("codex-info-settings-duplicate");
        try
        {
            var path = Path.Combine(root.FullName, "settings.json");
            File.WriteAllText(
                path,
                "{\"language\":\"ja\",\"setupCompleted\":true,\"connectionConfigured\":true,\"timeZoneId\":\"local\",\"connectionProfile\":\"none\",\"connectionProfile\":\"none\"}");

            var loaded = new ClientSettingsStore(path).Load();

            Assert.True(loaded.SettingsCorrupt);
            Assert.Equal(ClientSettings.Default, loaded with { SettingsCorrupt = false });
        }
        finally
        {
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public void ModelUsageFormatsEveryColumnAndPublishesLanguageChanges()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        var originalCulture = CultureInfo.CurrentCulture;
        try
        {
            CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("en-US");
            using var apiModel = new ModelUsageViewModel(new ApiDetailsModelUsage("SOL", 1_234_567, 23_456, 789, 0, 0, 0));
            Assert.Equal("SOL", apiModel.Name);
            Assert.Equal("1,234,567", apiModel.InputTokensText);
            Assert.Equal("23,456", apiModel.CachedInputTokensText);
            Assert.Equal("789", apiModel.OutputTokensText);
            Assert.Equal("$0.00", apiModel.InputDollarsText);
            Assert.Equal("$0.00", apiModel.CachedInputDollarsText);
            Assert.Equal("$0.00", apiModel.OutputDollarsText);

            using var detailsModel = new ModelUsageViewModel(
                new ApiDetailsModelUsage("TERRA", 12, 34, 56, double.NaN, double.PositiveInfinity, double.NegativeInfinity));
            var changed = new List<string?>();
            detailsModel.PropertyChanged += (_, eventArgs) => changed.Add(eventArgs.PropertyName);
            Assert.Equal("12", detailsModel.InputTokensText);
            Assert.Equal("34", detailsModel.CachedInputTokensText);
            Assert.Equal("56", detailsModel.OutputTokensText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, detailsModel.InputDollarsText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, detailsModel.CachedInputDollarsText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, detailsModel.OutputDollarsText);
            Assert.Equal(LocalizationService.Current.Input, detailsModel.InputLabel);
            Assert.Equal(LocalizationService.Current.CachedInput, detailsModel.CachedInputLabel);
            Assert.Equal(LocalizationService.Current.Output, detailsModel.OutputLabel);

            LocalizationService.SetLanguage(originalLanguage == "ja" ? "en" : "ja");
            Assert.Contains(nameof(ModelUsageViewModel.InputTokensText), changed);
            Assert.Contains(nameof(ModelUsageViewModel.CachedInputTokensText), changed);
            Assert.Contains(nameof(ModelUsageViewModel.OutputTokensText), changed);
            Assert.Contains(nameof(ModelUsageViewModel.InputDollarsText), changed);
            Assert.Contains(nameof(ModelUsageViewModel.CachedInputDollarsText), changed);
            Assert.Contains(nameof(ModelUsageViewModel.OutputDollarsText), changed);
            Assert.Contains(nameof(ModelUsageViewModel.InputLabel), changed);
            Assert.Contains(nameof(ModelUsageViewModel.CachedInputLabel), changed);
            Assert.Contains(nameof(ModelUsageViewModel.OutputLabel), changed);

            detailsModel.Dispose();
            detailsModel.Dispose();
        }
        finally
        {
            CultureInfo.CurrentCulture = originalCulture;
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void AsyncCommandHonorsCanExecuteAndSwallowsCancellation()
    {
        var canExecute = false;
        var executions = 0;
        var command = new AsyncCommand(
            () =>
            {
                executions++;
                return Task.CompletedTask;
            },
            () => canExecute);
        var changed = 0;
        command.CanExecuteChanged += (_, _) => changed++;

        Assert.False(command.CanExecute(null));
        command.Execute(null);
        Assert.Equal(0, executions);

        canExecute = true;
        command.RaiseCanExecuteChanged();
        Assert.Equal(1, changed);
        command.Execute(null);
        Assert.Equal(1, executions);

        var canceled = new AsyncCommand(
            () => Task.FromCanceled(new CancellationToken(canceled: true)),
            () => true);
        Assert.Null(Record.Exception(() => canceled.Execute(null)));
    }

    [Fact]
    public void MainDisconnectedPropertiesUseSafeLocalizedFallbacks()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            LocalizationService.SetLanguage("en");
            using var main = new MainWindowViewModel(new NeverCalledDetailsClient());
            var changed = new List<string?>();
            main.PropertyChanged += (_, eventArgs) => changed.Add(eventArgs.PropertyName);

            Assert.Equal(LocalizationService.Current.Refresh, main.RefreshButtonText);
            Assert.False(main.HasQuota);
            Assert.Equal(0UL, main.ActiveThreadCount);
            Assert.Equal("0", main.ActiveThreadCountLabel);
            Assert.Equal(LocalizationService.Current.UnavailableValue, main.RemainingPercentText);
            Assert.Equal(0, main.RemainingPercentValue);
            Assert.Equal(LocalizationService.Current.QuotaWaiting, main.QuotaWindowText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, main.QuotaRemainingText);
            Assert.Equal(0, main.QuotaRemainingPeriodValue);
            Assert.Equal($"{LocalizationService.Current.ModelUsage}: {LocalizationService.Current.UnavailableValue}", main.ModelUsageUnavailableText);
            Assert.Same(main.Models, main.CurrentModels);
            Assert.Equal(LocalizationService.Current.UnavailableValue, main.PlanText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, main.ActiveThreadCountText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, main.ResetAtText);
            Assert.Equal(LocalizationService.Current.UnavailableValue, main.ObservedAtText);
            Assert.Equal(LocalizationService.Current.LastReceivedUnavailable, main.LastReceivedText);
            Assert.Equal(LocalizationService.Current.Connecting, main.StatusTitle);
            Assert.Same(main.StatusBackground, main.StatusBackground);
            Assert.Contains(LocalizationService.Current.UnavailableValue, main.DetailsStatusText);

            LocalizationService.SetLanguage("ja");
            Assert.Contains(nameof(MainWindowViewModel.RefreshButtonText), changed);
            Assert.Contains("未取得", main.DetailsStatusText, StringComparison.Ordinal);
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public async Task SetupExposesProfilesAndLocalizedPropertiesWithoutLaunchingSsh()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            using var main = new MainWindowViewModel(new CountingDetailsClient(ReadyDetails()));
            var session = new RecordingSettingsSession(ClientSettings.Default);
            using var setup = new SetupViewModel(main, session);
            var changed = new List<string?>();
            setup.PropertyChanged += (_, eventArgs) => changed.Add(eventArgs.PropertyName);

            main.Start();
            await EventuallyAsync(() => main.IsAuthenticated);
            Assert.Equal(3, setup.ConnectionProfileOptions.Count);
            Assert.Equal(LocalizationService.Current.ApiCommand, setup.ApiCommand);
            Assert.Equal(main.StatusTitle, setup.StatusTitle);
            Assert.Equal(main.StatusDetail, setup.StatusDetail);
            Assert.Equal(LocalizationService.Current.SshStart, setup.SshActionText);
            Assert.Equal(LocalizationService.Current.SshNotReady, setup.SshStatusText);
            Assert.Equal(LocalizationService.Current.Continue, setup.ContinueText);

            setup.SelectedSshConfigAlias = "alias.example";
            Assert.Equal("alias.example", setup.SshHost);
            setup.SelectedSshConfigAlias = null;
            setup.SelectedConnectionProfile = ConnectionProfiles.SshConfigAlias;
            setup.SelectedConnectionSelector = "alias.example";
            Assert.False(setup.IsConnectionSelectionValid);
            setup.SelectedConnectionProfile = ConnectionProfiles.Wsl;
            setup.SelectedConnectionSelector = "Ubuntu-24.04";
            Assert.False(setup.IsConnectionSelectionValid);

            setup.SelectedConnectionProfile = ConnectionProfiles.None;
            setup.SelectedConnectionSelector = null!;
            setup.SshHost = "localhost";
            setup.SshUser = string.Empty;
            Assert.True(setup.CanStartSsh);
            Assert.Contains("localhost", setup.SshCommand, StringComparison.Ordinal);
            Assert.Equal(LocalizationService.Current.SshReadyStatus, setup.SshStatusText);
            setup.SshHost = string.Empty;
            Assert.True(setup.IsConnectionSelectionValid);

            LocalizationService.SetLanguage("en");
            Assert.Contains(nameof(SetupViewModel.ConnectionProfileOptions), changed);
            Assert.Contains(nameof(SetupViewModel.ApiCommand), changed);
            Assert.Contains(nameof(SetupViewModel.ContinueText), changed);
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void ConnectionSupervisorFailsClosedAndIsIdempotent()
    {
        using var supervisor = new ConnectionSupervisor();
        Assert.True(supervisor.EnsureStarted(ClientSettings.Default));
        Assert.False(supervisor.IsRunning);
        Assert.True(supervisor.EnsureStarted(ClientSettings.Default));
        supervisor.Stop();

        Assert.False(supervisor.EnsureStarted(new ClientSettings("xx", true)));
        supervisor.Dispose();
        supervisor.Dispose();
        Assert.False(supervisor.EnsureStarted(ClientSettings.Default));
    }

    [Fact]
    public async Task MainPublishesCompleteAuthenticatedGenerationAndPresentationProperties()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var quota = new ApiQuota(75, now + 8 * 86_400, 31 * 86_400, true);
        var details = new ApiDetailsSnapshot(
            ApiState.Ready,
            now,
            true,
            "エンタープライズ",
            quota,
            [
                new ApiDetailsModelUsage("LUNA", 3, 2, 1, 0.3, 0.2, 0.1),
                new ApiDetailsModelUsage("SOL", 9, 8, 7, 0.9, 0.8, 0.7),
                new ApiDetailsModelUsage("TERRA", 6, 5, 4, 0.6, 0.5, 0.4),
            ],
            4,
            [new ApiHistoryPeriod("current", now - 100, now + 100, true, "current")],
            [],
            [
                new ApiThreadDetails("sol", "SOL", null, "gpt-sol", "SOL", 1, 1, 2, now, now, false, 0, false),
                new ApiThreadDetails("terra", "TERRA", null, "gpt-terra", "TERRA", 1, 1, 2, now, now, false, 0, false),
                new ApiThreadDetails("luna", "LUNA", null, "gpt-luna", "LUNA", 1, 1, 2, now, now, false, 0, false),
                new ApiThreadDetails("other", "Other", null, "gpt-other", "SOL TERRA", 1, 1, 2, now, now, false, 0, false),
            ],
            "概算 $9")
        {
            PublishedPair = PublishedPairTestFixtures.Canonical,
        };

        using var main = new MainWindowViewModel(
            new SequenceCoreDetailsClient(DetailsFetchResult.Success(details)),
            new SequenceDetailsClient(DetailsFetchResult.Success(details)));
        var notifications = new List<string?>();
        main.PropertyChanged += (_, eventArgs) => notifications.Add(eventArgs.PropertyName);
        main.Start();
        await EventuallyAsync(() => main.HasDetails);

        Assert.True(main.CanRefresh);
        Assert.True(main.IsAuthenticated);
        Assert.True(main.HasQuota);
        Assert.True(main.HasModels);
        Assert.False(main.HasNoModels);
        Assert.Equal(4UL, main.ActiveThreadCount);
        Assert.True(main.HasActiveThreads);
        Assert.False(main.HasNoActiveThreads);
        Assert.Equal(1, main.ActiveSolCount);
        Assert.Equal(1, main.ActiveTerraCount);
        Assert.Equal(1, main.ActiveLunaCount);
        Assert.Equal(1, main.ActiveOtherCount);
        Assert.Equal(LocalizationService.Current.MonthlyQuota, main.QuotaWindowText);
        Assert.Equal("75%", main.RemainingPercentText);
        Assert.Equal(75, main.RemainingPercentValue);
        Assert.NotEqual(LocalizationService.Current.UnavailableValue, main.QuotaRemainingText);
        Assert.Equal("エンタープライズ", main.PlanText);
        Assert.Equal(LocalizationService.Current.Connected, main.AuthenticationText);
        Assert.Equal("current", main.ModelUsagePeriodText);
        Assert.Equal("概算 $9", main.EstimatedCostText);
        Assert.Contains("最新", main.DetailsStatusText, StringComparison.Ordinal);
        Assert.Equal(3, main.Models.Count);
        Assert.Equal("SOL", main.Models[0].Name);
        Assert.Equal("TERRA", main.Models[1].Name);
        Assert.Equal("LUNA", main.Models[2].Name);
        Assert.NotEmpty(main.ActiveThreadCountLabel);
        Assert.NotEmpty(main.ActiveThreadCountText);
        Assert.NotEmpty(main.ResetAtText);
        Assert.NotEmpty(main.ObservedAtText);
        Assert.NotEmpty(main.LastReceivedText);
        Assert.NotEmpty(main.StatusTitle);
        Assert.NotEmpty(main.StatusDetail);
        Assert.NotNull(main.StatusBackground);
        Assert.NotNull(main.StatusBorder);
        Assert.NotNull(main.StatusAccent);
        Assert.True(main.RefreshCommand.CanExecute(null));
        Assert.True(main.CheckAuthCommand.CanExecute(null) == false);
        Assert.Contains(nameof(MainWindowViewModel.StatusTitle), notifications);
    }

    [Fact]
    public async Task MainClassifiesThrownDetailsFailuresAndDisposesClients()
    {
        var combinedClient = new DisposableThrowingCombinedClient();
        using (var main = new MainWindowViewModel(combinedClient))
        {
            main.Start();
            await EventuallyAsync(() => main.StatusTitle == LocalizationService.Current.TransportError);
            Assert.False(main.IsAuthenticated);
        }
        Assert.True(combinedClient.Disposed);

        var status = ReadyDetails();
        var detailsClient = new ThrowingDetailsClient();
        using var complete = new MainWindowViewModel(
            new SequenceCoreDetailsClient(DetailsFetchResult.Success(status)),
            detailsClient);
        complete.Start();
        await EventuallyAsync(() => complete.StatusTitle == LocalizationService.Current.TransportError);
        Assert.False(complete.HasDetails);
    }

    [Fact]
    public async Task MainKeepsCompleteLastGoodAfterInvalidDetailsAndRecoversWithOneValidGeneration()
    {
        var status = ReadyDetails();
        var first = new ApiDetailsSnapshot(
            status.State,
            status.ObservedAt,
            status.Authenticated,
            status.PlanLabel,
            status.Quota,
            [new ApiDetailsModelUsage("SOL", 1, 2, 3, 1, 1, 1)],
            status.ActiveThreadCount,
            [],
            [],
            [],
            "first")
        {
            PublishedPair = PublishedPairTestFixtures.Canonical,
        };
        var recovered = first with
        {
            ObservedAt = status.ObservedAt + 60,
            Quota = status.Quota! with { RemainingPercent = 41 },
            Models = [new ApiDetailsModelUsage("SOL", 999, 2, 3, 323.674247, 0, 0)],
            EstimatedCostLabel = "recovered",
        };
        using var main = new MainWindowViewModel(
            new SequenceCoreDetailsClient(DetailsFetchResult.Success(status)),
            new SequenceDetailsClient(
                DetailsFetchResult.Success(first),
                DetailsFetchResult.FromFailure(DetailsFetchFailure.Response),
                DetailsFetchResult.Success(recovered)));

        main.Start();
        await EventuallyAsync(() => main.DetailsSnapshot?.EstimatedCostLabel == "first");
        var lastGood = main.DetailsSnapshot;
        var lastGoodObserved = main.ObservedAtText;

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => main.DetailsStatusAutomationText == "error");
        Assert.Same(lastGood, main.DetailsSnapshot);
        Assert.Equal(lastGoodObserved, main.ObservedAtText);
        Assert.Equal(75, main.RemainingPercentValue);
        Assert.Equal("$1.00", main.Models[0].InputDollarsText);
        Assert.True(main.CanRefresh);

        main.RefreshCommand.Execute(null);
        await EventuallyAsync(() => main.DetailsSnapshot?.EstimatedCostLabel == "recovered");
        Assert.Same(recovered, main.DetailsSnapshot);
        Assert.Equal(41, main.RemainingPercentValue);
        Assert.Equal("$323.67", main.Models[0].InputDollarsText);
        Assert.Equal("ready", main.DetailsStatusAutomationText);
    }

    [Fact]
    public async Task MainStartIsIdempotentAndDisposeDisablesCommands()
    {
        var client = new CountingDetailsClient(ReadyDetails());
        var main = new MainWindowViewModel(client);
        main.Start();
        main.Start();
        await EventuallyAsync(() => client.CallCount == 1);
        Assert.True(main.CanRefresh);
        main.Dispose();
        main.Dispose();
        Assert.False(main.CanRefresh);
        Assert.False(main.RefreshCommand.CanExecute(null));
        Assert.False(main.AuthCommand.CanExecute(null));
        Assert.False(main.CheckAuthCommand.CanExecute(null));
    }

    [Fact]
    public async Task SetupCoversProfileSelectionValidationAndAuthenticationStep()
    {
        var originalSettings = App.CurrentSettings;
        App.CurrentSettings = ClientSettings.Default;
        try
        {
            var main = new MainWindowViewModel(new SequenceCoreDetailsClient(
                DetailsFetchResult.Success(PublishedPairTestFixtures.DetailsGeneration(
                    ApiState.AuthRequired, 1, false, null, null, [], 0)),
                DetailsFetchResult.Success(ReadyDetails())));
            var session = new RecordingSettingsSession(ClientSettings.Default);
            using (main)
            using (var setup = new SetupViewModel(main, session))
            {
                main.Start();
                await EventuallyAsync(() => main.IsAuthRequired);
                Assert.True(setup.IsConnectionStep);
                Assert.False(setup.IsAuthStep);
                Assert.False(setup.IsDoneStep);
                Assert.Equal([ConnectionSelectors.None], setup.ConnectionSelectorOptions);
                Assert.True(setup.IsConnectionSelectionValid);
                Assert.True(setup.CanContinue);

                setup.SelectedConnectionProfile = "invalid";
                Assert.Equal(ConnectionProfiles.None, setup.SelectedConnectionProfile);
                setup.SelectedConnectionProfile = ConnectionProfiles.Wsl;
                Assert.Empty(setup.ConnectionSelectorOptions);
                setup.SelectedConnectionSelector = null!;
                Assert.Equal(ConnectionSelectors.None, setup.SelectedConnectionSelector);
                setup.SelectedConnectionProfile = ConnectionProfiles.SshConfigAlias;
                setup.SelectedConnectionSelector = "work.example";
                Assert.Equal("work.example", setup.SshHost);
                Assert.False(setup.IsConnectionSelectionValid);

                setup.SshHost = "linux.example";
                setup.SshUser = "salty";
                setup.SelectedConnectionSelector = "linux.example";
                Assert.True(setup.CanStartSsh);
                Assert.Contains("salty@linux.example", setup.SshCommand, StringComparison.Ordinal);
                setup.SshUser = "bad user";
                Assert.False(setup.CanStartSsh);
                setup.SshUser = "salty";
                setup.SshHost = "host;whoami";
                Assert.False(setup.CanStartSsh);
                setup.StartOrStopSsh();

                setup.SshHost = string.Empty;
                setup.SshUser = string.Empty;
                setup.SelectedConnectionProfile = ConnectionProfiles.None;
                Assert.True(setup.CanContinue);
                Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
                Assert.Equal(1, setup.Step);
                Assert.True(setup.IsAuthStep);
                Assert.Single(session.Generations);
                Assert.False(session.Current.SetupCompleted);
                Assert.False(setup.CanContinue);
                setup.Continue();

                main.RefreshCommand.Execute(null);
                await EventuallyAsync(() => main.IsAuthenticated);
                Assert.True(setup.CanContinue);
                Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
                Assert.Equal(2, setup.Step);
                Assert.True(setup.IsDoneStep);
                Assert.Equal(SetupAdvanceOutcome.CloseRequested, setup.Advance());
                Assert.Equal(2, setup.Step);
                Assert.True(session.Current.SetupCompleted);

                var built = setup.BuildSettings(ClientSettings.Default);
                Assert.True(built.ConnectionConfigured);
                Assert.Equal(ConnectionProfiles.None, built.ConnectionProfile);
                Assert.Equal(ConnectionSelectors.None, built.ConnectionSelector);
                Assert.Equal(LocalizationService.Current.Close, setup.ContinueText);
            }
        }
        finally
        {
            App.CurrentSettings = originalSettings;
        }
    }

    [Fact]
    public void SettingsViewModelExposesFallbacksAndNotifiesOnSelectionChanges()
    {
        var originalSettings = App.CurrentSettings;
        var root = Directory.CreateTempSubdirectory("codex-info-settings-coverage");
        try
        {
            var store = new ClientSettingsStore(Path.Combine(root.FullName, "settings.json"));
            var viewModel = new SettingsViewModel(store);
            var changed = new List<string?>();
            viewModel.PropertyChanged += (_, args) => changed.Add(args.PropertyName);

            Assert.NotEmpty(viewModel.LanguageOptions);
            Assert.Equal(LocalizationService.Current.LanguageCode, viewModel.SelectedLanguageCode);
            Assert.NotNull(viewModel.SelectedLanguage);
            Assert.Equal(LocalizationService.Current.Unavailable, viewModel.StatusTitle);
            Assert.Equal(LocalizationService.Current.UnavailableDetails, viewModel.StatusDetail);
            Assert.False(viewModel.CanAuthenticate);
            Assert.Equal(2, viewModel.TimeZoneOptions.Count);
            Assert.Equal(LocalizationService.Current.LocalTimeZone, viewModel.SelectedTimeZone);
            viewModel.SelectedLanguageCode = "en";
            viewModel.SelectedLanguageCode = "EN";
            viewModel.SelectedTimeZoneId = "UTC";
            viewModel.SelectedTimeZoneId = "UTC";
            Assert.Equal(LocalizationService.Current.UtcTimeZone, viewModel.SelectedTimeZone);
            Assert.Null(viewModel.LanguageOptions.FirstOrDefault(option => option.LanguageCode == "missing"));
            viewModel.Refresh();
            viewModel.StartAuthentication();
            Assert.Contains(nameof(SettingsViewModel.SelectedLanguage), changed);
            Assert.Contains(nameof(SettingsViewModel.SelectedTimeZone), changed);
            viewModel.Dispose();
            viewModel.Dispose();
        }
        finally
        {
            App.CurrentSettings = originalSettings;
            root.Delete(recursive: true);
        }
    }

    [Fact]
    public void LegalNoticesNavigateWithinBoundsAndRefreshWithLanguage()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            using var main = new MainWindowViewModel(new NeverCalledDetailsClient());
            using var viewModel = new LegalNoticesWindowViewModel(main);
            Assert.True(viewModel.HasNotices);
            Assert.Equal(9, viewModel.PageCount);
            Assert.Equal(0, viewModel.CurrentPageIndex);
            Assert.Equal(1, viewModel.CurrentPageNumber);
            Assert.Equal(viewModel.CurrentPageNumber, viewModel.CurrentPage);
            Assert.True(viewModel.CanGoNext);
            Assert.False(viewModel.CanGoBack);
            Assert.NotEmpty(viewModel.CurrentNoticeName);
            Assert.NotEmpty(viewModel.CurrentNoticeText);
            Assert.NotEmpty(viewModel.PagePositionText);
            Assert.True(viewModel.NextCommand.CanExecute(null));

            viewModel.NextCommand.Execute(null);
            Assert.Equal(1, viewModel.CurrentPageIndex);
            viewModel.BackCommand.Execute(null);
            Assert.Equal(0, viewModel.CurrentPageIndex);
            viewModel.BackCommand.Execute(null);
            Assert.Equal(0, viewModel.CurrentPageIndex);

            for (var index = 0; index < viewModel.PageCount + 2; index++)
            {
                viewModel.NextCommand.Execute(null);
            }
            Assert.Equal(viewModel.PageCount - 1, viewModel.CurrentPageIndex);
            Assert.False(viewModel.CanGoNext);

            LocalizationService.SetLanguage("en");
            Assert.Equal("Back", viewModel.BackText);
            Assert.Equal("Next", viewModel.NextText);
            Assert.Contains("Page", viewModel.PagePositionText, StringComparison.Ordinal);
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void LegalNoticesUseLocalizedNavigationForEveryCatalogLanguage()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            using var main = new MainWindowViewModel(new NeverCalledDetailsClient());
            foreach (var language in LocalizationService.Languages)
            {
                LocalizationService.SetLanguage(language.LanguageCode);
                using var viewModel = new LegalNoticesWindowViewModel(main);
                Assert.False(string.IsNullOrWhiteSpace(viewModel.BackText));
                Assert.False(string.IsNullOrWhiteSpace(viewModel.NextText));
                Assert.False(string.IsNullOrWhiteSpace(viewModel.PagePositionText));
                Assert.Equal(language.LegalCodeName, viewModel.Notices[0].Name);
                Assert.Equal(9, viewModel.PageCount);
            }
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
        }
    }

    [Fact]
    public void SettingsViewModelExposesLocalizedEndpointAndTimezoneChanges()
    {
        var originalLanguage = LocalizationService.Current.LanguageCode;
        var root = Directory.CreateTempSubdirectory("codex-info-settings-labels");
        try
        {
            var viewModel = new SettingsViewModel(new ClientSettingsStore(Path.Combine(root.FullName, "settings.json")));
            var before = viewModel.CurrentEndpoint;
            LocalizationService.SetLanguage("en");
            Assert.NotEqual(before, viewModel.CurrentEndpoint);
            Assert.Equal(LocalizationService.Current.LocalTimeZone, viewModel.SelectedTimeZone);
            viewModel.SelectedTimeZoneId = "UTC";
            Assert.Equal(LocalizationService.Current.UtcTimeZone, viewModel.SelectedTimeZone);
            viewModel.SelectedTimeZoneId = "local";
            Assert.Equal(LocalizationService.Current.LocalTimeZone, viewModel.SelectedTimeZone);
            viewModel.Dispose();
        }
        finally
        {
            LocalizationService.SetLanguage(originalLanguage);
            root.Delete(recursive: true);
        }
    }

    private static ApiDetailsSnapshot ReadyDetails()
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        return PublishedPairTestFixtures.DetailsGeneration(
            ApiState.Ready,
            now,
            true,
            "Pro",
            new ApiQuota(75, now + 604_800, 604_800, false),
            [new ApiDetailsModelUsage("SOL", 1, 2, 3, 0, 0, 0)],
            1);
    }

    private static async Task EventuallyAsync(Func<bool> condition)
    {
        var stopwatch = Stopwatch.StartNew();
        while (!condition())
        {
            if (stopwatch.Elapsed > TimeSpan.FromSeconds(3))
            {
                throw new TimeoutException("Expected presentation state was not reached.");
            }

            await Task.Delay(10);
        }
    }

    private static void WithEnvironment(string name, string? value, Action action)
    {
        var original = Environment.GetEnvironmentVariable(name);
        Environment.SetEnvironmentVariable(name, value);
        try
        {
            action();
        }
        finally
        {
            Environment.SetEnvironmentVariable(name, original);
        }
    }

    private static void WithEnvironment(string name, string? value, Func<Task> action)
    {
        var original = Environment.GetEnvironmentVariable(name);
        Environment.SetEnvironmentVariable(name, value);
        try
        {
            action().GetAwaiter().GetResult();
        }
        finally
        {
            Environment.SetEnvironmentVariable(name, original);
        }
    }

    private sealed class SequenceCoreDetailsClient(params DetailsFetchResult[] results) : HealthyDetailsClientBase
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

    private sealed class DisposableThrowingCombinedClient : HealthyDetailsClientBase, IDisposable
    {
        public bool Disposed { get; private set; }

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("details transport failed");

        public void Dispose() => Disposed = true;
    }

    private sealed class ThrowingDetailsClient : ILoopbackDetailsClient
    {
        public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
            throw new IOException("details transport failed");
    }

    private sealed class CountingDetailsClient(ApiDetailsSnapshot snapshot) : HealthyDetailsClientBase
    {
        public int CallCount { get; private set; }

        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            CallCount++;
            return Task.FromResult(DetailsFetchResult.Success(snapshot));
        }
    }

    private sealed class NeverCalledDetailsClient : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("No details request expected.");
    }

    private sealed class RecordingSettingsSession(ClientSettings initial) : IClientSettingsSession
    {
        public ClientSettings Current { get; private set; } = initial;

        public List<ClientSettings> Generations { get; } = [];

        public void Save(ClientSettings settings)
        {
            Current = settings;
            Generations.Add(settings);
        }
    }
}
