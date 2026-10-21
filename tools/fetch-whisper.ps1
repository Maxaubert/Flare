<#
.SYNOPSIS
  Fetches the whisper.cpp CPU engine that Flare bundles (M8 Task 28), at build time.

.DESCRIPTION
  PowerShell port of Prism Terminal's core/tools/fetch-whisper.mjs at commit e5848cb. The engine is
  the OFFICIAL whisper.cpp release zip, pinned by tag and checked by byte count and SHA-256 BEFORE it
  is unpacked. Its 15 files land flat in -Out. The four Microsoft C++ runtime DLLs are copied from
  System32 only when Authenticode says Valid and the signer is Microsoft Corporation. The MIT notice
  is written beside them, and a stamp file holding the zip's SHA-256 is written last. A folder that
  already holds the complete engine for this pin is left alone.

  Nothing is compiled here. Run it once before the e2e (vendor/ is gitignored):
    powershell -NoProfile -File tools\fetch-whisper.ps1 -Out vendor\whisper

.PARAMETER Out
  The folder to fill, for example vendor\whisper.
#>
param(
  [Parameter(Mandatory = $true)]
  [string]$Out
)

$ErrorActionPreference = 'Stop'

# These values MUST say what ENGINE in crates/flare-dictation/src/catalog.rs says. The test
# crates/flare-dictation/tests/fetch_whisper.rs reads this file as text and fails when they differ.
$Tag = 'b5130'
$Asset = 'whisper-bin-x64.zip'
$Sha256 = 'f9ec6c52a2e949b62ab51fa21d0d497958f9e41c3010c157c4e42932d5316f3c'
$Bytes = 8573270
$Url = "https://github.com/ggml-org/whisper.cpp/releases/download/$Tag/$Asset"
$Files = @(
  'whisper-server.exe',
  'whisper.dll',
  'ggml.dll',
  'ggml-base.dll',
  'ggml-cpu-alderlake.dll',
  'ggml-cpu-cannonlake.dll',
  'ggml-cpu-cascadelake.dll',
  'ggml-cpu-haswell.dll',
  'ggml-cpu-icelake.dll',
  'ggml-cpu-sandybridge.dll',
  'ggml-cpu-skylakex.dll',
  'ggml-cpu-sse42.dll',
  'ggml-cpu-x64.dll',
  'parakeet-cli.exe',
  'parakeet.dll'
)
# Microsoft's C++ runtime: every engine file imports it, and no official zip carries it.
$Runtime = @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'vcomp140.dll')
# Where the zip keeps the files. They land flattened in -Out.
$ZipDir = 'Release'

if ([Net.ServicePointManager]::SecurityProtocol -notmatch 'Tls12') {
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}

$OutDir = [IO.Path]::GetFullPath($Out)
$stamp = Join-Path $OutDir '.source'
$license = Join-Path $OutDir 'LICENSE-whisper.cpp.txt'

# Every file, the runtime and the notice, not only the server: the stamp is the zip's checksum, which
# a changed file list does not change.
$missing = @($Files + $Runtime | Where-Object { -not (Test-Path -LiteralPath (Join-Path $OutDir $_)) })
$stamped = (Test-Path -LiteralPath $stamp) -and ((Get-Content -LiteralPath $stamp -Raw).Trim() -eq $Sha256)
if ($missing.Count -eq 0 -and $stamped -and (Test-Path -LiteralPath $license)) {
  Write-Host "whisper: $OutDir already holds $Asset of $Tag"
  return
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('flare-whisper-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  $zip = Join-Path $tmp $Asset
  $mb = [math]::Round($Bytes / 1e6, 1)
  Write-Host "whisper: downloading $Asset of $Tag ($mb MB)"
  Invoke-WebRequest -Uri $Url -OutFile $zip -UseBasicParsing

  # Checked BEFORE anything is unpacked or copied.
  $size = (Get-Item -LiteralPath $zip).Length
  if ($size -ne $Bytes) {
    throw "$Asset is $size bytes, expected $Bytes (a cut-off download?)"
  }
  $got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($got -ne $Sha256) {
    throw "SHA-256 mismatch, refusing to unpack`n  expected $Sha256`n  got      $got"
  }

  # Windows' own tar, by full path (bsdtar, reads zip). A bare `tar` can be GNU tar on PATH, which
  # cannot read a zip.
  $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
  if (-not (Test-Path -LiteralPath $tar)) {
    throw "$tar is missing; it unpacks the zip (Windows 10 1803 or later)"
  }
  $tree = Join-Path $tmp 'x'
  New-Item -ItemType Directory -Path $tree | Out-Null
  & $tar -xf $zip -C $tree
  if ($LASTEXITCODE -ne 0) { throw "tar could not unpack $Asset" }

  $release = Join-Path $tree $ZipDir
  foreach ($name in $Files) {
    if (-not (Test-Path -LiteralPath (Join-Path $release $name))) {
      throw "$ZipDir/$name is missing from the archive"
    }
  }

  New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
  # Only what a previous pin could have left: an engine file no longer in the list would still be
  # found, and loaded, by ggml. The folder itself is never removed.
  Get-ChildItem -LiteralPath $OutDir -File |
    Where-Object { $_.Name -match '^(whisper|ggml|parakeet)[\w.-]*\.(exe|dll)$' } |
    Remove-Item -Force
  foreach ($name in $Files) {
    Copy-Item -LiteralPath (Join-Path $release $name) -Destination $OutDir -Force
  }

  # The runtime travels with the engine, copied only when Windows vouches for it.
  $sys = Join-Path $env:SystemRoot 'System32'
  foreach ($name in $Runtime) {
    $from = Join-Path $sys $name
    if (-not (Test-Path -LiteralPath $from)) {
      throw "$from is missing: install the Visual C++ 2015-2022 x64 redistributable on the build machine"
    }
    $sig = Get-AuthenticodeSignature -LiteralPath $from
    if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
      throw "$from is not validly signed by Microsoft (status $($sig.Status))"
    }
    Copy-Item -LiteralPath $from -Destination $OutDir -Force
  }

  # whisper.cpp is MIT: its notice travels with every copy. Taken from the zip when it has one;
  # otherwise from the same tag's LICENSE (the official zip carries none, as of b5130).
  $licenseSrc = Get-ChildItem -LiteralPath $tree -Recurse -File -Filter 'LICENSE*' | Select-Object -First 1
  if ($licenseSrc) {
    Copy-Item -LiteralPath $licenseSrc.FullName -Destination $license -Force
  } else {
    Invoke-WebRequest -Uri "https://raw.githubusercontent.com/ggml-org/whisper.cpp/$Tag/LICENSE" `
      -OutFile $license -UseBasicParsing
  }

  # Last, so the stamp only ever vouches for a complete folder.
  Set-Content -LiteralPath $stamp -Value $Sha256 -Encoding ascii
  Write-Host "whisper: unpacked $($Files.Count + $Runtime.Count + 1) files into $OutDir"
}
finally {
  Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
