// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia.Controls;
using Avalonia.Input;
using CodexInfo.WindowsClient.ViewModels;
using CodexInfo.WindowsClient.Core;

namespace CodexInfo.WindowsClient;

public partial class SettingsWindow : Window
{
    private readonly MainWindow? mainWindow;
    private string? accountSelectionAtOpen;

    public SettingsWindow() : this(new SettingsViewModel(App.SettingsStore), null) { }

    public SettingsWindow(SettingsViewModel viewModel, MainWindow? mainWindow = null)
    {
        InitializeComponent();
        this.mainWindow = mainWindow;
        DataContext = viewModel;
        Opened += async (_, _) => await viewModel.RefreshRuntimeVersionsAsync();
        Closed += (_, _) => viewModel.Dispose();
    }

    private void OnTitlePointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (e.Source is not Button && e.GetCurrentPoint(this).Properties.PointerUpdateKind == PointerUpdateKind.LeftButtonPressed)
        {
            WindowDragBehavior.Begin(this, e);
        }
    }

    private void OnAccountSelectorCheckedChanged(object? sender, Avalonia.Interactivity.RoutedEventArgs eventArgs)
    {
        var open = AccountSelector.IsChecked == true;
        accountSelectionAtOpen = open
            ? (DataContext as SettingsViewModel)?.SelectedAccount?.Id
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

    private void OnSave(object? sender, Avalonia.Interactivity.RoutedEventArgs e)
    {
        if (DataContext is SettingsViewModel viewModel && viewModel.Save())
        {
            Close();
        }
    }

    private void OnClose(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => Close();

    private void OnOpenSetup(object? sender, Avalonia.Interactivity.RoutedEventArgs e)
    {
        mainWindow?.OpenSetupFromChild();
    }

    private async void OnRefresh(object? sender, Avalonia.Interactivity.RoutedEventArgs e)
    {
        if (DataContext is SettingsViewModel viewModel)
        {
            viewModel.Refresh();
            await viewModel.RefreshRuntimeVersionsAsync();
        }
    }
    private void OnAuth(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => (DataContext as SettingsViewModel)?.StartAuthentication();
    private void OnOpenLegal(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => mainWindow?.OpenLegalFromChild();
}
