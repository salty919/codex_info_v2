// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections.ObjectModel;
using System.Reflection;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.Primitives;
using Avalonia.Headless;
using Avalonia.Interactivity;
using Avalonia.Themes.Fluent;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphSelectInteractionTests
{
    [Fact]
    public Task PeriodDirectoryRefreshPreservesTheOpenBoundList() => RunOnUiThread(() =>
    {
        var current = new ApiHistoryPeriod("current", 2_000, 3_000, true, "Current");
        var past = new ApiHistoryPeriod("past", 1_000, 2_000, false, "History");
        var periods = new ObservableCollection<ApiHistoryPeriod> { current, past };
        var selector = new GraphSelect
        {
            ItemsSource = periods,
            SelectedItem = past,
            IsChecked = true,
        };
        var window = new Window { Content = selector };
        window.Show();
        var list = selector.FindControl<ListBox>("MenuList");
        Assert.NotNull(list);
        Assert.Same(periods, list.ItemsSource);
        Assert.Same(past, list.SelectedItem);
        Invoke(selector, "OnFieldClick", new RoutedEventArgs(Button.ClickEvent));
        Assert.True(selector.IsOpen);

        var refreshed = past with { Label = "Updated history" };
        periods.Clear();
        periods.Add(current);
        periods.Add(refreshed);
        selector.SelectedItem = refreshed;

        Assert.Same(refreshed, list.SelectedItem);
        Assert.True(selector.IsOpen);
        Assert.True(selector.IsChecked);
        selector.Close();
        window.Close();
    });

    [Theory]
    [InlineData("same-period", true)]
    [InlineData("another-period", false)]
    public Task SelectionUsesPeriodIdentityRatherThanRefreshedRecordIdentity(string nextId, bool remainsOpen) => RunOnUiThread(() =>
    {
        var period = new ApiHistoryPeriod("same-period", 1_000, 2_000, false, "History");
        var selector = OpenSelector(period);
        var refreshed = period with { Id = nextId, Label = "Updated history" };
        Invoke(selector, "OnSelectionChanged", new SelectionChangedEventArgs(
            SelectingItemsControl.SelectionChangedEvent, new[] { period }, new[] { refreshed }));
        Assert.Equal(remainsOpen, selector.IsOpen);
        Assert.Equal(remainsOpen, selector.IsChecked);
        selector.Close();
    });

    [Fact]
    public Task EmptyReplacementEventPreservesMenuUntilExplicitClose() => RunOnUiThread(() =>
    {
        var period = new ApiHistoryPeriod("same-period", 1_000, 2_000, false, "History");
        var selector = OpenSelector(period);
        Invoke(selector, "OnSelectionChanged", new SelectionChangedEventArgs(
            SelectingItemsControl.SelectionChangedEvent, new[] { period }, Array.Empty<object>()));
        Assert.True(selector.IsOpen);
        selector.Close();
        Assert.False(selector.IsOpen);
        Assert.False(selector.IsChecked);
    });

    private static GraphSelect OpenSelector(ApiHistoryPeriod period)
    {
        var selector = new GraphSelect { ItemsSource = new[] { period }, SelectedItem = period, IsChecked = true };
        Invoke(selector, "OnFieldClick", new RoutedEventArgs(Button.ClickEvent));
        Assert.True(selector.IsOpen);
        return selector;
    }

    private static void Invoke(GraphSelect selector, string method, RoutedEventArgs args)
    {
        var handler = typeof(GraphSelect).GetMethod(method, BindingFlags.NonPublic | BindingFlags.Instance);
        Assert.NotNull(handler);
        handler.Invoke(selector, [selector, args]);
    }

    private static async Task RunOnUiThread(Action test)
    {
        using var session = HeadlessUnitTestSession.StartNew(typeof(TestApplication));
        await session.Dispatch(test, CancellationToken.None);
    }

    public sealed class TestApplication : Application
    {
        public TestApplication() => Styles.Add(new FluentTheme());

        public static AppBuilder BuildAvaloniaApp() => AppBuilder.Configure<TestApplication>()
            .UseHeadless(new AvaloniaHeadlessPlatformOptions());
    }
}
