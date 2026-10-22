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
| WebView2 child HWND over the panel slot | `WebContentsView` in the window, `setBounds` to the placeholder's rectangle. The SAME airspace problem: the view is composited above the main page, so HTML overlays are covered too. Hide it and show a `capturePage()` still while any overlay is open (`lessons.md`, stage 3) |
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
| Airspace: one property `overlay-open` from a closed list of 15 overlays, a pure phase machine (`step`, `captured`), a still taken BEFORE the window hides | One `overlayOpen` computed in the renderer from a closed list, the same state machine (it is plain logic in `flare_surfaces::web`), `capturePage()` before `setVisible(false)` |
| `AcceleratorKeyPressed` decides at once whether the app takes a key; the chord table less Ctrl+F and the agent guard; a second Escape within 600 ms hands the keys back | `before-input-event` on the view's `webContents` with `preventDefault()`; the same chord set and the same double Escape |
| `SetFocus(host)` on every pointer press on the host; the page taking the keys is not the window losing them | `mainWindow.webContents.focus()` and `view.webContents.focus()`; check `blur` is reported when the page had the focus |
| User data folder `<profile>\WebView2` or `%LOCALAPPDATA%\Flare\WebView2`, shared by all tabs, never Edge's | `session.fromPartition('persist:flare-web')` under the app's own profile (never `defaultSession`, never `partition: ''`) |
| Every permission denied; `window.open` and `target=_blank` go to the same panel; only http, https and about:blank | `session.setPermissionRequestHandler((_, __, cb) => cb(false))` and `setPermissionCheckHandler`; `setWindowOpenHandler` returns `{action: 'deny'}` and loads the URL in the view; `will-navigate` and `will-redirect` check the same scheme set |
| Downloads: held by `DownloadStarting` until Flare's own question ("Save this file?", file and host, Save or Cancel) is answered; a no, a closed panel or a closing window cancels (S6) | Electron saves silently to Downloads unless a `will-download` handler stops it: pause the item, show the SAME question (a dialog in `dialogs/` with a test pinning its text and buttons), then `item.setSavePath(<Downloads>/<name>)` or `item.cancel()`. Not `setSaveDialogOptions`: the two apps must ask the same thing |
| The web views are closed in `finish()` before the window hides (`WebCtl::close_all`), pending calls answered `unavailable` | `view.webContents.close()` for every view and `removeChildView` in `before-quit`/the window's `close`, before the window is destroyed; answer the pending calls first (rule 28) |
| A capture that does not answer within 250 ms is ended by a timer and the page is hidden without a still | `capturePage()` on a busy or hung renderer may never resolve: race it against a 250 ms timer, then `setVisible(false)` with no picture; drop the late result by token |
| A process failure is judged by kind: a lost renderer ends that panel (window off, note shown, keys back), a lost browser process ends every panel and the environment; GPU, utility and frame exits only log | `webContents.on('render-process-gone', (_, d) => d.reason)` and `app.on('child-process-gone')`: treat `crashed`, `oom` and `killed` of the main renderer as fatal for that view, and recreate the view, never reuse it |
| `browser.open` and `browser.navigate` answered on the completion of the navigation they started (the browser's navigation id) | `did-start-navigation` / `did-redirect-navigation` bind the call, `did-finish-load` / `did-fail-load` settle it; never on a load that began before the call |
| `flare_surfaces::web::normalize_url` and `navigation_allowed` (pure, with tests) | Port the two functions and their tests as they are (the `URL` class parses; the rules are the same: no login, no space, only http/https/about:blank, a bare `localhost` gets `http://`) |
| Test seams `WEB_ROOT` (virtual host `flare-test.local`) and `WEBVIEW_RUNTIME=missing` | `protocol.handle` / `session.protocol` for a test host; an injected runtime check is moot, test the missing-runtime message with the string alone |
| An agent call is a pure state machine (`flare_surfaces::agent_job::Job`): `start(op, refs)`, `on_reply(id, result)`, `on_tick(now)`, answers `Calls` / `Idle` / `Done`; the window owns the browser, the clock and the timer | The same class in TypeScript (it needs no Electron): `debugger.sendCommand(method, params)` returns a promise, so each `Out::Calls` batch is `await Promise.all`, joined by call id. Port the tests with it (a scripted browser answering from a table). One job per tab; a second call is refused, not queued |
| `CallDevToolsProtocolMethod` for look, click, type, press, scroll, wait, eval; no port opened | `webContents.debugger.attach('1.3')` on the view's `webContents` (detach on close), then `sendCommand`. The CDP methods and params are the same, so `agent_web.rs` (tree trim, quads, key table, wait template) ports as it is. Attach once per view, not per call |
| `DOM.enable` first; `DOM.getContentQuads` (not `getBoxModel`) per element; `DOM.scrollIntoViewIfNeeded`; `DOM.focus` | The same calls. The DOM agent refuses node calls until `DOM.enable` |
| Screenshot: `Page.captureScreenshot` with `clip` (the visual viewport in DOCUMENT space, `pageX/pageY` added) and `scale`, at the display's own resolution, 1600 wide at most; the real scale read from the PNG header | The same call over `debugger.sendCommand` (not `capturePage()`, which cannot clip the way the scale needs and whose size follows the window). Ask `window.devicePixelRatio` with one fixed script; read the width from the IHDR (`png_size_b64`) |
| Refs: `e<n>`, document order after the box-less are dropped, cleared on `NavigationStarting`, a takeover and the next look | Clear on `did-start-navigation` (main frame, not same-document) and on `did-navigate`; the numbering is the trim's, so port `candidates` unchanged |
| The ribbon is in the HEAD of the panel over the title, so the page never changes size for it | A DOM bar over the panel's header in the main renderer (the view is below the header, not under it): do not put a row between the header and the view |
| Takeover: keys through the accelerator hook the panel already has; mouse buttons by polling `GetAsyncKeyState` + `WindowFromPoint` on the 100 ms timer; the agent's own key presses marked while in flight; an e2e request stands in for the person | `before-input-event` on the view's `webContents` for keys (a DevTools-dispatched key did NOT reach Flare's hook in WebView2 155; check whether Electron's does and mark the agent's presses either way). For the mouse, Electron has no global button-state read: use `webContents.on('input-event')` or `before-mouse-event` if the version has it (it reports the person's pointer input, not the agent's CDP events), else treat a pointer move the agent did not make over the view during a job as the takeover. The agent's CDP mouse events are `isTrusted` in the page, so a listener in the page cannot tell them from the person's |
| `flare.web.agentEval` ("Let agents run scripts in pages", off) read from the pipe's threads at every request through the preference store | Read the setting in the main process on every `browser.eval` request (the pipe server lives there); the grant is checked per request, never cached at `hello` |
| `flare-mcp`'s `browser_*` tools and the page-content wrapper (clean, quote, markers) | The same binary serves both apps; if PT ships its own server, port `browser.rs` with its tests (the hostile page cases are the contract) |

Stage 1 detail: `decisions.md` (Stage 1 and its review fixes) and `lessons.md` (Stage 1).
Stage 2 detail: `decisions.md` (Stage 2), `lessons.md` (Stage 2) and `protocol.md` (the final contract of the channel and the tools).
Stage 3 detail: `decisions.md` (Stage 3), `lessons.md` (Stage 3) and `protocol.md` (`browser.open`, `browser.navigate`, `panel.open`, the address rules and the wait).
Stage 4 detail: `decisions.md` (Stage 4), `lessons.md` (Stage 4) and `protocol.md` (`browser.look` and the other agent calls, refs, the `interrupted` error, the MCP browser tools and how a look is written for the model).
