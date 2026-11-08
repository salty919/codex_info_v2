# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
$ErrorActionPreference = 'Stop'
$tokens = $null
$errors = $null
$source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../tools/Run-WindowsClientE2E.ps1'))
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Calendar runner has syntax errors.' }
foreach ($name in @('Get-E2ECalendarExpectedDays', 'Assert-E2ECalendarDays', 'Get-E2EFixtureObservationTime', 'Assert-E2ECalendarVisibleValues')) {
    $definition = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $false)
    if ($null -eq $definition) { throw "Calendar evaluation is missing: $name" }
    . ([ScriptBlock]::Create($definition.Extent.Text))
}
function Assert-E2E { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message } }

foreach ($entry in @(
        @{ Input = '2026-10-13T00:00:35Z'; Boundary = $true; Expected = '2026-10-01T00:30:00Z' },
        @{ Input = '2026-10-01T00:10:35Z'; Boundary = $true; Expected = '2026-10-01T00:10:00Z' },
        @{ Input = '2026-10-13T00:00:35Z'; Boundary = $false; Expected = '2026-10-13T00:00:00Z' })) {
    $actual = Get-E2EFixtureObservationTime -Now ([DateTimeOffset]::Parse($entry.Input)) -CalendarBoundary $entry.Boundary
    if ($actual -ne [DateTimeOffset]::Parse($entry.Expected).ToUnixTimeSeconds()) { throw 'Fixture clock does not exercise the calendar boundary or preserve normal time.' }
}

$guardEnd = $source.IndexOf('function Resolve-E2EOutputDirectory', [StringComparison]::Ordinal)
if ($guardEnd -lt 0) { throw 'Calendar mode validation boundary is missing.' }
$guard = [ScriptBlock]::Create($source.Substring(0, $guardEnd))
& $guard -Fixture -CalendarOnly
$rejected = $false
try { & $guard -Fixture -CalendarOnly -NarrowPeriodCostFixture }
catch { $rejected = $_.Exception.Message -eq 'Narrow period-cost validation cannot be combined with another specialized E2E mode.' }
if (-not $rejected) { throw 'Calendar must reject a fixture with different totals.' }

$noon = [DateTimeOffset]::Parse('2026-10-12T12:00:00Z').ToUnixTimeSeconds()
$days = Get-E2ECalendarExpectedDays -Now $noon
if ($days.Count -ne 1 -or $days['2026/10/12'].Tokens -ne 10800 -or
    $days['2026/10/12'].Dollars -ne [decimal]10.80) { throw 'Calendar fixture same-day arithmetic differs.' }
$boundary = [DateTimeOffset]::Parse('2026-10-01T00:30:00Z').ToUnixTimeSeconds()
$days = Get-E2ECalendarExpectedDays -Now $boundary
if ($days.Count -ne 2 -or $days['2026/09/30'].Tokens -ne 3600 -or
    $days['2026/09/30'].Dollars -ne [decimal]3.60 -or $days['2026/10/01'].Tokens -ne 7200 -or
    $days['2026/10/01'].Dollars -ne [decimal]7.20) { throw 'Calendar fixture month boundary differs.' }

# Exercise the same producer, converter, H2 listener and qualified client used
# by the native E2E. API-version/fallback assertions are superseded by #690.
. (Join-Path $PSScriptRoot 'E2E-WireContractFixture.ps1')
Initialize-E2EWireContractFixture
& {
    function Get-E2EFixtureObservationTime { return [DateTimeOffset]::Parse('2026-10-01T00:30:00Z').ToUnixTimeSeconds() }
    foreach ($calendar in @($true, $false)) {
        $CalendarOnly = $calendar
        $documents = New-E2EFixtureDocuments
        Start-E2EWireFixture -Documents $documents
        try {
            $periods = @(Read-E2EWireContract '/history/periods?account=account-7&purpose=selectable')
            Assert-E2E ($periods.Count -eq 2) 'Both independently recorded periods must be served.'
            foreach ($period in $periods) {
                Assert-E2E ((@($period.PSObject.Properties.Name | Sort-Object) -join ',') -ceq 'current,end_at,id,label,origin_at,reset_at,start_at') 'Period facts must retain their seven exact fields.'
            }
            if ($calendar) {
                $currentPeriod = @($periods | Where-Object id -eq 'e2e-current')[0]
                $pastPeriod = @($periods | Where-Object id -eq 'e2e-past')[0]
                $origin = [DateTimeOffset]::Parse('2026-10-01T00:00:00Z').ToUnixTimeSeconds()
                $reset = [DateTimeOffset]::Parse('2026-10-01T04:00:00Z').ToUnixTimeSeconds()
                $pastOrigin = [DateTimeOffset]::Parse('2026-09-30T22:50:00Z').ToUnixTimeSeconds()
                Assert-E2E ($currentPeriod.origin_at -eq $origin -and $currentPeriod.start_at -eq ($boundary - 60) -and $currentPeriod.reset_at -eq $reset -and $pastPeriod.origin_at -eq $pastOrigin) 'Verified origins must precede the first observations on their recorded days.'
                $main = @(Read-E2EWireContract '/state?parts=main')[0]
                Assert-E2E ($main.quota.window_seconds -eq 14400 -and $main.quota.reset_at -eq $reset -and ($main.quota.reset_at - $main.quota.window_seconds) -eq $currentPeriod.origin_at) 'Period origin must agree with the official recorded quota window.'
            } else {
                foreach ($period in $periods) { Assert-E2E ($null -eq $period.origin_at) 'Unverified origins must remain unknown.' }
            }
        } finally { [CodexInfoWindowsE2EFixtureServer]::Stop() }
        # Each fixture instance owns immutable control revision files.
        $script:e2eOutput = Join-Path $OutputDirectory ('wire-contract-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $script:e2eOutput | Out-Null
    }
}
Write-Host 'windows-calendar-contract: PASS production H2 fixture, exact origins, quota and unknown-origin preservation'

$start = [datetime]'2026-10-01'
$today = [datetime]'2026-10-03'
$expected = @{
    '2026/10/01' = @{ Tokens = 3000000; Dollars = [decimal]7.50 }
    '2026/10/03' = @{ Tokens = 0; Dollars = [decimal]0 }
}
foreach ($metric in @('Tokens', 'Dollars')) {
    $value = if ($metric -eq 'Tokens') { '3 M' } else { '$7.50' }
    $cells = @{ '2026/10/01' = @('1', $value, 'Recorded part'); '2026/10/04' = @('4') }
    Assert-E2ECalendarVisibleValues -Cells $cells -Expected $expected -Today $today -Metric $metric
    $cells['2026/10/01'] = @('1', '999', 'Recorded part')
    $rejected = $false
    try { Assert-E2ECalendarVisibleValues -Cells $cells -Expected $expected -Today $today -Metric $metric }
    catch { $rejected = $true }
    if (-not $rejected) { throw "Calendar visible $metric value must be checked independently of its accessible name." }
}
$cases = @('valid', 'wrong-total', 'missing-day', 'duplicate-day', 'future-value', 'unmeasured-zero', 'missing-unit')
foreach ($metric in @('Tokens', 'Dollars')) {
    foreach ($case in $cases) {
        $amount = if ($metric -eq 'Tokens') { '3,000,000 Tokens' } else { '$7.50' }
        $zero = if ($metric -eq 'Tokens') { '0 Tokens' } else { '$0.00' }
        $names = @("2026/10/01 $amount Recorded part", ('2026/10/02 ' + [char]0x2014 + ' Unmeasured'), "2026/10/03 $zero Recorded part")
        for ($day = 4; $day -le 31; $day++) { $names += '2026/10/{0:00}' -f $day }
        switch ($case) {
            'wrong-total' { $names[0] = '2026/10/01 999 Recorded part' }
            'missing-day' { $names = @($names | Select-Object -Skip 1) }
            'duplicate-day' { $names += $names[0] }
            'future-value' { $names[3] += ' 0' }
            'unmeasured-zero' { $names[1] = "2026/10/02 $zero Recorded part" }
            'missing-unit' { $names[0] = if ($metric -eq 'Tokens') { '2026/10/01 3,000,000 Recorded part' } else { '2026/10/01 7.50 Recorded part' } }
        }
        $failure = $null
        try { Assert-E2ECalendarDays -Names $names -Expected $expected -Month $start -Today $today -Metric $metric }
        catch { $failure = $_.Exception.Message }
        if (($null -ne $failure) -ne ($case -ne 'valid')) { throw "Calendar oracle $metric/$case failed: $failure" }
        Write-Host "windows-calendar-contract: PASS $metric/$case"
    }
}
Write-Host 'windows-calendar-contract: PASS clock, visible values, daily totals and mode isolation'
