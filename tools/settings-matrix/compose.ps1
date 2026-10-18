<#
.SYNOPSIS
  Side-by-sides (Prism Terminal beside Flare) and the contact sheets for the settings matrix.

.DESCRIPTION
  Needs ImageMagick 7 (magick) on PATH. For every image both sides have, writes
  <Shots>\side\<name>.png (Prism Terminal left, Flare right, each captioned "Prism Terminal
  <version>" and "Flare") with `magick pt flare +append`. For each of the three main scalings
  (100, 150, 225 percent) writes <Shots>\sheet-<pct>.png, the side-by-sides of that scaling in the
  list's order as a tiled contact sheet. An image only one side has is reported and left out.

.PARAMETER Shots
  The matrix folder (default: docs\screenshots\m7); it holds flare\ and pt\.
#>
param(
    [string]$Shots = ''
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Shots) { $Shots = Join-Path $root 'docs\screenshots\m7' }
$flare = Join-Path $Shots 'flare'
$pt = Join-Path $Shots 'pt'
$side = Join-Path $Shots 'side'
New-Item -ItemType Directory -Force -Path $side | Out-Null

$jobs = @((& node (Join-Path $root 'tools\pt-shot\pt-settings.mjs') --list) | ConvertFrom-Json | ForEach-Object { $_ })
$ptVersion = 'Prism Terminal'
$verFile = Join-Path $pt 'VERSION.txt'
if (Test-Path -LiteralPath $verFile) {
    $ptVersion = 'Prism Terminal ' + (Get-Content -LiteralPath $verFile -Raw).Trim()
}

$caption = @('-background', '#1c1c1c', '-gravity', 'north', '-splice', '0x24', '-fill', '#e6e6e6',
    '-font', 'Arial', '-pointsize', '14', '-gravity', 'northwest', '-annotate', '+8+4')

$made = 0
$skipped = 0
foreach ($job in $jobs) {
    $ptFile = Join-Path $pt ("{0}.png" -f $job.name)
    $flFile = Join-Path $flare ("{0}.png" -f $job.name)
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
    $sheet = Join-Path $Shots ("sheet-{0}.png" -f $pct)
    & magick montage -background '#202020' -fill '#e6e6e6' -font Arial -pointsize 14 @files -tile 3x -geometry '640x+6+6' $sheet
    if ($LASTEXITCODE -ne 0) { throw "montage failed: $pct" }
    $sheets++
}
Write-Host "side-by-sides: $made (skipped $skipped); sheets: $sheets; folder: $Shots"
