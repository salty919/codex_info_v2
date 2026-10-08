# Measures the installed Graph preview through the same input actions exposed
# to a user. Toggle and metric-button latency ends after the expected UIA state
# and changed control surface; each sample also requires a changed Graph.Plot
# GDI frame within the bounded timeout. Period menu samples retain their
# screen-change plus UIA postcondition contract.
[CmdletBinding()]
param(
    [string]$ClientPath = '',
    [string]$OutputDirectory = '',
    [ValidateRange(30, 1000)][int]$Iterations = 30,
    [ValidateRange(250, 5000)][int]$PaintTimeoutMilliseconds = 2000
)

$ErrorActionPreference = 'Stop'

function Assert-GraphLatency {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )

    if (-not $Condition) {
        throw "ASSERT: $Message"
    }
}

function Resolve-GraphLatencyOutputDirectory {
    param([string]$Requested)

    $candidate = if ([string]::IsNullOrWhiteSpace($Requested)) {
        if (-not [string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
            Join-Path $env:RUNNER_TEMP 'codex-info-windows-graph-latency'
        }
        else {
            Join-Path ([IO.Path]::GetTempPath()) 'codex-info-windows-graph-latency'
        }
    }
    else {
        $Requested
    }
    $resolved = [IO.Path]::GetFullPath($candidate)
    $repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..')).TrimEnd(
        [char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar))
    $repositoryPrefix = $repositoryRoot + [IO.Path]::DirectorySeparatorChar
    Assert-GraphLatency (
        -not $resolved.Equals($repositoryRoot, [StringComparison]::OrdinalIgnoreCase) -and
        -not $resolved.StartsWith($repositoryPrefix, [StringComparison]::OrdinalIgnoreCase) -and
        -not $resolved.StartsWith(($repositoryRoot + [IO.Path]::AltDirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)
    ) 'Latency artifacts must be written outside the repository.'
    return $resolved
}

$script:graphLatencyOutput = Resolve-GraphLatencyOutputDirectory $OutputDirectory
New-Item -ItemType Directory -Path $script:graphLatencyOutput -Force | Out-Null
$script:graphLatencyLogPath = Join-Path $script:graphLatencyOutput 'windows-graph-latency.log'
$script:graphLatencyReportPath = Join-Path $script:graphLatencyOutput 'windows-graph-latency.json'
$script:graphPhysicalInputSequence = 0

function Write-GraphLatencyLog {
    param([Parameter(Mandatory = $true)][string]$Message)

    $line = "{0} {1}" -f ([DateTimeOffset]::Now.ToString('o')), $Message
    Add-Content -LiteralPath $script:graphLatencyLogPath -Value $line -Encoding utf8
    Write-Host $line
}

function Get-GraphLatencyMilliseconds {
    param([Parameter(Mandatory = $true)][long]$StartTimestamp)

    return (([Diagnostics.Stopwatch]::GetTimestamp() - $StartTimestamp) * 1000.0 /
        [Diagnostics.Stopwatch]::Frequency)
}

function Wait-GraphLatencyPredicate {
    param(
        [Parameter(Mandatory = $true)][scriptblock]$Probe,
        [Parameter(Mandatory = $true)][string]$Description,
        [int]$TimeoutMilliseconds = 5000
    )

    $deadline = [Diagnostics.Stopwatch]::GetTimestamp() +
        [long]($TimeoutMilliseconds * [Diagnostics.Stopwatch]::Frequency / 1000.0)
    while ([Diagnostics.Stopwatch]::GetTimestamp() -lt $deadline) {
        try {
            $value = & $Probe
            if ($null -ne $value -and [bool]$value) {
                return $value
            }
        }
        catch {
            # Avalonia can replace the element tree while a frame is painted.
            # Re-querying this finite target is safe; timeout remains fatal.
        }
        [Threading.Thread]::Yield() | Out-Null
    }
    throw "TIMEOUT: $Description (${TimeoutMilliseconds}ms)"
}

if (-not ('CodexInfoWindowsGraphLatencyWin32' -as [type])) {
    Add-Type -AssemblyName System.Drawing
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    $drawingImplementation = [System.Drawing.Bitmap].Assembly
    $drawingReferences = @(
        $drawingImplementation.Location
        [System.Drawing.Size].Assembly.Location
        [System.Security.Cryptography.SHA256].Assembly.Location
    )
    $drawingDependencyPattern = '^(System\.Private\.Windows\.|Microsoft\.Win32\.SystemEvents$)'
    $pendingDrawingDependencies = @($drawingImplementation.GetReferencedAssemblies() |
        Where-Object { $_.Name -match $drawingDependencyPattern })
    $loadedDrawingDependencies = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    for ($dependencyIndex = 0; $dependencyIndex -lt $pendingDrawingDependencies.Count; $dependencyIndex++) {
        $dependencyName = $pendingDrawingDependencies[$dependencyIndex]
        if (-not $loadedDrawingDependencies.Add($dependencyName.FullName)) {
            continue
        }
        $dependencyAssembly = [Reflection.Assembly]::Load($dependencyName)
        $drawingReferences += $dependencyAssembly.Location
        $pendingDrawingDependencies += @($dependencyAssembly.GetReferencedAssemblies() |
            Where-Object { $_.Name -match $drawingDependencyPattern })
    }
    $drawingReferences = @($drawingReferences |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -Unique)
    Assert-GraphLatency ($drawingReferences.Count -gt 0) 'System.Drawing implementation assembly could not be resolved.'
    Add-Type -ReferencedAssemblies $drawingReferences -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

public static class CodexInfoWindowsGraphLatencyWin32 {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern uint GetDoubleClickTime();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int command);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(
        uint count, INPUT[] inputs, int size);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(
        IntPtr hWnd, IntPtr insertAfter, int x, int y, int width, int height, uint flags);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr extra);

    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT {
        public uint Type;
        public INPUTUNION Data;
    }

    [StructLayout(LayoutKind.Explicit)]
    public struct INPUTUNION {
        [FieldOffset(0)] public MOUSEINPUT Mouse;
        [FieldOffset(0)] public KEYBDINPUT Keyboard;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct MOUSEINPUT {
        public int X;
        public int Y;
        public uint MouseData;
        public uint Flags;
        public uint Time;
        public UIntPtr ExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct KEYBDINPUT {
        public ushort VirtualKey;
        public ushort ScanCode;
        public uint Flags;
        public uint Time;
        public UIntPtr ExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT {
        public int X;
        public int Y;
    }

    public sealed class ScreenFrame {
        public string Hash { get; set; }
        public bool HasVariation { get; set; }
        public int Width { get; set; }
        public int Height { get; set; }
    }

    public static bool SendMouseButton(uint flag) {
        var inputs = new INPUT[1];
        inputs[0].Type = 0;
        inputs[0].Data.Mouse.Flags = flag;
        return SendInput(1, inputs, Marshal.SizeOf(typeof(INPUT))) == 1;
    }

    public static bool SendKeyboardKey(ushort virtualKey, bool released) {
        var inputs = new INPUT[1];
        inputs[0].Type = 1;
        inputs[0].Data.Keyboard.VirtualKey = virtualKey;
        inputs[0].Data.Keyboard.Flags = released ? 0x0002u : 0u;
        return SendInput(1, inputs, Marshal.SizeOf(typeof(INPUT))) == 1;
    }

    public static ScreenFrame CaptureScreenFrame(IntPtr hWnd) {
        RECT rect;
        if (!GetWindowRect(hWnd, out rect)) {
            throw new InvalidOperationException("GetWindowRect failed.");
        }
        return CaptureScreenFrame(hWnd, rect.Left, rect.Top, rect.Right - rect.Left, rect.Bottom - rect.Top);
    }

    public static void CaptureWindowPng(IntPtr hWnd, string path) {
        RECT rect;
        if (!GetWindowRect(hWnd, out rect)) {
            throw new InvalidOperationException("GetWindowRect failed.");
        }
        var width = rect.Right - rect.Left;
        var height = rect.Bottom - rect.Top;
        using (var bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb))
        using (var graphics = Graphics.FromImage(bitmap)) {
            graphics.CopyFromScreen(rect.Left, rect.Top, 0, 0, new Size(width, height), CopyPixelOperation.SourceCopy);
            bitmap.Save(path, ImageFormat.Png);
        }
    }

    public static ScreenFrame CaptureScreenFrame(IntPtr hWnd, int left, int top, int width, int height) {
        RECT window;
        if (!GetWindowRect(hWnd, out window)) {
            throw new InvalidOperationException("GetWindowRect failed.");
        }
        left = Math.Max(window.Left, left);
        top = Math.Max(window.Top, top);
        width = Math.Min(width, window.Right - left);
        height = Math.Min(height, window.Bottom - top);
        if (width <= 0 || height <= 0) {
            throw new InvalidOperationException(String.Format("Invalid capture bounds: {0}x{1}", width, height));
        }

        using (var bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb))
        using (var graphics = Graphics.FromImage(bitmap)) {
            graphics.CopyFromScreen(
                left,
                top,
                0,
                0,
                new Size(width, height),
                CopyPixelOperation.SourceCopy);
            var area = new Rectangle(0, 0, width, height);
            var data = bitmap.LockBits(area, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
            try {
                var length = Math.Abs(data.Stride) * height;
                var bytes = new byte[length];
                Marshal.Copy(data.Scan0, bytes, 0, length);
                byte[] digest;
                using (var sha = SHA256.Create()) {
                    digest = sha.ComputeHash(bytes);
                }

                var hasVariation = false;
                if (bytes.Length >= 8) {
                    var first = BitConverter.ToInt32(bytes, 0);
                    for (var index = 4; index + 3 < bytes.Length; index += 4) {
                        if (BitConverter.ToInt32(bytes, index) != first) {
                            hasVariation = true;
                            break;
                        }
                    }
                }
                return new ScreenFrame {
                    Hash = BitConverter.ToString(digest).Replace("-", String.Empty).ToLowerInvariant(),
                    HasVariation = hasVariation,
                    Width = width,
                    Height = height
                };
            }
            finally {
                bitmap.UnlockBits(data);
            }
        }
    }
}
'@
}

[CodexInfoWindowsGraphLatencyWin32]::SetProcessDPIAware() | Out-Null
$script:graphPhysicalInputSettleMilliseconds =
    [int][CodexInfoWindowsGraphLatencyWin32]::GetDoubleClickTime() + 50
$script:graphMenuPaintProbeExtraHeight = 72

function Complete-GraphPhysicalClickCycle {
    # P90/P95 stop at the first changed paint.  This interval is outside the
    # measurement and prevents the following sample from being interpreted as
    # the second half of the same Windows double-click gesture.
    [Threading.Thread]::Sleep($script:graphPhysicalInputSettleMilliseconds)
}

function Find-GraphWindow {
    param(
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [string]$Title = 'Codex Info Graph'
    )

    $script:graphLatencySearchProcessId = [uint32]$ProcessId
    $script:graphLatencySearchTitle = $Title
    $script:graphLatencySearchWindow = [IntPtr]::Zero
    $callback = [CodexInfoWindowsGraphLatencyWin32+EnumWindowsProc] {
        param([IntPtr]$Handle, [IntPtr]$Extra)
        [uint32]$owner = 0
        [CodexInfoWindowsGraphLatencyWin32]::GetWindowThreadProcessId($Handle, [ref]$owner) | Out-Null
        if ($owner -ne $script:graphLatencySearchProcessId -or
            -not [CodexInfoWindowsGraphLatencyWin32]::IsWindowVisible($Handle)) {
            return $true
        }
        $title = New-Object System.Text.StringBuilder 256
        [CodexInfoWindowsGraphLatencyWin32]::GetWindowText($Handle, $title, $title.Capacity) | Out-Null
        if ($title.ToString().IndexOf($script:graphLatencySearchTitle, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            $script:graphLatencySearchWindow = $Handle
            return $false
        }
        return $true
    }
    [CodexInfoWindowsGraphLatencyWin32]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
    return $script:graphLatencySearchWindow
}

function Bring-GraphWindowToFront {
    param([Parameter(Mandatory = $true)][IntPtr]$Handle)

    # Keep the measured HWND visible to GDI even when the runner has another
    # window in the foreground.  The process is terminated in every finally.
    [CodexInfoWindowsGraphLatencyWin32]::ShowWindow($Handle, 9) | Out-Null
    [CodexInfoWindowsGraphLatencyWin32]::SetWindowPos(
        $Handle,
        [IntPtr](-1),
        0,
        0,
        0,
        0,
        0x0001 -bor 0x0002 -bor 0x0040) | Out-Null
    [CodexInfoWindowsGraphLatencyWin32]::BringWindowToTop($Handle) | Out-Null
    [CodexInfoWindowsGraphLatencyWin32]::SetForegroundWindow($Handle) | Out-Null
    [CodexInfoWindowsGraphLatencyWin32]::SetCursorPos(10, 10) | Out-Null
}

function Wait-GraphInputTarget {
    param([Parameter(Mandatory = $true)][IntPtr]$Handle)

    Bring-GraphWindowToFront $Handle
    Wait-GraphLatencyPredicate -Description 'Graph foreground input target' -TimeoutMilliseconds 2000 -Probe {
        return [CodexInfoWindowsGraphLatencyWin32]::GetForegroundWindow() -eq $Handle
    } | Out-Null
}

function Get-GraphScreenFrame {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [System.Windows.Automation.AutomationElement]$ObservationElement = $null,
        [int]$ObservationExtraHeight = 0
    )

    if ($null -ne $ObservationElement) {
        $rectangle = $ObservationElement.Current.BoundingRectangle
        Assert-GraphLatency (-not $ObservationElement.Current.IsOffscreen -and
            $rectangle.Width -gt 0 -and $rectangle.Height -gt 0) 'Observed control has no visible bounds.'
        return [CodexInfoWindowsGraphLatencyWin32]::CaptureScreenFrame(
            $Handle,
            [int][Math]::Floor($rectangle.Left),
            [int][Math]::Floor($rectangle.Top),
            [int][Math]::Ceiling($rectangle.Width),
            [int][Math]::Ceiling($rectangle.Height) + $ObservationExtraHeight)
    }
    return [CodexInfoWindowsGraphLatencyWin32]::CaptureScreenFrame($Handle)
}

function Wait-GraphStableFrame {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [System.Windows.Automation.AutomationElement]$ObservationElement = $null,
        [int]$ObservationExtraHeight = 0,
        [int]$TimeoutMilliseconds = 2000
    )

    $deadline = [Diagnostics.Stopwatch]::GetTimestamp() +
        [long]($TimeoutMilliseconds * [Diagnostics.Stopwatch]::Frequency / 1000.0)
    $previous = $null
    $stableSince = 0L
    $minimumStableTicks = [long](25 * [Diagnostics.Stopwatch]::Frequency / 1000.0)
    while ([Diagnostics.Stopwatch]::GetTimestamp() -lt $deadline) {
        try {
            $current = Get-GraphScreenFrame $Handle $ObservationElement $ObservationExtraHeight
            if ($current.HasVariation -and $null -ne $previous -and $current.Hash -eq $previous.Hash) {
                if ($stableSince -eq 0) {
                    $stableSince = [Diagnostics.Stopwatch]::GetTimestamp()
                }
                elseif ([Diagnostics.Stopwatch]::GetTimestamp() - $stableSince -ge $minimumStableTicks) {
                    return $current
                }
            }
            else {
                $stableSince = 0L
            }
            $previous = $current
        }
        catch {
            # The window can be recreated during a render; keep polling the
            # same finite HWND until the bounded deadline.
        }
        [Threading.Thread]::Yield() | Out-Null
    }
    throw "TIMEOUT: stable GDI frame (${TimeoutMilliseconds}ms)"
}

function Wait-GraphFirstPaint {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][long]$StartTimestamp,
        [int]$TimeoutMilliseconds = 5000
    )

    $frame = Wait-GraphLatencyPredicate -Description 'Graph first GDI paint' -TimeoutMilliseconds $TimeoutMilliseconds -Probe {
        try {
            $candidate = Get-GraphScreenFrame $Handle
            if ($candidate.HasVariation -and $candidate.Hash.Length -eq 64) {
                return $candidate
            }
        }
        catch { }
        return $false
    }
    return [pscustomobject]@{
        latency_ms = Get-GraphLatencyMilliseconds $StartTimestamp
        paint_hash = $frame.Hash
        width = $frame.Width
        height = $frame.Height
    }
}

function Get-GraphUiaRoot {
    param([Parameter(Mandatory = $true)][IntPtr]$Handle)

    $root = [System.Windows.Automation.AutomationElement]::FromHandle($Handle)
    Assert-GraphLatency ($null -ne $root) 'Graph UI Automation root is missing.'
    return $root
}

function Find-GraphElementByAutomationId {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)][string]$AutomationId
    )

    $condition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
        $AutomationId)
    return $Root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
}

function Wait-GraphElementByAutomationId {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][string]$AutomationId,
        [int]$TimeoutMilliseconds = 5000
    )

    return Wait-GraphLatencyPredicate -Description "Graph element $AutomationId" -TimeoutMilliseconds $TimeoutMilliseconds -Probe {
        try {
            $root = Get-GraphUiaRoot $Handle
            $element = Find-GraphElementByAutomationId $root $AutomationId
            if ($null -ne $element -and $element.Current.IsEnabled) {
                return $element
            }
        }
        catch { }
        return $false
    }
}

function Get-GraphToggleState {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element)

    $pattern = $null
    Assert-GraphLatency ($Element.TryGetCurrentPattern(
        [System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) 'Graph toggle has no TogglePattern.'
    return $pattern.Current.ToggleState
}

function Invoke-GraphPhysicalClick {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element)

    $pressed = [CodexInfoWindowsGraphLatencyWin32]::SendMouseButton(0x0002)
    Assert-GraphLatency $pressed 'SendInput did not enqueue physical left-button down.'
    try {
        # Keep the button down for one 60 Hz frame.  A zero-duration synthetic
        # down/up batch can be consumed after Windows already reports the
        # button released, which is not representative of a human click.
        [Threading.Thread]::Sleep(16)
    }
    finally {
        $released = [CodexInfoWindowsGraphLatencyWin32]::SendMouseButton(0x0004)
    }
    Assert-GraphLatency $released 'SendInput did not enqueue physical left-button up.'
}

function Invoke-GraphEscape {
    param([Parameter(Mandatory = $true)][IntPtr]$Handle)

    Wait-GraphInputTarget $Handle
    Assert-GraphLatency ([CodexInfoWindowsGraphLatencyWin32]::SendKeyboardKey(0x1B, $false)) `
        'SendInput did not enqueue Escape key down.'
    try {
        [Threading.Thread]::Sleep(16)
    }
    finally {
        $released = [CodexInfoWindowsGraphLatencyWin32]::SendKeyboardKey(0x1B, $true)
    }
    Assert-GraphLatency $released 'SendInput did not enqueue Escape key up.'
}

function Position-GraphPointer {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element,
        [ValidateRange(0.1, 0.9)][double]$HorizontalFraction = 0.5
    )

    $rectangle = $Element.Current.BoundingRectangle
    Assert-GraphLatency (-not $Element.Current.IsOffscreen -and
        $rectangle.Width -gt 0 -and $rectangle.Height -gt 0) 'Graph control has no clickable bounds.'
    $x = [int][Math]::Round($rectangle.Left + $rectangle.Width * $HorizontalFraction)
    $y = [int][Math]::Round($rectangle.Top + $rectangle.Height / 2)
    Assert-GraphLatency ([CodexInfoWindowsGraphLatencyWin32]::SetCursorPos($x, $y)) 'SetCursorPos failed.'
}

function Get-VisibleGraphMenuItemCount {
    param([Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Root)

    $condition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::ListItem)
    $items = $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
    $count = 0
    foreach ($item in $items) {
        try {
            $rectangle = $item.Current.BoundingRectangle
            if (-not $item.Current.IsOffscreen -and $item.Current.IsEnabled -and
                $rectangle.Width -gt 0 -and $rectangle.Height -gt 0) {
                $count++
            }
        }
        catch { }
    }
    return $count
}

function Get-GraphMenuItemCount {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][string]$MenuAutomationId
    )

    try {
        $menu = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) $MenuAutomationId
        if ($null -eq $menu) { return -1 }
        return Get-VisibleGraphMenuItemCount $menu
    }
    catch {
        # Do not treat an unavailable UIA tree as a closed menu; that would
        # allow a postcondition to pass without observing the actual state.
        return -1
    }
}

function Wait-GraphToggleState {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][string]$AutomationId,
        [Parameter(Mandatory = $true)][System.Windows.Automation.ToggleState]$Expected,
        [int]$TimeoutMilliseconds = 2000
    )

    Wait-GraphLatencyPredicate -Description "$AutomationId state $Expected" -TimeoutMilliseconds $TimeoutMilliseconds -Probe {
        try {
            $element = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) $AutomationId
            if ($null -ne $element -and (Get-GraphToggleState $element) -eq $Expected) {
                return $true
            }
        }
        catch { }
        return $false
    } | Out-Null
}

function Wait-GraphMenuState {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][string]$MenuAutomationId,
        [Parameter(Mandatory = $true)][bool]$Open,
        [int]$TimeoutMilliseconds = 2000
    )

    $description = if ($Open) { "$MenuAutomationId open" } else { "$MenuAutomationId closed" }
    Wait-GraphLatencyPredicate -Description $description -TimeoutMilliseconds $TimeoutMilliseconds -Probe {
        try {
            $menu = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) $MenuAutomationId
            if ($null -eq $menu) { return $false }
            $count = Get-GraphMenuItemCount $Handle $MenuAutomationId
            if ($Open) {
                $bounds = $menu.Current.BoundingRectangle
                return $menu.Current.IsEnabled -and -not $menu.Current.IsOffscreen -and
                    $bounds.Width -gt 0 -and $bounds.Height -gt 0 -and $count -gt 0
            }
            return -not $menu.Current.IsEnabled -and $count -eq 0
        }
        catch { return $false }
    } | Out-Null
}

function Measure-GraphActionPaint {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element,
        [Parameter(Mandatory = $true)][ValidateSet('toggle')][string]$Action,
        [Parameter(Mandatory = $true)][string]$Description,
        [int]$ObservationExtraHeight = 0,
        [int]$TimeoutMilliseconds = 2000
    )

    # Move before the measurement so hover paint is not mistaken for click
    # acknowledgement. Establish a stable baseline before the input timestamp.
    Wait-GraphInputTarget $Handle
    $script:graphPhysicalInputSequence++
    $horizontalFraction = if (($script:graphPhysicalInputSequence % 2) -eq 0) { 0.35 } else { 0.65 }
    Position-GraphPointer -Element $Element -HorizontalFraction $horizontalFraction
    # Thus neither
    # baseline capture nor a fixed probe sleep is charged to the latency.
    # The control surface is the first user-visible acknowledgement. Avalonia
    # presents it in the same window frame as the plot invalidation, while the
    # small region keeps GDI observer cost below the interaction budget.
    $baseline = Wait-GraphStableFrame -Handle $Handle -ObservationElement $Element `
        -ObservationExtraHeight $ObservationExtraHeight -TimeoutMilliseconds $TimeoutMilliseconds
    $inputStart = [Diagnostics.Stopwatch]::GetTimestamp()
    switch ($Action) {
        'toggle' { Invoke-GraphPhysicalClick $Element }
        default { throw "Unsupported measured action: $Action" }
    }

    $deadline = [Diagnostics.Stopwatch]::GetTimestamp() +
        [long]($TimeoutMilliseconds * [Diagnostics.Stopwatch]::Frequency / 1000.0)
    $paint = $null
    while ([Diagnostics.Stopwatch]::GetTimestamp() -lt $deadline) {
        try {
            $candidate = Get-GraphScreenFrame $Handle $Element $ObservationExtraHeight
            # UIA may publish the state before the compositor presents it.  A
            # changed GDI hash is the only completion condition for this sample.
            if ($candidate.Hash -ne $baseline.Hash) {
                $paint = $candidate
                break
            }
        }
        catch { }
        [Threading.Thread]::Yield() | Out-Null
    }
    if ($null -eq $paint) {
        throw "TIMEOUT: $Description did not produce a changed GDI frame (${TimeoutMilliseconds}ms)"
    }
    $elapsed = Get-GraphLatencyMilliseconds $inputStart
    return [pscustomobject]@{
        latency_ms = $elapsed
        baseline_hash = $baseline.Hash
        paint_hash = $paint.Hash
        width = $paint.Width
        height = $paint.Height
    }
}

function Measure-GraphControlStateAction {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Element,
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$PlotElement,
        [Parameter(Mandatory = $true)][string]$AutomationId,
        [System.Windows.Automation.ToggleState]$ExpectedToggleState,
        [ValidateSet('True', 'False')][string]$ExpectedHelpText,
        [Parameter(Mandatory = $true)][string]$Description,
        [int]$TimeoutMilliseconds = 2000
    )

    $expectsHelpText = $PSBoundParameters.ContainsKey('ExpectedHelpText')
    $expectsToggleState = $PSBoundParameters.ContainsKey('ExpectedToggleState')
    Assert-GraphLatency ($expectsHelpText -xor $expectsToggleState) `
        'Provide exactly one expected ToggleState or HelpText value.'
    $expectedState = if ($expectsHelpText) { $ExpectedHelpText } else { [string]$ExpectedToggleState }

    # Establish both baselines before the physical input timestamp. The small
    # control surface is the user's immediate acknowledgement. Capturing and
    # hashing the whole plot is substantially slower on hosted Windows agents,
    # so it remains a mandatory correctness postcondition without polluting
    # the acknowledgement P90/P95 with observer cost.
    Wait-GraphInputTarget $Handle
    $script:graphPhysicalInputSequence++
    $horizontalFraction = if (($script:graphPhysicalInputSequence % 2) -eq 0) { 0.35 } else { 0.65 }
    Position-GraphPointer -Element $Element -HorizontalFraction $horizontalFraction
    $plotObservation = $PlotElement
    try {
        $freshPlot = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) 'Graph.Plot'
        if ($null -ne $freshPlot) { $plotObservation = $freshPlot }
    }
    catch { }
    $plotBaseline = Wait-GraphStableFrame -Handle $Handle -ObservationElement $plotObservation `
        -TimeoutMilliseconds $TimeoutMilliseconds
    $controlBaseline = Wait-GraphStableFrame -Handle $Handle -ObservationElement $Element `
        -TimeoutMilliseconds $TimeoutMilliseconds
    $baselineState = if ($expectsHelpText) {
        [string]$Element.Current.HelpText
    }
    else {
        [string](Get-GraphToggleState $Element)
    }
    Assert-GraphLatency ($baselineState -ne $expectedState) `
        "$Description started in the expected state $expectedState."

    $inputStart = [Diagnostics.Stopwatch]::GetTimestamp()
    Invoke-GraphPhysicalClick $Element
    $deadline = [Diagnostics.Stopwatch]::GetTimestamp() +
        [long]($TimeoutMilliseconds * [Diagnostics.Stopwatch]::Frequency / 1000.0)
    $controlPaint = $null
    $observedState = $null
    $stateObserved = $false
    while ([Diagnostics.Stopwatch]::GetTimestamp() -lt $deadline) {
        try {
            $currentElement = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) $AutomationId
            if ($null -ne $currentElement) {
                $currentState = if ($expectsHelpText) {
                    [string]$currentElement.Current.HelpText
                }
                else {
                    [string](Get-GraphToggleState $currentElement)
                }
                $observedState = $currentState
                if ($currentState -eq $expectedState) {
                    $stateObserved = $true
                }
                if ($stateObserved -and $null -eq $controlPaint) {
                    $candidateControl = Get-GraphScreenFrame $Handle $currentElement
                    if ($candidateControl.Hash -ne $controlBaseline.Hash) {
                        $controlPaint = $candidateControl
                    }
                }
            }
        }
        catch { }
        if ($stateObserved -and $null -ne $controlPaint) { break }
        [Threading.Thread]::Yield() | Out-Null
    }
    if (-not $stateObserved -or $null -eq $controlPaint) {
        $observedStateText = if ($null -eq $observedState) { 'unobserved' } else { [string]$observedState }
        Write-GraphLatencyLog ("control-ack-timeout: name={0} expected_state={1} observed_state={2} state_observed={3} baseline_hash={4}" -f
            $Description, $expectedState, $observedStateText, $stateObserved, $controlBaseline.Hash)
        throw "TIMEOUT: $Description did not observe state $expectedState and changed control paint (${TimeoutMilliseconds}ms)"
    }

    $elapsed = Get-GraphLatencyMilliseconds $inputStart
    $plotPaint = $null
    while ([Diagnostics.Stopwatch]::GetTimestamp() -lt $deadline) {
        try {
            $plotObservation = $PlotElement
            $freshPlot = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) 'Graph.Plot'
            if ($null -ne $freshPlot) { $plotObservation = $freshPlot }
            $candidatePlot = Get-GraphScreenFrame $Handle $plotObservation
            if ($candidatePlot.Hash -ne $plotBaseline.Hash) {
                $plotPaint = $candidatePlot
                break
            }
        }
        catch { }
        [Threading.Thread]::Yield() | Out-Null
    }
    if ($null -eq $plotPaint) {
        Write-GraphLatencyLog ("control-plot-timeout: name={0} baseline_plot_hash={1}" -f
            $Description, $plotBaseline.Hash)
        throw "TIMEOUT: $Description did not produce a changed Graph.Plot frame (${TimeoutMilliseconds}ms)"
    }

    return [pscustomobject]@{
        latency_ms = $elapsed
        baseline_hash = $controlBaseline.Hash
        paint_hash = $controlPaint.Hash
        toggle_baseline_hash = if ($expectsToggleState) { $controlBaseline.Hash } else { $null }
        toggle_paint_hash = if ($expectsToggleState) { $controlPaint.Hash } else { $null }
        selection_baseline_hash = if ($expectsHelpText) { $controlBaseline.Hash } else { $null }
        selection_paint_hash = if ($expectsHelpText) { $controlPaint.Hash } else { $null }
        plot_baseline_hash = $plotBaseline.Hash
        plot_hash = $plotPaint.Hash
        state_kind = if ($expectsHelpText) { 'help_text' } else { 'toggle_state' }
        expected_state = $expectedState
        observed_state = [string]$observedState
        state_observed = $stateObserved
        expected_toggle_state = if ($expectsToggleState) { $expectedState } else { $null }
        observed_toggle_state = if ($expectsToggleState) { [string]$observedState } else { $null }
        toggle_state_observed = $expectsToggleState -and $stateObserved
        expected_help_text = if ($expectsHelpText) { $expectedState } else { $null }
        observed_help_text = if ($expectsHelpText) { [string]$observedState } else { $null }
        help_text_observed = $expectsHelpText -and $stateObserved
        graph_plot_changed = $true
        width = $controlPaint.Width
        height = $controlPaint.Height
    }
}

function Get-GraphLatencyStats {
    param(
        [Parameter(Mandatory = $true)][object[]]$Samples,
        [Parameter(Mandatory = $true)][string]$Description
    )

    $values = @($Samples | ForEach-Object { [double]$_.latency_ms } | Sort-Object)
    Assert-GraphLatency ($values.Count -ge $Iterations) "$Description has fewer than $Iterations samples."
    $p90Index = [Math]::Max(0, [int][Math]::Ceiling($values.Count * 0.90) - 1)
    $p95Index = [Math]::Max(0, [int][Math]::Ceiling($values.Count * 0.95) - 1)
    return [pscustomobject]@{
        count = $values.Count
        p90_ms = $values[$p90Index]
        p95_ms = $values[$p95Index]
        max_ms = $values[-1]
        min_ms = $values[0]
    }
}

function Get-GraphLatencyDiagnosticStats {
    param([object[]]$Samples)

    $values = @($Samples | ForEach-Object { [double]$_.latency_ms } | Sort-Object)
    if ($values.Count -eq 0) {
        return [ordered]@{
            count = 0
            p90_ms = $null
            p95_ms = $null
            max_ms = $null
            min_ms = $null
        }
    }
    $p90Index = [Math]::Max(0, [int][Math]::Ceiling($values.Count * 0.90) - 1)
    $p95Index = [Math]::Max(0, [int][Math]::Ceiling($values.Count * 0.95) - 1)
    return [ordered]@{
        count = $values.Count
        p90_ms = $values[$p90Index]
        p95_ms = $values[$p95Index]
        max_ms = $values[-1]
        min_ms = $values[0]
    }
}

function Assert-GraphLatencyBudget {
    param(
        [Parameter(Mandatory = $true)][psobject]$Stats,
        [Parameter(Mandatory = $true)][double]$P90Limit,
        [Parameter(Mandatory = $true)][double]$P95Limit,
        [Parameter(Mandatory = $true)][double]$ColdLimit,
        [Parameter(Mandatory = $true)][double]$ColdMilliseconds,
        [Parameter(Mandatory = $true)][string]$Description
    )

    Assert-GraphLatency ($Stats.p90_ms -le $P90Limit) "$Description P90 $($Stats.p90_ms)ms exceeds ${P90Limit}ms."
    Assert-GraphLatency ($Stats.p95_ms -le $P95Limit) "$Description P95 $($Stats.p95_ms)ms exceeds ${P95Limit}ms."
    Assert-GraphLatency ($ColdMilliseconds -le $ColdLimit) "$Description cold interaction $ColdMilliseconds ms exceeds ${ColdLimit}ms."
}

function Measure-GraphToggleSeries {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$PlotElement,
        [Parameter(Mandatory = $true)][string]$AutomationId,
        [Parameter(Mandatory = $true)][string]$Name,
        [int]$Count = 30
    )

    $samples = [System.Collections.Generic.List[object]]::new()
    $activeControl = [ordered]@{
        name = $Name
        automation_id = $AutomationId
        plot_automation_id = 'Graph.Plot'
        kind = 'toggle'
        phase = 'starting'
        samples = @()
        stats = Get-GraphLatencyDiagnosticStats -Samples @()
    }
    if ($null -ne $script:graphLatencyActiveCase) {
        $script:graphLatencyActiveCase['controls'] = @($script:graphLatencyActiveCase['controls']) + @($activeControl)
        $script:graphLatencyActiveCase['phase'] = "toggle:$Name"
        Save-GraphLatencyCheckpoint "toggle-start:$Name"
    }
    $first = Wait-GraphElementByAutomationId $Handle $AutomationId
    $state = Get-GraphToggleState $first
    for ($index = 1; $index -le $Count; $index++) {
        $element = Wait-GraphElementByAutomationId $Handle $AutomationId
        $expected = if ($state -eq [System.Windows.Automation.ToggleState]::On) {
            [System.Windows.Automation.ToggleState]::Off
        }
        else {
            [System.Windows.Automation.ToggleState]::On
        }
        $activeControl['phase'] = "sample-$index-input"
        $sample = Measure-GraphControlStateAction -Handle $Handle -Element $element -PlotElement $PlotElement `
            -AutomationId $AutomationId -ExpectedToggleState $expected `
            -Description "$Name sample $index" -TimeoutMilliseconds $PaintTimeoutMilliseconds
        $sampleRecord = [pscustomobject]@{
            index = $index
            latency_ms = $sample.latency_ms
            state = [string]$expected
            baseline_hash = $sample.baseline_hash
            paint_hash = $sample.paint_hash
            plot_automation_id = 'Graph.Plot'
            plot_baseline_hash = $sample.plot_baseline_hash
            plot_hash = $sample.plot_hash
            toggle_state_observed = [bool]$sample.toggle_state_observed
            plot_changed = [bool]$sample.graph_plot_changed
        }
        $samples.Add($sampleRecord)
        $activeControl['samples'] = @($activeControl['samples']) + @($sampleRecord)
        $activeControl['stats'] = Get-GraphLatencyDiagnosticStats -Samples @($activeControl['samples'])
        $activeControl['completed_samples'] = $index
        $activeControl['phase'] = "sample-$index-observed"
        Write-GraphLatencyLog ("sample: kind=toggle name={0} index={1} latency_ms={2} expected_state={3} graph_plot_changed={4}" -f
            $Name, $index, $sample.latency_ms, $expected, $sample.graph_plot_changed)
        Wait-GraphToggleState -Handle $Handle -AutomationId $AutomationId -Expected $expected
        $state = $expected
        Complete-GraphPhysicalClickCycle
    }
    $stats = Get-GraphLatencyStats -Samples @($samples) -Description $Name
    $coldMilliseconds = [double]$samples[0].latency_ms
    $activeControl['stats'] = $stats
    $activeControl['cold_ms'] = $coldMilliseconds
    $activeControl['phase'] = 'budget-check'
    Write-GraphLatencyLog ("series-stats: kind=toggle name={0} count={1} p90_ms={2} p95_ms={3} cold_ms={4}" -f
        $Name, $stats.count, $stats.p90_ms, $stats.p95_ms, $coldMilliseconds)
    Save-GraphLatencyCheckpoint "toggle-stats:$Name"
    Assert-GraphLatencyBudget -Stats $stats -P90Limit 75 -P95Limit 100 `
        -ColdLimit 250 -ColdMilliseconds $coldMilliseconds -Description "toggle $Name"
    $activeControl['phase'] = 'pass'
    return [pscustomobject]@{
        name = $Name
        automation_id = $AutomationId
        plot_automation_id = 'Graph.Plot'
        kind = 'toggle'
        samples = @($samples)
        stats = $stats
        cold_ms = $coldMilliseconds
        budget_ms = [ordered]@{ p90 = 75; p95 = 100; cold_max = 250 }
    }
}

function Measure-GraphMenu {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][string]$AutomationId,
        [Parameter(Mandatory = $true)][string]$MenuAutomationId,
        [Parameter(Mandatory = $true)][string]$Name,
        [int]$Count = 30
    )

    # Every sample has the same finite meaning: a closed list is expanded by
    # one physical click.  Closing is test setup for the next sample, not part
    # of the expansion-latency distribution, and uses the physical Escape path
    # after the measured paint and open postcondition have passed.
    Wait-GraphElementByAutomationId $Handle $AutomationId | Out-Null
    $initialMenu = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) $MenuAutomationId
    Assert-GraphLatency ($null -ne $initialMenu) "$Name menu UIA root is missing."
    Assert-GraphLatency ((Get-GraphMenuItemCount $Handle $MenuAutomationId) -eq 0) "$Name menu did not start closed."

    $samples = [System.Collections.Generic.List[object]]::new()
    for ($index = 1; $index -le $Count; $index++) {
        $element = Wait-GraphElementByAutomationId $Handle $AutomationId
        Assert-GraphLatency ((Get-GraphToggleState $element) -eq [System.Windows.Automation.ToggleState]::Off) `
            "$Name selector did not start sample $index closed."
        $sample = Measure-GraphActionPaint -Handle $Handle -Element $element -Action toggle `
            -Description "$Name sample $index" -ObservationExtraHeight $script:graphMenuPaintProbeExtraHeight `
            -TimeoutMilliseconds $PaintTimeoutMilliseconds
        try {
            Wait-GraphToggleState -Handle $Handle -AutomationId $AutomationId `
                -Expected ([System.Windows.Automation.ToggleState]::On)
            Wait-GraphMenuState -Handle $Handle -MenuAutomationId $MenuAutomationId -Open $true
        }
        catch {
            $selectorState = 'unavailable'
            $menuEnabled = 'unavailable'
            $menuOffscreen = 'unavailable'
            $menuBounds = 'unavailable'
            $cursor = New-Object CodexInfoWindowsGraphLatencyWin32+POINT
            try {
                $currentSelector = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) $AutomationId
                if ($null -ne $currentSelector) { $selectorState = [string](Get-GraphToggleState $currentSelector) }
                $currentMenu = Find-GraphElementByAutomationId (Get-GraphUiaRoot $Handle) $MenuAutomationId
                if ($null -ne $currentMenu) {
                    $menuEnabled = [string]$currentMenu.Current.IsEnabled
                    $menuOffscreen = [string]$currentMenu.Current.IsOffscreen
                    $bounds = $currentMenu.Current.BoundingRectangle
                    $menuBounds = '{0}x{1}+{2}+{3}' -f $bounds.Width, $bounds.Height, $bounds.Left, $bounds.Top
                }
            }
            catch { }
            [CodexInfoWindowsGraphLatencyWin32]::GetCursorPos([ref]$cursor) | Out-Null
            $foreground = [CodexInfoWindowsGraphLatencyWin32]::GetForegroundWindow()
            $failureImage = Join-Path $script:graphLatencyOutput ("failure-{0}-{1}.png" -f $Name, $index)
            try { [CodexInfoWindowsGraphLatencyWin32]::CaptureWindowPng($Handle, $failureImage) } catch { }
            Write-GraphLatencyLog ("menu-transition-fail: name={0} index={1} expected_open=true selector_state={2} menu_enabled={3} menu_offscreen={4} menu_bounds={5} items={6} foreground=0x{7:X} expected_hwnd=0x{8:X} cursor={9},{10} image={11}" -f
                $Name, $index, $selectorState, $menuEnabled, $menuOffscreen,
                $menuBounds, (Get-GraphMenuItemCount $Handle $MenuAutomationId), $foreground.ToInt64(), $Handle.ToInt64(),
                $cursor.X, $cursor.Y, $failureImage)
            throw
        }
        Complete-GraphPhysicalClickCycle
        $samples.Add([pscustomobject]@{
            index = $index
            latency_ms = $sample.latency_ms
            open = $true
            baseline_hash = $sample.baseline_hash
            paint_hash = $sample.paint_hash
        })
        Invoke-GraphEscape $Handle
        Wait-GraphToggleState -Handle $Handle -AutomationId $AutomationId `
            -Expected ([System.Windows.Automation.ToggleState]::Off)
        Wait-GraphMenuState -Handle $Handle -MenuAutomationId $MenuAutomationId -Open $false
    }
    $stats = Get-GraphLatencyStats -Samples @($samples) -Description $Name
    $coldMilliseconds = [double]$samples[0].latency_ms
    Assert-GraphLatencyBudget -Stats $stats -P90Limit 100 -P95Limit 150 `
        -ColdLimit 250 -ColdMilliseconds $coldMilliseconds -Description "menu $Name"
    return [pscustomobject]@{
        name = $Name
        automation_id = $AutomationId
        kind = 'menu'
        samples = @($samples)
        stats = $stats
        cold_ms = $coldMilliseconds
        budget_ms = [ordered]@{ p90 = 100; p95 = 150; cold_max = 250 }
    }
}

function Assert-GraphMenuRoundTripAfterToggles {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][string]$AutomationId,
        [Parameter(Mandatory = $true)][string]$MenuAutomationId,
        [Parameter(Mandatory = $true)][string]$Name
    )

    $open = Wait-GraphElementByAutomationId $Handle $AutomationId
    Measure-GraphActionPaint -Handle $Handle -Element $open -Action toggle `
        -Description "$Name post-toggle verification open" -ObservationExtraHeight $script:graphMenuPaintProbeExtraHeight `
        -TimeoutMilliseconds $PaintTimeoutMilliseconds | Out-Null
    Wait-GraphToggleState -Handle $Handle -AutomationId $AutomationId `
        -Expected ([System.Windows.Automation.ToggleState]::On)
    Wait-GraphMenuState -Handle $Handle -MenuAutomationId $MenuAutomationId -Open $true
    Complete-GraphPhysicalClickCycle

    Invoke-GraphEscape $Handle
    Wait-GraphToggleState -Handle $Handle -AutomationId $AutomationId `
        -Expected ([System.Windows.Automation.ToggleState]::Off)
    Wait-GraphMenuState -Handle $Handle -MenuAutomationId $MenuAutomationId -Open $false
    Write-GraphLatencyLog "post-toggle-menu-round-trip: name=$Name PASS"
}

function Assert-GraphMetricSelectionState {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][ValidateSet('Tokens', 'Dollars')][string]$SelectedMetric
    )

    $otherMetric = if ($SelectedMetric -eq 'Tokens') { 'Dollars' } else { 'Tokens' }
    $selectedButton = Wait-GraphElementByAutomationId $Handle "Graph.Metric.$SelectedMetric"
    $otherButton = Wait-GraphElementByAutomationId $Handle "Graph.Metric.$otherMetric"
    Assert-GraphLatency ([string]$selectedButton.Current.HelpText -eq 'True') `
        "Graph metric $SelectedMetric is not selected."
    Assert-GraphLatency ([string]$otherButton.Current.HelpText -eq 'False') `
        "Graph metric $otherMetric is not unselected."
}

function Measure-GraphMetricButtonSwitch {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Handle,
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$PlotElement,
        [int]$Count = 30
    )

    $metricAutomationIds = @('Graph.Metric.Tokens', 'Graph.Metric.Dollars')
    $samples = [System.Collections.Generic.List[object]]::new()
    $activeControl = [ordered]@{
        name = 'metric'
        automation_id = 'Graph.Metric.Tokens'
        automation_ids = $metricAutomationIds
        plot_automation_id = 'Graph.Plot'
        kind = 'metric-button-switch'
        phase = 'starting'
        samples = @()
        stats = Get-GraphLatencyDiagnosticStats -Samples @()
    }
    if ($null -ne $script:graphLatencyActiveCase) {
        $script:graphLatencyActiveCase['controls'] = @($script:graphLatencyActiveCase['controls']) + @($activeControl)
        $script:graphLatencyActiveCase['phase'] = 'metric-button-switch'
        Save-GraphLatencyCheckpoint 'metric-button-switch-start'
    }

    Assert-GraphMetricSelectionState -Handle $Handle -SelectedMetric 'Tokens'
    $selectedMetric = 'Tokens'
    for ($index = 1; $index -le $Count; $index++) {
        $expectedMetric = if ($selectedMetric -eq 'Tokens') { 'Dollars' } else { 'Tokens' }
        $automationId = "Graph.Metric.$expectedMetric"
        $element = Wait-GraphElementByAutomationId $Handle $automationId
        Assert-GraphLatency ([string]$element.Current.HelpText -eq 'False') `
            "$automationId was already selected before sample $index."

        $activeControl['phase'] = "sample-$index-input"
        $sample = Measure-GraphControlStateAction -Handle $Handle -Element $element -PlotElement $PlotElement `
            -AutomationId $automationId -ExpectedHelpText 'True' `
            -Description "metric $expectedMetric sample $index" -TimeoutMilliseconds $PaintTimeoutMilliseconds
        Assert-GraphMetricSelectionState -Handle $Handle -SelectedMetric $expectedMetric

        $sampleRecord = [pscustomobject]@{
            index = $index
            metric = $expectedMetric
            automation_id = $automationId
            latency_ms = $sample.latency_ms
            expected_help_text = $sample.expected_help_text
            observed_help_text = $sample.observed_help_text
            help_text_observed = [bool]$sample.help_text_observed
            baseline_hash = $sample.selection_baseline_hash
            paint_hash = $sample.selection_paint_hash
            plot_automation_id = 'Graph.Plot'
            plot_baseline_hash = $sample.plot_baseline_hash
            plot_hash = $sample.plot_hash
            plot_changed = [bool]$sample.graph_plot_changed
        }
        $samples.Add($sampleRecord)
        $activeControl['samples'] = @($activeControl['samples']) + @($sampleRecord)
        $activeControl['stats'] = Get-GraphLatencyDiagnosticStats -Samples @($activeControl['samples'])
        $activeControl['completed_samples'] = $index
        $activeControl['phase'] = "sample-$index-observed"
        Write-GraphLatencyLog ("sample: kind=metric-button-switch index={0} metric={1} latency_ms={2} help_text={3} graph_plot_changed={4}" -f
            $index, $expectedMetric, $sample.latency_ms, $sample.observed_help_text, $sample.graph_plot_changed)
        $selectedMetric = $expectedMetric
        Complete-GraphPhysicalClickCycle
    }

    $stats = Get-GraphLatencyStats -Samples @($samples) -Description 'metric button switches'
    $coldMilliseconds = [double]$samples[0].latency_ms
    $activeControl['stats'] = $stats
    $activeControl['cold_ms'] = $coldMilliseconds
    $activeControl['phase'] = 'budget-check'
    Write-GraphLatencyLog ("metric-button-switch-stats: count={0} p90_ms={1} p95_ms={2} cold_ms={3}" -f
        $stats.count, $stats.p90_ms, $stats.p95_ms, $coldMilliseconds)
    Save-GraphLatencyCheckpoint 'metric-button-switch-stats'
    # Retain the existing metric control limits while measuring its new direct-button action.
    Assert-GraphLatencyBudget -Stats $stats -P90Limit 100 -P95Limit 150 `
        -ColdLimit 250 -ColdMilliseconds $coldMilliseconds -Description 'metric button switches'
    $activeControl['phase'] = 'pass'
    return [pscustomobject]@{
        name = 'metric'
        automation_id = 'Graph.Metric.Tokens'
        automation_ids = $metricAutomationIds
        plot_automation_id = 'Graph.Plot'
        kind = 'metric-button-switch'
        samples = @($samples)
        stats = $stats
        cold_ms = $coldMilliseconds
        budget_ms = [ordered]@{ p90 = 100; p95 = 150; cold_max = 250 }
    }
}

function Invoke-GraphPointCase {
    param(
        [Parameter(Mandatory = $true)][string]$ResolvedClientPath,
        [Parameter(Mandatory = $true)][int]$Points
    )

    $env:CODEX_INFO_WINDOWS_PREVIEW = 'graph'
    $env:CODEX_INFO_WINDOWS_PREVIEW_SIZE = '940x640'
    $env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS = [string]$Points
    $env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS = '0'
    $script:graphLatencyActiveCase = [ordered]@{
        points = $Points
        logical_size = '940x640'
        phase = 'starting'
        process_id = $null
        window_handle = $null
        controls = @()
    }
    $script:graphLatencyReport['active_case'] = $script:graphLatencyActiveCase
    Save-GraphLatencyCheckpoint "case-start:$Points"
    $process = $null
    try {
        Write-GraphLatencyLog "case-start: points=$Points size=940x640"
        $startupStart = [Diagnostics.Stopwatch]::GetTimestamp()
        $process = Start-Process -FilePath $ResolvedClientPath -PassThru
        $script:graphLatencyActiveCase['process_id'] = $process.Id
        $script:graphLatencyActiveCase['phase'] = 'window-discovery'
        $window = Wait-GraphLatencyPredicate -Description "fresh Graph window for $Points points" -TimeoutMilliseconds 10000 -Probe {
            if ($process.HasExited) { return $false }
            $candidate = Find-GraphWindow -ProcessId $process.Id
            if ($candidate -eq [IntPtr]::Zero) { return $false }
            return $candidate
        }
        Bring-GraphWindowToFront $window
        $script:graphLatencyActiveCase['window_handle'] = ('0x{0:X}' -f $window.ToInt64())
        $script:graphLatencyActiveCase['phase'] = 'startup-paint'
        # Process startup is not the click-to-paint SLO.  Keep it as a
        # diagnostic while applying the 250 ms cold limit to the first real
        # interaction of every selector/toggle below.
        $startup = Wait-GraphFirstPaint -Handle $window -StartTimestamp $startupStart -TimeoutMilliseconds 10000
        Write-GraphLatencyLog ("startup: points={0} latency_ms={1} gdi_hash={2} bounds={3}x{4}" -f
            $Points, $startup.latency_ms, $startup.paint_hash, $startup.width, $startup.height)
        $script:graphLatencyActiveCase['startup_first_paint'] = $startup
        $script:graphLatencyActiveCase['phase'] = 'controls'
        Save-GraphLatencyCheckpoint "startup:$Points"

        $plot = Wait-GraphElementByAutomationId -Handle $window -AutomationId 'Graph.Plot'
        Assert-GraphLatency ($plot.Current.BoundingRectangle.Width -gt 0 -and
            $plot.Current.BoundingRectangle.Height -gt 0) 'Graph plot has no rendered bounds.'

        $controls = [System.Collections.Generic.List[object]]::new()
        $controls.Add((Measure-GraphMenu -Handle $window -AutomationId 'Graph.PeriodSelector' `
            -MenuAutomationId 'Graph.PeriodMenu' -Name 'period' -Count $Iterations))
        $controls.Add((Measure-GraphMetricButtonSwitch -Handle $window -PlotElement $plot -Count $Iterations))
        $toggleCases = @(
            @{ Id = 'Graph.Toggle.Remaining'; Name = 'Remaining' },
            @{ Id = 'Graph.Toggle.LUNA'; Name = 'LUNA' },
            @{ Id = 'Graph.Toggle.TERRA'; Name = 'TERRA' },
            @{ Id = 'Graph.Toggle.SOL'; Name = 'SOL' }
        )
        foreach ($toggleCase in $toggleCases) {
            $controls.Add((Measure-GraphToggleSeries -Handle $window -PlotElement $plot -AutomationId $toggleCase.Id `
                -Name $toggleCase.Name -Count $Iterations))
        }
        Assert-GraphMenuRoundTripAfterToggles -Handle $window -AutomationId 'Graph.PeriodSelector' `
            -MenuAutomationId 'Graph.PeriodMenu' -Name 'period'
        $toggleSamples = @($controls | Where-Object { $_.kind -eq 'toggle' } |
            ForEach-Object { $_.samples })
        $menuSamples = @($controls | Where-Object { $_.kind -eq 'menu' } |
            ForEach-Object { $_.samples })
        $metricButtonSamples = @($controls | Where-Object { $_.kind -eq 'metric-button-switch' } |
            ForEach-Object { $_.samples })
        $toggleStats = Get-GraphLatencyStats -Samples $toggleSamples -Description 'all toggle samples'
        $menuStats = Get-GraphLatencyStats -Samples $menuSamples -Description 'all menu samples'
        $metricButtonStats = Get-GraphLatencyStats -Samples $metricButtonSamples -Description 'metric button samples'
        $toggleColdMax = (@($controls | Where-Object { $_.kind -eq 'toggle' } |
            ForEach-Object { [double]$_.cold_ms } | Measure-Object -Maximum).Maximum)
        $menuColdMax = (@($controls | Where-Object { $_.kind -eq 'menu' } |
            ForEach-Object { [double]$_.cold_ms } | Measure-Object -Maximum).Maximum)
        $metricButtonColdMax = (@($controls | Where-Object { $_.kind -eq 'metric-button-switch' } |
            ForEach-Object { [double]$_.cold_ms } | Measure-Object -Maximum).Maximum)
        Assert-GraphLatencyBudget -Stats $toggleStats -P90Limit 75 -P95Limit 100 `
            -ColdLimit 250 -ColdMilliseconds $toggleColdMax -Description 'all toggles'
        Assert-GraphLatencyBudget -Stats $menuStats -P90Limit 100 -P95Limit 150 `
            -ColdLimit 250 -ColdMilliseconds $menuColdMax -Description 'all menus'
        Assert-GraphLatencyBudget -Stats $metricButtonStats -P90Limit 100 -P95Limit 150 `
            -ColdLimit 250 -ColdMilliseconds $metricButtonColdMax -Description 'metric button switches'
        $script:graphLatencyActiveCase['phase'] = 'pass'
        $script:graphLatencyActiveCase['aggregate'] = [ordered]@{
            toggles = $toggleStats
            menus = $menuStats
            metric_button_switches = $metricButtonStats
        }
        Save-GraphLatencyCheckpoint "case-pass:$Points"
        Write-GraphLatencyLog ("case-pass: points={0} toggle_p90_ms={1} toggle_p95_ms={2} menu_p90_ms={3} menu_p95_ms={4} metric_button_p90_ms={5} metric_button_p95_ms={6}" -f
            $Points, $toggleStats.p90_ms, $toggleStats.p95_ms, $menuStats.p90_ms, $menuStats.p95_ms,
            $metricButtonStats.p90_ms, $metricButtonStats.p95_ms)
        return [pscustomobject]@{
            points = $Points
            logical_size = '940x640'
            process_id = $process.Id
            window_handle = ('0x{0:X}' -f $window.ToInt64())
            startup_first_paint = $startup
            controls = @($controls)
            aggregate = [ordered]@{
                toggles = $toggleStats
                menus = $menuStats
                metric_button_switches = $metricButtonStats
            }
        }
    }
    finally {
        if ($null -ne $process) {
            try {
                if (-not $process.HasExited) {
                    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
                }
                $process.WaitForExit(3000) | Out-Null
            }
            catch { }
        }
    }
}

$resolvedClientPath = if ([string]::IsNullOrWhiteSpace($ClientPath)) {
    Join-Path $env:LOCALAPPDATA 'Programs\Codex Info Monitor\CodexInfo.WindowsClient.exe'
}
else {
    [IO.Path]::GetFullPath($ClientPath)
}
$script:graphLatencyReport = [ordered]@{
    schema_version = 1
    status = 'RUNNING'
    started_at = [DateTimeOffset]::Now.ToString('o')
    client_path = $resolvedClientPath
    preview = 'graph'
    logical_size = '940x640'
    points = @(10080, 44640)
    iterations = $Iterations
    thresholds_ms = [ordered]@{
        toggle_p90 = 75
        toggle_p95 = 100
        menu_p90 = 100
        menu_p95 = 150
        metric_button_switch_p90 = 100
        metric_button_switch_p95 = 150
        cold_max = 250
    }
    physical_input = [ordered]@{
        provider = 'SendInput'
        independent_click_settle_ms = $script:graphPhysicalInputSettleMilliseconds
    }
    cases = @()
}

function Save-GraphLatencyReport {
    [IO.File]::WriteAllText(
        $script:graphLatencyReportPath,
        ($script:graphLatencyReport | ConvertTo-Json -Depth 30),
        [System.Text.UTF8Encoding]::new($false))
}

function Save-GraphLatencyCheckpoint {
    param([Parameter(Mandatory = $true)][string]$Reason)

    try {
        Save-GraphLatencyReport
    }
    catch {
        # A report write must not hide the measured failure.  The final log
        # still records this checkpoint failure and the top-level catch makes
        # one last best-effort JSON write.
        Write-GraphLatencyLog ("report-checkpoint-fail: reason={0} error={1}" -f $Reason, $_.Exception.Message)
    }
}

$oldPreview = $env:CODEX_INFO_WINDOWS_PREVIEW
$oldPreviewSize = $env:CODEX_INFO_WINDOWS_PREVIEW_SIZE
$oldPreviewPoints = $env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS
$oldPreviewDelay = $env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS
try {
    Assert-GraphLatency (Test-Path -LiteralPath $resolvedClientPath -PathType Leaf) "Installed client not found: $resolvedClientPath"
    foreach ($points in @(10080, 44640)) {
        $case = Invoke-GraphPointCase -ResolvedClientPath $resolvedClientPath -Points $points
        $script:graphLatencyReport['cases'] += $case
        $script:graphLatencyReport.Remove('active_case') | Out-Null
        $script:graphLatencyActiveCase = $null
        Save-GraphLatencyReport
    }
    $allToggleSamples = [System.Collections.Generic.List[object]]::new()
    $allMenuSamples = [System.Collections.Generic.List[object]]::new()
    $allMetricButtonSamples = [System.Collections.Generic.List[object]]::new()
    foreach ($case in $script:graphLatencyReport['cases']) {
        foreach ($control in $case.controls) {
            foreach ($sample in $control.samples) {
                if ($control.kind -eq 'toggle') {
                    $allToggleSamples.Add($sample)
                }
                elseif ($control.kind -eq 'metric-button-switch') {
                    $allMetricButtonSamples.Add($sample)
                }
                else {
                    $allMenuSamples.Add($sample)
                }
            }
        }
    }
    $globalToggleStats = Get-GraphLatencyStats -Samples @($allToggleSamples) -Description 'all points toggle samples'
    $globalMenuStats = Get-GraphLatencyStats -Samples @($allMenuSamples) -Description 'all points menu samples'
    $globalMetricButtonStats = Get-GraphLatencyStats -Samples @($allMetricButtonSamples) -Description 'all points metric button samples'
    $globalToggleColdMax = (@($script:graphLatencyReport['cases'] | ForEach-Object {
        $_.controls | Where-Object { $_.kind -eq 'toggle' } | ForEach-Object { [double]$_.cold_ms }
    } | Measure-Object -Maximum).Maximum)
    $globalMenuColdMax = (@($script:graphLatencyReport['cases'] | ForEach-Object {
        $_.controls | Where-Object { $_.kind -eq 'menu' } | ForEach-Object { [double]$_.cold_ms }
    } | Measure-Object -Maximum).Maximum)
    $globalMetricButtonColdMax = (@($script:graphLatencyReport['cases'] | ForEach-Object {
        $_.controls | Where-Object { $_.kind -eq 'metric-button-switch' } | ForEach-Object { [double]$_.cold_ms }
    } | Measure-Object -Maximum).Maximum)
    Assert-GraphLatencyBudget -Stats $globalToggleStats -P90Limit 75 -P95Limit 100 `
        -ColdLimit 250 -ColdMilliseconds $globalToggleColdMax -Description 'all points toggles'
    Assert-GraphLatencyBudget -Stats $globalMenuStats -P90Limit 100 -P95Limit 150 `
        -ColdLimit 250 -ColdMilliseconds $globalMenuColdMax -Description 'all points menus'
    Assert-GraphLatencyBudget -Stats $globalMetricButtonStats -P90Limit 100 -P95Limit 150 `
        -ColdLimit 250 -ColdMilliseconds $globalMetricButtonColdMax -Description 'all points metric button switches'
    $script:graphLatencyReport['aggregate'] = [ordered]@{
        toggles = $globalToggleStats
        menus = $globalMenuStats
        metric_button_switches = $globalMetricButtonStats
    }
    $script:graphLatencyReport['cold_interaction_max_ms'] = [ordered]@{
        toggles = $globalToggleColdMax
        menus = $globalMenuColdMax
        metric_button_switches = $globalMetricButtonColdMax
    }
    $script:graphLatencyReport['status'] = 'PASS'
    $script:graphLatencyReport['completed_at'] = [DateTimeOffset]::Now.ToString('o')
    Save-GraphLatencyReport
    Write-GraphLatencyLog "windows-graph-latency: PASS points=10080,44640 iterations=$Iterations report=$script:graphLatencyReportPath"
}
catch {
    $script:graphLatencyReport['status'] = 'FAIL'
    $script:graphLatencyReport['failure'] = $_.Exception.ToString()
    $script:graphLatencyReport['completed_at'] = [DateTimeOffset]::Now.ToString('o')
    try { Save-GraphLatencyReport } catch { }
    Write-GraphLatencyLog "windows-graph-latency: FAIL $($_.Exception.Message) report=$script:graphLatencyReportPath"
    throw
}
finally {
    if ($null -eq $oldPreview) { Remove-Item Env:CODEX_INFO_WINDOWS_PREVIEW -ErrorAction SilentlyContinue }
    else { $env:CODEX_INFO_WINDOWS_PREVIEW = $oldPreview }
    if ($null -eq $oldPreviewSize) { Remove-Item Env:CODEX_INFO_WINDOWS_PREVIEW_SIZE -ErrorAction SilentlyContinue }
    else { $env:CODEX_INFO_WINDOWS_PREVIEW_SIZE = $oldPreviewSize }
    if ($null -eq $oldPreviewPoints) { Remove-Item Env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS -ErrorAction SilentlyContinue }
    else { $env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS = $oldPreviewPoints }
    if ($null -eq $oldPreviewDelay) { Remove-Item Env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS -ErrorAction SilentlyContinue }
    else { $env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS = $oldPreviewDelay }
}
