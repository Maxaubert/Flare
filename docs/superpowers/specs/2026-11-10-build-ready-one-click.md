# Build ready: one click installs

Status: approved by the owner, 2026-11-10. Owner: "remove this menu ... i just want the button install now", and on the
details: the existing "Install and restart / Cancel" prompt is enough, and "Remove all of them" (idle install, Not today,
Discard).

## What changes

- **The chip has no menu.** Clicking "Build ready" is Install now.
- **An agent at work.** The click shows the existing close question ("Stop the agent and install the update?" with
  Install and restart and Cancel). No new question. With no agent working, the install starts at once.
- **No install by itself.** The idle install goes: a staged build installs only when the person clicks. The idle rule
  (`flare_update::idle`) and its timer go with it, unless something else still uses them.
- **Not today and Discard go.** A staged build stays on offer until it is installed or a new stage replaces it.
- **The tooltip** says "Build <commit>: click to install and restart".
- **Kept as it is.** The install toast with Cancel, the swap, the rollback, the start notices, the confirm after 20 s,
  and the agent-started-during-the-swap check.

## Tests and docs

- `staged_flow` and `ui_tests::staged_flows`: a click installs, a click with an agent working asks first and Cancel keeps
  the build on offer, no menu opens, nothing installs on its own after a minute idle.
- The e2e `idleInstall` scenario becomes a click install (still offscreen).
- `docs/features.md` (the staged build section) and a regression rule.
