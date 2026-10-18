# Checks the vpk output in -Out for the release of -Version. Skeleton: Task 29 records the real file names and fills in the checks.
param([Parameter(Mandatory)][string]$Out, [Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
throw "verify-pack: the expected file names for $Version in $Out are not recorded yet (Task 29 fills them in)"
