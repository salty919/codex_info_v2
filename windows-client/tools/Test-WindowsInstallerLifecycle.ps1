# Verifies the installed Windows setup, update, UI, and uninstall lifecycle once.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [Parameter(Mandatory = $true)][string]$SourceSha
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$setup = (Resolve-Path $InstallerPath).Path

function Invoke-BoundedSetupProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Operation
    )
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru
    if (-not $process.WaitForExit(300000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "$Operation timed out."
    }
    if ($process.ExitCode -ne 0) {
        throw "$Operation failed with exit code $($process.ExitCode)"
    }
}

Invoke-BoundedSetupProcess -Path $setup -Operation 'Installer' -Arguments @(
    '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/MERGETASKS=desktopicon'
)

$install = Join-Path $env:LOCALAPPDATA 'Programs\Codex Info Monitor'
$start = Join-Path ([Environment]::GetFolderPath('Programs')) 'Codex Info\Codex Info Monitor.lnk'
$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Codex Info Monitor.lnk'
$exe = Join-Path $install 'CodexInfo.WindowsClient.exe'
$nativeNotice = Join-Path $install 'THIRD-PARTY-LICENSES\skiasharp.nativeassets.win32-3.119.4-THIRD-PARTY-NOTICES.txt'
$uninstaller = Get-ChildItem -LiteralPath $install -Filter 'unins*.exe' -File | Select-Object -First 1
if (-not (Test-Path $exe -PathType Leaf)) { throw 'Installed client executable is missing.' }
if (-not (Test-Path $nativeNotice -PathType Leaf)) { throw 'Required SkiaSharp native notice is missing from the installed payload.' }
if ($null -eq $uninstaller) { throw 'Installed uninstaller is missing.' }
if (-not (Test-Path $start -PathType Leaf)) { throw 'Start-menu shortcut is missing.' }
if (-not (Test-Path $desktop -PathType Leaf)) { throw 'Desktop shortcut is missing.' }

$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexInfo.WindowsClient_is1'
if (-not (Test-Path $key)) { throw 'Per-user Apps uninstall registration is missing.' }
if ((Get-ItemPropertyValue $key DisplayIcon) -ne $exe) { throw 'Apps entry icon does not target the product executable.' }
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($start)
if ($shortcut.TargetPath -ne $exe) { throw 'Start-menu shortcut target is incorrect.' }
if ($shortcut.WorkingDirectory -ne $install) { throw 'Start-menu shortcut working directory is incorrect.' }

$settings = Join-Path $env:LOCALAPPDATA 'CodexInfo'
$sentinel = Join-Path $settings ("installer-preserve-" + [Guid]::NewGuid().ToString('N') + '.txt')
New-Item -ItemType Directory -Path $settings -Force | Out-Null
Set-Content -LiteralPath $sentinel -Value 'preserve'
Invoke-BoundedSetupProcess -Path $setup -Operation 'Installer update' -Arguments @(
    '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
)
if (-not (Test-Path $sentinel -PathType Leaf)) { throw 'Update removed user settings.' }

$e2eOutput = Join-Path ([IO.Path]::GetTempPath()) ("codex-info-windows-e2e-" + [Guid]::NewGuid().ToString('N'))
& (Join-Path $PSScriptRoot 'Run-WindowsClientE2E.ps1') -ClientPath $exe -OutputDirectory $e2eOutput -Fixture -SourceSha $SourceSha
$moveOutput = @(& (Join-Path $root 'scripts\windows_window_move_smoke.ps1') -ClientPath $exe -AllowPhysicalInput *>&1)
$moveOutput | Tee-Object -FilePath (Join-Path $e2eOutput 'window-move-smoke.log')
if ($moveOutput.Count -eq 0 -or [string]$moveOutput[-1] -ne 'window-move-smoke: PASS') {
    throw 'Physical window move smoke did not report its terminal PASS contract.'
}

Invoke-BoundedSetupProcess -Path $uninstaller.FullName -Operation 'Uninstaller' -Arguments @(
    '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
)
if (Test-Path $install) { throw 'Install directory still exists after uninstall.' }
if (Test-Path $start) { throw 'Start-menu shortcut still exists after uninstall.' }
if (Test-Path $desktop) { throw 'Desktop shortcut still exists after uninstall.' }
if (Test-Path $key) { throw 'Per-user uninstall registration still exists.' }
if (-not (Test-Path $sentinel -PathType Leaf)) { throw 'Uninstall removed user settings.' }
Remove-Item -LiteralPath $sentinel -Force
