// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using System.Globalization;
using CodexInfo.WindowsClient.Infrastructure;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Theme;

namespace CodexInfo.WindowsClient.Settings;

public sealed record ClientSettings(string Language, bool SetupCompleted)
{
    /// <summary>Connection setup was confirmed at least once.</summary>
    /// <remarks>SSH host/user are intentionally not persisted; this flag only
    /// prevents an intrusive first-run dialog from reopening on every launch.
    /// </remarks>
    public bool ConnectionConfigured { get; init; }
    /// <summary>True when the persisted settings could not be parsed.</summary>
    /// <remarks>This is not persisted and never implies a connection. It lets
    /// startup avoid reopening the first-run wizard on every launch while the
    /// main screen exposes the disconnected state and Settings remains the
    /// recovery path.</remarks>
    [JsonIgnore]
    public bool SettingsCorrupt { get; init; }
    public string TimeZoneId { get; init; } = "local";
    /// <summary>The only durable connection mode. Never persist expanded SSH values.</summary>
    public string ConnectionProfile { get; init; } = ConnectionProfiles.None;
    /// <summary>A WSL distribution token or literal OpenSSH Host alias.</summary>
    public string ConnectionSelector { get; init; } = ConnectionSelectors.None;
    /// <summary>Windows presentation palette; old six-key files use the original dark palette.</summary>
    public string ThemeId { get; init; } = ThemePalette.ClassicDark;
    public static ClientSettings Default { get; } = new("ja", false);
}

public static class ConnectionProfiles
{
    public const string None = "none";
    public const string Wsl = "wsl";
    public const string SshConfigAlias = "sshConfigAlias";
}

public static class ConnectionSelectors
{
    public const string None = "none";

    // This is intentionally the OpenSSH Host-label grammar from the product
    // contract. We do not expand HostName/User/Port/IdentityFile here.
    private static readonly Regex SshAliasPattern = new(
        "^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$",
        RegexOptions.CultureInvariant | RegexOptions.Compiled);

    public static bool IsValid(ClientSettings settings)
    {
        if (!LocalizationService.Languages.Any(language =>
                string.Equals(language.LanguageCode, settings.Language, StringComparison.OrdinalIgnoreCase)))
        {
            return false;
        }

        if (settings.TimeZoneId is not ("local" or "UTC"))
        {
            return false;
        }

        return settings.ConnectionProfile switch
        {
            ConnectionProfiles.None => settings.ConnectionSelector == None,
            ConnectionProfiles.Wsl => IsWslToken(settings.ConnectionSelector),
            ConnectionProfiles.SshConfigAlias => SshAliasPattern.IsMatch(settings.ConnectionSelector),
            _ => false,
        };
    }

    public static bool IsSshAlias(string? selector) =>
        selector is not null && SshAliasPattern.IsMatch(selector);

    public static bool IsWslToken(string? selector)
    {
        if (string.IsNullOrWhiteSpace(selector) || selector == None || selector.Length > 255)
        {
            return false;
        }

        // A distribution name is passed as one ArgumentList token. Reject
        // whitespace/control characters rather than trying to parse a shell
        // command or accepting an expanded path.
        return selector.All(character => !char.IsControl(character) && !char.IsWhiteSpace(character));
    }
}

public sealed class ClientSettingsStore
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web)
    {
        WriteIndented = true,
    };

    private readonly string path;

    public ClientSettingsStore()
        : this(ResolveDefaultPath())
    {
    }

    private static string ResolveDefaultPath()
    {
        var fixturePort = Environment.GetEnvironmentVariable("CODEX_INFO_WINDOWS_E2E_FIXTURE_PORT");
        if (string.IsNullOrWhiteSpace(fixturePort))
        {
            return Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "CodexInfo",
                "settings.json");
        }

        if (!int.TryParse(fixturePort, NumberStyles.None, CultureInfo.InvariantCulture, out var port) ||
            port is < 1 or > 65535)
        {
            throw new InvalidOperationException("The Windows E2E fixture port is invalid.");
        }

        var fixtureSettingsPath = Environment.GetEnvironmentVariable("CODEX_INFO_WINDOWS_E2E_SETTINGS_PATH");
        if (string.IsNullOrWhiteSpace(fixtureSettingsPath) || !Path.IsPathFullyQualified(fixtureSettingsPath))
        {
            throw new InvalidOperationException("The Windows E2E fixture settings path must be absolute.");
        }

        return Path.GetFullPath(fixtureSettingsPath);
    }

    /// <summary>Creates a store at an explicit path for isolated tests.</summary>
    public ClientSettingsStore(string path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            throw new ArgumentException("A settings path is required.", nameof(path));
        }

        this.path = Path.GetFullPath(path);
    }

    public ClientSettings Load()
    {
        try
        {
            if (WindowsPathSafety.ContainsReparsePoint(path))
            {
                return ClientSettings.Default with { SettingsCorrupt = true };
            }

            if (!File.Exists(path))
            {
                return ClientSettings.Default;
            }

            var json = File.ReadAllText(path);
            if (!HasExactSettingsShape(json))
            {
                return ClientSettings.Default with { SettingsCorrupt = true };
            }

            var settings = JsonSerializer.Deserialize<ClientSettings>(json, JsonOptions);
            if (settings is not { Language.Length: > 0 })
            {
                return ClientSettings.Default with { SettingsCorrupt = true };
            }

            // Settings are presentation-only state. Normalize values loaded
            // from an older/corrupt install before they reach the UI so an
            // unsupported locale or timezone can never leave an empty selector
            // or alter the fixed API contract.
            var normalized = settings with
            {
                Language = LocalizationService.NormalizeLanguageCode(settings.Language),
                TimeZoneId = string.Equals(settings.TimeZoneId, "UTC", StringComparison.OrdinalIgnoreCase)
                    ? "UTC"
                    : "local",
            };
            return ConnectionSelectors.IsValid(normalized) && ThemePalette.IsValid(normalized.ThemeId)
                ? normalized
                : ClientSettings.Default with { SettingsCorrupt = true };
        }
        catch
        {
            // A corrupt file must not erase a previously configured state in
            // memory and must not cause the welcome wizard to loop forever.
            // Keep the safe disconnected defaults and mark the recovery state
            // for startup; Settings can overwrite it atomically.
            return ClientSettings.Default with { SettingsCorrupt = true };
        }
    }

    public void Save(ClientSettings settings)
    {
        var normalized = settings with
        {
            Language = LocalizationService.NormalizeLanguageCode(settings.Language),
            TimeZoneId = string.Equals(settings.TimeZoneId, "UTC", StringComparison.OrdinalIgnoreCase)
                ? "UTC"
                : "local",
        };
        if (!ConnectionSelectors.IsValid(normalized) || !ThemePalette.IsValid(normalized.ThemeId))
        {
            throw new ArgumentException("Settings contain an invalid connection or theme selection.", nameof(settings));
        }

        var directory = Path.GetDirectoryName(path)!;
        if (!WindowsPathSafety.EnsureDirectoryTreeWithoutReparse(directory) ||
            !WindowsPathSafety.IsMissingOrRegularFile(path))
        {
            throw new IOException("The settings path must contain no reparse points and target a regular file.");
        }

        var temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try
        {
            using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            using (var writer = new StreamWriter(stream))
            {
                writer.Write(JsonSerializer.Serialize(normalized, JsonOptions));
                writer.Flush();
                stream.Flush(true);
            }

            if (!WindowsPathSafety.EnsureDirectoryTreeWithoutReparse(directory) ||
                !WindowsPathSafety.IsMissingOrRegularFile(path))
            {
                throw new IOException("The settings path changed during save.");
            }

            File.Move(temporary, path, true);
        }
        finally
        {
            try
            {
                if (File.Exists(temporary))
                {
                    File.Delete(temporary);
                }
            }
            catch
            {
                // A failed cleanup leaves an unreferenced random-name file;
                // the durable settings target is never replaced by it.
            }
        }
    }

    private static bool HasExactSettingsShape(string json)
    {
        try
        {
            using var document = JsonDocument.Parse(json);
            if (document.RootElement.ValueKind != JsonValueKind.Object)
            {
                return false;
            }

            var expected = new HashSet<string>(StringComparer.Ordinal)
            {
                "language",
                "setupCompleted",
                "connectionConfigured",
                "timeZoneId",
                "connectionProfile",
                "connectionSelector",
            };
            var seen = new HashSet<string>(StringComparer.Ordinal);
            foreach (var property in document.RootElement.EnumerateObject())
            {
                if (!seen.Add(property.Name) ||
                    (!expected.Contains(property.Name) && property.Name != "themeId"))
                {
                    return false;
                }
            }

            // An existing six-key file remains valid. Save always emits the
            // seven-key form because ThemeId has a classic-dark default.
            if (seen.SetEquals(expected)) return true;
            expected.Add("themeId");
            return seen.SetEquals(expected);
        }
        catch (JsonException)
        {
            return false;
        }
    }
}
