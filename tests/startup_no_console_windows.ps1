param(
    [string] $Executable = (Join-Path $PSScriptRoot '..\target\release\git-helper.exe'),
    [int] $ObservationMilliseconds = 5000
)

$ErrorActionPreference = 'Stop'

Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class StartupWindowProbe
{
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern int GetClassName(IntPtr hWnd, StringBuilder className, int maxCount);

    [DllImport("user32.dll")]
    private static extern int GetWindowText(IntPtr hWnd, StringBuilder title, int maxCount);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    public static string[] VisibleWindows()
    {
        var windows = new List<string>();
        EnumWindows((window, _) =>
        {
            if (!IsWindowVisible(window))
                return true;

            uint processId;
            GetWindowThreadProcessId(window, out processId);

            var className = new StringBuilder(256);
            GetClassName(window, className, className.Capacity);

            var title = new StringBuilder(512);
            GetWindowText(window, title, title.Capacity);

            windows.Add(String.Format("{0}|{1}|{2}|{3}", window, processId, className, title));
            return true;
        }, IntPtr.Zero);
        return windows.ToArray();
    }
}
'@

$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
$baselineHandles = [System.Collections.Generic.HashSet[string]]::new()
foreach ($window in [StartupWindowProbe]::VisibleWindows()) {
    [void] $baselineHandles.Add($window.Split('|', 2)[0])
}

$application = Start-Process -FilePath $resolvedExecutable -WorkingDirectory (Split-Path $resolvedExecutable) -PassThru
$consoleClasses = @('ConsoleWindowClass', 'CASCADIA_HOSTING_WINDOW_CLASS')
$observedConsoles = [System.Collections.Generic.HashSet[string]]::new()
$deadline = [DateTime]::UtcNow.AddMilliseconds($ObservationMilliseconds)

try {
    while ([DateTime]::UtcNow -lt $deadline -and -not $application.HasExited) {
        foreach ($window in [StartupWindowProbe]::VisibleWindows()) {
            $parts = $window.Split('|', 4)
            if ($baselineHandles.Contains($parts[0]) -or $parts[1] -eq [string] $application.Id) {
                continue
            }

            $processName = try { (Get-Process -Id ([int] $parts[1]) -ErrorAction Stop).ProcessName } catch { '' }
            if ($consoleClasses -contains $parts[2] -or $processName -in @('conhost', 'OpenConsole', 'WindowsTerminal')) {
                [void] $observedConsoles.Add("process=$processName pid=$($parts[1]) class=$($parts[2]) title=$($parts[3])")
            }
        }
        Start-Sleep -Milliseconds 10
    }
}
finally {
    if (-not $application.HasExited) {
        $application.CloseMainWindow() | Out-Null
        if (-not $application.WaitForExit(1500)) {
            Stop-Process -Id $application.Id -Force
        }
    }
}

if ($observedConsoles.Count -gt 0) {
    Write-Error "Visible console windows appeared during startup:`n$($observedConsoles -join "`n")"
}

Write-Output 'PASS: startup opened no console windows.'
