# Reproduces the installed Windows UI Automation step used by the main-PR
# release-candidate workflow. Local preparation installs the exact candidate
# directly; published-release upgrade validation remains a separate CI step.
# The UI assertions and physical window-move smoke are identical for local and
# CI runs; installation cleanup is CI opt-in.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$SourceSha,

    [string]$ClientPath = '',
    [string]$OutputDirectory = '',
    [string]$CandidateOutputDirectory = 'artifacts/windows-installer',
    [switch]$PrepareCandidate,
    [switch]$CleanupInstallation
)

$ErrorActionPreference = 'Stop'

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$install = Join-Path $env:LOCALAPPDATA 'Programs\Codex Info Monitor'
if ([string]::IsNullOrWhiteSpace($ClientPath)) {
    $ClientPath = Join-Path $install 'CodexInfo.WindowsClient.exe'
}
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path ([IO.Path]::GetTempPath()) 'codex-info-windows-e2e'
}

$runner = Join-Path $PSScriptRoot 'Run-WindowsClientE2E.ps1'
$moveSmoke = Join-Path $repositoryRoot 'scripts/windows_window_move_smoke.ps1'
$ensureDotNetSdk = Join-Path $PSScriptRoot 'Ensure-DotNetSdk.ps1'
$ensureCompiler = Join-Path $PSScriptRoot 'Ensure-InnoSetupCompiler.ps1'
$buildInstaller = Join-Path $PSScriptRoot 'Build-WindowsInstaller.ps1'
if (-not (Test-Path -LiteralPath $runner -PathType Leaf)) {
    throw "Windows UI E2E runner is missing: $runner"
}
if (-not (Test-Path -LiteralPath $moveSmoke -PathType Leaf)) {
    throw "Physical window-move smoke is missing: $moveSmoke"
}
$versionPropsPath = Join-Path $repositoryRoot 'windows-client/Directory.Build.props'
$versionDocument = [xml](Get-Content -LiteralPath $versionPropsPath -Raw)
$versionNodes = $versionDocument.SelectNodes(
    "/*[local-name()='Project']/*[local-name()='PropertyGroup']/*[local-name()='Version']")
if ($versionNodes.Count -ne 1) {
    throw 'Directory.Build.props must contain exactly one Version element.'
}
$expectedProductVersion = "$($versionNodes[0].InnerText.Trim())+$SourceSha"
$installedMatches = (Test-Path -LiteralPath $ClientPath -PathType Leaf) -and
    ((Get-Item -LiteralPath $ClientPath).VersionInfo.ProductVersion -ceq $expectedProductVersion)
if ($PrepareCandidate -and -not $installedMatches) {
    foreach ($requiredScript in ($ensureDotNetSdk, $ensureCompiler, $buildInstaller)) {
        if (-not (Test-Path -LiteralPath $requiredScript -PathType Leaf)) {
            throw "Windows candidate preparation script is missing: $requiredScript"
        }
    }
    if ([IO.Path]::IsPathRooted($CandidateOutputDirectory)) {
        throw 'CandidateOutputDirectory must be relative to the repository root.'
    }
    & $ensureDotNetSdk
    & $ensureCompiler
    & $buildInstaller -OutputDirectory $CandidateOutputDirectory -SourceSha $SourceSha
    $candidateSetup = Join-Path $repositoryRoot `
        (Join-Path $CandidateOutputDirectory 'CodexInfo.WindowsClient.Setup.exe')
    $candidateInstall = Start-Process -FilePath $candidateSetup `
        -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') `
        -Wait -PassThru
    if ($candidateInstall.ExitCode -ne 0) {
        throw "Candidate installer failed with exit code $($candidateInstall.ExitCode)."
    }
}
if (-not (Test-Path -LiteralPath $ClientPath -PathType Leaf)) {
    throw "Installed Windows client is missing: $ClientPath"
}
$actualProductVersion = (Get-Item -LiteralPath $ClientPath).VersionInfo.ProductVersion
if ($actualProductVersion -cne $expectedProductVersion) {
    throw "Installed client identity mismatch: expected $expectedProductVersion, found $actualProductVersion"
}

try {
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    $moveSmokeLog = Join-Path $OutputDirectory 'window-move-smoke.log'
    & $runner -ClientPath $ClientPath -OutputDirectory $OutputDirectory -Fixture -SourceSha $SourceSha
    "source-sha: $SourceSha" | Set-Content -LiteralPath $moveSmokeLog -Encoding utf8
    $moveSmokeOutput = @(& $moveSmoke -ClientPath $ClientPath -AllowPhysicalInput *>&1)
    $moveSmokeOutput | Tee-Object -FilePath $moveSmokeLog -Append
    if ($moveSmokeOutput.Count -eq 0 -or [string]$moveSmokeOutput[-1] -ne 'window-move-smoke: PASS') {
        throw 'Physical window move smoke did not report its terminal PASS contract.'
    }
}
finally {
    if ($CleanupInstallation) {
        $uninstaller = Get-ChildItem -LiteralPath $install -Filter 'unins*.exe' -File -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($null -ne $uninstaller) {
            $uninstallProcess = Start-Process -FilePath $uninstaller.FullName `
                -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') -Wait -PassThru
            if ($uninstallProcess.ExitCode -ne 0) {
                throw "E2E cleanup uninstall failed with exit code $($uninstallProcess.ExitCode)"
            }
        }
        $sentinel = Join-Path $env:LOCALAPPDATA 'CodexInfo\installer-preserve-sentinel.txt'
        $start = Join-Path ([Environment]::GetFolderPath('Programs')) 'Codex Info\Codex Info Monitor.lnk'
        $desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Codex Info Monitor.lnk'
        $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexInfo.WindowsClient_is1'
        Start-Sleep -Seconds 4
        if (Test-Path -LiteralPath $install) { throw 'Install directory still exists after E2E uninstall.' }
        if (Test-Path -LiteralPath $start) { throw 'Start-menu shortcut still exists after E2E uninstall.' }
        if (Test-Path -LiteralPath $desktop) { throw 'Desktop shortcut still exists after E2E uninstall.' }
        if (Test-Path -LiteralPath $key) { throw 'Apps registration still exists after E2E uninstall.' }
        if (-not (Test-Path -LiteralPath $sentinel -PathType Leaf)) {
            throw 'E2E uninstall removed user settings.'
        }
        Remove-Item -LiteralPath $sentinel -Force
    }
}
