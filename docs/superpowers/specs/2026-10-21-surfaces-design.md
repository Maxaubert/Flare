# Surfaces: Flare as an app for agents, not only a text grid

Status: approved direction (owner, 2026-10-21, "build it robustly so we can support it in the future";
not urgent). Plan: `docs/superpowers/plans/2026-10-21-surfaces.md`. Porting notes for Prism
Terminal: `docs/surfaces/README.md`.

## Why

Common terminals are built around a shell and a text grid. Flare is built around AI agents (Claude
Code, Codex). Agents produce things that are not text: images, diagrams, web pages, edits to many
files, plans, questions. Today the agent can only answer in tables and terminal characters, and the
user leaves the terminal to look at anything. Surfaces make Flare a place where the agent can SHOW
things, and where the agent can USE things (a browser it can see and click), the way the iOS simulator
MCP lets Claude see, tap and type in a simulator on a Mac
([joshuayoes/ios-simulator-mcp](https://github.com/joshuayoes/ios-simulator-mcp): `ui_describe_all`,
`ui_tap`, `ui_type`, `ui_view` screenshot).

The owner's words (2026-10-21): "less like a terminal but more like an app where you can do stuff";
"if Claude provides a link to an image in a folder, you can view it inside the terminal"; "Claude
could generate diagrams or models"; "other mods allow you to play YouTube videos, so it can't just be
a frame here and there"; "it's important that Claude can interact with them ... Claude could use the
browser inside of Flare and look around and click around instead of opening Chrome".

## Goals

1. **Panels.** A tab can show content beside its terminal: native viewers (image, text) and web
   content (a real browser, mini-apps, diagrams, video at full frame rate).
2. **One control channel.** Everything that can be done to a panel is a command on one local,
   per-user, versioned channel. The UI, the agent tools and mods all use it. Nothing has a private
   back door.
3. **Agent tools.** Flare ships an MCP server so Claude Code (and any MCP client) gets tools to show
   content and to drive the browser panel: open, navigate, look (screenshot and an element snapshot),
   click, type, press, scroll, wait. Claude sees what it does.
4. **Flare reads the conversation.** For a tab running Claude Code, Flare follows the session's
   transcript file and shows what the agent did: changed files with diffs, the task list.
5. **Mods.** A mod is a folder with a manifest that declares permissions. Mods get panels, the control
   channel (scoped to their permissions) and can expose tools to the agent.
6. **Documented for the port.** Prism Terminal will get the same features after Flare. Every stage
   records decisions, the protocol and the holes hit, in `docs/surfaces/`.

## Non-goals (for now)

- Pictures drawn inside the text grid (kitty graphics protocol, sixel, iTerm images). Tracked as its
  own issue; it is a terminal feature, not a panel feature, and does not block anything here.
- Running third-party mods built for other terminals unchanged (terminal-browser by zenbu-labs). Our
  mods are our format.
- A mod store, signing, remote installs, network-hosted mods.
- Rewriting how Claude Code draws its own UI. Flare never edits or restyles the agent's text.

## Anti-goals (a failure even if it works)

- The terminal gets slower. Panels cost nothing while closed: no WebView2 process, no watcher,
  no pipe client, no timer (rule 34: nothing on the UI thread waits).
- An agent or a mod reaches what the user did not allow: files outside what it was handed, the user's
  real browser profile, keystrokes into a shell, other users' sessions.
- A web panel covers Flare's own menus or dialogs, or is blurry or misplaced at 100 to 300% scaling or
  after a monitor move.
- Protocol drift between Flare and Prism Terminal. The protocol is versioned and documented once.

## Constraints

- Windows only. DPI 100 to 300%, across monitor moves (owner rule).
- Existing rules in `docs/regression-rules.md` hold. In particular: 1 and 2 (nothing new types into a
  shell), 13 and 14 (paths are re-resolved by the backend; runnable files are never run), 34 (no
  blocking on the UI thread), 36 and 40 (state stored atomically, unknown values read as default).
- New dependencies are listed in README with a reason and in `THIRD-PARTY-NOTICES.md` in the same PR.
- The e2e stays offscreen and never takes the foreground (`docs/e2e.md`).

## Architecture

```
            +-----------------------------  Flare window  ------------------------------+
            |  tabs | terminal grid (wgpu)            | splitter | panel slot             |
            |       |                                 |          |  native viewer (Slint) |
            |       |                                 |          |  or WebView2 (child    |
            |       |                                 |          |  HWND over the slot)   |
            +---------------------------------------------------------------------------+
                 ^ UiMsg                                        ^
                 |                                              |
        +------------------+    JSON lines over a named pipe    |
        |  surfaces host   | <-------------------------------+  |
        |  (UI thread,     |                                 |  |
        |  owns panels)    |   \\.\pipe\flare-ctl-<user>      |  |
        +------------------+                                 |  |
                                                  +----------+--+------+
                                                  | flare-mcp (stdio)  | <- Claude Code (MCP)
                                                  | mods (token)       |
                                                  +--------------------+
```

### Panels

- A tab has at most one open panel in v1, shown to the right of the grid in a **panel slot** with a
  draggable splitter. Width is stored per tab as a fraction (atomic, rule 36). The grid refits once
  when the slot settles (rule 24).
- A panel has a **kind**: `image`, `text`, `web`, `changes`, `tasks`, `board`, `mod:<id>`. Unknown
  kinds read from storage are dropped (rule 40).
- **Native panels** (image, text, changes, tasks, board) are Slint. **Web panels** (browser, mini-app,
  diagram, video, mod UI) are one WebView2 controller each, parented to the Flare window as a child
  HWND and moved to the slot's rectangle in physical pixels on every layout change, DPI change and
  monitor move.
- **Airspace.** A child HWND paints above Slint. While any Flare overlay is open (menu, dialog,
  settings, help, picker, find bar over the slot), the web panel is hidden and a still of its last
  frame is drawn in its place. One Slint property (`overlay-open`) decides it; a test pins every
  overlay to it.
- A panel never takes keyboard focus on its own. Focus moves to it on a click in it, or the panel
  chord, and back to the grid with Esc (twice in a web page that uses Esc) or the chord.

### Control channel (protocol v1)

- Transport: a named pipe `\\.\pipe\flare-ctl-<hash of the user SID>`, ACL to the current user only,
  created by the running Flare. JSON, one object per line, request `{id, method, params}`, answer
  `{id, result}` or `{id, error: {code, message}}`, events `{event, params}`.
- First message is `hello {client, version, token?, tab_token?}`. The answer lists `protocol: 1`, the
  capabilities, and the caller's grants. A tab-scoped client passes `tab_token` (from `FLARE_TAB_TOKEN`, a
  per-tab secret every shell gets, rule 12); `tab` is refused, because the public `FLARE_TAB_ID` would let
  any process act as a tab (stage 2 review; `protocol.md` and `decisions.md` are the source of truth). A caller with no token gets the **agent** grant; a mod's token
  maps to its manifest permissions.
- Methods are namespaced: `panel.*`, `view.*` (show content), `browser.*` (drive the web panel),
  `convo.*` (read what Flare knows about the conversation), `tabs.*` (list only). Every method is
  documented in `docs/surfaces/protocol.md` with its grant, params, result and errors.
- Limits: 16 MB per message, 64 clients, per-client rate limit, every request answered within its
  timeout (default 15 s, browser waits up to 60 s). A malformed line closes that client only.

### Agent tools (flare-mcp)

- `flare-mcp.exe` ships beside `flare.exe`. It is a stdio MCP server (JSON-RPC 2.0, no network) that
  connects to the control channel with the tab's secret from `FLARE_TAB_TOKEN`, only to the pipe served by the
  process in `FLARE_CTL_PID`. Outside Flare it answers every
  tool with "not running inside Flare".
- The bundled `flare-status` Claude Code plugin declares it in its manifest, so Claude in a Flare tab
  gets the tools with no setup. Users of other MCP clients add it by path.
- Tools (v1): `show_image`, `show_file`, `show_diagram` (Mermaid), `show_html` (mini-app),
  `open_page`, `browser_look` (screenshot as MCP image content plus a compact element list with refs),
  `browser_click` (ref or x,y), `browser_type`, `browser_press`, `browser_scroll`, `browser_navigate`
  (url, back, forward, reload), `browser_wait` (text, selector or time), `browser_eval` (off unless the
  user turns it on), `close_panel`.
- The agent's browser is visible to the user in the panel. A thin ribbon says "Claude is using this
  page" while a tool call is running, and the user can take over at any time.

### Browser engine

- WebView2 (Edge runtime, present on Windows 11 and updated by Windows). Rust binding: `webview2-com`
  plus the `windows` crate already in the workspace.
- Agent control goes through `CallDevToolsProtocolMethod` (the Chrome DevTools Protocol inside the
  controller, no debugging port opened): `Page.captureScreenshot`, `Accessibility.getFullAXTree` /
  `DOM.getDocument` for the snapshot, `Input.dispatchMouseEvent`, `Input.insertText`,
  `Input.dispatchKeyEvent`. Calls are awaited in order.
- Profile: `%LOCALAPPDATA%\Flare\WebView2\<profile>` with separate profiles for the user's browser
  panel, the agent's (same as user's in v1, by owner choice later), mini-apps (no network unless the
  tool asks) and each mod. Never the user's Edge profile.
- No runtime installed: the web panel shows a message with a link to install it. Nothing else breaks.
- Mini-apps and diagrams are served from a per-panel folder through a virtual host
  (`https://flare-app.local/`), with a strict Content Security Policy. Mermaid is bundled, not fetched.

### Conversation reader

- Claude Code writes each session to `~/.claude/projects/<folder slug>/<session id>.jsonl`. Flare
  already learns the session id per tab from the `flare-status` hook (OSC 777 `session=`). A reader on
  a worker thread tails that file (append-only, bounded reads, survives truncation and rotation) and
  folds lines into a model: edits (Edit, MultiEdit, Write, NotebookEdit tool inputs), the task list
  (TodoWrite and the task tools), images and files the agent named.
- The Changes panel lists edited files; selecting one shows the edit as a diff from the tool input
  (old and new strings), not by running git. The Tasks panel shows the current list.
- The reader is generic over agent kinds; Codex support is a later adapter.

### Mods

- `%APPDATA%\Flare\mods\<id>\mod.json`: `id`, `name`, `version`, `protocol: 1`, `permissions`
  (`panel`, `view`, `browser`, `convo`, `tools`), `panel` (an HTML entry served from the mod folder),
  `tools` (tool names and JSON schemas the mod answers through the channel).
- A mod is off until the user enables it in Settings > Mods, which shows its permissions. Enabling
  issues a token; the mod's page gets a `window.flare` bridge bound to that token.
- Mod tools are exposed to the agent through flare-mcp (`mod_<id>_<tool>`), routed to the mod.

## Security rules (new, numbered after the last rule in regression-rules.md at build time)

- S1. The control channel is per user (ACL) and never listens on a network.
- S2. A `view.*` path is resolved again by the backend (rule 13); only files the user could already
  open are shown; runnable files are shown as text or refused, never run (rule 14).
- S3. The agent grant can show content and drive the panel browser. It cannot type into a shell,
  read other tabs' screens, change settings, or enable mods.
- S4. Web panels never use the user's own browser profile. `browser_eval` is off by default.
- S5. A mod gets exactly its granted permissions; a disabled mod's token is dead.
- S6. Downloads from web panels go to the user's Downloads with the WebView2 prompt; no silent saves.

## Testing

- Pure logic (protocol parse, grants, layout maths, transcript fold, manifest parse) has unit tests.
- UI behaviour has `ui_tests` (headless Slint).
- Each stage adds e2e scenarios (offscreen) in `flare-e2e`: open an image panel, drive the channel,
  the MCP round trip, the web panel at 100% and 225% (position matches the slot), browser look/click.
- GPU and WebView2 tests run one at a time (gentle rules).

## Decisions left to the builder (recorded in docs/surfaces/decisions.md as they are made)

- Default panel chord, splitter limits, the exact element snapshot format, how stills are captured for
  airspace, the rate limits.

## Later (issues, not in this build)

Inline pictures in the grid (kitty protocol); timeline scrubbing; "point at this" from a panel to the
agent; screenshot-to-agent hotkey; voice commands; 3D model viewer; Codex transcript adapter;
multiple panels per tab.
