<#
Read-only window capture. Finds a visible top-level window whose title matches -Title (wildcards)
and saves it to -Out with PrintWindow(PW_RENDERFULLCONTENT). Never focuses, starts, stops, moves
or sends input to anything. Exit 0 and prints the size on success; exit 2 if no window matches.
Usage: tools\shot-window.ps1 -Title "Prism Terminal*" -Out shot.png
#>
param(
  [Parameter(Mandatory)][string]$Title,
  [Parameter(Mandatory)][string]$Out
)
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class ShotWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static List<IntPtr> All() {
    var l = new List<IntPtr>();
    EnumWindows((h, _) => { if (IsWindowVisible(h)) l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static string TitleOf(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
  public static RECT RectOf(IntPtr h) { RECT r; GetWindowRect(h, out r); return r; }
}
'@
$hwnd = [IntPtr]::Zero
foreach ($h in [ShotWin]::All()) {
  if ([ShotWin]::TitleOf($h) -like $Title) { $hwnd = $h; break }
}
if ($hwnd -eq [IntPtr]::Zero) { Write-Error "no window matches '$Title'"; exit 2 }
$r = [ShotWin]::RectOf($hwnd)
$w = $r.R - $r.L; $hgt = $r.B - $r.T
if ($w -le 0 -or $hgt -le 0) { Write-Error "window has no size"; exit 2 }
$bmp = New-Object System.Drawing.Bitmap $w, $hgt
$g = [System.Drawing.Graphics]::FromImage($bmp)
$dc = $g.GetHdc()
$ok = [ShotWin]::PrintWindow($hwnd, $dc, 2)   # PW_RENDERFULLCONTENT
$g.ReleaseHdc($dc); $g.Dispose()
if (-not $ok) { $bmp.Dispose(); Write-Error "PrintWindow failed"; exit 3 }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "$w x $hgt -> $Out"
