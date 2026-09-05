// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows.Input;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Updates;

namespace CodexInfo.WindowsClient.ViewModels;

/// <summary>
/// Owns the presentation state for the shared Windows update flow. A normal
/// UI startup performs the same bounded check/start transition as the
/// headless update-only trigger; the command remains available for retry.
/// </summary>
public sealed class UpdateViewModel : INotifyPropertyChanged, IDisposable
{
    private readonly IWindowsUpdateCoordinator coordinator;
    private readonly UpdateAsyncCommand updateCommand;
    private readonly CancellationTokenSource lifetime = new();
    private string? availableVersion;
    private UpdateStartStatus? startStatus;
    private bool busy;
    private bool disposed;
    private int started;
    private int updateStarted;
    private Task checkTask = Task.CompletedTask;

    public UpdateViewModel(IWindowsUpdateCoordinator coordinator)
    {
        this.coordinator = coordinator ?? throw new ArgumentNullException(nameof(coordinator));
        updateCommand = new UpdateAsyncCommand(this);
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public string? AvailableVersion => availableVersion;

    public bool HasAvailableUpdate => !string.IsNullOrWhiteSpace(availableVersion) &&
        startStatus is not UpdateStartStatus.Started and
        not UpdateStartStatus.NoAvailableUpdate and
        not UpdateStartStatus.Busy;

    public bool IsBusy => busy;

    /// <summary>True when the shared status banner has update information to show.</summary>
    public bool IsNotificationVisible => busy || HasAvailableUpdate || startStatus is not null && startStatus != UpdateStartStatus.NoAvailableUpdate;

    public bool IsUpdateActionVisible => HasAvailableUpdate && !busy && !disposed;

    public UpdateStartStatus? StartStatus => startStatus;

    public ICommand UpdateCommand => updateCommand;

    public string ActionText => startStatus is
        UpdateStartStatus.DownloadFailed or
        UpdateStartStatus.IntegrityFailed or
        UpdateStartStatus.LaunchFailed or
        UpdateStartStatus.DiscoveryFailed or
        UpdateStartStatus.OldVersionFailed or
        UpdateStartStatus.SafeBlocked
        ? LocalizationService.Current.Retry
        : LocalizationService.Current.UpdateButtonText;

    public string NotificationText
    {
        get
        {
            var texts = LocalizationService.Current;
            if (busy || startStatus == UpdateStartStatus.Busy) return texts.UpdatePreparing;
            var statusText = startStatus switch
            {
                UpdateStartStatus.Started => texts.UpdateStarted,
                UpdateStartStatus.DownloadFailed => texts.UpdateDownloadFailed,
                UpdateStartStatus.IntegrityFailed => texts.UpdateIntegrityFailed,
                UpdateStartStatus.LaunchFailed => texts.UpdateLaunchFailed,
                UpdateStartStatus.DiscoveryFailed => texts.UpdateDownloadFailed,
                UpdateStartStatus.OldVersionFailed => texts.UpdateLaunchFailed,
                UpdateStartStatus.SafeBlocked => texts.UpdateLaunchFailed,
                _ => string.Empty,
            };
            return statusText.Length > 0
                ? statusText
                : HasAvailableUpdate
                    ? texts.UpdateAvailableText(FormatVersion(availableVersion!))
                    : string.Empty;
        }
    }

    // StatusText is a small presentation alias for callers that do not need
    // to know that the notification is rendered in StatusBanner.
    public string StatusText => NotificationText;

    /// <summary>Starts the one background check and returns the check task.</summary>
    public Task StartAsync()
    {
        if (disposed) return Task.CompletedTask;
        if (Interlocked.Exchange(ref started, 1) == 0)
        {
            checkTask = CheckInBackgroundAsync(lifetime.Token);
        }

        return checkTask;
    }

    public void Start() => _ = StartAsync();

    /// <summary>Runs the explicit update action. The command is the normal caller.</summary>
    public Task StartAvailableUpdateAsync() => RunUpdateAsync();

    public void Dispose()
    {
        if (Interlocked.Exchange(ref disposed, true)) return;

        lifetime.Cancel();
        updateCommand.RaiseCanExecuteChanged();
        coordinator.Dispose();
        lifetime.Dispose();
    }

    private async Task CheckInBackgroundAsync(CancellationToken cancellationToken)
    {
        UpdateCheckResult result;
        try
        {
            result = await coordinator.CheckAsync(cancellationToken);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            return;
        }
        catch
        {
            // A failed check is deliberately silent. In particular it must
            // not manufacture an available update or alter backend state.
            result = new UpdateCheckResult(null, IsFailure: true);
        }

        if (disposed) return;

        availableVersion = result.IsFailure || string.IsNullOrWhiteSpace(result.AvailableVersion)
            ? null
            : result.AvailableVersion.Trim();
        startStatus = null;
        NotifyState();

        // Startup convergence is deliberately independent of the backend
        // details/auth pipeline. The coordinator owns release validation,
        // exclusive lease, pending state, and Setup launch for every trigger.
        if (HasAvailableUpdate)
        {
            await RunUpdateAsync();
        }
    }

    private async Task RunUpdateAsync()
    {
        if (disposed || !HasAvailableUpdate || Interlocked.Exchange(ref updateStarted, 1) != 0) return;

        busy = true;
        NotifyState();
        try
        {
            UpdateStartStatus result;
            try
            {
                result = await coordinator.StartAvailableUpdateAsync(lifetime.Token);
            }
            catch (OperationCanceledException) when (lifetime.IsCancellationRequested)
            {
                return;
            }
            catch
            {
                result = UpdateStartStatus.LaunchFailed;
            }

            if (disposed) return;

            startStatus = result;
            if (result is UpdateStartStatus.Started or UpdateStartStatus.NoAvailableUpdate)
            {
                availableVersion = null;
            }
        }
        finally
        {
            busy = false;
            Interlocked.Exchange(ref updateStarted, 0);
            if (!disposed) NotifyState();
        }
    }

    private static string FormatVersion(string version) =>
        version.StartsWith("v", StringComparison.OrdinalIgnoreCase) ? version : $"v{version}";

    private void NotifyState()
    {
        OnPropertyChanged(nameof(AvailableVersion));
        OnPropertyChanged(nameof(HasAvailableUpdate));
        OnPropertyChanged(nameof(IsBusy));
        OnPropertyChanged(nameof(IsNotificationVisible));
        OnPropertyChanged(nameof(IsUpdateActionVisible));
        OnPropertyChanged(nameof(StartStatus));
        OnPropertyChanged(nameof(NotificationText));
        OnPropertyChanged(nameof(StatusText));
        OnPropertyChanged(nameof(ActionText));
        updateCommand.RaiseCanExecuteChanged();
    }

    private void OnPropertyChanged([CallerMemberName] string? propertyName = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));

    private sealed class UpdateAsyncCommand(UpdateViewModel owner) : ICommand
    {
        public event EventHandler? CanExecuteChanged;

        public bool CanExecute(object? parameter) => owner.IsUpdateActionVisible;

        public void Execute(object? parameter) => _ = owner.RunUpdateAsync();

        public void RaiseCanExecuteChanged() => CanExecuteChanged?.Invoke(this, EventArgs.Empty);
    }
}
