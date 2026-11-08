# Flare: design (2026-10-17)

Flare is Prism Terminal rewritten as a native Rust Windows app. This document is what a builder
plans from. It moves into the Flare repo as `docs/superpowers/specs/2026-10-17-flare-design.md`
once the repo exists.

## Intent and the why

The owner wants a terminal that is fast and light the way native terminals are (Zed, Alacritty,
WezTerm), with everything Prism Terminal does. Prism Terminal is Electron + React + xterm.js; its
terminal core is shared with Prism (the media viewer), so rewriting it in place would break Prism.
Flare is therefore a new, separate product. Prism Terminal and its `core/` stay exactly as they
are and keep serving Prism. The owner accepts that Flare and Prism Terminal will drift apart.

Success: Flare does everything Prism Terminal does, looks the same, and is clearly faster and
lighter (startup, memory, heavy output, scrolling), measured against Prism Terminal on this PC.

## Decided by the owner (2026-10-16/17)

- **Name:** Flare. **Repo:** `github.com/Maxaubert/Flare`, private, cloned at
  `C:\Users\Admin\Documents\Claude\Github\Flare`.
- **Platform:** Windows 10 1809+ / Windows 11, x64 only.
- **Licence:** proprietary, the same terms as Prism (Wind's licence, see Prism's `LICENSE`).
- **Look:** a 1:1 port of Prism Terminal's design and themes. Familiar, not a redesign.
- **Scope of "done":** full parity with Prism Terminal (everything below), not a cut-down first
  version.
- **Rendering:** fully native, GPU by default, with a non-GPU (software) renderer the user can pick,
  as Tabby and WezTerm allow.
- **Process:** design and plan now, no throwaway spike first.
- **Tools on the machine:** Rust 1.99 (MSVC), clippy, rustfmt, rust-analyzer, cargo-deny,
  plus cargo-binstall, cargo-nextest, slint-viewer, slint-lsp, cargo-about (third-party notices),
  cargo-machete (unused dependencies) and bacon (watch loop).
- **Who builds:** most work is delegated to Sonnet agents (Haiku for mechanical work), reviews are
  done by Opus. Fable is never used.

## Goals

1. Feature parity with Prism Terminal (inventory below).
2. Native performance: cold start under ~300 ms to a usable prompt, idle memory a fraction of
   Prism Terminal's, smooth scrolling and no dropped frames on heavy output (for example
   `Get-ChildItem -Recurse C:\Windows` or a large `cat`).
3. GPU and software rendering, chosen in Settings, with automatic fallback to software when the GPU
   path fails to start (remote desktop, VMs, broken drivers).
4. The same look: themes, custom theme, fonts, acrylic/Mica, tab strip along the top or as a
   sidebar, the start screen, settings pages.
5. A test suite at least as strong as Prism Terminal's, ported, not reinvented.

## Non-goals

- Anything Prism Terminal lists as out of scope: split panes, several shells per tab, per-shell
  profiles, a tray icon, multiple windows, SSH management, app styles separate from the terminal
  theme, anything that reads or shows files.
- Embedding in Prism, or sharing code with Prism Terminal's `core/`. Flare is standalone.
- Linux. (macOS was a non-goal until 2026-11-08; see `2026-11-08-macos-port.md`.)
- Command help (#12): it is Prism Terminal's; port it only as part of parity (it is in scope
  because it is in Prism Terminal), never extend it.

## Anti-goals (failure even if it works)

- Slower or heavier than Prism Terminal anywhere a user would notice.
- A look that drifts from Prism Terminal's without an owner decision.
- Breaking or touching Prism Terminal or Prism in any way.
- Weakening any of Prism Terminal's safety rules (below).
- An e2e that steals focus or shows windows on the owner's screen.

## Constraints

- **Prism Terminal's regression rules carry over** where they still apply to a native app
  (`PrismTerminal/CLAUDE.md` "Rules that must not regress" and `docs/regression-rules.md`). In
  particular: what reaches a shell is inert until the user acts (paste sanitising, per-shell path
  quoting, tools by full path); the only command the app writes into a shell is the agent resume,
  as the startup command with a validated id carried in the environment; the updater installs only
  what it offered; atomic state writes; closing the window quits and waits for every shell to be
  gone; the indicator is the agent's own word (title first, Claude's hook signal above it, output
  scoring only as fallback); typing is heard from keys, not from the terminal's own replies; cells
  are not characters; ConPTY sends nothing on a resize (keep the prompt line); the theme drives the
  chrome with measured contrast. Electron-only rules (sandbox flags, asarUnpack, `titleBarStyle`)
  are replaced by their native meaning, recorded in Flare's own `docs/regression-rules.md`.
- **The product is for other people:** a feature bundles or fetches what it needs and works on a
  fresh Windows install (dictation models fetched with pinned checksums, as in Prism Terminal).
- **Diagnostics:** a local `diag.jsonl` log like Prism Terminal's (stalls, slow work, errors,
  crumbs), never sent anywhere; typed text, the clipboard and audio are never written.
- **Global repo rules apply:** code through PRs, owner's yes per merge, version bump in the PR,
  CI gates, e2e before push, install as the last verification step.
- **No new dependency without a reason**, recorded in the README's dependency list and in
  `THIRD-PARTY-NOTICES.md` in the same PR.

## Feature inventory (parity list)

The builder's first task is to turn this into a complete checklist from Prism Terminal's source,
`docs/`, unit tests and e2e scenarios, so nothing is missed. Known areas:

- **Tabs:** one shell and its folder per tab; strip along the top or as a sidebar (#162); new,
  close, reorder, rename, close confirmation; tab restore on start, with agent resume.
- **Shells:** PowerShell 7, Windows PowerShell, cmd and WSL, as Prism Terminal detects them;
  the bundled ConPTY.
- **The + button:** ask / fixed-folder modes, pinned and recent folders list.
- **Start screen.**
- **Terminal:** xterm-compatible emulation, colours, cursor styles, selection, copy and paste (the
  paste and drop rules, `sanitizePaste`, `quotePath`), find in scrollback, links, scrollback size,
  resize behaviour.
- **Agent indicator:** title parsing, output scoring fallback, Claude Code hooks (OSC 777) via the
  bundled Claude plugin, the marks and colours (working, done, question, failed), agent clock and
  title, the poll.
- **Themes:** the full theme list, custom theme, fonts, acrylic/Mica material, contrast floors, the
  theme driving the whole window chrome.
- **Settings:** every page Prism Terminal has (Terminal, Agents, Dictation, appearance, and so on),
  the grouped-card layout, settings search.
- **Dictation:** local Whisper, hold-to-talk hotkey (default per Prism Terminal at port time),
  models fetched with pinned checksums, the pill, microphone only.
- **Updates:** the update chip and its window, install only what was offered (built, switched off
  until the repo is public).
- **Windows integration:** single instance (second launch hands its folder to the running app),
  Explorer verbs ("Open in Flare"), command-line arguments, window state.
- **Diagnostics log** and its reader tool.
- **Migration:** on first run, Flare imports Prism Terminal's settings, themes, custom theme, fonts
  and shells once. After that the two apps are independent. Tabs are not imported.

## The how: decided by the owner

- Rust, native UI, no web view.
- GPU rendering with a selectable software renderer.
- **UI toolkit: Slint** (owner, 2026-10-17). Proprietary use is under Slint's royalty-free
  desktop licence, which asks for attribution: show "Made with Slint" (for example the
  `AboutSlint` widget) on Flare's About page.
- **Terminal engine: `alacritty_terminal`** (owner, 2026-10-17).
- **Installer and updates: Velopack** (owner, 2026-10-17): per-user, no admin, delta updates from
  GitHub Releases. Flare keeps Prism Terminal's update chip and update window as its UI on top of
  Velopack; the "install only what was offered" rule still holds.
- **First-run import:** settings, themes, custom theme, fonts and shells from Prism Terminal, once;
  tabs start fresh (owner, 2026-10-17).
- **Updates wait for a public repo** (owner, 2026-10-17): the repo stays private while Flare is
  built, and the updater is built and tested but switched off until the owner makes Flare public.
  Installers are attached to the private repo's releases meanwhile; the owner installs them by hand.
- **Parity target is frozen** at the Prism Terminal release current on 2026-10-17 (0.55.x/0.56.0;
  the builder records the exact tag and commit in the plan). Later Prism Terminal features,
  including the in-progress catch-up/orchestrator and dictate-anywhere work, are NOT in scope; the
  owner adds them to Flare one by one.
- **Side by side with Flare's own names** (owner, 2026-10-17): both apps can be installed at once.
  Flare has its own Claude plugin (not `prism-terminal-status`), its own hook signal (not
  `prism-agent`), its own tab id variable (not `PRISM_TAB_ID`), its own Explorer verbs, profile
  folder and single-instance lock. Sonara learning Flare's tab id is a separate change in Sonara's
  repo, raised with the owner when Flare reaches it. An e2e check proves the two apps never clash.
- **No stable copy for now** (owner, 2026-10-17): the owner keeps working in Prism Terminal while
  Flare is built, so Flare's install steps need not protect a working copy, and the stable-copy flow
  is not ported.
- **Dictation uses Prism Terminal's engines** (owner, 2026-10-17): Flare bundles the same
  `whisper-server.exe` and `parakeet-cli.exe` programs, the same model catalogue and the optional
  GPU pack, and keeps "pause media while dictating". `cpal` (or equivalent) only captures the
  microphone; recognition stays in the bundled programs.

## The how: left to the builder (recommended starting points)

These are recommendations; the builder may change any of them with a reason written in the plan.

- **Pty:** `portable-pty` (ConPTY), with Flare shipping a bundled `conpty.dll` + `OpenConsole.exe`
  as Prism Terminal does.
- **Text rendering:** `cosmic-text` or `swash` for shaping and glyph rasterising, a glyph atlas on
  the GPU path, the same rasteriser into a pixel buffer on the software path.
- **GPU:** `wgpu` (DirectX 12 first) for the terminal grid.
- **Window chrome:** a custom title bar drawn by Flare; acrylic/Mica through `window-vibrancy` or
  direct DWM calls.

**The main technical risk** is drawing a GPU (and software) terminal grid inside the UI toolkit's
window without tearing or double rendering, at high frame rates, while the toolkit draws the chrome
around it. The plan's first milestone must prove this (grid + chrome + both renderers + resize)
before anything else is built on it.

## Testing

- **Unit tests:** every Prism Terminal unit test whose logic Flare keeps (about 1,570 tests in
  166 files) is ported to a Rust `#[test]` alongside the module it covers. A module is done when
  its ported tests pass. Tests for Electron-only plumbing are dropped with a one-line note.
- **E2E:** Prism Terminal's ~1,100 e2e checks become the checklist for a native e2e harness. The
  harness drives the real app through a test-only control channel (for example a named pipe enabled
  by `--e2e`), runs it offscreen and unfocusable, never takes the foreground, gives each scenario
  its own profile and reaps its processes, as Prism Terminal's does. Nothing opens outside the app
  during a run, and the real clipboard is saved and restored.
- **Performance tests:** startup time, idle memory, heavy-output throughput and frame times,
  compared with Prism Terminal, on both renderers.
- **CI:** `cargo fmt --check`, `cargo clippy -D warnings`, unit tests on PRs and main; the e2e is the
  local pre-push gate.

## How it gets built

- A plan of milestones, each a PR: (1) window + chrome + grid + both renderers proven; (2) pty,
  shells, one working tab; (3) tabs, restore, single instance; (4) themes, fonts, material;
  (5) selection, paste/drop, find, links; (6) agent indicator and hooks; (7) settings pages;
  (8) dictation; (9) updates, installer, Explorer verbs, migration; (10) parity sweep and
  performance tuning.
- Each milestone runs as a workflow: Sonnet agents port modules and their tests, Haiku does
  mechanical stages, an Opus agent reviews every milestone's diff against Prism Terminal's
  behaviour and these rules before the PR is opened.

## Calls made by the planner (not the owner)

- `portable-pty`, `wgpu`, `cosmic-text`/`swash` recommendations above.
- **Import source:** the Stable profile (`%APPDATA%\PrismTerminalStable`) when it exists, since the
  owner works there, else `%APPDATA%\PrismTerminal`. Settings that live in the web view's storage
  (`core/renderer/lib/prefStore.ts`) are read from Chromium's on-disk Local Storage (LevelDB); if a
  setting cannot be read reliably it is skipped, never guessed.
- **"Looks the same" is checked by eye:** each UI milestone's PR includes side-by-side screenshots of
  Flare and Prism Terminal on the same theme for the owner; no pixel-diff gate.
- **Performance bar** (blocks only milestone 10; every milestone reports its numbers): cold start to
  a usable prompt under 300 ms; idle memory at most a third of Prism Terminal's; heavy-output
  throughput at least twice Prism Terminal's with no frame over 33 ms on the GPU renderer; the
  software renderer at least matches Prism Terminal on all three.
- Command help is ported for parity but not extended.
