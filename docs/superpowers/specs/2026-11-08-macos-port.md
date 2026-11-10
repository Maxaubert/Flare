# Flare on macOS: spec and plan (2026-11-08)

Status: approved by Max 2026-11-08: build every phase that does not need the Developer ID (signing, notarization, notifications, mic entitlement, signed updates wait); merge PRs when Windows and macOS CI are green, without asking per PR (Max away). Audit behind it (5 agents plus a trial
`cargo check` on macOS 27): `~/Documents/Claude/research/flare/2026-11-08-flare-macos-port-audit.md`.

## Decisions by Max (2026-11-08)

| Question | Decision |
|---|---|
| Where | Same repo, small short-lived PRs, merged as they go. No Mac branch, no temp repo. Windows work continues on main in parallel. |
| Scope | Full parity with Windows before the Mac build counts as done. Phases still land one by one. |
| Look | Mac-native chrome: traffic lights, the Mac menu bar, Cmd shortcuts, Mac glass. Same Flare content, themes and panels inside. |
| Signing | Apple Developer ID now: signed and notarized builds, notifications, mic access, updates. |
| Web engine | WKWebView, but only if agent browsing loses nothing. Phase 1 proves that first; if it cannot, CEF instead. |
| Shells | zsh, bash, fish and PowerShell (pwsh) are all first class. |
| Visual E2E | In CI only (GitHub macOS runners). Locally only headless unit tests; nothing takes the screen. |
| Targets | Apple Silicon (aarch64-apple-darwin), macOS 15 or later. |

This changes the design spec's non-goal "macOS or Linux" (2026-10-17 spec, line 54) to "Linux".

## Where the code stands

- About 80-85% is portable logic. The stack is already cross-platform: Slint on winit, wgpu,
  cosmic-text, alacritty_terminal, portable-pty behind the `PtyProcess`/`PtySpawner` traits.
- Nothing compiles on a Mac yet: `windows-sys`/`windows` are plain dependencies in about 12 crates,
  `std::os::windows` imports are unguarded, wgpu is built with `dx12` only, and `build.rs` needs RC.EXE.
- The Windows glue sits mostly in `flare-win` (39 modules), `flare-shells` (pwsh/cmd/WSL model),
  Windows path grammar in `flare-term`/`flare-tabs`, `flare-ctl` (named pipes), `flare-web` (WebView2),
  `flare-dictation` (WASAPI), `flare-update` (Velopack swap) and the e2e tooling.

## Rules for the port

1. **Win32 only in `flare-win` or behind `cfg(windows)`.** Mac code lives in `macos.rs` modules or a
   `platform` seam, never mixed into shared logic. Applies to Windows work from now on too
   (add to `docs/regression-rules.md`).
2. **Windows behaviour does not change.** Every Mac PR keeps the Windows CI green and the parity
   ratchet untouched.
3. **A `macos-latest` CI job** runs on every PR: fmt, clippy, nextest on what compiles. It may fail
   until phase 0 is done, then becomes a required check.
4. **Each feature reaches the Mac with its tests**: shared tests run on both, platform tests are
   gated, and `docs/parity.md` gets a macOS column.
5. **Security walls are re-proved, not assumed** (web sandbox, plugin permissions, control channel
   peer checks, link allowlists).

## Phases

Estimates are rough; the plan runs phase after phase, each as several small PRs.

| Phase | What | Done when | Est. |
|---|---|---|---|
| 0 Compile | Add the aarch64-apple-darwin target, gate Windows deps under `[target.'cfg(windows)']`, wgpu `metal`, `build.rs` keyed on `CARGO_CFG_TARGET_OS`, stub Windows-only modules, add the macOS CI job | `cargo check`, clippy and the portable tests pass on macOS | 1 wk |
| 1 Web spike | Prove WKWebView (objc2) against the agent browsing contract: `browser_look` screenshot and numbered controls, trusted clicks and keys (NSEvent into the view), scroll, wait, navigate, iframes, downloads, the sandbox for mini-apps (content rules, no network), the "Claude is using this page" stop-on-user-input | A written verdict per tool. Any gap that cannot be closed = switch to CEF | 1 wk |
| 2 Terminal core | PTY via openpty, process groups (killpg) and libproc for the child tree; shells: zsh, bash, fish, pwsh with OSC 7 cwd hooks and login-shell PATH; POSIX path grammar for links, cwd follow and tab restore; Metal render; fonts | Shells run, tabs restore, links and selection pass their tests on Mac | 2-3 wk |
| 3 Mac chrome | Native window (transparent titlebar, traffic lights, glass via NSVisualEffectView), Mac menu bar, Cmd keymap, clipboard, file drop, openers, single instance, settings in `~/Library/Application Support/Flare` | Flare looks and behaves like a Mac app | 2 wk |
| 4 Agent tools | `flare-ctl` over a Unix socket with peer-uid checks, `flare-mcp`, agent detection, changes panel, dock badge and attention, notifications | All `show_*`, `set_tab_*`, `convo.state` tools pass on Mac | 1-2 wk |
| 5 Web panel | The WKWebView (or CEF) backend behind the existing `WebBackend` trait: web panel, address bar, downloads, mini-apps, diagrams, live preview, mod and plugin pages, then the agent automation driver | Web and agent browsing parity, security walls re-proved | 3-4 wk |
| 6 Plugins | Plugin helpers (process groups plus a reaper), per-OS helper entry in `plugin.json` (protocol stays compatible), Finder "Open in Flare" service | Sample plugins run on Mac | 1 wk |
| 7 Dictation | Mic via CoreAudio/cpal, global hotkey (CGEventTap), whisper and Parakeet with Metal | Dictation parity; media pause only if a public API allows it (MediaRemote is locked since macOS 15.4) | 2 wk |
| 8 Ship | `.app` bundle and `.icns`, Developer ID signing, notarization, Velopack osx-arm64 updates (whole-bundle swap so signatures hold), DMG, release notes | A signed, notarized release updates itself | 1-2 wk |
| Ongoing | Mac E2E in CI (accessory policy, Unix socket hook), Windows-only scenarios gated | E2E green on macOS runners | |

Total: about 3 months to full parity at a steady pace.

Phase 5 progress: step 1 (2026-11-08) is the engine, `crates/flare-cef`: CEF 156.0.14 pinned and cached in
`~/Library/Caches/flare-cef`, the dev bundle with its five helpers (ad hoc), the CEF `NSApplication`, windowless
pixels, headless; `agent_job`'s DevTools methods (11 then, 14 with main's covered-click check) and the runaway-eval timeout proved on a Mac (rules 1020 to 1022,
`docs/parity.md` "macOS port, phase 5"). Step 2 (2026-11-08) is the web panel: `WebBackend` on CEF
(`crates/flare-web/src/macos/host.rs`), the page as a picture in its box with the window's pointer, wheel, keys and
input method handed over, the walls decided in `flare_web::walls` and re-proved with the spike's observers, the real
app's `NSApplication` made CEF's kind before winit (rules 1023 to 1026). Step 3 (2026-11-08) re-proves the walls behind Flare's proxy against
the spike's proxy page (every spelling of this machine and its network, http, https, ws and wss: 0 hits, 0 accepts, 0
mDNS queries; the web panel control leaks) and runs the real `agent_job` through `WebBackend::cdp` of the CEF host, with
the agent's editing chords given the command a Mac's text system would make of them (rules 1027 and 1028).
A review of the three steps (2026-11-08) proved six defects, fixed with checks that fail without the fix (rules 1029 to
1034): a profile per mini-app (a shared worker read another app's files), Flare's switches only (the command line
opened a DevTools port), held downloads staged outside `~/Downloads`, no call to Google from the engine with no page
asking, the mock keychain written down as an accepted gap until phase 8, and no colour or print panel from a page.

Phase 8 progress: step 1 (2026-11-08) is `Flare.app`: `tools/release/mac/make-app.sh` builds the release programs,
`flare-mac-app` bundles them (the CEF framework, five helpers, `flare-mcp`, the icon, the plugin in `Resources`) and
signs inside out with the Developer ID and per-program entitlements, then the script notarizes, staples and makes the
DMG; `.github/workflows/macos-app.yml` does the same in CI from secrets (rules 1036 to 1039, `docs/parity.md` "macOS
port, phase 8"). On Max's Mac a Developer ID `codesign` asked the keychain for the key with a prompt on the screen, so
the signed and notarized run waits for the key's access list to let `codesign` in.

A review of step 1 (2026-11-08) led to rules 1040 to 1043: no `codesign` with the identity unless an in-process check
with the keychain UI off says the key cannot prompt (`flare-mac-app check-key`; Max's key lacks the `apple:`
partition), the tests' own signing variables, the release check comparing each Mach-O's entitlements with its role's
and requiring a Developer ID Application signer, Chromium 156's GPU `allow-jit`, a terminal's camera, microphone and
Apple Events keys with their usage strings, CI cleanup that runs on failure and no cached CEF in the signing job, and
the Velopack order: sign and notarize the bundle `vpk pack` produced (`flare-mac-app resign`), check it inside the
update package.

Step 3 (2026-11-09) is updates and the release: `tools/release/mac/pack.sh` packs `Flare.app` with vpk 1.2.161 for
osx-arm64 and signs the bundle again by role after vpk adds `UpdateMac` (`flare-mac-app` as vpk's `codesign`), vpk
notarizes and staples it, and the bundle is checked inside the update package; `tools/release/mac/update-e2e.sh`
proves a whole-bundle update headless on a Mac (0.0.1 to 0.0.2 into a temp folder, the real `VelopackFeed` and
`UpdateMac`, no restart, no window, the swapped bundle still verifies); `.github/workflows/macos-app.yml` runs after
each Windows release and adds the Mac build to it when the secrets are set, `docs/releasing-macos.md` (rules 1046 to
1048). Proved ad hoc only: the signed, notarized run is one command for Max once the Developer ID key lets `codesign`
in without a prompt. A review of step 3 (2026-11-09) led to rules 1049 to 1052: the Mac build is the commit the release
was tagged at, one run per version, signed only from `main` through the `mac-release` environment; no key file,
password or scratch folder outlives its use; a switch between the mock and the real profile key sets cookies aside;
ad hoc signing carries each role's identifier, entitlements and (tools) the hardened runtime, so the ad-hoc pack
proves `UpdateMac`'s signature.

Phase 6 progress: step 1 (2026-11-08) is the helper's lifetime: `flare_proc::guard` starts a helper in a session and
group of its own (`setsid`) with an orphan guard in that group, which ends the group, the session and every
descendant it saw (escapees from the group or the session included) when Flare lets its pipe go, when the helper ends,
or when Flare ends or crashes; `flare_mods::helper::Host` runs helpers on the Mac with it (stop, update, removal,
restarts, `stop_all`), proved with real `sh` trees and a stand-in Flare killed with SIGKILL (rules 1053 to 1056,
`docs/parity.md` "macOS port, phase 6"). Step 2 (2026-11-08) is the per-OS helper entry: `plugin.json` keeps
`helper` as the Windows `.exe` and adds `helpers` (`windows`, `macos`), each system starting only its own program
(`Manifest::helper_here`), a Windows manifest loading unchanged everywhere and the channel's bytes unchanged; on a Mac
the program must be an executable inside the folder that is not quarantined (refused with a sentence rather than left
to a Gatekeeper dialog), and an install keeps a zip entry's execute bit; a stop while a helper starts no longer leaves
a zombie that held the update's and the removal's wait to its timeout; every sample plugin and the app's helper tests
run on the Mac (rules 1057 to 1060). Step 3 (2026-11-09) is "Open in Flare": an `NSServices` entry for folders and
files (`flare_win::open_in::plist_keys`, for phase 8's `release::app_keys` to take) whose provider, and an
`application:openURLs:` added to winit's app delegate before the loop runs, turn Finder's pasteboard or an `open -a
Flare` request into the hand-off a second launch sends, so a folder opens as a new tab and a file as its folder; the
command line's paths are kept out of open documents; checked with `plutil` and the handlers called as functions,
nothing registered with the system (rules 1061 to 1063). No Dock-icon drop: it needs a document type, which phase 8
leaves out. Left for the merge with phase 8: the one line that adds the keys to `Flare.app`, and a test on the built
bundle that `NSPortName` equals `CFBundleExecutable`. A review of phase 6 (2026-11-09) fixed: the guard, a fork that
never exec'd, kept a copy-on-write snapshot of Flare's memory per helper (+843 MB in an 800 MB probe), so it now execs
Flare's own program (`guard::run_if_guard` first in `ship_ctl::bootstrap`, rule 1054); a helper's `kill -9 0` killed the
guard and left `set -m` jobs and `setsid` escapees running, so the guard leads a group of its own and Flare makes its
own pass over the session and the group before the reap (rule 1053); the end is marked before any reap, also when
`waitid` fails (rule 1055); a zip made on Windows installs the Mac helper executable (rule 1059); any quarantined file
in the folder refuses the helper (rule 1058); off Windows a path ending in `"` is no longer rewritten (rule 1064).

## Known risks

- **Agent browsing on WKWebView** (phase 1 decides). No DevTools protocol and no http(s) request
  interception; replaced by JS snapshots, NSEvent input, WKContentRuleList and proxy settings (macOS 14+).
- **Headless E2E on macOS**: no hidden desktop, offscreen windows get clamped; CI only.
- **Signing gates features**: notifications, mic, updates and downloaded engines need the Developer ID
  before they can be tested for real.
- **Dictation media pause** may have no public API on macOS.
- **Parallel Windows work** adds Mac work whenever new Win32 code lands outside `flare-win`; rule 1 above.

## Needed from Max

- Enrol in the Apple Developer Program ($99/yr) before phase 7/8 (needed for notifications in phase 4
  to be tested for real).
- Approve this spec and plan.
