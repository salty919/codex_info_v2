// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Infrastructure;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Settings;

namespace CodexInfo.WindowsClient.ViewModels;

public sealed class SettingsViewModel : INotifyPropertyChanged
{
    private readonly ClientSettingsStore store;
    private readonly MainWindowViewModel? main;
    private string selectedLanguageCode;
    private string selectedTimeZoneId;
    private bool saveFailed;

    public SettingsViewModel(ClientSettingsStore store, MainWindowViewModel? main = null)
    {
        this.store = store;
        this.main = main;
        selectedLanguageCode = LocalizationService.Current.LanguageCode;
        selectedTimeZoneId = App.CurrentSettings.TimeZoneId;
        LanguageOptions = new ReadOnlyCollection<UiText>(LocalizationService.Languages.ToList());
        LocalizationService.LanguageChanged += OnLanguageChanged;
        if (main is not null) main.PropertyChanged += OnMainPropertyChanged;
    }

    public event PropertyChangedEventHandler? PropertyChanged;
    public event EventHandler? Saved;

    public ReadOnlyCollection<UiText> LanguageOptions { get; }
    public UiText Texts => LocalizationService.Current;
    public string SelectedLanguageCode
    {
        get => selectedLanguageCode;
        set
        {
            if (string.Equals(selectedLanguageCode, value, StringComparison.OrdinalIgnoreCase)) return;
            selectedLanguageCode = value;
            Notify();
            Notify(nameof(SelectedLanguage));
        }
    }

    public UiText? SelectedLanguage => LanguageOptions.FirstOrDefault(option => option.LanguageCode == selectedLanguageCode);
    public IReadOnlyList<TimeZoneOption> TimeZoneOptions =>
    [
        new("local", Texts.LocalTimeZone),
        new("UTC", Texts.UtcTimeZone),
    ];
    public string SelectedTimeZoneId
    {
        get => selectedTimeZoneId;
        set
        {
            if (selectedTimeZoneId == value) return;
            selectedTimeZoneId = value;
            Notify();
            Notify(nameof(SelectedTimeZone));
        }
    }
    public string SelectedTimeZone => selectedTimeZoneId == "UTC" ? Texts.UtcTimeZone : Texts.LocalTimeZone;
    public string CurrentEndpoint => Texts.ConnectionEndpoint;
    public string StatusTitle => main?.StatusTitle ?? Texts.Unavailable;
    public string StatusDetail => saveFailed ? Texts.SettingsSaveFailed : main?.StatusDetail ?? Texts.UnavailableDetails;
    public bool SaveFailed => saveFailed;
    public bool CanAuthenticate => main?.IsAuthRequired == true;
    public void Refresh() => main?.RefreshCommand.Execute(null);
    public void StartAuthentication() => main?.AuthCommand.Execute(null);

    public bool Save()
    {
        var language = LocalizationService.NormalizeLanguageCode(selectedLanguageCode);
        var timeZone = string.Equals(selectedTimeZoneId, "UTC", StringComparison.OrdinalIgnoreCase)
            ? "UTC"
            : "local";
        var current = store.Load();
        var updated = current with
        {
            Language = language,
            TimeZoneId = timeZone,
            // SettingsCorrupt is an in-memory recovery marker only. A
            // successful durable rewrite closes that recovery generation.
            SettingsCorrupt = false,
        };
        try
        {
            store.Save(updated);
        }
        catch (Exception)
        {
            saveFailed = true;
            Notify(nameof(SaveFailed));
            Notify(nameof(StatusDetail));
            return false;
        }

        selectedTimeZoneId = timeZone;
        LocalizationService.SetLanguage(language);
        LocalizationService.SetTimeZone(timeZone);
        App.CurrentSettings = updated;
        saveFailed = false;
        Notify(nameof(SaveFailed));
        Notify(nameof(StatusDetail));
        Saved?.Invoke(this, EventArgs.Empty);
        return true;
    }

    public void Dispose()
    {
        LocalizationService.LanguageChanged -= OnLanguageChanged;
        if (main is not null) main.PropertyChanged -= OnMainPropertyChanged;
    }

    private void OnLanguageChanged(object? sender, EventArgs eventArgs)
    {
        Notify(nameof(Texts));
        Notify(nameof(CurrentEndpoint));
        Notify(nameof(SelectedTimeZone));
        Notify(nameof(TimeZoneOptions));
        Notify(nameof(StatusTitle));
        Notify(nameof(StatusDetail));
        Notify(nameof(CanAuthenticate));
        Notify(nameof(SaveFailed));
    }

    private void OnMainPropertyChanged(object? sender, PropertyChangedEventArgs eventArgs)
    {
        if (eventArgs.PropertyName is nameof(MainWindowViewModel.StatusTitle) or nameof(MainWindowViewModel.StatusDetail) or nameof(MainWindowViewModel.IsAuthRequired))
        {
            Notify(nameof(StatusTitle)); Notify(nameof(StatusDetail)); Notify(nameof(CanAuthenticate));
        }
    }

    private void Notify([CallerMemberName] string? propertyName = null) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
}

public sealed record TimeZoneOption(string Id, string Label);

public sealed class SetupViewModel : INotifyPropertyChanged, IDisposable
{
    private readonly MainWindowViewModel main;
    private readonly IClientSettingsSession settingsSession;
    private readonly ISetupConnectionEnvironment connectionEnvironment;
    private IConnectionChildProcess? sshProcess;
    private string sshUser = string.Empty;
    private string sshHost = string.Empty;
    private string? selectedSshConfigAlias;
    private bool sshLaunchFailed;
    private bool settingsSaveFailed;
    private string selectedConnectionProfile;
    private string selectedConnectionSelector;
    private int step;

    public SetupViewModel(MainWindowViewModel main)
        : this(main, App.SettingsSession, new WindowsSetupConnectionEnvironment())
    {
    }

    public SetupViewModel(MainWindowViewModel main, IClientSettingsSession settingsSession)
        : this(main, settingsSession, new WindowsSetupConnectionEnvironment())
    {
    }

    internal SetupViewModel(
        MainWindowViewModel main,
        IClientSettingsSession settingsSession,
        ISetupConnectionEnvironment connectionEnvironment)
    {
        ArgumentNullException.ThrowIfNull(main);
        ArgumentNullException.ThrowIfNull(settingsSession);
        ArgumentNullException.ThrowIfNull(connectionEnvironment);
        this.main = main;
        this.settingsSession = settingsSession;
        this.connectionEnvironment = connectionEnvironment;
        main.PropertyChanged += OnMainPropertyChanged;
        LocalizationService.LanguageChanged += OnLanguageChanged;
        SshConfigAliases = connectionEnvironment.LoadSshConfigAliases();
        WslDistributions = connectionEnvironment.LoadWslDistributions();
        var saved = settingsSession.Current;
        selectedConnectionProfile = saved.ConnectionProfile is ConnectionProfiles.Wsl or ConnectionProfiles.SshConfigAlias
            ? saved.ConnectionProfile
            : ConnectionProfiles.None;
        selectedConnectionSelector = selectedConnectionProfile == ConnectionProfiles.None
            ? ConnectionSelectors.None
            : saved.ConnectionSelector;
    }

    public event PropertyChangedEventHandler? PropertyChanged;
    public UiText Texts => LocalizationService.Current;
    public IReadOnlyList<ConnectionProfileOption> ConnectionProfileOptions =>
    [
        new(ConnectionProfiles.None, Texts.ConnectionProfileNone),
        new(ConnectionProfiles.Wsl, Texts.ConnectionProfileWsl),
        new(ConnectionProfiles.SshConfigAlias, Texts.ConnectionProfileSsh),
    ];
    public string SelectedConnectionProfile
    {
        get => selectedConnectionProfile;
        set
        {
            var next = value is ConnectionProfiles.Wsl or ConnectionProfiles.SshConfigAlias
                ? value
                : ConnectionProfiles.None;
            if (selectedConnectionProfile == next) return;
            selectedConnectionProfile = next;
            selectedConnectionSelector = next switch
            {
                ConnectionProfiles.Wsl when WslDistributions.Count > 0 => WslDistributions[0],
                ConnectionProfiles.SshConfigAlias when SshConfigAliases.Count > 0 => SshConfigAliases[0],
                _ => ConnectionSelectors.None,
            };
            if (next == ConnectionProfiles.SshConfigAlias
                && selectedConnectionSelector != ConnectionSelectors.None)
            {
                SshHost = selectedConnectionSelector;
            }
            Notify();
            Notify(nameof(SelectedConnectionSelector));
            Notify(nameof(ConnectionSelectorOptions));
            Notify(nameof(CanContinue));
            Notify(nameof(SshCommand));
        }
    }
    public string SelectedConnectionSelector
    {
        get => selectedConnectionSelector;
        set
        {
            var next = value ?? ConnectionSelectors.None;
            if (selectedConnectionSelector == next) return;
            selectedConnectionSelector = next;
            if (selectedConnectionProfile == ConnectionProfiles.SshConfigAlias) SshHost = next;
            Notify();
            Notify(nameof(CanContinue));
            Notify(nameof(SshCommand));
        }
    }
    public IReadOnlyList<string> ConnectionSelectorOptions => selectedConnectionProfile switch
    {
        ConnectionProfiles.Wsl => WslDistributions,
        ConnectionProfiles.SshConfigAlias => SshConfigAliases,
        _ => [ConnectionSelectors.None],
    };
    public IReadOnlyList<string> WslDistributions { get; }
    public int Step { get => step; private set { step = value; Notify(); Notify(nameof(IsConnectionStep)); Notify(nameof(IsAuthStep)); Notify(nameof(IsDoneStep)); } }
    public bool IsConnectionStep => Step == 0;
    public bool IsAuthStep => Step == 1;
    public bool IsDoneStep => Step == 2;
    public IReadOnlyList<string> SshConfigAliases { get; }
    public string? SelectedSshConfigAlias
    {
        get => selectedSshConfigAlias;
        set
        {
            if (selectedSshConfigAlias == value) return;
            selectedSshConfigAlias = value;
            if (!string.IsNullOrWhiteSpace(value)) SshHost = value;
            Notify();
        }
    }
    public string SshUser
    {
        get => sshUser;
        set
        {
            if (sshUser == value) return;
            sshUser = value;
            sshLaunchFailed = false;
            Notify();
            Notify(nameof(SshCommand));
            Notify(nameof(CanStartSsh));
            Notify(nameof(SshStatusText));
        }
    }

    public string SshHost
    {
        get => sshHost;
        set
        {
            if (sshHost == value) return;
            sshHost = value;
            sshLaunchFailed = false;
            Notify();
            Notify(nameof(SshCommand));
            Notify(nameof(CanStartSsh));
            Notify(nameof(SshStatusText));
        }
    }
    public bool SshRunning => sshProcess is { HasExited: false };
    public bool CanStartSsh => IsSafeSshHost(SshHost) && (string.IsNullOrWhiteSpace(SshUser) || IsSafeSshUser(SshUser));
    public string SshActionText => SshRunning ? Texts.SshStop : Texts.SshStart;
    public string SshStatusText => sshLaunchFailed
        ? Texts.SshLaunchFailedStatus
        : SshRunning
            ? Texts.SshRunningStatus
            : CanStartSsh
                ? Texts.SshReadyStatus
                : Texts.SshNotReady;
    public bool SettingsSaveFailed => settingsSaveFailed;
    public bool CanContinue => Step switch
    {
        0 => IsConnectionSelectionValid && (main.HasQuota || main.IsAuthRequired || main.HasDetails),
        // Starting `wsl.exe codex login` is not authentication completion.
        // The user must explicitly re-check and receive an authenticated
        // status snapshot before the setup flow can be marked complete.
        1 => main.IsAuthenticated,
        _ => true,
    };
    public string SshCommand => CanStartSsh
        ? $"ssh -N -L 8787:127.0.0.1:8787 {(string.IsNullOrWhiteSpace(SshUser) ? (SelectedConnectionSelector == ConnectionSelectors.None ? SshHost : SelectedConnectionSelector) : $"{SshUser}@{(SelectedConnectionSelector == ConnectionSelectors.None ? SshHost : SelectedConnectionSelector)}")}"
        : Texts.SshCommand;
    public string ApiCommand => Texts.ApiCommand;
    public string StatusTitle => main.StatusTitle;
    public string StatusDetail => settingsSaveFailed ? Texts.SettingsSaveFailed : main.StatusDetail;
    public string ContinueText => Step == 2 ? Texts.Close : Texts.Continue;

    public void Refresh() => main.RefreshCommand.Execute(null);
    public void StartAuthentication() => main.AuthCommand.Execute(null);
    public void StartOrStopSsh()
    {
        if (SshRunning)
        {
            try { sshProcess?.Kill(); } catch { /* process may have exited */ }
            return;
        }

        if (!CanStartSsh) return;
        try
        {
            sshLaunchFailed = false;
            var target = string.IsNullOrWhiteSpace(SshUser) ? SshHost : $"{SshUser}@{SshHost}";
            var process = connectionEnvironment.CreateSshProcess(target);
            process.Exited += OnSshExited;
            if (process.Start())
            {
                sshProcess = process;
                Notify(nameof(SshRunning)); Notify(nameof(SshActionText));
                Refresh();
            }
            else
            {
                process.Dispose();
                sshLaunchFailed = true;
                Notify(nameof(SshStatusText));
            }
        }
        catch
        {
            sshProcess = null;
            sshLaunchFailed = true;
            Notify(nameof(SshRunning)); Notify(nameof(SshActionText));
            Notify(nameof(SshStatusText));
        }
    }

    public bool IsConnectionSelectionValid => selectedConnectionProfile switch
    {
        ConnectionProfiles.Wsl => ConnectionSelectors.IsWslToken(selectedConnectionSelector)
            && WslDistributions.Contains(selectedConnectionSelector, StringComparer.Ordinal),
        ConnectionProfiles.SshConfigAlias => ConnectionSelectors.IsSshAlias(selectedConnectionSelector)
            && SshConfigAliases.Contains(selectedConnectionSelector, StringComparer.OrdinalIgnoreCase),
        // `none` is a supported profile for an already-running local
        // listener (for example a manually managed WSL service). It becomes
        // configured only after the health/status path has been observed.
        ConnectionProfiles.None => string.IsNullOrWhiteSpace(SshHost)
            && string.IsNullOrWhiteSpace(SshUser)
            && (main.HasQuota || main.IsAuthRequired || main.HasDetails),
        _ => false,
    };

    public ClientSettings BuildSettings(ClientSettings current) => current with
    {
        ConnectionConfigured = IsConnectionSelectionValid,
        ConnectionProfile = IsConnectionSelectionValid ? selectedConnectionProfile : ConnectionProfiles.None,
        ConnectionSelector = IsConnectionSelectionValid ? selectedConnectionSelector : ConnectionSelectors.None,
    };

    public SetupAdvanceOutcome Advance()
    {
        if (IsConnectionStep && !CanContinue)
        {
            return SetupAdvanceOutcome.StayOpen;
        }

        if (IsAuthStep && !CanContinue)
        {
            StartAuthentication();
            return SetupAdvanceOutcome.StayOpen;
        }

        if (IsDoneStep)
        {
            if (!PersistSettings(setupCompleted: true))
            {
                return SetupAdvanceOutcome.StayOpen;
            }

            return SetupAdvanceOutcome.CloseRequested;
        }

        if (IsConnectionStep)
        {
            if (!PersistSettings(setupCompleted: false))
            {
                return SetupAdvanceOutcome.StayOpen;
            }
        }

        Continue();
        return SetupAdvanceOutcome.StayOpen;
    }

    public void Continue()
    {
        if (!CanContinue)
        {
            if (IsAuthStep)
            {
                StartAuthentication();
            }
            return;
        }

        Step = Math.Min(2, Step + 1);
    }

    private bool PersistSettings(bool setupCompleted)
    {
        var current = settingsSession.Current;
        var updated = BuildSettings(current) with
        {
            SetupCompleted = setupCompleted || current.SetupCompleted,
            Language = Texts.LanguageCode,
            // A successful setup save is the explicit recovery boundary for
            // a corrupt or partially-written prior settings generation.
            SettingsCorrupt = false,
        };
        try
        {
            settingsSession.Save(updated);
        }
        catch (Exception)
        {
            settingsSaveFailed = true;
            Notify(nameof(SettingsSaveFailed));
            Notify(nameof(StatusDetail));
            return false;
        }

        settingsSaveFailed = false;
        Notify(nameof(SettingsSaveFailed));
        Notify(nameof(StatusDetail));
        if (!setupCompleted)
        {
            // The main window starts before Setup has a selector.  Apply the
            // newly saved profile now so WSL/SSH is actually started during
            // first-run setup instead of waiting for a process restart.
            main.ApplyConnectionSettings(updated);
        }
        return true;
    }

    public void Dispose()
    {
        main.PropertyChanged -= OnMainPropertyChanged;
        LocalizationService.LanguageChanged -= OnLanguageChanged;
        sshProcess?.Dispose();
    }

    private void OnMainPropertyChanged(object? sender, PropertyChangedEventArgs eventArgs)
    {
        if (eventArgs.PropertyName is nameof(MainWindowViewModel.StatusTitle)
            or nameof(MainWindowViewModel.StatusDetail)
            or nameof(MainWindowViewModel.IsAuthRequired)
            or nameof(MainWindowViewModel.IsAuthenticated)
            or nameof(MainWindowViewModel.HasQuota)
            or nameof(MainWindowViewModel.HasDetails))
        {
            Notify(nameof(StatusTitle));
            Notify(nameof(StatusDetail));
            Notify(nameof(CanContinue));
            Notify(nameof(SettingsSaveFailed));
        }
    }

    private void OnLanguageChanged(object? sender, EventArgs eventArgs)
    {
        Notify(nameof(Texts));
        Notify(nameof(ConnectionProfileOptions));
        Notify(nameof(ConnectionSelectorOptions));
        Notify(nameof(SshCommand));
        Notify(nameof(SshStatusText));
        Notify(nameof(ApiCommand));
        Notify(nameof(SshActionText));
        Notify(nameof(ContinueText));
        Notify(nameof(StatusTitle));
        Notify(nameof(StatusDetail));
        Notify(nameof(CanContinue));
        Notify(nameof(SettingsSaveFailed));
    }

    private void OnSshExited(object? sender, EventArgs eventArgs)
    {
        sshProcess?.Dispose();
        sshProcess = null;
        Notify(nameof(SshRunning));
        Notify(nameof(SshActionText));
        Notify(nameof(SshStatusText));
    }

    private static bool IsSafeSshHost(string value)
    {
        if (string.IsNullOrWhiteSpace(value) || value.Length > 255) return false;
        return value.All(character => char.IsLetterOrDigit(character) || character is '.' or '-' or '_' or ':');
    }

    private static bool IsSafeSshUser(string value) =>
        value.Length <= 128 && value.All(character => char.IsLetterOrDigit(character) || character is '.' or '-' or '_');

    private void Notify([CallerMemberName] string? propertyName = null) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
}

public sealed record ConnectionProfileOption(string Id, string Label);
