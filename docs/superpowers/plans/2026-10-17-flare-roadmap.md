# Flare Roadmap (implementation plan, top level)

> **For agentic workers:** this is the top-level plan. Each milestone gets its own detailed plan in
> `docs/superpowers/plans/` (task by task, TDD, checkbox steps) written when the milestone starts.
> Execute those with superpowers:subagent-driven-development, run as Workflow scripts.

**Goal:** Flare, a native Rust Windows terminal with full parity with Prism Terminal (frozen at its
2026-10-17 release), the same look, GPU and software rendering, and clearly better performance.

**Architecture:** A Cargo workspace. Slint draws all chrome (title bar, tabs, settings, dialogs). The
terminal grid is drawn by Flare's own renderer into a texture (GPU, `wgpu`) or a pixel buffer
(software) and shown inside the Slint scene as an image, so both renderers share one layout path.
`alacritty_terminal` holds each tab's grid, parser and scrollback; `portable-pty` (ConPTY, with a
bundled `conpty.dll` + `OpenConsole.exe`) runs the shells. Logic lives in small, UI-free crates so
the ported unit tests run without a window.

**Tech Stack:** Rust 1.99 (MSVC, x64), Slint, wgpu (DirectX 12 first), alacritty_terminal,
portable-pty, cosmic-text or swash, Velopack, cargo-nextest, cargo-deny, cargo-about.

**Spec:** `docs/superpowers/specs/2026-10-17-flare-design.md`. Executors read it before any task.

## Global Constraints

- Windows 10 1809+ / Windows 11, x64 only. Proprietary licence (`LICENSE`).
- Parity target: Prism Terminal at the release current on 2026-10-17 (exact tag and commit recorded
  in `docs/parity.md` by Milestone 0). Nothing newer is in scope.
- Flare's own names everywhere: profile `%APPDATA%\Flare`, its own single-instance lock, Claude
  plugin `flare-status`, hook signal `OSC 777;flare-agent;...`, tab id variable `FLARE_TAB_ID`,
  Explorer verb "Open in Flare". Never Prism names.
- Prism Terminal's regression rules hold in their native meaning (`docs/regression-rules.md`,
  written in Milestone 0 from Prism Terminal's).
- No new dependency without a reason in `README.md`'s dependency list and an entry in
  `THIRD-PARTY-NOTICES.md` (generated with cargo-about) in the same PR; `cargo deny check` passes.
- Slint attribution ("Made with Slint", for example `AboutSlint`) on the About page.
- No em-dashes in code comments, docs or UI copy.
- Diagnostics log `%APPDATA%\Flare\logs\diag.jsonl`: never typed text, clipboard or audio.
- The updater is built but switched off until the owner makes the repo public.
- Dictation bundles Prism Terminal's `whisper-server.exe`, `parakeet-cli.exe`, model catalogue and
  optional GPU pack.
- CI on every PR: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo nextest run --workspace`, `cargo deny check`.
- The e2e runs offscreen, unfocusable, never takes the foreground, one profile per scenario, reaps
  its processes, saves and restores the real clipboard, opens nothing outside the app.
- Performance bar (blocks Milestone 10 only; every milestone's PR reports the numbers): cold start
  to a usable prompt under 300 ms; idle memory at most a third of Prism Terminal's; heavy-output
  throughput at least twice Prism Terminal's with no frame over 33 ms (GPU); the software renderer
  at least matches Prism Terminal on all three.
- Each UI milestone's PR includes side-by-side screenshots of Flare and Prism Terminal on the same
  theme.
- Builders: Sonnet (mechanical stages Haiku). Reviews: Opus. Never Fable.
- PRs are stacked, one per milestone, each based on the previous branch; nothing is merged without
  the owner's yes per PR.

## Review Focus

- Very fast output (a runaway `cat` of a 100 MB file): the UI stays responsive and Ctrl+C reaches
  the shell; pinned by a throughput test in Milestone 2 and the e2e in Milestone 10.
- Resizing during heavy output and at 225% DPI: no crash, the prompt line survives (ConPTY sends
  nothing on resize); pinned in Milestones 1 and 2.
- GPU unavailable (remote desktop, VM, driver failure): Flare starts on the software renderer by
  itself and says so in Settings; pinned in Milestone 1.
- Paste of hostile text (control characters, bracketed-paste escapes, multi-line) and dropped paths
  with spaces or quotes: inert until the user acts; pinned in Milestone 5 with Prism Terminal's
  `sanitizePaste` and `quotePath` tests ported.
- Both apps installed side by side: no shared lock, profile, verb, plugin or signal; pinned by an
  e2e scenario in Milestone 9.

---

## Milestone 0: Repo, parity inventory and rules (branch `m0-foundation`)

- Workspace skeleton: `Cargo.toml` workspace, `crates/` layout below, `rust-toolchain.toml` (1.99,
  MSVC), `.gitignore`, `rustfmt.toml`, `deny.toml`, `about.toml`, CI workflow, `LICENSE` names Flare.
- `docs/parity.md`: the complete checklist of Prism Terminal behaviour, built from its source,
  docs, unit tests and e2e scenarios, grouped by milestone, each line naming the Prism Terminal
  file or test it comes from. This is the definition of done for every later milestone.
- `docs/regression-rules.md`: Prism Terminal's rules in their native meaning.
- `docs/test-port.md`: every Prism Terminal unit test file mapped to the Flare crate that will port
  it, or marked dropped (Electron-only) with a reason.

**Crate layout** (later milestones fill them):

| Crate | Responsibility |
|---|---|
| `crates/flare` | The binary: startup, wiring, window, Slint UI (`ui/*.slint`) |
| `crates/flare-render` | Grid renderers: GPU (`wgpu`) and software, glyph atlas, text shaping |
| `crates/flare-term` | Tab terminal: `alacritty_terminal` + `portable-pty`, resize, input encoding |
| `crates/flare-shells` | Shell detection, command lines, path quoting, paste sanitising |
| `crates/flare-agent` | Agent indicator: title parsing, output scoring, OSC 777 hooks, poll |
| `crates/flare-theme` | Themes, custom theme, contrast floors, chrome colours |
| `crates/flare-settings` | Settings model, persistence (atomic writes), Prism Terminal import |
| `crates/flare-dictation` | Dictation: engine processes, models, hotkey, mic, media pause |
| `crates/flare-update` | Update check and install over Velopack (off until public) |
| `crates/flare-win` | Windows glue: single instance, Explorer verbs, DWM material, argv |
| `crates/flare-diag` | Diagnostics log |
| `tools/e2e` | Native e2e harness (Rust test binary driving Flare over a `--e2e` named pipe) |

## Milestone 1: Window, chrome and the grid on both renderers (branch `m1-render`)

The riskiest piece first. A Slint window with Prism Terminal's title bar and an empty tab strip, a
terminal grid of static sample text (ANSI colours, bold, wide characters, emoji, a cursor) drawn by
`flare-render` on wgpu and on the software path, shown in the Slint scene, resizing correctly at
100% to 225% DPI. A Settings switch (temporary, in a debug panel) picks the renderer; a GPU failure
at start falls back to software. Frame-time and memory numbers recorded.

## Milestone 2: Pty, shells and one live tab (branch `m2-term`)

`flare-term` + `flare-shells`: spawn the default shell through bundled ConPTY, feed
`alacritty_terminal`, keyboard input encoding (physical keys, Ctrl+C/V by key code), resize without
losing the prompt line, scrollback, clean quit waiting for every shell. Heavy-output throughput test.

## Milestone 3: Tabs, restore, single instance (branch `m3-tabs`)

Tab strip top and sidebar, new/close/reorder, close confirmation, the + button with ask and
fixed-folder modes and the pinned/recent list, start screen, tab restore, single instance with
argument hand-off, window state, closing the window quits.

## Milestone 4: Themes, fonts, material (branch `m4-theme`)

Every Prism Terminal theme, custom theme, fonts, acrylic/Mica, contrast floors, the theme driving
the chrome. Side-by-side screenshots for every theme.

## Milestone 5: Selection, paste, drop, find, links (branch `m5-input`)

Selection and copy, paste and drop rules (`sanitizePaste`, `quotePath`), find in scrollback, links.

## Milestone 6: Agent indicator and hooks (branch `m6-agent`)

Title parsing, output scoring, the `flare-status` Claude plugin and `OSC 777;flare-agent`, marks and
colours, agent clock and title, agent resume on restore (the only command Flare writes).

## Milestone 7: Settings pages (branch `m7-settings`)

Every Prism Terminal settings page in the grouped-card layout, settings search, command help, About
with Slint attribution, first-run import from Prism Terminal.

## Milestone 8: Dictation (branch `m8-dictation`)

Bundled engines and models with pinned checksums, GPU pack, hotkey, the pill, microphone only,
pause media while dictating.

## Milestone 9: Updates, installer, Windows integration (branch `m9-ship`)

Velopack installer (per-user), the update chip and window (switched off until public), Explorer
verbs, diagnostics log reader, side-by-side install e2e with Prism Terminal.

## Milestone 10: Parity sweep and performance (branch `m10-parity`)

Walk `docs/parity.md` to zero open lines, finish the e2e checklist, tune to the performance bar,
final Opus review of the whole app against Prism Terminal.

## Execution

Per milestone: an Opus agent writes the detailed plan from `docs/parity.md` and this roadmap; a
Workflow runs Sonnet builders per task (in parallel where tasks are independent, each porting its
tests first) and an Opus reviewer per task; then an Opus whole-milestone review; then the PR,
stacked on the previous milestone's branch.
