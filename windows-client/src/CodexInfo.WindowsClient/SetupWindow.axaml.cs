// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Input.Platform;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.ViewModels;

namespace CodexInfo.WindowsClient;

public partial class SetupWindow : Window
{
    public SetupWindow() : this(CreateDefaultViewModel()) { }

    public SetupWindow(SetupViewModel viewModel)
    {
        InitializeComponent();
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

    private void OnRefresh(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => (DataContext as SetupViewModel)?.Refresh();

    private void OnContinue(object? sender, Avalonia.Interactivity.RoutedEventArgs e)
    {
        if (DataContext is SetupViewModel viewModel
            && viewModel.Advance() == SetupAdvanceOutcome.CloseRequested)
        {
            Close();
        }
    }

    private async void OnCopySsh(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => await CopyTextAsync((DataContext as SetupViewModel)?.SshCommand);

    private void OnStartOrStopSsh(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => (DataContext as SetupViewModel)?.StartOrStopSsh();

    private async void OnCopyApi(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => await CopyTextAsync((DataContext as SetupViewModel)?.ApiCommand);

    private async Task CopyTextAsync(string? text)
    {
        if (!string.IsNullOrEmpty(text) && Clipboard is { } clipboard) await clipboard.SetTextAsync(text);
    }

    private void OnClose(object? sender, Avalonia.Interactivity.RoutedEventArgs e) => Close();

    private static SetupViewModel CreateDefaultViewModel()
    {
        var client = new LoopbackStatusClient();
        return new SetupViewModel(new MainWindowViewModel(client));
    }
}
