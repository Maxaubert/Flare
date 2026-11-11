# Shipping Flare

How Flare is packed, released, installed and removed. Written from a real local pack (2026-10-18, vpk 1.2.161,
Task 29 of the M9 plan). Anything not seen with the eyes says so.

## How a release happens

1. A PR bumps `version` under `[workspace.package]` in `Cargo.toml` (one place; every crate follows it) and is
   merged to `main` (the owner says yes in chat first).
2. `.github/workflows/release.yml` runs on every push to `main`. It reads the version, and when `v<version>`
   already exists as a release it publishes nothing and says so (one release per version; it does not fail).
3. A fresh version goes through the gates in this order: no em-dashes, `cargo fmt --check`, `cargo clippy
   --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`, `cargo deny check`. Then the
   release build, the release notes (GitHub's generated notes through `tools/release/notes.ps1`), the download of
   the previous release (so the pack can make a delta), `tools/release/pack.ps1`, `tools/release/verify-pack.ps1`,
   the side-by-side run (Flare installed for real beside Prism Terminal on the clean runner), and the publish
   (`vpk upload github --publish`, tag `v<version>`).
4. The updater inside the app is compiled out until the repository is public: `docs/updater-flip.md`.
5. The Mac build is added to the same release afterwards by `.github/workflows/macos-app.yml`, when its signing
   secrets are set: `docs/releasing-macos.md`.

## The secret the side-by-side run needs (2026-11-11)

The side-by-side run downloads the latest Prism Terminal installer with `gh release download`. Since 2026-11-11 the
Prism Terminal repository is `arkivert/PrismTerminal` and it is private, so the workflow's own `GITHUB_TOKEN` cannot
read it ("release not found"). Owner decision 2026-11-11: a repository secret holds a token that can.

- Name: `PRISM_RELEASE_TOKEN` (the Prism Terminal release token), a repository secret of this repository
  (Settings > Secrets and variables > Actions).
- Value: a fine-grained personal access token, resource owner `arkivert`, repository access only
  `arkivert/PrismTerminal` (the Prism Terminal repository), permission Contents: Read-only. Nothing else.
- Used by: the side-by-side step of `release.yml` and of `installer-e2e.yml`, and only that step (its `GH_TOKEN`).
  No other step gets it, and nothing prints it.
- Missing: that step fails (it does not skip) with one line, and a release then publishes nothing:
  `PRISM_RELEASE_TOKEN is not set: add a fine-grained read-only token for arkivert/PrismTerminal as a repository secret (docs/shipping.md)` (Prism Terminal).
- Dependabot pull requests get only Dependabot secrets, so an `installer-e2e` run on one fails the same way unless
  the same token is also added as a Dependabot secret of the same name (the Prism Terminal token).
- When the token expires, make a new one with the same settings and replace the secret's value.
- Held by regression rule 1127 and `only_the_side_by_side_step_gets_the_neighbour_token_and_it_fails_without_it`
  (`crates/flare/tests/release_files.rs`).

## Packing locally (installs nothing)

```
dotnet tool install --tool-path <scratch>\vpk vpk --version 1.2.161
$env:PATH = "<scratch>\vpk;$env:PATH"
cargo build --release -p flare --target-dir <TARGET>
pwsh tools\release\pack.ps1 -Version 0.0.1 -Exe <TARGET>\release\flare.exe -Notes <notes.md> -Out <temp>\Releases
pwsh tools\release\pack.ps1 -Version 0.0.2 -Exe <TARGET>\release\flare.exe -Notes <notes.md> -Out <temp>\Releases
pwsh tools\release\verify-pack.ps1 -Out <temp>\Releases -Version 0.0.2
```

The second pack into the same `Releases` folder is what makes a delta. The packed files are never committed.

## The vpk flags (decision D8)

| Flag | Why |
|---|---|
| `--packId Flare` | The install folder name and the update key. |
| `--packTitle Flare` | Names the shortcut and the Apps and Features entry. |
| `--packAuthors Max` | The publisher shown in Apps and Features. |
| `--packVersion <x.y.z>` | The version from `Cargo.toml`. |
| `--packDir <stage>` | The staging folder `pack.ps1` fills; it fails when anything shipped beside the exe is missing. |
| `--mainExe Flare.exe` | The app the shortcut and the updater start. |
| `--icon build\icon.ico` | The icon on `Update.exe`, `Setup.exe` and the stub. |
| `--shortcuts StartMenuRoot` | A Start menu shortcut only; no desktop shortcut. |
| `--aumid com.flare.terminal` | The taskbar identity, the same one the app sets itself. |
| `--runtime win-x64` | x64 only. |
| `--channel win` | The default channel; an installed app reads only `releases.win.json`. |
| `--delta BestSpeed` | Makes the delta package against the previous full package in the folder. |
| `--releaseNotes <file>` | The notes the update window shows (`NotesMarkdown`). |
| `--outputDir <Releases>` | Where the files land. |

No splash image (one-click setup), unsigned, no MSI.

## What the installer does

`Flare-win-Setup.exe` is one-click: no questions, no licence page, no folder choice. It installs per user to
`%LOCALAPPDATA%\Flare` (no elevation), creates a Start menu shortcut only, adds the Apps and Features entry and
starts Flare. Uninstalling removes the app folder, the shortcut and the entry. It keeps `%APPDATA%\Flare`
(settings, logs) and `%LOCALAPPDATA%\FlareDictation` (the dictation models): user data stays.

## Hooks (decision D10)

`VelopackApp::build()` runs first in `main`, with `set_auto_apply_on_startup(false)`. One real hook,
`on_before_uninstall_fast_callback`, removes both Explorer verb keys. The after-install and after-update hooks do
nothing: the verb is reconciled at the next launch. `on_first_run` only stores the version; the diagnostics log
writes the `first-run` crumb from it. Under `--e2e` the uninstall hook touches only a sandbox root under
`Software\FlareE2E\` and does nothing otherwise.

## The Explorer verb

- Windows 11 shows the classic verb under Show more options (Shift+F10 opens that menu directly).
- The label is `Open in Flare`. Two keys, both per user under `HKCU\Software\Classes`:
  `Directory\shell\Flare` (the folder you right-click, `"%1"`) and `Directory\Background\shell\Flare` (the folder
  you are inside, `"%V"`). Each has a `command` subkey that runs the installed exe.
- The app writes them through the registry API on launch when it is an installed (packaged) copy, relabels a stale
  one, and writes nothing when the owner has switched the row off in Settings (the one stored fact is the NO).
- The Velopack uninstall hook deletes both keys. A portable copy never writes them.

## Installing a local build with Build ready (2026-10-29, one click since 2026-11-10)

For Max's own copies (`%LOCALAPPDATA%\StableFlare\current\FlareStable.exe` on `--user-data-dir=%APPDATA%\FlareStable`,
and `%LOCALAPPDATA%\Flare\current\Flare.exe`). It replaces closing Stable by hand to copy a build in. Ask Max first:
"Stage the build for <copy>? It installs when you click Build ready."

```
cargo build --release -p flare -p flare-mcp -p flare-dictation --bin flare --bin flare-mcp --bin flare-parakeet-host
pwsh tools\stage-install.ps1 -Stable          # or no switch for the plain Flare copy
pwsh tools\stage-install.ps1 -Stable -Bin <TARGET>\release -Repo <checkout>
```

- The script writes only `<install>\staged\` (`staged.json` last); the running copy shows "Build ready" and installs only
  when Max clicks it (with an agent at work the close question asks first). Nothing is closed by force; the restart uses the normal end of
  life, so the sessions resume.
- `-Root <folder> -ExeName <name>` stages into any folder with a `current\` (the tests use a temp one).
- A commit with uncommitted changes is named `<sha>-dirty`.
- After an install `<install>\current\` holds `<name>.old` for 20 s (or until the next start, if in use), and
  `<install>\install.json` until the new build is good. A build that dies early twice is rolled back at the third start.
  Only files that differ are swapped. A second stage installs only once the first build is confirmed (20 s up).
  `install.json` in state `swapping` means a swap was cut off; the next start puts it back.
- The Velopack path (public releases) is unchanged.

## SmartScreen

Flare is unsigned. Windows SmartScreen may warn on the first run of `Flare-win-Setup.exe` ("More info", then
"Run anyway"). Signing is not set up.

## Delta updates

A delta is made by `vpk pack` when the previous release's full package is in the output folder. The workflow gets
it with `vpk download github` before the pack (the step may find nothing on the first release). The feed then
lists a `Delta` asset beside the `Full` one and an installed copy downloads the smaller file.

## The updater switch

The update check and install are built and tested but compiled out of the shipped binary by the `live-updates`
cargo feature until the repository is public. The prepared change is in `docs/updater-flip.md`.

## Recorded facts (2026-10-18, vpk 1.2.161)

Packed with `tools/release/pack.ps1` from the release build of this branch, version 0.0.1 then 0.0.2 into one
`Releases` folder. The exe was the same build both times, so the delta below is a floor, not a typical size.

**Output files and sizes** (what `Releases` held):

| File | Bytes |
|---|---|
| `Flare-win-Setup.exe` | 24,800,965 |
| `Flare-win-Portable.zip` | 17,174,711 |
| `Flare-0.0.1-full.nupkg` | 17,230,541 |
| `Flare-0.0.2-full.nupkg` | 17,230,533 |
| `Flare-0.0.2-delta.nupkg` | 59,060 |
| `releases.win.json` | 1,058 |
| `assets.win.json` | 249 |
| `RELEASES` | 148 |

The package names have no `win` part: `Flare-<ver>-full.nupkg` and `Flare-<ver>-delta.nupkg` (not
`Flare-<ver>-win-full.nupkg`). Setup and portable names do: `Flare-win-Setup.exe`, `Flare-win-Portable.zip`.
The delta is 0.34 percent of the full package (59,060 of 17,230,533). The Setup is larger than the full package
(it embeds it plus the installer). The stub `Flare.exe` is 523,776 bytes and `Update.exe` 5,264,896 bytes.

**`releases.win.json`:** `{"Assets":[ ... ]}`, newest first, every asset has `PackageId`, `Version`, `Type`
(`Full` or `Delta`), `FileName`, `SHA1`, `SHA256`, `Size`, `NotesMarkdown`, `NotesHTML`. Real values:

```
{"PackageId":"Flare","Version":"0.0.2","Type":"Full","FileName":"Flare-0.0.2-full.nupkg","SHA1":"3E80...","SHA256":"9FB7...","Size":17230533,"NotesMarkdown":"- Add a thing\n- Fix a thing","NotesHTML":"<ul>\n<li>Add a thing</li>\n<li>Fix a thing</li>\n</ul>"}
{"PackageId":"Flare","Version":"0.0.2","Type":"Delta","FileName":"Flare-0.0.2-delta.nupkg","Size":59060, ...same notes...}
{"PackageId":"Flare","Version":"0.0.1","Type":"Full","FileName":"Flare-0.0.1-full.nupkg","Size":17230541, ...}
```

`NotesMarkdown` is the notes file with line ends normalised to 
 and the trailing newline trimmed. `assets.win.json` lists the
upload set (`Delta`, `Installer`, `Full`, `Portable`; not the older full package). `RELEASES` is the legacy
SHA1 list (the two full packages).

**`Flare-win-Portable.zip` unpacks to** `.portable` (0 bytes), `Update.exe`, `Flare.exe` (the 523,776-byte stub)
and `current\` holding `Flare.exe` (the real app), `sq.version` (the nuspec of the packed version), `conpty.dll`,
`OpenConsole.exe`, `claude-plugin\`, `LICENSE`, `THIRD-PARTY-NOTICES.md`. It holds the latest version only
(0.0.2). **This confirms D9:** the `.portable` marker is there, so `is_packaged` says no for a portable copy.

**Icons and version resource:** the stub `Flare.exe`, `Update.exe` and `Flare-win-Setup.exe` each have one icon
(`ExtractIconExW` count 1). `current\Flare.exe` carries the version resource (ProductName Flare, FileDescription
Flare, CompanyName Max, OriginalFilename Flare.exe). Its version comes from `Cargo.toml` (0.1.0 here), not from
`--packVersion`; the installed version Velopack reports comes from `sq.version`. The stub also carries a version
resource (Flare, 0.1.0).

**Packages folder:** `velopack::locator::create_config_from_root_dir(root)` sets `PackagesDir = <root>\packages`,
`ManifestPath = <root>\current\sq.version`, `UpdateExePath = <root>\Update.exe`. This confirms Task 11's
`<root>\packages`. Whether Setup leaves the installed version's full `.nupkg` in that folder was NOT checked:
running Setup would install the app, which this task does not do. It is checked in the update e2e job (Task 38),
which installs on a clean runner. Until then, do not rely on a delta base being present on an installed copy.

**Silent uninstall:** `Update.exe uninstall --silent` (a subcommand, not `--uninstall`; from `Update.exe --help`
run on the unzipped portable copy: `Update.exe uninstall [OPTIONS]` with the global option `-s, --silent`).
Other subcommands: `apply`, `start`, `patch`. Global options: `--verbose`, `--rootDir`, `--packageDir`, `--log`.

**Portable run:** the unzipped `current\Flare.exe --probe-backend` exited 0 in about 1.1 seconds and printed
`backend: gpu`, so `VelopackApp::run` is harmless in that layout. No window opened.

**Packed feed test:** `crates/flare-update/tests/packed_feed.rs` (`#[ignore]`; set `FLARE_PACKED_DIR` to a
`Releases` folder holding 0.0.1 and 0.0.2) runs the real `VelopackFeed` over a `FileSource`: `check()` finds 0.0.2
from a layout that says 0.0.1, the notes equal `releases.win.json`, `download("0.0.3")` is `Failed("not offered")`,
`download("0.0.2")` reports rising progress and lands `Flare-0.0.2-full.nupkg` in `<root>\packages`, and `discard`
removes it. `apply` is not called. It passed against this pack.
