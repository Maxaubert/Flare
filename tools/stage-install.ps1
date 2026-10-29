# Stages a release build for one installed copy of Flare (install when idle, 2026-10-29).
#
# Copies the release files into <install>\staged\ under the names they have in <install>\current\, and writes
# staged.json LAST: {commit, built_at, files: {name: sha256}}. The running app is not touched: Flare sees the
# staged build, shows "Build ready" and installs it once no agent is working (or when Max clicks Install now).
# A second stage replaces the first. Always ask Max before staging.
#
#   pwsh tools\stage-install.ps1 -Stable              # %LOCALAPPDATA%\StableFlare (FlareStable.exe)
#   pwsh tools\stage-install.ps1                      # %LOCALAPPDATA%\Flare (Flare.exe)
#   pwsh tools\stage-install.ps1 -Bin <target>\release -Repo <checkout>
#   pwsh tools\stage-install.ps1 -Root <temp install> -ExeName Flare.exe   # a test install
param(
  [switch]$Stable,
  [string]$Repo = (Split-Path -Parent $PSScriptRoot),
  [string]$Bin,
  [string]$Root,
  [string]$ExeName,
  [string]$Log
)
$ErrorActionPreference = 'Stop'
function Say($m) { $line = "$(Get-Date -Format HH:mm:ss) $m"; Write-Host $line; if ($Log) { Add-Content -Path $Log -Value $line } }

if (-not $Bin) { $Bin = Join-Path $Repo 'target\release' }
if (-not $Root) {
  $Root = if ($Stable) { Join-Path $env:LOCALAPPDATA 'StableFlare' } else { Join-Path $env:LOCALAPPDATA 'Flare' }
}
if (-not $ExeName) { $ExeName = if ($Stable) { 'FlareStable.exe' } else { 'Flare.exe' } }

try {
  $cur = Join-Path $Root 'current'
  if (-not (Test-Path $cur)) { throw "$cur does not exist: this is not an installed copy" }
  # What goes in: name in current\ = source file.
  $files = [ordered]@{}
  $files[$ExeName] = Join-Path $Bin 'flare.exe'
  $files['flare-mcp.exe'] = Join-Path $Bin 'flare-mcp.exe'
  $files['flare-parakeet-host.exe'] = Join-Path $Bin 'flare-parakeet-host.exe'
  $files['LICENSE'] = Join-Path $Repo 'LICENSE'
  $files['THIRD-PARTY-NOTICES.md'] = Join-Path $Repo 'THIRD-PARTY-NOTICES.md'
  $plugin = Join-Path $Repo 'crates\flare-agent\claude-plugin'
  if (Test-Path $plugin) {
    Get-ChildItem -Path $plugin -Recurse -File | ForEach-Object {
      $rel = $_.FullName.Substring($plugin.Length).TrimStart('\') -replace '\\', '/'
      $files["claude-plugin/$rel"] = $_.FullName
    }
  }
  foreach ($k in @($files.Keys)) {
    if (-not (Test-Path $files[$k])) { throw "$($files[$k]) is missing; build first (cargo build --release -p flare -p flare-mcp -p flare-parakeet-host)" }
  }

  $commit = (& git -C $Repo rev-parse HEAD 2>$null)
  if ($LASTEXITCODE -ne 0 -or -not $commit) { throw "cannot read the commit of $Repo" }
  $global:LASTEXITCODE = 0
  $commit = $commit.Trim()
  $dirty = (& git -C $Repo status --porcelain --untracked-files=no 2>$null)
  if ($dirty) { $commit = "$commit-dirty" }

  # The old stage goes first, its manifest before anything else, so a half-copied stage is never ready.
  $staged = Join-Path $Root 'staged'
  $manifest = Join-Path $staged 'staged.json'
  if (Test-Path $manifest) { Remove-Item -Force $manifest }
  if (Test-Path $staged) { Remove-Item -Recurse -Force $staged }
  New-Item -ItemType Directory -Force -Path $staged | Out-Null

  $hashes = [ordered]@{}
  foreach ($k in $files.Keys) {
    $to = Join-Path $staged ($k -replace '/', '\')
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $to) | Out-Null
    Copy-Item -Force $files[$k] $to
    $hashes[$k] = (Get-FileHash -Algorithm SHA256 -Path $to).Hash.ToLowerInvariant()
  }
  $built = (Get-Item $files[$ExeName]).LastWriteTimeUtc.ToString('yyyy-MM-ddTHH:mm:ssZ')
  $json = [ordered]@{ commit = $commit; built_at = $built; files = $hashes } | ConvertTo-Json -Depth 4
  # Written next to it, then renamed: Flare never reads half a manifest.
  $tmp = "$manifest.tmp"
  [IO.File]::WriteAllText($tmp, $json, (New-Object Text.UTF8Encoding($false)))
  Move-Item -Force $tmp $manifest
  Say "staged $($commit.Substring(0, [Math]::Min(7, $commit.Length))) for $Root ($($hashes.Count) files); it installs when idle"
} catch {
  Say "FAILED: $($_.Exception.Message)"
  exit 1
}
