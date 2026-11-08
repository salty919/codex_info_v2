# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
# Shared by the existing Calendar/Main fixture contract tests. No UI or user
# profile is launched. Every request uses the same qualified H2 client as E2E.
. (Join-Path $PSScriptRoot 'E2E-WireFixture.ps1')

function Initialize-E2EWireContractFixture {
    foreach ($name in @('New-E2EFixtureDocuments', 'Get-E2EFixtureObservationTime')) {
        $definition = $ast.Find({ param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
        }, $false)
        if ($null -eq $definition) { throw "Missing real fixture producer: $name" }
        . ([ScriptBlock]::Create('function script:' + $definition.Extent.Text.Substring(9)))
    }
    foreach ($name in @('e2eFixtureUnusedMinimumSeconds', 'e2eFixturePastIdleStartFraction', 'e2eFixturePastIdleEndFraction')) {
        $assignment = $ast.Find({ param($node)
            $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
            $node.Left.Extent.Text -eq ('$script:' + $name)
        }, $false)
        . ([ScriptBlock]::Create($assignment.Extent.Text))
    }
    $script:e2eProductVersion = '1.2.3'
    $script:e2eRepositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
    $script:e2eOutput = Join-Path $OutputDirectory ('wire-contract-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $script:e2eOutput | Out-Null
    $script:FixtureHostPath = $env:CODEX_INFO_E2E_FIXTURE_HOST
    if ([string]::IsNullOrWhiteSpace($script:FixtureHostPath)) {
        $publish = Join-Path $OutputDirectory 'wire-contract-host'
        $script:FixtureHostPath = Join-Path $publish 'CodexInfo.WindowsClient.E2E.FixtureHost.exe'
        if (-not (Test-Path -LiteralPath $script:FixtureHostPath -PathType Leaf)) {
            & dotnet publish (Join-Path $PSScriptRoot 'CodexInfo.WindowsClient.E2E.FixtureHost/CodexInfo.WindowsClient.E2E.FixtureHost.csproj') --configuration Release --no-self-contained -p:RestoreLockedMode=true --output $publish
            if ($LASTEXITCODE -ne 0) { throw 'Could not build the H2 contract fixture host.' }
        }
        $env:CODEX_INFO_E2E_FIXTURE_HOST = $script:FixtureHostPath
    }
}

function Read-E2EWireContract {
    param([string]$Target, [int]$ExpectedStatus = 200)
    $cache = Join-Path $script:e2eOutput ('probe-' + [Guid]::NewGuid().ToString('N'))
    $arguments = @('--probe', "http://127.0.0.1:$script:e2eFixturePort/", '--target', $Target, '--cache', $cache)
    if ($ExpectedStatus -ne 200) { $arguments += @('--expect-http-status', [string]$ExpectedStatus) }
    $body = @(& $script:FixtureHostPath @arguments) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "Qualified wire probe failed for $Target" }
    $snapshot = $body | ConvertFrom-Json
    if ($ExpectedStatus -ne 200) {
        if ($snapshot.http_status -ne $ExpectedStatus) { throw 'HTTP failure classification differs.' }
        return $snapshot
    }
    foreach ($view in $snapshot.views) {
        if ($view.id -cnotmatch '^[0-9a-f]{64}$' -or $view.image -cnotmatch '^[0-9a-f]{64}$') { throw 'Unqualified contract view.' }
    }
    return @($snapshot.views | ForEach-Object { $_.rows } | ForEach-Object { $_.value })
}
