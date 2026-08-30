# Runs the finite Windows UI Automation acceptance path against the installed
# client.  The normal mode uses the configured loopback service.  CI may pass
# -Fixture to provide a bounded, local /v1/status + /v1/details pair; this still
# drives the installed EXE and the real rendered windows, but does not require
# an account or an SSH tunnel.
[CmdletBinding()]
param(
    [string]$ClientPath = '',
    [string]$OutputDirectory = '',
    [switch]$Fixture,
    [switch]$FixtureContractTest,
    [string]$SourceSha = ''
)

$ErrorActionPreference = 'Stop'

function Resolve-E2EOutputDirectory {
    param([string]$Requested)

    if (-not [string]::IsNullOrWhiteSpace($Requested)) {
        return [IO.Path]::GetFullPath($Requested)
    }

    # Keep raw logs and screenshots outside the repository.  /mnt/d is the
    # usual WSL hand-off location; the other locations cover hosted Windows
    # runners and local Windows machines without a D: mount.
    if (Test-Path -LiteralPath '/mnt/d' -PathType Container) {
        return [IO.Path]::GetFullPath('/mnt/d/temp/codex-info-windows-e2e')
    }
    if (Test-Path -LiteralPath '/home/salty' -PathType Container) {
        return [IO.Path]::GetFullPath('/home/salty/.cache/codex-info-windows-e2e')
    }
    if (-not [string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
        return [IO.Path]::GetFullPath((Join-Path $env:RUNNER_TEMP 'codex-info-windows-e2e'))
    }
    return [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) 'codex-info-windows-e2e'))
}

$script:e2eOutput = Resolve-E2EOutputDirectory $OutputDirectory
$script:e2eRepositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..')).TrimEnd([char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar))
if ($script:e2eOutput.Equals($script:e2eRepositoryRoot, [StringComparison]::OrdinalIgnoreCase) -or
    $script:e2eOutput.StartsWith($script:e2eRepositoryRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'E2E artifacts must be written outside the repository.'
}
if (Test-Path -LiteralPath $script:e2eOutput -PathType Container) {
    # Evidence must belong to this invocation.  Never append to or reuse a
    # prior run's log/screenshots, even when a hosted runner reuses its temp
    # directory after a retry.
    Remove-Item -LiteralPath $script:e2eOutput -Recurse -Force
}
New-Item -ItemType Directory -Path $script:e2eOutput -Force | Out-Null
$script:e2eLogPath = Join-Path $script:e2eOutput 'windows-client-e2e.log'
$script:e2eSourceSha = if (-not [string]::IsNullOrWhiteSpace($SourceSha)) { $SourceSha } elseif (-not [string]::IsNullOrWhiteSpace($env:GITHUB_SHA)) { $env:GITHUB_SHA } else { 'unknown' }
$script:e2eWindowRecords = [System.Collections.Generic.List[object]]::new()
$script:e2eProcess = $null
$script:e2eFixtureRunning = $false
$script:e2eFixturePort = 8787
$script:e2ePreviewEnabled = -not [string]::IsNullOrWhiteSpace($env:CODEX_INFO_WINDOWS_PREVIEW)
$script:e2eSettingsPath = Join-Path $env:LOCALAPPDATA 'CodexInfo\settings.json'
$script:e2eSettingsBackup = Join-Path ([IO.Path]::GetTempPath()) ("codex-info-e2e-settings-" + [Guid]::NewGuid().ToString('N') + '.json')
$script:e2eSettingsWasPresent = $false

function Write-E2E {
    param([Parameter(Mandatory = $true)][string]$Message)

    $line = "{0} {1}" -f ([DateTimeOffset]::Now.ToString('o')), $Message
    Add-Content -LiteralPath $script:e2eLogPath -Value $line -Encoding utf8
    # Host output keeps helper return values (UIA elements, screenshots, and
    # records) out of the PowerShell pipeline while still exposing the raw
    # line in an interactive/CI log.
    Write-Host $line
}

function Assert-E2E {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )

    if (-not $Condition) {
        throw "ASSERT: $Message"
    }
}

function Wait-E2E {
    param(
        [Parameter(Mandatory = $true)][scriptblock]$Probe,
        [Parameter(Mandatory = $true)][string]$Description,
        [int]$TimeoutSeconds = 30
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $value = & $Probe
            if ($null -ne $value -and [bool]$value) {
                return $value
            }
        }
        catch {
            # UIA elements can be replaced while Avalonia paints a new state.
            # Re-querying the same finite target is safe; a timeout is still a
            # hard failure and is never reported as a skip/pass.
        }
        Start-Sleep -Milliseconds 200
    }
    throw "TIMEOUT: $Description (${TimeoutSeconds}s)"
}

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing

$compilerReferenceRoot = Join-Path $PSHOME 'ref'
$compilerReferences = @(Get-ChildItem -LiteralPath $compilerReferenceRoot -Filter '*.dll' -File |
    Select-Object -ExpandProperty FullName)
$runtimeReferences = @(
    [System.Drawing.Bitmap].Assembly.Location
    ([System.Reflection.Assembly]::Load('System.Private.Windows.GdiPlus')).Location
    ([System.Reflection.Assembly]::Load('System.Private.Windows.Core')).Location
)
Assert-E2E ($compilerReferences.Count -gt 0) 'PowerShell compiler references could not be resolved.'
Assert-E2E (@($runtimeReferences | Where-Object { -not (Test-Path -LiteralPath $_ -PathType Leaf) }).Count -eq 0) 'Windows drawing runtime references could not be resolved.'
$compilerReferences += $runtimeReferences

Add-Type -ReferencedAssemblies $compilerReferences -TypeDefinition @'
using System;
using System.Drawing;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class CodexInfoWindowsE2EWin32 {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool IsWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int command);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int x, int y, int cx, int cy, uint flags);
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr extra);
    [StructLayout(LayoutKind.Sequential)] public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }
}

public sealed class CodexInfoGraphPixelMeasurement {
    public int PeriodStartX { get; set; }
    public int PeriodEndX { get; set; }
    public int PlotSpan { get; set; }
    public int GutterWidth { get; set; }
    public int[] GridCenters { get; set; }
    public int[] SeriesGutterTop { get; set; }
    public int[] SeriesGutterBottom { get; set; }
    public int[] SeriesPixelCount { get; set; }
    public int[] SeriesGutterPixelCount { get; set; }
    public int[] SeriesRightmost { get; set; }
}

public static class CodexInfoGraphPixelScanner {
    private static readonly Color GridColor = ColorTranslator.FromHtml("#263548");
    private static readonly Color[] SeriesColors = new[] {
        ColorTranslator.FromHtml("#56B2F5"),
        ColorTranslator.FromHtml("#A88CF5"),
        ColorTranslator.FromHtml("#5DC98A"),
        ColorTranslator.FromHtml("#E6A23C"),
    };

    public static CodexInfoGraphPixelMeasurement Scan(
        string path,
        int plotLeft,
        int plotTop,
        int plotWidth,
        int plotHeight) {
        using (var bitmap = new Bitmap(path)) {
            if (plotLeft < 0 || plotTop < 0 || plotWidth <= 0 || plotHeight <= 40 ||
                plotLeft + plotWidth > bitmap.Width || plotTop + plotHeight > bitmap.Height) {
                throw new InvalidOperationException("Graph plot bounds are outside the capture.");
            }

            int yStart = plotTop + 20;
            int yEnd = plotTop + plotHeight - 20;
            int sampledHeight = yEnd - yStart;
            int requiredGridPixels = (int)Math.Ceiling(sampledHeight * 0.40);
            var gridColumns = new bool[plotWidth];
            for (int localX = 0; localX < plotWidth; localX++) {
                int matches = 0;
                for (int y = yStart; y < yEnd; y++) {
                    if (Matches(bitmap.GetPixel(plotLeft + localX, y), GridColor, 8)) matches++;
                }
                gridColumns[localX] = matches >= requiredGridPixels;
            }

            var centers = new List<int>();
            int runStart = -1;
            for (int x = 0; x <= plotWidth; x++) {
                bool isGrid = x < plotWidth && gridColumns[x];
                if (isGrid && runStart < 0) runStart = x;
                if (!isGrid && runStart >= 0) {
                    centers.Add((runStart + x - 1) / 2);
                    runStart = -1;
                }
            }
            if (centers.Count < 4) {
                throw new InvalidOperationException("Fewer than four visible vertical period-grid groups were detected.");
            }

            int bestStart = -1;
            int visiblePeriodGridCount = 0;
            double bestScore = double.PositiveInfinity;
            foreach (int candidateCount in new[] { 5, 4 }) {
                if (centers.Count < candidateCount) continue;
                int candidateIntervals = candidateCount - 1;
                for (int start = 0; start <= centers.Count - candidateCount; start++) {
                    double average = (centers[start + candidateIntervals] - centers[start]) / (double)candidateIntervals;
                    double score = 0;
                    for (int index = 0; index < candidateIntervals; index++) {
                        score = Math.Max(score, Math.Abs((centers[start + index + 1] - centers[start + index]) - average));
                    }
                    if (score < bestScore) {
                        bestScore = score;
                        bestStart = start;
                        visiblePeriodGridCount = candidateCount;
                    }
                }
            }
            if (bestStart < 0 || bestScore > 3) {
                throw new InvalidOperationException(
                    "Equally spaced period-grid groups were not detected: " + string.Join(",", centers));
            }

            int periodStart = centers[bestStart];
            int intervalCount = visiblePeriodGridCount - 1;
            double periodStep = (centers[bestStart + intervalCount] - periodStart) / (double)intervalCount;
            int periodEnd = visiblePeriodGridCount == 5
                ? centers[bestStart + 4]
                : (int)Math.Round(periodStart + 4 * periodStep);
            if (periodEnd <= periodStart || periodEnd >= plotWidth) {
                throw new InvalidOperationException("The inferred period-end grid is outside the plot.");
            }
            if (visiblePeriodGridCount == 4) {
                int endpointEvidence = 0;
                for (int y = yStart; y < yEnd; y++) {
                    bool rowMatches = false;
                    for (int localX = Math.Max(0, periodEnd - 3);
                        localX <= Math.Min(plotWidth - 1, periodEnd + 3) && !rowMatches;
                        localX++) {
                        Color pixel = bitmap.GetPixel(plotLeft + localX, y);
                        rowMatches = Matches(pixel, GridColor, 8);
                        for (int series = 0; series < SeriesColors.Length && !rowMatches; series++) {
                            rowMatches = Matches(pixel, SeriesColors[series], 24);
                        }
                    }
                    if (rowMatches) endpointEvidence++;
                }
                if (endpointEvidence < requiredGridPixels) {
                    throw new InvalidOperationException("The inferred period-end grid has no vertical grid/series evidence.");
                }
            }
            var gutterTop = new[] { int.MaxValue, int.MaxValue, int.MaxValue, int.MaxValue };
            var gutterBottom = new[] { int.MinValue, int.MinValue, int.MinValue, int.MinValue };
            var count = new int[4];
            var gutterCount = new int[4];
            var rightmost = new[] { int.MinValue, int.MinValue, int.MinValue, int.MinValue };
            for (int localX = 0; localX < plotWidth; localX++) {
                for (int localY = 0; localY < plotHeight; localY++) {
                    Color pixel = bitmap.GetPixel(plotLeft + localX, plotTop + localY);
                    for (int series = 0; series < SeriesColors.Length; series++) {
                        if (!Matches(pixel, SeriesColors[series], 24)) continue;
                        count[series]++;
                        rightmost[series] = Math.Max(rightmost[series], localX);
                        if (localX > periodEnd) {
                            gutterCount[series]++;
                            gutterTop[series] = Math.Min(gutterTop[series], localY);
                            gutterBottom[series] = Math.Max(gutterBottom[series], localY);
                        }
                    }
                }
            }

            return new CodexInfoGraphPixelMeasurement {
                PeriodStartX = periodStart,
                PeriodEndX = periodEnd,
                PlotSpan = periodEnd - periodStart,
                GutterWidth = plotWidth - 1 - periodEnd,
                GridCenters = centers.ToArray(),
                SeriesGutterTop = gutterTop,
                SeriesGutterBottom = gutterBottom,
                SeriesPixelCount = count,
                SeriesGutterPixelCount = gutterCount,
                SeriesRightmost = rightmost,
            };
        }
    }

    private static bool Matches(Color actual, Color expected, int tolerance) {
        return Math.Abs(actual.R - expected.R) <= tolerance &&
            Math.Abs(actual.G - expected.G) <= tolerance &&
            Math.Abs(actual.B - expected.B) <= tolerance;
    }
}

public static class CodexInfoWindowsE2ECaptureTestWindow {
    private const uint WS_POPUP = 0x80000000;
    private const uint WS_VISIBLE = 0x10000000;
    private const uint WS_EX_TOOLWINDOW = 0x00000080;
    private const uint CS_HREDRAW = 0x00000002;
    private const uint CS_VREDRAW = 0x00000001;
    private const uint WM_PAINT = 0x000F;
    private const uint WM_PRINT = 0x0317;
    private const uint WM_PRINTCLIENT = 0x0318;
    private const int ERROR_CLASS_ALREADY_EXISTS = 1410;
    private static readonly string TargetClassName = "CodexInfoWindowsE2ECaptureSelfTestTarget";
    private static readonly string DecoyClassName = "CodexInfoWindowsE2ECaptureSelfTestDecoy";
    private static readonly WindowProc Callback = WindowProcedure;
    private static bool targetClassRegistered;
    private static bool decoyClassRegistered;

    private delegate IntPtr WindowProc(IntPtr hWnd, uint message, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct WNDCLASS {
        public uint style;
        public WindowProc lpfnWndProc;
        public int cbClsExtra;
        public int cbWndExtra;
        public IntPtr hInstance;
        public IntPtr hIcon;
        public IntPtr hCursor;
        public IntPtr hbrBackground;
        public string lpszMenuName;
        public string lpszClassName;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct PAINTSTRUCT {
        public IntPtr hdc;
        public bool fErase;
        public RECT rcPaint;
        public bool fRestore;
        public bool fIncUpdate;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 32)] public byte[] rgbReserved;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr GetModuleHandle(string moduleName);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern ushort RegisterClass(ref WNDCLASS windowClass);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateWindowEx(
        uint extendedStyle,
        string className,
        string windowName,
        uint style,
        int x,
        int y,
        int width,
        int height,
        IntPtr parent,
        IntPtr menu,
        IntPtr instance,
        IntPtr parameter);

    [DllImport("user32.dll", SetLastError = true)] private static extern bool DestroyWindow(IntPtr hWnd);
    [DllImport("user32.dll")] private static extern bool UpdateWindow(IntPtr hWnd);
    [DllImport("user32.dll")] private static extern IntPtr DefWindowProc(IntPtr hWnd, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern IntPtr BeginPaint(IntPtr hWnd, out PAINTSTRUCT paint);
    [DllImport("user32.dll")] private static extern bool EndPaint(IntPtr hWnd, ref PAINTSTRUCT paint);
    [DllImport("user32.dll")] private static extern bool GetClientRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr hWnd, StringBuilder className, int count);
    [DllImport("gdi32.dll")] private static extern IntPtr CreateSolidBrush(uint colorRef);
    [DllImport("gdi32.dll")] private static extern bool DeleteObject(IntPtr handle);
    [DllImport("user32.dll")] private static extern int FillRect(IntPtr hdc, ref RECT rect, IntPtr brush);

    private static void EnsureClass(string className, bool target) {
        if (target ? targetClassRegistered : decoyClassRegistered) return;
        var windowClass = new WNDCLASS {
            style = CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc = Callback,
            hInstance = GetModuleHandle(null),
            lpszClassName = className
        };
        var atom = RegisterClass(ref windowClass);
        if (atom == 0 && Marshal.GetLastWin32Error() != ERROR_CLASS_ALREADY_EXISTS) {
            throw new InvalidOperationException("Capture self-test RegisterClass failed: " + Marshal.GetLastWin32Error());
        }
        if (target) targetClassRegistered = true;
        else decoyClassRegistered = true;
    }

    public static IntPtr Create(string title, int x, int y, int width, int height) {
        var target = title.StartsWith("TARGET", StringComparison.Ordinal);
        var className = target ? TargetClassName : DecoyClassName;
        EnsureClass(className, target);
        return CreateWindowEx(
            WS_EX_TOOLWINDOW,
            className,
            title,
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            width,
            height,
            IntPtr.Zero,
            IntPtr.Zero,
            GetModuleHandle(null),
            IntPtr.Zero);
    }

    public static void Show(IntPtr hWnd) {
        if (hWnd == IntPtr.Zero) return;
        CodexInfoWindowsE2EWin32.ShowWindow(hWnd, 5);
        UpdateWindow(hWnd);
    }

    public static void Destroy(IntPtr hWnd) {
        if (hWnd != IntPtr.Zero) DestroyWindow(hWnd);
    }

    private static IntPtr WindowProcedure(IntPtr hWnd, uint message, IntPtr wParam, IntPtr lParam) {
        if (message == WM_PAINT) {
            PAINTSTRUCT paint;
            var hdc = BeginPaint(hWnd, out paint);
            Paint(hWnd, hdc);
            EndPaint(hWnd, ref paint);
            return IntPtr.Zero;
        }
        if (message == WM_PRINT || message == WM_PRINTCLIENT) {
            Paint(hWnd, wParam);
            return IntPtr.Zero;
        }
        return DefWindowProc(hWnd, message, wParam, lParam);
    }

    private static void Paint(IntPtr hWnd, IntPtr hdc) {
        if (hdc == IntPtr.Zero) return;
        RECT rect;
        if (!GetClientRect(hWnd, out rect)) return;
        var className = new StringBuilder(96);
        GetClassName(hWnd, className, className.Capacity);
        // COLORREF is 0x00BBGGRR. The target class is blue and the decoy
        // class is red so the self-test distinguishes requested HWND pixels.
        var colorRef = className.ToString().Equals(TargetClassName, StringComparison.Ordinal)
            ? 0x00C06020u
            : 0x000000C0u;
        var brush = CreateSolidBrush(colorRef);
        if (brush != IntPtr.Zero) {
            FillRect(hdc, ref rect, brush);
            DeleteObject(brush);
        }
    }
}
'@

if ($Fixture -or $FixtureContractTest) {
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;

public static class CodexInfoWindowsE2EFixtureServer {
    private static TcpListener listener;
    private static Thread worker;
    private static volatile bool running;
    private static string statusBody;
    private static string detailsBody;
    private static string publishedPair;
    private static int healthRequests;
    private static int statusRequests;
    private static int detailsRequests;
    private static int preflightRequests;
    private static int clientRequests;

    public static bool Start(string status, string details, string pair, int port) {
        if (running) return false;
        try {
            statusBody = status;
            detailsBody = details;
            publishedPair = pair;
            healthRequests = 0;
            statusRequests = 0;
            detailsRequests = 0;
            preflightRequests = 0;
            clientRequests = 0;
            listener = new TcpListener(IPAddress.Loopback, port);
            listener.Start();
            running = true;
            worker = new Thread(Loop) { IsBackground = true, Name = "CodexInfoWindowsE2EFixture" };
            worker.Start();
            return true;
        }
        catch (SocketException) {
            running = false;
            try { if (listener != null) listener.Stop(); } catch { }
            listener = null;
            return false;
        }
    }

    public static bool IsRunning() { return running; }

    public static int BoundPort() {
        return listener == null ? 0 : ((IPEndPoint)listener.LocalEndpoint).Port;
    }

    public static string RequestSummary() {
        return String.Format(
            "health={0} status={1} details={2} preflight={3} client={4}",
            healthRequests, statusRequests, detailsRequests, preflightRequests, clientRequests);
    }

    public static void Stop() {
        running = false;
        try { if (listener != null) listener.Stop(); } catch { }
        try { if (worker != null) worker.Join(1000); } catch { }
        listener = null;
        worker = null;
    }

    private static void Loop() {
        while (running) {
            TcpClient client = null;
            try {
                client = listener.AcceptTcpClient();
                Handle(client);
            }
            catch (SocketException) {
                if (running) { }
            }
            catch (ObjectDisposedException) {
                if (running) { }
            }
            catch { }
            finally {
                try { if (client != null) client.Close(); } catch { }
            }
        }
    }

    private static void Handle(TcpClient client) {
        using (var stream = client.GetStream()) {
            var buffer = new byte[8192];
            var used = 0;
            while (used < buffer.Length) {
                var read = stream.Read(buffer, used, buffer.Length - used);
                if (read <= 0) break;
                used += read;
                if (used >= 4 &&
                    buffer[used - 4] == 13 && buffer[used - 3] == 10 &&
                    buffer[used - 2] == 13 && buffer[used - 1] == 10) break;
            }
            var request = Encoding.ASCII.GetString(buffer, 0, used);
            var firstLine = request.Split(new[] { "\r\n" }, StringSplitOptions.None)[0];
            var parts = firstLine.Split(new[] { ' ' }, StringSplitOptions.RemoveEmptyEntries);
            var code = 404;
            var reason = "Not Found";
            var body = "{\"api_version\":\"v1\",\"error\":\"not_found\"}";
            var includePublishedPair = false;
            if (parts.Length >= 2 && parts[0] == "GET") {
                if (parts[1] == "/v1/status") {
                    Interlocked.Increment(ref statusRequests);
                    RecordRequestPhase(request);
                    code = 200;
                    reason = "OK";
                    body = statusBody;
                    includePublishedPair = true;
                }
                else if (parts[1] == "/v1/health") {
                    Interlocked.Increment(ref healthRequests);
                    RecordRequestPhase(request);
                    code = 200;
                    reason = "OK";
                    body = "{\"api_version\":\"v1\",\"service\":\"codex-info\"}";
                }
                else if (parts[1] == "/v1/details") {
                    Interlocked.Increment(ref detailsRequests);
                    RecordRequestPhase(request);
                    code = 200;
                    reason = "OK";
                    body = detailsBody;
                    includePublishedPair = true;
                }
            }
            else if (parts.Length >= 2) {
                code = 405;
                reason = "Method Not Allowed";
                body = "{\"api_version\":\"v1\",\"error\":\"method_not_allowed\"}";
            }
            var payload = Encoding.UTF8.GetBytes(body);
            var header = String.Format(
                "HTTP/1.1 {0} {1}\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {2}\r\nConnection: close\r\n",
                code, reason, payload.Length);
            if (includePublishedPair) {
                header += "Codex-Info-Published-Pair: " + publishedPair + "\r\n";
            }
            header += "\r\n";
            var headerBytes = Encoding.ASCII.GetBytes(header);
            stream.Write(headerBytes, 0, headerBytes.Length);
            stream.Write(payload, 0, payload.Length);
            stream.Flush();
        }
    }

    private static void RecordRequestPhase(string request) {
        if (request.IndexOf("X-Codex-Info-E2E-Phase: preflight", StringComparison.OrdinalIgnoreCase) >= 0) {
            Interlocked.Increment(ref preflightRequests);
        }
        else {
            Interlocked.Increment(ref clientRequests);
        }
    }
}
'@
}

[CodexInfoWindowsE2EWin32]::SetProcessDPIAware() | Out-Null

function Find-E2EWindow {
    param(
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [Parameter(Mandatory = $true)][string]$TitleFragment
    )

    $script:e2eSearchProcessId = [uint32]$ProcessId
    $script:e2eSearchTitle = $TitleFragment
    $script:e2eSearchWindow = [IntPtr]::Zero
    $callback = [CodexInfoWindowsE2EWin32+EnumWindowsProc] {
        param([IntPtr]$Handle, [IntPtr]$Extra)
        [uint32]$owner = 0
        [CodexInfoWindowsE2EWin32]::GetWindowThreadProcessId($Handle, [ref]$owner) | Out-Null
        if ($owner -ne $script:e2eSearchProcessId -or
            -not [CodexInfoWindowsE2EWin32]::IsWindowVisible($Handle)) {
            return $true
        }
        $title = New-Object System.Text.StringBuilder 256
        [CodexInfoWindowsE2EWin32]::GetWindowText($Handle, $title, $title.Capacity) | Out-Null
        if ($title.ToString().IndexOf($script:e2eSearchTitle, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            $script:e2eSearchWindow = $Handle
            return $false
        }
        return $true
    }
    [CodexInfoWindowsE2EWin32]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
    return $script:e2eSearchWindow
}

function Get-E2EWindowBounds {
    param([Parameter(Mandatory = $true)][IntPtr]$Handle)

    $rect = New-Object CodexInfoWindowsE2EWin32+RECT
    Assert-E2E ([CodexInfoWindowsE2EWin32]::GetWindowRect($Handle, [ref]$rect)) 'GetWindowRect failed.'
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    Assert-E2E ($width -gt 0 -and $height -gt 0) "Invalid window bounds ${width}x${height}."
    return [pscustomobject]@{
        Left = $rect.Left
        Top = $rect.Top
        Width = $width
        Height = $height
    }
}

function Record-E2EWindow {
    param(
        [Parameter(Mandatory = $true)][string]$Role,
        [Parameter(Mandatory = $true)][int]$ExpectedProcessId,
        [Parameter(Mandatory = $true)][IntPtr]$Handle
    )

    [uint32]$owner = 0
    [CodexInfoWindowsE2EWin32]::GetWindowThreadProcessId($Handle, [ref]$owner) | Out-Null
    Assert-E2E ($owner -eq [uint32]$ExpectedProcessId) "$Role HWND is owned by PID $owner, expected $ExpectedProcessId."
    $bounds = Get-E2EWindowBounds $Handle
    $record = [pscustomobject]@{
        role = $Role
        pid = [int]$owner
        hwnd = ('0x{0:X}' -f $Handle.ToInt64())
        left = $bounds.Left
        top = $bounds.Top
        width = $bounds.Width
        height = $bounds.Height
        recorded_at = [DateTimeOffset]::Now.ToString('o')
    }
    $script:e2eWindowRecords.Add($record)
    Write-E2E ("window: role={0} pid={1} hwnd={2} bounds={3}x{4}+{5}+{6}" -f
        $Role, $owner, $record.hwnd, $bounds.Width, $bounds.Height, $bounds.Left, $bounds.Top)
    return $record
}

function Bring-E2EWindowToFront {
    param([Parameter(Mandatory = $true)][IntPtr]$Handle)

    [CodexInfoWindowsE2EWin32]::ShowWindow($Handle, 9) | Out-Null
    [CodexInfoWindowsE2EWin32]::BringWindowToTop($Handle) | Out-Null
    [CodexInfoWindowsE2EWin32]::SetForegroundWindow($Handle) | Out-Null
    Start-Sleep -Milliseconds 250
}

function Get-E2EUiaRoot {
    param([Parameter(Mandatory = $true)][IntPtr]$Handle)

    return [System.Windows.Automation.AutomationElement]::FromHandle($Handle)
}

function Get-E2EAllDescendants {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root)

    $lastFailure = $null
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        try {
            $all = [System.Collections.Generic.List[object]]::new()
            $all.Add($Root)
            $descendants = $Root.FindAll(
                [System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.Condition]::TrueCondition)
            foreach ($element in $descendants) { $all.Add($element) }
            return $all
        }
        catch [System.Runtime.InteropServices.COMException] {
            $lastFailure = $_
            if ($attempt -lt 5) { Start-Sleep -Milliseconds 100 }
        }
    }

    throw $lastFailure
}

function Find-E2EElementByAutomationId {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][string]$AutomationId
    )

    $condition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
        $AutomationId)
    return $Root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
}

function Get-E2EElementsByAutomationId {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][string]$AutomationId
    )

    $condition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
        $AutomationId)
    return @($Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition))
}

function Assert-E2EMainProductVersion {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root)

    $versions = @(Get-E2EElementsByAutomationId $Root 'Main.ProductVersion')
    Assert-E2E ($versions.Count -eq 1) "Main product version must have exactly one UIA element, found $($versions.Count)."
    $value = [string]$versions[0].Current.Name
    Assert-E2E ($value -match '^v[0-9]+\.[0-9]+\.[0-9]+$') "Main product version is malformed: '$value'."
    Write-E2E "main-product-version: PASS value=$value count=$($versions.Count)"
}

function Assert-E2ENoChildProductVersion {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][string]$Role
    )

    $versions = @(Get-E2EElementsByAutomationId $Root 'Main.ProductVersion')
    Assert-E2E ($versions.Count -eq 0) "$Role child window must not expose Main.ProductVersion, found $($versions.Count)."
    Write-E2E "child-product-version: PASS role=$Role count=$($versions.Count)"
}

function Find-E2EButtonByName {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][string]$Name
    )

    foreach ($element in Get-E2EAllDescendants $Root) {
        try {
            if ($element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
                [string]$element.Current.Name -eq $Name) {
                return $element
            }
        }
        catch { }
    }
    return $null
}

function Get-E2EControlElements {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][System.Windows.Automation.ControlType]$ControlType
    )

    $result = [System.Collections.Generic.List[object]]::new()
    foreach ($element in Get-E2EAllDescendants $Root) {
        try {
            if ($element.Current.ControlType -eq $ControlType) { $result.Add($element) }
        }
        catch { }
    }
    return $result
}

function Get-E2EVisibleControlElements {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][System.Windows.Automation.ControlType]$ControlType
    )

    return @(Get-E2EControlElements $Root $ControlType | Where-Object {
        try {
            $rectangle = $_.Current.BoundingRectangle
            -not $_.Current.IsOffscreen -and $_.Current.IsEnabled -and
                $rectangle.Width -gt 0 -and $rectangle.Height -gt 0
        }
        catch { $false }
    })
}

function Get-E2ETextValues {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root)

    $values = [System.Collections.Generic.List[string]]::new()
    foreach ($element in Get-E2EAllDescendants $Root) {
        try {
            $name = [string]$element.Current.Name
            if (-not [string]::IsNullOrWhiteSpace($name)) { $values.Add($name.Trim()) }
        }
        catch { }
        try {
            $valuePattern = $null
            if ($element.TryGetCurrentPattern(
                    [System.Windows.Automation.ValuePattern]::Pattern,
                    [ref]$valuePattern)) {
                $value = [string]$valuePattern.Current.Value
                if (-not [string]::IsNullOrWhiteSpace($value)) { $values.Add($value.Trim()) }
            }
        }
        catch { }
    }
    return @($values | Select-Object -Unique)
}

function Invoke-E2EElement {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element)

    $pattern = $null
    Assert-E2E ($Element.TryGetCurrentPattern(
        [System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) 'Element has no InvokePattern.'
    $pattern.Invoke()
}

function Get-E2EToggleState {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element)

    $pattern = $null
    Assert-E2E ($Element.TryGetCurrentPattern(
        [System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) 'Element has no TogglePattern.'
    return $pattern.Current.ToggleState
}

function Toggle-E2EElement {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element)

    $pattern = $null
    Assert-E2E ($Element.TryGetCurrentPattern(
        [System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) 'Element has no TogglePattern.'
    $pattern.Toggle()
}

function Select-E2EListItem {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $items = Get-E2EVisibleControlElements $Root ([System.Windows.Automation.ControlType]::ListItem)
    $item = $items | Where-Object { [string]$_.Current.Name -eq $Label } | Select-Object -First 1
    Assert-E2E ($null -ne $item) "List item '$Label' is missing."
    $selection = $null
    if ($item.TryGetCurrentPattern(
            [System.Windows.Automation.SelectionItemPattern]::Pattern,
            [ref]$selection)) {
        $selection.Select()
        return
    }
    $invoke = $null
    Assert-E2E ($item.TryGetCurrentPattern(
        [System.Windows.Automation.InvokePattern]::Pattern, [ref]$invoke)) "List item '$Label' is not selectable."
    $invoke.Invoke()
}

function Get-E2ESelectedListItemLabel {
    param([Parameter(Mandatory = $true)][object[]]$Items)

    foreach ($item in $Items) {
        try {
            $selection = $null
            if ($item.TryGetCurrentPattern(
                    [System.Windows.Automation.SelectionItemPattern]::Pattern,
                    [ref]$selection) -and $selection.Current.IsSelected) {
                $label = [string]$item.Current.Name
                if (-not [string]::IsNullOrWhiteSpace($label)) { return $label }
            }
        }
        catch { }
    }
    return ''
}

function Get-E2ESelectorLabel {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Selector)

    $helpText = [string]$Selector.Current.HelpText
    if (-not [string]::IsNullOrWhiteSpace($helpText)) {
        return $helpText.Trim()
    }

    $values = @(Get-E2ETextValues $Selector)
    # The selector's own accessible name is a localized field label
    # (for example "Reset time"), while its child text is the selected
    # value. Exclude the root name dynamically instead of maintaining an
    # English-only list that makes a Japanese installed run time out.
    $rootName = [string]$Selector.Current.Name
    $filtered = $values | Where-Object {
        $_ -ne $rootName -and $_ -notin @(
            'Reset time', 'Model usage', 'Remaining quota',
            'Reset period', 'Dollars', 'Tokens')
    }
    if ($filtered.Count -gt 0) { return [string]$filtered[-1] }
    if ($values.Count -gt 0) { return [string]$values[-1] }
    return ''
}

function Wait-E2ESelectorLabel {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][string]$AutomationId,
        [Parameter(Mandatory = $true)][string]$Expected
    )

    $script:e2eLastSelectorLabel = ''
    try {
        Wait-E2E -Description "selector $AutomationId displays '$Expected'" -Probe {
            $selector = Find-E2EElementByAutomationId $Root $AutomationId
            if ($null -eq $selector) { return $false }
            $script:e2eLastSelectorLabel = Get-E2ESelectorLabel $selector
            return $script:e2eLastSelectorLabel -eq $Expected -or $script:e2eLastSelectorLabel.Contains($Expected)
        } | Out-Null
    }
    catch {
        Write-E2E "selector: FAIL id=$AutomationId expected='$Expected' observed='$script:e2eLastSelectorLabel'"
        throw
    }
}

function Capture-E2EWindow {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][string]$Name
    )

    $safeName = ($Name -replace '[^A-Za-z0-9_.-]', '_')
    $path = Join-Path $script:e2eOutput "$safeName.png"
    # UIA state is published before the compositor necessarily presents the
    # corresponding frame.  Allow one bounded paint interval before taking
    # the independent target-window observation.
    Start-Sleep -Milliseconds 250
    $bounds = Get-E2EWindowBounds $Handle
    $bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        # Capture the requested HWND directly.  PrintWindow is deliberately
        # used instead of a desktop-coordinate copy so another foreground or
        # occluding window can never replace the requested window's pixels.
        $graphics.Clear([System.Drawing.Color]::Magenta)
        $hdc = $graphics.GetHdc()
        try {
            $printWindowFlags = 2 # PW_RENDERFULLCONTENT
            $printed = [CodexInfoWindowsE2EWin32]::PrintWindow(
                $Handle,
                $hdc,
                [uint32]$printWindowFlags)
            $printWindowError = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
        }
        finally {
            $graphics.ReleaseHdc($hdc)
        }
        Assert-E2E $printed "PrintWindow failed for HWND 0x$('{0:X}' -f $Handle.ToInt64()) (last-error=$printWindowError)."
        Assert-E2E ($bitmap.Width -eq $bounds.Width -and $bitmap.Height -eq $bounds.Height) 'Target HWND capture dimensions changed.'
        $sentinelArgb = [System.Drawing.Color]::Magenta.ToArgb()
        $changedPixels = 0
        for ($x = 0; $x -lt $bitmap.Width -and $changedPixels -eq 0; $x++) {
            for ($y = 0; $y -lt $bitmap.Height; $y++) {
                if ($bitmap.GetPixel($x, $y).ToArgb() -ne $sentinelArgb) {
                    $changedPixels++
                    break
                }
            }
        }
        Assert-E2E ($changedPixels -gt 0) 'PrintWindow returned empty/unchanged target content.'
        $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-E2E "capture: name=$safeName path=$path sha256=$hash size=$($bounds.Width)x$($bounds.Height)"
    return [pscustomobject]@{ Path = $path; Hash = $hash }
}

function Assert-E2ECaptureColor {
    param(
        [Parameter(Mandatory = $true)][psobject]$Capture,
        [Parameter(Mandatory = $true)][System.Drawing.Color]$Expected,
        [Parameter(Mandatory = $true)][string]$Description
    )

    $bitmap = [System.Drawing.Bitmap]::FromFile($Capture.Path)
    try {
        Assert-E2E ($bitmap.Width -gt 0 -and $bitmap.Height -gt 0) "$Description produced an empty bitmap."
        $pixel = $bitmap.GetPixel([int]($bitmap.Width / 2), [int]($bitmap.Height / 2))
        $dr = $pixel.R - $Expected.R
        $dg = $pixel.G - $Expected.G
        $db = $pixel.B - $Expected.B
        $distance = ($dr * $dr) + ($dg * $dg) + ($db * $db)
        Assert-E2E ($distance -le (4 * 4)) "$Description returned unexpected target pixels: actual=#$('{0:X2}{1:X2}{2:X2}' -f $pixel.R, $pixel.G, $pixel.B) expected=#$('{0:X2}{1:X2}{2:X2}' -f $Expected.R, $Expected.G, $Expected.B)."
    }
    finally {
        $bitmap.Dispose()
    }
}

function Assert-E2ECaptureExpectedFailure {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$Action
    )

    $failure = $null
    try {
        & $Action | Out-Null
    }
    catch {
        $failure = $_.Exception.Message
    }
    Assert-E2E ($null -ne $failure) "Capture self-test case=$Name unexpectedly passed."
    Write-E2E "capture-self-test: PASS case=$Name rejected=$failure"
}

function Invoke-E2ECaptureSelfTest {
    $target = [IntPtr]::Zero
    $decoy = [IntPtr]::Zero
    $capturedPaths = [System.Collections.Generic.List[string]]::new()
    try {
        $target = [CodexInfoWindowsE2ECaptureTestWindow]::Create('TARGET', 40, 40, 160, 100)
        $decoy = [CodexInfoWindowsE2ECaptureTestWindow]::Create('DECOY', 40, 40, 160, 100)
        Assert-E2E ($target -ne [IntPtr]::Zero -and $decoy -ne [IntPtr]::Zero) 'Capture self-test could not create target and decoy HWNDs.'
        [CodexInfoWindowsE2ECaptureTestWindow]::Show($target)
        [CodexInfoWindowsE2ECaptureTestWindow]::Show($decoy)
        $topmost = [IntPtr](-1)
        Assert-E2E ([CodexInfoWindowsE2EWin32]::SetWindowPos($decoy, $topmost, 40, 40, 160, 100, [uint32]0x0043)) 'Capture self-test could not place the decoy above the target HWND.'
        [CodexInfoWindowsE2EWin32]::BringWindowToTop($decoy) | Out-Null
        [CodexInfoWindowsE2EWin32]::SetForegroundWindow($decoy) | Out-Null
        Start-Sleep -Milliseconds 100

        $targetWhileOccluded = Capture-E2EWindow -Handle $target -Name 'capture-self-test-target-occluded'
        $capturedPaths.Add($targetWhileOccluded.Path)
        Assert-E2ECaptureColor -Capture $targetWhileOccluded -Expected ([System.Drawing.ColorTranslator]::FromHtml('#2060C0')) -Description 'target HWND while decoy is topmost'

        $decoyCapture = Capture-E2EWindow -Handle $decoy -Name 'capture-self-test-decoy'
        $capturedPaths.Add($decoyCapture.Path)
        Assert-E2ECaptureColor -Capture $decoyCapture -Expected ([System.Drawing.ColorTranslator]::FromHtml('#C00000')) -Description 'decoy HWND'
        Write-E2E 'capture-self-test: PASS target HWND content remained target-colored while the red decoy was topmost'

        [CodexInfoWindowsE2EWin32]::ShowWindow($decoy, 0) | Out-Null
        $targetAfterOccluder = Capture-E2EWindow -Handle $target -Name 'capture-self-test-target-after-occluder'
        $capturedPaths.Add($targetAfterOccluder.Path)
        Assert-E2ECaptureColor -Capture $targetAfterOccluder -Expected ([System.Drawing.ColorTranslator]::FromHtml('#2060C0')) -Description 'target HWND after decoy removal'
        Write-E2E 'capture-self-test: PASS requested HWND capture is stable before and after occluder removal'

        Assert-E2ECaptureExpectedFailure -Name 'invalid-hwnd' -Action {
            Capture-E2EWindow -Handle ([IntPtr]::Zero) -Name 'capture-self-test-invalid'
        }

        $destroyedTarget = $target
        [CodexInfoWindowsE2ECaptureTestWindow]::Destroy($target)
        $target = [IntPtr]::Zero
        Assert-E2ECaptureExpectedFailure -Name 'capture-failure' -Action {
            Capture-E2EWindow -Handle $destroyedTarget -Name 'capture-self-test-destroyed'
        }
    }
    finally {
        [CodexInfoWindowsE2ECaptureTestWindow]::Destroy($target)
        [CodexInfoWindowsE2ECaptureTestWindow]::Destroy($decoy)
        foreach ($path in $capturedPaths) {
            if (Test-Path -LiteralPath $path -PathType Leaf) {
                Remove-Item -LiteralPath $path -Force
            }
        }
    }
    Write-E2E 'capture-self-test: PASS target, occluding decoy, and invalid/failure cases'
}

function Set-E2EGraphLogicalSize {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][int]$LogicalWidth,
        [Parameter(Mandatory = $true)][int]$LogicalHeight,
        [Parameter(Mandatory = $true)][double]$Scale
    )

    $physicalWidth = [int][Math]::Floor(($LogicalWidth * $Scale) + 0.5)
    $physicalHeight = [int][Math]::Floor(($LogicalHeight * $Scale) + 0.5)
    $bounds = Get-E2EWindowBounds $Handle
    $flags = [uint32]0x0016 # SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
    Assert-E2E ([CodexInfoWindowsE2EWin32]::SetWindowPos(
        $Handle, [IntPtr]::Zero, $bounds.Left, $bounds.Top,
        $physicalWidth, $physicalHeight, $flags)) `
        "Graph resize to ${LogicalWidth}x${LogicalHeight} logical failed."
    Wait-E2E -Description "Graph resize ${LogicalWidth}x${LogicalHeight} logical" -Probe {
        $current = Get-E2EWindowBounds $Handle
        return $current.Width -eq $physicalWidth -and $current.Height -eq $physicalHeight
    } | Out-Null
    Start-Sleep -Milliseconds 300
}

function Wait-E2EGraphLoadSettled {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root)

    Start-Sleep -Milliseconds 300
    Wait-E2E -Description 'Graph period/metric load settles' -Probe {
        $progress = @(Get-E2EAllDescendants $Root | Where-Object {
            $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::ProgressBar -and
            -not $_.Current.IsOffscreen
        })
        return $progress.Count -eq 0
    } | Out-Null
}

function Get-E2EGraphMeasurement {
    param(
        [Parameter(Mandatory = $true)][psobject]$Capture,
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Plot,
        [Parameter(Mandatory = $true)][IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][string]$Description
    )

    $windowBounds = Get-E2EWindowBounds $WindowHandle
    $plotRect = $Plot.Current.BoundingRectangle
    $plotLeft = [int][Math]::Floor(($plotRect.Left - $windowBounds.Left) + 0.5)
    $plotTop = [int][Math]::Floor(($plotRect.Top - $windowBounds.Top) + 0.5)
    $plotWidth = [int][Math]::Floor($plotRect.Width + 0.5)
    $plotHeight = [int][Math]::Floor($plotRect.Height + 0.5)
    $measurement = [CodexInfoGraphPixelScanner]::Scan(
        $Capture.Path, $plotLeft, $plotTop, $plotWidth, $plotHeight)
    $seriesNames = @('Remaining', 'SOL', 'TERRA', 'LUNA')
    for ($index = 0; $index -lt $seriesNames.Count; $index++) {
        Assert-E2E ($measurement.SeriesPixelCount[$index] -gt 0) `
            "$Description has no visible $($seriesNames[$index]) color pixels."
        Assert-E2E ($measurement.SeriesGutterPixelCount[$index] -gt 0) `
            "$Description has no $($seriesNames[$index]) leader/glyph pixels in the endpoint gutter."
        Assert-E2E ($measurement.SeriesRightmost[$index] -le $plotWidth - 3) `
            "$Description clips $($seriesNames[$index]) at the right plot edge."
    }
    Write-E2E ("graph-resize-measurement: state={0} plot={1}x{2} grids={3} start={4} end={5} span={6} gutter={7}" -f
        $Description, $plotWidth, $plotHeight, ($measurement.GridCenters -join ','),
        $measurement.PeriodStartX, $measurement.PeriodEndX,
        $measurement.PlotSpan, $measurement.GutterWidth)
    return [pscustomobject]@{
        Pixels = $measurement
        PlotBoundsWidth = $plotWidth
        PlotBoundsHeight = $plotHeight
    }
}

function Wait-E2EGraphPixelsReady {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][string]$Description
    )

    return Wait-E2E -Description "$Description rendered graph pixels" -TimeoutSeconds 30 -Probe {
        $candidatePlot = Find-E2EElementByAutomationId $Root 'Graph.Plot'
        if ($null -eq $candidatePlot) { return $false }
        $candidateCapture = Capture-E2EWindow $WindowHandle 'graph-ready-probe'
        try {
            return Get-E2EGraphMeasurement -Capture $candidateCapture -Plot $candidatePlot `
                -WindowHandle $WindowHandle -Description $Description
        }
        catch {
            return $false
        }
    }
}

function Invoke-E2EGraphPixelScannerSelfTest {
    $validPath = Join-Path $script:e2eOutput 'graph-pixel-scanner-self-test-valid.png'
    $invalidPath = Join-Path $script:e2eOutput 'graph-pixel-scanner-self-test-invalid.png'
    $gridColor = [System.Drawing.ColorTranslator]::FromHtml('#263548')
    $background = [System.Drawing.ColorTranslator]::FromHtml('#101925')
    $seriesColors = @('#56B2F5', '#A88CF5', '#5DC98A', '#E6A23C') |
        ForEach-Object { [System.Drawing.ColorTranslator]::FromHtml($_) }
    foreach ($case in @(
        @{ Path = $validPath; GridXs = @(10, 50, 90, 130, 170, 230) },
        @{ Path = $invalidPath; GridXs = @(10, 50, 90, 130) }
    )) {
        $bitmap = New-Object System.Drawing.Bitmap(240, 140)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.Clear($background)
            $gridPen = New-Object System.Drawing.Pen($gridColor, 1)
            try {
                foreach ($x in $case.GridXs) { $graphics.DrawLine($gridPen, $x, 5, $x, 135) }
            }
            finally { $gridPen.Dispose() }
            for ($index = 0; $index -lt $seriesColors.Count; $index++) {
                $seriesPen = New-Object System.Drawing.Pen($seriesColors[$index], 2)
                try { $graphics.DrawLine($seriesPen, 175, 30 + ($index * 20), 215, 30 + ($index * 20)) }
                finally { $seriesPen.Dispose() }
            }
            $bitmap.Save($case.Path, [System.Drawing.Imaging.ImageFormat]::Png)
        }
        finally {
            $graphics.Dispose()
            $bitmap.Dispose()
        }
    }

    try {
        $valid = [CodexInfoGraphPixelScanner]::Scan($validPath, 0, 0, 240, 140)
        Assert-E2E ($valid.PeriodStartX -eq 10 -and $valid.PeriodEndX -eq 170 -and
            $valid.PlotSpan -eq 160 -and $valid.GutterWidth -eq 69) `
            'Graph pixel scanner rejected the valid synthetic geometry.'
        Assert-E2E (($valid.SeriesGutterPixelCount | Where-Object { $_ -le 0 }).Count -eq 0) `
            'Graph pixel scanner missed synthetic endpoint colors.'
        Write-E2E 'graph-pixel-scanner-self-test: PASS valid fixed-gutter geometry'
        Assert-E2ECaptureExpectedFailure -Name 'graph-grid-negative' -Action {
            [CodexInfoGraphPixelScanner]::Scan($invalidPath, 0, 0, 240, 140)
        }
    }
    finally {
        foreach ($path in @($validPath, $invalidPath)) {
            if (Test-Path -LiteralPath $path -PathType Leaf) { Remove-Item -LiteralPath $path -Force }
        }
    }
}

function Assert-E2EImageChanged {
    param(
        [Parameter(Mandatory = $true)][psobject]$Before,
        [Parameter(Mandatory = $true)][psobject]$After,
        [Parameter(Mandatory = $true)][string]$Description
    )
    Assert-E2E ($Before.Hash -ne $After.Hash) "$Description did not change the rendered window."
}

function Get-E2EGraphOracleSeries {
    return @(
        [pscustomobject]@{
            Name = 'LUNA'
            Color = [System.Drawing.ColorTranslator]::FromHtml('#E6A23C')
        },
        [pscustomobject]@{
            Name = 'TERRA'
            Color = [System.Drawing.ColorTranslator]::FromHtml('#5DC98A')
        },
        [pscustomobject]@{
            Name = 'SOL'
            Color = [System.Drawing.ColorTranslator]::FromHtml('#A88CF5')
        }
    )
}

function Get-E2EGraphIdleBackgroundColor {
    $plotBackground = [System.Drawing.ColorTranslator]::FromHtml('#101925')
    $idleBand = [System.Drawing.ColorTranslator]::FromHtml('#3F5D7C')
    $opacity = 0.22
    return [System.Drawing.Color]::FromArgb(
        255,
        [int][Math]::Round($plotBackground.R + (($idleBand.R - $plotBackground.R) * $opacity)),
        [int][Math]::Round($plotBackground.G + (($idleBand.G - $plotBackground.G) * $opacity)),
        [int][Math]::Round($plotBackground.B + (($idleBand.B - $plotBackground.B) * $opacity)))
}

function Get-E2EGraphCompositedColor {
    param(
        [Parameter(Mandatory = $true)][System.Drawing.Color]$Background,
        [Parameter(Mandatory = $true)][System.Drawing.Color]$Series,
        [Parameter(Mandatory = $true)][double]$Alpha
    )
    return [System.Drawing.Color]::FromArgb(
        255,
        [int][Math]::Round($Background.R + (($Series.R - $Background.R) * $Alpha)),
        [int][Math]::Round($Background.G + (($Series.G - $Background.G) * $Alpha)),
        [int][Math]::Round($Background.B + (($Series.B - $Background.B) * $Alpha)))
}

function Test-E2EGraphCompositedSeriesPixel {
    param(
        [Parameter(Mandatory = $true)][System.Drawing.Color]$Pixel,
        [Parameter(Mandatory = $true)][System.Drawing.Color]$Background,
        [Parameter(Mandatory = $true)][System.Drawing.Color]$Series
    )

    $pixelChannels = @($Pixel.R, $Pixel.G, $Pixel.B)
    $backgroundChannels = @($Background.R, $Background.G, $Background.B)
    $seriesChannels = @($Series.R, $Series.G, $Series.B)
    # The weakest observed real series row (TERRA #274748 over #1A2838)
    # implies alpha ~= 0.19. Keep a small rounding margin while rejecting
    # the plot axis (#263548), whose strongest implied alpha is below 0.18.
    $minimumCompositedAlpha = 0.18
    $alphas = @()
    for ($channel = 0; $channel -lt 3; $channel++) {
        $delta = $seriesChannels[$channel] - $backgroundChannels[$channel]
        if ([Math]::Abs($delta) -lt 16) {
            # A weak source channel must remain near the idle background; a
            # large excursion here belongs to a different series color.
            if ([Math]::Abs($pixelChannels[$channel] - $backgroundChannels[$channel]) -gt 12) {
                return $false
            }
            continue
        }
        $alpha = ($pixelChannels[$channel] - $backgroundChannels[$channel]) / [double]$delta
        if ($alpha -lt $minimumCompositedAlpha -or $alpha -gt 0.72) {
            return $false
        }
        $alphas += $alpha
    }
    if ($alphas.Count -lt 2) {
        return $false
    }
    $minimum = ($alphas | Measure-Object -Minimum).Minimum
    $maximum = ($alphas | Measure-Object -Maximum).Maximum
    return ($maximum - $minimum -le 0.22)
}

function Get-E2EGraphFlatLineCandidates {
    param(
        [Parameter(Mandatory = $true)][System.Drawing.Bitmap]$Bitmap,
        [Parameter(Mandatory = $true)][int]$Left,
        [Parameter(Mandatory = $true)][int]$Top,
        [Parameter(Mandatory = $true)][int]$Right,
        [Parameter(Mandatory = $true)][int]$Bottom,
        [Parameter(Mandatory = $true)][psobject]$Series,
        [Parameter(Mandatory = $true)][System.Drawing.Color]$Background,
        [double]$FlatStartFraction = 0.06,
        [double]$FlatEndFraction = 0.84,
        [double]$FlatCoverageThreshold = 0.90
    )

    $width = $Right - $Left
    $height = $Bottom - $Top
    $flatLeft = $Left + [int][Math]::Floor($width * $FlatStartFraction)
    $flatRight = $Left + [int][Math]::Floor($width * $FlatEndFraction)
    $flatWidth = $flatRight - $flatLeft
    if ($flatWidth -le 0) { return @() }

    # First find rows with meaningful compositor-aware coverage.  A sloped
    # line distributes its pixels across many rows and cannot form a strong
    # contiguous row candidate here.
    $rowCoverage = @{}
    $rowCandidateThreshold = 0.20
    for ($y = $Top; $y -lt $Bottom; $y++) {
        $matchedColumns = 0
        for ($x = $flatLeft; $x -lt $flatRight; $x++) {
            if (Test-E2EGraphCompositedSeriesPixel -Pixel $Bitmap.GetPixel($x, $y) -Background $Background -Series $Series.Color) {
                $matchedColumns++
            }
        }
        $coverage = $matchedColumns / [double]$flatWidth
        if ($coverage -ge $rowCandidateThreshold) {
            $rowCoverage[$y] = $coverage
        }
    }

    $groups = [System.Collections.Generic.List[object]]::new()
    $groupStart = $null
    $previousRow = $null
    for ($y = $Top; $y -lt $Bottom; $y++) {
        if (-not $rowCoverage.ContainsKey($y)) { continue }
        if ($null -eq $groupStart) {
            $groupStart = $y
        }
        elseif ($y -ne ($previousRow + 1)) {
            $groups.Add([pscustomobject]@{ Top = $groupStart; Bottom = $previousRow })
            $groupStart = $y
        }
        $previousRow = $y
    }
    if ($null -ne $groupStart) {
        $groups.Add([pscustomobject]@{ Top = $groupStart; Bottom = $previousRow })
    }

    $candidates = [System.Collections.Generic.List[object]]::new()
    $maximumRowSpan = [Math]::Max(6, [int][Math]::Ceiling($height * 0.04))
    foreach ($group in $groups) {
        $matchedColumns = [System.Collections.Generic.HashSet[int]]::new()
        $minimumMatchedRow = $Bottom
        $maximumMatchedRow = $Top
        for ($x = $flatLeft; $x -lt $flatRight; $x++) {
            for ($y = $group.Top; $y -le $group.Bottom; $y++) {
                if (Test-E2EGraphCompositedSeriesPixel -Pixel $Bitmap.GetPixel($x, $y) -Background $Background -Series $Series.Color) {
                    $null = $matchedColumns.Add($x)
                    if ($y -lt $minimumMatchedRow) { $minimumMatchedRow = $y }
                    if ($y -gt $maximumMatchedRow) { $maximumMatchedRow = $y }
                }
            }
        }
        if ($matchedColumns.Count -eq 0) { continue }
        $coverage = $matchedColumns.Count / [double]$flatWidth
        $rowSpan = $maximumMatchedRow - $minimumMatchedRow + 1
        if ($coverage -ge $FlatCoverageThreshold -and $rowSpan -le $maximumRowSpan) {
            $candidates.Add([pscustomobject]@{
                RowTop = $minimumMatchedRow
                RowBottom = $maximumMatchedRow
                Center = [int][Math]::Round(($minimumMatchedRow + $maximumMatchedRow) / 2.0)
                CoverageColumns = $matchedColumns.Count
                Width = $flatWidth
                Coverage = $coverage
                RowSpan = $rowSpan
            })
        }
    }
    return @($candidates)
}

function Find-E2EGraphSharedRisingSegment {
    param(
        [Parameter(Mandatory = $true)][System.Drawing.Bitmap]$Bitmap,
        [Parameter(Mandatory = $true)][int]$Left,
        [Parameter(Mandatory = $true)][int]$Top,
        [Parameter(Mandatory = $true)][int]$Right,
        [Parameter(Mandatory = $true)][int]$Bottom,
        [Parameter(Mandatory = $true)][psobject[]]$Series,
        [Parameter(Mandatory = $true)][System.Collections.IDictionary]$FlatRows,
        [double]$RisingCenterFraction = 0.84
    )

    $width = $Right - $Left
    $height = $Bottom - $Top
    $risingCenter = $Left + [int][Math]::Floor($width * $RisingCenterFraction)
    $risingHalfWidth = [Math]::Max(3, [int][Math]::Ceiling($width * 0.015))
    $risingLeft = [Math]::Max($Left, $risingCenter - $risingHalfWidth)
    $risingRight = [Math]::Min($Right, $risingCenter + $risingHalfWidth + 1)
    $connectionTolerance = [Math]::Max(4, [int][Math]::Ceiling($height * 0.02))
    $minimumVerticalExtent = [Math]::Max(12, [int][Math]::Ceiling($height * 0.08))
    $lunaCenter = [int]$FlatRows['LUNA'].Center
    $terraCenter = [int]$FlatRows['TERRA'].Center
    $solCenter = [int]$FlatRows['SOL'].Center
    $minimumSharedVerticalExtent = [Math]::Max(20, ($solCenter - $lunaCenter) + $minimumVerticalExtent)
    $maxAllowedGap = 2
    $candidateColumns = [System.Collections.Generic.List[object]]::new()
    $sourceColors = @($Series | ForEach-Object { $_.Color })
    for ($x = $risingLeft; $x -lt $risingRight; $x++) {
        $matchedRows = [System.Collections.Generic.List[int]]::new()
        for ($y = $Top; $y -lt $Bottom; $y++) {
            $pixel = $Bitmap.GetPixel($x, $y)
            foreach ($sourceColor in $sourceColors) {
                $dr = $pixel.R - $sourceColor.R
                $dg = $pixel.G - $sourceColor.G
                $db = $pixel.B - $sourceColor.B
                if ((($dr * $dr) + ($dg * $dg) + ($db * $db)) -le (24 * 24)) {
                    $matchedRows.Add($y)
                    break
                }
            }
        }
        if ($matchedRows.Count -eq 0) { continue }
        $runs = [System.Collections.Generic.List[object]]::new()
        $runStart = $null
        $lastRow = $null
        $runHits = 0
        foreach ($row in $matchedRows) {
            if ($null -eq $runStart) {
                $runStart = $row
                $runHits = 1
            }
            elseif (($row - $lastRow - 1) -le $maxAllowedGap) {
                $runHits++
            }
            else {
                $runs.Add([pscustomobject]@{ Top = $runStart; Bottom = $lastRow; Hits = $runHits })
                $runStart = $row
                $runHits = 1
            }
            $lastRow = $row
        }
        if ($null -ne $runStart) {
            $runs.Add([pscustomobject]@{ Top = $runStart; Bottom = $lastRow; Hits = $runHits })
        }
        foreach ($run in $runs) {
            $runSpan = $run.Bottom - $run.Top + 1
            $runDensity = $run.Hits / [double]$runSpan
            $touchesLowestFlat = $run.Bottom -ge ($solCenter - $connectionTolerance) -and
                $run.Bottom -le ($solCenter + $connectionTolerance)
            $passesUpperFlat = $run.Top -le ($lunaCenter - $connectionTolerance)
            if ($touchesLowestFlat -and $passesUpperFlat -and
                $runSpan -ge $minimumSharedVerticalExtent -and $runDensity -ge 0.75) {
                $candidateColumns.Add([pscustomobject]@{
                    X = $x
                    Top = $run.Top
                    Bottom = $run.Bottom
                    Hits = $run.Hits
                    Span = $runSpan
                    Density = $runDensity
                })
                break
            }
        }
    }
    Assert-E2E ($candidateColumns.Count -ge 1) "Past graph shared right-edge rising step is missing, detached, short, or non-contiguous: columns=$($candidateColumns.Count) expected-x=$risingCenter."
    $bestRun = $candidateColumns | Sort-Object Density, Span -Descending | Select-Object -First 1
    $minimumContributionPixels = 4
    $minimumContributionExtent = [Math]::Max(4, [int][Math]::Ceiling($height * 0.02))
    $contributions = @{}
    foreach ($item in $Series) {
        $contributionHits = 0
        $minimumContributionRow = $Bottom
        $maximumContributionRow = $Top
        foreach ($column in $candidateColumns) {
            for ($y = $column.Top; $y -le $column.Bottom; $y++) {
                $pixel = $Bitmap.GetPixel($column.X, $y)
                $dr = $pixel.R - $item.Color.R
                $dg = $pixel.G - $item.Color.G
                $db = $pixel.B - $item.Color.B
                if ((($dr * $dr) + ($dg * $dg) + ($db * $db)) -le (24 * 24)) {
                    $contributionHits++
                    if ($y -lt $minimumContributionRow) { $minimumContributionRow = $y }
                    if ($y -gt $maximumContributionRow) { $maximumContributionRow = $y }
                }
            }
        }
        $contributionExtent = if ($contributionHits -gt 0) { $maximumContributionRow - $minimumContributionRow + 1 } else { 0 }
        Assert-E2E ($contributionHits -ge $minimumContributionPixels -and $contributionExtent -ge $minimumContributionExtent) "Past graph shared right-edge step has insufficient $($item.Name) source contribution under overdraw: pixels=$contributionHits extent=$contributionExtent."
        $contributions[$item.Name] = [pscustomobject]@{
            Pixels = $contributionHits
            Top = $minimumContributionRow
            Bottom = $maximumContributionRow
            Extent = $contributionExtent
        }
    }
    return [pscustomobject]@{
        Columns = $candidateColumns.Count
        PixelHits = ($candidateColumns | Measure-Object -Property Hits -Sum).Sum
        Top = $bestRun.Top
        Bottom = $bestRun.Bottom
        Center = $risingCenter
        Density = $bestRun.Density
        ConnectionTolerance = $connectionTolerance
        MinimumVerticalExtent = $minimumSharedVerticalExtent
        Contributions = $contributions
    }
}

function Assert-E2EGraphModelPixels {
    param(
        [Parameter(Mandatory = $true)][System.Drawing.Bitmap]$Bitmap,
        [Parameter(Mandatory = $true)][int]$Left,
        [Parameter(Mandatory = $true)][int]$Top,
        [Parameter(Mandatory = $true)][int]$Right,
        [Parameter(Mandatory = $true)][int]$Bottom
    )

    $width = $Right - $Left
    $height = $Bottom - $Top
    Assert-E2E ($width -gt 0 -and $height -gt 0) 'Graph oracle bounds are empty.'
    $idleBackground = Get-E2EGraphIdleBackgroundColor
    $flatStartFraction = 0.06
    $flatEndFraction = 0.84
    $risingCenterFraction = 0.84
    $flatCoverageThreshold = 0.90
    $flatLeft = $Left + [int][Math]::Floor($width * $flatStartFraction)
    $flatRight = $Left + [int][Math]::Floor($width * $flatEndFraction)
    Assert-E2E ($flatRight -gt $flatLeft) 'Graph oracle expected flat interval is empty.'
    $series = @(Get-E2EGraphOracleSeries)
    $selected = @{}
    foreach ($item in $series) {
        $candidates = @(Get-E2EGraphFlatLineCandidates -Bitmap $Bitmap -Left $Left -Top $Top -Right $Right -Bottom $Bottom -Series $item -Background $idleBackground -FlatStartFraction $flatStartFraction -FlatEndFraction $flatEndFraction -FlatCoverageThreshold $flatCoverageThreshold)
        Assert-E2E ($candidates.Count -gt 0) "Past graph $($item.Name) has no compositor-aware horizontal flat corridor with $([int]($flatCoverageThreshold * 100))% coverage."
        $selected[$item.Name] = $candidates | Sort-Object Coverage, RowSpan -Descending | Select-Object -First 1
    }

    $minimumSeparation = [Math]::Max(4, [int][Math]::Ceiling($height * 0.02))
    $lunaCenter = [int]$selected['LUNA'].Center
    $terraCenter = [int]$selected['TERRA'].Center
    $solCenter = [int]$selected['SOL'].Center
    Assert-E2E ($lunaCenter + $minimumSeparation -le $terraCenter -and
        $terraCenter + $minimumSeparation -le $solCenter) "Past graph discovered model rows have invalid vertical order/separation: LUNA=$lunaCenter TERRA=$terraCenter SOL=$solCenter minimum-separation=$minimumSeparation."
    $uniqueCenters = @(@($lunaCenter, $terraCenter, $solCenter) | Select-Object -Unique)
    Assert-E2E ($uniqueCenters.Count -eq 3) "Past graph model rows share one physical y group: LUNA=$lunaCenter TERRA=$terraCenter SOL=$solCenter."
    foreach ($name in @('LUNA', 'TERRA', 'SOL')) {
        $center = [int]$selected[$name].Center
        Assert-E2E ($center -ge $Top -and $center -lt $Bottom) "Past graph $name discovered flat row is outside plot bounds: row=$center bounds=$Top..$($Bottom - 1)."
    }

    $sharedStep = Find-E2EGraphSharedRisingSegment -Bitmap $Bitmap -Left $Left -Top $Top -Right $Right -Bottom $Bottom -Series $series -FlatRows $selected -RisingCenterFraction $risingCenterFraction
    $results = @()
    foreach ($item in $series) {
        $flat = $selected[$item.Name]
        $contribution = $sharedStep.Contributions[$item.Name]
        $results += [pscustomobject]@{
            Name = $item.Name
            FlatCoverageColumns = $flat.CoverageColumns
            FlatWidth = $flat.Width
            FlatCoverage = $flat.Coverage
            FlatRowTop = $flat.RowTop
            FlatRowBottom = $flat.RowBottom
            FlatCenter = $flat.Center
            RisingColumns = $sharedStep.Columns
            RisingPixelHits = $sharedStep.PixelHits
            RisingTop = $sharedStep.Top
            RisingBottom = $sharedStep.Bottom
            RisingDensity = $sharedStep.Density
            RisingContributionPixels = $contribution.Pixels
            RisingContributionExtent = $contribution.Extent
        }
    }
    return $results
}

function Assert-E2EGraphHasModelData {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Plot,
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][psobject]$Capture
    )

    $window = Get-E2EWindowBounds $Handle
    $plotBounds = $Plot.Current.BoundingRectangle
    $bitmap = [System.Drawing.Bitmap]::FromFile($Capture.Path)
    try {
        [int]$left = [Math]::Max(0, [int]($plotBounds.Left - $window.Left))
        [int]$top = [Math]::Max(0, [int]($plotBounds.Top - $window.Top))
        [int]$right = [Math]::Min($bitmap.Width, [int]($plotBounds.Right - $window.Left))
        [int]$bottom = [Math]::Min($bitmap.Height, [int]($plotBounds.Bottom - $window.Top))
        Assert-E2E ($right -gt $left -and $bottom -gt $top) 'Graph plot bounds are outside the captured window.'
        $results = @(Assert-E2EGraphModelPixels -Bitmap $bitmap -Left $left -Top $top -Right $right -Bottom $bottom)
    }
    finally {
        $bitmap.Dispose()
    }
    $terra = $results | Where-Object { $_.Name -eq 'TERRA' }
    Write-E2E ("graph-past-model-data: PASS flat coverage LUNA={0}/{1}({2:P1}) TERRA={3}/{4}({5:P1}) SOL={6}/{7}({8:P1}); rising columns LUNA={9} TERRA={10} SOL={11}" -f
        ($results | Where-Object { $_.Name -eq 'LUNA' }).FlatCoverageColumns,
        ($results | Where-Object { $_.Name -eq 'LUNA' }).FlatWidth,
        ($results | Where-Object { $_.Name -eq 'LUNA' }).FlatCoverage,
        $terra.FlatCoverageColumns, $terra.FlatWidth, $terra.FlatCoverage,
        ($results | Where-Object { $_.Name -eq 'SOL' }).FlatCoverageColumns,
        ($results | Where-Object { $_.Name -eq 'SOL' }).FlatWidth,
        ($results | Where-Object { $_.Name -eq 'SOL' }).FlatCoverage,
        ($results | Where-Object { $_.Name -eq 'LUNA' }).RisingColumns,
        $terra.RisingColumns,
        ($results | Where-Object { $_.Name -eq 'SOL' }).RisingColumns)
}

function Test-E2EGraphIdleBandPixel {
    param(
        [Parameter(Mandatory = $true)][System.Drawing.Color]$Pixel
    )

    # #3F5D7C at opacity .22 over #101925 composites to #1A2838.  Compare
    # each channel against that computed composite so the plot surface
    # (#101925) and grid/axis (#263548) cannot satisfy the idle-band oracle.
    $expected = Get-E2EGraphIdleBackgroundColor
    $tolerance = 8
    $redDelta = [Math]::Abs([int]$Pixel.R - [int]$expected.R)
    $greenDelta = [Math]::Abs([int]$Pixel.G - [int]$expected.G)
    $blueDelta = [Math]::Abs([int]$Pixel.B - [int]$expected.B)
    return $redDelta -le $tolerance -and $greenDelta -le $tolerance -and $blueDelta -le $tolerance
}

function Assert-E2EGraphIdleBandBitmap {
    param(
        [Parameter(Mandatory = $true)][System.Drawing.Bitmap]$Bitmap,
        [Parameter(Mandatory = $true)][int]$Left,
        [Parameter(Mandatory = $true)][int]$Top,
        [Parameter(Mandatory = $true)][int]$Right,
        [Parameter(Mandatory = $true)][int]$Bottom,
        [double]$ExpectedStartFraction = 0.01,
        [double]$ExpectedEndFraction = 0.35,
        [int]$VerticalInset = 20
    )

    Assert-E2E ($Left -ge 0 -and $Top -ge 0 -and $Right -le $Bitmap.Width -and $Bottom -le $Bitmap.Height) `
        'Idle-band bitmap bounds are outside the captured bitmap.'
    Assert-E2E ($Right -gt $Left -and $Bottom -gt $Top) 'Idle-band bitmap bounds are empty.'
    Assert-E2E ($ExpectedStartFraction -ge 0 -and $ExpectedEndFraction -le 1 -and
        $ExpectedEndFraction -gt $ExpectedStartFraction) 'Idle-band expected range is invalid.'

    [int]$plotWidth = $Right - $Left
    [int]$plotHeight = $Bottom - $Top
    Assert-E2E ($VerticalInset -ge 0 -and ($plotHeight - (2 * $VerticalInset)) -gt 0) `
        'Idle-band vertical scan corridor is empty.'
    [int]$expectedLeft = $Left + [int][Math]::Floor($plotWidth * $ExpectedStartFraction)
    [int]$expectedRight = $Left + [int][Math]::Floor($plotWidth * $ExpectedEndFraction)
    Assert-E2E ($expectedRight -gt $expectedLeft) 'Idle-band expected range is sub-pixel.'
    [int]$expectedWidth = $expectedRight - $expectedLeft
    [int]$scanTop = $Top + $VerticalInset
    [int]$scanBottom = $Bottom - $VerticalInset
    [int]$scanHeight = $scanBottom - $scanTop
    [int]$minimumColumnHits = [Math]::Max(4, [int][Math]::Ceiling($scanHeight * 0.10))
    [int]$minimumSampleHits = [Math]::Max(20, [int][Math]::Ceiling($scanHeight * 0.10))
    [int]$minimumCoveredColumns = [int][Math]::Ceiling($expectedWidth * 0.80)

    [int]$hits = 0
    [int]$coveredColumns = 0
    $columnHits = @{}
    for ($x = $expectedLeft; $x -lt $expectedRight; $x++) {
        [int]$columnHitCount = 0
        for ($y = $scanTop; $y -lt $scanBottom; $y++) {
            $pixel = $Bitmap.GetPixel($x, $y)
            if (Test-E2EGraphIdleBandPixel -Pixel $pixel) {
                $hits = $hits + 1
                $columnHitCount = $columnHitCount + 1
            }
        }
        $columnHits[$x] = $columnHitCount
        if ($columnHitCount -ge $minimumColumnHits) {
            $coveredColumns = $coveredColumns + 1
        }
    }

    # Keep every sample expression scalar.  A comma-terminated expression in
    # an @(...) literal is an Object[] in Windows PowerShell and can make the
    # subsequent addition fail with "does not contain op_Addition".
    [int]$sampleOffset25 = [int][Math]::Floor($expectedWidth * 0.25)
    [int]$sampleOffset50 = [int][Math]::Floor($expectedWidth * 0.50)
    [int]$sampleOffset75 = [int][Math]::Floor($expectedWidth * 0.75)
    [int]$sampleColumn25 = $expectedLeft + $sampleOffset25
    [int]$sampleColumn50 = $expectedLeft + $sampleOffset50
    [int]$sampleColumn75 = $expectedLeft + $sampleOffset75
    foreach ($column in @($sampleColumn25, $sampleColumn50, $sampleColumn75)) {
        [int]$columnHitCount = [int]$columnHits[[int]$column]
        Assert-E2E ($columnHitCount -ge $minimumSampleHits) `
            "Past graph idle-band color is missing at expected x=$column (hits=$columnHitCount minimum=$minimumSampleHits)."
    }
    Assert-E2E ($coveredColumns -ge $minimumCoveredColumns) `
        "Past graph idle-band has insufficient horizontal coverage (columns=$coveredColumns expected>=$minimumCoveredColumns)."
    [int]$minimumTotalHits = [Math]::Max(100, $minimumCoveredColumns * $minimumColumnHits)
    Assert-E2E ($hits -ge $minimumTotalHits) `
        "Past graph has insufficient idle-band color pixels in the expected interval (hits=$hits expected>=$minimumTotalHits)."

    return [pscustomobject]@{
        Hits = $hits
        CoveredColumns = $coveredColumns
        ExpectedColumns = $expectedWidth
        SampleColumns = @($sampleColumn25, $sampleColumn50, $sampleColumn75)
        MinimumColumnHits = $minimumColumnHits
        MinimumSampleHits = $minimumSampleHits
        ScanHeight = $scanHeight
    }
}

function Assert-E2EGraphHasIdleBand {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Plot,
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][psobject]$Capture,
        [double]$ExpectedStartFraction = 0.01,
        [double]$ExpectedEndFraction = 0.35
    )

    # UIA is responsible only for locating the plot rectangle.  The bounded
    # pixel contract is shared with the finite bitmap self-test below.
    $window = Get-E2EWindowBounds $Handle
    $plotBounds = $Plot.Current.BoundingRectangle
    $bitmap = [System.Drawing.Bitmap]::FromFile($Capture.Path)
    try {
        [int]$left = [Math]::Max(0, [int]($plotBounds.Left - $window.Left))
        [int]$top = [Math]::Max(0, [int]($plotBounds.Top - $window.Top))
        [int]$right = [Math]::Min($bitmap.Width, [int]($plotBounds.Right - $window.Left))
        [int]$bottom = [Math]::Min($bitmap.Height, [int]($plotBounds.Bottom - $window.Top))
        $result = Assert-E2EGraphIdleBandBitmap -Bitmap $bitmap -Left $left -Top $top -Right $right -Bottom $bottom `
            -ExpectedStartFraction $ExpectedStartFraction -ExpectedEndFraction $ExpectedEndFraction
    }
    finally {
        $bitmap.Dispose()
    }
    Write-E2E ("graph-past-idle-band: PASS pixels={0} columns={1}/{2} range={3}-{4} color=#3F5D7C opacity=0.22" -f
        $result.Hits, $result.CoveredColumns, $result.ExpectedColumns, $ExpectedStartFraction, $ExpectedEndFraction)
}

function Assert-E2EQuotaGaugePalette {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][psobject]$Capture
    )

    $gauge = Find-E2EElementByAutomationId $Root 'Main.QuotaPeriodGauge'
    Assert-E2E ($null -ne $gauge) 'Main quota period gauge is missing.'
    $rectangle = $gauge.Current.BoundingRectangle
    Assert-E2E (-not $gauge.Current.IsOffscreen -and $rectangle.Width -gt 70 -and $rectangle.Height -ge 8) `
        'Main quota period gauge has invalid rendered bounds.'
    $window = Get-E2EWindowBounds $Handle
    $cellWidth = $rectangle.Width / 7.0
    $sampleY = [int][Math]::Floor($rectangle.Top - $window.Top + $rectangle.Height / 2.0)
    # The fixture has one half of its reset window remaining.  These samples
    # cover full, fractional, and empty cells away from the one-pixel boundary.
    $samples = @(
        @{ name = 'full-1'; cell = 0; fraction = 0.50; expected = '#56B2F5' },
        @{ name = 'full-3'; cell = 2; fraction = 0.50; expected = '#56B2F5' },
        @{ name = 'partial-filled'; cell = 3; fraction = 0.25; expected = '#56B2F5' },
        @{ name = 'partial-unfilled'; cell = 3; fraction = 0.75; expected = '#326799' },
        @{ name = 'empty-5'; cell = 4; fraction = 0.50; expected = '#326799' },
        @{ name = 'empty-7'; cell = 6; fraction = 0.50; expected = '#326799' }
    )
    $bitmap = [System.Drawing.Bitmap]::FromFile($Capture.Path)
    try {
        foreach ($sample in $samples) {
            $sampleX = [int][Math]::Floor(
                $rectangle.Left - $window.Left + ($sample.cell + $sample.fraction) * $cellWidth)
            Assert-E2E ($sampleX -ge 0 -and $sampleX -lt $bitmap.Width -and
                $sampleY -ge 0 -and $sampleY -lt $bitmap.Height) `
                "Quota palette sample '$($sample.name)' is outside the captured window."
            $pixel = $bitmap.GetPixel($sampleX, $sampleY)
            $actual = '#{0:X2}{1:X2}{2:X2}' -f $pixel.R, $pixel.G, $pixel.B
            Assert-E2E ($actual -eq $sample.expected) `
                "Quota palette sample '$($sample.name)' is $actual, expected $($sample.expected)."
        }
    }
    finally {
        $bitmap.Dispose()
    }
    Write-E2E 'main-quota-gauge: seven cells, two X-authority surface colors, and half-period boundary PASS'
}

function Get-E2EFixtureHeaderValues {
    param(
        [Parameter(Mandatory = $true)][psobject]$Response,
        [Parameter(Mandatory = $true)][string]$Name
    )

    if ($null -eq $Response.Headers) {
        return @()
    }
    foreach ($key in $Response.Headers.Keys) {
        if ([StringComparer]::OrdinalIgnoreCase.Equals([string]$key, $Name)) {
            return @($Response.Headers[$key])
        }
    }
    return @()
}

function Invoke-E2EFixtureRawRequest {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('/v1/health', '/v1/status', '/v1/details')]
        [string]$Path
    )

    $request = [System.Net.HttpWebRequest]::Create("http://127.0.0.1:$($script:e2eFixturePort)$Path")
    $request.Method = 'GET'
    $request.KeepAlive = $false
    $request.Headers.Add('X-Codex-Info-E2E-Phase', 'preflight')
    $response = $null
    $stream = $null
    $reader = $null
    try {
        try {
            $response = [System.Net.HttpWebResponse]$request.GetResponse()
        }
        catch [System.Net.WebException] {
            if ($null -eq $_.Exception.Response) {
                throw
            }
            $response = [System.Net.HttpWebResponse]$_.Exception.Response
        }
        $stream = $response.GetResponseStream()
        $reader = [System.IO.StreamReader]::new($stream, [System.Text.Encoding]::UTF8)
        $body = $reader.ReadToEnd()
        $headers = [ordered]@{}
        foreach ($headerName in $response.Headers.AllKeys) {
            $headers[$headerName] = @($response.Headers.GetValues($headerName))
        }
        return [pscustomobject]@{
            Path = $Path
            StatusCode = [int]$response.StatusCode
            StatusDescription = [string]$response.StatusDescription
            Headers = $headers
            Body = $body
        }
    }
    finally {
        if ($null -ne $reader) { $reader.Dispose() }
        if ($null -ne $stream) { $stream.Dispose() }
        if ($null -ne $response) { $response.Dispose() }
    }
}

function Assert-E2EFixtureJsonKeys {
    param(
        [Parameter(Mandatory = $true)][psobject]$Json,
        [Parameter(Mandatory = $true)][string[]]$Expected,
        [Parameter(Mandatory = $true)][string]$Endpoint
    )

    $actual = @($Json.PSObject.Properties | ForEach-Object { [string]$_.Name })
    $missing = @($Expected | Where-Object { $actual -notcontains $_ })
    $unexpected = @($actual | Where-Object { $Expected -notcontains $_ })
    Assert-E2E ($actual.Count -eq $Expected.Count -and $missing.Count -eq 0 -and $unexpected.Count -eq 0) "$Endpoint top-level keys are not exact: actual=$($actual -join ',') missing=$($missing -join ',') unexpected=$($unexpected -join ',')"
}

function Assert-E2EFixtureNumericProperty {
    param(
        [Parameter(Mandatory = $true)][psobject]$Json,
        [Parameter(Mandatory = $true)][string]$Name,
        [switch]$Integer,
        [double]$Minimum = 0,
        [double]$Maximum = [double]::PositiveInfinity
    )

    $property = $Json.PSObject.Properties[$Name]
    Assert-E2E ($null -ne $property) "Fixture numeric property is missing: $Name."
    $value = $property.Value
    $isNumeric = $value -is [System.Byte] -or $value -is [System.SByte] -or
        $value -is [System.Int16] -or $value -is [System.UInt16] -or
        $value -is [System.Int32] -or $value -is [System.UInt32] -or
        $value -is [System.Int64] -or $value -is [System.UInt64] -or
        $value -is [System.Single] -or $value -is [System.Double] -or
        $value -is [System.Decimal]
    Assert-E2E $isNumeric "Fixture numeric property has an invalid JSON number kind: $Name."
    $number = [double]$value
    Assert-E2E (-not [double]::IsNaN($number) -and -not [double]::IsInfinity($number)) "Fixture numeric property is not finite: $Name."
    Assert-E2E ($number -ge $Minimum -and $number -le $Maximum) "Fixture numeric property is outside its range: $Name=$number."
    if ($Integer) {
        Assert-E2E ($number -le [double][Int64]::MaxValue) "Fixture integer property exceeds Int64 range: $Name=$number."
        Assert-E2E ($number -eq [Math]::Truncate($number)) "Fixture integer property is not integral: $Name=$number."
        return [Int64]$number
    }
    return $number
}

function Assert-E2EFixtureHistorySamples {
    param([Parameter(Mandatory = $true)][psobject]$DetailsJson)

    $historyGapsProperty = @($DetailsJson.PSObject.Properties | Where-Object { $_.Name -eq 'history_gaps' })
    Assert-E2E ($historyGapsProperty.Count -eq 1 -and $historyGapsProperty[0].Value -is [System.Array]) 'Fixture details history_gaps must be an array.'
    $periodsProperty = @($DetailsJson.PSObject.Properties | Where-Object { $_.Name -eq 'history_periods' })
    Assert-E2E ($periodsProperty.Count -eq 1 -and $periodsProperty[0].Value -is [System.Array]) 'Fixture details history_periods must be an array.'
    $periods = @($DetailsJson.history_periods)
    Assert-E2E ($periods.Count -eq 2) "Fixture details history_periods count changed: $($periods.Count)."
    $expectedPeriodKeys = @('id', 'start_at', 'end_at', 'reset_at', 'label', 'current')
    $periodRecords = @()
    $periodIds = @{}
    $periodResets = @{}
    $currentPeriodCount = 0
    foreach ($period in $periods) {
        Assert-E2EFixtureJsonKeys -Json $period -Expected $expectedPeriodKeys -Endpoint 'Fixture history period'
        $idProperty = $period.PSObject.Properties['id']
        $labelProperty = $period.PSObject.Properties['label']
        $currentProperty = $period.PSObject.Properties['current']
        Assert-E2E ($null -ne $idProperty -and $idProperty.Value -is [string] -and -not [string]::IsNullOrWhiteSpace([string]$idProperty.Value)) 'Fixture history period id must be a non-empty string.'
        Assert-E2E ($null -ne $labelProperty -and $labelProperty.Value -is [string] -and -not [string]::IsNullOrWhiteSpace([string]$labelProperty.Value)) 'Fixture history period label must be a non-empty string.'
        Assert-E2E ($null -ne $currentProperty -and $currentProperty.Value -is [bool]) 'Fixture history period current must be boolean.'
        $periodId = [string]$idProperty.Value
        Assert-E2E (-not $periodIds.ContainsKey($periodId)) "Fixture history period id is duplicated: $periodId."
        $periodIds[$periodId] = $true
        $periodStart = Assert-E2EFixtureNumericProperty -Json $period -Name 'start_at' -Integer
        $periodEnd = Assert-E2EFixtureNumericProperty -Json $period -Name 'end_at' -Integer
        $periodReset = Assert-E2EFixtureNumericProperty -Json $period -Name 'reset_at' -Integer
        Assert-E2E ($periodEnd -ge $periodStart) "Fixture history period bounds are inverted: $periodId."
        Assert-E2E ($periodReset -gt $periodStart) "Fixture history period reset_at is not after start_at: $periodId."
        Assert-E2E (-not $periodResets.ContainsKey([string]$periodReset)) "Fixture history period reset identity is duplicated: $periodReset."
        $periodResets[[string]$periodReset] = $true
        if ([bool]$currentProperty.Value) { $currentPeriodCount++ }
        $periodRecords += [pscustomobject]@{
            Id = $periodId
            StartAt = $periodStart
            EndAt = $periodEnd
            ResetAt = $periodReset
        }
    }
    Assert-E2E ($currentPeriodCount -eq 1) "Fixture history periods must have exactly one current period: count=$currentPeriodCount."

    $samplesProperty = @($DetailsJson.PSObject.Properties | Where-Object { $_.Name -eq 'history_samples' })
    Assert-E2E ($samplesProperty.Count -eq 1 -and $samplesProperty[0].Value -is [System.Array]) 'Fixture details history_samples must be an array.'
    $samples = @($DetailsJson.history_samples)
    Assert-E2E ($samples.Count -eq 5) "Fixture details history_samples count changed: $($samples.Count)."
    $expectedSampleKeys = @(
        'timestamp', 'reset_at', 'remaining_percent',
        'sol_dollars', 'terra_dollars', 'luna_dollars',
        'sol_tokens', 'terra_tokens', 'luna_tokens'
    )
    $seenSampleKeys = @{}
    $hasPreviousSample = $false
    $previousReset = [Int64]::MinValue
    $previousTimestamp = [Int64]::MinValue
    foreach ($sample in $samples) {
        Assert-E2EFixtureJsonKeys -Json $sample -Expected $expectedSampleKeys -Endpoint 'Fixture history sample'
        $timestamp = Assert-E2EFixtureNumericProperty -Json $sample -Name 'timestamp' -Integer
        $reset = Assert-E2EFixtureNumericProperty -Json $sample -Name 'reset_at' -Integer
        $null = Assert-E2EFixtureNumericProperty -Json $sample -Name 'remaining_percent' -Minimum 0 -Maximum 100
        $null = Assert-E2EFixtureNumericProperty -Json $sample -Name 'sol_dollars' -Minimum 0
        $null = Assert-E2EFixtureNumericProperty -Json $sample -Name 'terra_dollars' -Minimum 0
        $null = Assert-E2EFixtureNumericProperty -Json $sample -Name 'luna_dollars' -Minimum 0
        $null = Assert-E2EFixtureNumericProperty -Json $sample -Name 'sol_tokens' -Integer
        $null = Assert-E2EFixtureNumericProperty -Json $sample -Name 'terra_tokens' -Integer
        $null = Assert-E2EFixtureNumericProperty -Json $sample -Name 'luna_tokens' -Integer
        Assert-E2E (($timestamp % 60) -eq 0) "Fixture history sample timestamp must be minute bucket aligned (timestamp % 60 == 0): $timestamp."
        $matchingPeriods = @($periodRecords | Where-Object { $_.ResetAt -eq $reset })
        Assert-E2E ($matchingPeriods.Count -eq 1) "Fixture history sample reset_at has no unique period identity: $reset."
        $period = $matchingPeriods[0]
        Assert-E2E ($timestamp -ge $period.StartAt -and $timestamp -le $period.EndAt) "Fixture history sample timestamp is outside its period bounds: period=$($period.Id) timestamp=$timestamp."
        $sampleKey = '{0}:{1}' -f $reset, $timestamp
        Assert-E2E (-not $seenSampleKeys.ContainsKey($sampleKey)) "Fixture history sample identity is duplicated: $sampleKey."
        $seenSampleKeys[$sampleKey] = $true
        if ($hasPreviousSample) {
            Assert-E2E ($reset -gt $previousReset -or ($reset -eq $previousReset -and $timestamp -ge $previousTimestamp)) "Fixture history_samples are not ordered by (reset_at,timestamp): previous=($previousReset,$previousTimestamp) current=($reset,$timestamp)."
        }
        $previousReset = $reset
        $previousTimestamp = $timestamp
        $hasPreviousSample = $true
    }
    return $true
}

function Assert-E2EFixtureWireContract {
    param(
        [Parameter(Mandatory = $true)][psobject]$Health,
        [Parameter(Mandatory = $true)][psobject]$Status,
        [Parameter(Mandatory = $true)][psobject]$Details
    )

    Assert-E2E ($Health.StatusCode -eq 200) "Fixture health status is not HTTP 200: $($Health.StatusCode)."
    Assert-E2E ($Status.StatusCode -eq 200) "Fixture status status is not HTTP 200: $($Status.StatusCode)."
    Assert-E2E ($Details.StatusCode -eq 200) "Fixture details status is not HTTP 200: $($Details.StatusCode)."

    $healthPairs = @(Get-E2EFixtureHeaderValues -Response $Health -Name 'Codex-Info-Published-Pair')
    $statusPairs = @(Get-E2EFixtureHeaderValues -Response $Status -Name 'Codex-Info-Published-Pair')
    $detailsPairs = @(Get-E2EFixtureHeaderValues -Response $Details -Name 'Codex-Info-Published-Pair')
    Assert-E2E ($healthPairs.Count -eq 0) "Fixture health must not expose Codex-Info-Published-Pair: count=$($healthPairs.Count)."
    Assert-E2E ($statusPairs.Count -eq 1) "Fixture status must expose exactly one Codex-Info-Published-Pair: count=$($statusPairs.Count)."
    Assert-E2E ($detailsPairs.Count -eq 1) "Fixture details must expose exactly one Codex-Info-Published-Pair: count=$($detailsPairs.Count)."
    $statusPair = [string]$statusPairs[0]
    $detailsPair = [string]$detailsPairs[0]
    Assert-E2E ($statusPair -cmatch '^v1:[0-9a-f]{64}$') "Fixture status published pair is not canonical lowercase v1/sha256: '$statusPair'."
    Assert-E2E ($detailsPair -cmatch '^v1:[0-9a-f]{64}$') "Fixture details published pair is not canonical lowercase v1/sha256: '$detailsPair'."
    Assert-E2E ($statusPair -ceq $detailsPair) 'Fixture status/details published pair mismatch.'

    try {
        $statusJson = ConvertFrom-Json -InputObject ([string]$Status.Body)
    }
    catch {
        throw "Fixture status body is not valid JSON: $($_.Exception.Message)"
    }
    try {
        $detailsJson = ConvertFrom-Json -InputObject ([string]$Details.Body)
    }
    catch {
        throw "Fixture details body is not valid JSON: $($_.Exception.Message)"
    }
    $expectedStatusKeys = @(
        'api_version', 'state', 'observed_at', 'authenticated',
        'plan_label', 'quota', 'models', 'active_thread_count'
    )
    $expectedDetailsKeys = @(
        'api_version', 'state', 'observed_at', 'authenticated',
        'plan_label', 'quota', 'models', 'active_thread_count',
        'history_periods', 'history_gaps', 'history_samples',
        'threads', 'estimated_cost_label'
    )
    Assert-E2EFixtureJsonKeys -Json $statusJson -Expected $expectedStatusKeys -Endpoint 'Fixture status'
    Assert-E2EFixtureJsonKeys -Json $detailsJson -Expected $expectedDetailsKeys -Endpoint 'Fixture details'

    Assert-E2EFixtureHistorySamples -DetailsJson $detailsJson | Out-Null
    return $true
}

function Assert-E2EFixturePreflightResponses {
    param(
        [Parameter(Mandatory = $true)][psobject]$Health,
        [Parameter(Mandatory = $true)][psobject]$Status,
        [Parameter(Mandatory = $true)][psobject]$Details
    )
    Assert-E2EFixtureWireContract -Health $Health -Status $Status -Details $Details | Out-Null
    return $true
}

function Invoke-E2EFixturePreflight {
    $responses = [ordered]@{}
    foreach ($requestSpec in @(
            @{ Name = 'health'; Path = '/v1/health' },
            @{ Name = 'status'; Path = '/v1/status' },
            @{ Name = 'details'; Path = '/v1/details' })) {
        $response = Invoke-E2EFixtureRawRequest -Path $requestSpec.Path
        $responses[$requestSpec.Name] = $response
        $pairCount = @(Get-E2EFixtureHeaderValues -Response $response -Name 'Codex-Info-Published-Pair').Count
        $rawPath = Join-Path $script:e2eOutput ("fixture-preflight-{0}.raw.json" -f $requestSpec.Name)
        [IO.File]::WriteAllText($rawPath, [string]$response.Body, [Text.UTF8Encoding]::new($false))
        Write-E2E ("fixture-preflight: request={0} status={1} pair-count={2} body-bytes={3} raw={4}" -f
            $requestSpec.Name, $response.StatusCode, $pairCount, ([Text.Encoding]::UTF8.GetByteCount([string]$response.Body)), $rawPath)
    }
    Assert-E2EFixturePreflightResponses -Health $responses['health'] -Status $responses['status'] -Details $responses['details'] | Out-Null
    Write-E2E 'fixture-preflight: PASS (health/status/details raw responses satisfy strict wire contract)'
    return [pscustomobject]@{
        Health = $responses['health']
        Status = $responses['status']
        Details = $responses['details']
    }
}

function New-E2EFixtureDocuments {
    $rawNow = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $now = $rawNow - ($rawNow % 60)
    $currentStart = $now - 7200
    $currentReset = $now + 7200
    $pastStart = $now - 25200
    $pastReset = $now - 14400
    $publishedPair = 'v1:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
    $status = @"
{"api_version":"v1","state":"ready","observed_at":$now,"authenticated":true,"plan_label":"Pro","quota":{"remaining_percent":72.0,"reset_at":$currentReset,"window_seconds":14400,"monthly":false},"models":[{"name":"SOL","input_tokens":1200,"cached_input_tokens":200,"output_tokens":400},{"name":"TERRA","input_tokens":2400,"cached_input_tokens":500,"output_tokens":800},{"name":"LUNA","input_tokens":3600,"cached_input_tokens":700,"output_tokens":1100}],"active_thread_count":3}
"@
    # Keep this wire fixture as explicit JSON.  The details endpoint is a
    # strict thirteen-field contract; serializing nested PowerShell dictionaries
    # can silently change null/number kinds between Windows PowerShell builds.
    $details = @"
{"api_version":"v1","state":"ready","observed_at":$now,"authenticated":true,"plan_label":"Pro","quota":{"remaining_percent":72.0,"reset_at":$currentReset,"window_seconds":14400,"monthly":false},"models":[{"name":"SOL","input_tokens":1200,"cached_input_tokens":200,"output_tokens":400,"input_dollars":1.20,"cached_input_dollars":0.20,"output_dollars":0.40},{"name":"TERRA","input_tokens":2400,"cached_input_tokens":500,"output_tokens":800,"input_dollars":2.40,"cached_input_dollars":0.50,"output_dollars":0.80},{"name":"LUNA","input_tokens":3600,"cached_input_tokens":700,"output_tokens":1100,"input_dollars":3.60,"cached_input_dollars":0.70,"output_dollars":1.10}],"active_thread_count":3,"history_periods":[{"id":"e2e-current","start_at":$currentStart,"end_at":$now,"reset_at":$currentReset,"label":"Current period","current":true},{"id":"e2e-past","start_at":$pastStart,"end_at":$pastReset,"reset_at":$pastReset,"label":"Past period","current":false}],"history_samples":[{"timestamp":$($currentStart + 60),"reset_at":$currentReset,"remaining_percent":92.0,"sol_dollars":0.25,"terra_dollars":0.50,"luna_dollars":0.75,"sol_tokens":100,"terra_tokens":200,"luna_tokens":300},{"timestamp":$($now - 60),"reset_at":$currentReset,"remaining_percent":72.0,"sol_dollars":1.20,"terra_dollars":2.40,"luna_dollars":3.60,"sol_tokens":1200,"terra_tokens":2400,"luna_tokens":3600},{"timestamp":$($pastStart + 60),"reset_at":$pastReset,"remaining_percent":98.0,"sol_dollars":0.10,"terra_dollars":0.20,"luna_dollars":0.30,"sol_tokens":50,"terra_tokens":100,"luna_tokens":150},{"timestamp":$($pastStart + 3600),"reset_at":$pastReset,"remaining_percent":98.0,"sol_dollars":0.10,"terra_dollars":0.20,"luna_dollars":0.30,"sol_tokens":50,"terra_tokens":100,"luna_tokens":150},{"timestamp":$($pastReset - 60),"reset_at":$pastReset,"remaining_percent":84.0,"sol_dollars":0.60,"terra_dollars":1.20,"luna_dollars":1.80,"sol_tokens":600,"terra_tokens":1200,"luna_tokens":1800}],"threads":[{"id":"e2e-root","title":"E2E root task","parent_thread_id":null,"model":"TERRA","model_label":"TERRA","total_tokens":2400,"context_usage_tokens":800,"context_window_tokens":16000,"created_at":$($now - 3600),"last_user_message_at":$($now - 300),"is_subagent":false,"depth":0},{"id":"e2e-child","title":"E2E child task","parent_thread_id":"e2e-root","model":"LUNA","model_label":"LUNA","total_tokens":1200,"context_usage_tokens":400,"context_window_tokens":16000,"created_at":$($now - 2400),"last_user_message_at":$($now - 600),"is_subagent":true,"depth":1},{"id":"e2e-orphan","title":"E2E orphan task","parent_thread_id":"missing-parent","model":"SOL","model_label":"SOL","total_tokens":600,"context_usage_tokens":null,"context_window_tokens":null,"created_at":$($now - 1200),"last_user_message_at":null,"is_subagent":true,"depth":null}],"estimated_cost_label":"USD 12.34"}
"@
    # Keep the explicit sample values above while enforcing the wire order
    # independently of PowerShell object serialization: past -> current.
    $details = $details.Replace(',"history_samples":[', ',"history_gaps":[],"history_samples":[')
    $historySamplesMarker = '"history_samples":['
    $historySamplesStart = $details.IndexOf($historySamplesMarker)
    $historySamplesEnd = $details.IndexOf('],"threads"', $historySamplesStart)
    if ($historySamplesStart -lt 0 -or $historySamplesEnd -lt 0) {
        throw 'Fixture details history_samples boundary is missing.'
    }
    $historySamplesBodyStart = $historySamplesStart + $historySamplesMarker.Length
    $historySamplesBody = $details.Substring($historySamplesBodyStart, $historySamplesEnd - $historySamplesBodyStart)
    $sampleObjects = @([regex]::Matches($historySamplesBody, '\{[^{}]+\}') | ForEach-Object { $_.Value })
    if ($sampleObjects.Count -ne 5) {
        throw "Fixture details history_samples expected five objects, found $($sampleObjects.Count)."
    }
    $orderedSampleObjects = @(
        $sampleObjects[2], $sampleObjects[3], $sampleObjects[4],
        $sampleObjects[0], $sampleObjects[1]
    )
    $details = $details.Substring(0, $historySamplesBodyStart) +
        ($orderedSampleObjects -join ',') +
        $details.Substring($historySamplesEnd)
    return [pscustomobject]@{
        Status = $status.Trim()
        Details = $details.Trim()
        PublishedPair = $publishedPair
        Now = $now
    }
}

function Enter-E2EFixture {
    $documents = New-E2EFixtureDocuments
    [IO.File]::WriteAllText(
        (Join-Path $script:e2eOutput 'fixture-status.json'),
        $documents.Status,
        [Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText(
        (Join-Path $script:e2eOutput 'fixture-details.json'),
        $documents.Details,
        [Text.UTF8Encoding]::new($false))
    if (Test-Path -LiteralPath $script:e2eSettingsPath -PathType Leaf) {
        $script:e2eSettingsWasPresent = $true
        Copy-Item -LiteralPath $script:e2eSettingsPath -Destination $script:e2eSettingsBackup -Force
    }
    $settingsDirectory = Split-Path -Parent $script:e2eSettingsPath
    New-Item -ItemType Directory -Path $settingsDirectory -Force | Out-Null
    $settingsJson = '{"language":"en","setupCompleted":true,"connectionConfigured":true,"timeZoneId":"UTC","connectionProfile":"none","connectionSelector":"none"}'
    [IO.File]::WriteAllText($script:e2eSettingsPath, $settingsJson, [Text.UTF8Encoding]::new($false))
    Assert-E2E ([CodexInfoWindowsE2EFixtureServer]::Start($documents.Status, $documents.Details, $documents.PublishedPair, $script:e2eFixturePort)) "Could not bind the fixture to loopback port $script:e2eFixturePort."
    $script:e2eFixtureRunning = $true
    Write-E2E "fixture: PASS periods=2 threads=3 endpoint=http://127.0.0.1:$script:e2eFixturePort"
}

function Exit-E2EFixture {
    if ($script:e2eFixtureRunning) {
        [CodexInfoWindowsE2EFixtureServer]::Stop()
        $script:e2eFixtureRunning = $false
    }
    if ($script:e2eSettingsWasPresent) {
        Copy-Item -LiteralPath $script:e2eSettingsBackup -Destination $script:e2eSettingsPath -Force
    }
    elseif (Test-Path -LiteralPath $script:e2eSettingsPath -PathType Leaf) {
        Remove-Item -LiteralPath $script:e2eSettingsPath -Force
    }
    if (Test-Path -LiteralPath $script:e2eSettingsBackup -PathType Leaf) {
        Remove-Item -LiteralPath $script:e2eSettingsBackup -Force
    }
}

function New-E2EContractTestResponse {
    param(
        [Parameter(Mandatory = $true)][int]$StatusCode,
        [Parameter(Mandatory = $true)][string]$Body,
        [Parameter(Mandatory = $true)][System.Collections.IDictionary]$Headers
    )
    return [pscustomobject]@{
        StatusCode = $StatusCode
        Headers = $Headers
        Body = $Body
    }
}

function Assert-E2EExpectedContractFailure {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][psobject]$Health,
        [Parameter(Mandatory = $true)][psobject]$Status,
        [Parameter(Mandatory = $true)][psobject]$Details,
        [string]$ExpectedReason = ''
    )
    try {
        Assert-E2EFixturePreflightResponses -Health $Health -Status $Status -Details $Details | Out-Null
    }
    catch {
        if (-not [string]::IsNullOrWhiteSpace($ExpectedReason) -and $_.Exception.Message -notlike "*$ExpectedReason*") {
            throw "fixture-contract-negative: FAIL case=$Name returned an unexpected reason: $($_.Exception.Message)"
        }
        Write-E2E "fixture-contract-negative: PASS case=$Name failure=$($_.Exception.Message)"
        return
    }
    throw "fixture-contract-negative: FAIL case=$Name unexpectedly passed."
}

function New-E2EGraphOracleSyntheticBitmap {
    param(
        [ValidateSet('valid', 'valid-offset', 'missing-flat', 'label-vertical-only', 'wrong-geometry', 'wrong-order', 'sloped', 'wrong-background', 'axis-row-missing-terra', 'axis-row-missing-sol', 'wrong-endpoint', 'short-rise', 'detached-rise', 'missing-series-contribution', 'non-contiguous')]
        [string]$Variant = 'valid'
    )

    $width = 120
    $height = 100
    $bitmap = [System.Drawing.Bitmap]::new($width, $height)
    $idleBackground = Get-E2EGraphIdleBackgroundColor
    for ($x = 0; $x -lt $width; $x++) {
        for ($y = 0; $y -lt $height; $y++) {
            $bitmap.SetPixel($x, $y, $idleBackground)
        }
    }
    $flatLeft = [int][Math]::Floor($width * 0.06)
    $flatRight = [int][Math]::Floor($width * 0.84)
    $risingCenter = [int][Math]::Floor($width * 0.84)
    $syntheticLayout = @{
        LUNA = [pscustomobject]@{ FlatCenterFraction = 0.728; RiseTopFraction = 0.140 }
        TERRA = [pscustomobject]@{ FlatCenterFraction = 0.766; RiseTopFraction = 0.375 }
        SOL = [pscustomobject]@{ FlatCenterFraction = 0.806; RiseTopFraction = 0.610 }
    }
    $seriesIndex = 0
    foreach ($item in (Get-E2EGraphOracleSeries)) {
        $layout = $syntheticLayout[$item.Name]
        $flatCenter = [int][Math]::Round($height * $layout.FlatCenterFraction)
        if ($Variant -eq 'valid-offset') {
            $flatCenter += 14
        }
        if ($Variant -eq 'wrong-order') {
            if ($item.Name -eq 'LUNA') { $flatCenter = [int][Math]::Round($height * $syntheticLayout.SOL.FlatCenterFraction) }
            elseif ($item.Name -eq 'SOL') { $flatCenter = [int][Math]::Round($height * $syntheticLayout.LUNA.FlatCenterFraction) }
        }
        $drawFlat = $Variant -notin @('missing-flat', 'label-vertical-only')
        if ($Variant -eq 'wrong-geometry') {
            if ($item.Name -eq 'LUNA') { $flatCenter = [int][Math]::Round($height * 0.79) }
            elseif ($item.Name -eq 'TERRA') { $flatCenter = [int][Math]::Round($height * 0.80) }
            else { $flatCenter = [int][Math]::Round($height * 0.81) }
        }
        if ($drawFlat) {
            $flatColor = if ($Variant -eq 'wrong-background') {
                [System.Drawing.Color]::White
            }
            elseif ($Variant -eq 'axis-row-missing-terra' -and $item.Name -eq 'TERRA') {
                [System.Drawing.ColorTranslator]::FromHtml('#263548')
            }
            elseif ($Variant -eq 'axis-row-missing-sol' -and $item.Name -eq 'SOL') {
                [System.Drawing.ColorTranslator]::FromHtml('#263548')
            }
            elseif ($item.Name -eq 'TERRA') {
                Get-E2EGraphCompositedColor -Background $idleBackground -Series $item.Color -Alpha 0.25
            }
            else {
                Get-E2EGraphCompositedColor -Background $idleBackground -Series $item.Color -Alpha 0.45
            }
            for ($x = $flatLeft; $x -lt $flatRight; $x++) {
                if ($Variant -eq 'sloped') {
                    $lineCenter = $flatCenter + [int][Math]::Round((($x - $flatLeft) / [double]($flatRight - $flatLeft)) * 24)
                }
                else {
                    $lineCenter = $flatCenter
                }
                for ($offset = -1; $offset -le 1; $offset++) {
                    $lineY = $lineCenter + $offset
                    if ($lineY -ge 0 -and $lineY -lt $height) {
                        $bitmap.SetPixel($x, $lineY, $flatColor)
                    }
                }
            }
        }
        $drawRise = $Variant -ne 'missing-series-contribution' -or $item.Name -ne 'TERRA'
        $riseX = $risingCenter
        if ($Variant -in @('wrong-geometry', 'wrong-endpoint')) {
            $riseX = $flatLeft - 20 + $seriesIndex
        }
        $riseTop = [int][Math]::Floor($height * $layout.RiseTopFraction)
        if ($Variant -eq 'valid-offset') {
            $riseTop += 14
        }
        if ($Variant -eq 'short-rise') {
            $riseTop = $flatCenter - 8
        }
        if ($Variant -eq 'wrong-endpoint') {
            $riseTop = [int][Math]::Floor($height * $layout.RiseTopFraction)
        }
        $riseBottom = $flatCenter + 2
        if ($Variant -eq 'detached-rise') {
            $riseBottom = $flatCenter - 8
        }
        if ($drawRise) {
            for ($y = $riseTop; $y -le $riseBottom; $y++) {
                if ($riseX -ge 0 -and $riseX -lt $width -and $y -ge 0 -and $y -lt $height) {
                    $bitmap.SetPixel($riseX, $y, $item.Color)
                }
            }
            if ($riseX + 1 -ge 0 -and $riseX + 1 -lt $width) {
                for ($y = $riseTop; $y -le $riseBottom; $y++) {
                    if ($y -ge 0 -and $y -lt $height) {
                        $bitmap.SetPixel($riseX + 1, $y, $item.Color)
                    }
                }
            }
        }
        $seriesIndex++
    }
    if ($Variant -eq 'non-contiguous') {
        for ($x = $risingCenter; $x -le ($risingCenter + 1); $x++) {
            for ($y = 60; $y -le 70; $y++) {
                $bitmap.SetPixel($x, $y, $idleBackground)
            }
        }
    }
    return $bitmap
}

function New-E2EGraphIdleBandSyntheticBitmap {
    param(
        [ValidateSet('valid', 'no-band', 'wrong-color', 'too-short', 'narrow-band', 'wrong-interval', 'wrong-sample-columns')]
        [string]$Variant = 'valid'
    )

    [int]$width = 120
    [int]$height = 100
    $bitmap = [System.Drawing.Bitmap]::new($width, $height)
    $plotBackground = [System.Drawing.ColorTranslator]::FromHtml('#101925')
    $idleComposite = Get-E2EGraphIdleBackgroundColor
    $wrongColor = [System.Drawing.ColorTranslator]::FromHtml('#263548')
    for ($x = 0; $x -lt $width; $x++) {
        for ($y = 0; $y -lt $height; $y++) {
            $bitmap.SetPixel($x, $y, $plotBackground)
        }
    }

    [int]$expectedLeft = [int][Math]::Floor($width * 0.01)
    [int]$expectedRight = [int][Math]::Floor($width * 0.35)
    [int]$expectedWidth = $expectedRight - $expectedLeft
    [int]$scanTop = 20
    [int]$scanBottom = $height - 20
    [int]$sampleOffset25 = [int][Math]::Floor($expectedWidth * 0.25)
    [int]$sampleOffset50 = [int][Math]::Floor($expectedWidth * 0.50)
    [int]$sampleOffset75 = [int][Math]::Floor($expectedWidth * 0.75)
    [int]$sampleColumn25 = $expectedLeft + $sampleOffset25
    [int]$sampleColumn50 = $expectedLeft + $sampleOffset50
    [int]$sampleColumn75 = $expectedLeft + $sampleOffset75

    [bool]$drawBand = $Variant -ne 'no-band'
    [int]$bandLeft = $expectedLeft
    [int]$bandRight = $expectedRight
    [int]$bandTop = $scanTop
    [int]$bandBottom = $scanBottom
    $bandColor = $idleComposite
    switch ($Variant) {
        'wrong-color' {
            $bandColor = $wrongColor
        }
        'too-short' {
            $bandTop = $scanTop + 10
            $bandBottom = $bandTop + 4
        }
        'narrow-band' {
            $bandRight = $expectedLeft + [Math]::Max(1, [int][Math]::Floor($expectedWidth * 0.15))
        }
        'wrong-interval' {
            $bandLeft = [int][Math]::Floor($width * 0.55)
            $bandRight = [int][Math]::Floor($width * 0.90)
        }
    }
    if ($drawBand) {
        for ($x = $bandLeft; $x -lt $bandRight; $x++) {
            for ($y = $bandTop; $y -lt $bandBottom; $y++) {
                $bitmap.SetPixel($x, $y, $bandColor)
            }
        }
    }
    if ($Variant -eq 'wrong-sample-columns') {
        foreach ($sampleColumn in @($sampleColumn25, $sampleColumn50, $sampleColumn75)) {
            for ($y = $scanTop; $y -lt $scanBottom; $y++) {
                $bitmap.SetPixel([int]$sampleColumn, $y, $plotBackground)
            }
        }
    }
    return $bitmap
}

function Assert-E2EGraphIdleBandExpectedFailure {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][System.Drawing.Bitmap]$Bitmap
    )
    try {
        $null = Assert-E2EGraphIdleBandBitmap -Bitmap $Bitmap -Left 0 -Top 0 -Right $Bitmap.Width -Bottom $Bitmap.Height
    }
    catch {
        Write-E2E "graph-idle-band-self-test: PASS case=$Name failure=$($_.Exception.Message)"
        return
    }
    throw "graph-idle-band-self-test: FAIL case=$Name unexpectedly passed."
}

function Invoke-E2EGraphIdleBandSelfTest {
    $validBitmap = New-E2EGraphIdleBandSyntheticBitmap -Variant 'valid'
    try {
        $validResult = Assert-E2EGraphIdleBandBitmap -Bitmap $validBitmap -Left 0 -Top 0 -Right $validBitmap.Width -Bottom $validBitmap.Height
        Assert-E2E ($validResult.CoveredColumns -ge 32) 'Graph idle-band self-test did not establish meaningful column coverage.'
        Assert-E2E ($validResult.ScanHeight -ge 40) 'Graph idle-band self-test did not establish meaningful vertical coverage.'
        Write-E2E ("graph-idle-band-self-test: PASS valid composite=#1A2838 tolerance=8 pixels={0} columns={1}/{2} vertical={3}" -f
            $validResult.Hits, $validResult.CoveredColumns, $validResult.ExpectedColumns, $validResult.ScanHeight)
    }
    finally {
        $validBitmap.Dispose()
    }
    foreach ($variant in @('no-band', 'wrong-color', 'too-short', 'narrow-band', 'wrong-interval', 'wrong-sample-columns')) {
        $bitmap = New-E2EGraphIdleBandSyntheticBitmap -Variant $variant
        try {
            Assert-E2EGraphIdleBandExpectedFailure -Name $variant -Bitmap $bitmap
        }
        finally {
            $bitmap.Dispose()
        }
    }
    Write-E2E 'graph-idle-band-self-test: PASS one valid composite/range and six finite negative bitmap cases'
}

function Assert-E2EGraphOracleExpectedFailure {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][System.Drawing.Bitmap]$Bitmap
    )
    try {
        $null = @(Assert-E2EGraphModelPixels -Bitmap $Bitmap -Left 0 -Top 0 -Right $Bitmap.Width -Bottom $Bitmap.Height)
    }
    catch {
        Write-E2E "graph-oracle-self-test: PASS case=$Name failure=$($_.Exception.Message)"
        return
    }
    throw "graph-oracle-self-test: FAIL case=$Name unexpectedly passed."
}

function Invoke-E2EGraphOracleSelfTest {
    $validBitmap = New-E2EGraphOracleSyntheticBitmap -Variant 'valid'
    try {
        $validResults = @(Assert-E2EGraphModelPixels -Bitmap $validBitmap -Left 0 -Top 0 -Right $validBitmap.Width -Bottom $validBitmap.Height)
        Assert-E2E ($validResults.Count -eq 3) "Graph oracle self-test returned unexpected series count: $($validResults.Count)."
        Write-E2E ("graph-oracle-self-test: PASS valid series={0} flat-threshold=90% compositor=idle-background-to-series-alpha" -f $validResults.Count)
    }
    finally {
        $validBitmap.Dispose()
    }
    $offsetBitmap = New-E2EGraphOracleSyntheticBitmap -Variant 'valid-offset'
    try {
        $offsetResults = @(Assert-E2EGraphModelPixels -Bitmap $offsetBitmap -Left 0 -Top 0 -Right $offsetBitmap.Width -Bottom $offsetBitmap.Height)
        Assert-E2E ($offsetResults.Count -eq 3) "Graph oracle offset self-test returned unexpected series count: $($offsetResults.Count)."
        Write-E2E ("graph-oracle-self-test: PASS valid-offset series={0} y-origin-independent" -f $offsetResults.Count)
    }
    finally {
        $offsetBitmap.Dispose()
    }
    foreach ($variant in @('missing-flat', 'label-vertical-only', 'wrong-geometry', 'wrong-order', 'sloped', 'wrong-background', 'axis-row-missing-terra', 'axis-row-missing-sol', 'wrong-endpoint', 'short-rise', 'detached-rise', 'missing-series-contribution', 'non-contiguous')) {
        $bitmap = New-E2EGraphOracleSyntheticBitmap -Variant $variant
        try {
            Assert-E2EGraphOracleExpectedFailure -Name $variant -Bitmap $bitmap
        }
        finally {
            $bitmap.Dispose()
        }
    }
    Write-E2E 'graph-oracle-self-test: PASS valid painter-order overlap + valid-offset + thirteen finite negative bitmap cases'
}

function Invoke-E2EFixtureContractTests {
    $documents = New-E2EFixtureDocuments
    $health = New-E2EContractTestResponse -StatusCode 200 -Body '{"api_version":"v1","service":"codex-info"}' -Headers ([ordered]@{})
    $pairHeaders = [ordered]@{
        'Codex-Info-Published-Pair' = @($documents.PublishedPair)
    }
    $status = New-E2EContractTestResponse -StatusCode 200 -Body $documents.Status -Headers $pairHeaders
    $details = New-E2EContractTestResponse -StatusCode 200 -Body $documents.Details -Headers $pairHeaders
    Assert-E2EFixturePreflightResponses -Health $health -Status $status -Details $details | Out-Null
    Write-E2E 'fixture-contract: PASS valid response'

    $missingPairStatus = New-E2EContractTestResponse -StatusCode 200 -Body $documents.Status -Headers ([ordered]@{})
    Assert-E2EExpectedContractFailure -Name 'pair-missing' -Health $health -Status $missingPairStatus -Details $details

    $mismatchedDetailsHeaders = [ordered]@{
        'Codex-Info-Published-Pair' = @('v1:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff')
    }
    $mismatchedDetails = New-E2EContractTestResponse -StatusCode 200 -Body $documents.Details -Headers $mismatchedDetailsHeaders
    Assert-E2EExpectedContractFailure -Name 'pair-mismatch' -Health $health -Status $status -Details $mismatchedDetails

    $detailsWithoutGapsBody = $documents.Details.Replace(',"history_gaps":[]', '')
    $detailsWithoutGaps = New-E2EContractTestResponse -StatusCode 200 -Body $detailsWithoutGapsBody -Headers $pairHeaders
    Assert-E2EExpectedContractFailure -Name 'history-gaps-missing' -Health $health -Status $status -Details $detailsWithoutGaps

    $detailsJson = ConvertFrom-Json -InputObject $documents.Details
    $unorderedSamples = @($detailsJson.history_samples)
    $firstSample = $unorderedSamples[0]
    $unorderedSamples[0] = $unorderedSamples[2]
    $unorderedSamples[2] = $firstSample
    $detailsJson.history_samples = $unorderedSamples
    $unorderedDetails = New-E2EContractTestResponse -StatusCode 200 -Body ($detailsJson | ConvertTo-Json -Compress -Depth 10) -Headers $pairHeaders
    Assert-E2EExpectedContractFailure -Name 'history-sample-order' -Health $health -Status $status -Details $unorderedDetails

    $minuteMisalignedJson = ConvertFrom-Json -InputObject $documents.Details
    $minuteMisalignedJson.history_samples[0].timestamp = [Int64]$minuteMisalignedJson.history_samples[0].timestamp + 35
    $minuteMisalignedDetails = New-E2EContractTestResponse -StatusCode 200 -Body ($minuteMisalignedJson | ConvertTo-Json -Compress -Depth 10) -Headers $pairHeaders
    Assert-E2EExpectedContractFailure -Name 'history-sample-minute-bucket' -Health $health -Status $status -Details $minuteMisalignedDetails -ExpectedReason 'minute bucket'
    Invoke-E2EGraphIdleBandSelfTest
    Invoke-E2ECaptureSelfTest
    Invoke-E2EGraphPixelScannerSelfTest
    Invoke-E2EGraphOracleSelfTest
    Write-E2E 'fixture-contract: PASS five negative cases rejected individually'
}

function Open-E2EChildWindow {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$MainRoot,
        [Parameter(Mandatory = $true)][string]$ButtonName,
        [string]$ButtonAutomationId = '',
        [Parameter(Mandatory = $true)][string]$Title,
        [Parameter(Mandatory = $true)][string]$Role,
        [Parameter(Mandatory = $true)][int]$ProcessId
    )

    $button = Wait-E2E -Description "main button '$ButtonName'" -Probe {
        $candidate = if ([string]::IsNullOrWhiteSpace($ButtonAutomationId)) {
            Find-E2EButtonByName $MainRoot $ButtonName
        }
        else {
            Find-E2EElementByAutomationId $MainRoot $ButtonAutomationId
        }
        if ($null -ne $candidate -and
            $candidate.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
            $candidate.Current.IsEnabled) { return $candidate }
        # Keep a localized real-data run usable when an older installed build
        # predates the navigation AutomationId attributes.
        if (-not [string]::IsNullOrWhiteSpace($ButtonAutomationId)) {
            $localized = Find-E2EButtonByName $MainRoot $ButtonName
            if ($null -ne $localized -and $localized.Current.IsEnabled) { return $localized }
        }
        return $false
    }
    Invoke-E2EElement $button
    $handle = Wait-E2E -Description "$Title window" -Probe {
        $candidate = Find-E2EWindow $ProcessId $Title
        if ($candidate -eq [IntPtr]::Zero) { return $false }
        return $candidate
    }
    Bring-E2EWindowToFront $handle
    $record = Record-E2EWindow $Role $ProcessId $handle
    return [pscustomobject]@{
        Handle = $handle
        Root = Get-E2EUiaRoot $handle
        Record = $record
    }
}

function Find-E2ECloseButton {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root)

    # Stable AutomationIds are locale- and layout-independent.  Prefer them
    # over the bounded geometry fallback, which can otherwise confuse a
    # right-aligned selector with the title-bar close command.
    foreach ($automationId in @(
            'Main.Window.Close',
            'Graph.Window.Close',
            'Threads.Window.Close',
            'Legal.Window.Close',
            'Settings.Window.Close',
            'Setup.Window.Close')) {
        $candidate = Find-E2EElementByAutomationId $Root $automationId
        if ($null -ne $candidate -and
            $candidate.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
            $candidate.Current.IsEnabled) {
            return $candidate
        }
    }

    $named = @('Close')
    $buttons = @(Get-E2EControlElements $Root ([System.Windows.Automation.ControlType]::Button))
    foreach ($button in $buttons) {
        try {
            if ($named -contains [string]$button.Current.Name) { return $button }
        }
        catch { }
    }
    # The product's borderless windows put close at the right edge of the
    # title row. This bounded geometry fallback is locale-independent.
    $windowBounds = Get-E2EWindowBounds ([IntPtr]$Root.Current.NativeWindowHandle)
    $candidates = @($buttons | Where-Object {
        $rect = $_.Current.BoundingRectangle
        $rect.Right -ge ($windowBounds.Left + $windowBounds.Width - 80) -and
            $rect.Top -le ($windowBounds.Top + 90)
    })
    return $candidates | Sort-Object { $_.Current.BoundingRectangle.Right } -Descending | Select-Object -First 1
}

try {
    if ($FixtureContractTest) {
        Write-E2E 'fixture-contract-test: start'
        $contractDocuments = New-E2EFixtureDocuments
        Assert-E2E ([CodexInfoWindowsE2EFixtureServer]::Start($contractDocuments.Status, $contractDocuments.Details, $contractDocuments.PublishedPair, 0)) 'Could not bind the fixture contract test to an ephemeral loopback port.'
        $script:e2eFixturePort = [CodexInfoWindowsE2EFixtureServer]::BoundPort()
        $script:e2eFixtureRunning = $true
        Write-E2E 'fixture-contract-test: fixture server started without launching the client'
        Invoke-E2EFixturePreflight | Out-Null
        Invoke-E2EFixtureContractTests
        return
    }
    $resolvedClientPath = if ([string]::IsNullOrWhiteSpace($ClientPath)) {
        Join-Path $env:LOCALAPPDATA 'Programs\Codex Info Monitor\CodexInfo.WindowsClient.exe'
    }
    else { [IO.Path]::GetFullPath($ClientPath) }
    Assert-E2E (Test-Path -LiteralPath $resolvedClientPath -PathType Leaf) "Installed client not found: $resolvedClientPath"
    Write-E2E "start: client=$resolvedClientPath fixture=$Fixture output=$script:e2eOutput"
    Write-E2E "source-sha: $script:e2eSourceSha"

    if ($Fixture) { Enter-E2EFixture }
    if ($Fixture) {
        Invoke-E2EFixturePreflight | Out-Null
    }
    $script:e2eProcess = Start-Process -FilePath $resolvedClientPath -PassThru
    $clientPid = $script:e2eProcess.Id
    Write-E2E "process: pid=$clientPid"

    $mainHandle = Wait-E2E -Description 'Main window' -Probe {
        $candidate = Find-E2EWindow $clientPid 'Codex Info Monitor'
        if ($candidate -eq [IntPtr]::Zero) { return $false }
        return $candidate
    }
    Bring-E2EWindowToFront $mainHandle
    $mainRecord = Record-E2EWindow 'Main' $clientPid $mainHandle
    $mainRoot = Get-E2EUiaRoot $mainHandle
    $mainGauge = Wait-E2E -Description 'Main quota period gauge' -Probe {
        $candidate = Find-E2EElementByAutomationId $mainRoot 'Main.QuotaPeriodGauge'
        if ($null -ne $candidate) {
            $rect = $candidate.Current.BoundingRectangle
            if (-not $candidate.Current.IsOffscreen -and $rect.Width -gt 0 -and $rect.Height -gt 0) {
                return $candidate
            }
        }
        return $false
    }
    $mainCapture = Capture-E2EWindow $mainHandle '01-main-ready'
    Assert-E2E ($mainCapture.Hash.Length -eq 64) 'Main screenshot hash is missing.'
    Assert-E2EMainProductVersion $mainRoot
    if ($Fixture -or $script:e2ePreviewEnabled) {
        Assert-E2EQuotaGaugePalette -Root $mainRoot -Handle $mainHandle -Capture $mainCapture
    }
    else {
        $gauge = $mainGauge
        $gaugeRect = $gauge.Current.BoundingRectangle
        Assert-E2E (-not $gauge.Current.IsOffscreen -and $gaugeRect.Width -gt 0 -and $gaugeRect.Height -gt 0) 'Main quota period gauge is not visible.'
        Write-E2E ("main-quota-gauge: observed bounds={0}x{1}" -f $gaugeRect.Width, $gaugeRect.Height)
    }
    if ($Fixture) {
        Write-E2E ("fixture: requests={0}" -f [CodexInfoWindowsE2EFixtureServer]::RequestSummary())
    }

    # Give the bounded initial refresh one UI turn before opening a child
    # window.  The child-window assertions below inspect the rendered graph,
    # period options, metrics, and rows directly; a mutable summary TextBlock
    # peer is deliberately not used as a proxy for those surfaces.
    Start-Sleep -Seconds 5
    $startupLoading = Find-E2EElementByAutomationId $mainRoot 'Main.StartupLoading'
    Assert-E2E ($null -eq $startupLoading -or $startupLoading.Current.IsOffscreen -or -not $startupLoading.Current.IsEnabled) `
        'Startup loading surface is still visible after the first refresh window.'
    Write-E2E 'main-startup-loading: PASS (first complete generation is visible)'
    $detailsStatus = Find-E2EElementByAutomationId $mainRoot 'Main.DetailsStatus'
    Assert-E2E ($null -ne $detailsStatus) 'Main details status is missing.'
    $detailsStatusText = [string]$detailsStatus.Current.Name
    Write-E2E ("main: details status='{0}' observed" -f $detailsStatusText)
    # A screenshot or a successful status request is not sufficient evidence:
    # the main surface must have accepted the matching details generation.
    # Consume the locale-independent AutomationProperties.Name contract rather
    # than attempting to decode localized rendered text.
    $detailsContract = Find-E2EElementByAutomationId $mainRoot 'Main.DetailsGenerationContract'
    Assert-E2E ($null -ne $detailsContract) 'Main details generation contract is missing.'
    $detailsContractText = [string]$detailsContract.Current.Name
    $detailsIsLatest = $detailsContractText -eq 'ready'
    $detailsHasFailure = $detailsContractText -eq 'error'
    Write-E2E ("main: details contract value='{0}'" -f $detailsContractText)
    Write-E2E ("main: details contract latest={0} failure={1} length={2}" -f $detailsIsLatest, $detailsHasFailure, $detailsStatusText.Length)
    Assert-E2E ($detailsIsLatest -and -not $detailsHasFailure) `
        "Main details status is not a complete accepted generation: '$detailsStatusText'"
    Write-E2E 'main-details-status: PASS (matching status/details generation accepted)'

    # Finite path: one Graph window, one period round-trip, two metrics, then
    # one OFF/ON cycle for each of four independent series.  No combinations
    # of these controls are generated.
    Write-E2E 'case-1: open Graph'
    $graph = Open-E2EChildWindow -MainRoot $mainRoot -ButtonName 'Graph' -ButtonAutomationId 'Main.OpenGraph' -Title 'Codex Info Graph' -Role 'Graph' -ProcessId $clientPid
    $graphRoot = $graph.Root
    Assert-E2ENoChildProductVersion $graphRoot 'Graph'
    Wait-E2EGraphLoadSettled $graphRoot
    $plot = Wait-E2E -Description 'Graph plot' -Probe {
        $candidate = Find-E2EElementByAutomationId $graphRoot 'Graph.Plot'
        if ($null -eq $candidate) { return $false }
        $rect = $candidate.Current.BoundingRectangle
        if ($candidate.Current.IsOffscreen -or $rect.Width -le 0 -or $rect.Height -le 0) { return $false }
        return $candidate
    }
    $null = Wait-E2EGraphPixelsReady -Root $graphRoot -WindowHandle $graph.Handle -Description 'initial-current'
    Write-E2E ("graph: plot bounds={0}x{1}" -f $plot.Current.BoundingRectangle.Width, $plot.Current.BoundingRectangle.Height)
    $initialGraphBounds = Get-E2EWindowBounds $graph.Handle
    $graphScaleX = $initialGraphBounds.Width / 940.0
    $graphScaleY = $initialGraphBounds.Height / 640.0
    Assert-E2E ([Math]::Abs($graphScaleX - $graphScaleY) -le 0.02) `
        "Graph initial window does not map 940x640 logical through one DPI scale: x=$graphScaleX y=$graphScaleY."
    $graphScale = ($graphScaleX + $graphScaleY) / 2
    Write-E2E ("graph: dpi-scale={0:N3} initial-physical={1}x{2}" -f
        $graphScale, $initialGraphBounds.Width, $initialGraphBounds.Height)
    $periodSelector = Find-E2EElementByAutomationId $graphRoot 'Graph.PeriodSelector'
    $metricSelector = Find-E2EElementByAutomationId $graphRoot 'Graph.MetricSelector'
    Assert-E2E ($null -ne $periodSelector -and $null -ne $metricSelector) 'Graph selectors are missing.'
    $currentLabel = Get-E2ESelectorLabel $periodSelector
    $graphCurrent = Capture-E2EWindow $graph.Handle '02-graph-current'

    Write-E2E 'case-2: period current -> past -> current and display-value assertions'
    Toggle-E2EElement $periodSelector
    $periodItems = Wait-E2E -Description 'two Graph period options' -Probe {
        # Only the open in-window menu is enabled; the other pre-measured menu
        # remains disabled.  Filtering by enabled UIA state is DPI-independent.
        $items = @(Get-E2EVisibleControlElements $graphRoot ([System.Windows.Automation.ControlType]::ListItem))
        if ($items.Count -ge 2) { return $items }
        return $false
    }
    $periodLabels = @($periodItems | ForEach-Object { [string]$_.Current.Name } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
    Assert-E2E ($periodLabels.Count -ge 2) "Graph period menu exposes fewer than two values: $($periodLabels -join ', ')."
    $selectedPeriodLabel = Get-E2ESelectedListItemLabel $periodItems
    if (-not [string]::IsNullOrWhiteSpace($selectedPeriodLabel)) {
        $currentLabel = $selectedPeriodLabel
    }
    Assert-E2E ($periodLabels -contains $currentLabel) 'Current period display value is not represented by a selected menu item.'
    $pastLabel = [string]($periodLabels | Where-Object { $_ -ne $currentLabel } | Select-Object -First 1)
    Assert-E2E (-not [string]::IsNullOrWhiteSpace($pastLabel)) 'Past period option is missing.'
    Select-E2EListItem $graphRoot $pastLabel
    Wait-E2ESelectorLabel $graphRoot 'Graph.PeriodSelector' $pastLabel
    Wait-E2EGraphLoadSettled $graphRoot
    $null = Wait-E2EGraphPixelsReady -Root $graphRoot -WindowHandle $graph.Handle -Description 'past-period'
    $graphPast = Capture-E2EWindow $graph.Handle '03-graph-past'
    Assert-E2EImageChanged $graphCurrent $graphPast 'Current-to-past period selection'
    if ($Fixture -or $script:e2ePreviewEnabled) {
        Assert-E2EGraphHasModelData $plot $graph.Handle $graphPast
    }
    if ($Fixture) {
        Assert-E2EGraphHasIdleBand $plot $graph.Handle $graphPast
    }

    $periodSelector = Find-E2EElementByAutomationId $graphRoot 'Graph.PeriodSelector'
    Toggle-E2EElement $periodSelector
    Select-E2EListItem $graphRoot $currentLabel
    Wait-E2ESelectorLabel $graphRoot 'Graph.PeriodSelector' $currentLabel
    Wait-E2EGraphLoadSettled $graphRoot
    $null = Wait-E2EGraphPixelsReady -Root $graphRoot -WindowHandle $graph.Handle -Description 'current-period-restored'
    $graphCurrentAgain = Capture-E2EWindow $graph.Handle '04-graph-current-again'
    Assert-E2EImageChanged $graphPast $graphCurrentAgain 'Past-to-current period selection'

    Write-E2E 'case-3: select both metric values'
    $metricSelector = Find-E2EElementByAutomationId $graphRoot 'Graph.MetricSelector'
    $initialMetric = Get-E2ESelectorLabel $metricSelector
    Toggle-E2EElement $metricSelector
    $metricItems = Wait-E2E -Description 'two Graph metric options' -Probe {
        $items = @(Get-E2EVisibleControlElements $graphRoot ([System.Windows.Automation.ControlType]::ListItem))
        if ($items.Count -ge 2) { return $items }
        return $false
    }
    $metricLabels = @($metricItems | ForEach-Object { [string]$_.Current.Name } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
    Assert-E2E ($metricLabels.Count -eq 2) "Metric menu must expose exactly two values: $($metricLabels -join ', ')."
    $selectedMetricLabel = Get-E2ESelectedListItemLabel $metricItems
    if (-not [string]::IsNullOrWhiteSpace($selectedMetricLabel)) {
        $initialMetric = $selectedMetricLabel
    }
    Assert-E2E ($metricLabels -contains $initialMetric) 'Initial metric display value is not represented by a selected menu item.'
    $otherMetric = [string]($metricLabels | Where-Object { $_ -ne $initialMetric } | Select-Object -First 1)
    Assert-E2E (-not [string]::IsNullOrWhiteSpace($otherMetric)) 'Second metric option is missing.'
    Select-E2EListItem $graphRoot $otherMetric
    Wait-E2ESelectorLabel $graphRoot 'Graph.MetricSelector' $otherMetric
    Wait-E2EGraphLoadSettled $graphRoot
    $null = Wait-E2EGraphPixelsReady -Root $graphRoot -WindowHandle $graph.Handle -Description 'other-metric'
    Wait-E2E -Description "axis for metric '$otherMetric'" -Probe {
        $texts = Get-E2ETextValues $graphRoot
        if ($texts -contains $otherMetric -or ($texts -join ' ') -like "*$otherMetric*") { return $true }
        return $false
    } | Out-Null
    $graphOtherMetric = Capture-E2EWindow $graph.Handle '05-graph-other-metric'
    Assert-E2EImageChanged $graphCurrentAgain $graphOtherMetric "Metric selection '$otherMetric'"

    $metricSelector = Find-E2EElementByAutomationId $graphRoot 'Graph.MetricSelector'
    Toggle-E2EElement $metricSelector
    Select-E2EListItem $graphRoot $initialMetric
    Wait-E2ESelectorLabel $graphRoot 'Graph.MetricSelector' $initialMetric
    Wait-E2EGraphLoadSettled $graphRoot
    $null = Wait-E2EGraphPixelsReady -Root $graphRoot -WindowHandle $graph.Handle -Description 'initial-metric-restored'
    $graphInitialMetricAgain = Capture-E2EWindow $graph.Handle '06-graph-initial-metric'
    Assert-E2EImageChanged $graphOtherMetric $graphInitialMetricAgain "Metric selection '$initialMetric'"

    Write-E2E 'case-4: fixed endpoint gutter across finite horizontal resize states'
    $allMetricMeasurements = @{}
    $resizeMetricLabels = @($initialMetric, $otherMetric)
    for ($metricIndex = 0; $metricIndex -lt $resizeMetricLabels.Count; $metricIndex++) {
        $metricLabel = $resizeMetricLabels[$metricIndex]
        # Localized labels can both sanitize to the same ASCII filename.
        # Prefix the stable selector order so evidence from one metric never
        # overwrites the other metric's captures.
        $metricKey = "metric-$metricIndex-" + ($metricLabel -replace '[^A-Za-z0-9_.-]', '_')
        $currentMetricLabel = Get-E2ESelectorLabel (Find-E2EElementByAutomationId $graphRoot 'Graph.MetricSelector')
        if ($currentMetricLabel -ne $metricLabel -and -not $currentMetricLabel.Contains($metricLabel)) {
            Toggle-E2EElement (Find-E2EElementByAutomationId $graphRoot 'Graph.MetricSelector')
            Select-E2EListItem $graphRoot $metricLabel
            Wait-E2ESelectorLabel $graphRoot 'Graph.MetricSelector' $metricLabel
            Wait-E2EGraphLoadSettled $graphRoot
            $null = Wait-E2EGraphPixelsReady -Root $graphRoot -WindowHandle $graph.Handle -Description "metric-$metricKey"
        }

        $measurements = @{}
        foreach ($resizeState in @(
            @{ Name = '700x640'; Width = 700; Height = 640 },
            @{ Name = '940x640'; Width = 940; Height = 640 },
            @{ Name = '1000x640'; Width = 1000; Height = 640 },
            @{ Name = 'restore-940x640'; Width = 940; Height = 640 },
            @{ Name = '700x480'; Width = 700; Height = 480 }
        )) {
            Set-E2EGraphLogicalSize -Handle $graph.Handle `
                -LogicalWidth $resizeState.Width -LogicalHeight $resizeState.Height -Scale $graphScale
            $graphRoot = Get-E2EUiaRoot $graph.Handle
            $plot = Wait-E2E -Description "Graph plot at $($resizeState.Name)" -Probe {
                $candidate = Find-E2EElementByAutomationId $graphRoot 'Graph.Plot'
                if ($null -eq $candidate -or $candidate.Current.IsOffscreen) { return $false }
                if ($candidate.Current.BoundingRectangle.Width -le 0 -or $candidate.Current.BoundingRectangle.Height -le 0) { return $false }
                return $candidate
            }
            $description = "$metricKey-$($resizeState.Name)"
            $capture = Capture-E2EWindow $graph.Handle ("resize-{0}" -f $description)
            $measurements[$resizeState.Name] = Get-E2EGraphMeasurement `
                -Capture $capture -Plot $plot -WindowHandle $graph.Handle -Description $description
        }

        $target = $measurements['940x640']
        foreach ($stateName in @('700x640', '940x640', '1000x640', '700x480')) {
            $actual = $measurements[$stateName]
            Assert-E2E ([Math]::Abs($actual.Pixels.GutterWidth - $target.Pixels.GutterWidth) -le 2) `
                "$metricLabel endpoint gutter changed at ${stateName}: target=$($target.Pixels.GutterWidth) actual=$($actual.Pixels.GutterWidth)."
        }
        Assert-E2E ($measurements['700x640'].Pixels.PlotSpan -lt $target.Pixels.PlotSpan -and
            $target.Pixels.PlotSpan -lt $measurements['1000x640'].Pixels.PlotSpan) `
            "$metricLabel plot span did not grow monotonically with same-height window width."
        foreach ($pair in @(
            @($measurements['700x640'], $target),
            @($target, $measurements['1000x640'])
        )) {
            $spanIncrease = $pair[1].Pixels.PlotSpan - $pair[0].Pixels.PlotSpan
            $uiaWidthIncrease = $pair[1].PlotBoundsWidth - $pair[0].PlotBoundsWidth
            Assert-E2E ([Math]::Abs($spanIncrease - $uiaWidthIncrease) -le 3) `
                "$metricLabel plot span increase $spanIncrease does not match Graph.Plot width increase $uiaWidthIncrease."
        }
        foreach ($sameHeightState in @('700x640', '1000x640')) {
            for ($seriesIndex = 0; $seriesIndex -lt 4; $seriesIndex++) {
                Assert-E2E ([Math]::Abs(
                    $measurements[$sameHeightState].Pixels.SeriesGutterTop[$seriesIndex] -
                    $target.Pixels.SeriesGutterTop[$seriesIndex]) -le 2) `
                    "$metricLabel series $seriesIndex endpoint top changed at $sameHeightState."
                Assert-E2E ([Math]::Abs(
                    $measurements[$sameHeightState].Pixels.SeriesGutterBottom[$seriesIndex] -
                    $target.Pixels.SeriesGutterBottom[$seriesIndex]) -le 2) `
                    "$metricLabel series $seriesIndex endpoint bottom changed at $sameHeightState."
            }
        }
        $restored = $measurements['restore-940x640']
        Assert-E2E ([Math]::Abs($restored.Pixels.GutterWidth - $target.Pixels.GutterWidth) -le 2 -and
            [Math]::Abs($restored.Pixels.PlotSpan - $target.Pixels.PlotSpan) -le 2) `
            "$metricLabel did not restore its 940x640 gutter/plot span after 1000x640."
        Write-E2E ("graph-resize: PASS metric={0} gutter={1}px states=700x640,940x640,1000x640,700x480 restore=PASS" -f
            $metricLabel, $target.Pixels.GutterWidth)
        $allMetricMeasurements[$metricLabel] = $measurements
    }

    Set-E2EGraphLogicalSize -Handle $graph.Handle -LogicalWidth 940 -LogicalHeight 640 -Scale $graphScale
    $graphRoot = Get-E2EUiaRoot $graph.Handle
    $currentMetricLabel = Get-E2ESelectorLabel (Find-E2EElementByAutomationId $graphRoot 'Graph.MetricSelector')
    if ($currentMetricLabel -ne $initialMetric -and -not $currentMetricLabel.Contains($initialMetric)) {
        Toggle-E2EElement (Find-E2EElementByAutomationId $graphRoot 'Graph.MetricSelector')
        Select-E2EListItem $graphRoot $initialMetric
        Wait-E2ESelectorLabel $graphRoot 'Graph.MetricSelector' $initialMetric
        Wait-E2EGraphLoadSettled $graphRoot
    }

    Write-E2E 'case-5: each series toggle OFF then ON exactly once'
    $toggleCases = @(
        @{ Id = 'Graph.Toggle.Remaining'; Name = 'Remaining' },
        @{ Id = 'Graph.Toggle.LUNA'; Name = 'LUNA' },
        @{ Id = 'Graph.Toggle.TERRA'; Name = 'TERRA' },
        @{ Id = 'Graph.Toggle.SOL'; Name = 'SOL' }
    )
    $toggleIndex = 0
    foreach ($toggleCase in $toggleCases) {
        $toggleIndex++
        $toggle = Find-E2EElementByAutomationId $graphRoot $toggleCase.Id
        Assert-E2E ($null -ne $toggle) "Graph toggle is missing: $($toggleCase.Id)"
        $initialState = Get-E2EToggleState $toggle
        Assert-E2E ($initialState -eq [System.Windows.Automation.ToggleState]::On) "$($toggleCase.Name) is not initially ON."
        $beforeOff = Capture-E2EWindow $graph.Handle ("07-toggle-{0}-before" -f $toggleCase.Name)
        Toggle-E2EElement $toggle
        Wait-E2E -Description "$($toggleCase.Name) OFF" -Probe {
            $candidate = Find-E2EElementByAutomationId $graphRoot $toggleCase.Id
            if ($null -ne $candidate -and (Get-E2EToggleState $candidate) -eq [System.Windows.Automation.ToggleState]::Off) { return $true }
            return $false
        } | Out-Null
        $off = Capture-E2EWindow $graph.Handle ("08-toggle-{0}-off" -f $toggleCase.Name)
        Assert-E2EImageChanged $beforeOff $off "$($toggleCase.Name) OFF render"
        Toggle-E2EElement (Find-E2EElementByAutomationId $graphRoot $toggleCase.Id)
        Wait-E2E -Description "$($toggleCase.Name) ON" -Probe {
            $candidate = Find-E2EElementByAutomationId $graphRoot $toggleCase.Id
            if ($null -ne $candidate -and (Get-E2EToggleState $candidate) -eq [System.Windows.Automation.ToggleState]::On) { return $true }
            return $false
        } | Out-Null
        $on = Capture-E2EWindow $graph.Handle ("09-toggle-{0}-on" -f $toggleCase.Name)
        Assert-E2EImageChanged $off $on "$($toggleCase.Name) ON render"
        Write-E2E "toggle: name=$($toggleCase.Name) off=Off on=On cycle=$toggleIndex"
    }

    $closeGraph = Find-E2ECloseButton $graphRoot
    Assert-E2E ($null -ne $closeGraph) 'Graph Close button is missing.'
    Invoke-E2EElement $closeGraph
    Wait-E2E -Description 'Graph window close' -Probe {
        return (Find-E2EWindow $clientPid 'Codex Info Graph') -eq [IntPtr]::Zero
    } | Out-Null

    Write-E2E 'case-5: open Threads and assert root/child/orphan rows and columns'
    $threads = Open-E2EChildWindow -MainRoot $mainRoot -ButtonName 'Threads' -ButtonAutomationId 'Main.OpenThreads' -Title 'Codex Info Threads' -Role 'Threads' -ProcessId $clientPid
    $threadsRoot = $threads.Root
    Assert-E2ENoChildProductVersion $threadsRoot 'Threads'
    $threadTexts = Wait-E2E -Description 'Threads rows' -Probe {
        $values = @(Get-E2ETextValues $threadsRoot)
        if ($values.Count -ge 8) { return $values }
        return $false
    }
    $fixtureRows = @(
        @{ Title = 'E2E root task'; Id = 'e2e-root'; Model = 'TERRA'; Column = 'Depth 0' },
        @{ Title = 'E2E child task'; Id = 'e2e-child'; Model = 'LUNA'; Column = 'Depth 1' },
        @{ Title = 'E2E orphan task'; Id = 'e2e-orphan'; Model = 'SOL'; Column = 'missing-parent' }
    )
    if ($Fixture) {
        foreach ($row in $fixtureRows) {
            Assert-E2E (@($threadTexts | Where-Object { $_ -eq $row.Title }).Count -gt 0) "Threads row title missing: $($row.Title)"
            $rowElement = Wait-E2E -Description "Threads row identity '$($row.Id)'" -Probe {
                $candidate = Find-E2EElementByAutomationId $threadsRoot $row.Id
                if ($null -ne $candidate) { return $candidate }
                return $false
            }
            Assert-E2E ([string]$rowElement.Current.Name -eq $row.Title) "Threads row identity/title mismatch: $($row.Id)"
            Assert-E2E (@($threadTexts | Where-Object { $_ -eq $row.Model }).Count -gt 0) "Threads model column missing: $($row.Model)"
            Assert-E2E (@($threadTexts | Where-Object { $_ -like "*$($row.Column)*" }).Count -gt 0) "Threads metadata column missing: $($row.Column)"
        }
        Assert-E2E (@($threadTexts | Where-Object { $_ -like '*Parent: e2e-root*' }).Count -gt 0) 'Child parent column is missing.'
        Assert-E2E (@($threadTexts | Where-Object { $_ -like '*Parent: missing-parent*' }).Count -gt 0) 'Orphan parent column is missing.'
    }
    else {
        # Real data mode accepts the server's row identities, but still
        # requires a row container with several visible cells (title, model,
        # and metadata). An empty or status-only window cannot pass.
        $threadRows = @(Get-E2EControlElements $threadsRoot ([System.Windows.Automation.ControlType]::ListItem))
        if ($threadRows.Count -gt 0) {
            $richRows = @($threadRows | Where-Object { @(Get-E2ETextValues $_).Count -ge 4 })
            Assert-E2E ($richRows.Count -gt 0) 'Threads rows did not expose representative columns.'
        }
        else {
            # Avalonia versions differ in whether ItemsControl containers are
            # surfaced as ListItem. Keep the observable fallback bounded while
            # still requiring multiple non-empty cell values.
            $nonEmpty = @($threadTexts | Where-Object { $_.Length -ge 2 })
            Assert-E2E ($nonEmpty.Count -ge 4) 'Threads did not expose a real data row and columns.'
        }
    }
    $threadCapture = Capture-E2EWindow $threads.Handle '10-threads-rows'
    Assert-E2E ($threadCapture.Hash.Length -eq 64) 'Threads screenshot hash is missing.'

    Write-E2E 'case-6: open Legal and assert plain-text legal notice'
    $legal = Open-E2EChildWindow -MainRoot $mainRoot -ButtonName 'Legal' -ButtonAutomationId 'Main.OpenLegal' -Title 'Codex Info Legal' -Role 'Legal' -ProcessId $clientPid
    $legalRoot = $legal.Root
    Assert-E2ENoChildProductVersion $legalRoot 'Legal'
    $legalNext = Find-E2EElementByAutomationId $legalRoot 'Legal.Page.Next'
    Assert-E2E ($null -ne $legalNext) 'Legal Next button is missing.'
    $legalPagePosition = Find-E2EElementByAutomationId $legalRoot 'Legal.Page.Position'
    Assert-E2E ($null -ne $legalPagePosition) 'Legal page position is missing.'
    $legalPageNames = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $expectedLegalPageCount = 9
    for ($legalPage = 1; $legalPage -le $expectedLegalPageCount; $legalPage++) {
        $expectedPositionSuffix = "$legalPage / $expectedLegalPageCount"
        Wait-E2E -Description "Legal page $legalPage position" -Probe {
            $value = [string]$legalPagePosition.Current.Name
            return $value.EndsWith($expectedPositionSuffix, [StringComparison]::Ordinal)
        } | Out-Null
        $legalText = Wait-E2E -Description "Legal plain-text notice page $legalPage" -Probe {
            $candidate = Find-E2EElementByAutomationId $legalRoot 'Legal.Notice.Text'
            if ($null -eq $candidate -or $candidate.Current.IsOffscreen) { return $false }
            $value = [string]$candidate.Current.Name
            if ([string]::IsNullOrWhiteSpace($value)) { return $false }
            return $candidate
        }
        $legalName = Find-E2EElementByAutomationId $legalRoot 'Legal.Notice.Name'
        Assert-E2E ($null -ne $legalName) "Legal notice name is missing on page $legalPage."
        $null = $legalPageNames.Add([string]$legalName.Current.Name)
        $legalValue = [string]$legalText.Current.Name
        $scanMarkdown = $legalPage -notin @(2, 3, 6)
        foreach ($legalRawLine in $legalValue -split "`n") {
            $legalLine = $legalRawLine.TrimEnd("`r")
            if ($legalLine.StartsWith([string][char]0xFF3B, [StringComparison]::Ordinal) -and
                $legalLine.EndsWith([string][char]0xFF3D, [StringComparison]::Ordinal)) {
                $scanMarkdown = $legalLine.EndsWith(".md$([char]0xFF3D)", [StringComparison]::OrdinalIgnoreCase)
                continue
            }
            if (-not $scanMarkdown) { continue }
            foreach ($forbidden in @('<!--', '-->', '```', '](', '`')) {
                Assert-E2E ($legalLine.IndexOf($forbidden, [StringComparison]::Ordinal) -lt 0) "Legal page $legalPage exposes raw Markdown marker: $forbidden"
            }
            Assert-E2E (-not [regex]::IsMatch($legalLine, '<https?://[^>]+>')) "Legal page $legalPage exposes a raw Markdown autolink."
            Assert-E2E (-not [regex]::IsMatch($legalLine, '\*\*[^*\r\n]+\*\*')) "Legal page $legalPage exposes raw Markdown strong emphasis."
            Assert-E2E (-not [regex]::IsMatch($legalLine, '~~[^~\r\n]+~~')) "Legal page $legalPage exposes raw Markdown strikethrough."
            Assert-E2E (-not [regex]::IsMatch($legalLine, '(?<!\w)_[^_\r\n]+_(?!\w)')) "Legal page $legalPage exposes raw Markdown emphasis."
            Assert-E2E (-not [regex]::IsMatch($legalLine, '^\s*#{1,6}\s+')) "Legal page $legalPage exposes a raw Markdown heading."
        }
        if ($legalPage -eq 1) {
            Assert-E2E ($legalValue.IndexOf('GPL', [StringComparison]::Ordinal) -ge 0) 'Legal notice lost the GPL legal content.'
            $legalCapture = Capture-E2EWindow $legal.Handle '11-legal-plain-text-page-1'
            Assert-E2E ($legalCapture.Hash.Length -eq 64) 'Legal page 1 screenshot hash is missing.'
        }
        elseif ($legalPage -eq 8) {
            $legalCapture = Capture-E2EWindow $legal.Handle '12-legal-plain-text-page-8'
            Assert-E2E ($legalCapture.Hash.Length -eq 64) 'Legal page 8 screenshot hash is missing.'
        }
        if ($legalPage -lt $expectedLegalPageCount) {
            Assert-E2E ($legalNext.Current.IsEnabled) "Legal Next button is disabled before page $expectedLegalPageCount."
            Invoke-E2EElement $legalNext
        }
    }
    Assert-E2E ($legalPageNames.Count -eq $expectedLegalPageCount) "Legal page navigation did not expose $expectedLegalPageCount distinct chapters."
    Assert-E2E (-not $legalNext.Current.IsEnabled) 'Legal Next button remains enabled on the final page.'
    $legalBack = Find-E2EElementByAutomationId $legalRoot 'Legal.Page.Back'
    Assert-E2E ($null -ne $legalBack -and $legalBack.Current.IsEnabled) 'Legal Back button is unavailable on the final page.'
    Invoke-E2EElement $legalBack
    Wait-E2E -Description 'Legal Back navigation from page 9 to page 8' -Probe {
        return ([string]$legalPagePosition.Current.Name).EndsWith('8 / 9', [StringComparison]::Ordinal)
    } | Out-Null

    $legalMinimize = Find-E2EElementByAutomationId $legalRoot 'Legal.Window.Minimize'
    Assert-E2E ($null -ne $legalMinimize -and $legalMinimize.Current.IsEnabled) 'Legal Minimize button is unavailable.'
    Invoke-E2EElement $legalMinimize
    Wait-E2E -Description 'Legal window minimize' -Probe {
        return [CodexInfoWindowsE2EWin32]::IsIconic($legal.Handle)
    } | Out-Null
    [CodexInfoWindowsE2EWin32]::ShowWindow($legal.Handle, 9) | Out-Null
    Wait-E2E -Description 'Legal window restore after minimize' -Probe {
        return -not [CodexInfoWindowsE2EWin32]::IsIconic($legal.Handle)
    } | Out-Null
    $legalRoot = Get-E2EUiaRoot $legal.Handle
    Write-E2E 'legal-plain-text: PASS (all 9 rendered notices, Back, Minimize, and Close are usable)'

    $closeLegal = Find-E2ECloseButton $legalRoot
    Assert-E2E ($null -ne $closeLegal) 'Legal Close button is missing.'
    Invoke-E2EElement $closeLegal
    Wait-E2E -Description 'Legal window close' -Probe {
        return (Find-E2EWindow $clientPid 'Codex Info Legal') -eq [IntPtr]::Zero
    } | Out-Null

    Write-E2E 'case-7: same PID and HWND records'
    $allPids = @($script:e2eWindowRecords | ForEach-Object { $_.pid } | Select-Object -Unique)
    Assert-E2E ($allPids.Count -eq 1 -and $allPids[0] -eq $clientPid) "Window PID set is not singleton: $($allPids -join ',')."
    $allHwnds = @($script:e2eWindowRecords | ForEach-Object { $_.hwnd } | Select-Object -Unique)
    Assert-E2E ($allHwnds.Count -eq $script:e2eWindowRecords.Count) 'Window HWND records are not unique.'
    $windowRecordPath = Join-Path $script:e2eOutput 'window-records.json'
    $script:e2eWindowRecords | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $windowRecordPath -Encoding utf8
    Write-E2E "windows: PASS records=$($script:e2eWindowRecords.Count) pid=$clientPid records_path=$windowRecordPath"

    $graphEvidence = if ($Fixture) { 'past-period model and idle-band pixels' } else { 'past-period model pixels' }
    Write-E2E ("windows-client-e2e: PASS (Graph open, {0}, period current/past/current, 2 metrics, 4 toggle OFF/ON cycles, Threads rows/columns, Legal plain text, PID/HWND records)" -f $graphEvidence)
    $script:e2eSuccess = $true
}
catch {
    if ($null -ne $script:e2eProcess -and $null -ne $mainRoot) {
        try {
            $failureStatus = Find-E2EElementByAutomationId $mainRoot 'Main.DetailsStatus'
            if ($null -ne $failureStatus) {
                Write-E2E ("main: failure details status='{0}'" -f $failureStatus.Current.Name)
            }
            $null = Capture-E2EWindow $mainHandle 'failure-main'
        }
        catch { }
    }
    Write-E2E "windows-client-e2e: FAIL $($_.Exception.Message)"
    throw
}
finally {
    if ($null -ne $script:e2eProcess) {
        try {
            if (-not $script:e2eProcess.HasExited) {
                Stop-Process -Id $script:e2eProcess.Id -Force -ErrorAction SilentlyContinue
            }
        }
        catch { }
    }
    if ($Fixture) {
        try { Exit-E2EFixture } catch { Write-E2E "fixture-cleanup: FAIL $($_.Exception.Message)" }
    }
}

# A successful script invocation returns naturally.  Failures are thrown from
# the acceptance assertions above, so callers cannot mistake a SKIP for PASS.
