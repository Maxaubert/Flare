# Surfaces: notes for the Prism Terminal port

Flare builds Surfaces first (panels, the control channel, agent tools, the browser the agent can
drive, the conversation reader, mods). Prism Terminal gets the same features afterwards. This folder
is what PT's build reads first, so it does not hit the same holes.

- Design: `../superpowers/specs/2026-10-21-surfaces-design.md`
- Plan and stages: `../superpowers/plans/2026-10-21-surfaces.md`
- `protocol.md`: the control channel, every method (the contract both apps share).
- `decisions.md`: every choice made while building, with the reason and date.
- `lessons.md`: holes hit, what fixed them, and what it means for Electron.

## What changes for Electron (filled in as stages land)

| Flare piece | Prism Terminal equivalent |
|---|---|
| WebView2 child HWND over the panel slot | `WebContentsView` in the window (no airspace problem for Chromium overlays drawn by the renderer, but the same z-order issue for native menus) |
| CDP via `CallDevToolsProtocolMethod` | `webContents.debugger.sendCommand` (same CDP methods) |
| Named pipe server in Rust | Node `net.createServer` on the same pipe name pattern, protocol unchanged |
| A random secret per tab in `FLARE_TAB_TOKEN`, looked up on every request | The same: `crypto.randomBytes` in the main process when the pty is spawned, a `Map` token to tab, `timingSafeEqual`; `hello` takes `tab_token` and refuses a `tab` (see `lessons.md`, stage 2) |
| `FLARE_CTL_PIPE`, `FLARE_CTL_PID` and `FLARE_MCP_EXE` set in every shell by `pty_env`, only once the server runs | The same variables in `ptyEnv` (set after `listen` succeeds), the same drop-an-inherited-copy rule; the client connects only to the pipe served by `FLARE_CTL_PID` |
| `flare-mcp` named in the bundled plugin's `mcpServers` with its absolute path written in at install (no block if the exe is missing) | The same: render `plugin.json` when the plugin is copied; never `${VAR}` |
| A relative path from `flare-mcp` is made absolute against the agent's own folder | The same; the channel's own rule for other clients is the tab's cwd |
| `flare-mcp.exe` | The same stdio MCP server can be shared if kept app-neutral (it only speaks the protocol) |
| Picture and text viewers (Slint, decoded on a worker) | `img` and `pre` in the panel's view; the main process reads and decodes over IPC and checks the same allowlist (see `lessons.md`, stage 1) |
| The splitter's single refit on release (rule 24) | xterm `fit()` called once on the splitter's release, not on every move |
| `panels.json` beside `tabs.json`, matched by folder and place | The same file and the same matching rule; flush it on `before-quit` and session end like the other stores |
| Chords Ctrl+Shift+P and Ctrl+Shift+O | The same chords in the shared key table; both always the app's |
| Worker generation number plus a shared "newest" check | An `AbortController` per load; one decode at a time in the main process |
| A transcript the agent writes to `~/.claude/projects/<slug>/<id>.jsonl`, tailed by a Rust worker | The same file, read by a main-process worker (`fs.open`, a 4 MB cap per read, `\n` splits, a partial line carried); the fold is pure JSON and ports line for line; the replacement test is `ino` plus `dev`, not `birthtime` (see `convo.md` and `lessons.md`, stage 6) |

Stage 1 detail: `decisions.md` (Stage 1 and its review fixes) and `lessons.md` (Stage 1).
Stage 2 detail: `decisions.md` (Stage 2), `lessons.md` (Stage 2) and `protocol.md` (the final contract of the channel and the tools).
Stage 6 library detail: `convo.md` (the transcript format as Flare reads it, with what is guessed), `decisions.md` and `lessons.md` (Stage 6 library).
