// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Infrastructure;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.Updates;
using CodexInfo.WindowsClient.ViewModels;

namespace CodexInfo.WindowsClient;

public partial class App : Application
{
    public static ClientSettingsStore SettingsStore { get; } = new();

    public static ClientSettings CurrentSettings { get; internal set; } = ClientSettings.Default;

    public static IClientSettingsSession SettingsSession { get; } = new ClientSettingsSession(
        SettingsStore,
        () => CurrentSettings,
        settings => CurrentSettings = settings);

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            var settings = SettingsStore.Load();
            var preview = PreviewEnvironment.Enabled;
            // Visual fixtures are deliberately isolated from the user's
            // persisted setup state.  They must never make a real install
            // appear configured or contact the account endpoint.
            CurrentSettings = preview
                ? settings with
                {
                    SetupCompleted = !PreviewEnvironment.IsSetup,
                    ConnectionConfigured = !PreviewEnvironment.IsSetup,
                }
                : settings;
            LocalizationService.SetLanguage(settings.Language);
            LocalizationService.SetTimeZone(settings.TimeZoneId);
            ILoopbackDetailsClient detailsClient;
            if (preview)
            {
                var client = new PreviewLoopbackClient();
                detailsClient = client;
            }
            else
            {
                var client = new LoopbackStatusClient();
                detailsClient = client;
            }
            var supervisor = preview ? null : new ConnectionSupervisor();
            IWindowsUpdateCoordinator updateCoordinator = preview
                ? new PreviewUpdateCoordinator()
                : new WindowsUpdateCoordinator(
                    new WindowsUpdateClient(),
                    new WindowsInstallerLauncher(),
                    typeof(App).Assembly.GetName().Version ?? new Version(1, 0, 0),
                    Path.Combine(
                        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                        "CodexInfo",
                        "updates"));
            var viewModel = new MainWindowViewModel(
                detailsClient,
                supervisor,
                updateCoordinator);
            desktop.MainWindow = new MainWindow
            {
                DataContext = viewModel,
            };
        }

        base.OnFrameworkInitializationCompleted();
    }
}
