<#
.SYNOPSIS
  Renders Flare's eight update scenes at the eight display scalings with each renderer.

.DESCRIPTION
  For each scene of update-chip, update-chip-notice, update-window, update-window-running,
  update-window-installing, update-window-notice, update-window-small and update-window-empty, each
  scale of 1.0 1.25 1.5 1.75 2.0 2.25 2.5 3.0 and each renderer (software, then gpu), runs

    flare.exe --scene <name> --scale <s> --renderer <r> --size 1000x640 --screenshot <png>

  and writes <Out>\<scene>-<scale>-<renderer>.png, the scale written with an underscore for its dot
  (1_25, 2_25), the same tag the Prism Terminal images wear. update-window-small ignores --size: the
  scene is 1000 x 420 by its own rule. The gpu pass is skipped, with a note, when `--probe-backend
  --renderer gpu` finds no adapter (it reports a software fallback).

  Fails (exit 1) when any call exits non-zero, writes no file or writes an empty one, naming the
  call. flare.exe is a GUI-subsystem binary: the script waits on the process and reads its exit code.
  Nothing here opens a window: the scenes draw into a buffer.

.PARAMETER Exe
  The flare.exe to run (default: target\release\flare.exe under the repo root).

.PARAMETER Out
  The folder for the images (default: docs\screenshots\m9\flare).

.PARAMETER Only
  Render only the scenes whose name contains this text.

.PARAMETER Scales
  The scales to render, comma separated (default: all eight, 1,1.25,1.5,1.75,2,2.25,2.5,3).

.EXAMPLE
  powershell -File tools\m9-matrix\run.ps1 -Exe <path to flare.exe> -Out docs\screenshots\m9\flare
#>
param(
    [string]$Exe = '',
    [string]$Out = '',
    [string]$Only = '',
    [string]$Scales = '1,1.25,1.5,1.75,2,2.25,2.5,3'
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Exe) { $Exe = Join-Path $root 'target\release\flare.exe' }
if (-not $Out) { $Out = Join-Path $root 'docs\screenshots\m9\flare' }
if (-not (Test-Path -LiteralPath $Exe)) { throw "flare.exe not found: $Exe" }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$scaleList = @($Scales -split '[,; ]+' | Where-Object { $_ } | ForEach-Object { [double]::Parse($_, [cultureinfo]::InvariantCulture) })
if ($scaleList.Count -eq 0) { throw 'no scale given' }

$scenes = @(
    'update-chip', 'update-chip-notice', 'update-window', 'update-window-running',
    'update-window-installing', 'update-window-notice', 'update-window-small', 'update-window-empty'
)
if ($Only) { $scenes = @($scenes | Where-Object { $_ -like "*$Only*" }) }
if ($scenes.Count -eq 0) { throw 'no scene matches -Only' }

function Run-Flare([string[]]$FlareArgs) {
    $proc = Start-Process -FilePath $Exe -PassThru -Wait -WindowStyle Hidden -ArgumentList $FlareArgs
    return $proc.ExitCode
}

# The gpu pass needs an adapter: ask flare itself, and keep its answer for the note.
$probeFile = Join-Path ([IO.Path]::GetTempPath()) ("m9-probe-{0}.txt" -f [guid]::NewGuid())
$gpu = $false
$gpuNote = ''
try {
    $psi = Start-Process -FilePath $Exe -ArgumentList @('--probe-backend', '--renderer', 'gpu') `
        -PassThru -Wait -WindowStyle Hidden -RedirectStandardOutput $probeFile
    $said = if (Test-Path -LiteralPath $probeFile) { (Get-Content -LiteralPath $probeFile -Raw) } else { '' }
    if ($psi.ExitCode -eq 0 -and $said -match 'backend:\s*gpu' -and $said -notmatch 'fallback') { $gpu = $true }
    else { $gpuNote = ($said + " exit $($psi.ExitCode)").Trim() }
}
finally {
    Remove-Item -LiteralPath $probeFile -Force -ErrorAction SilentlyContinue
}
$renderers = @('software')
if ($gpu) { $renderers += 'gpu' }
else { Write-Host "note: no gpu adapter, the gpu pass is skipped ($gpuNote)" }

$total = 0
$failures = @()
foreach ($scene in $scenes) {
    foreach ($scale in $scaleList) {
        $s = $scale.ToString([cultureinfo]::InvariantCulture)
        $tag = $s.Replace('.', '_')
        foreach ($r in $renderers) {
            $png = Join-Path $Out ("{0}-{1}-{2}.png" -f $scene, $tag, $r)
            if (Test-Path -LiteralPath $png) { Remove-Item -LiteralPath $png -Force }
            $flags = @('--scene', $scene, '--scale', $s, '--renderer', $r, '--size', '1000x640', '--screenshot', $png)
            $code = Run-Flare $flags
            if ($code -ne 0) { $failures += "exit $code : $scene $s $r"; continue }
            if (-not (Test-Path -LiteralPath $png)) { $failures += "no image : $scene $s $r"; continue }
            if ((Get-Item -LiteralPath $png).Length -eq 0) { $failures += "empty image : $scene $s $r"; continue }
            $total++
        }
    }
}

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Error $_ -ErrorAction Continue }
    Write-Host "rendered $total images into $Out, $($failures.Count) failed"
    exit 1
}
Write-Host "rendered $total images into $Out ($($renderers -join ' + '))"
exit 0
