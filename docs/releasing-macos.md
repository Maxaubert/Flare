# Releasing Flare for macOS

How the Mac app is built, signed, notarized, packed for Velopack updates and published (port phase 8, regression
rules 1036 to 1048). Windows releases are in `docs/shipping.md` and do not change for any of this.

## What a Mac release is

For Apple Silicon, macOS 15 or later, channel `osx` of the same GitHub release `v<version>` the Windows build is in:

| File | What it is |
| --- | --- |
| `Flare-<version>-osx-full.nupkg` | The update package: the whole `Flare.app`, signed and notarized, with Velopack's `UpdateMac` and `sq.version` |
| `Flare-<version>-osx-delta.nupkg` | The delta from the previous Mac release (when there is one) |
| `releases.osx.json`, `assets.osx.json`, `RELEASES-osx` | The feed Velopack reads (`releases.win.json` beside it is Windows') |
| `Flare-osx-Portable.zip` | The same app, zipped with `ditto` (unzip and drag to Applications) |
| `Flare-<version>-arm64.dmg` | The installer: the app and an `/Applications` link, signed, notarized, stapled |

An update swaps the whole bundle: Velopack's `UpdateMac` unpacks the new `Flare.app` beside the old one and exchanges
the two in one call (`renamex_np`, as Sparkle does), so every signature in the app is the new release's own and
nothing is patched in place (rules 1046 and 1047). The packages wait in `~/Library/Caches/velopack/Flare/packages`.
The updater inside the app stays compiled out until the repository is public, exactly as on Windows
(`docs/updater-flip.md`; `vars.FLARE_LIVE_UPDATES`).

## The scripts

All under `tools/release/mac/`, all writing under `target/` only:

- `make-app.sh`: builds the release programs and `Flare.app` (`flare-mac-app`: the CEF framework, the five helpers,
  `flare-mcp`, the icon, the plugin), signed inside out by role (rules 1036 to 1042). Alone, with notary credentials, it
  also notarizes the app and makes a DMG; a release runs it with `FLARE_NOTARIZE=0`, because the app it makes is not
  the one that ships.
- `pack.sh <Flare.app> <version> <Releases dir> [notes.md]`: `vpk pack` for osx-arm64. vpk adds `UpdateMac` and
  `sq.version`, and its `codesign` is `flare-mac-app` (a link named `codesign` first on vpk's `PATH`), which signs the
  whole bundle again by role at that moment, so `UpdateMac` gets no entitlements instead of Velopack's broad ones
  (rule 1046). vpk then notarizes and staples that bundle and makes the packages. The script checks the bundle inside
  the update package (unpacked the way Velopack's updater unpacks it) and inside the portable zip:
  `codesign --verify --deep --strict`, `flare-mac-app verify` (team, Developer ID Application signer, hardened runtime,
  timestamp, each program's keys), `stapler validate`, `spctl` "source=Notarized Developer ID". Then the DMG, from
  that same bundle.
- `update-e2e.sh`: the update proved on this Mac, headless (rule 1047). Packs the throwaway versions 0.0.1 and 0.0.2,
  installs 0.0.1 from its portable zip into a temp folder (never `/Applications`), updates it with the real
  `VelopackFeed` and `UpdateMac` with no restart, and checks the swapped bundle; nothing is opened, no window
  appears (the window server is sampled the whole time).
- `ci-keychain.sh`: CI only, the secrets into a throwaway keychain.
- `lib.sh`: notarization and the DMG, shared.

## Building on a Mac

Once: Xcode 26 or later (the macOS 26 SDK; the ci macos job builds on it), the Rust toolchain of
`rust-toolchain.toml`, `brew install ninja cmake zstd`, and vpk at the version the workflows pin (`VPK_VERSION`,
1.2.161), which needs the .NET 8 SDK:

```sh
curl -sSL https://dot.net/v1/dotnet-install.sh | bash -s -- --channel 8.0 --install-dir "$HOME/.dotnet"
export DOTNET_ROOT="$HOME/.dotnet" PATH="$HOME/.dotnet:$HOME/.dotnet/tools:$PATH"
dotnet tool install -g vpk --version 1.2.161
```

Ad hoc (runs on this Mac only, cannot be notarized; what the tests and `update-e2e.sh` use by default):

```sh
tools/release/mac/make-app.sh                          # target/mac-release/Flare.app
FLARE_E2E_BUILD=1 tools/release/mac/update-e2e.sh      # the update, headless
```

### Signed and notarized, by Max

Two things first, both once, both Max's own (a prompt or a password on the screen is his to answer):

1. The Developer ID key must let `codesign` in without a prompt. Today its partition list is only `apple-tool:`, so
   every Developer ID `codesign` puts a keychain prompt on the screen, and `flare-mac-app check-key` refuses before
   anything is signed (rule 1040). Fix it with (asks for the login keychain's password in the terminal):

   ```sh
   security set-key-partition-list -S apple-tool:,apple: -s ~/Library/Keychains/login.keychain-db
   ```

2. The notarytool profile `notary` (this Mac has one since 2026-11-14). It is shared by every app Max notarizes on
   this Mac (Flare, Sonara, Universal Clipboard) and holds the app-specific password "notarytool (all apps)"; the
   old `flare-notary` profile is dead (its password was revoked). To make it again:
   `xcrun notarytool store-credentials notary --apple-id <Apple ID> --team-id WV7BY8CUM8` (asks for an
   app-specific password from account.apple.com).

Then the one command that proves a signed, notarized release updates itself (two notarizations, about 10 to 30
minutes; Gatekeeper must say "Notarized Developer ID" for the bundle before and after the update):

```sh
FLARE_MAC_SIGN_IDENTITY="Developer ID Application: MAX AUBERT (WV7BY8CUM8)" FLARE_NOTARY_PROFILE=notary \
  FLARE_E2E_BUILD=1 tools/release/mac/update-e2e.sh
```

A real release by hand, the same way CI does it:

```sh
export FLARE_MAC_SIGN_IDENTITY="Developer ID Application: MAX AUBERT (WV7BY8CUM8)" FLARE_NOTARY_PROFILE=notary
FLARE_NOTARIZE=0 tools/release/mac/make-app.sh
tools/release/mac/pack.sh target/mac-release/Flare.app <version> Releases notes.md
```

## CI: `.github/workflows/macos-app.yml`

- **After every release run** (`workflow_run` of `release`, success, on `main`): when the release `v<version>`
  exists without `releases.osx.json` and the five secrets below are set, it builds the commit `v<version>` is tagged
  at (never the newer commit that ended the release run: a merge without a version bump also ends it with success),
  signs, packs, notarizes, staples and checks, then adds the Mac files to that release (`vpk upload github --channel
  osx --merge`, and the DMG). One run per version at a time (`concurrency: macos-app-<version>`), and the release is
  read again just before the upload, so two quick merges cannot both publish (rule 1049). Without the secrets it
  writes a notice and stops; with nothing to add (no new release, or the Mac build already there) too. It is a
  workflow of its own that starts after the Windows one has finished, so it can never fail or hold up a Windows
  release, and `release.yml` is unchanged (rule 1048).
- **By hand** (Actions, macos-app, Run workflow): the same build kept as a workflow artifact for 30 days, never
  published. Signed and notarized only when run from `main`; from any other branch the job gets no environment and
  so no secret, and the build is ad hoc (rule 1049).
- The release notes are the Windows release's own text (`gh release view v<version>`), so both say the same.
- The runner is `macos-26` (Xcode 26). Flare needs no newer SDK: the ci macos job builds and links the whole workspace
  there (run 42533159787, 2026-11-08, image macos-26-arm64), every Flare program is built for macOS 11 or later and
  CEF's framework for 13, all under the app's `LSMinimumSystemVersion` 15.0. If a later change needs an API newer than
  that SDK, the link fails on the runner, loudly; move the runner up then.

### The secrets, and how Max makes them

The secrets live in a GitHub **environment** named `mac-release` that only `main` may use, never as repository
secrets: a repository secret reaches a run by hand from any branch, and a branch can change the workflow file it runs
(rule 1049). Once:

1. Settings > Environments > New environment, name `mac-release`.
2. Deployment branches and tags: Selected branches and tags, add the rule `main` (no other).
3. Add the five secrets under that environment's Environment secrets, or
   `gh secret set <NAME> --env mac-release --repo Maxaubert/Flare`.
4. If any of the five were set as repository secrets before, delete them there
   (`gh secret delete <NAME> --repo Maxaubert/Flare`).

None of them is ever printed, written into the repository or kept on the runner (rules 1039 and 1050).

| Secret | What |
| --- | --- |
| `MACOS_CERT_P12_BASE64` | The "Developer ID Application: MAX AUBERT (WV7BY8CUM8)" certificate with its private key, as a `.p12`, base64 |
| `MACOS_CERT_PASSWORD` | The password the `.p12` was exported with |
| `APPLE_API_KEY_P8_BASE64` | An App Store Connect API key (`AuthKey_<KEYID>.p8`), base64 |
| `APPLE_API_KEY_ID` | That key's id (10 characters) |
| `APPLE_API_ISSUER_ID` | The issuer id (a UUID, shown above the key list) |

The certificate, as a `.p12`:

1. Open Keychain Access, the login keychain, My Certificates.
2. Right-click "Developer ID Application: MAX AUBERT (WV7BY8CUM8)" (the arrow beside it shows the private key; the
   certificate alone is not enough), Export, format Personal Information Exchange (.p12), a strong password.
3. `base64 -i Flare-DeveloperID.p12 | gh secret set MACOS_CERT_P12_BASE64 --env mac-release --repo Maxaubert/Flare`,
   then `gh secret set MACOS_CERT_PASSWORD --env mac-release --repo Maxaubert/Flare` (it asks for the value), then
   delete the `.p12` for good, never to the Bin (it is the signing key, and the Bin keeps it):
   `rm -P Flare-DeveloperID.p12`. Export it outside any git repository; `.gitignore` refuses `*.p12`, `*.p8` and
   `AuthKey_*` anyway.

The App Store Connect API key (what notarization signs in with in CI):

1. App Store Connect, Users and Access, Integrations, App Store Connect API, Team Keys, the + button; any name, access
   Developer.
2. Download `AuthKey_<KEYID>.p8` (Apple lets you download it once). The key id is in the list, the issuer id above it.
3. `base64 -i AuthKey_<KEYID>.p8 | gh secret set APPLE_API_KEY_P8_BASE64 --env mac-release --repo Maxaubert/Flare`,
   then `gh secret set APPLE_API_KEY_ID` and `gh secret set APPLE_API_ISSUER_ID` the same way (with `--env
   mac-release`). Then delete the `.p8` for good (`rm -P AuthKey_<KEYID>.p8`, never to the Bin) once the secrets are set;
   a new one can always be made.

In the job, the programs are built first; only then does `ci-keychain.sh` put the certificate in a keychain with a
random password that only `codesign` may use and store the API key as the notarytool profile `flare-ci` in that same
keychain (vpk notarizes with a profile only). The `.p12` and the `.p8` are deleted when that script ends, however it
ends; the keychain locks itself after the job's 150 minutes without use, and the step after the packing deletes it
whatever happened before, so no later action (the artifact, the upload, the cache) runs while a key is on the runner
(rule 1050). Notarization never takes a password on the command line: a keychain profile or an API key only.

## When something fails

- `flare-mac-app check-key` refuses: the key would prompt; run the partition-list command above (rule 1040).
- `flare-mac-app as vpk's codesign: ...`: vpk called `codesign` with arguments the shim does not know (a new vpk), or
  with another identity than the script set. Read `crates/flare-cef/src/macos/vpk.rs` before changing `VPK_VERSION`.
- Notarization: `target/mac-release/notary-*.json` (the DMG) or vpk's output (the app) has the submission id;
  `xcrun notarytool log <id> --keychain-profile notary` says why.
- `the update package's Flare.app` fails `codesign --verify`: something changed the bundle after vpk signed it; the
  packages are not to be shipped.
