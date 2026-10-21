# Fetches the pinned ConPTY build from NuGet, checks every hash, copies conpty.dll and OpenConsole.exe.
param([string]$Out = "$PSScriptRoot\..\crates\flare-term\bundled\x64")
$ErrorActionPreference = 'Stop'
$ver = '1.25.260303002-preview'
$url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$ver/microsoft.windows.console.conpty.$ver.nupkg"
$want = @{
  nupkg = '7e477694e4095717c29725f7191ef13a5dda125e3a4d28a5fcc765a13dc9d967'
  dll   = '3319b484b80bb53d1f4d0a9eb0ea60fd0f61da69db7280ca43b84215f19245ff'
  exe   = '7f68c840226505004215c0b82d4e502c24b5bc3f4b93c4baaaa19bd679c0def8'
}
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("flare-conpty-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  $pkg = Join-Path $tmp 'conpty.zip'
  Invoke-WebRequest -Uri $url -OutFile $pkg
  if ((Get-FileHash $pkg -Algorithm SHA256).Hash.ToLower() -ne $want.nupkg) { throw 'nupkg hash differs' } else { 'OK nupkg' }
  Expand-Archive -Path $pkg -DestinationPath (Join-Path $tmp 'x')
  $dll = Join-Path $tmp 'x\runtimes\win-x64\native\conpty.dll'
  $exe = Join-Path $tmp 'x\build\native\runtimes\x64\OpenConsole.exe'
  if ((Get-FileHash $dll -Algorithm SHA256).Hash.ToLower() -ne $want.dll) { throw 'conpty.dll hash differs' } else { 'OK conpty.dll' }
  if ((Get-FileHash $exe -Algorithm SHA256).Hash.ToLower() -ne $want.exe) { throw 'OpenConsole.exe hash differs' } else { 'OK OpenConsole.exe' }
  New-Item -ItemType Directory -Force -Path $Out | Out-Null
  Copy-Item $dll, $exe -Destination $Out -Force
} finally { Remove-Item -Recurse -Force $tmp }
