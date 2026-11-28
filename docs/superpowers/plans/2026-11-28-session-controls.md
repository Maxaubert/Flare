# Claude session controls: implementation plan

Spec: `docs/superpowers/specs/2026-11-28-session-controls-design.md`. One PR, tests first.

1. **`flare-shells::resume::typed_start_line(shell_id, resume)`**: the start line for a live shell (pwsh, powershell,
   zsh, bash, fish: `claude [--resume <id>]`; cmd: the same inside the `NoDefaultCurrentDirectoryInExePath` guard;
   other shells: none). Only an id that passes the session check is used. Unit tests per shell.
2. **`flare-term::handle::SessionKey` and `TermHandle::session_key`**: the typed intent (Exit, Enter, Interrupt,
   Start). Unit tests on the bytes written.
3. **`flare::agent_hub`**: `last_sessions` (kept when the agent leaves, gone with the tab), `last_claude_session`,
   prompt freshness (`at_prompt`: a prompt report after the last typed key) and a prompt counter (`prompts_seen`),
   `look_soon` (asks the poll). Unit tests.
4. **`flare::claude_sessions`** (pure): `TabFacts`, the target sets (`end_targets`, `start_targets`), the row list
   with counts, the busy question's words, and `Runner`, the per-tab job state machine (wait idle, draft check, exit,
   enter, gone, restart start; interrupt for End now; deadlines) driven by `step(now, facts) -> Vec<Act>`. Unit tests
   for which tabs each action touches and for the Wait and End now flows.
5. **Dialog third button** (`ui/dialog.slint`, `ui/app.slint`): `alt-label`, `alt` callback, focus order Cancel,
   Wait, End now; unset for every other question. The dialog preview test for the new button.
6. **`flare::appctl::claude_glue`**: the tab menu rows (this tab, the flyout), the picks, the busy question in the
   dialog chain (gates on downloads and app links), the runner stepped in `host_wake` and `tick`, its `next_due`.
7. **Window tests** (`appctl/tests/claude_sessions_tests.rs`): the real controller with fake shells: rows and counts,
   End on idle tabs types `/exit` then Enter, a busy tab asks, Wait ends it once idle, End now sends ESC first, a
   draft is kept, Start types the resume line, Restart ends then starts at the next prompt, nothing on zero tabs.
8. **Docs**: rules 1380 to 1383 in `docs/regression-rules.md`, a line in `docs/features.md`, rule 2's exception.
9. `cargo fmt --all --check`, `cargo clippy -p flare -p flare-term -p flare-shells --all-targets -- -D warnings`, the
   tests of the touched crates. Commit, push, PR (not merged).
