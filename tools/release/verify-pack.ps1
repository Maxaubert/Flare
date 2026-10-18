# Checks the vpk output in -Out for the release of -Version. File names and shapes are the ones recorded in
# docs/shipping.md ("Recorded facts", vpk 1.2.161). Reads only; installs nothing.
param([Parameter(Mandatory)][string]$Out, [Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$problems = New-Object System.Collections.Generic.List[string]
function Fail([string]$m) { $script:problems.Add($m) }

$names = @{
  Setup    = 'Flare-win-Setup.exe'
  Portable = 'Flare-win-Portable.zip'
  Full     = ""
  Feed     = 'releases.win.json'
}
# vpk 1.2.161 names the packages without the "win" part (recorded fact).
$names.Full = "Flare-$Version-full.nupkg"
foreach ($n in $names.Setup, $names.Portable, $names.Full, $names.Feed, 'assets.win.json', 'RELEASES') {
  $p = Join-Path $Out $n
  if (-not (Test-Path -LiteralPath $p)) { Fail "missing output file: $n" }
  elseif ((Get-Item -LiteralPath $p).Length -eq 0) { Fail "empty output file: $n" }
}

$feedPath = Join-Path $Out $names.Feed
if (Test-Path -LiteralPath $feedPath) {
  $assets = (Get-Content -LiteralPath $feedPath -Raw | ConvertFrom-Json).Assets
  $full = @($assets | Where-Object { $_.Version -eq $Version -and $_.Type -eq 'Full' })
  if ($full.Count -ne 1) { Fail "releases.win.json must hold exactly one Full asset for $Version (found $($full.Count))" }
  else {
    if ($full[0].PackageId -ne 'Flare') { Fail "PackageId is '$($full[0].PackageId)', not Flare" }
    if ($full[0].FileName -ne $names.Full) { Fail "the Full asset is named '$($full[0].FileName)', expected $($names.Full)" }
    if ([string]::IsNullOrWhiteSpace($full[0].NotesMarkdown)) { Fail 'the Full asset has no NotesMarkdown' }
    $pkg = Join-Path $Out $full[0].FileName
    if ((Test-Path -LiteralPath $pkg) -and ((Get-Item -LiteralPath $pkg).Length -ne $full[0].Size)) { Fail "the size of $($full[0].FileName) differs from the feed" }
  }
  # A delta is made only when an earlier full package sits in the same folder (the workflow downloads it first).
  $earlier = @(Get-ChildItem -LiteralPath $Out -Filter 'Flare-*-full.nupkg' | Where-Object { $_.Name -ne $names.Full })
  $delta = @($assets | Where-Object { $_.Version -eq $Version -and $_.Type -eq 'Delta' })
  if ($earlier.Count -gt 0 -and $delta.Count -ne 1) { Fail "an earlier full package exists, so a delta for $Version is expected (found $($delta.Count))" }
  foreach ($d in $delta) { if (-not (Test-Path -LiteralPath (Join-Path $Out $d.FileName))) { Fail "the feed names $($d.FileName) but the file is missing" } }
  if ($earlier.Count -eq 0) { Write-Host 'note: no earlier full package in the folder, so no delta was made (the first release, or the download step found nothing)' }
}

$zipPath = Join-Path $Out $names.Portable
if (Test-Path -LiteralPath $zipPath) {
  $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
  try {
    $entries = @($zip.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
    foreach ($need in '.portable', 'Update.exe', 'Flare.exe', 'current/Flare.exe', 'current/sq.version', 'current/conpty.dll', 'current/OpenConsole.exe', 'current/claude-plugin/hooks/hooks.json', 'current/LICENSE', 'current/THIRD-PARTY-NOTICES.md') {
      if ($entries -notcontains $need) { Fail "the portable zip lacks $need" }
    }
    if (@($entries | Where-Object { $_ -like '*.pdb' }).Count -gt 0) { Fail 'the portable zip carries a .pdb' }
  } finally { $zip.Dispose() }
}

if ($problems.Count -gt 0) {
  $problems | ForEach-Object { Write-Host "verify-pack: $_" }
  throw "verify-pack: $($problems.Count) problem(s) in $Out for $Version"
}
Write-Host "verify-pack: $Version is complete in $Out"
