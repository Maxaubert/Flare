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
| `flare-mcp.exe` | The same stdio MCP server can be shared if kept app-neutral (it only speaks the protocol) |
| Picture and text viewers (Slint, decoded on a worker) | `img` and `pre` in the panel's view; the main process reads and decodes over IPC and checks the same allowlist (see `lessons.md`, stage 1) |
| The splitter's single refit on release (rule 24) | xterm `fit()` called once on the splitter's release, not on every move |
| `panels.json` beside `tabs.json`, matched by folder and place | The same file and the same matching rule; flush it on `before-quit` and session end like the other stores |
| Chords Ctrl+Shift+P and Ctrl+Shift+O | The same chords in the shared key table; both always the app's |
| Worker generation number plus a shared "newest" check | An `AbortController` per load; one decode at a time in the main process |

Stage 1 detail: `decisions.md` (Stage 1 and its review fixes) and `lessons.md` (Stage 1).
