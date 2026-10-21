<#
.SYNOPSIS
  Makes the contact sheets of the headless DPI matrix (the images run.ps1 wrote).

.DESCRIPTION
  Writes into the sheets folder (default docs\screenshots\m10\dpi):

    sheet-<scene>.png   one per scene: the eight scalings side by side, each downscaled to a common height
    sheet-100.png       every scene as a grid at 100% (tag 1)
    sheet-150.png       every scene as a grid at 150% (tag 1_5)
    sheet-225.png       every scene as a grid at 225% (tag 2_25)

  Each scene's images are the software renderer's by default (-Renderer gpu for the other). Downscaling
  uses the high-quality bicubic mode of System.Drawing. Only the sheets and the suspects' full images are
  committed: the 592 or 720 full images stay in the scratch folder. The suspects listed in
  suspects.txt (check.py) are copied to <sheets>\suspects when -CopySuspects is given.

.PARAMETER Images
  The folder run.ps1 wrote.

.PARAMETER Sheets
  Where the sheets go (default docs\screenshots\m10\dpi under the repo root).

.PARAMETER Renderer
  software (default) or gpu.

.PARAMETER CopySuspects
  Copy each suspect's full image into <sheets>\suspects.

.EXAMPLE
  powershell -File tools\dpi-matrix\compose.ps1 -Images <scratch>\dpi
#>
param(
    [Parameter(Mandatory = $true)][string]$Images,
    [string]$Sheets = '',
    [ValidateSet('software', 'gpu')][string]$Renderer = 'software',
    [switch]$CopySuspects
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Sheets) { $Sheets = Join-Path $root 'docs\screenshots\m10\dpi' }
if (-not (Test-Path -LiteralPath $Images)) { throw "no images folder: $Images" }
New-Item -ItemType Directory -Force -Path $Sheets | Out-Null

$scales = @('1', '1_25', '1_5', '1_75', '2', '2_25', '2_5', '3')
$ground = [System.Drawing.Color]::FromArgb(0x1C, 0x1C, 0x1C)
$ink = [System.Drawing.Color]::FromArgb(0xE6, 0xE6, 0xE6)
$labelFont = New-Object System.Drawing.Font 'Segoe UI', 12
$brush = New-Object System.Drawing.SolidBrush $ink
$groundBrush = New-Object System.Drawing.SolidBrush $ground

function Read-Image([string]$path) {
    # Copy the bytes first: FromFile keeps the file locked, and the scratch folder is reused.
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $stream = New-Object System.IO.MemoryStream (, $bytes)
    return [System.Drawing.Image]::FromStream($stream)
}

function Save-Sheet([System.Drawing.Bitmap]$bitmap, [string]$path) {
    $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
}

function Draw-Scaled($graphics, [System.Drawing.Image]$img, [int]$x, [int]$y, [int]$w, [int]$h) {
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.DrawImage($img, $x, $y, $w, $h)
}

$scenes = @(Get-ChildItem -LiteralPath $Images -Filter "*-1-$Renderer.png" | ForEach-Object {
        $_.Name -replace "-1-$Renderer\.png$", ''
    } | Where-Object { $_ -ne 'badge' } | Sort-Object)
if ($scenes.Count -eq 0) { throw "no $Renderer images in $Images" }

# One sheet per scene: the eight scalings side by side at one height.
$height = 200
$gap = 8
$labelH = 22
foreach ($scene in $scenes) {
    $imgs = @()
    foreach ($tag in $scales) {
        $p = Join-Path $Images "$scene-$tag-$Renderer.png"
        if (Test-Path -LiteralPath $p) { $imgs += , @($tag, (Read-Image $p)) }
    }
    $widths = @($imgs | ForEach-Object { [int][math]::Round($_[1].Width * $height / $_[1].Height) })
    $total = [int](($widths | Measure-Object -Sum).Sum + $gap * ($imgs.Count + 1))
    $bmp = New-Object System.Drawing.Bitmap $total, ($height + $labelH + 2 * $gap)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear($ground)
    $x = $gap
    for ($i = 0; $i -lt $imgs.Count; $i++) {
        $g.DrawString(($imgs[$i][0] -replace '_', '.') + 'x', $labelFont, $brush, $x, 2)
        Draw-Scaled $g $imgs[$i][1] $x ($labelH) $widths[$i] $height
        $x += $widths[$i] + $gap
    }
    $g.Dispose()
    Save-Sheet $bmp (Join-Path $Sheets "sheet-$scene.png")
    foreach ($item in $imgs) { $item[1].Dispose() }
}

# The grids: every scene at one scale, six to a row, each cell 320 px wide.
$cell = 320
$cols = 6
$grids = @(@{ Tag = '1'; Name = 'sheet-100.png' }, @{ Tag = '1_5'; Name = 'sheet-150.png' }, @{ Tag = '2_25'; Name = 'sheet-225.png' })
foreach ($grid in $grids) {
    $cells = @()
    foreach ($scene in $scenes) {
        $p = Join-Path $Images "$scene-$($grid.Tag)-$Renderer.png"
        if (Test-Path -LiteralPath $p) { $cells += , @($scene, (Read-Image $p)) }
    }
    if ($cells.Count -eq 0) { continue }
    $rows = [math]::Ceiling($cells.Count / $cols)
    $rowH = @()
    for ($r = 0; $r -lt $rows; $r++) {
        $h = 0
        for ($c = 0; $c -lt $cols; $c++) {
            $k = $r * $cols + $c
            if ($k -lt $cells.Count) {
                $ch = [int][math]::Round($cells[$k][1].Height * $cell / $cells[$k][1].Width)
                if ($ch -gt $h) { $h = $ch }
            }
        }
        $rowH += ($h + 26)
    }
    $totalH = [int](($rowH | Measure-Object -Sum).Sum + $gap * ($rows + 1))
    $bmp = New-Object System.Drawing.Bitmap ($cols * $cell + ($cols + 1) * $gap), $totalH
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear($ground)
    $y = $gap
    for ($r = 0; $r -lt $rows; $r++) {
        for ($c = 0; $c -lt $cols; $c++) {
            $k = $r * $cols + $c
            if ($k -ge $cells.Count) { break }
            $x = $gap + $c * ($cell + $gap)
            $img = $cells[$k][1]
            $g.DrawString($cells[$k][0], $labelFont, $brush, $x, $y)
            $ch = [int][math]::Round($img.Height * $cell / $img.Width)
            Draw-Scaled $g $img $x ($y + 22) $cell $ch
        }
        $y += $rowH[$r] + $gap
    }
    $g.Dispose()
    Save-Sheet $bmp (Join-Path $Sheets $grid.Name)
    foreach ($item in $cells) { $item[1].Dispose() }
}

if ($CopySuspects) {
    $list = Join-Path $Images 'suspects.txt'
    if (Test-Path -LiteralPath $list) {
        $dest = Join-Path $Sheets 'suspects'
        New-Item -ItemType Directory -Force -Path $dest | Out-Null
        foreach ($line in (Get-Content -LiteralPath $list)) {
            if ($line -match '^SUSPECT (\S+) (\S+) (\S+):') {
                $name = "$($Matches[1])-$($Matches[3])-$($Matches[2]).png"
                $src = Join-Path $Images $name
                if (Test-Path -LiteralPath $src) { Copy-Item -LiteralPath $src -Destination $dest -Force }
            }
        }
    }
}

$made = @(Get-ChildItem -LiteralPath $Sheets -Filter 'sheet-*.png').Count
Write-Host "wrote $made sheet(s) into $Sheets from $($scenes.Count) scenes ($Renderer)"
