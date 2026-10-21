# Builds the staging folder and runs vpk pack (decision D8). Fails when anything the app ships beside it is missing.
param(
  [Parameter(Mandatory)][string]$Version,
  [Parameter(Mandatory)][string]$Exe,
  [Parameter(Mandatory)][string]$Notes,
  [Parameter(Mandatory)][string]$Out,
  [string]$Stage = (Join-Path $env:TEMP 'flare-stage')
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
function Need([string]$p) { if (-not (Test-Path -LiteralPath $p)) { throw "missing: $p" }; return $p }
if (Test-Path -LiteralPath $Stage) { Remove-Item -Recurse -Force -LiteralPath $Stage }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item -LiteralPath (Need $Exe) -Destination (Join-Path $Stage 'Flare.exe')
foreach ($f in 'conpty.dll', 'OpenConsole.exe') {
  Copy-Item -LiteralPath (Need (Join-Path $root "crates\flare-term\bundled\x64\$f")) -Destination $Stage
}
# The Claude plugin: M6 D8, the app looks for <exe dir>\claude-plugin and ships it only when hooks\hooks.json exists.
$plugin = Need (Join-Path $root 'crates\flare-agent\claude-plugin')
Need (Join-Path $plugin 'hooks\hooks.json') | Out-Null
Copy-Item -Recurse -LiteralPath $plugin -Destination (Join-Path $Stage 'claude-plugin')
# SEAM M8 (dictation is built last and is not in this branch yet): the dictation engine folders that ship beside the
# app. When M8 lands it adds one entry per folder here, { Source = <folder in the repo>; Name = <name the app looks for> },
# copied from M8's build.rs copy list. Until then the list is empty and nothing is copied.
$dictationSeam = @()
foreach ($d in $dictationSeam) {
  Copy-Item -Recurse -LiteralPath (Need (Join-Path $root $d.Source)) -Destination (Join-Path $Stage $d.Name)
}
foreach ($f in 'LICENSE', 'THIRD-PARTY-NOTICES.md') { Copy-Item -LiteralPath (Need (Join-Path $root $f)) -Destination $Stage }
vpk pack --packId Flare --packTitle Flare --packAuthors Max --packVersion $Version --packDir $Stage --mainExe Flare.exe --icon (Join-Path $root 'build\icon.ico') --shortcuts StartMenuRoot --aumid com.flare.terminal --runtime win-x64 --channel win --delta BestSpeed --releaseNotes $Notes --outputDir $Out
exit $LASTEXITCODE
