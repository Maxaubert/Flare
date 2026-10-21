<#
.SYNOPSIS
  Side-by-sides (Prism Terminal beside Flare) and the contact sheets for the theme matrix.

.DESCRIPTION
  Needs ImageMagick 7 (magick) on PATH. For each theme and scale, writes
  docs/screenshots/m4/side/<id>-<pct>.png (Prism Terminal left, Flare right; the GPU renderer
  when it was captured, otherwise software). For each scale and renderer, writes
  docs/screenshots/m4/sheet-<pct>-<renderer>.png, 39 tiles in wall order, each captioned with its id.
#>
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$shot = Join-Path $root 'docs\screenshots\m4'
$flare = Join-Path $shot 'flare'
$pt = Join-Path $shot 'pt'
$side = Join-Path $shot 'side'
New-Item -ItemType Directory -Force -Path $side | Out-Null

$themes = @([regex]::Matches((Get-Content -LiteralPath (Join-Path $root 'tools\oracle\out\wall_order.json') -Raw), '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
$pcts = @(100, 150, 225)
$ptVersion = 'Prism Terminal 0.58.0'
$verFile = Join-Path $pt 'VERSION.txt'
if (Test-Path -LiteralPath $verFile) {
    $ptVersion = 'Prism Terminal ' + (Get-Content -LiteralPath $verFile -Raw).Trim()
}

foreach ($id in $themes) {
    foreach ($pct in $pcts) {
        $ptFile = Join-Path $pt ("{0}-{1}.png" -f $id, $pct)
        $gpuFile = Join-Path $flare ("{0}-{1}-gpu.png" -f $id, $pct)
        $swFile = Join-Path $flare ("{0}-{1}-software.png" -f $id, $pct)
        if (Test-Path -LiteralPath $gpuFile) { $fl = $gpuFile; $label = 'Flare (gpu)' }
        else { $fl = $swFile; $label = 'Flare (software)' }
        $out = Join-Path $side ("{0}-{1}.png" -f $id, $pct)
        $capt = @('-background', '#1c1c1c', '-gravity', 'north', '-splice', '0x24', '-fill', '#e6e6e6',
                  '-font', 'Arial', '-pointsize', '14', '-gravity', 'northwest', '-annotate', '+8+4')
        $left = @($ptFile) + $capt + @($ptVersion)
        $right = @($fl) + $capt + @($label)
        & magick '(' @left ')' '(' @right ')' '+append' $out
        if ($LASTEXITCODE -ne 0) { throw "side by side failed: $id $pct" }
    }
}

$sheets = 0
foreach ($pct in $pcts) {
    foreach ($renderer in @('software', 'gpu')) {
        $args = @()
        $missing = $false
        foreach ($id in $themes) {
            $f = Join-Path $flare ("{0}-{1}-{2}.png" -f $id, $pct, $renderer)
            if (-not (Test-Path -LiteralPath $f)) { $missing = $true; break }
            $args += @('-label', $id, $f)
        }
        if ($missing) { Write-Host "skip sheet $pct $renderer (images missing)"; continue }
        $sheet = Join-Path $shot ("sheet-{0}-{1}.png" -f $pct, $renderer)
        & magick montage -background '#202020' -fill '#e6e6e6' -font Arial -pointsize 14 @args -tile 6x7 -geometry '640x410+6+6' $sheet
        if ($LASTEXITCODE -ne 0) { throw "montage failed: $pct $renderer" }
        $sheets++
    }
}
Write-Host "side-by-sides: $($themes.Count * $pcts.Count); sheets: $sheets; folder: $shot"
