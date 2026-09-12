# Builds a standard Windows setup wizard containing the published client.
[CmdletBinding()]
param(
    [string]$Configuration = 'Release',
    [string]$Runtime = 'win-x64',
    [string]$OutputDirectory = 'artifacts/windows-installer',
    [string]$SourceSha = ''
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$clientProject = Join-Path $root 'windows-client\src\CodexInfo.WindowsClient\CodexInfo.WindowsClient.csproj'
$versionProps = Join-Path $root 'windows-client\Directory.Build.props'
$installerScript = Join-Path $root 'windows-client\installer\CodexInfo.WindowsClient.iss'
$productIcon = Join-Path $root 'windows-client\src\CodexInfo.WindowsClient\Assets\CodexInfo.ico'
$output = Join-Path $root $OutputDirectory
$work = Join-Path ([IO.Path]::GetTempPath()) ("codex-info-installer-" + [Guid]::NewGuid().ToString('N'))
$payload = Join-Path $work 'payload'

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

    $version = $versionNodes[0].InnerText.Trim()
    if ($version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
        throw "Version must be a stable X.Y.Z value: $version"
    }

    return $version
}

$version = Get-AuthoritativeVersion -Path $versionProps
if (-not [string]::IsNullOrWhiteSpace($SourceSha) -and
    $SourceSha -cnotmatch '^[0-9a-f]{40}$') {
    throw "SourceSha is not a full lowercase commit SHA: $SourceSha"
}

$compilerCandidates = @(
    $env:INNO_SETUP_COMPILER,
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 7\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 7\ISCC.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 7\ISCC.exe')
) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
$compiler = $compilerCandidates |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
    Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($compiler)) {
    throw 'Inno Setup compiler was not found. Install JRSoftware.InnoSetup.7 or set INNO_SETUP_COMPILER.'
}

try {
    New-Item -ItemType Directory -Path $payload -Force | Out-Null
    dotnet restore $clientProject --runtime $Runtime --locked-mode
    if ($LASTEXITCODE -ne 0) {
        throw "dotnet restore failed with exit code $LASTEXITCODE"
    }
    $revisionArgument = @()
    if (-not [string]::IsNullOrWhiteSpace($SourceSha)) {
        $revisionArgument = @("-p:SourceRevisionId=$SourceSha")
    }
    dotnet publish $clientProject --configuration $Configuration --runtime $Runtime `
        --self-contained true --output $payload --no-restore @revisionArgument
    if ($LASTEXITCODE -ne 0) {
        throw "dotnet publish failed with exit code $LASTEXITCODE"
    }
    & (Join-Path $PSScriptRoot 'Collect-ThirdPartyNotices.ps1') -Destination $payload
    New-Item -ItemType Directory -Path $output -Force | Out-Null
    & $compiler "/DPayloadDir=$payload" "/DOutputDir=$output" "/DProductIcon=$productIcon" "/DProductVersion=$version" $installerScript
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup compiler failed with exit code $LASTEXITCODE"
    }
    $setup = Join-Path $output 'CodexInfo.WindowsClient.Setup.exe'
    if (-not (Test-Path $setup -PathType Leaf)) {
        throw "Installer publish did not create $setup"
    }
    Write-Host "Created $setup"
}
finally {
    if (Test-Path $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
