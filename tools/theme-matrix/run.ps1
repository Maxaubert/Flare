<#
.SYNOPSIS
  Renders every theme in the wall at 100, 150 and 225 percent on both grid painters.

.DESCRIPTION
  Runs flare.exe --scene theme-sample for each of the 39 themes (wall order, from
  tools/oracle/out/wall_order.json), each scale and each renderer, writing
  docs/screenshots/m4/flare/<id>-<pct>-<renderer>.png. Exit code 2 from the GPU renderer means the
  GPU is unavailable: the run records it and carries on with the software renderer only. Any other
  non-zero exit stops the run.

.PARAMETER Exe
  The flare.exe to run (default: target\release\flare.exe under the repo root).

.EXAMPLE
  powershell -File tools\theme-matrix\run.ps1 -Exe <path to flare.exe>
#>
param(
    [string]$Exe = ''
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Exe) { $Exe = Join-Path $root 'target\release\flare.exe' }
if (-not (Test-Path -LiteralPath $Exe)) { throw "flare.exe not found: $Exe" }

$themes = @([regex]::Matches((Get-Content -LiteralPath (Join-Path $root 'tools\oracle\out\wall_order.json') -Raw), '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
if ($themes.Count -ne 39) { throw "expected 39 themes, found $($themes.Count)" }

# pt-default first, then the rest in wall order.
$ordered = @('pt-default') + @($themes | Where-Object { $_ -ne 'pt-default' })

$scales = @(
    @{ Scale = '1.0'; Pct = 100 },
    @{ Scale = '1.5'; Pct = 150 },
    @{ Scale = '2.25'; Pct = 225 }
)

$outDir = Join-Path $root 'docs\screenshots\m4\flare'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$gpuOk = $true
$total = 0

foreach ($renderer in @('software', 'gpu')) {
    if ($renderer -eq 'gpu' -and -not $gpuOk) { continue }
    foreach ($id in $ordered) {
        foreach ($s in $scales) {
            $png = Join-Path $outDir ("{0}-{1}-{2}.png" -f $id, $s.Pct, $renderer)
            # flare.exe is a GUI-subsystem binary: `&` returns at once without an exit code, so wait on the process.
            $proc = Start-Process -FilePath $Exe -PassThru -Wait -WindowStyle Hidden -ArgumentList @(
                '--scene', 'theme-sample', '--theme', $id, '--scale', $s.Scale, '--size', '1000x640',
                '--renderer', $renderer, '--screenshot', $png)
            $code = $proc.ExitCode
            if ($code -eq 2 -and $renderer -eq 'gpu') {
                Write-Host "GPU unavailable (exit 2) at $id $($s.Pct): recording, continuing with software only."
                $gpuOk = $false
                break
            }
            if ($code -ne 0) { throw "flare.exe exit $code for $id $($s.Pct) $renderer" }
            if (-not (Test-Path -LiteralPath $png)) { throw "no image written: $png" }
            $total++
        }
        if (-not $gpuOk -and $renderer -eq 'gpu') { break }
    }
}

Write-Host "rendered $total images into $outDir"
if (-not $gpuOk) { Write-Host 'GPU renderer images are incomplete (see above).' }
exit 0
