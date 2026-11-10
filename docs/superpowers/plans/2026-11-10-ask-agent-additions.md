# Plan: Ask agent additions

Spec: `docs/superpowers/specs/2026-11-10-ask-agent-additions-design.md`. One PR per item, one at a time from fresh `main`.
Regression rules start at 238.

| # | Item | Branch | Size |
|---|---|---|---|
| 1 | Ask about the page | `feat/ask-page` | S |
| 2 | Markdown views | `feat/md-views` | L |
| 3 | Select text in files | `feat/file-select` | L |
| 4 | Arrows | `feat/arrows` | L |

Item 3 comes after item 2 so the Code view gets selection. Arrows come last because they are the largest and start with a probe.

Each item: worktree, tests first for pure logic, gate (fmt, clippy `-D warnings`, touched crates), independent review, fixes, docs,
PR, CI compared with main's four known failures.

## 1. Ask about the page
1. `ask_agent.rs`: title line in `SELECTION_SCRIPT`, three-part `page_answer`, `frame_page`; hostile-title tests.
2. `web_ctl.rs`: carry the title and whether the tab is a plain web page.
3. `appctl/sessions_glue.rs`: fall back to `frame_page` on a plain web page; keep the note elsewhere.
4. `appctl/tests/ask_tests.rs`; docs.

## 2. Markdown views
1. `preview::is_markdown`; `flare_surfaces::mdview` with `pulldown-cmark` as a direct pinned dependency (already in the lockfile),
   own escaping emitter, CSP, caps; check `deny.toml` and notices.
2. `TabPanel.view` and the `view` key in `store.rs`, with rule 40 fallbacks.
3. `AppSpec.source`; `panel_ctl.rs` renders on a worker, persists as text plus view, restores, falls back to Code.
4. Three-way control in `ui/panel.slint`; wrap button only in Code; `appctl.rs` wiring.
5. Ask agent label is the file name; check existing tests that open `.md` files; docs.

## 3. Select text in files
1. `crates/flare/src/panel_sel.rs`: pure selection model, word and line, text, label, per-row paint spans.
2. `ui/panel.slint`: pointer callbacks, row highlight, keys, Esc order; confirm wheel scrolling still works with a non-interactive Flickable.
3. `panel_ctl.rs`: hold the selection, clear on reload or tab change, drag auto-scroll.
4. `appctl.rs`, `sessions_glue.rs`: Ctrl+C and Ask agent with a selection; docs.

## 4. Arrows
1. Probe: can a layered pad window catch the mouse without taking focus? Record the result in `docs/surfaces/decisions.md`. Fall back
   to a single click if not.
2. `flare-win/agent_cursor.rs`: `compose_arrow`, `ArrowWindow`, a recording fake.
3. `browser_point`: `Method::BrowserPoint`, grant lists, `ctl_app.rs`, `flare-mcp` tool, `web_glue.rs`, `web_ctl.rs` state, re-measure, clears.
4. Draw mode: button in `ui/panel.slint`, tick-polled drag, `DOM.getNodeForLocation`, `frame_arrow`, `deliver_ask`.
5. `agent_job.rs`: `user_arrow` on the next look and the arrow drawn on the screenshot; `flare-mcp` output; docs and rules.
