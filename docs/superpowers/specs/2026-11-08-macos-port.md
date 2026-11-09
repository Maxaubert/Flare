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
pixels, headless; `agent_job`'s DevTools methods (11 then, 14 with main's covered-click check) and the runaway-eval timeout proved on a Mac (rules 251 to 253,
`docs/parity.md` "macOS port, phase 5"). Step 2 (2026-11-08) is the web panel: `WebBackend` on CEF
(`crates/flare-web/src/macos/host.rs`), the page as a picture in its box with the window's pointer, wheel, keys and
input method handed over, the walls decided in `flare_web::walls` and re-proved with the spike's observers, the real
app's `NSApplication` made CEF's kind before winit (rules 254 to 257). Step 3 (2026-11-08) re-proves the walls behind Flare's proxy against
the spike's proxy page (every spelling of this machine and its network, http, https, ws and wss: 0 hits, 0 accepts, 0
mDNS queries; the web panel control leaks) and runs the real `agent_job` through `WebBackend::cdp` of the CEF host, with
the agent's editing chords given the command a Mac's text system would make of them (rules 258 and 259).
A review of the three steps (2026-11-08) proved six defects, fixed with checks that fail without the fix (rules 260 to
265): a profile per mini-app (a shared worker read another app's files), Flare's switches only (the command line
opened a DevTools port), held downloads staged outside `~/Downloads`, no call to Google from the engine with no page
asking, the mock keychain written down as an accepted gap until phase 8, and no colour or print panel from a page.

Phase 7 progress: step 1 (2026-11-09) is the microphone and the link. The microphone is a CoreAudio input queue behind
the WASAPI seams (`crates/flare-dictation/src/macos/mic.rs`: input devices by UID, the default read at each open, 16 kHz
by the shared resampler, a lost device reported once), with the access read and never asked for: off or restricted
refuses before any device is touched and Settings names the System Settings pane (`NSMicrophoneUsageDescription` is
phase 8's Info.plist). The link between copies is a Unix socket in a 0700 folder under the per-user temp folder with
the named pipe's handshake, a lock per name and `getpeereid` on both ends (`macos/link_sock.rs`), and every link test
now runs on both systems; a WAV through the file seam drives the controller end to end with a fake engine (rules 304
to 307, `docs/parity.md` "macOS port, phase 7"). Step 2 (2026-11-09) is the key from any app (`macos/keyhook.rs`): a
chord that ends in a key is a Carbon hot key (no permission), a modifier binding (the default Ctrl+Cmd, Right Option)
a listen-only event tap made only once Input Monitoring reads as granted, never asked for, with Settings naming the
pane meanwhile and the window reading the key; the picker's digits and Esc are Carbon hot keys while switched on; one
copy holds the key by `flock`. Bindings are named and checked as Mac keys (Right Alt is Right Option). Nothing is typed
into other apps (Windows does not either), so no Accessibility; media pause is off on the Mac with its reason, as no
public API can pause another app's player (rules 308 to 311). Step 3 (2026-11-09) is the reaping job and the engines
with Metal: each engine child in a process group of its own, handed to one `/bin/sh` reaper that kills every group it
holds when its pipe closes, so also when Flare is killed with `SIGKILL` (`macos/job.rs`), and every engine and host
test now runs on both systems; the bundled engine is whisper.cpp built from the commit and tree tag b5130 names (the
release has no macOS program) with Metal embedded and only system links (`tools/build-whisper-macos.sh`, built and
given a real pass in CI), run first on Metal and once for good on the CPU (`-ng`), with no NVIDIA pack, Turbo as the
Recommended pick, the Parakeet host loading the built `libggml` and `libparakeet`, and models in
`~/Library/Application Support/FlareDictation` with no quarantine flag (rules 312 to 316). The engine goes into the
bundle in phase 8. A review of steps 1 to 3 (2026-11-09) tightened them: a tap whose Input Monitoring is revoked
ends and lets the lock go, a register gives up as soon as the hook stops (no 2 s stall at quit), the reaper withdraws a
pid before it is reaped, answers every line and watches Flare's pid, the system's Control chords are refused as a
binding, the microphone row says the access as it is now, an opened-up link folder of this user is made private
again, and the engine folder is reused only with exactly its stamped files, and a media worker that crashes closes its queue before
its reply (a test flake, shared with Windows) (rules 317 to 326). Next: cues and the
widget.

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
