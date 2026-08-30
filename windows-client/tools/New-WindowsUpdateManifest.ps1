# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only

<#
.SYNOPSIS
    Creates the deterministic Windows client update manifest for a built installer.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [string]$Version,
    [string]$OutputPath = 'artifacts/windows-installer/CodexInfo.WindowsClient.update.json',
    [Parameter(Mandatory = $true)][string]$Repository
)

$ErrorActionPreference = 'Stop'

function Get-AuthoritativeVersion {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Version properties file was not found: $Path"
    }

    try {
        $document = [xml](Get-Content -LiteralPath $Path -Raw)
    }
    catch {
        throw "Version properties file is not valid XML: $Path"
    }
    if ($null -eq $document) {
        throw "Version properties file is not valid XML: $Path"
    }

    $versionNodes = $document.SelectNodes("/*[local-name()='Project']/*[local-name()='PropertyGroup']/*[local-name()='Version']")
    if ($versionNodes.Count -ne 1) {
        throw "Version properties file must contain exactly one Project/PropertyGroup/Version element: $Path"
    }

    $authoritativeVersion = $versionNodes[0].InnerText.Trim()
    if ($authoritativeVersion -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
        throw "Version must be a stable X.Y.Z value: $authoritativeVersion"
    }

    return $authoritativeVersion
}

$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$versionProps = Join-Path $root 'windows-client/Directory.Build.props'
$authoritativeVersion = Get-AuthoritativeVersion -Path $versionProps

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = $authoritativeVersion
}
elseif ($Version -ne $authoritativeVersion) {
    throw "Requested manifest version $Version does not match the authoritative version $authoritativeVersion."
}

if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw "Version must be a stable X.Y.Z value: $Version"
}
if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
    throw "Installer was not found: $InstallerPath"
}
$installer = Get-Item -LiteralPath $InstallerPath
if ($installer.Name -ne 'CodexInfo.WindowsClient.Setup.exe') {
    throw "Installer must be named CodexInfo.WindowsClient.Setup.exe: $($installer.Name)"
}
if ($installer.Length -le 0) {
    throw "Installer must be non-empty: $InstallerPath"
}

$sha256 = (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
if ($sha256 -notmatch '^[0-9a-f]{64}$') {
    throw "Installer SHA-256 hash is not a lowercase 64-character hexadecimal value."
}

$manifestPath = if ([IO.Path]::IsPathRooted($OutputPath)) {
    [IO.Path]::GetFullPath($OutputPath)
}
else {
    [IO.Path]::GetFullPath((Join-Path (Get-Location).Path $OutputPath))
}
$manifestDirectory = Split-Path -Parent $manifestPath
if (-not [string]::IsNullOrWhiteSpace($manifestDirectory)) {
    New-Item -ItemType Directory -Path $manifestDirectory -Force | Out-Null
}

$manifest = [ordered]@{
    schema_version = 1
    version = $Version
    installer = [ordered]@{
        name = 'CodexInfo.WindowsClient.Setup.exe'
        url = "https://github.com/$Repository/releases/download/windows-v$Version/CodexInfo.WindowsClient.Setup.exe"
        sha256 = $sha256
        size = [int64]$installer.Length
    }
}
$json = $manifest | ConvertTo-Json -Compress -Depth 3
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText($manifestPath, $json + "`n", $utf8NoBom)

Write-Host "Created $manifestPath"
