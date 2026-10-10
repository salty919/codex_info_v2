# Verifies that every borderless user-facing Windows surface accepts a real
# left-button drag on its title area and changes its screen position.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ClientPath,
    [switch]$AllowPhysicalInput
)

$ErrorActionPreference = 'Stop'
if (-not $AllowPhysicalInput) {
    Write-Output 'window-move-smoke: SKIP (physical cursor input is opt-in; rerun with -AllowPhysicalInput)'
    exit 0
}
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CodexInfoMoveSmokeWin32 {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr extra);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int command);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr hWnd, uint flags);
    [DllImport("user32.dll")] public static extern bool GetMonitorInfo(IntPtr monitor, ref MONITORINFO info);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr extra);
}
'@

[CodexInfoMoveSmokeWin32]::SetProcessDPIAware() | Out-Null
$virtualLeft = [CodexInfoMoveSmokeWin32]::GetSystemMetrics(76)
$virtualTop = [CodexInfoMoveSmokeWin32]::GetSystemMetrics(77)
$virtualWidth = [CodexInfoMoveSmokeWin32]::GetSystemMetrics(78)
$virtualHeight = [CodexInfoMoveSmokeWin32]::GetSystemMetrics(79)
Write-Output "virtual-desktop: PASS origin=$virtualLeft,$virtualTop size=${virtualWidth}x${virtualHeight}"

if (-not (Test-Path -LiteralPath $ClientPath -PathType Leaf)) { throw "Client not found: $ClientPath" }

$cases = @(
    @{ Preview = 'normal'; Title = 'Codex Info Monitor'; CrossMonitor = $false },
    @{ Preview = 'setup'; Title = 'Codex Info Setup' },
    @{ Preview = 'graph'; Title = 'Codex Info Graph' },
    @{ Preview = 'threads'; Title = 'Codex Info Threads' },
    @{ Preview = 'legal'; Title = 'Codex Info License' },
    @{ Preview = 'settings'; Title = 'Codex Info Settings' },
    # Keep the long cross-monitor movement in its own fresh process so its
    # deliberately off-screen traversal cannot contaminate the ordinary
    # six-window close checks.
    @{ Preview = 'normal'; Title = 'Codex Info Monitor'; CrossMonitor = $true }
)
$leftDown = 0x0002
$leftUp = 0x0004

foreach ($case in $cases) {
    $env:CODEX_INFO_WINDOWS_PREVIEW = $case.Preview
    $env:CODEX_INFO_WINDOWS_PREVIEW_SIZE = '760x680'
    $process = Start-Process -FilePath $ClientPath -PassThru
    try {
        $window = [IntPtr]::Zero
        for ($i = 0; $i -lt 80 -and $window -eq [IntPtr]::Zero; $i++) {
            Start-Sleep -Milliseconds 250
            $callback = [CodexInfoMoveSmokeWin32+EnumWindowsProc] {
                param([IntPtr]$handle, [IntPtr]$extra)
                [uint32]$owner = 0
                [CodexInfoMoveSmokeWin32]::GetWindowThreadProcessId($handle, [ref]$owner) | Out-Null
                if ($owner -ne [uint32]$process.Id -or -not [CodexInfoMoveSmokeWin32]::IsWindowVisible($handle)) { return $true }
                $title = New-Object System.Text.StringBuilder 256
                [CodexInfoMoveSmokeWin32]::GetWindowText($handle, $title, $title.Capacity) | Out-Null
                if ($title.ToString() -like "*$($case.Title)*") { $script:codexInfoMoveWindow = $handle; return $false }
                return $true
            }
            $script:codexInfoMoveWindow = [IntPtr]::Zero
            [CodexInfoMoveSmokeWin32]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
            $window = $script:codexInfoMoveWindow
        }
        if ($window -eq [IntPtr]::Zero) { throw "Fresh window not found: $($case.Preview) / $($case.Title)" }
        [CodexInfoMoveSmokeWin32]::ShowWindow($window, 9) | Out-Null
        [CodexInfoMoveSmokeWin32]::BringWindowToTop($window) | Out-Null
        [CodexInfoMoveSmokeWin32]::SetForegroundWindow($window) | Out-Null
        Start-Sleep -Milliseconds 400
        $before = New-Object CodexInfoMoveSmokeWin32+RECT
        [CodexInfoMoveSmokeWin32]::GetWindowRect($window, [ref]$before) | Out-Null
        $monitor = [CodexInfoMoveSmokeWin32]::MonitorFromWindow($window, 2)
        $monitorInfo = New-Object CodexInfoMoveSmokeWin32+MONITORINFO
        $monitorInfo.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($monitorInfo)
        if ($monitor -eq [IntPtr]::Zero -or -not [CodexInfoMoveSmokeWin32]::GetMonitorInfo($monitor, [ref]$monitorInfo)) {
            throw "Monitor information unavailable: $($case.Preview) / $($case.Title)"
        }
        $windowCenterX = [Math]::Round(($before.Left + $before.Right) / 2.0)
        $windowCenterY = [Math]::Round(($before.Top + $before.Bottom) / 2.0)
        $workCenterX = [Math]::Round(($monitorInfo.rcWork.Left + $monitorInfo.rcWork.Right) / 2.0)
        $workCenterY = [Math]::Round(($monitorInfo.rcWork.Top + $monitorInfo.rcWork.Bottom) / 2.0)
        if ([Math]::Abs($windowCenterX - $workCenterX) -gt 1 -or [Math]::Abs($windowCenterY - $workCenterY) -gt 1) {
            throw "Window centering mismatch: $($case.Preview) actual=$windowCenterX,$windowCenterY expected=$workCenterX,$workCenterY"
        }
        Write-Output "window-center: PASS $($case.Preview) center=$windowCenterX,$windowCenterY"
        # Try several points in the left title band.  The center may be a
        # command button, and DPI rounding can put one logical row on its
        # border; every candidate remains inside the non-control title area.
        $after = New-Object CodexInfoMoveSmokeWin32+RECT
        $moved = $false
        foreach ($offset in @(@(80, 36), @(100, 54), @(180, 54), @(260, 36))) {
            $x = $before.Left + $offset[0]
            $y = $before.Top + $offset[1]
            $candidateTrace = @()
            [CodexInfoMoveSmokeWin32]::SetWindowPos($window, [IntPtr]::Zero, $before.Left, $before.Top, 0, 0, 0x0001 -bor 0x0010) | Out-Null
            [CodexInfoMoveSmokeWin32]::ShowWindow($window, 9) | Out-Null
            [CodexInfoMoveSmokeWin32]::BringWindowToTop($window) | Out-Null
            [CodexInfoMoveSmokeWin32]::SetForegroundWindow($window) | Out-Null
            Start-Sleep -Milliseconds 400
            [CodexInfoMoveSmokeWin32]::SetCursorPos($x, $y) | Out-Null
            Start-Sleep -Milliseconds 300
            [CodexInfoMoveSmokeWin32]::mouse_event($leftDown, 0, 0, 0, [UIntPtr]::Zero)
            Start-Sleep -Milliseconds 300
            foreach ($step in 1..8) {
                [CodexInfoMoveSmokeWin32]::SetCursorPos($x + (15 * $step), $y + (10 * $step)) | Out-Null
                Start-Sleep -Milliseconds 220
                $sample = New-Object CodexInfoMoveSmokeWin32+RECT
                [CodexInfoMoveSmokeWin32]::GetWindowRect($window, [ref]$sample) | Out-Null
                $candidateTrace += $sample
            }
            [CodexInfoMoveSmokeWin32]::mouse_event($leftUp, 0, 0, 0, [UIntPtr]::Zero)
            Start-Sleep -Milliseconds 180
            [CodexInfoMoveSmokeWin32]::GetWindowRect($window, [ref]$after) | Out-Null
            if ($after.Left -ne $before.Left -or $after.Top -ne $before.Top) {
                $previous = $before
                foreach ($sample in $candidateTrace) {
                    if ($sample.Left -lt $previous.Left -or $sample.Top -lt $previous.Top) {
                        $traceText = ($candidateTrace | ForEach-Object { "($($_.Left),$($_.Top))" }) -join ' '
                        throw "Window drag jitter detected: $($case.Preview) / $($case.Title) trace=$traceText"
                    }
                    $previous = $sample
                }
                $moved = $true
                break
            }
        }
        if (-not $moved) { throw "Window did not move: $($case.Preview) / $($case.Title)" }
        Write-Output "window-move: PASS $($case.Preview) before=$($before.Left),$($before.Top) after=$($after.Left),$($after.Top)"

        if ($case.CrossMonitor -and $virtualWidth -gt 3000) {
            # This host exposes a multi-monitor virtual desktop.  Exercise a
            # long native drag so the pointer crosses a monitor boundary (the
            # close check below still returns the window to its centered rect).
            [CodexInfoMoveSmokeWin32]::SetWindowPos($window, [IntPtr]::Zero, $before.Left, $before.Top, 0, 0, 0x0001 -bor 0x0010) | Out-Null
            Start-Sleep -Milliseconds 250
            $crossX = $before.Left + 80
            $crossY = $before.Top + 36
            [CodexInfoMoveSmokeWin32]::SetCursorPos($crossX, $crossY) | Out-Null
            Start-Sleep -Milliseconds 300
            [CodexInfoMoveSmokeWin32]::mouse_event($leftDown, 0, 0, 0, [UIntPtr]::Zero)
            Start-Sleep -Milliseconds 300
            foreach ($step in 1..8) {
                [CodexInfoMoveSmokeWin32]::SetCursorPos($crossX + (275 * $step), $crossY + (8 * $step)) | Out-Null
                Start-Sleep -Milliseconds 220
            }
            [CodexInfoMoveSmokeWin32]::mouse_event($leftUp, 0, 0, 0, [UIntPtr]::Zero)
            Start-Sleep -Milliseconds 250
            $crossAfter = New-Object CodexInfoMoveSmokeWin32+RECT
            [CodexInfoMoveSmokeWin32]::GetWindowRect($window, [ref]$crossAfter) | Out-Null
            if ($crossAfter.Left -lt ($before.Left + 1500)) {
                throw "Cross-monitor drag did not travel across the virtual desktop: before=$($before.Left) after=$($crossAfter.Left)"
            }
            Write-Output "window-cross-monitor: PASS normal before=$($before.Left),$($before.Top) after=$($crossAfter.Left),$($crossAfter.Top)"
            continue
        }

        # Verify the title-bar close control remains clickable after the drag
        # handler is installed.  DPI-aware coordinates are required here.
        $closed = $false
        # The drag assertion above deliberately moves right/down.  On a host
        # whose virtual desktop ends immediately after the test window, the
        # right-side close glyph can therefore be outside the reachable cursor
        # range even though the window itself moved correctly.  Return only
        # the test window to its known in-bounds starting rectangle before
        # exercising the close button; this does not weaken the move assertion.
        [CodexInfoMoveSmokeWin32]::SetWindowPos($window, [IntPtr]::Zero, $before.Left, $before.Top, 0, 0, 0x0001 -bor 0x0010) | Out-Null
        Start-Sleep -Milliseconds 250
        [CodexInfoMoveSmokeWin32]::GetWindowRect($window, [ref]$after) | Out-Null
        [CodexInfoMoveSmokeWin32]::ShowWindow($window, 9) | Out-Null
        [CodexInfoMoveSmokeWin32]::BringWindowToTop($window) | Out-Null
        [CodexInfoMoveSmokeWin32]::SetForegroundWindow($window) | Out-Null
        Start-Sleep -Milliseconds 350
        foreach ($xInset in @(30, 60, 90, 120)) {
            foreach ($closeOffset in @(20, 30, 40, 50, 60, 70, 80)) {
                $closeX = $after.Right - $xInset
                $closeY = $after.Top + $closeOffset
                [CodexInfoMoveSmokeWin32]::SetCursorPos($closeX, $closeY) | Out-Null
                [CodexInfoMoveSmokeWin32]::mouse_event($leftDown, 0, 0, 0, [UIntPtr]::Zero)
                Start-Sleep -Milliseconds 80
                [CodexInfoMoveSmokeWin32]::mouse_event($leftUp, 0, 0, 0, [UIntPtr]::Zero)
                Start-Sleep -Milliseconds 180
                if (-not [CodexInfoMoveSmokeWin32]::IsWindowVisible($window)) { $closed = $true; break }
            }
            if ($closed) { break }
        }
        if (-not $closed) { throw "Window close control did not close: $($case.Preview) / $($case.Title)" }
        Write-Output "window-close: PASS $($case.Preview)"

    }
    finally {
        if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
    }
}

Write-Output 'window-move-smoke: PASS'
