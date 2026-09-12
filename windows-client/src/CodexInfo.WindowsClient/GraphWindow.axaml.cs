// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.Primitives;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.VisualTree;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.ViewModels;

namespace CodexInfo.WindowsClient;

public partial class GraphWindow : Window
{
    private string? periodSelectionAtOpen;
    private string? metricSelectionAtOpen;
    private string? accountSelectionAtOpen;

    public GraphWindow()
    {
        InitializeComponent();
        AttachMenuDismissHandlers();
        AttachResponsiveLayout();
    }

    public GraphWindow(GraphWindowViewModel viewModel)
    {
        InitializeComponent();
        AttachMenuDismissHandlers();
        AttachResponsiveLayout();
        DataContext = viewModel;
        Closed += (_, _) => viewModel.Dispose();
    }

    private void AttachResponsiveLayout()
    {
        SizeChanged += (_, _) => UpdateContentMargin();
        UpdateContentMargin();
    }

    private void UpdateContentMargin()
    {
        var horizontal = Math.Clamp(20d + ((Bounds.Width - 700d) / 24d), 20d, 30d);
        GraphContent.Margin = new Thickness(horizontal, 24d, horizontal, 24d);
    }

    private void OnMinimizeWindow(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs)
    {
        WindowState = WindowState.Minimized;
    }

    private void OnMaximizeWindow(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs)
    {
        WindowState = WindowState == WindowState.Maximized ? WindowState.Normal : WindowState.Maximized;
    }

    private void OnCloseWindow(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs)
    {
        Close();
    }

    private void OnPeriodSelectorCheckedChanged(object? sender, RoutedEventArgs eventArgs)
    {
        var open = PeriodSelector.IsChecked == true;
        periodSelectionAtOpen = open
            ? (DataContext as GraphWindowViewModel)?.SelectedPeriod?.Id
            : null;
        SetMenuOpen(PeriodMenu, open);
        if (open)
        {
            SetMenuOpen(MetricMenu, false);
            MetricSelector.IsChecked = false;
        }
    }

    private void OnMetricSelectorCheckedChanged(object? sender, RoutedEventArgs eventArgs)
    {
        var open = MetricSelector.IsChecked == true;
        metricSelectionAtOpen = open
            ? (DataContext as GraphWindowViewModel)?.SelectedMetric
            : null;
        SetMenuOpen(MetricMenu, open);
        if (open)
        {
            SetMenuOpen(PeriodMenu, false);
            PeriodSelector.IsChecked = false;
        }
    }

    private void OnAccountSelectorCheckedChanged(object? sender, RoutedEventArgs eventArgs)
    {
        var open = AccountSelector.IsChecked == true;
        accountSelectionAtOpen = open
            ? (DataContext as GraphWindowViewModel)?.SelectedAccount?.Id
            : null;
        SetMenuOpen(AccountMenu, open);
        if (open)
        {
            SetMenuOpen(PeriodMenu, false);
            SetMenuOpen(MetricMenu, false);
            PeriodSelector.IsChecked = false;
            MetricSelector.IsChecked = false;
        }
    }

    private void OnPeriodSelectionChanged(object? sender, SelectionChangedEventArgs eventArgs)
    {
        if (!PeriodMenu.IsEnabled || sender is not ListBox { SelectedItem: ApiHistoryPeriod selected } ||
            string.Equals(selected.Id, periodSelectionAtOpen, StringComparison.Ordinal))
        {
            return;
        }

        SetMenuOpen(PeriodMenu, false);
        PeriodSelector.IsChecked = false;
    }

    private void OnMetricSelectionChanged(object? sender, SelectionChangedEventArgs eventArgs)
    {
        if (!MetricMenu.IsEnabled || sender is not ListBox { SelectedItem: string selected } ||
            string.Equals(selected, metricSelectionAtOpen, StringComparison.Ordinal))
        {
            return;
        }

        SetMenuOpen(MetricMenu, false);
        MetricSelector.IsChecked = false;
    }

    private void OnAccountSelectionChanged(object? sender, SelectionChangedEventArgs eventArgs)
    {
        if (!AccountMenu.IsEnabled || sender is not ListBox { SelectedItem: ApiAccount selected } ||
            string.Equals(selected.Id, accountSelectionAtOpen, StringComparison.Ordinal))
        {
            return;
        }

        SetMenuOpen(AccountMenu, false);
        AccountSelector.IsChecked = false;
    }

    private static void SetMenuOpen(Control menu, bool open)
    {
        // Keep the bounded list measured so opening is a compositor-only
        // opacity change instead of synchronous template creation/layout.
        menu.Opacity = open ? 1 : 0;
        menu.IsEnabled = open;
        menu.IsHitTestVisible = open;
    }

    private void AttachMenuDismissHandlers()
    {
        AddHandler(
            InputElement.PointerPressedEvent,
            OnWindowPointerPressed,
            RoutingStrategies.Tunnel);
        KeyDown += OnWindowKeyDown;
    }

    private void OnWindowPointerPressed(object? sender, PointerPressedEventArgs eventArgs)
    {
        if (IsResizeTarget(eventArgs.Source))
        {
            return;
        }

        if (IsWithin(eventArgs.Source, PeriodSelector) ||
            IsWithin(eventArgs.Source, PeriodMenu) ||
            IsWithin(eventArgs.Source, MetricSelector) ||
            IsWithin(eventArgs.Source, MetricMenu) ||
            IsWithin(eventArgs.Source, AccountSelector) ||
            IsWithin(eventArgs.Source, AccountMenu))
        {
            return;
        }

        if (PeriodMenu.IsEnabled || MetricMenu.IsEnabled || AccountMenu.IsEnabled)
        {
            CloseMenus();
            eventArgs.Handled = true;
            return;
        }

        if (IsInteractiveSource(eventArgs.Source))
        {
            return;
        }

        if (eventArgs.GetCurrentPoint(this).Properties.PointerUpdateKind == PointerUpdateKind.LeftButtonPressed)
        {
            WindowDragBehavior.Begin(this, eventArgs);
            eventArgs.Handled = true;
        }
    }

    private void OnWindowKeyDown(object? sender, KeyEventArgs eventArgs)
    {
        if (eventArgs.Key != Key.Escape)
        {
            return;
        }

        CloseMenus();
        eventArgs.Handled = true;
    }

    private void CloseMenus()
    {
        SetMenuOpen(PeriodMenu, false);
        SetMenuOpen(MetricMenu, false);
        SetMenuOpen(AccountMenu, false);
        PeriodSelector.IsChecked = false;
        MetricSelector.IsChecked = false;
        AccountSelector.IsChecked = false;
    }

    private static bool IsWithin(object? source, Visual ancestor)
    {
        for (var current = source as Visual; current is not null; current = current.GetVisualParent())
        {
            if (ReferenceEquals(current, ancestor))
            {
                return true;
            }
        }

        return false;
    }

    private bool IsResizeTarget(object? source) =>
        IsWithin(source, ResizeNorthWest) || IsWithin(source, ResizeNorth) ||
        IsWithin(source, ResizeNorthEast) || IsWithin(source, ResizeWest) ||
        IsWithin(source, ResizeEast) || IsWithin(source, ResizeSouthWest) ||
        IsWithin(source, ResizeSouth) || IsWithin(source, ResizeSouthEast);

    private bool IsInteractiveSource(object? source)
    {
        for (var current = source as Visual; current is not null && !ReferenceEquals(current, this); current = current.GetVisualParent())
        {
            if (current is Button or ListBox or ListBoxItem or ScrollBar or Thumb or TextBox or ComboBox)
            {
                return true;
            }
        }

        return false;
    }

    private void BeginResize(WindowEdge edge, PointerPressedEventArgs eventArgs)
    {
        if (eventArgs.GetCurrentPoint(this).Properties.PointerUpdateKind == PointerUpdateKind.LeftButtonPressed)
        {
            BeginResizeDrag(edge, eventArgs);
        }
    }

    private void OnResizeNorthWest(object? sender, PointerPressedEventArgs e) => BeginResize(WindowEdge.NorthWest, e);
    private void OnResizeNorth(object? sender, PointerPressedEventArgs e) => BeginResize(WindowEdge.North, e);
    private void OnResizeNorthEast(object? sender, PointerPressedEventArgs e) => BeginResize(WindowEdge.NorthEast, e);
    private void OnResizeWest(object? sender, PointerPressedEventArgs e) => BeginResize(WindowEdge.West, e);
    private void OnResizeEast(object? sender, PointerPressedEventArgs e) => BeginResize(WindowEdge.East, e);
    private void OnResizeSouthWest(object? sender, PointerPressedEventArgs e) => BeginResize(WindowEdge.SouthWest, e);
    private void OnResizeSouth(object? sender, PointerPressedEventArgs e) => BeginResize(WindowEdge.South, e);
    private void OnResizeSouthEast(object? sender, PointerPressedEventArgs e) => BeginResize(WindowEdge.SouthEast, e);
}
