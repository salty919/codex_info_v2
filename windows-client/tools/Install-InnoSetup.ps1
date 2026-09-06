# Installs the exact Inno Setup compiler used for the Windows release build.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$installerUrl = 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe'
$expectedSha256 = '0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f'
$temporaryRoot = if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
    [IO.Path]::GetTempPath()
}
else {
    $env:RUNNER_TEMP
}
$installer = Join-Path $temporaryRoot 'innosetup-7.1.0-x64.exe'
$installDirectory = Join-Path $temporaryRoot ("inno-7.1.0-" + [Guid]::NewGuid().ToString('N'))
$compiler = Join-Path $installDirectory 'ISCC.exe'
$installed = $false

try {
    Invoke-WebRequest -Uri $installerUrl -OutFile $installer -TimeoutSec 300
    $actualSha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
    if (-not $actualSha256.Equals($expectedSha256, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Inno Setup installer SHA-256 mismatch: $actualSha256"
    }

    $process = Start-Process -FilePath $installer -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER',
        "/DIR=`"$installDirectory`""
    ) -PassThru
    if (-not $process.WaitForExit(300000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw 'Inno Setup installer timed out.'
    }
    if ($process.ExitCode -ne 0) {
        throw "Inno Setup installer failed with exit code $($process.ExitCode)."
    }
    if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
        throw 'Inno Setup 7.1.0 compiler was not installed.'
    }
    $installed = $true
    Write-Output $compiler
}
finally {
    if (Test-Path -LiteralPath $installer -PathType Leaf) {
        Remove-Item -LiteralPath $installer -Force
    }
    if (-not $installed -and (Test-Path -LiteralPath $installDirectory)) {
        Remove-Item -LiteralPath $installDirectory -Recurse -Force
    }
}
