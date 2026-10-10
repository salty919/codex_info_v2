# Captures one fresh installed Windows client window for acceptance evidence.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$OutputPath,
    [string]$ThreadsOutputPath = '',
    [string]$MainScrolledOutputPath = '',
    [string]$Preview = 'setup',
    [string]$PreviewSize = '760x680',
    [string]$ClientPath = '',
    [int]$GraphPoints = 3,
    [ValidateRange(0, 5000)][int]$GraphBuildDelayMilliseconds = 0,
    [ValidateRange(0, 7)][int]$ThreadCount = 6,
    [switch]$ConfiguredService,
    [switch]$OpenGraphPeriodMenu,
    [ValidateSet('Tokens', 'Dollars')][string]$GraphMetric
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CodexInfoCaptureWin32 {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int command);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr extra);
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr extra);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
'@
[CodexInfoCaptureWin32]::SetProcessDPIAware() | Out-Null

if ($ConfiguredService) {
    if ($PSBoundParameters.ContainsKey('Preview') -or $OpenGraphPeriodMenu -or
        $PSBoundParameters.ContainsKey('GraphMetric') -or
        -not [string]::IsNullOrWhiteSpace($MainScrolledOutputPath)) {
        throw 'ConfiguredService captures only the live Main window without preview or Graph selector options'
    }
    # The launched client must use its persisted service settings and real
    # account state, even when this shell previously ran a preview capture.
    foreach ($previewVariable in @(
        'CODEX_INFO_WINDOWS_PREVIEW',
        'CODEX_INFO_WINDOWS_PREVIEW_SIZE',
        'CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS',
        'CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS',
        'CODEX_INFO_WINDOWS_PREVIEW_THREAD_COUNT')) {
        Remove-Item "Env:$previewVariable" -ErrorAction SilentlyContinue
    }
    $expectedTitle = 'Codex Info Monitor'
} else {
    if (-not [string]::IsNullOrWhiteSpace($ThreadsOutputPath)) {
        throw 'ThreadsOutputPath requires ConfiguredService'
    }
    if (-not [string]::IsNullOrWhiteSpace($MainScrolledOutputPath)) {
        if ($Preview -ne 'model-breakdown') {
            throw 'MainScrolledOutputPath requires the model-breakdown preview'
        }
        if ([string]::Equals(
                [System.IO.Path]::GetFullPath($OutputPath),
                [System.IO.Path]::GetFullPath($MainScrolledOutputPath),
                [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'MainScrolledOutputPath must differ from OutputPath'
        }
    }
    $env:CODEX_INFO_WINDOWS_PREVIEW = $Preview
    $env:CODEX_INFO_WINDOWS_PREVIEW_SIZE = $PreviewSize
    $env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS = $GraphPoints
    $env:CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS = $GraphBuildDelayMilliseconds
    $env:CODEX_INFO_WINDOWS_PREVIEW_THREAD_COUNT = $ThreadCount
    $expectedTitle = switch ($Preview) {
        { $_ -in @('normal', 'auth', 'error', 'warning', 'danger', 'zero', 'full', 'update', 'model-breakdown') } { 'Codex Info Monitor' }
        'graph' { 'Codex Info Graph' }
        { $_ -in @('threads', 'threads-tree', 'threads-branches') } { 'Codex Info Threads' }
        'legal' { 'Codex Info License' }
        'settings' { 'Codex Info Settings' }
        default { 'Codex Info Setup' }
    }
}
$script:codexInfoCaptureTitle = $expectedTitle
$exe = if ([string]::IsNullOrWhiteSpace($ClientPath)) {
    Join-Path $env:LOCALAPPDATA 'Programs\Codex Info Monitor\CodexInfo.WindowsClient.exe'
} else {
    $ClientPath
}
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Installed client not found: $exe" }
$process = Start-Process -FilePath $exe -PassThru
try {
    $window = [IntPtr]::Zero
    for ($i = 0; $i -lt 80 -and $window -eq [IntPtr]::Zero; $i++) {
        # Allow the DirectComposition surface to present a complete stable
        # frame before GDI screen capture; partial compositor frames are not
        # valid visual evidence.
        Start-Sleep -Milliseconds 750
        $process.Refresh()
        $callback = [CodexInfoCaptureWin32+EnumWindowsProc] {
            param([IntPtr]$handle, [IntPtr]$extra)
            [uint32]$owner = 0
            [CodexInfoCaptureWin32]::GetWindowThreadProcessId($handle, [ref]$owner) | Out-Null
            if ($owner -ne [uint32]$process.Id -or -not [CodexInfoCaptureWin32]::IsWindowVisible($handle)) { return $true }
            $title = New-Object System.Text.StringBuilder 256
            [CodexInfoCaptureWin32]::GetWindowText($handle, $title, $title.Capacity) | Out-Null
            if ($title.ToString() -like ("*{0}*" -f $script:codexInfoCaptureTitle)) { $script:codexInfoCaptureWindow = $handle; return $false }
            return $true
        }
        $script:codexInfoCaptureWindow = [IntPtr]::Zero
        [CodexInfoCaptureWin32]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
        $window = $script:codexInfoCaptureWindow
    }
    if ($window -eq [IntPtr]::Zero) { throw "Fresh $expectedTitle window did not open" }
    [CodexInfoCaptureWin32]::ShowWindow($window, 9) | Out-Null
    # Acceptance capture must not silently photograph an unrelated window if
    # foreground activation is denied. Keep only this fresh test HWND topmost
    # for its short capture lifetime; the process is terminated in `finally`.
    [CodexInfoCaptureWin32]::SetWindowPos($window, [IntPtr](-1), 80, 80, 0, 0, 0x0001) | Out-Null
    [CodexInfoCaptureWin32]::BringWindowToTop($window) | Out-Null
    [CodexInfoCaptureWin32]::SetForegroundWindow($window) | Out-Null
    Start-Sleep -Milliseconds 500
    if ($ConfiguredService) {
        # Read only the accepted Main generation on this process's HWND.
        # A Setup window, preview, pending generation, or failure is not live
        # authenticated Main evidence.
        $automationRoot = [System.Windows.Automation.AutomationElement]::FromHandle($window)
        $condition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
            'Main.DetailsGenerationContract')
        $authenticatedContentCondition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
            'Main.QuotaObservedAt')
        $generationState = 'missing'
        $authenticatedContentVisible = $false
        for ($readyAttempt = 0; $readyAttempt -lt 40; $readyAttempt++) {
            $generation = $automationRoot.FindFirst(
                [System.Windows.Automation.TreeScope]::Descendants,
                $condition)
            if ($null -ne $generation) {
                $generationState = $generation.Current.Name
            }
            $observedAt = $automationRoot.FindFirst(
                [System.Windows.Automation.TreeScope]::Descendants,
                $authenticatedContentCondition)
            $authenticatedContentVisible = $null -ne $observedAt -and -not $observedAt.Current.IsOffscreen
            if ($generationState -eq 'ready' -and $authenticatedContentVisible) {
                break
            }
            Start-Sleep -Milliseconds 500
        }
        if ($generationState -ne 'ready' -or -not $authenticatedContentVisible) {
            throw "Configured authenticated Main is not ready: generation=$generationState authenticatedContentVisible=$authenticatedContentVisible"
        }
    }
    $graphMetricBound = $PSBoundParameters.ContainsKey('GraphMetric')
    if ($OpenGraphPeriodMenu -or $graphMetricBound) {
        # Locate semantic controls instead of scaling stale pixel coordinates.
        # This remains correct across DPI and responsive widths.
        $automationRoot = [System.Windows.Automation.AutomationElement]::FromHandle($window)
        if ($graphMetricBound) {
            $metricAutomationId = 'Graph.Metric.Switch'
            $expectedState = if ($GraphMetric -eq 'Tokens') { 'True' } else { 'False' }
            $metricCondition = New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
                $metricAutomationId)
            $metricSwitch = $automationRoot.FindFirst(
                [System.Windows.Automation.TreeScope]::Descendants,
                $metricCondition)
            if ($null -eq $metricSwitch) { throw "Graph metric switch is missing: $metricAutomationId" }
            if (-not $metricSwitch.Current.IsEnabled -or $metricSwitch.Current.IsOffscreen) {
                throw "Graph metric switch is not available: $metricAutomationId"
            }
            $priorSelectedState = $metricSwitch.Current.HelpText
            if ($priorSelectedState -notin @('True', 'False')) {
                throw "Graph metric switch has an unknown selected state: $metricAutomationId"
            }
            $wasSelected = $priorSelectedState -ceq $expectedState
            $plotAutomationId = 'Graph.Plot'
            $plotCondition = New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
                $plotAutomationId)
            $plot = $automationRoot.FindFirst(
                [System.Windows.Automation.TreeScope]::Descendants,
                $plotCondition)
            $priorPlotHelpText = if ($null -eq $plot) { $null } else { $plot.Current.HelpText }
            $toggle = $null
            if (-not $metricSwitch.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$toggle)) {
                throw "Graph metric switch has no TogglePattern: $metricAutomationId"
            }
            if (-not $wasSelected) { $toggle.Toggle() }
            $selected = $false
            for ($attempt = 0; $attempt -lt 40; $attempt++) {
                $metricSwitch = $automationRoot.FindFirst(
                    [System.Windows.Automation.TreeScope]::Descendants,
                    $metricCondition)
                if ($null -ne $metricSwitch -and $metricSwitch.Current.HelpText -ceq $expectedState) {
                    $selected = $true
                    break
                }
                Start-Sleep -Milliseconds 50
            }
            if (-not $selected) { throw "Graph metric switch did not become selected: $metricAutomationId" }
            # The switch state can update before the asynchronous graph scene.
            # Always wait for a visible plot and a hidden loading indicator; when
            # switching metrics, also require the plot's metric-axis UIA value to
            # change before capturing the accepted frame.
            $sceneReady = $false
            $sceneWaitDeadline = [DateTime]::UtcNow.AddSeconds(10)
            $progressCondition = New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                [System.Windows.Automation.ControlType]::ProgressBar)
            while ([DateTime]::UtcNow -lt $sceneWaitDeadline) {
                $plot = $automationRoot.FindFirst(
                    [System.Windows.Automation.TreeScope]::Descendants,
                    $plotCondition)
                $plotVisible = $false
                $plotHelpTextChanged = $wasSelected
                if ($null -ne $plot) {
                    $plotState = $plot.Current
                    $plotBounds = $plotState.BoundingRectangle
                    $plotVisible = -not $plotState.IsOffscreen -and
                        $plotBounds.Width -gt 0 -and $plotBounds.Height -gt 0
                    $plotHelpTextChanged = $wasSelected -or
                        $plotState.HelpText -cne $priorPlotHelpText
                }
                if ($plotVisible -and $plotHelpTextChanged) {
                    $progressBars = $automationRoot.FindAll(
                        [System.Windows.Automation.TreeScope]::Descendants,
                        $progressCondition)
                    $visibleProgressBar = $false
                    foreach ($progressBar in $progressBars) {
                        $progressBounds = $progressBar.Current.BoundingRectangle
                        if (-not $progressBar.Current.IsOffscreen -and
                            $progressBounds.Width -gt 0 -and $progressBounds.Height -gt 0) {
                            $visibleProgressBar = $true
                            break
                        }
                    }
                    if (-not $visibleProgressBar) {
                        $sceneReady = $true
                        break
                    }
                }
                Start-Sleep -Milliseconds 50
            }
            if (-not $sceneReady) {
                $failureRect = New-Object CodexInfoCaptureWin32+RECT
                [CodexInfoCaptureWin32]::GetWindowRect($window, [ref]$failureRect) | Out-Null
                $failureBitmap = New-Object System.Drawing.Bitmap(($failureRect.Right - $failureRect.Left), ($failureRect.Bottom - $failureRect.Top))
                $failureGraphics = [System.Drawing.Graphics]::FromImage($failureBitmap)
                try {
                    $failureGraphics.CopyFromScreen($failureRect.Left, $failureRect.Top, 0, 0, $failureBitmap.Size)
                    $failureBitmap.Save("$OutputPath.failure.png", [System.Drawing.Imaging.ImageFormat]::Png)
                }
                finally { $failureGraphics.Dispose(); $failureBitmap.Dispose() }
                $currentPlotHelp = if ($null -eq $plot) { '<missing>' } else { $plot.Current.HelpText }
                throw "Graph metric scene did not settle after selecting $GraphMetric within 10 seconds: plotVisible=$plotVisible wasSelected=$wasSelected helpChanged=$plotHelpTextChanged progressVisible=$visibleProgressBar priorHelp='$priorPlotHelpText' currentHelp='$currentPlotHelp'; diagnostic=$OutputPath.failure.png"
            }
        }
        if ($OpenGraphPeriodMenu) {
            $periodAutomationId = 'Graph.PeriodSelector'
            $periodCondition = New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
                $periodAutomationId)
            $periodSelector = $automationRoot.FindFirst(
                [System.Windows.Automation.TreeScope]::Descendants,
                $periodCondition)
            if ($null -eq $periodSelector) { throw "Graph selector is missing: $periodAutomationId" }
            $toggle = $null
            if (-not $periodSelector.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$toggle)) {
                throw "Graph selector has no TogglePattern: $periodAutomationId"
            }
            $toggle.Toggle()
        }
        [CodexInfoCaptureWin32]::SetWindowPos($window, [IntPtr](-1), 80, 80, 0, 0, 0x0001) | Out-Null
        Start-Sleep -Milliseconds 250
    } else {
        [CodexInfoCaptureWin32]::SetCursorPos(10, 10) | Out-Null
    }
    $rect = New-Object CodexInfoCaptureWin32+RECT
    [CodexInfoCaptureWin32]::GetWindowRect($window, [ref]$rect) | Out-Null
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    if ($width -le 0 -or $height -le 0) { throw "Invalid window bounds: ${width}x${height}" }
    $bitmap = New-Object System.Drawing.Bitmap($width, $height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
    $bitmap.Save($OutputPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    $bitmap.Dispose()
    $captureMode = if ($ConfiguredService) { 'configured-service' } else { "preview:$Preview" }
    Write-Output "capture: PASS mode=$captureMode pid=$($process.Id) hwnd=$window size=${width}x${height} path=$OutputPath"
    if (-not [string]::IsNullOrWhiteSpace($MainScrolledOutputPath)) {
        $automationRoot = [System.Windows.Automation.AutomationElement]::FromHandle($window)
        $scrollCondition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
            'Main.ModelUsageScroll')
        $modelScrollViewer = $automationRoot.FindFirst(
            [System.Windows.Automation.TreeScope]::Descendants,
            $scrollCondition)
        if ($null -eq $modelScrollViewer) { throw 'Main model usage scroll viewer is missing' }
        $scrollPattern = $null
        if (-not $modelScrollViewer.TryGetCurrentPattern(
                [System.Windows.Automation.ScrollPattern]::Pattern,
                [ref]$scrollPattern)) {
            throw 'Main model usage scroll viewer has no ScrollPattern'
        }
        if (-not $scrollPattern.Current.VerticallyScrollable) {
            throw 'Model breakdown preview did not overflow the six-row viewport'
        }
        $logicalScale = $height / 542.0
        $viewportHeight = $modelScrollViewer.Current.BoundingRectangle.Height / $logicalScale
        if ([Math]::Abs($viewportHeight - 132) -gt 1) {
            throw "Main model viewport is not six 22px rows: height=$viewportHeight"
        }
        if ([Math]::Abs($scrollPattern.Current.VerticalViewSize - 75) -gt 0.5) {
            throw 'Main model viewport does not expose six of eight rows'
        }
        if ($scrollPattern.Current.VerticalScrollPercent -gt 0.1) {
            throw 'Main model viewport did not start at the first row'
        }
        $scrollPattern.SetScrollPercent(
            [System.Windows.Automation.ScrollPattern]::NoScroll, 100)
        $atBottom = $false
        for ($scrollAttempt = 0; $scrollAttempt -lt 20; $scrollAttempt++) {
            Start-Sleep -Milliseconds 100
            if ($scrollPattern.Current.VerticalScrollPercent -ge 99.9) {
                $atBottom = $true
                break
            }
        }
        if (-not $atBottom) { throw 'Main model rows did not reach the end of the scroll viewport' }
        [CodexInfoCaptureWin32]::SetForegroundWindow($window) | Out-Null
        Start-Sleep -Milliseconds 250
        $scrolledBitmap = New-Object System.Drawing.Bitmap($width, $height)
        $scrolledGraphics = [System.Drawing.Graphics]::FromImage($scrolledBitmap)
        try {
            $scrolledGraphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $scrolledBitmap.Size)
            $scrolledBitmap.Save($MainScrolledOutputPath, [System.Drawing.Imaging.ImageFormat]::Png)
        }
        finally {
            $scrolledGraphics.Dispose()
            $scrolledBitmap.Dispose()
        }
        Write-Output "capture: PASS mode=preview:$($Preview):scrolled pid=$($process.Id) hwnd=$window size=${width}x${height} path=$MainScrolledOutputPath"
    }
    if (-not [string]::IsNullOrWhiteSpace($ThreadsOutputPath)) {
        $automationRoot = [System.Windows.Automation.AutomationElement]::FromHandle($window)
        $openCondition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
            'Main.OpenThreadDetails')
        $openButton = $automationRoot.FindFirst(
            [System.Windows.Automation.TreeScope]::Descendants,
            $openCondition)
        if ($null -eq $openButton -or $openButton.Current.IsOffscreen) {
            throw 'Accepted Main has no visible Threads detail action'
        }
        $invoke = $null
        if (-not $openButton.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$invoke)) {
            throw 'Threads detail action has no InvokePattern'
        }
        $invoke.Invoke()
        $script:codexInfoCaptureTitle = 'Codex Info Threads'
        $threadsWindow = [IntPtr]::Zero
        for ($attempt = 0; $attempt -lt 30 -and $threadsWindow -eq [IntPtr]::Zero; $attempt++) {
            Start-Sleep -Milliseconds 500
            $script:codexInfoCaptureWindow = [IntPtr]::Zero
            [CodexInfoCaptureWin32]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
            $threadsWindow = $script:codexInfoCaptureWindow
        }
        if ($threadsWindow -eq [IntPtr]::Zero) { throw 'Fresh Threads window did not open from accepted Main' }
        [CodexInfoCaptureWin32]::ShowWindow($threadsWindow, 9) | Out-Null
        [CodexInfoCaptureWin32]::SetWindowPos($threadsWindow, [IntPtr](-1), 80, 80, 0, 0, 0x0001) | Out-Null
        [CodexInfoCaptureWin32]::BringWindowToTop($threadsWindow) | Out-Null
        [CodexInfoCaptureWin32]::SetForegroundWindow($threadsWindow) | Out-Null
        Start-Sleep -Milliseconds 500
        $threadRect = New-Object CodexInfoCaptureWin32+RECT
        [CodexInfoCaptureWin32]::GetWindowRect($threadsWindow, [ref]$threadRect) | Out-Null
        $threadWidth = $threadRect.Right - $threadRect.Left
        $threadHeight = $threadRect.Bottom - $threadRect.Top
        if ($threadWidth -le 0 -or $threadHeight -le 0) {
            throw "Invalid Threads window bounds: ${threadWidth}x${threadHeight}"
        }
        $threadBitmap = New-Object System.Drawing.Bitmap($threadWidth, $threadHeight)
        $threadGraphics = [System.Drawing.Graphics]::FromImage($threadBitmap)
        try {
            $threadGraphics.CopyFromScreen($threadRect.Left, $threadRect.Top, 0, 0, $threadBitmap.Size)
            $threadBitmap.Save($ThreadsOutputPath, [System.Drawing.Imaging.ImageFormat]::Png)
        }
        finally {
            $threadGraphics.Dispose()
            $threadBitmap.Dispose()
        }
        Write-Output "capture: PASS mode=configured-service:threads pid=$($process.Id) hwnd=$threadsWindow size=${threadWidth}x${threadHeight} path=$ThreadsOutputPath"
    }
}
finally {
    if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
}
