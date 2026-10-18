<#
.SYNOPSIS
  Side-by-sides (Prism Terminal beside Flare) and the contact sheets for the M5 terminal scenes.

.DESCRIPTION
  Needs ImageMagick 7 (magick) on PATH. For every scene image both sides have (the list is
  `node tools\pt-shot\pt-term.mjs --list`: 12 scenes at 100, 150 and 225 percent), writes
  <Pt>\side\<name>.png (Prism Terminal left, Flare's software image right, each captioned) with
  `magick pt flare +append`. For each of the three scalings writes <Pt>\sheet-<pct>.png, the
  side-by-sides of that scaling in the list's order as a tiled contact sheet. An image only one
  side has is reported and left out.

.PARAMETER Pt
  The folder pt-term.mjs wrote (default: docs\screenshots\m10\m5-pt).

.PARAMETER Flare
  Flare's M5 images, <scene>-<pct>.png (default: docs\screenshots\m5).

.PARAMETER Only
  Limits the list to names that contain this text (a trial run).
#>
param(
    [string]$Pt = '',
    [string]$Flare = '',
    [string]$Only = ''
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Pt) { $Pt = Join-Path $root 'docs\screenshots\m10\m5-pt' }
if (-not $Flare) { $Flare = Join-Path $root 'docs\screenshots\m5' }
$side = Join-Path $Pt 'side'
New-Item -ItemType Directory -Force -Path $side | Out-Null

$jobs = @((& node (Join-Path $root 'tools\pt-shot\pt-term.mjs') --list) | ConvertFrom-Json | ForEach-Object { $_ })
if ($Only) { $jobs = @($jobs | Where-Object { $_.name -like "*$Only*" }) }
$ptVersion = 'Prism Terminal'
$verFile = Join-Path $Pt 'VERSION.txt'
if (Test-Path -LiteralPath $verFile) {
    $ptVersion = 'Prism Terminal ' + (Get-Content -LiteralPath $verFile -Raw).Trim()
}

$caption = @('-background', '#1c1c1c', '-gravity', 'north', '-splice', '0x24', '-fill', '#e6e6e6',
    '-font', 'Arial', '-pointsize', '14', '-gravity', 'northwest', '-annotate', '+8+4')

$made = 0
$skipped = 0
foreach ($job in $jobs) {
    $ptFile = Join-Path $Pt ("{0}.png" -f $job.name)
    $flFile = Join-Path $Flare ("{0}.png" -f $job.name)
    if (-not (Test-Path -LiteralPath $ptFile) -or -not (Test-Path -LiteralPath $flFile)) {
        Write-Host "skip $($job.name): an image is missing"
        $skipped++
        continue
    }
    $out = Join-Path $side ("{0}.png" -f $job.name)
    $left = @($ptFile) + $caption + @($ptVersion)
    $right = @($flFile) + $caption + @('Flare')
    & magick '(' @left ')' '(' @right ')' '+append' $out
    if ($LASTEXITCODE -ne 0) { throw "side by side failed: $($job.name)" }
    $made++
}

$sheets = 0
foreach ($pct in @(100, 150, 225)) {
    $files = @()
    foreach ($job in ($jobs | Where-Object { $_.pct -eq $pct })) {
        $f = Join-Path $side ("{0}.png" -f $job.name)
        if (Test-Path -LiteralPath $f) { $files += @('-label', $job.name, $f) }
    }
    if ($files.Count -eq 0) { Write-Host "skip sheet $pct (no side-by-sides)"; continue }
    $sheet = Join-Path $Pt ("sheet-{0}.png" -f $pct)
    & magick montage -background '#202020' -fill '#e6e6e6' -font Arial -pointsize 14 @files -tile 3x -geometry '640x+6+6' $sheet
    if ($LASTEXITCODE -ne 0) { throw "montage failed: $pct" }
    $sheets++
}
Write-Host "side-by-sides: $made (skipped $skipped); sheets: $sheets; folder: $Pt"
