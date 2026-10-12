# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [switch]$CallerOnly
)

$ErrorActionPreference = 'Stop'
$runner = Join-Path $PSScriptRoot '../tools/Run-WindowsClientE2E.ps1'
$source = [IO.File]::ReadAllText($runner)
$boundary = $source.IndexOf('function Write-E2E {', [StringComparison]::Ordinal)
if ($boundary -lt 0) { throw 'E2E initialization boundary is missing.' }
$initialization = $source.Substring(0, $boundary)
$testRoot = Join-Path ([IO.Path]::GetFullPath($OutputDirectory)) ([Guid]::NewGuid().ToString('N'))
$originalLocalAppData = $env:LOCALAPPDATA
$failures = [System.Collections.Generic.List[string]]::new()
$focusCases = @()

try {
    $cases = if ($CallerOnly) { @() } else {
        @('fixture-settings', 'existing-evidence', 'worktree-artifacts', 'repository-root')
    }
    foreach ($case in $cases) {
        $caseRoot = Join-Path $testRoot $case
        $repository = Join-Path $caseRoot 'repository'
        $tools = Join-Path $repository 'windows-client/tools'
        $null = New-Item -ItemType Directory -Path $tools -Force
        [IO.File]::WriteAllText((Join-Path $repository 'windows-client/Directory.Build.props'),
            '<Project><PropertyGroup><Version>1.2.3</Version></PropertyGroup></Project>')
        $subject = Join-Path $tools 'Initialize.ps1'
        [IO.File]::WriteAllText($subject, $initialization)
        $env:LOCALAPPDATA = Join-Path $caseRoot 'user-data'
        $userDirectory = Join-Path $env:LOCALAPPDATA 'CodexInfo'
        $null = New-Item -ItemType Directory -Path $userDirectory -Force
        $userSettings = Join-Path $userDirectory 'settings.json'
        [IO.File]::WriteAllText($userSettings, '{"user":"must-remain-unchanged"}')
        $output = switch ($case) {
            'worktree-artifacts' { Join-Path $repository 'artifacts/e2e' }
            'repository-root' { $repository }
            default { Join-Path $caseRoot 'evidence' }
        }
        $sentinel = Join-Path $output 'prior-result.txt'
        if ($case -eq 'existing-evidence' -or $case -eq 'repository-root') {
            $null = New-Item -ItemType Directory -Path $output -Force
            [IO.File]::WriteAllText($sentinel, 'preserve earlier evidence')
        }
        $failure = $null
        try {
            . $subject -Fixture -OutputDirectory $output
        }
        catch { $failure = $_.Exception.Message }

        $passed = switch ($case) {
            'fixture-settings' {
                $null -eq $failure -and
                $script:e2eSettingsPath -eq (Join-Path $output 'fixture-settings.json') -and
                [IO.File]::ReadAllText($userSettings) -eq '{"user":"must-remain-unchanged"}'
            }
            'worktree-artifacts' { $null -eq $failure -and [IO.Directory]::Exists($output) }
            default {
                $null -ne $failure -and [IO.File]::Exists($sentinel) -and
                [IO.File]::ReadAllText($sentinel) -eq 'preserve earlier evidence'
            }
        }
        if ($passed) { Write-Host "windows-e2e-isolation: PASS $case" }
        else {
            $failures.Add($case)
            Write-Host "windows-e2e-isolation: FAIL $case error=$failure"
        }
    }

    # Execute the real caller's invocation block with recording scripts.
    # Installer identity and physical mouse input are outside this contract.
    $callerRoot = Join-Path $testRoot 'caller'
    $null = New-Item -ItemType Directory -Path $callerRoot -Force
    $runner = Join-Path $callerRoot 'Runner.ps1'
    [IO.File]::WriteAllText($runner, @'
param([string]$ClientPath, [string]$OutputDirectory, [switch]$Fixture, [switch]$SettingsTabs)
@{ fixture = [bool]$Fixture; settings = [bool]$SettingsTabs; output = $OutputDirectory } |
    ConvertTo-Json -Compress | Add-Content -LiteralPath (Join-Path $PSScriptRoot 'calls.jsonl')
'@)
    $moveSmoke = Join-Path $callerRoot 'Move.ps1'
    [IO.File]::WriteAllText($moveSmoke, @'
param([string]$ClientPath, [switch]$AllowPhysicalInput)
'window-move-smoke: PASS'
'@)
    $ClientPath = Join-Path $callerRoot 'unused-client.exe'
    $OutputDirectory = Join-Path $callerRoot 'results'
    $tokens = $null
    $parseErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile(
        (Join-Path $PSScriptRoot '../tools/Reproduce-WindowsInstalledE2E.ps1'), [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count -ne 0) { throw 'Windows E2E caller has syntax errors.' }
    $invokeBlock = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.TryStatementAst]
    }, $false)
    if ($null -eq $invokeBlock) { throw 'Windows E2E caller invocation block is missing.' }
    $body = $invokeBlock.Body.Extent.Text
    & ([ScriptBlock]::Create($body.Substring(1, $body.Length - 2)))
    $calls = @(Get-Content -LiteralPath (Join-Path $callerRoot 'calls.jsonl') | ConvertFrom-Json)
    $callerPassed = $calls.Count -eq 2 -and $calls[0].fixture -and $calls[1].fixture -and
        -not $calls[0].settings -and $calls[1].settings -and
        $calls[0].output -eq (Join-Path $OutputDirectory 'standard') -and
        $calls[1].output -eq (Join-Path $OutputDirectory 'settings')
    if ($callerPassed) { Write-Host 'windows-e2e-isolation: PASS caller-settings-route' }
    else {
        $failures.Add('caller-settings-route')
        Write-Host "windows-e2e-isolation: FAIL caller-settings-route calls=$($calls.Count)"
    }
    $priorLog = Join-Path $OutputDirectory 'window-move-smoke.log'
    [IO.File]::WriteAllText($priorLog, 'preserve prior caller evidence')
    $failure = $null
    try { & ([ScriptBlock]::Create($body.Substring(1, $body.Length - 2))) }
    catch { $failure = $_.Exception.Message }
    $afterCalls = @(Get-Content -LiteralPath (Join-Path $callerRoot 'calls.jsonl'))
    if ($null -ne $failure -and $afterCalls.Count -eq 2 -and
        [IO.File]::ReadAllText($priorLog) -eq 'preserve prior caller evidence') {
        Write-Host 'windows-e2e-isolation: PASS caller-existing-evidence'
    }
    else {
        $failures.Add('caller-existing-evidence')
        Write-Host 'windows-e2e-isolation: FAIL caller-existing-evidence'
    }

    if (-not $CallerOnly) {
        # Execute the actual input helper against deterministic native results.
        # No real cursor, window or user process is touched by these cases.
        Add-Type -TypeDefinition @'
using System;
public static class CodexInfoWindowsE2EWin32 {
    public struct POINT { public int X; public int Y; }
    public static uint Owner;
    public static IntPtr Foreground, Hit;
    public static bool Activate;
    public static int Clicks, CursorMoves;
    public static uint GetWindowThreadProcessId(IntPtr h, out uint owner) { owner=Owner; return 1; }
    public static IntPtr GetForegroundWindow() { return Foreground; }
    public static IntPtr WindowFromPoint(POINT p) { return Hit; }
    public static IntPtr GetAncestor(IntPtr h, uint flags) { return h; }
    public static bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags) { return true; }
    public static bool GetCursorPos(out POINT p) { p=new POINT { X=1,Y=2 }; return true; }
    public static bool SetCursorPos(int x,int y) { CursorMoves++; return true; }
    public static void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra) {
        Clicks++; if (flags==4 && Activate) Foreground=new IntPtr(7);
    }
}
'@
        function Bring-E2EWindowToFront { param([IntPtr]$Handle) }
        function Get-E2EWindowBounds { param([IntPtr]$Handle) return @{ Left=0; Top=0 } }
        function Assert-E2E { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message } }
        $runnerAst = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$parseErrors)
        $focus = $runnerAst.Find({ param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Focus-E2EInputWindow'
        }, $false)
        if ($null -eq $focus) { throw 'Owned input helper is missing.' }
        . ([ScriptBlock]::Create($focus.Extent.Text))
        $script:e2eProcess = [pscustomobject]@{ Id=42 }
        $focusCases = @('already-focused', 'activate-owned-title', 'covered-title', 'foreign-process', 'activation-refused')
        foreach ($case in $focusCases) {
            [CodexInfoWindowsE2EWin32]::Owner = if ($case -eq 'foreign-process') { 99 } else { 42 }
            [CodexInfoWindowsE2EWin32]::Foreground = if ($case -eq 'already-focused') { 7 } else { 9 }
            [CodexInfoWindowsE2EWin32]::Hit = if ($case -eq 'covered-title') { 9 } else { 7 }
            [CodexInfoWindowsE2EWin32]::Activate = $case -ne 'activation-refused'
            [CodexInfoWindowsE2EWin32]::Clicks = 0
            [CodexInfoWindowsE2EWin32]::CursorMoves = 0
            $failure = $null
            try { Focus-E2EInputWindow ([IntPtr]7) } catch { $failure = $_.Exception.Message }
            $expectedFailure = $case -in @('covered-title', 'foreign-process', 'activation-refused')
            $expectedClicks = if ($case -in @('activate-owned-title', 'activation-refused')) { 2 } else { 0 }
            $passed = ($null -ne $failure) -eq $expectedFailure -and
                [CodexInfoWindowsE2EWin32]::Clicks -eq $expectedClicks -and
                [CodexInfoWindowsE2EWin32]::CursorMoves -eq $expectedClicks
            if ($passed) { Write-Host "windows-e2e-isolation: PASS $case" }
            else { $failures.Add($case); Write-Host "windows-e2e-isolation: FAIL $case error=$failure" }
        }
    }
}
finally { $env:LOCALAPPDATA = $originalLocalAppData }

if ($failures.Count -ne 0) {
    throw "E2E isolation failures: $($failures -join ', ')"
}
Write-Host "windows-e2e-isolation: PASS $($cases.Count + 2 + $focusCases.Count) cases"
