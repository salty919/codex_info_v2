// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Reflection;

namespace CodexInfo.WindowsClient.Core;

/// <summary>
/// Exposes the Windows client assembly version generated from Directory.Build.props.
/// Keeping this in Core gives every Windows surface one display owner rather
/// than allowing individual windows to hard-code a release string.
/// </summary>
public static class ProductInfo
{
    /// <summary>The canonical stable X.Y.Z version used by service contracts.</summary>
    public static string Version
    {
        get
        {
            var information = typeof(ProductInfo).Assembly
                .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;
            var value = information?.Split('+')[0];
            return ProductVersion.TryParse(value, out var version) ? version!.ToString() : "unknown";
        }
    }

    public static string DisplayVersion
    {
        get => Version == "unknown" ? "vunknown" : $"v{Version}";
    }
}
