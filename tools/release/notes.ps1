# GitHub's generated release notes for a tag, or the one sentence the parser treats as "no notes".
param([Parameter(Mandatory)][string]$Tag, [Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = 'Continue'
$body = $null
try {
  $body = gh api -X POST "repos/$env:GITHUB_REPOSITORY/releases/generate-notes" -f "tag_name=$Tag" --jq .body
  if ($LASTEXITCODE -ne 0) { $body = $null }
} catch { $body = $null }
if ([string]::IsNullOrWhiteSpace($body)) { $body = 'No notes were published with this release.' }
[System.IO.File]::WriteAllText($Out, $body, (New-Object System.Text.UTF8Encoding($false)))
Get-Content -LiteralPath $Out -TotalCount 5
exit 0
