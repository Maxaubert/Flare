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

# One SHA-256 per engine file (audit L64, 2026-10-20): taken from the files of the verified zip. A folder is
# trusted only when every file hashes to its pin, never by the presence of a name or a stamp.
$FileHashes = @{
  'whisper-server.exe'       = '74c13bcb83b944412b971886d37893611fb31846780c3a78db3847b3132306bc'
  'whisper.dll'              = 'b7bb4ba92bd36b8afe0c00059ca6a0f69c6767193bfb5f0761e9ed27b1087803'
  'ggml.dll'                 = 'c6e88687d6aa0238f2e834a87958f38d2dc96075a6a7bf6fb8b9cd728a0faed2'
  'ggml-base.dll'            = 'a5241b52206f61c9dfd6f0f53a8ef19076f9528ead2db1e039a66d7e03817bfc'
  'ggml-cpu-alderlake.dll'   = '5a5b11dcd38e321b13f85c95414940db9eab1132be3da6342f03dfb1d8e51bd5'
  'ggml-cpu-cannonlake.dll'  = '2c858781450b52eda95c381232cc65c9e19cbf621cc7b254d8c44fdbab77791e'
  'ggml-cpu-cascadelake.dll' = 'ddf49bb749b34800afcb3d6224544966a05c5d00f1d0b6565bee9f3b010dd53c'
  'ggml-cpu-haswell.dll'     = '6b772e094b8976e22b4c043be86a1a8deda4b50511dc803a693b652c51b24944'
  'ggml-cpu-icelake.dll'     = 'bbd87ea5920edc401054848071ad2ef8c96bbf8007c4e4cd0ea82ba9b9e3bd10'
  'ggml-cpu-sandybridge.dll' = 'de2ad5f84c7dfa557d515b3678d6452ce0216590268b2ca79cc537fe87cf239d'
  'ggml-cpu-skylakex.dll'    = '9fa3f9d984ca42d568181dd345072299db2978b328800343218c2c7f000b7152'
  'ggml-cpu-sse42.dll'       = '740fc769ef433985dfdb24a609a42d5acf188abda52287a4d89b89b567507c19'
  'ggml-cpu-x64.dll'         = '43ccf32b9b70aa4c47d0a12ac2ed3c241e0045571acdad565ecd9d1f99ffc08b'
  'parakeet-cli.exe'         = '8de0c0805272b17d779aa2a0a12d4e6e99b076d3843e12da54ee5ae7fe00c829'
  'parakeet.dll'             = 'fac30f11a9ed424e7897ed02c7186377ff7201bc9bc59bc5e025c84d296cd367'
}
# The notice is the repository's own copy, not a download from a tag.
$LicenseSrc = Join-Path $PSScriptRoot '..\LICENSE-whisper.cpp.txt'
if ([Net.ServicePointManager]::SecurityProtocol -notmatch 'Tls12') {
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}

$OutDir = [IO.Path]::GetFullPath($Out)
$stamp = Join-Path $OutDir '.source'
$license = Join-Path $OutDir 'LICENSE-whisper.cpp.txt'

# Every engine file is hashed against its pin (a swapped or truncated file fails, a present name does
# not pass), and the runtime and the notice must exist. The stamp alone proves nothing.
$missing = @($Files + $Runtime | Where-Object { -not (Test-Path -LiteralPath (Join-Path $OutDir $_)) })
$wrong = @($Files | Where-Object {
  (Test-Path -LiteralPath (Join-Path $OutDir $_)) -and
  ((Get-FileHash -LiteralPath (Join-Path $OutDir $_) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $FileHashes[$_])
})
if ($missing.Count -eq 0 -and $wrong.Count -eq 0 -and (Test-Path -LiteralPath $license)) {
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
    $have = (Get-FileHash -LiteralPath (Join-Path $release $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($have -ne $FileHashes[$name]) { throw "$name in the archive hashes to $have, expected $($FileHashes[$name])" }
  }
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

  # whisper.cpp is MIT: its notice travels with every copy. It is the repository's committed copy
  # (the official zip carries none, as of b5130), so nothing mutable is downloaded.
  if (-not (Test-Path -LiteralPath $LicenseSrc)) { throw "$LicenseSrc is missing from the repository" }
  Copy-Item -LiteralPath $LicenseSrc -Destination $license -Force

  # Last, so the stamp only ever vouches for a complete folder.
  Set-Content -LiteralPath $stamp -Value $Sha256 -Encoding ascii
  Write-Host "whisper: unpacked $($Files.Count + $Runtime.Count + 1) files into $OutDir"
}
finally {
  Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
