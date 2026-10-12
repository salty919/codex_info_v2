// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using Avalonia.Controls;
using Avalonia.Headless;
using Avalonia.Interactivity;
using Avalonia.VisualTree;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class SetupWindowInteractionTests
{
    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task AuthenticationButtonStartsOnceWithoutCompletingSetup(bool launchSucceeds)
    {
        using var session = HeadlessUnitTestSession.StartNew(typeof(TestApplication));
        await session.Dispatch(async () =>
        {
            var calls = 0;
            var oldLanguage = LocalizationService.Current.LanguageCode;
            LocalizationService.SetLanguage("en");
            var details = PublishedPairTestFixtures.DetailsGeneration(
                ApiState.AuthRequired, null, false, null, null, [], 0);
            using var main = new MainWindowViewModel(new AuthRequiredClient(details),
                authenticationLauncher: () => { calls++; return launchSucceeds; });
            var settings = new MemorySettings();
            using var setup = new SetupViewModel(main, settings);
            var window = new SetupWindow(setup);
            try
            {
                main.Start();
                using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                while (!main.IsAuthRequired)
                    await Task.Delay(10, timeout.Token);
                Assert.Equal(SetupAdvanceOutcome.StayOpen, setup.Advance());
                Assert.True(setup.IsAuthStep);
                var savesBeforeClick = settings.SaveCalls;
                window.Show();
                var button = Assert.Single(window.GetVisualDescendants().OfType<Button>(),
                    control => Equals(control.Content, "Start authentication"));
                Assert.True(button.IsEffectivelyVisible);
                Assert.True(button.IsEffectivelyEnabled);
                Assert.True(button.Bounds.Width > 0 && button.Bounds.Height > 0);
                button.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                while (calls == 0)
                    await Task.Delay(10, timeout.Token);

                Assert.Equal(1, calls);
                Assert.False(main.IsAuthenticated);
                Assert.True(setup.IsAuthStep);
                Assert.False(setup.CanContinue);
                Assert.False(settings.Current.SetupCompleted);
                Assert.Equal(savesBeforeClick, settings.SaveCalls);
                Assert.Equal(launchSucceeds, main.IsAuthCheckVisible);
                Assert.Equal(!launchSucceeds, main.IsAuthStartVisible);
            }
            finally
            {
                window.Close();
                LocalizationService.SetLanguage(oldLanguage);
            }
        }, CancellationToken.None);
    }

    private sealed class MemorySettings : IClientSettingsSession
    {
        public ClientSettings Current { get; private set; } = ClientSettings.Default;
        public int SaveCalls { get; private set; }
        public void Save(ClientSettings settings)
        {
            Current = settings;
            SaveCalls++;
        }
    }

    private sealed class AuthRequiredClient(ApiDetailsSnapshot details) : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(DetailsFetchResult.Success(details));
    }

    public sealed class TestApplication
    {
        public static AppBuilder BuildAvaloniaApp() => AppBuilder.Configure<App>()
            .UseHeadless(new AvaloniaHeadlessPlatformOptions());
    }
}
