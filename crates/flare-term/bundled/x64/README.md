# Bundled ConPTY (x64)

- Source: NuGet package `Microsoft.Windows.Console.ConPTY`, version `1.25.260303002-preview`
  (`https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/1.25.260303002-preview/microsoft.windows.console.conpty.1.25.260303002-preview.nupkg`).
- Files and SHA-256:
  - `conpty.dll` (from `runtimes/win-x64/native/`): `3319b484b80bb53d1f4d0a9eb0ea60fd0f61da69db7280ca43b84215f19245ff`
  - `OpenConsole.exe` (from `build/native/runtimes/x64/`): `7f68c840226505004215c0b82d4e502c24b5bc3f4b93c4baaaa19bd679c0def8`
  - package `.nupkg`: `7e477694e4095717c29725f7191ef13a5dda125e3a4d28a5fcc765a13dc9d967`
- Licence: MIT, (c) Microsoft Corporation. Project: `https://github.com/microsoft/terminal`.
- Same build that Prism Terminal's node-pty ships.

Do not edit these files; run `tools/fetch-conpty.ps1`. The test `bundled_files_match_the_pinned_hashes`
fails if they differ.
