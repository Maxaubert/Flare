# Flare plugins: design

Date: 2026-10-25. Owner: Max. Status: approved for build ("make a plan, review it, then go ahead", 2026-10-25).
Background review: `C:\Users\Admin\Documents\Claude\research\flare\2026-10-25-mod-support.md`.

## 1. Intent

Flare is built for agents, and it should also be open: anyone can write a plugin that adds to the tab bar, the
sidebar, the panels and the session itself, and anyone can download and install one. Today a mod can only show a
page in its own panel and offer tools to Claude. A plugin must be able to change most of what you see and do in
Flare, while Flare stays fast and stable.

The owner's examples, which v1 must make possible (each is a sample or acceptance test, section 11):

1. **Call-me**: turning call-me on for a session puts a phone badge on that tab; off removes it.
2. **To-do across sessions**: a list in the vertical tab panel (or a panel) of things to check per session; when
   the owner tells that session he has looked at an item, it is ticked off and leaves the list.
3. **Browser in use**: while a session drives a browser, its tab shows a globe badge.
4. **Real progress**: a workflow shows an estimated progress bar on its tab that reflects weighted work (9 of 10
   agents done but the last is half the work reads 50 %, not 90 %).
5. **What next**: a panel or sidebar section per session with what remains (for example open GitHub issues).
6. **Inline media**: Claude shows an image or an audio file in the session.
7. **Sonara player**: an audio bar for the speech of a session with a seek bar, rewind and skip.
8. **Session buttons**: buttons on a Claude session such as "Compact" or "Tell it X" that send to that session.
9. **Look changes**: replace a tab's icon (the Claude mark), add an effect to a tab.

## 2. Goals, non-goals, anti-goals

Goals
- One plugin system. Today's mods become plugins (protocol 1 mods keep working unchanged).
- Plugins are downloaded and installed from Settings > Plugins (a zip file or a URL to a zip), turned on and off
  there, updated and uninstalled there. They do not ship with Flare.
- A plugin may ask for anything up to full control (all tabs, typing into sessions, network, its own process). The
  user decides: Flare shows plainly what a plugin asks for before it is turned on, and again when an update asks
  for more. Optional means the user's choice, not Flare's.
- Plugins contribute UI as data; Flare draws it. Rich UI lives in web views that Flare places in fixed slots.
- Claude (skills, workflows) can use the same surfaces for its own tab without any plugin (call-me badge, progress).

Non-goals (v1)
- A public plugin store, ratings, signing or automatic updates. Install is by file or URL.
- Plugins drawing native UI, injecting into Slint, or replacing Flare's own windows.
- A Node/Python runtime inside Flare. A plugin that needs one ships or names its own helper program.
- The Prism Terminal (Electron) port of plugins.

Anti-goals (a failure even if it works)
- A plugin can make Flare slow, hang the UI thread or crash Flare. (Every plugin call is bounded, rate limited and
  off the UI thread; a dead plugin is shown as such and never takes Flare down.)
- A plugin's badge or button can pass for Flare's own agent marks (working, question, failed). Plugin UI has its own
  look.
- A plugin gets more than the user approved, or keeps it after being turned off.
- Writing a plugin needs Rust or a Flare rebuild.

## 3. What a plugin is

A folder with `plugin.json` (protocol 2) or `mod.json` (protocol 1, today's mods, unchanged rules) and web files.
Installed under `%APPDATA%\<Flare profile>\plugins\<id>\` (today's `mods\` folder is read too; same id space).

```json
{
  "id": "todo",
  "name": "Session to-dos",
  "version": "1.0.0",
  "protocol": 2,
  "description": "Things to check per session; ticked off when you tell the session.",
  "author": "someone",
  "homepage": "https://github.com/someone/flare-todo",
  "permissions": ["tabs", "decorate", "sidebar", "sessions"],
  "background": "background.html",
  "panel": { "entry": "panel.html", "title": "To-dos" },
  "contributes": {
    "sidebar": [{ "key": "todos", "title": "To-dos" }],
    "actions": [{ "key": "add", "title": "Add to-do", "icon": "plus", "where": ["tab-menu"] }],
    "icons": { "check": "M4 12l5 5L20 6" }
  },
  "tools": [ { "name": "add_todo", "description": "...", "input_schema": { "type": "object" } } ]
}
```

Protocol 2 keeps every protocol 1 rule (ids, version, unknown keys refused, text cleaning, entry path checks, tool
names and limits) and adds the keys below. Flare refuses a protocol it does not know.

| Key | Meaning |
|---|---|
| `author`, `homepage` | Shown in Settings. Text rules as `name`; `homepage` must be `https://`. |
| `background` | An `.html` entry that runs while the plugin is on, in a hidden view, whatever tab is shown. Needs no permission by itself. |
| `panel` | As today: the plugin's page in a tab's panel slot. |
| `bar` | `{ "entry": "bar.html", "height": 48 }`: the plugin's strip docked under the terminal of a tab (section 5.5). Height 24 to 160. Needs `bar`. |
| `contributes.sidebar` | Sidebar sections (section 5.3). Needs `sidebar`. At most 2. |
| `contributes.actions` | Buttons and menu items (section 5.4). Needs `actions`. At most 8. |
| `contributes.icons` | Named SVG path icons the plugin uses in badges and actions (section 5.1). At most 32, each a path string of at most 2 KB, path-command characters only. |
| `helper` | `{ "command": "helper.exe", "args": [] }`: a program inside the plugin folder Flare starts while the plugin is on. Needs `process`. |
| `tools` | As today. In protocol 2 the background page may answer them (no panel needed). |

## 4. Permissions

A plugin gets exactly what it lists and the user approved (rule S5 kept). Protocol 1 names keep their meaning.

| Permission | Allows | Plain words in Settings |
|---|---|---|
| `panel`, `view`, `convo`, `tools`, `browser` | As today. | As today. |
| `tabs` | See every tab (id, title, folder, agent kind and state, Claude session id) and act on any tab, not only the bound one. Without it a plugin acts only on the tab that shows it. | "See all your tabs and what their agents are doing" |
| `decorate` | Badges, progress, icon and effect on tabs (5.1, 5.2). | "Add badges and progress to tabs" |
| `sidebar` | Its sidebar sections. | "Add a section to the sidebar" |
| `actions` | Its buttons and menu items, and their click events. | "Add buttons to tabs and menus" |
| `bar` | Its strip under a tab's terminal. | "Show a bar under the terminal" |
| `sessions` | Read the conversation of Claude sessions: user prompts, assistant replies, tool calls (from the transcript Flare already reads). | "Read what you and Claude say in sessions" |
| `input` | Type text or a command into a tab's shell (for example `/compact` to Claude). | "Type into your sessions" |
| `network` | Its pages may reach the internet (`connect-src`, `img-src`, `media-src` https), in a networked profile of its own. | "Use the internet" |
| `process` | Flare starts its `helper` program, which runs as the user with full access. | "Run its own program on this PC (full access)" |

Rules
- `process` is full control and Settings says so in those words. `input` + `sessions` together are shown with a note
  that the plugin can read and steer Claude.
- Approval is stored as today (`plugins.json`, the protocol 1 `mods.json` read once and migrated): the permissions
  approved with the switch. An update that asks for more turns the plugin off until approved (regression rule 212 kept).
- `eval` stays the user's own switch and is never a plugin permission.
- Tokens as today: in memory, minted on, killed off. The helper gets its token in the environment variable
  `FLARE_PLUGIN_TOKEN` and the pipe name in `FLARE_CTL_PIPE`, and speaks the same channel as a page.

## 5. Surfaces (what Flare draws for plugins)

Every surface is data a plugin sets through the channel; Flare draws it in Slint with its own look and limits.
Every surface is keyed `(plugin id, key)`; setting the same key replaces, clearing removes. All of a plugin's
surfaces go when it is turned off or its process dies (the helper) or its background page dies, except what the
user put there.

### 5.1 Tab badges

- `tabs.badge.set {tab?, key, icon, tip?, tone?}` and `tabs.badge.clear {tab?, key}`.
- `icon`: a built-in name (`phone`, `globe`, `bell`, `mic`, `eye`, `moon`, `dot`, `lock`, `bolt`, `check`, `star`,
  `plus`, `pin`) or a name from the plugin's `contributes.icons`.
- `tone`: `ink` (default), `accent`, `muted`. Never the agent mark colours.
- At most 3 badges per tab across all sources, oldest dropped from view first (still listed in the tooltip).
- Drawn: horizontal strip, 14 px glyphs before the tab's close slot, hidden when the tab is narrower than 110 px; icon
  tabs show at most one, in the corner opposite the agent ring; sidebar rows show them left of the ring.
- `tab` defaults to the bound tab; another tab needs `tabs`.

### 5.2 Tab progress, icon and effect

- `tabs.progress.set {tab?, key, value?, label?}`: `value` 0 to 1, or absent for an indeterminate bar; `clear`
  removes. One progress per tab is drawn (the last set); a 2 px bar along the tab's bottom edge (strip) or row
  bottom (sidebar), in the accent colour, with the label in the tooltip.
- `tabs.icon.set {tab?, icon}` / `clear`: replaces the tab's leading icon (the agent mark slot keeps its ring and
  state colours; only the glyph changes). One override per tab; the last plugin wins and Settings shows which.
- `tabs.effect.set {tab?, effect}` / `clear`: one of a fixed set Flare implements (`glow`, `pulse`, `tint:<accent|ink>`),
  through `Motion.d` so reduced motion holds. No plugin-supplied animation code in v1.
- Rate limit for all of 5.1 and 5.2: 10 changes a second per plugin per tab; more answers `rate_limited`.

### 5.3 Sidebar sections

- Declared in `contributes.sidebar`; shown in the vertical tab panel under the tab list, one collapsible section
  per declaration, in the order the user drags them (stored in settings).
- Content is a list the plugin sets: `sidebar.set {key, items:[{id, text, sub?, icon?, checked?, tab?}]}`. At most
  50 items, `text` at most 120 characters, `sub` 120. An item with `tab` jumps to that tab on click. An item with
  `checked` draws a checkbox. Clicks and checkbox toggles come to the plugin as events (`sidebar.item`).
- Flare draws it with the sidebar's look; the plugin cannot style it. (Rich layouts belong in its panel.)
- In horizontal tab mode the sections are reachable from a "Plugins" button in the top bar that opens them as a
  dropdown list (same data).

### 5.4 Actions (buttons and menu items)

- Declared in `contributes.actions` with `where`: `tab-menu` (the tab's right-click menu), `tab-hover` (a small
  button on the tab while hovered, at most 1 per plugin), `top-bar` (a button left of the window buttons, at most 1
  per plugin, 3 in total), `session-bar` (in the plugin's own bar).
- `actions.set {key, tab?, enabled?, checked?, title?}` changes state per tab (for a toggle like call-me).
- A click sends `action` `{key, tab}` to the plugin's background page (or helper).

### 5.5 Session bar

- A plugin with `bar` may show its strip under a tab's terminal: `bar.show {tab?}`, `bar.hide {tab?}`. The terminal
  gives up the height (it reflows). One bar per tab at a time; a second plugin asking gets `busy` unless the user
  pinned it.
- The strip is the plugin's web page (sealed or networked as its permissions say), so it can draw a player with a
  seek bar. The Sonara player and inline audio controls live here.

### 5.6 Panels and inline media

- Panels as today, plus: the background page may open its panel in any tab it may act on.
- Inline media (example 6): Claude's own `show_image`, `show_file` and a new `show_audio` already open in the panel;
  "inline" in v1 means the panel or the session bar, never inside the terminal grid (kitty graphics stays issue #35).

## 6. Events (what a plugin hears)

The bridge gains `window.flare.on(name, fn)` for more names (each may be registered once per page), delivered to the
background page, the panel page and the bar page alike. The helper receives the same events as channel
notifications.

| Event | Needs | Payload |
|---|---|---|
| `tool` | `tools` | As today. |
| `tabs` | `tabs` (else the bound tab only) | `{kind: "opened"|"closed"|"focused"|"renamed"|"moved", tab}` |
| `agent` | `tabs` (else bound tab) | `{tab, kind: "claude"|"codex"|null, state: "working"|"question"|"done"|"failed"|"idle", session}` |
| `session` | `sessions` | `{tab, session, role: "user"|"assistant"|"tool", text, tool?, at}`; text capped at 16 KB, one event per transcript entry, at most 50 a second per plugin (older are dropped with a `dropped` count). |
| `action` | `actions` | `{key, tab}` |
| `sidebar.item` | `sidebar` | `{key, item, checked?}` |
| `plugin` | none | `{kind: "on"|"off"|"update"}` to its own pages. |

## 7. Calls (what a plugin asks)

New channel methods, each in a namespace whose grant is the matching permission:
`tabs.list` (all tabs with `tabs`), `tabs.badge.*`, `tabs.progress.*`, `tabs.icon.*`, `tabs.effect.*`
(`decorate`), `sidebar.set` (`sidebar`), `actions.set` (`actions`), `bar.show` / `bar.hide` (`bar`),
`session.read {tab, since?}` (`sessions`: the last 200 entries), `tabs.input {tab, text, enter?}` (`input`; text at
most 4 KB, control characters refused except tab, sent as one paste), `storage.get/set` (none: a small JSON store per
plugin under its data folder, 1 MB, for helpers and pages alike).

## 8. Claude's own surfaces (no plugin)

`flare-mcp` gains, for the caller's own tab only (the agent grant):
- `set_tab_badge {key, icon, tip?}` / `clear_tab_badge {key}`: built-in icons only.
- `set_tab_progress {value?, label?}` / `clear_tab_progress`.
- `show_audio {path}`.
Agent badges and progress are cleared when the tab's Claude session changes or the tab closes. The call-me skill uses
`set_tab_badge {key:"call-me", icon:"phone", tip:"Call-me is on"}`; a workflow reports weighted progress with
`set_tab_progress`.

## 9. Install, update, remove (Settings > Plugins)

- Settings > Mods becomes Settings > Plugins: installed plugins with name, version, author, homepage, the permissions
  in plain words, on/off, Update, Remove, and errors.
- Install from file: a `.zip` (at most 50 MB, at most 2000 entries, no entry outside the root, no links). Install from
  URL: an `https://` link to a zip, downloaded by Flare (same limits) to a temp folder, then the same path. The zip
  holds the plugin folder (or its contents at the root). Flare validates the manifest before anything is copied, shows
  what it asks for, and installs it OFF. Turning it on is the approval.
- Update: install a zip of the same id: the new version replaces the folder after the old one is stopped; more
  permissions turn it off until approved. Remove: stopped, folder moved to the Recycle Bin, approval and storage
  removed.
- Folders dropped into `plugins\` by hand still work (as `mods\` does today).

## 10. Runtime and limits

- Background, panel and bar pages are WebView2 views in the sealed (or, with `network`, a networked) mini-app
  browser, one profile per plugin (`flare-plugin-<id>`), served by the existing per-request `serve` rule.
- At most 6 plugins with a background page run at once (more are refused at enable with a reason). A background page
  that dies is restarted at most 3 times in 5 minutes, then the plugin shows "stopped" in Settings.
- The helper runs in a Windows job object Flare owns, so it dies with Flare. Its stdout and stderr go to
  `<data>\plugins-logs\<id>.log` (rotated at 1 MB). It is restarted like a background page.
- Every channel call from a plugin is answered off the UI thread or in bounded UI work; the UI applies plugin surface
  changes at most once a frame.
- Diagnostics: `plugin` lines in diag.jsonl for start, stop, crash, rate limit and refused calls (no content).

## 11. Acceptance (the owner's examples)

Each example in section 1 has a sample plugin in `sample-plugins/` or a skill change, and an e2e or appctl test:
call-me (skill + agent badge), `todo` (sidebar + sessions + decorate), `browser-badge` (tabs + agent events +
decorate: a globe while the session's tab has a browser panel open), progress (agent tool), `what-next` (panel +
network), inline media (`show_audio`), `sonara-bar` (bar + network or helper), `session-buttons` (actions + input),
`look` (icon + effect).

## 12. Calls made by the builder (owner may overrule)

- One system named "plugins"; mods are protocol 1 plugins.
- Web tech for plugin UI, plus an optional helper program; no embedded Node.
- Plugin UI is declarative where Flare draws it (badges, progress, sidebar lists, actions) and web where the plugin
  draws (panel, bar, background). No plugin web view over the tab strip or the sidebar (airspace and speed).
- No store in v1; install by file or URL.
- Sidebar sections are lists, not free web views.

## 13. Changes after review (2026-10-25, binding; they override the sections above where they differ)

Three reviews (builder check, code fit, safety and speed) ran on this design. Their findings are applied here.

Look and spoofing
- Plugin progress is never at the tab's foot (that edge is the agent's `MarkBar`; its "run" style is the working
  mark). Plugin progress is a small chip in the badge area: an outlined pill with a static fill; no indeterminate
  animation (absent `value` draws a static dotted pill). The agent's own `set_tab_progress` uses the same pill in the
  agent's reserved slot.
- Every plugin badge sits in a Flare-drawn outlined chip, distinct from the agent ring. No built-in `check` or `dot`
  badge; tones never equal the state colours; effects never touch the ring slot or the breathe rhythm; every tooltip
  starts with the plugin's name. Badge and progress tips are merged into the tab's one tooltip (no own TouchArea, so
  tab drags keep working).
- Badge slots: the agent has one reserved slot; each plugin shows at most one badge per tab; at most 2 chips drawn on
  a named tab of 150 px or more, 1 below, none under 110 px or on icon tabs. Each source keeps at most 4 keys per tab
  and 200 in total. Badges are flat fixed fields in the tab row (no nested model, so the row diff stays cheap).
- `tabs.icon.set` only on tabs with no agent kind; on agent tabs it is shown as a badge. First come; a second plugin
  gets `busy`; the previous override comes back when its owner turns off. Amended 2026-10-26 (owner): a plugin with
  `agent-looks` may pick how an agent's icon moves while it works (`agents.motion.*`), a motion from Flare's fixed list,
  never artwork, never a check or dot; the ring and bar stay Flare's (`docs/surfaces/decisions.md`, `plugins.md`).

Compatibility
- Protocol 1 keeps everything: `mod.json`, the `mods\` folder, `mods.json` as the switch store (protocol 2 entries
  are added to the same file), the page key `mods` (the page is titled Plugins), the host and profile
  `flare-mod-<id>` (protocol 2 uses the same prefix, so storage survives). `ASKABLE_V1` stays the old list.
  `plugin.json` must say protocol 2 and `mod.json` protocol 1; anything else is refused. `plugins\` is a second root;
  an id in both roots is refused in the second. Amended by phase 2c (owner, 2026-10-25): one folder, `plugins\`, for
  both protocols; `mods\` is moved into it once at start and no longer read (plan, "Phase 2c").
- Tool routing for protocol 1 and for plugins without a background page is unchanged (panel opened in the caller's
  tab). The e2e `modsSample` stays as it is.
- Views are keyed by slot `{owner, kind}` (panel, bar, background) instead of by tab, as its own phase before
  background pages.

Permissions
- `input` is full control and is shown as "Type and run commands in your terminals (full control)", in the same
  confirm tier as `process`. Regression rules 2 and 188 (S3) are amended for it in the phase that builds it: a typed
  pty intent for plugin input, reachable only with `input`, never by the agent.
- Click scope: a click on a plugin's action or sidebar item on tab X gives that plugin scope on X until the tab
  closes or the plugin turns off, so a plugin needs `tabs` only to act on tabs nobody clicked.
- Combined warnings in Settings: `input`+`sessions`, `network`+`sessions` ("can send what you and Claude say to the
  internet"), `network`+`tabs`. `network` refuses loopback, private hosts, `localhost` and `.local`; CSP adds
  `form-action 'self'; base-uri 'none'`, `frame-src 'none'`. Settings says it "can change what it does from the
  internet".
- Each new method family has its own namespace mapped to its permission; none goes under the open `Tabs` namespace.
  An exhaustive test: a plugin with no permissions may call only `hello` and its own-tab `tabs.list`. Install,
  update, remove and approve are `Ui` only. `storage.*` is refused for the agent. The wider `tabs.list` (all tabs,
  agent kind, state, session) is built in the background-page phase.
- Tab ids given to plugins are never the `FLARE_TAB_TOKEN` secret (test).

Stability and speed
- One limiter per plugin in the dispatcher (on the worker, before posting to the UI thread), shared by its pages and
  its helper: 50 calls a second, 2 MB a second, burst 100, messages at most 256 KB; plus per-call limits: decor 30/s
  per plugin across tabs and 10/s per tab, `sidebar.set` 5/s per key, `actions.set` 20/s, `bar.show`/`hide` and
  panel open 1/s per tab, `storage.set` 2/s (coalesced), `tabs.input` 2/s per tab, `session.read` 1/s per tab. Page
  messages over budget are dropped in the `WebMessageReceived` handler before the copy; JSON parsing moves to a worker.
- Safe mode: two unclean starts in a row with plugins on start the next one with plugins off and a banner; also
  `--no-plugins` and Shift held at launch. Crash counts persist per plugin and version.
- A background page is pinged every 10 s; no answer in 10 s closes it and counts as a crash.
- Events are coalesced and posts capped per frame. Helper notifications are routed by plugin id through the existing
  outbox. At most 4 pipe connections per plugin token.
- Every plugin view (background, panel, bar) is sealed by one function: no script dialogs (auto-dismissed), no
  context menu or browser accelerators for background and bar views, plus every existing wall (new windows,
  downloads, external URIs, permissions, DevTools, frame navigation), with a test per view kind. Background and bar
  views never take focus.
- Views: created lazily for the tab on screen; at most 2 live views per plugin per slot, 12 plugin views in total,
  6 plugin sidebar sections in total, one `tab-menu` group per plugin. Plugins get their own WebView2 environment,
  apart from the web panel and the mini-apps.
- A panel open by a plugin only replaces an empty panel or its own; else `busy`.

Lifecycle and install
- Turning off: every view of the plugin closes and its helper is stopped (job object with kill-on-close, terminated
  and waited for) before it reports off. A generation number per plugin is bumped on off, update and remove and
  checked when a result is applied.
- Settings says plainly that a full-access program can leave things behind outside Flare.
- Approval stores author, homepage, source URL and a content hash. Installing over an existing id shows the
  differences; a change of author, homepage or source turns it off until approved and wipes its profile and storage;
  a downgrade asks.
- Zip install: each entry written by Flare with create-new (never a library extract); refused: links and reparse
  points, encrypted entries, absolute or drive paths, `:`, control characters, `..`, parts ending in a dot or space,
  device names with any extension, case-insensitive duplicates, paths over 200 characters or deeper than 8; real
  decompressed bytes counted (16 MB per entry, 100 MB total, 50 MB compressed, 2000 entries). Extract to
  `plugins\.staging\<rand>`, validate the manifest from the staged copy, check every path, swap folders by rename,
  clean leftovers at start; install, update, remove and scan run on one worker queue. Layout: `plugin.json` at the
  root or exactly one top folder holding it.
- URL install: https on every redirect (at most 5), bytes counted while streaming, 60 s timeout, no cookies, not the
  web panel's profile; the source URL is recorded.
- Remove deletes the WebView2 profile, logs, storage, sidebar order and action state.
- Helper: `.exe` only, spawned by its canonical absolute path, current folder the plugin folder, at most 16 args of
  200 characters, `FLARE_TAB_TOKEN` and `FLARE_RESUME_ID` removed, the plugin token set only on the child.

Agent decor
- Cleared when the session id changes from one id to another, when the agent leaves the tab (its kind becomes none)
  and when the tab closes; a first session id after none does not clear. Codex tabs clear on agent exit and close.
- Not stored: a resumed session's skill sets its badge again. Hidden tabs (+N list) show no badges in v1.
- New MCP tools appear in Claude sessions started after the install (a running session keeps its tool list).

Examples, settled by the builder
- Globe badge: from Flare's own browser panel and the agent's `browser_*` tools (a `panel` field on tab info and a
  `tabs` event kind `panel`). Chrome or Playwright outside Flare would need `sessions` and is not in the sample.
- To-do tick-off: the sample offers a plugin tool (`todo_done`) that Claude calls when the owner says he looked at
  something, plus a `sessions` fallback that matches the item text in the owner's prompts.
- Sonara bar: a helper reads Sonara's `runtime.json` and relays skip, restart and replay. A real seek needs a position
  and seek API in Sonara itself: a follow-up in the Sonara repo.
- Weighted progress: a small workflow helper computes weights (each agent's share, review stages weighted) and calls
  `set_tab_progress`; part of Phase 8.
- Naming in code: `flare_mods::plugin` for Flare plugins, distinct from `flare_agent::plugin` (Flare's own Claude
  Code plugin).
