$root = Split-Path -Parent $PSScriptRoot
# Single-instance motion measurement: 3 phases x 15s of synthetic WM_MOUSEMOVE,
# per-second CPU samples. Progress is printed as it happens.
$ErrorActionPreference = "Continue"

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class W32 {
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function PostMove([IntPtr]$h, [int]$x, [int]$y) {
    $lp = [IntPtr](($y -shl 16) -bor ($x -band 0xFFFF))
    [void][W32]::PostMessageW($h, 0x0200, [IntPtr]::Zero, $lp)
}

$exe = "$root\target\release\monitor-gui.exe"
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $exe
$psi.WorkingDirectory = "$root"
$psi.UseShellExecute = $false
$proc = [System.Diagnostics.Process]::Start($psi)
Write-Output "started pid=$($proc.Id)"
Start-Sleep -Seconds 3
$h = $proc.MainWindowHandle
Write-Output "hwnd=$h"
$cr = New-Object W32+RECT
[void][W32]::GetClientRect($h, [ref]$cr)
$cw = $cr.Right - $cr.Left
$ch = $cr.Bottom - $cr.Top
Write-Output "client ${cw}x${ch}"
if ($cw -le 0 -or $ch -le 0) { Write-Output "BAD RECT"; $proc.Kill(); exit 1 }
$cx = $cw / 2; $cy = $ch / 2
$ax = $cw / 2 - 40; $ay = $ch / 2 - 40

foreach ($phase in 1..3) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $samples = @()
    $c0 = $proc.CPU
    $tLast = 0.0
    while ($sw.Elapsed.TotalSeconds -lt 15) {
        $t = $sw.Elapsed.TotalSeconds
        PostMove $h ([int]($cx + $ax * [math]::Sin($t * 9.1))) ([int]($cy + $ay * [math]::Sin($t * 13.7 + 1.3)))
        if ($t - $tLast -ge 1.0) {
            $samples += [math]::Round(($proc.CPU - $c0) / ($t - $tLast) * 100, 2)
            $c0 = $proc.CPU
            $tLast = $t
        }
        Start-Sleep -Milliseconds 8
    }
    $min = ($samples | Measure-Object -Minimum).Minimum
    $avg = [math]::Round(($samples | Measure-Object -Average).Average, 2)
    Write-Output "PHASE$phase per-sec=[$($samples -join ',')] min=$min avg=$avg"
}
Write-Output "killing"
$proc.Kill()
$proc.WaitForExit()
Write-Output "DONE"
