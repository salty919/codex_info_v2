# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
$ErrorActionPreference = 'Stop'
$source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../tools/Run-WindowsClientE2E.ps1'))
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Main recovery runner has syntax errors.' }
. (Join-Path $PSScriptRoot 'E2E-WireContractFixture.ps1')
function Assert-E2E { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message } }
Initialize-E2EWireContractFixture
$documents = New-E2EFixtureDocuments
Start-E2EWireFixture -Documents $documents
try {
    $baseline = @(Read-E2EWireContract '/state?parts=main')[0]
    Assert-E2E ($baseline.quota.remaining_percent -eq 72 -and $baseline.models.Count -eq 3 -and $baseline.thread_summary.open_count -eq 4) 'Known baseline usage is missing.'
    foreach ($unavailable in @($false, $true, $false)) {
        [CodexInfoWindowsE2EFixtureServer]::CurrentUnavailable = $unavailable
        if ($unavailable) { $null = Read-E2EWireContract '/state?parts=main' -ExpectedStatus 503 }
        else {
            $current = @(Read-E2EWireContract '/state?parts=main')[0]
            Assert-E2E (($current | ConvertTo-Json -Depth 40 -Compress) -ceq ($baseline | ConvertTo-Json -Depth 40 -Compress)) 'Recovery changed the recorded payload.'
        }
        $runtime = @(Read-E2EWireContract '/state?parts=runtime')[0]
        $choices = @(Read-E2EWireContract '/state?parts=account_choices')[0]
        Assert-E2E ($runtime.rest_product_version -ceq '1.2.3' -and $choices.default_account_id -ceq 'account-7') 'Main failure must not change UI runtime or account choices.'
    }
    [CodexInfoWindowsE2EFixtureServer]::AuthenticationRequired = $true
    $body = @(Read-E2EWireContract '/state?parts=main')[0]
    Assert-E2E ($body.acquisition_state -ceq 'auth_required' -and -not $body.authenticated -and $null -eq $body.quota -and $null -eq $body.plan_label -and $null -eq $body.observed_at -and @($body.models).Count -eq 0 -and $null -eq $body.thread_summary) 'Unauthenticated Main must not invent usage or a plan.'
    [CodexInfoWindowsE2EFixtureServer]::AuthenticationRequired = $false
    [CodexInfoWindowsE2EFixtureServer]::PartialModels = $true
    $body = @(Read-E2EWireContract '/state?parts=main')[0]
    Assert-E2E ($body.models_status -ceq 'partial' -and $body.quota.remaining_percent -eq 72 -and ($body.models | ConvertTo-Json -Depth 20 -Compress) -ceq ($baseline.models | ConvertTo-Json -Depth 20 -Compress)) 'Partial known models must retain their exact token and price facts.'
    [CodexInfoWindowsE2EFixtureServer]::EmptyThreads = $true
    $body = @(Read-E2EWireContract '/state?parts=main')[0]
    $threads = @(Read-E2EWireContract '/threads?account=account-7')
    Assert-E2E ($body.thread_summary.open_count -eq 0 -and $body.thread_summary.running_count -eq 0 -and $threads.Count -eq 0 -and $body.quota.remaining_percent -eq 72) 'Confirmed empty Threads must clear counts and rows while retaining quota.'
    [CodexInfoWindowsE2EFixtureServer]::EmptyThreads = $false
    [CodexInfoWindowsE2EFixtureServer]::PartialModels = $false
    $restored = @(Read-E2EWireContract '/state?parts=main')[0]
    Assert-E2E (($restored | ConvertTo-Json -Depth 40 -Compress) -ceq ($baseline | ConvertTo-Json -Depth 40 -Compress)) 'Fixture controls must restore the exact baseline.'
    Write-Host 'windows-main-recovery-fixture: PASS healthy, HTTP failure, authentication, partial known facts, empty threads and exact recovery'
} finally { [CodexInfoWindowsE2EFixtureServer]::Stop() }

$oracle = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
    $node.Name -eq 'Assert-E2EMainRecoverySnapshot'
}, $false)
if ($null -eq $oracle) { throw 'Native Main recovery has no recorded-value acceptance oracle.' }
. ([ScriptBlock]::Create($oracle.Extent.Text))
function Assert-E2E { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message } }
$rows = @('LUNA|1,800|$2.00|700|$0.70|1,100|$0.90', 'TERRA|1,100|$1.40|500|$0.50|800|$0.50', 'SOL|600|$0.70|200|$0.20|400|$0.30')
foreach ($case in @('initial', 'ready', 'last-good', 'recovered', 'missing-model', 'wrong-input', 'lost-quota', 'stale-error', 'missing-retry', 'invented-initial')) {
    $initial = $case -in @('initial', 'invented-initial')
    $failed = $initial -or $case -in @('last-good', 'missing-retry')
    $snapshot = @{
        Generation = if ($failed) { 'error' } else { 'ready' }
        RetryCount = if ($failed) { 1 } else { 0 }
        Quota = if ($initial) { '' } else { '72%' }
        Estimate = if ($initial) { '' } else { 'Estimated $7.20' }
        Rows = if ($initial) { @() } else { @($rows) }
    }
    switch ($case) {
        'missing-model' { $snapshot.Rows = $rows[0..1] }
        'wrong-input' { $snapshot.Rows = @($rows[0], $rows[1], 'SOL|800|$0.70|200|$0.20|400|$0.30') }
        'lost-quota' { $snapshot.Quota = '' }
        'stale-error' { $snapshot.Generation = 'error' }
        'missing-retry' { $snapshot.RetryCount = 0 }
        'invented-initial' { $snapshot.Quota = '0%' }
    }
    $failure = $null
    try { Assert-E2EMainRecoverySnapshot -Snapshot $snapshot -HasRecordedValues (-not $initial) -Failed $failed }
    catch { $failure = $_.Exception.Message }
    $shouldFail = $case -notin @('initial', 'ready', 'last-good', 'recovered')
    if (($null -ne $failure) -ne $shouldFail) { throw "Main recorded-value oracle differs for $case`: $failure" }
    Write-Host "windows-main-recovery-oracle: PASS $case"
}
