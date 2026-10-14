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

# Exercise the production document producer and the exact server Start call
# used by Enter-E2EFixture, rather than comparing an oracle with itself.
$producer = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'New-E2EFixtureDocuments'
}, $false)
. ([ScriptBlock]::Create($producer.Extent.Text))
$server = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.StringConstantExpressionAst] -and
    $node.Value.Contains('public static class CodexInfoWindowsE2EFixtureServer')
}, $true)
Add-Type -TypeDefinition $server.Value
$enter = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Enter-E2EFixture'
}, $false)
$serverStart = $enter.Find({ param($node)
    $node -is [System.Management.Automation.Language.InvokeMemberExpressionAst] -and
    $node.Static -and $node.Expression.TypeName.FullName -eq 'CodexInfoWindowsE2EFixtureServer' -and
    $node.Member.Value -eq 'Start'
}, $true)
$startFixture = [ScriptBlock]::Create($serverStart.Extent.Text)
$contractServerStart = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.InvokeMemberExpressionAst] -and
    $node.Static -and $node.Expression.TypeName.FullName -eq 'CodexInfoWindowsE2EFixtureServer' -and
    $node.Member.Value -eq 'Start' -and $node.Extent.Text.Contains('$contractDocuments.Details')
}, $true)
$startContractFixture = [ScriptBlock]::Create($contractServerStart.Extent.Text)
foreach ($name in @('e2eFixtureUnusedMinimumSeconds', 'e2eFixturePastIdleStartFraction', 'e2eFixturePastIdleEndFraction')) {
    $assignment = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
        $node.Left.Extent.Text -eq ('$script:' + $name)
    }, $false)
    . ([ScriptBlock]::Create($assignment.Extent.Text))
}
$script:e2eProductVersion = '1.2.3'
& {
    function Get-E2EFixtureObservationTime { return [DateTimeOffset]::Parse('2026-10-01T00:30:00Z').ToUnixTimeSeconds() }
    foreach ($calendar in @($true, $false)) {
        $CalendarOnly = $calendar
        $documents = New-E2EFixtureDocuments
        # The contract-test caller retains its original twelve arguments.
        $contractDocuments = $documents
        $startServer = if ($calendar) { $startFixture } else { $startContractFixture }
        Assert-E2E (& $startServer) 'Calendar contract fixture server could not start.'
        try {
            $endpoint = 'http://127.0.0.1:' + [CodexInfoWindowsE2EFixtureServer]::BoundPort()
            $v4Response = $null
            $v4Status = 0
            try {
                $v4Response = Invoke-WebRequest -UseBasicParsing -Uri "$endpoint/v4/history/periods?account=account-7"
                $v4Status = [int]$v4Response.StatusCode
            }
            catch {
                if ($null -eq $_.Exception.Response) { throw }
                $v4Status = [int]$_.Exception.Response.StatusCode
            }
            if ($calendar) {
                Assert-E2E ($v4Status -eq 200) "Calendar fixture must serve verified v4 period origins; received HTTP $v4Status."
                Assert-E2E ([string]$v4Response.Headers['Codex-Info-Published-Pair'] -ceq $documents.PublishedPair) 'Calendar v4 directory must use the same published pair.'
                $directory = $v4Response.Content | ConvertFrom-Json
                Assert-E2E ($directory.api_version -ceq 'v4' -and $directory.history_periods.Count -eq 2) 'Calendar directory must expose both v4 periods.'
                foreach ($period in $directory.history_periods) {
                    $fields = @($period.PSObject.Properties.Name | Sort-Object) -join ','
                    Assert-E2E ($fields -ceq 'current,end_at,id,label,origin_at,reset_at,start_at') 'Calendar v4 period must retain the exact seven-field contract.'
                }
                $currentPeriod = $directory.history_periods | Where-Object id -eq 'e2e-current'
                $pastPeriod = $directory.history_periods | Where-Object id -eq 'e2e-past'
                $origin = [DateTimeOffset]::Parse('2026-10-01T00:00:00Z').ToUnixTimeSeconds()
                $reset = [DateTimeOffset]::Parse('2026-10-01T04:00:00Z').ToUnixTimeSeconds()
                $pastOrigin = [DateTimeOffset]::Parse('2026-09-30T22:50:00Z').ToUnixTimeSeconds()
                Assert-E2E ($currentPeriod.origin_at -eq $origin -and $currentPeriod.start_at -eq ($boundary - 60) -and
                    $currentPeriod.reset_at -eq $reset -and $pastPeriod.origin_at -eq $pastOrigin) 'Calendar origins must precede the first observations on their respective recorded days.'
                $currentResponse = Invoke-WebRequest -UseBasicParsing -Uri "$endpoint/v3/current?account=account-7"
                $quota = ($currentResponse.Content | ConvertFrom-Json).quota
                Assert-E2E ([string]$currentResponse.Headers['Codex-Info-Published-Pair'] -ceq $documents.PublishedPair -and
                    $quota.window_seconds -eq 14400 -and $quota.reset_at -eq $reset -and
                    ($quota.reset_at - $quota.window_seconds) -eq $currentPeriod.origin_at) 'Calendar period origin must agree with the same-pair official quota window.'
            }
            else {
                Assert-E2E ($v4Status -eq 404) 'The normal fixture must retain its existing v3 fallback path.'
            }
            $v3Response = Invoke-WebRequest -UseBasicParsing -Uri "$endpoint/v3/history/periods?account=account-7"
            $v3Directory = $v3Response.Content | ConvertFrom-Json
            Assert-E2E ($v3Directory.api_version -ceq 'v3') 'The legacy period directory must remain v3.'
            foreach ($period in $v3Directory.history_periods) {
                Assert-E2E (@($period.PSObject.Properties.Name).Count -eq 6 -and
                    $null -eq $period.PSObject.Properties['origin_at']) 'Verified origins must not leak into the six-field v3 wire contract.'
            }
        }
        finally { [CodexInfoWindowsE2EFixtureServer]::Stop() }
    }
}
Write-Host 'windows-calendar-contract: PASS production fixture, v4 origins, official quota and v3 isolation'

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
