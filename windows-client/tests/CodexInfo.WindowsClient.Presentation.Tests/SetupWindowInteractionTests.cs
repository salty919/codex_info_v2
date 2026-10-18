// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using Avalonia.Controls;
using Avalonia.Automation;
using Avalonia.Input;
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
        await session.Dispatch<bool>(async () =>
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
            return true;
        }, CancellationToken.None);
    }

    [Theory]
    [InlineData("Setup")]
    [InlineData("Settings")]
    public async Task HeaderMinimizeInvokesTheWindowWithoutChangingFixedGeometry(string surface)
    {
        using var session = HeadlessUnitTestSession.StartNew(typeof(TestApplication));
        await session.Dispatch<bool>(() =>
        {
            var oldLanguage = LocalizationService.Current.LanguageCode;
            LocalizationService.SetLanguage("en");
            var temporary = Directory.CreateTempSubdirectory("codex-info-window-controls");
            using var main = new MainWindowViewModel(new AuthRequiredClient(
                PublishedPairTestFixtures.DetailsGeneration(ApiState.AuthRequired, null, false, null, null, [], 0)));
            using var setup = new SetupViewModel(main, new MemorySettings());
            var window = surface == "Setup"
                ? (Window)new SetupWindow(setup)
                : new SettingsWindow(new SettingsViewModel(new ClientSettingsStore(Path.Combine(temporary.FullName, "settings.json"))));
            try
            {
                window.Show();
                Assert.Equal(new Size(900, 480), window.ClientSize);
                Assert.False(window.CanResize);
                var minimize = WindowButton(window, surface + ".Window.Minimize");
                var close = WindowButton(window, surface + ".Window.Close");
                AssertVisibleAction(minimize);
                AssertVisibleAction(close);
                Assert.Equal("Minimize", AutomationProperties.GetName(minimize));
                Assert.Equal(close.TranslatePoint(default, window)!.Value.Y,
                    minimize.TranslatePoint(default, window)!.Value.Y);
                minimize.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                Assert.Equal(WindowState.Minimized, window.WindowState);
                Assert.Equal(900, window.Width);
                Assert.Equal(480, window.Height);
                window.WindowState = WindowState.Normal;
                close.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                Assert.False(window.IsVisible);
            }
            finally
            {
                window.Close();
                temporary.Delete(recursive: true);
                LocalizationService.SetLanguage(oldLanguage);
            }
            return Task.FromResult(true);
        }, CancellationToken.None);
    }

    [Fact]
    public async Task BackAndEscapePreserveSavedSettingsAndBusyCloseIgnoresLateSharedStatus()
    {
        using var session = HeadlessUnitTestSession.StartNew(typeof(TestApplication));
        await session.Dispatch<bool>(async () =>
        {
            var oldLanguage = LocalizationService.Current.LanguageCode;
            LocalizationService.SetLanguage("en");
            var at = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
            var ready = PublishedPairTestFixtures.DetailsGeneration(ApiState.Ready, at, true,
                "Pro", new ApiQuota(75, at + 604_800, 604_800, false), [], 0);
            var client = new DeferredDetailsClient(ready);
            using var main = new MainWindowViewModel(client);
            var settings = new MemorySettings();
            using var setup = new SetupViewModel(main, settings);
            var window = new SetupWindow(setup);
            try
            {
                main.Start();
                using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                while (!main.IsAuthenticated || !main.CanRefresh) await Task.Delay(10, timeout.Token);
                window.Show();
                var back = WindowButton(window, "Setup.Back");
                AssertVisibleAction(back, enabled: false);
                var next = Assert.Single(window.GetVisualDescendants().OfType<Button>(),
                    button => Equals(button.Content, "Continue"));
                next.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                Assert.Equal(1, setup.Step);
                Assert.Equal(1, settings.SaveCalls);
                next.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                Assert.Equal(2, setup.Step);
                Assert.Equal(1, settings.SaveCalls);
                AssertVisibleAction(back);
                back.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                Assert.Equal(1, setup.Step);
                Assert.Equal(1, settings.SaveCalls);
                window.KeyPress(Key.Escape, RawInputModifiers.None, PhysicalKey.Escape, null);
                window.KeyRelease(Key.Escape, RawInputModifiers.None, PhysicalKey.Escape, null);
                Assert.Equal(0, setup.Step);
                Assert.True(window.IsVisible);
                Assert.Equal(1, settings.SaveCalls);
                window.RaiseEvent(new KeyEventArgs
                {
                    RoutedEvent = InputElement.KeyDownEvent,
                    Key = Key.Escape,
                    Handled = true,
                });
                Assert.True(window.IsVisible);
                Assert.Equal(0, setup.Step);
                next.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                Assert.Equal(1, setup.Step);
                Assert.Equal(2, settings.SaveCalls);
                client.HoldNext = true;
                WindowButton(window, "Setup.Refresh").RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                await client.Requested.Task.WaitAsync(timeout.Token);
                Assert.False(main.CanRefresh);
                Assert.False(back.IsEffectivelyEnabled);
                Assert.True(WindowButton(window, "Setup.Window.Close").IsEffectivelyEnabled);
                // Closing destroys the headless TopLevel during key-down;
                // key-up belongs to the remaining application input target.
                window.KeyPress(Key.Escape, RawInputModifiers.None, PhysicalKey.Escape, null);
                Assert.False(window.IsVisible);
                Assert.Equal(1, setup.Step);
                Assert.Equal(2, settings.SaveCalls);
                client.Late.TrySetResult(DetailsFetchResult.Success(PublishedPairTestFixtures.DetailsGeneration(
                    ApiState.AuthRequired, null, false, null, null, [], 0)));
                while (!main.IsAuthRequired || !main.CanRefresh) await Task.Delay(10, timeout.Token);
                Assert.Equal(1, setup.Step);
                Assert.Equal(2, settings.SaveCalls);
                Assert.False(settings.Current.SetupCompleted);
                Assert.False(window.IsVisible);
            }
            finally
            {
                window.Close();
                LocalizationService.SetLanguage(oldLanguage);
            }
            return true;
        }, CancellationToken.None);
    }

    private static Button WindowButton(Window window, string id) =>
        Assert.Single(window.GetVisualDescendants().OfType<Button>(),
            button => AutomationProperties.GetAutomationId(button) == id);

    private static void AssertVisibleAction(Button button, bool enabled = true)
    {
        Assert.True(button.IsEffectivelyVisible);
        Assert.Equal(enabled, button.IsEffectivelyEnabled);
        Assert.True(button.Bounds.Width > 0 && button.Bounds.Height > 0);
    }

    private sealed class DeferredDetailsClient(ApiDetailsSnapshot initial) : HealthyDetailsClientBase
    {
        public bool HoldNext { get; set; }
        public TaskCompletionSource Requested { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);
        public TaskCompletionSource<DetailsFetchResult> Late { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default)
        {
            if (!HoldNext) return Task.FromResult(DetailsFetchResult.Success(initial));
            Requested.TrySetResult();
            return Late.Task;
        }
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
            .UseSkia()
            .UseHeadless(new AvaloniaHeadlessPlatformOptions { UseHeadlessDrawing = false });
    }
}
