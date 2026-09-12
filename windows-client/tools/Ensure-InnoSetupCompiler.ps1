# Installs the exact Inno Setup compiler used by the Windows release-candidate
# workflow. Local reproduction and CI share this implementation.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$compiler = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 7\ISCC.exe'
$installerUrl = 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe'
$expectedSha256 = '0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f'
$installer = Join-Path ([IO.Path]::GetTempPath()) 'innosetup-7.1.0-x64.exe'

try {
    Invoke-WebRequest -Uri $installerUrl -OutFile $installer

    $actualSha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
    if (-not $actualSha256.Equals($expectedSha256, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Inno Setup installer SHA-256 mismatch: $actualSha256"
    }

    $signature = Get-AuthenticodeSignature -LiteralPath $installer
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Inno Setup installer Authenticode status is $($signature.Status)."
    }
    $publisher = $signature.SignerCertificate.GetNameInfo(
        [System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName,
        $false
    )
    if ($publisher -cne 'Pyrsys B.V.') {
        throw "Unexpected Inno Setup installer publisher: $publisher"
    }

    $installProcess = Start-Process -FilePath $installer `
        -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER') `
        -Wait -PassThru
    if ($installProcess.ExitCode -ne 0) {
        throw "Inno Setup installer failed with exit code $($installProcess.ExitCode)."
    }
    if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
        throw 'Inno Setup 7.1.0 compiler was not installed.'
    }
    Write-Host "inno-setup-compiler: PASS ($compiler)"
}
finally {
    if (Test-Path -LiteralPath $installer -PathType Leaf) {
        Remove-Item -LiteralPath $installer -Force
    }
}
