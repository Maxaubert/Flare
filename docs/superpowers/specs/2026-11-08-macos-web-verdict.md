# macOS phase 1: the web engine verdict (2026-11-08)

The question (Max, 2026-11-08): WKWebView only if agent browsing loses nothing against WebView2; any gap that
cannot be closed means CEF. Spec: `2026-11-08-macos-port.md`, phase 1.

**Verdict: CEF.** One gap cannot be closed through any public WebKit API: a `browser_eval` script that never
returns hangs the page's process for good. That covers the agent's own loop, a page function the agent calls,
and a page getter set as a trap for whatever the agent touches. On Windows, CDP's `Runtime.evaluate` timeout
stops the script and the page answers again. On a Mac the only way out is a new view (a new content process),
so the page and its state are lost (checks `8.eval-runaway` and `B.trap-eval` fail). WKWebView has no call
that terminates, interrupts or time-limits a running script. `stopLoading`, `reload` and `loadRequest` wait
behind the script. Under the rule, that alone means CEF.

Everything else in the agent browsing contract reached parity or has a closure that was tested. If Max waives
the gap above, WKWebView is otherwise viable, and the closures it needs are listed in the tables.

This is the second round. The first round said WKWebView and called the runaway eval a residual. A skeptical
review then found defects: CSP breakage, a trap a page can set, sandboxed frames, proxy bypass, permissions,
missing rows and inflated counts. Every one was tested again here (section "Round 2").

## How it was proved

- `crates/flare-web/examples/wk_spike/` (macOS only, a stub elsewhere): objc2 0.6 / objc2-web-kit 0.3. Run it with
  `cargo run -p flare-web --example wk_spike -- <out dir> [scenario | adv]`; it writes `report.json` and PNG
  stills. `adv.rs` holds the round 2 scenarios; `observers.py` (started by them as a child process) is an mDNS
  sniffer, a raw TCP accept logger, a logging CONNECT proxy and a dual-stack HTTP server.
- **A CDP shim** (`shim.rs`) answers the 11 DevTools Protocol methods `agent_job` asks for (`DOM.enable`,
  `Page.getLayoutMetrics`, `Accessibility.getFullAXTree`, `Runtime.evaluate`, `Page.captureScreenshot`,
  `DOM.getContentQuads`, `DOM.scrollIntoViewIfNeeded`, `DOM.focus`, `Input.dispatchMouseEvent`,
  `Input.dispatchKeyEvent`, `Input.insertText`) from WKWebView, in Chromium's shapes. The real
  `flare_surfaces::agent_job::Job` (the state machine Windows runs) drives it, so trimming, refs, click points,
  key tables and the look's JSON are unchanged.
- **The agent's helper** (`agent.js`) runs in every frame in its own `WKContentWorld` ("flare-agent"): the page
  can neither see nor change it. It numbers nodes, builds the AX tree, measures boxes, focuses, scrolls and
  counts trusted input events.
- **Headless** (screen rule): activation policy Prohibited, the host window is never ordered in, events go
  straight to the view. The window server listed 0 windows of the process at the start and end of every run,
  and `NSApp.isActive` stayed false. One incident: a first permissions probe called the native
  `navigator.clipboard.readText()` with no gesture. The run loop then blocked until the 100 s timeout killed
  it, which matches WebKit's modal Paste menu. Whether a menu was drawn anywhere is not known. The probe was
  changed so native permission APIs are never called (rows below).
- Final run (macOS 27.2, Apple Silicon, 17 scenarios): **54 passed, 2 failed, 19 notes.** Both failures are the
  gap above. Checks that only pass through a closure the shim performs, or that prove nothing in this binary,
  are notes now, not passes (`2.press-editing-keys`, `8.permissions`). Evidence (temporary):
  `/private/tmp/claude-501/-Users-maxaubert/1b6247aa-2e12-4c0d-86a6-6f54e0401ad7/scratchpad/wkspike-final/`
  (`report.json`, PNG stills, `netlogs/`).

Result key: **parity** (same outcome as Windows, tested), **closable** (a gap with a closure; "tested" when the
closure ran in the spike), **open** (not tested, with why), **cannot close** (no public API).

## The agent browsing tools

| Capability | Windows (WebView2) | WKWebView | Tested | Result |
|---|---|---|---|---|
| `browser_look` picture | `Page.captureScreenshot` (clip, scale) | `takeSnapshotWithConfiguration`, NSImage to PNG | yes: 1600x1138 PNG for a 983x700 viewport at dpr 2, same `shot_scale`; zoom 1.5 gives dpr 3 | parity |
| Picture of a page not on screen | none (`HIDDEN_NOTE`) | a snapshot of a never-shown window is a fresh paint | yes: `look-window-not-visible.png` | better |
| `browser_look` numbered list | `Accessibility.getFullAXTree` + `DOM.getContentQuads` | JS AX snapshot in the agent world, same JSON, same `candidates()` | yes: test page 29 elements; Wikipedia 600 (cut), GitHub 187, Hacker News 454 | closable: names approximate Chromium's accname (an image-only link comes out unnamed). Plan: port a full accname computation and diff against Windows looks |
| Look speed | one CDP call per element, sent together | one script call per element box | yes: 1.4 s (187) to 4.4 s (600) | closable: boxes from the snapshot call (one round trip per frame) |
| The look's fixed question (`devicePixelRatio`) on a hostile page | `"timeout": CHECK_TIMEOUT_MS`: a trap costs the ratio, not the look | asked in the agent's world, where the page's getters do not exist | yes (`B.trap-fixed-questions`): a page whose `devicePixelRatio` getter loops forever; two looks in 34 ms each, ratio 2.0, a wait met. Before the fix: the first look came back empty after 6 s and the second failed | parity (tested) |
| Strict CSP pages (look ratio, wait, eval) | CDP is not bound by the page's CSP | the old `(0, eval)(src)` route was refused (EvalError); now no string is eval'ed: fixed questions run in the agent world, `browser_eval` through `callAsyncJavaScript` / `evaluateJavaScript` | yes (`A.csp-eval-wait`, `A.github-wait-eval`): local `script-src 'self'` page and github.com/login: wait met, eval of a value, a promise, statements and a throw, device_scale 2.0 | parity (tested) |
| Iframes in the list (scripted frames) | same-process frames (`docs/surfaces/lessons.md`, "Frames the browser puts in another process") | every frame: the parent's agent world probes each child for its `WKFrameInfo` | yes: same-origin and cross-origin frames listed and usable | parity or better (WebKit lists cross-origin frames too) |
| Iframes with `sandbox` and no allow-scripts | listed if same process; whether Chromium isolates opaque sandboxed frames in WebView2 was not checked | WebKit runs no user-script listeners there, so the probe goes unanswered. But `evaluateJavaScript` in the agent world with the navigation delegate's `WKFrameInfo` works (the page world is refused: "Cannot execute JavaScript in this document"). The helper is put in that way and the frame is paired with its iframe by viewport size, in document order | yes (`F.sandboxed-iframe-listed`): a `sandbox="allow-same-origin"` frame and an opaque `sandbox` srcdoc frame both listed; a ref click on the srcdoc frame's link navigated that frame | closable (tested). Caveat: the pairing by size is a heuristic; two unscripted frames of the same size whose load order differs from document order could swap |
| Closed shadow roots | in Chromium's AX tree | invisible to an isolated world by default | yes: with a page-world document-start shim, listed and clickable, the page still sees null | closable (tested) |
| `browser_click` (left, by ref and by point) | `Input.dispatchMouseEvent` | CGEvent-built NSEvent into the view | yes: trusted mousedown/mouseup/click; a point click checks a checkbox | parity |
| Pointer and hover events | trusted pointer events, `:hover` | the same from the NSEvent path | yes (`C.hover-*`, `C.pointerdown-*`): trusted pointerover/enter/down/up, `:hover` true, a CSS hover menu opens, a pointerdown-only control opens; with the window key, not key, and not key and not visible | parity |
| Double and middle click | clickCount 2, button middle | click state 2, `otherMouseDown:` | yes: trusted dblclick (detail 2), auxclick (button 1) | parity |
| Right click | button right | `rightMouseDown:` opens WebKit's on-screen context menu | **no** (screen rule) | open: empty the menu in a subclass's `willOpenMenu:withEvent:`. Untested |
| `browser_type` | `DOM.focus` + `Input.insertText` | agent-world `focus()` + `insertText:replacementRange:` | yes: accents, CJK, emoji, Hebrew, contenteditable, textarea newline; trusted beforeinput/input | parity |
| `browser_press` | `Input.dispatchKeyEvent` with VK codes | NSEvent `keyDown:`/`keyUp:` with a VK to Mac key code table | yes: End, Backspace, arrows, Enter, Escape, PageDown, F5, Delete, Home, Shift+Tab, Shift+B, Control+a arrive trusted with Windows' `key`/`code` | parity |
| Command chords (Meta+a, c, x, v, z) | the renderer runs Ctrl+A etc. itself | AppKit runs them through the menu bar, not the page | yes: without help nothing is selected; the shim calls `selectAll:` (likewise copy/cut/paste/undo) when the page did not cancel the keydown | closable (tested; `2.press-editing-keys` is a note because it passes only through this closure) |
| Control+a meaning | select all | line start (Emacs binding) on a Mac; the Windows `browser_type`/`browser_press` texts tell agents to use Control+a | yes (the keydown) | closable: map the agent's editing chords to Command on a Mac, or a per-OS tool text. A design choice, not an engine limit |
| Input while Flare is in the background | works | works: `document.hasFocus()` false, typing, keys and clicks land | yes | parity |
| `browser_scroll` | `mouseWheel` | pixel scroll-wheel CGEvent into `scrollWheel:` | yes: scrollY 0 to 600 for dy 600, trusted wheel | parity |
| `browser_wait` (text, selector, pause, timeout) | polls `Runtime.evaluate` | the same templates, run in the agent world | yes: slow navigation (2.3 s), selector, 300 ms pause, unmet text `met: false`, a strict-CSP page, github.com/login, a trapped page | parity (tested) |
| Load state, `browser_navigate`, same-document changes, `browser_state` | WebView2 events and calls | navigation delegate, `loadRequest`/`goBack`/`goForward`/`reload`/`stopLoading`, KVO on `URL` and `title` | yes: event list, statuses 200 and 404, stop -999, pushState | parity |
| `browser_eval` | `Runtime.evaluate` (returnByValue, awaitPromise, timeout) | expression: `callAsyncJavaScript` `return await (expr)`; statements: split in the agent world into statements (run as a script by `evaluateJavaScript`, so `var` makes a global as with CDP) and the last expression (awaited); no string is eval'ed | yes (`8.eval`, `A.csp-*`): a title, an awaited promise with an object, a throw, statements, a statement script ending in a promise (7), a last expression across lines, a global kept, a block's completion value, a `;` inside a string, CSP pages | parity (tested) |
| Runaway `browser_eval` (`while(true){}`, a page function that never returns) | the CDP timeout terminates the script; the page lives on | no public way to stop a script: the call times out (10 s), the page stays hung, `stopLoading` and reload do not help; a new WKWebView gets a new process (the page is lost); the hung process keeps spinning until killed (its pid is private API) | yes (`8.eval-runaway`: fails, as it should) | **cannot close** |
| A page trap for `browser_eval` (a getter that never returns) | as above: the page lives on | as above; after it, the agent's own world stops answering too (one main thread), so the next look fails after 10 s | yes (`B.trap-eval`: fails) | **cannot close** (the same gap, set by a hostile page) |
| Stop on the person's input ("Claude is using this page") | key hook + polling the real mouse buttons | local `NSEvent` monitor (sees what NSApp dispatches, never the agent's events); WebKit's re-sent keys recognised by object and timestamp; a tag in `kCGEventSourceUserData`; `NSEvent.pressedMouseButtons` | with simulated events (posted into NSApp's queue) only; real hardware input would need a person or `CGEventPost`, which moves the real pointer | parity (simulated). The timestamp rule is unproven against real input |
| App chords taken before the page | `AcceleratorKeyPressed` | the same monitor returns nil for the app's chord | yes: Command+T swallowed, the page saw nothing | parity |
| Iframes: read and click inside | coordinates in the main viewport | `evaluateJavaScript:inFrame:inContentWorld:`; clicks at offset points | yes: both frames clicked, typed into the cross-origin input | parity or better |
| Downloads | `DownloadStarting` with a deferral | `WKDownload`, destination held until the yes, `progress` | yes: held 500 ms, 4 MiB saved, progress 0 to 100 %; a no cancels | parity |
| `select` dropdowns | Chromium popup widget | a native NSMenu (modal) | **no** (screen) | open: CEF on a Mac uses native menus too, so it does not separate the engines |

## The mini-app sandbox, mod and plugin views, and the walls (rule 199)

| Capability | Windows | WKWebView | Tested | Result |
|---|---|---|---|---|
| App files | `SetVirtualHostNameToFolderMapping` | `WKURLSchemeHandler` on `flare-app://app/` | yes: own file served; another app and `../` refused | parity (origin changes) |
| Custom scheme a secure context | (https) | `isSecureContext` true, `crypto.subtle` | yes | parity |
| No network | request filter + dead `--proxy-server` | `WKContentRuleList`: block `.*`, let `flare-app://app/`, `blob:`, `data:` through | yes: fetch, XHR, WebSocket, EventSource, sendBeacon, img, CSS, prefetch, iframe, worker fetch, top navigation | parity |
| No network, the hard channels | as above | the same rule list, with and without the app CSP | yes (`D.exfil-*`): preconnect, dns-prefetch, `.local` images and fetch, ping, preload, modulepreload, `@import`, fonts, media, object, embed, svg, srcset, srcdoc, form, `import()`, worker URL: 0 server hits, 0 TCP accepts, 0 mDNS queries. The control without walls leaked 15 requests, a TCP preconnect and 3 mDNS queries | parity (tested) |
| Networked apps, mod and plugin views (netproxy) | `--proxy-server` + `--proxy-bypass-list=<-loopback>`: everything goes to the proxy | `proxyConfigurations` (HTTP CONNECT, macOS 14+) alone sends address literals and `localhost` straight past the proxy (`E.proxy-only`: 14 direct hits: 127.0.0.1, 127.1, 2130706433, 0x7f000001, 0177.0.0.1, `[::1]`, `[::ffff:127.0.0.1]`, LAN 192.168.0.47, `localhost`, `localhost.`, and ws:// to 127.0.0.1, `[::1]`, the LAN address, localhost). Closure: the proxy plus a rule list that blocks address literals (loopback, 0/8, private ranges, link-local, CGNAT, every IPv6 literal, `localhost` names; dotted only, since WebKit has already normalised odd spellings) | yes (`E.proxy-and-rules`): 0 direct hits for all of the above, http and ws; names still reach the proxy (`localtest.me`, `127.0.0.1.nip.io`, `maxs-mac-mini.local`, example.com), so it keeps its rebinding check | closable (tested). The same rules and proxy go on mod and plugin views (`modview::net_browser_arguments`) |
| WebRTC | `NO_WEBRTC_SCRIPT` + IP policy flag | the same script, page world, document start, all frames | yes: `RTCPeerConnection` undefined, also in a fresh about:blank frame; 0 UDP packets | parity |
| No file:// | not served | blocked | yes | parity |
| Isolation from the web panel | separate profile | separate non-persistent `WKWebsiteDataStore` | yes | parity |
| Mod and plugin pages on `https://flare-mod-<id>.local` | intercept https | https cannot be intercepted (`handlesURLScheme("https")`) | yes (the API answer) | closable: a `flare-mod://<id>/` scheme; mods written against an https origin need the new origin |

## Everything else the Windows host uses

| Capability | Windows | WKWebView | Tested | Result |
|---|---|---|---|---|
| Script at document start | `AddScriptToExecuteOnDocumentCreated` | `WKUserScript` (page or named world, all frames) | yes | parity |
| Page to host, host to page messages | `WebMessageReceived`, `PostWebMessageAsJson` | `WKScriptMessageHandler` with the sender's `WKFrameInfo`; `evaluateJavaScript` | page to host yes; a bridge round trip not | closable |
| Dialogs | `ScriptDialogOpening` | `WKUIDelegate` alert/confirm/prompt | yes: answered, no window | parity |
| `beforeunload` (the seal dismisses the leave-page question) | `ScriptDialogOpening` (kind beforeunload) | no public delegate; WebKit's default lets the navigation go | yes (`H.beforeunload`): after a trusted click, a link click and a host navigation both left the page; no dialog, 0 windows | parity (tested) |
| `window.close()` (ignored, the view stays) | `WindowCloseRequested`, nothing done | `webViewDidClose:`, nothing done | yes (`H.window-close`): the delegate was called once, the page still answered a look | parity (tested) |
| File upload picker | the OS dialog | `runOpenPanelWithParameters`: the host answers | yes | parity or better |
| New windows | `NewWindowRequested` into the same view | `createWebViewWithConfiguration` returns nil, loads in the same view | yes | parity |
| External schemes | cancel | `decidePolicyForNavigationAction` cancel | yes: `mailto:` refused | parity |
| Permissions: camera and microphone | `PermissionRequested`: deny all | `requestMediaCapturePermissionForOrigin`: deny | no: in this unbundled binary `navigator.mediaDevices` is undefined, so the delegate is never asked (`8.permissions` is a note) | open: retest in the signed `.app` (phase 8) |
| Permissions: clipboard read, location, notifications | deny all | no public delegate (WebKit shows its Paste menu; location goes to a system prompt). Closure: a page-world document-start script in every frame makes `Clipboard.read/readText`, `Geolocation.getCurrentPosition/watchPosition` and `Notification.requestPermission` answer "denied", locked (non-writable, non-configurable) | the closure yes (`G.permissions-denied`): after a trusted click, all three denied in the main frame, a fresh about:blank frame and a srcdoc frame, nothing restorable, 0 windows. The native behaviour was not run (screen; see the incident above) | closable (tested) |
| Console capture | not used | page-world hook + message handler | yes | n/a |
| Zoom | `SetZoomFactor` | `pageZoom` | yes | parity |
| Process failure | `ProcessFailed` | `webViewWebContentProcessDidTerminate` | yes | parity |
| Profiles | `SetProfileName` / InPrivate | `dataStoreForIdentifier` (macOS 14+) / non-persistent | yes | parity |
| DevTools | `SetAreDevToolsEnabled` | `isInspectable` | the flag | parity for the setting |
| Full screen | `ContainsFullScreenElementChanged` | off by default; `elementFullscreenEnabled` + KVO | **no** (screen) | open (web panel, not agent browsing) |
| Focus events | `GotFocus`/`LostFocus` | first-responder changes | no | open, small |
| Default context menu off | `SetAreDefaultContextMenusEnabled` | subclass `willOpenMenu:withEvent:` | no (screen) | open |
| Password autosave and autofill | WebView2 settings | no general public API | no | open (web panel, not agent browsing) |
| Content process id (e2e probe) | `BrowserProcessId` | private `_webProcessIdentifier` only | used for the kill tests | e2e only |

## Round 2: the review's defects and what happened to each

1. **CSP broke `browser_wait`, `browser_eval` and the look's ratio.** True (the old route's EvalError is in the
   report). Fixed in the shim: nothing is eval'ed. Tested on a local CSP page and github.com/login. The leftover
   (a statement script ending in a promise) is closed by splitting the script in the agent world. Now parity.
2. **A hostile page can trigger the runaway.** True. The look's and the wait's fixed questions now run in the
   agent world (tested on the trap page). `browser_eval` must run in the page's world, and no public API stops
   it. **This decides the verdict: CEF.**
3. **Sandboxed iframes were missing.** True. Closed by `evaluateJavaScript` in the agent world with the
   delegate's `WKFrameInfo`, plus pairing by size (tested, with the caveat above).
4. **More bypasses the proxy.** True: 14 kinds of direct hit, including ws and IPv6. Closed by the
   address-literal rule list (tested: 0 direct, names still proxied).
5. **The permissions row passed vacuously.** True. It is a note now. Clipboard read, location and notifications
   have a tested closure; camera and microphone stay open until the signed `.app`.
6. **`window.close()` and `beforeunload` rows were missing.** Added; both are parity (tested).
7. **Stop-on-user-input was tested on simulated events only.** Relabelled "parity (simulated)".
8. **The pass count was inflated.** `2.press-editing-keys`, `8.permissions` and the runaway check no longer
   count as parity passes. The runaway check now fails, as it should.

## If it is CEF: what phase 5 must prove on a Mac

- Windowless (off-screen) rendering, headless under the screen rule, with CDP via `SendDevToolsMessage` answering
  the same 11 methods, so `agent_job` runs unchanged. Also, `Runtime.evaluate`'s timeout must stop a runaway
  script there too.
- Request interception (`CefResourceRequestHandler`) for the mini-app, mod and plugin walls, re-proved with
  `observers.py` and the `adv-exfil` / `adv-proxy` pages.
- Native `<select>` popups and context menus (Chromium uses native menus on a Mac too), trusted input into an
  off-screen browser, and the stop-on-input monitor.
- Packaging: the CEF framework and its helper apps signed and notarized inside the bundle (phase 8), and the
  download size.

## Open items for Max

1. **The decision.** Under the rule it is CEF. WKWebView is viable only if Max waives the runaway-eval gap: on a
   Mac the agent gets a timeout and the view is replaced, so the page's state is lost where Windows keeps it.
   A page can set this up on purpose. If waived, the closures in the tables are the phase 5 work list.
2. **Not tested because they would take the screen:** right click (context menu), `<select>` popups, element
   full screen, native permission prompts, real hardware input for stop-on-input. These need the CI macOS
   runner or a person.
3. **WKWebView only:** the fidelity of the numbered list (an accname port and a side-by-side diff with Windows),
   the batched look, Command chord mapping, the closed-shadow shim, the mod scheme origin.
