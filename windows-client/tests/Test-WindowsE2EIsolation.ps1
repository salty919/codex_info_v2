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
$popupCases = @()
$hoverCases = @()

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
param([string]$ClientPath, [string]$OutputDirectory, [switch]$Fixture,
    [switch]$SettingsTabs, [switch]$ThemePresets, [switch]$GraphThemes, [switch]$CalendarOnly)
@{ fixture = [bool]$Fixture; settings = [bool]$SettingsTabs;
    themes = [bool]$ThemePresets; graph = [bool]$GraphThemes; calendar = [bool]$CalendarOnly; output = $OutputDirectory } |
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
    $expectedRoutes = @(
        @{ Directory = 'standard'; Settings = $false; Themes = $false; Graph = $false; Calendar = $false },
        @{ Directory = 'settings'; Settings = $true; Themes = $false; Graph = $false; Calendar = $false },
        @{ Directory = 'theme-presets'; Settings = $false; Themes = $true; Graph = $false; Calendar = $false },
        @{ Directory = 'graph-themes'; Settings = $false; Themes = $false; Graph = $true; Calendar = $false },
        @{ Directory = 'calendar'; Settings = $false; Themes = $false; Graph = $false; Calendar = $true }
    )
    $callerPassed = $calls.Count -eq $expectedRoutes.Count
    for ($index = 0; $callerPassed -and $index -lt $expectedRoutes.Count; $index++) {
        $actual = $calls[$index]
        $expected = $expectedRoutes[$index]
        $callerPassed = $actual.fixture -and $actual.settings -eq $expected.Settings -and
            $actual.themes -eq $expected.Themes -and $actual.graph -eq $expected.Graph -and
            $actual.calendar -eq $expected.Calendar -and
            $actual.output -eq (Join-Path $OutputDirectory $expected.Directory)
    }
    if ($callerPassed) { Write-Host 'windows-e2e-isolation: PASS caller-screen-routes' }
    else {
        $failures.Add('caller-screen-routes')
        Write-Host "windows-e2e-isolation: FAIL caller-screen-routes calls=$($calls.Count)"
    }
    $priorLog = Join-Path $OutputDirectory 'window-move-smoke.log'
    [IO.File]::WriteAllText($priorLog, 'preserve prior caller evidence')
    $failure = $null
    try { & ([ScriptBlock]::Create($body.Substring(1, $body.Length - 2))) }
    catch { $failure = $_.Exception.Message }
    $afterCalls = @(Get-Content -LiteralPath (Join-Path $callerRoot 'calls.jsonl'))
    if ($null -ne $failure -and $afterCalls.Count -eq $calls.Count -and
        [IO.File]::ReadAllText($priorLog) -eq 'preserve prior caller evidence') {
        Write-Host 'windows-e2e-isolation: PASS caller-existing-evidence'
    }
    else {
        $failures.Add('caller-existing-evidence')
        Write-Host 'windows-e2e-isolation: FAIL caller-existing-evidence'
    }

    if (-not $CallerOnly) {
        # These caller tests use no desktop or UIA runtime. The desktop double
        # rejects global subtree searches, even when a PID condition is supplied.
        Add-Type -TypeDefinition @'
using System;
using System.Linq;
namespace System.Windows.Automation {
    public static class ControlType { public static readonly object ListItem = new object(); }
    public enum TreeScope { Children, Descendants }
    public abstract class Condition { public abstract bool Matches(AutomationElement element); }
    public class PropertyCondition : Condition {
        readonly object property, value;
        public PropertyCondition(object property, object value) { this.property=property; this.value=value; }
        public override bool Matches(AutomationElement element) {
            return property == AutomationElement.ProcessIdProperty
                ? element.ProcessId == Convert.ToInt32(value) : element.AutomationId == (string)value;
        }
    }
    public class AndCondition : Condition {
        readonly Condition left, right;
        public AndCondition(Condition left, Condition right) { this.left=left; this.right=right; }
        public override bool Matches(AutomationElement element) { return left.Matches(element) && right.Matches(element); }
    }
    public class AutomationElement {
        public static readonly object ProcessIdProperty = new object(), AutomationIdProperty = new object();
        public static AutomationElement RootElement;
        public bool Desktop, Fail, IsOffscreen;
        public int ProcessId;
        public string AutomationId;
        public AutomationElement Current { get { return this; } }
        public AutomationElement[] Elements = new AutomationElement[0];
        public AutomationElement[] FindAll(TreeScope scope, Condition condition) {
            if (Desktop && scope == TreeScope.Descendants) throw new InvalidOperationException("Global desktop subtree query");
            if (Fail) throw new InvalidOperationException("Owned provider failure");
            return Elements.Where(condition.Matches).ToArray();
        }
    }
}
'@
        $hoverAst = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$parseErrors)
        $hoverHelper = $hoverAst.Find({ param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Find-E2EGraphHoverElement'
        }, $false)
        if ($null -eq $hoverHelper) { throw 'Graph hover query helper is missing.' }
        . ([ScriptBlock]::Create($hoverHelper.Extent.Text))
        $hoverCases = @('owned-popup', 'foreign-popup', 'hidden-popup', 'provider-error')
        foreach ($case in $hoverCases) {
            $timestamp = New-Object System.Windows.Automation.AutomationElement
            $timestamp.ProcessId = 42
            $timestamp.AutomationId = 'Graph.Hover.Timestamp'
            $timestamp.IsOffscreen = $case -eq 'hidden-popup'
            $owned = New-Object System.Windows.Automation.AutomationElement
            $owned.ProcessId = 42
            if ($case -ne 'foreign-popup') { $owned.Elements = @($timestamp) }
            $owned.Fail = $case -eq 'provider-error'
            $foreign = New-Object System.Windows.Automation.AutomationElement
            $foreign.ProcessId = 99
            $foreign.Elements = @($timestamp)
            $foreign.Fail = $true
            $desktop = New-Object System.Windows.Automation.AutomationElement
            $desktop.Desktop = $true
            $desktop.Elements = @($foreign, $owned)
            [System.Windows.Automation.AutomationElement]::RootElement = $desktop
            $failure = $null
            $actual = $null
            try { $actual = Find-E2EGraphHoverElement 42 'Graph.Hover.Timestamp' }
            catch { $failure = $_.Exception.Message }
            $passed = if ($case -eq 'provider-error') {
                $null -ne $failure -and $failure.Contains('Owned provider failure')
            } elseif ($case -eq 'owned-popup') {
                $null -eq $failure -and [object]::ReferenceEquals($actual, $timestamp)
            } else { $null -eq $failure -and $null -eq $actual }
            if ($passed) { Write-Host "windows-e2e-isolation: PASS hover-$case" }
            else { $failures.Add("hover-$case"); Write-Host "windows-e2e-isolation: FAIL hover-$case error=$failure" }
        }
        $popupStart = $source.IndexOf("    # The popup is an overlay in the Graph HWND", [StringComparison]::Ordinal)
        $popupEnd = $source.IndexOf('    Bring-E2EWindowToFront $Windows.Settings.Handle', $popupStart, [StringComparison]::Ordinal)
        if ($popupStart -lt 0 -or $popupEnd -le $popupStart) { throw 'Theme popup capture block is missing.' }
        $popupBody = [ScriptBlock]::Create($source.Substring($popupStart, $popupEnd - $popupStart))
        Add-Type -AssemblyName System.Drawing
        $pixelAst = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$parseErrors)
        $pixelHelper = $pixelAst.Find({ param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Assert-E2EThemePixel'
        }, $false)
        if ($null -eq $pixelHelper) { throw 'Theme pixel helper is missing.' }
        $imageRoot = Join-Path $testRoot 'popup-pixels'
        $null = New-Item -ItemType Directory -Path $imageRoot -Force
        $popupCases = @('visible', 'closed-after-capture', 'hidden-before-capture',
            'selection-hidden-before-capture', 'missing-popup-color', 'missing-selected-color')
        foreach ($case in $popupCases) {
            $passed = & {
                param($Case, $Body, $PixelHelper, $ImageRoot)
                $graph = @{ Root = 'graph'; Handle = [IntPtr]7 }
                $ThemeId = 'fixed-theme'
                $colors = @{ Popup = '#112233'; PopupSelected = '#445566' }
                $menu = [pscustomobject]@{ Current = [pscustomobject]@{
                    IsOffscreen = $Case -eq 'hidden-before-capture'
                    BoundingRectangle = [pscustomobject]@{ Left=10; Top=20; Width=60; Height=40; Right=70; Bottom=60 }
                } }
                $item = [pscustomobject]@{ Current = [pscustomobject]@{
                    Name = 'Past'; IsOffscreen = $false
                    BoundingRectangle = [pscustomobject]@{ Left=10; Top=40; Width=60; Height=20; Right=70; Bottom=60 }
                } }
                $state = @{ Captured=$false; PixelChecks=0 }
                function Assert-E2E { param($Condition,$Message) if (-not $Condition) { throw $Message } }
                function Find-E2EElementByAutomationId { param($Root,$Id) if ($Id -eq 'Graph.PeriodMenu') { return $menu }; return 'selector' }
                function Bring-E2EWindowToFront { param($Handle) }
                function Toggle-E2EElement { param($Element) }
                function Wait-E2E { param($Description,$Probe) $value = & $Probe; if (-not $value) { throw 'Menu is hidden before capture.' }; return $value }
                function Get-E2EVisibleControlElements { param($Root,$Type) return $item }
                function Get-E2ESelectedListItemLabel {
                    param($Items)
                    if ($Case -eq 'selection-hidden-before-capture') { $item.Current.IsOffscreen = $true }
                    return 'Past'
                }
                function Get-E2EWindowBounds { param($Handle) return @{ Left=0; Top=0 } }
                function Write-E2E { param($Message) $state.PixelChecks++ }
                . ([ScriptBlock]::Create($PixelHelper))
                function Capture-E2EWindow {
                    param($Handle,$Name)
                    $state.Captured = $true
                    $path = Join-Path $ImageRoot ($Case + '.png')
                    $bitmap = New-Object System.Drawing.Bitmap(80, 80)
                    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
                    $background = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(17,34,51))
                    $selectedBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(68,85,102))
                    try {
                        $graphics.Clear([System.Drawing.Color]::Black)
                        if ($Case -ne 'missing-popup-color') { $graphics.FillRectangle($background, 10, 20, 60, 40) }
                        if ($Case -notin @('missing-popup-color', 'missing-selected-color')) {
                            $graphics.FillRectangle($selectedBrush, 10, 40, 60, 20)
                        }
                        $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
                    }
                    finally { $selectedBrush.Dispose(); $background.Dispose(); $graphics.Dispose(); $bitmap.Dispose() }
                    if ($Case -eq 'closed-after-capture') {
                        $menu.Current.IsOffscreen = $true
                        $item.Current.IsOffscreen = $true
                        $menu.Current.BoundingRectangle = [pscustomobject]@{ Left=0; Top=0; Width=0; Height=0; Right=0; Bottom=0 }
                        $item.Current.BoundingRectangle = $menu.Current.BoundingRectangle
                    }
                    return @{ Path=$path }
                }
                $failure = $null
                try { & $Body } catch { $failure = $_.Exception.Message }
                if ($Case -eq 'hidden-before-capture') {
                    return $failure -eq 'Menu is hidden before capture.' -and -not $state.Captured -and $state.PixelChecks -eq 0
                }
                if ($Case -eq 'selection-hidden-before-capture') {
                    return $failure -eq 'Graph period selection is not visible before capture.' -and -not $state.Captured
                }
                if ($Case -in @('missing-popup-color', 'missing-selected-color')) {
                    $role = if ($Case -eq 'missing-popup-color') { 'popup-surface' } else { 'popup-selected' }
                    $successfulChecks = if ($Case -eq 'missing-popup-color') { 0 } else { 1 }
                    return $null -ne $failure -and $failure.StartsWith("fixed-theme/Graph/$role has no rendered ") -and
                        $state.Captured -and $state.PixelChecks -eq $successfulChecks
                }
                if ($null -ne $failure) { Write-Host "popup evidence detail: $failure" }
                return $null -eq $failure -and $state.PixelChecks -eq 2
            } $case $popupBody $pixelHelper.Extent.Text $imageRoot
            if ($passed) { Write-Host "windows-e2e-isolation: PASS popup-$case" }
            else { $failures.Add("popup-$case"); Write-Host "windows-e2e-isolation: FAIL popup-$case" }
        }
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
Write-Host "windows-e2e-isolation: PASS $($cases.Count + 2 + $focusCases.Count + $popupCases.Count + $hoverCases.Count) cases"
& (Join-Path $PSScriptRoot 'Test-WindowsCalendarE2E.ps1')
& (Join-Path $PSScriptRoot 'Test-WindowsMainRecoveryE2E.ps1')
& (Join-Path $PSScriptRoot 'Test-WindowsSetupE2E.ps1')
