<#
.SYNOPSIS
  Renders Flare's Settings scene for every image of the settings matrix.

.DESCRIPTION
  The list of images is the one tools\pt-shot\pt-settings.mjs prints with --list, so the Flare
  images and the Prism Terminal images are the same set by construction: the six pages at 100, 150
  and 225 percent, Appearance with each overlay (picker, editor, ask, help, wall-expanded) and
  Terminal with select and help at the same three scales, a Find result for Appearance, and
  Appearance and Terminal at all eight scalings (1, 1.25, 1.5, 1.75, 2, 2.25, 2.5, 3). Appearance
  has no select row, so it has no select image. A name that two groups share is rendered once.

  Runs flare.exe --scene settings with the software renderer for each image and writes
  <Out>\<page>-<variant>-<pct>.png. A non-zero exit stops the run. flare.exe is a GUI-subsystem
  binary: the script waits on the process and reads its exit code.

.PARAMETER Exe
  The flare.exe to run (default: target\release\flare.exe under the repo root).

.PARAMETER Out
  The folder for the images (default: docs\screenshots\m7\flare).

.PARAMETER Only
  Render only the images whose name contains this text.

.EXAMPLE
  powershell -File tools\settings-matrix\run.ps1 -Exe <path to flare.exe>
#>
param(
    [string]$Exe = '',
    [string]$Out = '',
    [string]$Only = ''
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Exe) { $Exe = Join-Path $root 'target\release\flare.exe' }
if (-not $Out) { $Out = Join-Path $root 'docs\screenshots\m7\flare' }
if (-not (Test-Path -LiteralPath $Exe)) { throw "flare.exe not found: $Exe" }
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$listJson = & node (Join-Path $root 'tools\pt-shot\pt-settings.mjs') --list
if ($LASTEXITCODE -ne 0) { throw 'pt-settings.mjs --list failed' }
$jobs = @($listJson | ConvertFrom-Json | ForEach-Object { $_ })
if ($Only) { $jobs = @($jobs | Where-Object { $_.name -like "*$Only*" }) }
if ($jobs.Count -eq 0) { throw 'no image matches -Only' }

$total = 0
foreach ($job in $jobs) {
    $png = Join-Path $Out ("{0}.png" -f $job.name)
    $scale = ([double]$job.scale).ToString([cultureinfo]::InvariantCulture)
    $flags = @('--scene', 'settings', '--settings-page', $job.page, '--scale', $scale,
        '--size', '1000x640', '--renderer', 'software', '--screenshot', $png)
    if ($job.open -ne 'none') { $flags += @('--settings-open', $job.open) }
    if ($job.find) { $flags += @('--settings-find', $job.find) }
    if (Test-Path -LiteralPath $png) { Remove-Item -LiteralPath $png -Force }
    $proc = Start-Process -FilePath $Exe -PassThru -Wait -WindowStyle Hidden -ArgumentList $flags
    if ($proc.ExitCode -ne 0) { throw "flare.exe exit $($proc.ExitCode) for $($job.name)" }
    if (-not (Test-Path -LiteralPath $png)) { throw "no image written: $png" }
    $total++
}

Write-Host "rendered $total images into $Out"
exit 0
