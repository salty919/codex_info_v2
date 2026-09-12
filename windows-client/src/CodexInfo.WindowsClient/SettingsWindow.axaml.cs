// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia.Controls;
using Avalonia.Input;
using CodexInfo.WindowsClient.ViewModels;

namespace CodexInfo.WindowsClient;

public partial class SettingsWindow : Window
{
    private readonly MainWindow? mainWindow;

    public SettingsWindow() : this(new SettingsViewModel(App.SettingsStore), null) { }

    public SettingsWindow(SettingsViewModel viewModel, MainWindow? mainWindow = null)
    {
        InitializeComponent();
        this.mainWindow = mainWindow;
        DataContext = viewModel;
        Closed += (_, _) => viewModel.Dispose();
    }

    private void OnTitlePointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (e.Source is not Button && e.GetCurrentPoint(this).Properties.PointerUpdateKind == PointerUpdateKind.LeftButtonPressed)
        {
            WindowDragBehavior.Begin(this, e);
        }
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

    private void OnRefresh(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => (DataContext as SettingsViewModel)?.Refresh();
    private void OnAuth(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => (DataContext as SettingsViewModel)?.StartAuthentication();
    private void OnOpenLegal(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => mainWindow?.OpenLegalFromChild();
}
