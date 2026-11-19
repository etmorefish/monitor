$root = Split-Path -Parent $PSScriptRoot
$null = New-Item -ItemType Directory -Force -Path "$root\shots"
# Final acceptance verification for the repaint fix (clean binary, no
# instrumentation): idle CPU, motion CPU (2x12s, per-second samples), and a
# pixel check that the hover readout is drawn.
$ErrorActionPreference = "Continue"

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class W32 {
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
Add-Type -AssemblyName System.Drawing

function PostMove([IntPtr]$h, [int]$x, [int]$y) {
    $lp = [IntPtr](($y -shl 16) -bor ($x -band 0xFFFF))
    [void][W32]::PostMessageW($h, 0x0200, [IntPtr]::Zero, $lp)
}

function Shot([System.Diagnostics.Process]$p, [string]$path) {
    $h = $p.MainWindowHandle
    $r = New-Object W32+RECT
    [void][W32]::GetWindowRect($h, [ref]$r)
    $w = $r.Right - $r.Left
    $ht = $r.Bottom - $r.Top
    $bmp = New-Object System.Drawing.Bitmap $w, $ht
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose()
    $bmp.Dispose()
}

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = "$root\target\release\monitor-gui.exe"
$psi.WorkingDirectory = "$root"
$psi.UseShellExecute = $false
$proc = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds 4
$h = $proc.MainWindowHandle

# 1) idle (nothing posted)
$c0 = $proc.CPU
Start-Sleep -Seconds 5
$idle = [math]::Round(($proc.CPU - $c0) / 5 * 100, 2)
Write-Output "IDLE cpu%=$idle"

$cr = New-Object W32+RECT
[void][W32]::GetClientRect($h, [ref]$cr)
$cw = $cr.Right - $cr.Left
$ch = $cr.Bottom - $cr.Top
$cx = $cw / 2; $cy = $ch / 2
$ax = $cw / 2 - 40; $ay = $ch / 2 - 40

# 2) two motion phases
foreach ($phase in 1..2) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $samples = @()
    $c0 = $proc.CPU
    $tLast = 0.0
    while ($sw.Elapsed.TotalSeconds -lt 12) {
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
    Write-Output "MOTION$phase per-sec=[$($samples -join ',')] min=$min avg=$avg"
}

# 3) hover the CPU chart, then leave
$xH = [int]($cw * 0.24); $yH = [int]($ch * 0.34)
PostMove $h $xH $yH
Start-Sleep -Milliseconds 350
PostMove $h ($xH + 2) $yH
Start-Sleep -Milliseconds 450
Shot $proc "$root\shots\hover.png"
Start-Sleep -Seconds 2
Shot $proc "$root\shots\idle.png"

$proc.Kill()
$proc.WaitForExit()

# 4) pixel check: readout region differs from the no-hover shot, plus dark
# text pixels (title) inside it. +39 px offset for the title bar.
$hb = [System.Drawing.Bitmap]::FromFile("$root\shots\hover.png")
$ib = [System.Drawing.Bitmap]::FromFile("$root\shots\idle.png")
$diff = 0
$dark = 0
for ($y = $yH + 39 - 30; $y -lt $yH + 39 + 140; $y++) {
    for ($x = $xH - 20; $x -lt $xH + 320; $x++) {
        if ($x -ge $hb.Width -or $y -ge $hb.Height) { continue }
        $hp = $hb.GetPixel($x, $y)
        $ip = $ib.GetPixel($x, $y)
        if ([math]::Abs($hp.R - $ip.R) + [math]::Abs($hp.G - $ip.G) + [math]::Abs($hp.B - $ip.B) -gt 30) { $diff++ }
        if ($hp.R -lt 80 -and $hp.G -lt 80 -and $hp.B -lt 100) { $dark++ }
    }
}
$hb.Dispose()
$ib.Dispose()
Write-Output "READOUT diffPixels=$diff darkTextPixels=$dark"
Write-Output "DONE"
