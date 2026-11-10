# Fails a build with live updates while deny.toml still ignores the zip 8.6 advisories that come in through
# velopack 1.2.161. The updater may only be turned on after velopack moves to zip >= 9 and the ignores are gone
# (audit 2026-10-20, L60; deny.toml comment). With the updater off, the ignores are allowed.
param(
    [string]$Deny = (Join-Path $PSScriptRoot '..\..\deny.toml'),
    [switch]$Live
)
$ids = 'RUSTSEC-2026-0367', 'RUSTSEC-2026-0368', 'RUSTSEC-2026-0369', 'RUSTSEC-2026-0429'
$text = Get-Content -Raw -LiteralPath $Deny
$left = @($ids | Where-Object { $text -match [regex]::Escape($_) })
if ($left.Count -gt 0 -and $Live) {
    Write-Host "::error::live updates are on but deny.toml still ignores $($left -join ', '); bump velopack to a zip >= 9 release and drop them first"
    exit 1
}
if ($left.Count -gt 0) { Write-Host "::notice::the zip advisories are still ignored ($($left -join ', ')); fine while the updater is off." }
exit 0
