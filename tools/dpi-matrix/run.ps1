<#
.SYNOPSIS
  Renders every scene of the headless DPI matrix at the eight display scalings with both renderers.

.DESCRIPTION
  The scenes are the names `flare.exe --scene list` prints (SCENES in crates/flare/src/headless.rs).
  For each scene, each scale of 1 1.25 1.5 1.75 2 2.25 2.5 3 and each renderer (software, then gpu), runs

    flare.exe --scene <name> --scale <s> --renderer <r> --size 1000x640 --screenshot <png>

  and writes <Out>\<scene>-<tag>-<renderer>.png (the scale's dot written as an underscore: 1_25, 2_25).
  The gpu pass is skipped, with a note, when `--probe-backend --renderer gpu` finds no adapter.
  Runs one flare.exe at a time (the owner uses this PC: nothing runs in parallel). Nothing opens a
  window: the scenes draw into a buffer. Then runs check.py over the folder (unless -NoCheck).

  Exit 1 when any render fails (exit code, missing or empty file) or check.py reports a failure.

.PARAMETER Exe
  The flare.exe to run (default: target\release\flare.exe under the repo root).

.PARAMETER Out
  The folder for the 592 or 720 images (default: a flare-dpi-matrix folder under TEMP). Keep it out of
  the repository: only the sheets and the suspects' images are committed (compose.ps1).

.PARAMETER Only
  Render only the scenes whose name contains this text.

.PARAMETER Scales
  The scales, comma separated (default: all eight).

.PARAMETER NoCheck
  Skip check.py.

.EXAMPLE
  powershell -File tools\dpi-matrix\run.ps1 -Exe <path to flare.exe> -Out <scratch>\dpi
#>
param(
    [string]$Exe = '',
    [string]$Out = '',
    [string]$Only = '',
    [string]$Scales = '1,1.25,1.5,1.75,2,2.25,2.5,3',
    [switch]$NoCheck
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Exe) { $Exe = Join-Path $root 'target\release\flare.exe' }
if (-not $Out) { $Out = Join-Path $env:TEMP 'flare-dpi-matrix' }
if (-not (Test-Path -LiteralPath $Exe)) { throw "flare.exe not found: $Exe" }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$scaleList = @($Scales -split '[,; ]+' | Where-Object { $_ } | ForEach-Object { [double]::Parse($_, [cultureinfo]::InvariantCulture) })
if ($scaleList.Count -eq 0) { throw 'no scale given' }

function Run-Flare([string[]]$FlareArgs, [string]$StdOut = '') {
    if ($StdOut) {
        $proc = Start-Process -FilePath $Exe -PassThru -Wait -WindowStyle Hidden -ArgumentList $FlareArgs -RedirectStandardOutput $StdOut
    } else {
        $proc = Start-Process -FilePath $Exe -PassThru -Wait -WindowStyle Hidden -ArgumentList $FlareArgs
    }
    return $proc.ExitCode
}

# The scene names come from the app itself, so the matrix can never drift from SCENES.
$listFile = Join-Path $Out 'scenes.txt'
$code = Run-Flare @('--scene', 'list') $listFile
if ($code -ne 0) { throw "flare.exe --scene list exited $code" }
$scenes = @(Get-Content -LiteralPath $listFile | Where-Object { $_.Trim() })
if ($scenes.Count -eq 0) { throw 'flare.exe --scene list printed no scenes' }
# `badge` is a contact sheet of every taskbar disc at its own scales, not a window: --scale does not
# apply to it (check.py's NOT_WINDOWS), so it is not rendered here.
$scenes = @($scenes | Where-Object { $_ -ne 'badge' })
if ($Only) { $scenes = @($scenes | Where-Object { $_ -like "*$Only*" }) }
if ($scenes.Count -eq 0) { throw 'no scene matches -Only' }

# The gpu pass needs an adapter: ask flare itself, and keep its answer for the note.
$probeFile = Join-Path $Out 'probe.txt'
$gpu = $false
$gpuNote = ''
$psi = Start-Process -FilePath $Exe -ArgumentList @('--probe-backend', '--renderer', 'gpu') `
    -PassThru -Wait -WindowStyle Hidden -RedirectStandardOutput $probeFile
$said = if (Test-Path -LiteralPath $probeFile) { (Get-Content -LiteralPath $probeFile -Raw) } else { '' }
if ($psi.ExitCode -eq 0 -and $said -match 'backend:\s*gpu' -and $said -notmatch 'fallback') { $gpu = $true }
else { $gpuNote = ($said + " exit $($psi.ExitCode)").Trim() }
Remove-Item -LiteralPath $probeFile -Force -ErrorAction SilentlyContinue
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
    $failures | ForEach-Object { Write-Host "FAILED $_" }
    Write-Host "rendered $total images into $Out, $($failures.Count) failed"
    exit 1
}
Write-Host "rendered $total images into $Out ($($renderers -join ' + '))"
if ($NoCheck) { exit 0 }

$check = Join-Path $root 'tools\dpi-matrix\check.py'
$checkArgs = @($check, $Out)
if (-not $gpu) { $checkArgs += @('--renderers', 'software') }
& python @checkArgs
exit $LASTEXITCODE
