# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
# Test-only file IPC. The HTTP host exposes exactly the five UI APIs.
if (-not ('CodexInfoWindowsE2EFixtureServer' -as [type])) {
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;

public static class CodexInfoWindowsE2EFixtureServer {
    private static Process process;
    private static string controlPath;
    private static string statePath;
    private static string sourcePath;
    private static string sourceFile;
    private static string outputPath;
    private static bool recorderAvailable = true;
    private static bool currentUnavailable;
    private static bool authenticationRequired;
    private static bool partialModels;
    private static bool emptyThreads;
    private static long revision;
    public static bool RecorderVersionAvailable { get { return recorderAvailable; } set { recorderAvailable = value; Publish(); } }
    public static bool CurrentUnavailable { get { return currentUnavailable; } set { currentUnavailable = value; Publish(); } }
    public static bool AuthenticationRequired { get { return authenticationRequired; } set { authenticationRequired = value; Publish(); } }
    public static bool PartialModels { get { return partialModels; } set { partialModels = value; Publish(); } }
    public static bool EmptyThreads { get { return emptyThreads; } set { emptyThreads = value; Publish(); } }
    public static int PartialMainRequests { get { return StateNumber("partial_main_requests"); } }
    public static int OldRouteRequests { get { return StateNumber("old_route_requests"); } }
    public static string SourcePath { get { return sourcePath; } }
    public static string HostPath { get; private set; }
    public static bool Start(string source, string host, string directory, int port) {
        if (IsRunning()) return false;
        sourcePath = source;
        sourceFile = Path.GetFileName(source);
        HostPath = host;
        outputPath = directory;
        controlPath = Path.Combine(directory, "fixture-control.json");
        statePath = Path.Combine(directory, "fixture-host-state.jsonl");
        recorderAvailable = true;
        currentUnavailable = authenticationRequired = partialModels = emptyThreads = false;
        revision = 0;
        Publish();
        if (File.Exists(statePath)) throw new InvalidOperationException("Fixture state path already exists.");
        var start = new ProcessStartInfo(host,
            "--source \"" + source + "\" --control \"" + controlPath + "\" --state \"" + statePath + "\" --port " + port) {
            UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardOutput = true, RedirectStandardError = true,
            WorkingDirectory = directory
        };
        process = new Process { StartInfo = start };
        process.OutputDataReceived += LogOutput;
        process.ErrorDataReceived += LogError;
        if (!process.Start()) return false;
        process.BeginOutputReadLine(); process.BeginErrorReadLine();
        var deadline = Stopwatch.StartNew();
        while (deadline.Elapsed < TimeSpan.FromSeconds(10)) {
            if (process.HasExited) throw new InvalidOperationException("Fixture host exited; inspect fixture-host-stderr.log.");
            if (BoundPort() > 0) return true;
            Thread.Sleep(20); // Local process readiness; never an API poll.
        }
        Stop(); return false;
    }
    private static void LogOutput(object sender, DataReceivedEventArgs data) { if (data.Data != null) lock (outputPath) File.AppendAllText(Path.Combine(outputPath, "fixture-host-stdout.log"), data.Data + Environment.NewLine); }
    private static void LogError(object sender, DataReceivedEventArgs data) { if (data.Data != null) lock (outputPath) File.AppendAllText(Path.Combine(outputPath, "fixture-host-stderr.log"), data.Data + Environment.NewLine); }
    public static void AtomicWrite(string path, string text) {
        if (path == sourcePath && IsRunning()) {
            var name = "fixture-source-" + Guid.NewGuid().ToString("N") + ".json";
            WritePublished(Path.Combine(outputPath, name), text);
            sourceFile = name;
            return;
        }
        var temporary = path + ".new";
        File.WriteAllText(temporary, text, new UTF8Encoding(false));
        if (File.Exists(path)) File.Replace(temporary, path, null);
        else File.Move(temporary, path);
    }
    private static void WritePublished(string path, string text) {
        var temporary = path + ".new";
        var bytes = new UTF8Encoding(false).GetBytes(text);
        using (var output = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.Read)) {
            output.Write(bytes, 0, bytes.Length);
            output.Flush(true);
        }
        File.Move(temporary, path);
    }
    public static void Publish() {
        if (controlPath == null) return;
        var next = ++revision;
        var published = Path.Combine(outputPath, "fixture-control-" + next.ToString("D20") + ".json");
        WritePublished(published, String.Format(
            "{{\"revision\":{0},\"recorder_available\":{1},\"current_unavailable\":{2},\"authentication_required\":{3},\"partial_models\":{4},\"empty_threads\":{5},\"source_file\":\"{6}\"}}",
            next, recorderAvailable.ToString().ToLowerInvariant(), currentUnavailable.ToString().ToLowerInvariant(),
            authenticationRequired.ToString().ToLowerInvariant(), partialModels.ToString().ToLowerInvariant(), emptyThreads.ToString().ToLowerInvariant(), sourceFile));
        if (IsRunning()) {
            var deadline = Stopwatch.StartNew();
            while (deadline.Elapsed < TimeSpan.FromSeconds(3)) {
                if (StateNumber("applied_revision") >= revision) return;
                if (process.HasExited) throw new InvalidOperationException("Fixture host exited during control update.");
                Thread.Sleep(10);
            }
            throw new TimeoutException("Fixture source change was not acknowledged.");
        }
    }
    private static int StateNumber(string name) {
        if (statePath == null || !File.Exists(statePath)) return 0;
        var match = Regex.Match(LatestState(), "\"" + name + "\"\\s*:\\s*([0-9]+)");
        return match.Success ? Int32.Parse(match.Groups[1].Value) : 0;
    }
    private static string ReadShared(string path) {
        using (var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete)) {
            var bytes = new byte[checked((int)stream.Length)];
            var used = 0;
            while (used < bytes.Length) { var count = stream.Read(bytes, used, bytes.Length - used); if (count == 0) break; used += count; }
            return Encoding.UTF8.GetString(bytes, 0, used);
        }
    }
    private static string LatestState() {
        var text = ReadShared(statePath);
        var end = text.LastIndexOf('\n');
        if (end < 0) return "{}";
        var begin = end == 0 ? 0 : text.LastIndexOf('\n', end - 1) + 1;
        return text.Substring(begin, end - begin);
    }
    public static int BoundPort() { return StateNumber("port"); }
    public static bool IsRunning() { return process != null && !process.HasExited; }
    public static string RequestSummary() { return statePath != null && File.Exists(statePath) ? LatestState() : "not-started"; }
    public static void Stop() {
        if (process == null) return;
        if (!process.HasExited) { process.Kill(); process.WaitForExit(5000); }
        process.Dispose(); process = null;
    }
}
'@
}

function Resolve-E2EFixtureHostPath {
    $path = if ([string]::IsNullOrWhiteSpace($FixtureHostPath)) {
        Join-Path $script:e2eRepositoryRoot 'windows-client/tests/CodexInfo.WindowsClient.E2E.FixtureHost/bin/Release/net10.0/CodexInfo.WindowsClient.E2E.FixtureHost.exe'
    } else { [IO.Path]::GetFullPath($FixtureHostPath) }
    Assert-E2E (Test-Path -LiteralPath $path -PathType Leaf) "Build the HTTP/2 fixture host first or supply -FixtureHostPath: $path"
    return $path
}

function ConvertTo-E2EWireSource {
    param([Parameter(Mandatory = $true)][psobject]$Documents)
    $current = ConvertFrom-Json -InputObject ([string]$Documents.Current)
    $accounts = ConvertFrom-Json -InputObject ([string]$Documents.Accounts)
    $threadBody = if ($null -ne $Documents.PSObject.Properties['Threads']) { $Documents.Threads } else { $script:e2eFixtureBaseDocuments.Threads }
    $threads = @((ConvertFrom-Json -InputObject ([string]$threadBody)).threads)
    foreach ($thread in $threads) {
        if ($null -eq $thread.PSObject.Properties['activity_status']) {
            Add-Member -InputObject $thread -MemberType NoteProperty -Name activity_status -Value 'running'
        }
    }
    $counts = @($threads | Group-Object model | Sort-Object Name | ForEach-Object {
        [ordered]@{ model = $_.Name; open_count = $_.Count; running_count = @($_.Group | Where-Object activity_status -eq 'running').Count; unknown_activity_count = @($_.Group | Where-Object activity_status -eq 'unknown').Count }
    })
    foreach ($model in $current.models) {
        if ($null -ne $model.estimated_cost -and $null -ne $model.estimated_cost.PSObject.Properties['price_version']) {
            Add-Member -InputObject $model.estimated_cost -MemberType NoteProperty -Name price_revision -Value $model.estimated_cost.price_version
            $model.estimated_cost.PSObject.Properties.Remove('price_version')
        }
    }
    $owner = @($accounts.accounts | Where-Object id -eq $accounts.default_account_id)[0]
    $main = [ordered]@{
        acquisition_state = $current.state; selection = [ordered]@{ account_id = $owner.id; login_id = $owner.login_id; is_current = [bool]$owner.is_current }
        observed_at = $current.observed_at; authenticated = [bool]$current.authenticated; plan_label = $current.plan_label
        quota = $current.quota; models = @($current.models); models_status = 'complete'
        thread_summary = [ordered]@{ open_count = $threads.Count; running_count = @($threads | Where-Object activity_status -eq 'running').Count; unknown_activity_count = @($threads | Where-Object activity_status -eq 'unknown').Count; by_model = $counts }
    }
    $periodBody = if ($null -ne $Documents.PSObject.Properties['PeriodsWithOrigin'] -and -not [string]::IsNullOrWhiteSpace([string]$Documents.PeriodsWithOrigin)) { $Documents.PeriodsWithOrigin } else { $Documents.Periods }
    $periods = @((ConvertFrom-Json -InputObject ([string]$periodBody)).history_periods)
    foreach ($period in $periods) { if ($null -eq $period.PSObject.Properties['origin_at']) { Add-Member -InputObject $period -MemberType NoteProperty -Name origin_at -Value $null } }
    $histories = [ordered]@{}
    foreach ($entry in @(@{ Id = 'e2e-current'; Property = 'CurrentHistory' }, @{ Id = 'e2e-past'; Property = 'PastHistory' }, @{ Id = 'e2e-oldest'; Property = 'OldestHistory' })) {
        if ($null -eq $Documents.PSObject.Properties[$entry.Property]) { continue }
        $page = ConvertFrom-Json -InputObject ([string]$Documents.PSObject.Properties[$entry.Property].Value)
        $histories[$entry.Id] = [ordered]@{ samples = @($page.history_samples); gaps = @($page.history_gaps) }
    }
    # Known account dates do not establish a complete continuous ownership
    # interval. Preserve only intervals explicitly present in the fixture.
    $intervals = @()
    if ($null -ne $owner.PSObject.Properties['ownership_intervals'] -and $null -ne $owner.ownership_intervals) { $intervals = @($owner.ownership_intervals) }
    return [ordered]@{
        main = $main; account_choices = [ordered]@{ default_account_id = $accounts.default_account_id; accounts = @($accounts.accounts | ForEach-Object { [ordered]@{ id = $_.id; login_id = $_.login_id; is_current = [bool]$_.is_current } }) }
        account_context = [ordered]@{ id = $owner.id; activation_at = $owner.activation_at; deactivation_at = $owner.deactivation_at; ownership_intervals = $intervals }
        periods = $periods; histories = $histories; threads = $threads
    }
}

function Start-E2EWireFixture {
    param([Parameter(Mandatory = $true)][psobject]$Documents)
    $script:e2eFixtureBaseDocuments = $Documents
    $source = ConvertTo-E2EWireSource $Documents
    $path = Join-Path $script:e2eOutput 'fixture-source.json'
    [CodexInfoWindowsE2EFixtureServer]::AtomicWrite($path, ($source | ConvertTo-Json -Depth 40 -Compress))
    Assert-E2E ([CodexInfoWindowsE2EFixtureServer]::Start($path, (Resolve-E2EFixtureHostPath), $script:e2eOutput, 0)) 'Could not bind the HTTP/2 fixture to an ephemeral loopback port.'
    $script:e2eFixturePort = [CodexInfoWindowsE2EFixtureServer]::BoundPort()
    $script:e2eFixtureRunning = $true
}

function Invoke-E2EFixtureOwnershipKnownnessSelfTest {
    param([Parameter(Mandatory = $true)][psobject]$Documents)
    $accounts = ConvertFrom-Json -InputObject ([string]$Documents.Accounts)
    $owner = @($accounts.accounts | Where-Object id -eq $accounts.default_account_id)[0]
    Assert-E2E ($null -eq $owner.PSObject.Properties['ownership_intervals']) 'The absent-ownership fixture must not assert an ownership collection.'
    $source = ConvertTo-E2EWireSource $Documents
    Assert-E2E (@($source.account_context.ownership_intervals).Count -eq 0) 'Missing ownership_intervals must not be invented from activation/deactivation.'
    Assert-E2E ($source.account_context.activation_at -eq $owner.activation_at -and $source.account_context.deactivation_at -eq $owner.deactivation_at) 'Known activation/deactivation must remain unchanged.'

    $copy = $Documents.PSObject.Copy()
    $start = [Int64]$owner.activation_at
    Add-Member -InputObject $owner -MemberType NoteProperty -Name ownership_intervals -Value @(
        [pscustomobject]@{ start_at = $start - 7200; end_at = $start - 3600 },
        [pscustomobject]@{ start_at = $start; end_at = $null }
    )
    $copy.Accounts = $accounts | ConvertTo-Json -Depth 30 -Compress
    $explicit = ConvertTo-E2EWireSource $copy
    $intervals = @($explicit.account_context.ownership_intervals)
    Assert-E2E ($intervals.Count -eq 2 -and $intervals[0].start_at -eq $start - 7200 -and $intervals[0].end_at -eq $start - 3600 -and $intervals[1].start_at -eq $start -and $null -eq $intervals[1].end_at) 'Explicit ownership intervals must retain both exact bounds and null.'
    Write-E2E 'fixture-ownership-knownness-self-test: PASS absent collection stays unasserted; known dates and explicit two-interval facts retained'
}

function Set-E2EWireFixtureScenario {
    param([Parameter(Mandatory = $true)][psobject]$Documents)
    $source = ConvertTo-E2EWireSource $Documents
    [CodexInfoWindowsE2EFixtureServer]::AtomicWrite([CodexInfoWindowsE2EFixtureServer]::SourcePath, ($source | ConvertTo-Json -Depth 40 -Compress))
    [CodexInfoWindowsE2EFixtureServer]::Publish()
}

function Invoke-E2EFixtureRawRequest {
    param([Parameter(Mandatory = $true)][string]$Path)
    Assert-E2E ($Path -cmatch '^/(state\?parts=|history/periods\?|history\?|threads\?)') 'Fixture preflight must use a UI resource.'
    $hostPath = [CodexInfoWindowsE2EFixtureServer]::HostPath
    $safeName = [Guid]::NewGuid().ToString('N')
    $errorPath = Join-Path $script:e2eOutput "fixture-probe-$safeName.stderr.log"
    $body = @(& $hostPath --probe "http://127.0.0.1:$($script:e2eFixturePort)/" --target $Path --cache (Join-Path $script:e2eOutput 'fixture-preflight-cache') 2>$errorPath) -join "`n"
    Assert-E2E ($LASTEXITCODE -eq 0) "Qualified fixture probe failed; inspect $errorPath"
    $snapshot = ConvertFrom-Json -InputObject $body
    Assert-E2EFixtureJsonKeys -Json $snapshot -Expected @('views', 'deliveries') -Endpoint 'Qualified UI snapshot'
    foreach ($view in $snapshot.views) {
        Assert-E2E ([string]$view.id -cmatch '^[0-9a-f]{64}$' -and [string]$view.image -cmatch '^[0-9a-f]{64}$') 'Qualified view identity/image is invalid.'
    }
    return [pscustomobject]@{ Path = $Path; Snapshot = $snapshot; Body = $body }
}

function Get-E2EFixtureRows {
    param([psobject]$Response, [string]$Kind)
    return @($Response.Snapshot.views | ForEach-Object { $_.rows } | Where-Object { $_.key[0] -ceq $Kind })
}

function Get-E2EFixtureSamples {
    param([psobject]$Response)
    return @(Get-E2EFixtureRows $Response 'sample' | ForEach-Object {
        $value = $_.value
        Add-Member -InputObject $value -MemberType NoteProperty -Name reset_at -Value ([Int64]$_.key[2])
        Add-Member -InputObject $value -MemberType NoteProperty -Name timestamp -Value ([Int64]$_.key[3])
        $value
    } | Sort-Object reset_at, timestamp)
}

function Invoke-E2EFixturePreflight {
    $responses = [ordered]@{}
    foreach ($spec in @(
        @{ Name = 'runtime'; Path = '/state?parts=runtime' },
        @{ Name = 'accounts'; Path = '/state?parts=account_choices' },
        @{ Name = 'context'; Path = '/state?parts=account_context&account=account-7' },
        @{ Name = 'current'; Path = '/state?parts=main&account=account-7' },
        @{ Name = 'periods'; Path = '/history/periods?account=account-7&purpose=selectable' },
        @{ Name = 'current-history'; Path = '/history?account=account-7&period=e2e-current' },
        @{ Name = 'past-history'; Path = '/history?account=account-7&period=e2e-past' },
        @{ Name = 'threads'; Path = '/threads?account=account-7' })) {
        $response = Invoke-E2EFixtureRawRequest $spec.Path
        $responses[$spec.Name] = $response
        $rawPath = Join-Path $script:e2eOutput ("fixture-preflight-{0}.raw.json" -f $spec.Name)
        [IO.File]::WriteAllText($rawPath, $response.Body, [Text.UTF8Encoding]::new($false))
        Write-E2E ("fixture-preflight: resource={0} complete-views={1} delivery-pages={2} raw={3}" -f $spec.Name, @($response.Snapshot.views).Count, @($response.Snapshot.deliveries).Count, $rawPath)
    }
    Assert-E2EFixtureResources -Responses $responses | Out-Null
    Assert-E2E ([CodexInfoWindowsE2EFixtureServer]::OldRouteRequests -eq 0) 'The fixture must receive no retired routes.'
    Write-E2E 'fixture-preflight: PASS (HTTP/2, five versionless UI APIs, complete qualified images, shared receipt cache)'
    return $responses
}

function Invoke-E2ENarrowPeriodCostFixturePreflight {
    param([Parameter(Mandatory = $true)][psobject]$Scenario)
    $responses = [ordered]@{}
    foreach ($spec in @(
        @{ Name = 'current'; Path = '/state?parts=main&account=account-7' },
        @{ Name = 'periods'; Path = '/history/periods?account=account-7&purpose=selectable' },
        @{ Name = 'oldest-history'; Path = '/history?account=account-7&period=e2e-oldest'; Expected = 274 },
        @{ Name = 'past-history'; Path = '/history?account=account-7&period=e2e-past'; Expected = 300 },
        @{ Name = 'current-history'; Path = '/history?account=account-7&period=e2e-current'; Expected = 106 })) {
        $response = Invoke-E2EFixtureRawRequest $spec.Path
        $responses[$spec.Name] = $response
        if ($spec.ContainsKey('Expected')) {
            $samples = @(Get-E2EFixtureSamples $response)
            Assert-E2E ($samples.Count -eq 2) 'Narrow period history must contain both direct observations.'
            $amount = [decimal]0
            foreach ($model in $samples[-1].models) { $amount += [decimal]$model.total_dollars }
            Assert-E2E ($amount -eq [decimal]$spec.Expected) "Narrow period received total differs: $($spec.Name)=$amount."
        }
    }
    Write-E2E 'narrow-period-cost-fixture-preflight: PASS three qualified histories and exact stored totals $274/$300/$106'
    return $responses
}
