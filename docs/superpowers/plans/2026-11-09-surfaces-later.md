# Plan: surfaces, later items (#36, #33, #17, #35)

Spec: `docs/superpowers/specs/2026-11-09-surfaces-later-design.md`. One feature at a time, each from fresh `main`,
because every PR adds a regression rule and cargo may only build once at a time.

## Order

| # | Issue | Branch | Why here |
|---|---|---|---|
| 1 | #36 3D viewer | `feat/model-viewer` | Most isolated, follows `show_audio` |
| 2 | #33 Ask agent | `feat/ask-agent` | Medium, touches panel header and menus |
| 3 | #17 Thumbnails | `feat/thumbs` | Medium, touches hover and settings |
| 4 | #35 Inline pictures | `feat/graphics` | Largest and may stop at the probe |

## Each feature

1. Worktree at `.claude\worktrees\<name>` from `origin/main`, shared `CARGO_TARGET_DIR`, `-j 4`, BelowNormal.
2. Implement the tasks below, tests first where the logic is pure.
3. Gate: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, tests for the touched crates.
4. Independent review of the diff; fix must-fix findings; rerun the gate.
5. Add a regression rule (next free number), `docs/features.md` entry, decision entry if a choice was made.
6. Push, open the PR, wait for CI, compare failures with main's list, merge if clean, close the issue, remove
   the worktree from outside it.

## Tasks

### #36 3D viewer
1. `tools/model-viewer/` (exact versions of three and esbuild, `src/viewer.js`); build once; commit
   `assets/three/{viewer.js,README.md,LICENSE}`; `THIRD-PARTY-NOTICES.md` entry.
2. `crates/flare-surfaces/src/model.rs`: `MODEL_MAX`, `MODEL_EXTS`, `model_ext(typed, real)`, `csp()`, `page()`; unit tests.
3. `crates/flare-ctl/src/method.rs` and `grant.rs`: `Method::ViewModel`, `view.model`, `BROWSER_TIMEOUT`, View permission.
4. `crates/flare/src/ctl_app.rs`: `view_model`, `read_model`, `MODEL_VIEWER` static, method list, dispatch, pinned-hash
   test, refusal tests, and the "pictures and text only" message names `show_model`.
5. `crates/flare-mcp/src/lib.rs`: `show_model` tool, routing and schema tests, `tests/stdio.rs`; point `show_html` at it.
6. e2e scenario `model` (hidden desktop), with a clear result if WebGL is unavailable there.
7. `docs/surfaces/protocol.md`, `docs/surfaces/decisions.md`, `docs/e2e.md`, `PRIVACY.md` if it lists tools.

### #33 Ask agent
1. `crates/flare/src/ask_agent.rs`: `frame_selection(label, text) -> Option<String>`; hostile input tests.
2. `appctl/sessions_glue.rs`: `deliver_ask(tab, text)` with the agent and question gates, `paste`, focus, no Enter.
3. `web_ctl.rs`: selection read through `run_script` with a typed continuation, dropped on navigation; update the
   trait's "e2e only" comment.
4. `ui/panel.slint`, `ui/app.slint`: header button and visibility; narrow panel check.
5. `appctl.rs`: dispatch by panel kind; grid selection row in the terminal menu.
6. Fake-pty and fake-web tests: bracketed bytes, no trailing CR, navigation drop, no agent, question up.

### #17 Thumbnails
1. `flare-surfaces/src/preview.rs`: `decode_thumb`, limits, shared checks; tests.
2. `flare-settings`: the pref, row, action; settings text list.
3. `crates/flare/src/thumbs.rs`: `ThumbCache`, `ThumbHover` with an explicit clock, worker.
4. `term_ui.rs`: `TermMsg::Thumb`, hooks in `on_move` and `tick`, hide reasons, `set_thumbs`.
5. `ui/thumb.slint`, `ui/app.slint`, placement with `place_tip` in `appctl.rs`.
6. Rig tests and a Slint component test at several scales; refusal tests for UNC, NUL and a link that changes kind.

### #35 Inline pictures
1. Probe (`crates/flare-term/tests/apc_conpty.rs`), findings in `docs/graphics.md`. **Stop here on a no-go**: docs only,
   straight to main, #35 stays open with the evidence.
2. On a go: `graphics.rs` and its tests.
3. `scan.rs` body state; `emulator.rs` arms and replies; `placeholder.rs`; cell pixel size; text filter choke point.
4. `flare-render`: `Grid.tiles`, `Scene.pictures`, CPU paint, GPU pipeline `picture.wgsl`, texture cache, parity tests at
   1.0, 1.25, 1.5 and 2.25.
5. `host.rs`, `live.rs`: cell pixel size on layout and scale change.

## Finish

Prepare one release build from final `main` for a quick install, and report per feature what merged, what was tested and
what wasn't. Ask about installing in the morning.
