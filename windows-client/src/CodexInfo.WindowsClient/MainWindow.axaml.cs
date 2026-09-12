// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia.Controls;
using Avalonia.Controls.Primitives;
using Avalonia;
using Avalonia.Input;
using Avalonia.Threading;
using Avalonia.VisualTree;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.ViewModels;

namespace CodexInfo.WindowsClient;

public partial class MainWindow : Window
{
    private GraphWindow? graphWindow;
    private ThreadsWindow? threadsWindow;
    private LegalNoticesWindow? legalNoticesWindow;
    private SettingsWindow? settingsWindow;
    private SetupWindow? setupWindow;
    private string? accountSelectionAtOpen;

    public MainWindow()
    {
        InitializeComponent();
        Opened += OnOpened;
        Closed += OnClosed;
    }

    private void OnOpened(object? sender, EventArgs eventArgs)
    {
        // Windows can retain an off-screen logical position when a WSL-launched
        // client crosses a DPI boundary.  Re-center the freshly-created window
        // in physical pixels so the complete surface is always reachable.
        Dispatcher.UIThread.Post(CenterOnScreen, DispatcherPriority.Loaded);

        if (DataContext is MainWindowViewModel viewModel)
        {
            viewModel.Start();
        }

        if (PreviewEnvironment.IsChild("graph"))
        {
            Dispatcher.UIThread.Post(OpenGraph, DispatcherPriority.Loaded);
        }
        else if (PreviewEnvironment.IsChild("threads"))
        {
            Dispatcher.UIThread.Post(OpenThreads, DispatcherPriority.Loaded);
        }
        else if (PreviewEnvironment.IsChild("legal"))
        {
            Dispatcher.UIThread.Post(OpenLegal, DispatcherPriority.Loaded);
        }
        else if (PreviewEnvironment.IsChild("settings"))
        {
            Dispatcher.UIThread.Post(() => OnOpenSettings(this, new Avalonia.Interactivity.RoutedEventArgs()), DispatcherPriority.Loaded);
        }
        else if (PreviewEnvironment.IsSetup || SetupLaunchPolicy.ShouldOpen(App.CurrentSettings))
        {
            Dispatcher.UIThread.Post(OpenSetup, DispatcherPriority.Loaded);
        }
    }

    private void CenterOnScreen()
    {
        if (Screens.ScreenFromWindow(this) is not { } screen || Bounds.Width <= 0 || Bounds.Height <= 0)
        {
            return;
        }

        var scale = RenderScaling;
        var width = (int)Math.Round(Bounds.Width * scale);
        var height = (int)Math.Round(Bounds.Height * scale);
        var area = screen.WorkingArea;
        Position = new PixelPoint(
            area.X + Math.Max(0, (area.Width - width) / 2),
            area.Y + Math.Max(0, (area.Height - height) / 2));
    }

    private void OnClosed(object? sender, EventArgs eventArgs)
    {
        graphWindow?.Close();
        threadsWindow?.Close();
        legalNoticesWindow?.Close();
        settingsWindow?.Close();
        setupWindow?.Close();
        if (DataContext is MainWindowViewModel viewModel)
        {
            viewModel.Dispose();
        }
    }

    private void OnOpenGraph(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs) => OpenGraph();

    private void OpenGraph()
    {
        if (DataContext is not MainWindowViewModel viewModel)
        {
            return;
        }

        if (graphWindow is { } existing)
        {
            existing.Activate();
            return;
        }

        var window = new GraphWindow(new ViewModels.GraphWindowViewModel(viewModel));
        ApplyPreviewSize(window);
        graphWindow = window;
        window.Closed += (_, _) => graphWindow = null;
        ShowIndependentWindow(window);
    }

    private void OnOpenThreads(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs) => OpenThreads();

    private void OnAccountSelectorCheckedChanged(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs)
    {
        var open = AccountSelector.IsChecked == true;
        accountSelectionAtOpen = open
            ? (DataContext as MainWindowViewModel)?.SelectedAccount?.Id
            : null;
        SetAccountMenuOpen(open);
    }

    private void OnAccountSelectionChanged(object? sender, SelectionChangedEventArgs eventArgs)
    {
        if (!AccountMenu.IsEnabled || sender is not ListBox { SelectedItem: ApiAccount selected } ||
            string.Equals(selected.Id, accountSelectionAtOpen, StringComparison.Ordinal))
        {
            return;
        }

        SetAccountMenuOpen(false);
        AccountSelector.IsChecked = false;
    }

    private void SetAccountMenuOpen(bool open)
    {
        AccountMenu.Opacity = open ? 1 : 0;
        AccountMenu.IsEnabled = open;
        AccountMenu.IsHitTestVisible = open;
    }

    private void OpenThreads()
    {
        if (DataContext is not MainWindowViewModel viewModel)
        {
            return;
        }

        if (threadsWindow is { } existing)
        {
            existing.Activate();
            return;
        }

        var window = new ThreadsWindow(new ViewModels.ThreadsWindowViewModel(viewModel));
        ApplyPreviewSize(window);
        threadsWindow = window;
        window.Closed += (_, _) => threadsWindow = null;
        ShowIndependentWindow(window);
    }

    private void OnOpenLegal(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs) => OpenLegal();

    private void OpenLegal()
    {
        if (DataContext is not MainWindowViewModel viewModel)
        {
            return;
        }

        if (legalNoticesWindow is { } existing)
        {
            existing.Activate();
            return;
        }

        var window = new LegalNoticesWindow(new ViewModels.LegalNoticesWindowViewModel(viewModel));
        ApplyPreviewSize(window);
        legalNoticesWindow = window;
        window.Closed += (_, _) => legalNoticesWindow = null;
        ShowIndependentWindow(window);
    }

    private void OnOpenSettings(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs)
    {
        if (settingsWindow is { } existing)
        {
            existing.Activate();
            return;
        }

        var window = new SettingsWindow(new SettingsViewModel(App.SettingsStore, DataContext as MainWindowViewModel), this);
        ApplyPreviewSize(window);
        settingsWindow = window;
        window.Closed += (_, _) => settingsWindow = null;
        ShowIndependentWindow(window);
    }

    public void OpenSetupFromChild() => OpenSetup();

    public void OpenLegalFromChild() => OnOpenLegal(this, new Avalonia.Interactivity.RoutedEventArgs());

    private void OpenSetup()
    {
        if (setupWindow is { } existing)
        {
            existing.Activate();
            return;
        }

        if (DataContext is not MainWindowViewModel viewModel)
        {
            return;
        }

        var window = new SetupWindow(new SetupViewModel(viewModel));
        ApplyPreviewSize(window);
        setupWindow = window;
        window.Closed += (_, _) => setupWindow = null;
        ShowIndependentWindow(window);
    }

    private void ShowIndependentWindow(Window window)
    {
        window.WindowStartupLocation = WindowStartupLocation.Manual;
        if (Screens.ScreenFromWindow(this) is { } screen)
        {
            var scale = screen.Scaling;
            var width = (int)Math.Round(window.Width * scale);
            var height = (int)Math.Round(window.Height * scale);
            var area = screen.WorkingArea;
            window.Position = new PixelPoint(
                area.X + Math.Max(0, (area.Width - width) / 2),
                area.Y + Math.Max(0, (area.Height - height) / 2));
        }

        window.Show();
    }

    private static void ApplyPreviewSize(Window window)
    {
        if (!PreviewEnvironment.Enabled || !PreviewEnvironment.TryGetSize(out var width, out var height))
        {
            return;
        }

        window.Width = width;
        window.Height = height;
    }

    private void OnTitlePointerPressed(object? sender, PointerPressedEventArgs eventArgs)
    {
        if (!IsInteractiveSource(eventArgs.Source) &&
            eventArgs.GetCurrentPoint(this).Properties.PointerUpdateKind == PointerUpdateKind.LeftButtonPressed)
        {
            WindowDragBehavior.Begin(this, eventArgs);
        }
    }

    private bool IsInteractiveSource(object? source)
    {
        for (var current = source as Visual;
             current is not null && !ReferenceEquals(current, this);
             current = current.GetVisualParent())
        {
            if (current is Button or ListBox or ListBoxItem or ScrollBar or TextBox or ComboBox)
            {
                return true;
            }
        }

        return false;
    }

    private void OnMinimizeWindow(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs)
    {
        WindowState = WindowState.Minimized;
    }

    private void OnCloseWindow(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs)
    {
        Close();
    }
}
