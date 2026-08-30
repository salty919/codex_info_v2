// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Core;

/// <summary>
/// Exposes the Windows client assembly version generated from Directory.Build.props.
/// Keeping this in Core gives every Windows surface one display owner rather
/// than allowing individual windows to hard-code a release string.
/// </summary>
public static class ProductInfo
{
    public static string DisplayVersion
    {
        get
        {
            var version = typeof(ProductInfo).Assembly.GetName().Version;
            return version is { Major: >= 0, Minor: >= 0, Build: >= 0 }
                ? $"v{version.Major}.{version.Minor}.{version.Build}"
                : "vunknown";
        }
    }
}
