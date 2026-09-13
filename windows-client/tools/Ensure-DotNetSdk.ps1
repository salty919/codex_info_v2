# Installs the same rolling .NET 10 SDK channel requested by setup-dotnet in
# the Windows workflow. The SDK is kept in a per-user build-tools cache and is
# added only to this PowerShell process environment.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$metadataUrl = 'https://builds.dotnet.microsoft.com/dotnet/release-metadata/10.0/releases.json'
$metadata = Invoke-RestMethod -Uri $metadataUrl
$sdkVersion = [string]$metadata.'latest-sdk'
if ($sdkVersion -cnotmatch '^10\.0\.[1-9][0-9]*$') {
    throw "The .NET 10 release metadata returned an invalid latest SDK version: $sdkVersion"
}

$matchingReleases = @($metadata.releases | Where-Object {
    $null -ne $_.sdk -and ([string]$_.sdk.version) -ceq $sdkVersion
})
if ($matchingReleases.Count -ne 1) {
    throw "The .NET 10 release metadata does not identify exactly one SDK $sdkVersion."
}
$sdkFiles = @($matchingReleases[0].sdk.files | Where-Object {
    ([string]$_.rid) -ceq 'win-x64' -and
    ([string]$_.name) -ceq 'dotnet-sdk-win-x64.zip'
})
if ($sdkFiles.Count -ne 1) {
    throw "The .NET 10 release metadata does not identify exactly one win-x64 SDK archive for $sdkVersion."
}
$sdkUrl = [string]$sdkFiles[0].url
$expectedSha512 = [string]$sdkFiles[0].hash
if (-not [Uri]::IsWellFormedUriString($sdkUrl, [UriKind]::Absolute) -or
    $expectedSha512 -cnotmatch '^[0-9a-f]{128}$') {
    throw "The .NET 10 SDK archive authority is invalid for $sdkVersion."
}

$toolsRoot = Join-Path $env:LOCALAPPDATA 'CodexInfo\build-tools\dotnet'
$sdkRoot = Join-Path $toolsRoot $sdkVersion
$dotnet = Join-Path $sdkRoot 'dotnet.exe'
if (-not (Test-Path -LiteralPath $dotnet -PathType Leaf)) {
    if (Test-Path -LiteralPath $sdkRoot) {
        throw "The cached .NET SDK directory is incomplete: $sdkRoot"
    }
    $temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) `
        ("codex-info-dotnet-sdk-" + [Guid]::NewGuid().ToString('N'))
    $archive = Join-Path $temporaryRoot 'dotnet-sdk-win-x64.zip'
    $extracted = Join-Path $temporaryRoot 'extracted'
    try {
        New-Item -ItemType Directory -Path $extracted -Force | Out-Null
        Invoke-WebRequest -Uri $sdkUrl -OutFile $archive
        $actualSha512 = (Get-FileHash -LiteralPath $archive -Algorithm SHA512).Hash
        if (-not $actualSha512.Equals($expectedSha512, [StringComparison]::OrdinalIgnoreCase)) {
            throw "The .NET SDK archive SHA-512 does not match release metadata: $actualSha512"
        }
        Expand-Archive -LiteralPath $archive -DestinationPath $extracted
        if (-not (Test-Path -LiteralPath (Join-Path $extracted 'dotnet.exe') -PathType Leaf)) {
            throw 'The verified .NET SDK archive does not contain dotnet.exe.'
        }
        New-Item -ItemType Directory -Path $toolsRoot -Force | Out-Null
        Move-Item -LiteralPath $extracted -Destination $sdkRoot
    }
    finally {
        if (Test-Path -LiteralPath $temporaryRoot) {
            Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
        }
    }
}

$env:DOTNET_ROOT = $sdkRoot
$env:DOTNET_ROOT_X64 = $sdkRoot
$env:DOTNET_MULTILEVEL_LOOKUP = '0'
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE = '1'
$env:PATH = "$sdkRoot;$($env:PATH)"
$installedSdks = @(& $dotnet --list-sdks)
if ($LASTEXITCODE -ne 0 -or
    @($installedSdks | Where-Object { $_ -match ('^' + [regex]::Escape($sdkVersion) + '\s+\[') }).Count -ne 1) {
    throw "The prepared dotnet host does not expose SDK $sdkVersion."
}
Write-Host "dotnet-sdk: PASS ($sdkVersion)"
