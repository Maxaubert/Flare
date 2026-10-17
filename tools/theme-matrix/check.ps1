<#
.SYNOPSIS
  Compares the software and GPU renderings of every theme at every scale.

.DESCRIPTION
  For each pair (software, gpu) of the same theme and scale, runs
  `magick compare -metric AE -fuzz 2%` and requires fewer than 2 percent of the pixels to differ
  (glyph edges are the only expected difference). Prints a table and exits 1 if any pair fails.
#>
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$flare = Join-Path $root 'docs\screenshots\m4\flare'
$themes = @([regex]::Matches((Get-Content -LiteralPath (Join-Path $root 'tools\oracle\out\wall_order.json') -Raw), '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
$pcts = @(100, 150, 225)

$rows = [System.Collections.Generic.List[object]]::new()
$bad = 0
foreach ($id in $themes) {
    foreach ($pct in $pcts) {
        $sw = Join-Path $flare ("{0}-{1}-software.png" -f $id, $pct)
        $gpu = Join-Path $flare ("{0}-{1}-gpu.png" -f $id, $pct)
        if (-not (Test-Path -LiteralPath $gpu) -or -not (Test-Path -LiteralPath $sw)) {
            $rows.Add([pscustomobject]@{ Theme = $id; Scale = $pct; Differ = 'n/a'; Percent = 'n/a'; Result = 'MISSING' })
            $bad++
            continue
        }
        $dims = (& magick identify -format '%w %h' $sw).Split(' ')
        $total = [double]$dims[0] * [double]$dims[1]
        # ImageMagick writes the AE count to stderr; 2>&1 captures it as text.
        # `magick compare` exits 1 when the images differ and writes the count to stderr, so its exit code is
        # ignored here and the text is read back.
        $prev = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $text = (& magick compare -metric AE -fuzz 2% $sw $gpu null: 2>&1 | ForEach-Object { $_.ToString() }) -join ' '
        $ErrorActionPreference = $prev
        $text = $text.Trim()
        $m = [regex]::Match($text, '^\s*([0-9]+(?:\.[0-9]+)?(?:e[+-]?[0-9]+)?)')
        if (-not $m.Success) { throw "unparsed compare output for $id ${pct}: $text" }
        $differ = [double]$m.Groups[1].Value
        $pctDiff = 100.0 * $differ / $total
        $ok = $pctDiff -lt 2.0
        if (-not $ok) { $bad++ }
        $rows.Add([pscustomobject]@{
            Theme = $id; Scale = $pct
            Differ = [int64]$differ
            Percent = ('{0:N3}' -f $pctDiff)
            Result = $(if ($ok) { 'ok' } else { 'FAIL' })
        })
    }
}
$rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
Write-Host "pairs checked: $($rows.Count); failing: $bad"
if ($bad -gt 0) { exit 1 }
exit 0
