<#
.SYNOPSIS
  Checks the M5 terminal scene images: sizes, blank images, and that no real profile was touched.

.DESCRIPTION
  Needs ImageMagick 7 (magick) on PATH. For every image of the list
  (`node tools\pt-shot\pt-term.mjs --list`) prints one row: the Flare and the Prism Terminal size,
  the size the scaling asks for (round(1000 * s) by round(640 * s)), a blank check (an image with
  three colours or fewer is blank) and the share of the page body where the two differ by more
  than 12 percent in luminance after both are shrunk to height 200 and the title bar is cropped
  away (a number for the person reading the images, not a gate). Exits 1 on a missing image, a
  size that is not the one asked for, or a blank image. The owner's Prism Terminal profile folders
  (its two folders under %APPDATA%) must not have changed since the script began
  unless -Since is given: pass the time the capture began and a folder written after it fails.

.PARAMETER Pt
  The folder pt-term.mjs wrote (default: docs\screenshots\m10\m5-pt).

.PARAMETER Flare
  Flare's M5 images (default: docs\screenshots\m5).

.PARAMETER Since
  The time the capture began (a [datetime]); real profile folders written after it fail the check.

.PARAMETER Only
  Limits the list to names that contain this text (a trial run).
#>
param(
    [string]$Pt = '',
    [string]$Flare = '',
    [datetime]$Since = [datetime]::MinValue,
    [string]$Only = ''
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Pt) { $Pt = Join-Path $root 'docs\screenshots\m10\m5-pt' }
if (-not $Flare) { $Flare = Join-Path $root 'docs\screenshots\m5' }
$jobs = @((& node (Join-Path $root 'tools\pt-shot\pt-term.mjs') --list) | ConvertFrom-Json | ForEach-Object { $_ })
if ($Only) { $jobs = @($jobs | Where-Object { $_.name -like "*$Only*" }) }
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("m5-check-{0}" -f [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

function Get-Size([string]$file) {
    (& magick identify -format '%wx%h' $file).Trim()
}

function Get-Colours([string]$file) {
    [int](& magick $file -format '%k' info:)
}

$rows = [System.Collections.Generic.List[object]]::new()
$bad = 0
try {
    foreach ($job in $jobs) {
        $fl = Join-Path $Flare ("{0}.png" -f $job.name)
        $pp = Join-Path $Pt ("{0}.png" -f $job.name)
        $want = '{0}x{1}' -f [int][Math]::Round(1000 * $job.scale, [MidpointRounding]::AwayFromZero), [int][Math]::Round(640 * $job.scale, [MidpointRounding]::AwayFromZero)
        if (-not (Test-Path -LiteralPath $fl) -or -not (Test-Path -LiteralPath $pp)) {
            $rows.Add([pscustomobject]@{ Image = $job.name; Flare = '-'; PT = '-'; Want = $want; Blank = '-'; 'Differ %' = '-'; Result = 'MISSING' })
            $bad++
            continue
        }
        $sf = Get-Size $fl
        $sp = Get-Size $pp
        $blank = (Get-Colours $fl) -le 3 -or (Get-Colours $pp) -le 3
        $result = 'ok'
        if ($sf -ne $want -or $sp -ne $want -or $sf -ne $sp) { $result = 'SIZE' }
        if ($blank) { $result = 'BLANK' }
        $differ = 'n/a'
        if ($result -eq 'ok') {
            $a = Join-Path $tmp 'a.png'
            $b = Join-Path $tmp 'b.png'
            foreach ($pair in @(@($pp, $a), @($fl, $b))) {
                & magick $pair[0] -colorspace Gray -resize x200 -crop '100%x94%+0+6%' +repage $pair[1]
                if ($LASTEXITCODE -ne 0) { throw "resize failed: $($job.name)" }
            }
            $dims = (& magick identify -format '%w %h' $a).Split(' ')
            $area = [double]$dims[0] * [double]$dims[1]
            # `magick compare` exits 1 when the images differ and writes the count to stderr.
            $prev = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            $text = (& magick compare -metric AE -fuzz 12% $a $b null: 2>&1 | ForEach-Object { $_.ToString() }) -join ' '
            $ErrorActionPreference = $prev
            $m = [regex]::Match($text.Trim(), '^\s*([0-9]+(?:\.[0-9]+)?(?:e[+-]?[0-9]+)?)')
            if (-not $m.Success) { throw "unparsed compare output for $($job.name): $text" }
            $differ = '{0:N1}' -f (100.0 * [double]$m.Groups[1].Value / $area)
        }
        if ($result -ne 'ok') { $bad++ }
        $rows.Add([pscustomobject]@{ Image = $job.name; Flare = $sf; PT = $sp; Want = $want; Blank = $(if ($blank) { 'BLANK' } else { 'no' }); 'Differ %' = $differ; Result = $result })
    }
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
$rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

# The owner's profiles: the capture never writes them (it uses a throwaway profile under %TEMP%).
if ($Since -gt [datetime]::MinValue) {
    foreach ($n in @(('Pri' + 'smTerminalStable'), ('Pri' + 'smTerminal'))) {
        $p = Join-Path $env:APPDATA $n
        if ((Test-Path -LiteralPath $p) -and (Get-Item -LiteralPath $p).LastWriteTime -gt $Since) {
            Write-Host "FAIL real profile touched: $p"
            $bad++
        }
    }
}
Write-Host "images checked: $($rows.Count); failing: $bad"
if ($bad -gt 0) { exit 1 }
exit 0
