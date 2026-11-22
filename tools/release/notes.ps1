# GitHub's generated release notes for a tag, or the one sentence the parser treats as "no notes".
param([Parameter(Mandatory)][string]$Tag, [Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = 'Continue'
$body = $null
try {
  # gh prints the notes as many lines, which PowerShell keeps as an array; join them with line breaks (rule 1341), or
  # WriteAllText joins them with spaces and the notes lose every heading and bullet.
  $body = (gh api -X POST "repos/$env:GITHUB_REPOSITORY/releases/generate-notes" -f "tag_name=$Tag" --jq .body) -join "`n"
  if ($LASTEXITCODE -ne 0) { $body = $null }
} catch { $body = $null }
if ([string]::IsNullOrWhiteSpace($body)) { $body = 'No notes were published with this release.' }
[System.IO.File]::WriteAllText($Out, $body, (New-Object System.Text.UTF8Encoding($false)))
Get-Content -LiteralPath $Out -TotalCount 5
exit 0
