# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
$ErrorActionPreference = 'Stop'
$source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../tools/Run-WindowsClientE2E.ps1'))
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Setup runner has syntax errors.' }
$oracle = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
    $node.Name -eq 'Assert-E2ESetupAuthState'
}, $false)
if ($null -eq $oracle) { throw 'Setup has no native authentication acceptance oracle.' }
. ([ScriptBlock]::Create($oracle.Extent.Text))
function Assert-E2E { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message } }
foreach ($case in @('unauthenticated', 'unavailable', 'authenticated', 'premature-continue', 'missing-auth-action', 'blocked-after-auth', 'stale-auth-action')) {
    $authenticated = $case -in @('authenticated', 'blocked-after-auth', 'stale-auth-action')
    $state = if ($authenticated) { 'Authenticated' } elseif ($case -eq 'unavailable') { 'Unavailable' } else { 'AuthRequired' }
    $canContinue = $authenticated
    $canAuthenticate = $state -eq 'AuthRequired'
    switch ($case) {
        'premature-continue' { $canContinue = $true }
        'missing-auth-action' { $canAuthenticate = $false }
        'blocked-after-auth' { $canContinue = $false }
        'stale-auth-action' { $canAuthenticate = $true }
    }
    $failure = $null
    try { Assert-E2ESetupAuthState -CanContinue $canContinue -CanAuthenticate $canAuthenticate -State $state }
    catch { $failure = $_.Exception.Message }
    if (($null -ne $failure) -ne ($case -notin @('unauthenticated', 'unavailable', 'authenticated'))) { throw "Setup acceptance oracle differs for $case`: $failure" }
    Write-Host "windows-setup-oracle: PASS $case"
}
