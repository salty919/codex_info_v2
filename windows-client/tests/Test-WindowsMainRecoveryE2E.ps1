# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
$ErrorActionPreference = 'Stop'
$source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../tools/Run-WindowsClientE2E.ps1'))
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Main recovery runner has syntax errors.' }
$server = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.StringConstantExpressionAst] -and
    $node.Value.Contains('public static class CodexInfoWindowsE2EFixtureServer')
}, $true)
if ($null -eq $server) { throw 'Real HTTP fixture definition is missing.' }
Add-Type -TypeDefinition $server.Value
$fault = [CodexInfoWindowsE2EFixtureServer].GetField('CurrentUnavailable')
if ($null -eq $fault) { throw 'Current HTTP failure cannot be exercised by the native Main acceptance path.' }

function Read-RecoveryResponse {
    param([string]$Route)
    $request = [Net.HttpWebRequest]::Create("http://127.0.0.1:$script:recoveryPort$Route")
    $request.Proxy = $null
    $request.Timeout = 5000
    $response = $null
    try { $response = $request.GetResponse() }
    catch [Net.WebException] {
        if ($null -eq $_.Exception.Response) { throw }
        $response = $_.Exception.Response
    }
    try {
        $reader = [IO.StreamReader]::new($response.GetResponseStream())
        try {
            return @{ Status = [int]$response.StatusCode; Body = $reader.ReadToEnd(); Pair = $response.Headers['Codex-Info-Published-Pair'] }
        }
        finally { $reader.Dispose() }
    }
    finally { $response.Dispose() }
}

try {
    if (-not [CodexInfoWindowsE2EFixtureServer]::Start('{}', '{"accounts":[]}', '{"api_version":"v3","quota":72,"active_thread_count":2,"open_session_thread_count":4}', '{}', '{}', '{}', '{}', '{}', '{}', 'fixture-pair', '1.2.3', 0)) {
        throw 'Could not start the isolated HTTP fixture.'
    }
    $script:recoveryPort = [CodexInfoWindowsE2EFixtureServer]::BoundPort()
    foreach ($unavailable in @($false, $true, $false)) {
        $fault.SetValue($null, $unavailable)
        $current = Read-RecoveryResponse '/v3/current?account=account-7'
        if ($unavailable) {
            if ($current.Status -ne 503 -or -not [string]::IsNullOrEmpty($current.Pair)) {
                throw 'Unavailable current must return HTTP 503 without a published pair.'
            }
        }
        elseif ($current.Status -ne 200 -or $current.Body -cne '{"api_version":"v3","quota":72,"active_thread_count":2,"open_session_thread_count":4}' -or $current.Pair -cne 'fixture-pair') {
            throw 'Healthy and recovered current must return the unchanged recorded payload and pair.'
        }
        $health = Read-RecoveryResponse '/v1/health'
        $accounts = Read-RecoveryResponse '/v3/accounts'
        if ($health.Status -ne 200 -or $accounts.Status -ne 200 -or $accounts.Body -cne '{"accounts":[]}') {
            throw 'Current failure must not change health or account discovery.'
        }
    }
    $authentication = [CodexInfoWindowsE2EFixtureServer].GetField('AuthenticationRequired')
    if ($null -eq $authentication) { throw 'Setup cannot exercise an unauthenticated current response.' }
    $authentication.SetValue($null, $true)
    $unauthenticated = Read-RecoveryResponse '/v3/current?account=account-7'
    $body = $unauthenticated.Body | ConvertFrom-Json
    if ($unauthenticated.Status -ne 200 -or $unauthenticated.Pair -cne 'fixture-pair' -or
        $body.state -cne 'auth_required' -or $body.authenticated -ne $false -or
        $null -ne $body.quota -or $null -ne $body.plan_label -or $null -ne $body.observed_at -or
        @($body.models).Count -ne 0 -or $body.active_thread_count -ne 0) {
        throw 'Unauthenticated current must contain no invented usage or plan.'
    }
    $authentication.SetValue($null, $false)
    $recovered = Read-RecoveryResponse '/v3/current?account=account-7'
    if ($recovered.Status -ne 200 -or $recovered.Body -cne '{"api_version":"v3","quota":72,"active_thread_count":2,"open_session_thread_count":4}') { throw 'Authentication recovery changed the original payload.' }
    $v4 = [CodexInfoWindowsE2EFixtureServer].GetField('PartialCurrentV4')
    if ($null -eq $v4) { throw 'Native Main cannot exercise a v4 partial current response.' }
    if ((Read-RecoveryResponse '/v4/current?account=account-7').Status -ne 404) { throw 'Legacy fixture must retain exact v4 fallback.' }
    $v4.SetValue($null, $true)
    $partial = Read-RecoveryResponse '/v4/current?account=account-7'
    $body = $partial.Body | ConvertFrom-Json
    if ($partial.Status -ne 200 -or $partial.Pair -cne 'fixture-pair' -or
        $body.api_version -cne 'v4' -or $body.models_status -cne 'partial' -or $body.quota -ne 72 -or
        $body.active_thread_count -ne 2 -or $body.open_session_thread_count -ne 4) {
        throw 'Partial v4 must preserve the recorded current values and pair.'
    }
    $empty = [CodexInfoWindowsE2EFixtureServer].GetField('EmptyThreads')
    if ($null -eq $empty) { throw 'Native Threads cannot exercise a confirmed empty generation.' }
    $empty.SetValue($null, $true)
    $body = (Read-RecoveryResponse '/v4/current?account=account-7').Body | ConvertFrom-Json
    $threads = (Read-RecoveryResponse '/v3/threads?account=account-7').Body | ConvertFrom-Json
    if ($body.active_thread_count -ne 0 -or $body.open_session_thread_count -ne 0 -or
        @($threads.threads).Count -ne 0 -or $body.quota -ne 72) {
        throw 'Empty Threads must clear both counts and rows without changing quota.'
    }
    $empty.SetValue($null, $false)
    $v4.SetValue($null, $false)
    if ((Read-RecoveryResponse '/v4/current?account=account-7').Status -ne 404) { throw 'V4 fixture state did not reset.' }
    Write-Host 'windows-main-recovery-fixture: PASS healthy, unavailable, authentication, v4 partial, empty threads and recovery'
}
finally { [CodexInfoWindowsE2EFixtureServer]::Stop() }

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
