# Installs the latest stable Windows release and upgrades it to the exact
# release candidate. Local reproduction and CI share this implementation.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$SourceSha,

    [string]$CandidateSetup = '',

    [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')]
    [string]$Repository = 'salty919/codex_info_v2',

    [switch]$RetainSentinel
)

$ErrorActionPreference = 'Stop'

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if ([string]::IsNullOrWhiteSpace($CandidateSetup)) {
    $CandidateSetup = Join-Path $repositoryRoot 'artifacts/windows-installer/CodexInfo.WindowsClient.Setup.exe'
}
$candidate = (Resolve-Path -LiteralPath $CandidateSetup).Path
$versionProps = Join-Path $repositoryRoot 'windows-client/Directory.Build.props'
$install = Join-Path $env:LOCALAPPDATA 'Programs\Codex Info Monitor'
$exe = Join-Path $install 'CodexInfo.WindowsClient.exe'
$start = Join-Path ([Environment]::GetFolderPath('Programs')) 'Codex Info\Codex Info Monitor.lnk'
$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Codex Info Monitor.lnk'
$settings = Join-Path $env:LOCALAPPDATA 'CodexInfo'
$sentinel = Join-Path $settings 'installer-preserve-sentinel.txt'

$document = [xml](Get-Content -LiteralPath $versionProps -Raw)
$versionNodes = $document.SelectNodes("/*[local-name()='Project']/*[local-name()='PropertyGroup']/*[local-name()='Version']")
if ($versionNodes.Count -ne 1) {
    throw 'Directory.Build.props must contain exactly one Version element.'
}
$candidateVersionText = $versionNodes[0].InnerText.Trim()
if ($candidateVersionText -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw "Candidate version is not stable X.Y.Z: $candidateVersionText"
}
$candidateVersion = [version]$candidateVersionText
$expectedProductVersion = "$candidateVersionText+$SourceSha"
if ((Test-Path -LiteralPath $exe -PathType Leaf) -and
    ([string](Get-Item -LiteralPath $exe).VersionInfo.ProductVersion -ceq $expectedProductVersion)) {
    Write-Host "windows-installer-upgrade: PASS (already installed: $expectedProductVersion)"
    return
}

$headers = @{
    Accept = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
    'User-Agent' = 'CodexInfo-Release-Acceptance'
}
if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_TOKEN)) {
    $headers.Authorization = "Bearer $($env:GITHUB_TOKEN)"
}
$latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/releases/latest" -Headers $headers
$tagMatch = [regex]::Match([string]$latest.tag_name, '^windows-v([0-9]+\.[0-9]+\.[0-9]+)$')
if ($latest.draft -or $latest.prerelease -or -not $tagMatch.Success) {
    throw 'Latest published release is not a canonical stable Windows release.'
}
$previousVersionText = $tagMatch.Groups[1].Value
$previousVersion = [version]$previousVersionText
if ($previousVersion -ge $candidateVersion) {
    throw "Candidate version must be newer than the latest published version: $previousVersionText -> $candidateVersionText"
}

$setupAssets = @($latest.assets | Where-Object { $_.name -ceq 'CodexInfo.WindowsClient.Setup.exe' })
$manifestAssets = @($latest.assets | Where-Object { $_.name -ceq 'CodexInfo.WindowsClient.update.json' })
if ($setupAssets.Count -ne 1 -or $manifestAssets.Count -ne 1) {
    throw 'Latest stable release must contain exactly one Windows Setup and update manifest.'
}
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ("codex-info-windows-upgrade-" + [Guid]::NewGuid().ToString('N'))
$previousSetup = Join-Path $temporaryRoot 'CodexInfo.WindowsClient.previous.Setup.exe'
$previousManifest = Join-Path $temporaryRoot 'CodexInfo.WindowsClient.previous.update.json'
$sentinelExisted = Test-Path -LiteralPath $sentinel -PathType Leaf

try {
    New-Item -ItemType Directory -Path $temporaryRoot -Force | Out-Null
    Invoke-WebRequest -Uri $setupAssets[0].browser_download_url -OutFile $previousSetup
    Invoke-WebRequest -Uri $manifestAssets[0].browser_download_url -OutFile $previousManifest
    $manifest = Get-Content -LiteralPath $previousManifest -Raw -Encoding utf8 | ConvertFrom-Json
    if ([string]$manifest.version -cne $previousVersionText -or
        [string]$manifest.installer.name -cne 'CodexInfo.WindowsClient.Setup.exe' -or
        [string]$manifest.installer.url -cne [string]$setupAssets[0].browser_download_url -or
        [Int64]$manifest.installer.size -ne [Int64](Get-Item -LiteralPath $previousSetup).Length -or
        [string]$manifest.installer.sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw 'Latest stable Windows manifest does not identify the downloaded previous Setup.'
    }
    $previousHash = (Get-FileHash -LiteralPath $previousSetup -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($previousHash -cne [string]$manifest.installer.sha256) {
        throw 'Latest stable Windows Setup digest does not match its published manifest.'
    }

    $previousInstall = Start-Process -FilePath $previousSetup `
        -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/MERGETASKS=desktopicon') `
        -Wait -PassThru
    if ($previousInstall.ExitCode -ne 0) {
        throw "Previous stable installer failed with exit code $($previousInstall.ExitCode)."
    }
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw 'Previous stable client executable is missing after install.'
    }
    $installedPrevious = [string](Get-Item -LiteralPath $exe).VersionInfo.ProductVersion
    if ($installedPrevious -cnotmatch ('^' + [regex]::Escape($previousVersionText) + '(\+|$)')) {
        throw "Previous stable installed the wrong version: $installedPrevious"
    }

    New-Item -ItemType Directory -Path $settings -Force | Out-Null
    Set-Content -LiteralPath $sentinel -Value 'preserve'
    $candidateInstall = Start-Process -FilePath $candidate `
        -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') `
        -Wait -PassThru
    if ($candidateInstall.ExitCode -ne 0) {
        throw "Candidate installer failed during in-place upgrade with exit code $($candidateInstall.ExitCode)."
    }
    if (-not (Test-Path -LiteralPath $sentinel -PathType Leaf)) {
        throw 'Candidate upgrade removed user settings.'
    }
    $installedCandidate = [string](Get-Item -LiteralPath $exe).VersionInfo.ProductVersion
    if ($installedCandidate -cne $expectedProductVersion) {
        throw "Candidate upgrade did not install the exact source revision: expected=$expectedProductVersion actual=$installedCandidate"
    }

    $nativeNotice = Join-Path $install 'THIRD-PARTY-LICENSES\skiasharp.nativeassets.win32-3.119.4-THIRD-PARTY-NOTICES.txt'
    $uninstaller = Get-ChildItem -LiteralPath $install -Filter 'unins*.exe' -File | Select-Object -First 1
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexInfo.WindowsClient_is1'
    if (-not (Test-Path -LiteralPath $nativeNotice -PathType Leaf) -or $null -eq $uninstaller -or
        -not (Test-Path -LiteralPath $start -PathType Leaf) -or -not (Test-Path -LiteralPath $desktop -PathType Leaf) -or
        -not (Test-Path -LiteralPath $key)) {
        throw 'Candidate upgrade did not preserve the required installed product surface.'
    }
    if ((Get-ItemPropertyValue $key DisplayIcon) -ne $exe) {
        throw 'Candidate upgrade left an invalid Apps registration icon.'
    }
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($start)
    if ($shortcut.TargetPath -ne $exe -or $shortcut.WorkingDirectory -ne $install) {
        throw 'Candidate upgrade left an invalid Start-menu shortcut.'
    }

    Write-Host "windows-installer-upgrade: PASS ($previousVersionText -> $installedCandidate)"
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
    if (-not $RetainSentinel -and -not $sentinelExisted -and
        (Test-Path -LiteralPath $sentinel -PathType Leaf)) {
        Remove-Item -LiteralPath $sentinel -Force
    }
}
