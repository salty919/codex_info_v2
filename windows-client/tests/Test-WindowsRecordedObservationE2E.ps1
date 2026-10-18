# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
# Runs only the production pixel assertion over independent in-memory bitmaps.
# It starts no fixture server, application, RPC, database or screen capture.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$runner = Join-Path $PSScriptRoot '../tools/Run-WindowsClientE2E.ps1'
$source = [IO.File]::ReadAllText($runner)
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Recorded-observation runner has syntax errors.' }
$callers = @($ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
    $node.GetCommandName() -in @('Assert-E2EGraphHasIdleBand', 'Assert-E2EGraphHasNoIdleBand')
}, $true))
if ($callers.Count -ne 1) { throw 'The real past-graph caller must use exactly one pixel assertion.' }
$callerName = $callers[0].GetCommandName()
$wrapper = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $callerName
}, $false)
if ($null -eq $wrapper) { throw 'The actual graph pixel assertion has no definition.' }
$bitmapCalls = @($wrapper.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
    $node.GetCommandName() -in @('Assert-E2EGraphIdleBandBitmap', 'Assert-E2EGraphNoIdleBandBitmap')
}, $true))
if ($bitmapCalls.Count -ne 1) { throw 'The real graph assertion must call one production bitmap oracle.' }
$bitmapName = $bitmapCalls[0].GetCommandName()
foreach ($name in @('Get-E2EGraphIdleBackgroundColor', 'Test-E2EGraphIdleBandPixel', $bitmapName)) {
    $definition = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $false)
    if ($null -eq $definition) { throw "Production bitmap dependency is missing: $name" }
    . ([ScriptBlock]::Create($definition.Extent.Text))
}
function Assert-E2E { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message } }

$failures = @()
foreach ($case in @('normal-plot-grid-and-points', 'opaque-full-height-idle-band')) {
    $bitmap = New-Object System.Drawing.Bitmap 200, 120
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $grid = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 38, 56, 80))
    $series = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 86, 178, 245))
    $point = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 113, 199, 255))
    $idle = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 22, 34, 50))
    try {
        # Literal colors and geometry are independent of the production oracle.
        $graphics.Clear([System.Drawing.Color]::FromArgb(255, 18, 28, 44))
        if ($case -eq 'opaque-full-height-idle-band') {
            $graphics.FillRectangle($idle, 0, 0, 90, 120)
        }
        foreach ($y in @(30, 60, 90)) { $graphics.DrawLine($grid, 0, $y, 199, $y) }
        foreach ($x in @(50, 100, 150)) { $graphics.DrawLine($grid, $x, 0, $x, 119) }
        $graphics.DrawLine($series, 10, 95, 190, 25)
        foreach ($x in @(20, 90, 160)) { $graphics.FillEllipse($point, $x, 70, 4, 4) }
        # An isolated same-colored marker is not a full-height band.
        $graphics.FillRectangle($idle, 110, 55, 3, 3)
        $graphics.Flush()
        $failure = $null
        try { & $bitmapName -Bitmap $bitmap -Left 0 -Top 0 -Right 200 -Bottom 120 | Out-Null }
        catch { $failure = $_.Exception.Message }
        $expectedRejection = $case -eq 'opaque-full-height-idle-band'
        $actualRejection = $null -ne $failure
        if ($actualRejection -ne $expectedRejection) {
            $failures += "$case expected-rejection=$expectedRejection actual-rejection=$actualRejection failure=$failure"
        }
        Write-Host "windows-recorded-observation-pixels: case=$case expected-rejection=$expectedRejection actual-rejection=$actualRejection"
    }
    finally {
        $graphics.Dispose()
        $grid.Dispose()
        $series.Dispose()
        $point.Dispose()
        $idle.Dispose()
        $bitmap.Dispose()
    }
}
if ($failures.Count -ne 0) { throw ($failures -join [Environment]::NewLine) }
if ($callerName -cne 'Assert-E2EGraphHasNoIdleBand' -or $bitmapName -cne 'Assert-E2EGraphNoIdleBandBitmap') {
    throw 'The actual legacy-observation caller still reaches an idle-required assertion.'
}
Write-Host 'windows-recorded-observation-pixels: PASS 2/2 production caller, normal plot accepted and opaque idle band rejected'
