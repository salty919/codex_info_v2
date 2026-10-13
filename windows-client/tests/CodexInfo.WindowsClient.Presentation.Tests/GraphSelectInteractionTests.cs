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
using CodexInfo.WindowsClient.Localization;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphSelectInteractionTests
{
    [Fact]
    public Task DateTreeInputsNeverStartGraphWindowDragging() => RunOnUiThread(() =>
    {
        var window = new GraphWindow();
        var predicate = typeof(GraphWindow).GetMethod("IsInteractiveSource", BindingFlags.NonPublic | BindingFlags.Instance);
        Assert.NotNull(predicate);
        Assert.Equal(true, predicate.Invoke(window, [new TreeView()]));
        Assert.Equal(true, predicate.Invoke(window, [new TreeViewItem()]));
        Assert.Equal(false, predicate.Invoke(window, [new TextBlock()]));
        window.Close();
    });

    [Fact]
    public Task GroupedPeriodsKeepEveryExactIdSelectableAcrossDirectoryRefresh() => RunOnUiThread(() =>
    {
        var previousZone = LocalizationService.DisplayTimeZone.Id;
        LocalizationService.SetTimeZone("UTC");
        try
        {
            var first = new ApiHistoryPeriod("reset-a", DateTimeOffset.Parse("2026-10-13T00:30:00Z").ToUnixTimeSeconds(), 1_900_000_000, false, "05:45");
            var second = first with { Id = "reset-b", StartAt = first.StartAt + 60, Label = "05:46" };
            var previous = first with { Id = "previous", StartAt = first.StartAt - 3600, Label = "Previous" };
            var periods = new ObservableCollection<ApiHistoryPeriod> { second, first, previous };
            var selector = new GraphSelect { ItemsSource = periods, SelectedItem = first, IsChecked = true };
            var grouped = typeof(GraphSelect).GetProperty("GroupPeriodsByDate");
            Assert.NotNull(grouped);
            grouped.SetValue(selector, true);
            var window = new Window { Content = selector };
            window.Show();
            Invoke(selector, "OnFieldClick", new RoutedEventArgs(Button.ClickEvent));
            var tree = selector.FindControl<TreeView>("PeriodTree");
            Assert.NotNull(tree);
            var groups = tree.ItemsSource!.Cast<object>().ToArray();
            Assert.Equal(["2026/10/13", "2026/10/12"], groups.Select(g => g.GetType().GetProperty("Label")!.GetValue(g)));
            var children = (IEnumerable<ApiHistoryPeriod>)groups[0].GetType().GetProperty("Periods")!.GetValue(groups[0])!;
            Assert.Equal(["reset-b", "reset-a"], children.Select(p => p.Id));
            Avalonia.Threading.Dispatcher.UIThread.RunJobs();
            var container = Assert.IsType<TreeViewItem>(tree.ContainerFromIndex(0));
            Assert.Equal(2, container.ItemCount);
            var click = container.TranslatePoint(new Point(60, 16), window);
            Assert.NotNull(click);
            window.MouseDown(click.Value, Avalonia.Input.MouseButton.Left);
            window.MouseUp(click.Value, Avalonia.Input.MouseButton.Left);
            Avalonia.Threading.Dispatcher.UIThread.RunJobs();
            Assert.True(container.IsExpanded);
            Assert.Equal(true, groups[0].GetType().GetProperty("IsExpanded")!.GetValue(groups[0]));
            tree.SelectedItem = groups[0];
            Assert.True(selector.IsOpen);
            Assert.Same(first, selector.SelectedItem);
            Assert.Same(first, tree.SelectedItem);
            periods.Clear();
            var refreshed = first with { Label = "Updated first" };
            periods.Add(second);
            periods.Add(refreshed);
            periods.Add(previous);
            selector.SelectedItem = refreshed;
            Avalonia.Threading.Dispatcher.UIThread.RunJobs();
            Assert.True(Assert.IsType<TreeViewItem>(tree.ContainerFromIndex(0)).IsExpanded);
            Assert.True(selector.IsOpen);
            Assert.Same(refreshed, tree.SelectedItem);
            tree.SelectedItem = second;
            Assert.Same(second, selector.SelectedItem);
            Assert.False(selector.IsOpen);
            window.Close();
        }
        finally { LocalizationService.SetTimeZone(previousZone); }
    });

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
