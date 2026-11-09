# Plugins

A plugin is a folder that adds to Flare: badges and progress on tabs, sections in the sidebar, buttons on tabs and in
menus, a bar under a tab's terminal, a panel beside it, a hidden background page, tools Claude can call, and, if you
allow it, a program of its own. It runs on your PC and gets exactly the permissions its manifest asks for and you
approve. This file is the contract, for plugin authors and for the Prism Terminal port, which must keep the same rules.

- Samples: `../../sample-plugins/` (`manifest-tour`, `browser-badge`, `todo`, `what-next`, `sonara-bar`,
  `session-buttons`, `look`, `agent-moves`). Every folder there loads as a valid plugin in
  `flare_mods::discover::tests::the_sample_plugins_are_valid`, so a sample that breaks a rule fails the build.
- Protocol 1 mods (`mod.json`, the older format) keep working unchanged; their own notes, the stage 8 rules this file
  builds on and the port's test vectors are in `mods.md`. Every rule there holds for a plugin too, unless this file
  says otherwise.
- The control channel the bridge speaks, method by method: `protocol.md`.
- Design: `../superpowers/specs/2026-10-25-plugin-system-design.md` (section 13 is binding where it differs); plan:
  `../superpowers/plans/2026-10-25-plugin-system.md`. The Rust reference: `crates/flare-mods` (manifest, discover,
  enabled, hub, bridge, helper, install, fetch), `crates/flare-ctl` (grants, methods, decor), `crates/flare-tabs`
  (`decor`).
- **Status (2026-10-26):** plugins phases 1 to 8 are built: everything below works in Flare, and so do the agent icon
  motions (`agents.motion.*`, owner decision 2026-10-26). Not built: a tab's own icon and effects (`tabs.icon.*`,
  `tabs.effect.*` in design 5.2), the cap of 4 pipe connections per plugin token, a
  plugin store, signing and automatic updates.
- **Brand, details sheet and choices (2026-11-08):** `brand` in the manifest, the details sheet with the person's own
  choice per permission, the centred questions and the flat tab-menu item (see "Brand", "The details sheet" and the
  rules below). Spec: `../superpowers/specs/2026-11-08-plugin-brand-and-sheet.md`.
- **Fill (2026-10-28):** the `fill` permission (`browser.fill`, `browser.fill.page`, `secret.ask`), tool scope, and tool
  calls answered by a helper (see "Filling logins"). Design: `../superpowers/specs/2026-10-28-plugin-fill.md`.

## A plugin in two minutes

1. Make a folder whose name is the plugin's id, with a `plugin.json`:

   ```json
   {
     "id": "hello-badge",
     "name": "Hello badge",
     "version": "0.1.0",
     "protocol": 2,
     "description": "Puts a star on every tab where Claude is working.",
     "permissions": ["tabs", "decorate"],
     "background": "background.html"
   }
   ```

2. Write `background.html`: a page that runs, hidden, while the plugin is on, and talks to Flare through
   `window.flare` (see "The bridge"). Keep scripts and styles inline or in the folder.
3. Zip the folder and install it from Settings > Plugins ("Install from file", or an `https` address), or copy the
   folder into `%APPDATA%\Flare\plugins\`. It arrives off; Settings shows what it asks for in plain words, and turning
   it on is your approval.

## Where plugins live and when they run

- **Folder:** `%APPDATA%\Flare\plugins\<id>\` (the profile's `plugins` folder), the one folder for protocol 1 and
  protocol 2 alike. The folder name is the id. A folder holds either `plugin.json` (protocol 2) or `mod.json`
  (protocol 1), never both. Settings > Plugins has an "Open plugins folder" button.
- **Off until you turn it on.** A plugin found in the folder, installed from a zip or updated to ask for more is off
  until you switch it on in Settings > Plugins, which shows its permissions first. The switch and what you approved are
  kept in `%APPDATA%\Flare\mods.json` (the name stays from protocol 1): the switch, the approved permissions and, for
  a plugin, its `author`, `homepage`, `source` and content `hash` (`sha256:` over the folder's files).
- **On means a token.** Turning it on mints the plugin's token (256 random bits, in memory only); turning it off kills
  it at once, closes every view of the plugin, stops its helper and takes away everything it drew. A restart mints new
  tokens for the plugins still on.
- **Safe mode.** Two starts in a row that end without closing properly while plugins are on make the next start keep
  every plugin off, with a row at the top of Settings > Plugins. `--no-plugins` and Shift held at launch do the same for
  one start. The switches are untouched (`<profile>\plugins-run.json` keeps the record).
- **Flare fetches no plugin by itself.** There is no store and no signing; you install by file or address.

## The manifest (`plugin.json`)

One JSON object, at most 64 KB, `"protocol": 2`. Unknown keys are refused at every level, so a typo is an error, never
ignored. Every refusal is one sentence that names the field, shown in Settings > Plugins for the folder. Text a person
reads (names, titles, descriptions, tips) refuses control characters and hidden Unicode format characters (zero-width
spaces, bidi overrides, the BOM).

| Key | Rule |
|---|---|
| `id` | Required. 3 to 40 of `a-z`, `0-9`, `-`; not a Windows device name; equal to the folder name. |
| `name` | Required. Not blank, at most 80 characters, one line. |
| `version` | Required. A semantic version (`1.0.0`, `1.0.0-rc.1`), no leading zeros. |
| `protocol` | Required. `2` in a `plugin.json` (a `mod.json` says `1`; anything else is refused). |
| `description` | Required. Not blank, at most 500 characters. |
| `author` | Optional. One line of at most 80 characters. |
| `brand` | Optional. `{"color": "#175ddc", "icon": "M..."}`: `color` is `#rrggbb` with at least 3:1 contrast against white (its buttons carry white words); `icon` (optional) an SVG path on the 24-unit box with the `contributes.icons` character and number rules, at most 8 KB and 1024 commands, drawn FILLED in `color` on a white rounded tile. See "Brand". |
| `homepage` | Optional. `https://` and a host, no space or user name, at most 200 characters. |
| `permissions` | Required (may be `[]`). Each name once, from the table under "Permissions". |
| `background` | Optional. An `.html` file in the folder: the hidden page that runs while the plugin is on. Needs no permission. |
| `panel` | Optional. `{"entry": "panel.html", "title": "To-dos"}`, the plugin's page in a tab's panel. Goes with `panel`, both ways. |
| `bar` | Optional. `{"entry": "bar.html", "height": 48}`, the strip under a tab's terminal, height 24 to 160. Goes with `bar`, both ways. |
| `contributes.sidebar` | At most 2 `{key, title}`; keys 1 to 24 of `a-z0-9-`, unique. Goes with `sidebar`, both ways. |
| `contributes.actions` | At most 8 `{key, title, icon?, where}`; `where` lists places from `tab-menu`, `tab-hover` (one action per plugin), `top-bar` (one per plugin) and `session-bar` (needs a `bar`). `icon` is a built-in badge or one of `contributes.icons`. Goes with `actions`, both ways. |
| `contributes.icons` | At most 32 `name: path`. A name is a key that is not a built-in badge; the path is an SVG path on the 24-unit box, drawn as a stroke in the chip's one colour: it starts with `M` or `m`, is at most 2 KB and 256 commands, every number within -1000 to 1000, and holds path commands, numbers, signs, dots, commas and spaces only. |
| `helper` | Optional. `{"command": "bin\\helper.exe", "args": []}`: an `.exe` in the folder, at most 16 arguments of 200 characters. Goes with `process`, both ways. |
| `tools` | Optional. At most 32 tools Claude can call (below). Needs `tools`, and a background page, a panel or a helper to answer them. |

Every path (`background`, `panel.entry`, `bar.entry`, `helper.command`) is checked three times: its syntax when read
(no `:`, no leading `/` or `\`, no `..`, `.` or empty part, at most 200 characters, `.html`/`.htm` or `.exe`), its
canonical place on disk when the folder is scanned (a link or junction that points out is refused, and the file must
exist), and again on every request a page makes or when the helper starts.

**Built-in badge icons:** `phone`, `globe`, `bell`, `mic`, `eye`, `moon`, `lock`, `bolt`, `star`, `plus`, `pin`. There
is no `check` and no `dot`: those would pass for Flare's own agent marks.

**Tools.** `{"name", "description", "input_schema"}`: `name` 1 to 40 of `a-z0-9_`, unique; `description` (what Claude
reads to decide when to call it) at most 500 characters; `input_schema` a JSON Schema object whose `type` is `object`,
at most 8 KB. Flare checks the shape only: the page checks the input. Claude sees the tool as
`mod_<id with - as _>_<name>` (`look` with `set_steps` is `mod_look_set_steps`), which must fit in 52 characters; a
name that is too long, or two that map to the same name, are left out and named in Settings. `flare-mcp`
lists the tools at every `tools/list` and tells the client when the set changes (`notifications/tools/list_changed`),
so a plugin turned on or off reaches a running session that lists again.

## Brand

A plugin's `brand` dresses only the surfaces the plugin owns: its card's logo in Settings > Plugins, its details sheet,
its questions (`secret.ask`, the fill approval) and its item in the tab menu. Never Flare's own chrome, menus or
dialogs. With a brand those surfaces use one fixed dark surface (ground `#1f242e`, field `#161c26`, cards `#2a303c`,
lines `#353d4c`, text white, muted `#bac0ce`) and the brand's colour for the primary button, the usual level, an on
toggle and links (a lighter tint, 35 percent toward white). `color` is the only input: a plugin cannot set the surface,
fonts or the warning colour. Without a brand they wear Flare's theme and its chip. Amber (full control, "Always
allow") is the same on both and never neutral. The words that say whose surface it is (the plugin's name under a
question) are always Flare's, so a plugin dressed as another still shows its own name, with the host it was installed
from when Settings installed it from an address ("Vault (from example.com)"); the tab named in a fill approval is in
quotes, as the tab's own words (what runs in a tab can set its title). Rust:
`flare_mods::manifest::Brand`, `flare::plugin_look`; Slint: `ui/plugin_look.slint`.

## The details sheet

The card's info button opens the plugin's details sheet: its logo, name, `version · author`, description, a
two-column grid of tiles, one per permission its manifest asks for, then its facts (its program, its homepage). The card itself lists no permission, so its switch turned ON opens this sheet for a mod that asks for
anything or names a maker; the sheet's **Turn on** then approves exactly what the sheet lists (the hub refuses a sheet
drawn before the manifest changed). Only a mod with nothing to approve goes on from the card; off always goes at once.
The sheet has no warning text and no button that opens the panel (2026-11-08): the Access tile says Full control in
amber, and a plugin that wants its panel opened offers its own item. The box is never taller than the window less 24 px above and below: the
description and the grid scroll (the wheel, the arrow and page keys) between the header and the foot. Each
tile's tooltip gives its words and the whole of what it covers (the line on the tile is cut). The tiles come from the
manifest and Flare's catalog (`flare_mods::grants`), the same for every plugin:

| Permission | Tile | Choices |
|---|---|---|
| `process` | Access | dropdown: Full control (amber), Off |
| `input` | Typing | dropdown: Full control (amber), Off |
| `fill` | Fills logins | dropdown: Ask each time (brand), Always allow (amber), Off |
| every other | its noun (Panel, Agent tool(s), Tab menu or Buttons and menus, Sidebar, Internet, ...) | toggle on/off |

A tile names what it covers in the manifest under its value (the tools by name, the menu items, the panel's title, the
sections). The person's choices are enforced: a permission turned off leaves the plugin's live grant, so its methods
answer `forbidden` exactly like one never asked for and what it would open is not there (Access Off: the helper is
stopped; Agent tool off: the tools leave the agent; Tab menu off: the items go). "Always allow" types a fill with no
question; the site, the tab, a waiting agent call and a page that moved are checked just the same. A change gives the
plugin a new token (its views close and start again with the new grant). The choices are kept with the switch in
`mods.json` (`"withheld": [names]`, `"fill": "always"`), on or off, survive updates and an off and on, and go with a
removal. An entry that is on writes its `permissions` WITHOUT the held back names, so an older Flare reads a smaller
approval and turns the plugin off. A choice never widens anything: a permission the manifest does not ask for cannot be
set, and a new one still turns the plugin off until approved. No log line names a choice.

**Known gap: a downgrade loses the choices.** An older Flare that rewrites `mods.json` drops `withheld` and `fill`.
Since an entry that is on stores its `permissions` without the held back names, the older Flare turns such a plugin off
(the stored approval is smaller than the manifest's list); if the person then turns it on there, it approves the whole
list, and after an upgrade the plugin holds everything again with no notice. Set the choices again on the details sheet
after going back to an older Flare.

## Permissions

A plugin gets exactly what it lists and you approved (rule S5), less what you turned off on its details sheet; nothing
else, and nothing after it is turned off.
`eval` (`browser.eval`) is your own switch in Settings and never a plugin permission.

| Permission | What it opens | Words in Settings | Warning on the card |
|---|---|---|---|
| `panel` | Its panel in a tab (`panel.open` of its own `mod:<id>`, `panel.close`, `panel.state`). | Show its panel in a tab | |
| `view` | `view.image`, `view.file`, `view.text`, `view.html`, `view.diagram`, `view.audio`, `view.model` in its own panel. | Show images, files and text | |
| `browser` | `browser.*` on its own browser panel (not built: answered `unavailable`). | Open and use its own web page | |
| `convo` | `convo.state`: the changes of its tab's Claude session. | Read the agent's changes | |
| `tools` | Answer Claude's calls to its tools (`tools.respond`). | Answer the agent's tool calls | |
| `tabs` | See every tab (title, folder, agent, state, Claude session, panel) and act on any tab, not only its own. | See all your tabs and what their agents are doing | with `network`: can send what your tabs show to the internet |
| `decorate` | Badges and progress on tabs (`tabs.badge.*`, `tabs.progress.*`). | Put badges and progress on tabs | |
| `sidebar` | Its sidebar sections (`sidebar.set`). | Add a section to the sidebar | |
| `actions` | Its buttons and menu items (`actions.set`) and their clicks. | Add buttons to tabs and menus | |
| `bar` | Its strip under a terminal (`bar.show`, `bar.hide`). | Show a bar under the terminal | |
| `sessions` | What you and Claude say in sessions (`session.read`, the `session` event). | Read what you and Claude say in sessions | with `input`: can read and steer Claude; with `network`: can send what you and Claude say to the internet |
| `input` | Typing into a tab's shell (`tabs.input`). | Type and run commands in your terminals (full control) | Full control |
| `network` | Its pages may reach public https addresses (no method). | Use the internet | can change what it does from the internet |
| `process` | Flare runs its `helper` program as you (no method). | Run its own program on this PC (full access) | Full control, and it can leave things behind outside Flare |
| `agent-looks` | How each agent's tab icon moves while it works (`agents.motion.*`), a motion from Flare's list. | Change how agent icons move while they work | |
| `fill` | Fill a saved login into a tab's web page after you approve each fill (`browser.fill`, `browser.fill.page`), and ask you for a secret in Flare's own masked field (`secret.ask`). | Fill logins into your web panel | Can type passwords into pages, after you approve each one; with `network`: can send the passwords you type for it to the internet |

The words are `flare_mods::hub::permission_words`, the warnings `permission_warnings` (the details sheet does not show them).

Rules:
- Each method family has its own namespace opened by exactly its permission; nothing rides on the open `tabs.list`.
  A plugin with no permission reaches only `hello`, its own-tab `tabs.list` and its own store (`storage.*`).
  `install`, `update`, `remove` and the switch are Flare's own interface only (`Ui`), never a channel call.
- `input` and `process` are full control and Settings says so in those words. Approve them only for a plugin you
  trust as you would a program you install.
- An update that asks for more, or comes from another author, homepage or source, turns the plugin off until you
  approve it again (regression rule 212).

## The bridge (`window.flare`)

Flare injects `window.flare` into every page of the plugin (background, panel, bar) before the page's own scripts. It
is frozen and holds three functions and no data; the token never reaches the page (the host adds the plugin's live
grant to every call).

```js
// Claude called one of the plugin's tools. One handler per page.
window.flare.on('tool', function (call) {
  // call = { callId: "c1", tool: "set_steps", input: { ... }, tab: "t3" }   (tab: where Claude called from)
  window.flare.respond(call.callId, { value: 0.5 });             // success: any JSON
  // window.flare.respond(call.callId, null, 'steps is empty');  // failure: a reason
});

// Any other event (below): one handler per name per page.
window.flare.on('agent', function (e) { /* { tab, kind, state, session } */ });

// A channel method the plugin's permissions allow. Resolves with the result, rejects with { code, message }.
window.flare.call('tabs.badge.set', { tab: 't3', key: 'busy', icon: 'bolt', tip: 'Building' })
  .catch(function (err) { /* err.code: forbidden, bad_params, rate_limited, busy, ... (protocol.md, section 6) */ });
```

| Member | Meaning |
|---|---|
| `on(name, fn)` | Listen to `tool`, `tabs`, `agent`, `plugin`, `action`, `sidebar.item` or `session`. Once per name per page; another name throws. |
| `respond(callId, result, error?)` | Answers one tool call, once. With a string `error` the call fails with that reason. |
| `call(method, params)` | A channel method. `hello` and `tools.respond` are not sent this way (`bad_params`); a plugin that was turned off gets `unauthorized`. |

Rules for pages:
- Answer every tool call within 30 seconds (else Claude gets `timeout`), even with an error. A plugin with a background
  page has its tools answered there and no panel opens; without one, the call opens the plugin's panel in the caller's
  tab (protocol 1 routing); a plugin with neither but a helper has them answered by its helper (the `tool` event below).
  What a tool returns is cleaned before Claude sees it.
- Everything Flare sends is data: a tool's `input` is what Claude wrote, an event's text is what a tab or a session
  said. Validate it, and never use it as code.
- A page is served from the plugin folder only (see "Pages and the serving rule"); it cannot open a window, a link or a
  download, and a script dialog is dismissed.

## Events

| Event | Needs | Payload |
|---|---|---|
| `tool` | `tools` | `{callId, tool, input, tab?}` |
| `tabs` | none for its own tab (a background page has none); `tabs` for every tab | `{kind, tab}`, `kind` one of `opened`, `closed`, `focused`, `renamed`, `moved`, `panel` (what the tab's panel shows changed). With `tabs`, `tab` is the wide `tabs.list` object `{id, title, folder, active, agent, state, session, panel}`; without, `{id, title, active}`. |
| `agent` | `tabs` | `{tab, kind: "claude" \| "codex" \| "agy" \| null, state: "working" \| "question" \| "done" \| "failed" \| "idle", session}`: a tab's agent kind, state or Claude session changed. |
| `session` | `sessions` (and a tab it may see) | `{tab, session, role: "user" \| "assistant" \| "tool", text, tool?, at?, dropped?}`: one entry of the tab's Claude session as it is written. |
| `action` | `actions` | `{key, tab}`: one of its actions was clicked (`tab` null for a top-bar button with no shell tab in front). |
| `sidebar.item` | `sidebar` | `{key, item}` for a click, `{key, item, checked}` for a tick on its box. |
| `plugin` | none | `{kind: "on"}` once when the handler is registered, `{kind: "off"}` as the plugin is turned off. |

An event goes to every page of the plugin that listens to it (an `action` to the background page when it listens,
else to every page that does), at most 32 a page in one round (one window refresh, which also coalesces what changed).
A helper hears `tabs`, `agent` and `session` as channel events on its connection (`protocol.md`, "Events"), and, for a
plugin with no background page and no panel, `tool` `{call_id, tool, tab, input}` (snake case, unlike a page's
`callId`), answered with `tools.respond {call_id, result | error}`.

## Which tab a call acts on

- A panel or a bar page sits in one tab: a call that names no tab acts there. A background page and a helper sit in
  none, so they name the tab.
- With `tabs` a call may name any tab by its public id (`tabs.list` gives them; they are never the tab's secret
  `FLARE_TAB_TOKEN`). Without it, only its own tab, and a background page none (`forbidden`).
- **Click scope.** A click on one of the plugin's actions or sidebar items on tab X (a tab's menu or hover button, an
  item that leads to X, a session-bar chip, a top-bar button while X is in front) lets the plugin act on X as if it
  held `tabs` for that tab, until X closes or the plugin is turned off. So a plugin needs `tabs` only for tabs nobody
  clicked (`sonara-bar` shows its bar where you asked for it, with no `tabs`).
- A tool call names the tab Claude called from (`call.tab`), but it gives no scope: `look` holds `tabs` to put its
  progress there. One exception, for `fill` only (**tool scope**): while Claude's call to the plugin waits for its
  answer, `browser.fill` and `browser.fill.page` may name the tab the call came from. Allow checks it again: if
  the call has ended by then, the fill is `forbidden` and nothing is typed.

## Methods by namespace

The params, results and errors of each are in `protocol.md`, section 5. Every param is checked on the channel's
worker, and every limit spent there, before the window hears of it.

| Namespace (permission) | Methods | Limits |
|---|---|---|
| Hello, Tabs (none) | `hello` (a helper's first call; a page needs none), `tabs.list` (own tab; every tab with `tabs`) | |
| Decor (`decorate`) | `tabs.badge.set {tab?, key, icon, tip?, tone?}`, `tabs.badge.clear {tab?, key}`, `tabs.progress.set {tab?, key, value?, label?}`, `tabs.progress.clear {tab?, key}` | 10 changes a second per tab, 30 per plugin across tabs; 4 badge keys per tab, 200 in all |
| Sidebar (`sidebar`) | `sidebar.set {key, items}` | 5 a second per section; 50 items |
| Actions (`actions`) | `actions.set {key, tab?, enabled?, checked?, title?}` | 20 a second per plugin |
| Bar (`bar`) | `bar.show {tab?}`, `bar.hide {tab?}` | once a second per tab; one bar a tab (`busy`) |
| Agents (`agent-looks`) | `agents.motion.set {agent, motion}`, `agents.motion.clear {agent}` | 10 a second per plugin per agent |
| Session (`sessions`) | `session.read {tab?}` | once a second per tab; the last 200 entries |
| Input (`input`) | `tabs.input {tab?, text, enter?}` | 2 a second per tab; 4096 bytes |
| Storage (none, plugins only) | `storage.get {key?}`, `storage.set {key?, value}` | 1 MB as written; written to disk at most twice a second |
| Panel (`panel`) | `panel.open {kind: "mod:<own id>", tab?}`, `panel.close`, `panel.state` | on another tab, a panel showing something else answers `busy` |
| View (`view`) | `view.image`, `view.file`, `view.text`, `view.html`, `view.diagram`, `view.audio` (a `url` needs `network` too), `view.model` | as for the agent |
| Convo (`convo`) | `convo.state` | |
| Tools (`tools`) | `tools.respond` (the bridge's `respond`) | 8 calls waiting per plugin |
| Browser (`browser`) | `browser.*` | answered `unavailable` (not built) |
| Fill (`fill`) | `browser.fill {tab?, login_name, login_uris, username?, password?, totp?}`, `browser.fill.page {tab?}`, `secret.ask {title, label, sub?, button?}` | one question at a time in all (`busy`); 6 asks a minute per plugin, fills and secrets together; a minute to answer |

**Decor.** A badge is a bare glyph (no ring) Flare draws before the tab's end (left of the ring on a sidebar row),
in the tone `ink` (default), `accent` or `muted` (`mic` never `accent`, Flare's dictation mark). A progress is a static
pill (`value` 0 to 1; no `value` draws a dotted pill for an unknown amount), never at the tab's foot, where the agent's
mark lives; nothing animates. A tab draws at most 2 chips (progress first, then one badge per source; 2 on a named tab
of 150 px or more, 1 from 110 px, none below or on an icon tab), and every piece is a line in the tab's tooltip that
starts with the plugin's name. Setting the same key replaces; a tab has one progress, the last set.

## Limits

- **One budget per plugin**, shared by its pages and its helper: 50 messages a second, a burst of 100, 2 MB a second,
  no message over 256 KB. A page's message over it is dropped before it is copied out of the page; a pipe call over it
  answers `rate_limited`. The per-method limits above come on top.
- **Views:** at most 6 plugins with a background page on at once (a seventh is refused at the switch, with the
  reason); at most 2 live bar views per plugin; at most 6 plugin sidebar sections drawn in all, 3 top-bar buttons in
  all, 2 hover buttons on a tab.
- **Restarts:** a background page or a helper that ends is started again at once, at most 3 times in 5 minutes; the
  next end stops it, and Settings says "Stopped" until the plugin is switched off and on. A background page is pinged
  every 10 seconds (the bridge answers by itself); no answer in 10 seconds, or still loading 10 seconds after it began,
  counts as an end.
- **Nothing on the UI thread:** every call is checked and answered off it, and the window applies what plugins changed
  at most once a frame. Diagnostics: `plugin` lines in `diag.jsonl` (start, stop, crash, rate limit, refusals; no
  content).

## Pages and the serving rule

- **Where they run.** Every page (background, panel, bar) is a WebView2 view in the plugins' own browser, apart from
  the web panel and the mini-apps, in the plugin's own profile `flare-mod-<id>` (its `localStorage` survives a
  restart), at the address `https://flare-mod-<id>.local/<path>`. A plugin with `network` runs in a second, networked
  plugin browser (`WebView2-plugin-net`), so gaining or losing `network` starts its page storage empty.
- **Served per request,** never by a folder mapping: each request's path is checked, joined to the canonical plugin
  folder, canonicalised and must still be a file inside it (`flare_mods::paths::serve`, `mods.md` "Serving rule").
  Only the plugin's own host is served; `data:` and `blob:` pass; anything else is refused.
- **The policy** (a header Flare sets; the page's own meta policy applies as well, so the page gets the intersection):
  - sealed (no `network`): `default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline';
    img-src 'self' data:`. A sealed page cannot `fetch` at all; it keeps data in `localStorage` or `storage.*`.
  - with `network`: adds `img-src https:`, `connect-src 'self' https:`, `media-src 'self' data: blob: https:`, and
    `frame-src 'none'; form-action 'self'; base-uri 'none'`. Only public https hosts are let out: loopback, private,
    shared and link-local ranges, a host without a dot, `localhost`, `*.local`, `*.lan`, `*.internal` and
    `*.home.arpa` are refused (`flare_surfaces::net`). The networked browser sends every connection (a WebSocket too)
    through Flare's filtering proxy on `127.0.0.1` (`flare_surfaces::netproxy`, issue #97), which looks the name up
    itself and connects only to a public address it checked, so a public name pointed at this PC or the local network
    (DNS rebinding) reaches nothing there.
- **Sealed views.** Every plugin view: no new window, no download, no external link, no permission prompt, no
  DevTools, own frames only, script dialogs dismissed. A background or bar view also has no context menu and no
  browser keys, and never takes the keyboard (a click in a bar hands the keys back to the terminal).

## Background pages

A plugin that is on and declares `background` gets one hidden view of it for as long as it is on, tied to no tab,
never shown. It is where a plugin keeps its state, answers its tools and hears events whatever tab is in front.
Everything the plugin drew on tabs goes when the page ends (it sets it again when it starts). Sample:
`../../sample-plugins/browser-badge/`.

**Turning off.** Every view of the plugin closes (background, panels, bars), its waiting calls fail, its helper is
stopped and its decor, sections, actions and click scopes go, before Settings shows it off. Its generation moves on, so
nothing checked before is applied after (`unauthorized`, even once it is on again).

## Sidebar sections and actions

Sample: `../../sample-plugins/todo/`.

- **Sections.** Each `contributes.sidebar` entry is a section Flare draws under the tab list in the vertical tab layout,
  in the sidebar's look, under a header with the plugin's name (and the section's title when it differs). The plugin
  cannot style it. You order and fold them (the header's menu: Move up, Move down, Fold); the order and folds are
  preferences (`plugins.sidebar.order`, `plugins.sidebar.folded`). They take at most 45 percent of the sidebar's height
  and scroll inside. In the horizontal layout a "Plugins" button in the title bar opens the same sections as a dropdown.
- **`sidebar.set {key, items}`** replaces a section's items: at most 50 `{id, text, sub?, icon?, checked?, tab?}`.
  `id` 1 to 64 characters and unique; `text` and `sub` clean lines of at most 120; `icon` a built-in badge or the
  plugin's own; `checked` draws a checkbox; `tab` (a tab the plugin may see) makes a click bring that tab to the front.
- **`actions.set {key, tab?, enabled?, checked?, title?}`** changes one action: on the tab it names, on the caller's
  own tab, or on every tab when a background page names none (the top bar's button reads that one). A field left out
  keeps its value; a tab's own state wins over the every-tab one field by field. `enabled: false` draws it faint and a
  click does nothing; `checked` draws it ticked; `title` (80 characters) replaces its words.
- **Where actions show.** `tab-menu`: in a shell tab's right-click menu; a plugin with exactly one there shows it flat,
  its title beside the plugin's logo (Flare's chip without a brand, never the action's own icon, so it cannot pass for
  one of Flare's items) and the plugin's name as its hint when the title does not name it; one with more has a flyout
  under its name whose entry wears the logo (Flare's chip without a brand). A plugin titles its item by state with `actions.set`. `tab-hover`: a
  small button in Flare's outlined chip on a hovered tab (named tabs of 110 px or more, and sidebar rows), never over
  the agent's ring. `top-bar`: a button left of the window buttons. `session-bar`: a chip at the right end of the
  plugin's own bar (at most 4). An action without an icon shows its title's first letter.

## Sessions, typing and storage

- **`session` events and `session.read` (`sessions`).** What is said in the Claude session of a tab the plugin may
  see: `role` `user` (a prompt), `assistant` (the reply's words) or `tool` (a tool call: `tool` its name, `text` the one
  argument that says what it does, else its input as JSON). Tool results, thinking, Claude Code's own notes and
  subagents are not entries. `text` is cleaned and at most 2000 characters; `at` is the transcript's time. Events come
  only for what is written after the page began to listen, at most 50 a second (the next event says how many were
  dropped). `session.read` gives the last 200 entries, oldest first (`session: null` when the tab has no known
  session). The transcript is followed on a worker only while a page that may see the tab listens.
- **`tabs.input {tab?, text, enter?}` (`input`).** Types `text` into the tab's shell the way your paste goes: one
  bracketed paste when the program asked for one (Claude Code does), with no Enter; `enter: true` presses Enter 300 ms
  later, if the tab is still open and the plugin still on. `text` is 1 to 4096 bytes, and any control character but tab
  and line breaks is refused, so nothing can end the paste early or press a key of its own. It is the only way into a
  shell from outside Flare's own interface (regression rules 2 and 188): Claude never has it. Sample:
  `../../sample-plugins/session-buttons/`.
- **`storage.get {key?}` and `storage.set {key?, value}` (every plugin, no permission).** The plugin's own JSON object
  in `<profile>\plugins-storage\<id>.json`, shared by its pages and its helper. `get` with a key gives `{value}` (null
  when absent), with none the whole object; `set` with a key sets it (null removes it), with none replaces the whole
  object. Keys 1 to 128 characters; at most 1 MB as written (`too_large`, nothing changed). A set is answered at once
  and written on a worker. Neither Claude nor a protocol 1 mod has a store. Samples: `what-next` (the list survives a
  restart) and `sonara-bar` (its pages share what "plays").

## Session bar, network and audio

- **`bar.show {tab?}` and `bar.hide {tab?}` (`bar`).** The plugin's `bar.entry` under the terminal of a tab it may see,
  `bar.height` tall. The grid is laid out shorter by that height and the shell resized once; nothing animates. One bar a
  tab: while another plugin's shows there, `show` answers `busy` with its name. The bar shows only while its tab is in
  front with its terminal, hides while a Flare menu or dialog is open (a browser view paints over Flare), and goes when
  the plugin stops or the tab closes. Its page has the bridge: its calls act on its tab. Sample:
  `../../sample-plugins/sonara-bar/`.
- **`network`.** See "Pages and the serving rule": https to public hosts only, from the plugin's pages; never plain
  http, scripts from the internet, frames, or a form that posts elsewhere. Sample: `what-next` with `network` added.
- **`view.audio {path | url, title?, autoplay?}` (`view`).** Flare's own audio player in the plugin's panel: a sound
  file (mp3, wav, ogg, oga, opus, m4a, aac, flac, webm, weba; at most 100 MB) or a public https address (which needs
  `network` too). It never loops and plays by itself only with `autoplay: true`, once.
- **`view.model {path, title?}` (`view`).** Flare's own 3D viewer in the plugin's panel: a `.glb`, `.gltf` or `.stl` file (at most
  64 MB). No network and no script of the plugin's.

## Agent icon motions

Owner decision 2026-10-26, amending design 13 ("`tabs.icon.set` works only on tabs with no agent") narrowly: a plugin
may pick how an agent's tab icon moves while that tab works. It picks a **motion from Flare's fixed list**; it never
sends the agent's artwork, never draws a check or a dot, and the agent's marks (ring, bar) stay Flare's.

- **`agents.motion.set {agent, motion}` and `agents.motion.clear {agent}` (`agent-looks`).** `agent` is `claude`,
  `codex` or `agy`. The motions, all drawn by Flare:

  | Motion | What it does | Agents |
  |---|---|---|
  | `none` | Still (holds an icon still over another plugin's older pick) | all |
  | `hop-blink` | A slow hop with a squash on landing (1.8 s), Clawd blinking every other landing (H1) | all (only Clawd has eyes) |
  | `spin-fade` | A turn every 3.5 s while fading from 0.55 to 1 and back over 1.8 s (O6) | all |
  | `rainbow-flow` | The colours run along the arch, one lap every 3 s, each path 0.3 s ahead of the last (A1) | agy only |
  | `hop` | The hop of `hop-blink`, no blink | all |
  | `spin` | A steady turn every 4 s | all |
  | `breathe` | Grows from 0.82 to 1.05 times its size and back, fading a little, 2.6 s each way | all |

- **When it moves.** Only on the tab of an agent that is working (its Working mark is on), in the strip and the sidebar
  (the +N list keeps its icons still), and never with Windows' animation effects off. Every frame comes from the mark
  clock that already runs while a tab works, so nothing new ticks while all is idle. Without a pick an icon stands
  still, as before. (On the software renderer a turn is left out; the icon still fades.)
- **Who wins.** One pick per plugin per agent; the newest pick wins. A plugin that clears its pick, is turned off or
  removed, or whose background page ends, takes its picks with it, and the older pick of another plugin shows again.
- **Answers.** `set` answers `{agent, motion}`; `clear` answers `{agent, cleared, motion}` with the motion the agent wears
  now. A motion or an agent not on the list, or `rainbow-flow` for an agent that is not agy, is `bad_params`, and the
  message lists what fits. A protocol 1 mod cannot ask for `agent-looks`.
- Sample: `../../sample-plugins/agent-moves/` sets the owner's picks (Claude `hop-blink`, Codex `spin-fade`, agy
  `rainbow-flow`) from its background page and has a panel, opened from a tab's menu, to pick one per agent (kept in
  its store).

## Filling logins (`fill`)

Owner decision 2026-10-28 (design: `../superpowers/specs/2026-10-28-plugin-fill.md`; the Bitwarden plugin it is for:
`Maxaubert/flare-plugins`). A password manager plugin signs the agent in with your saved login, you approve each fill in
Flare's own dialog, and the user name, password and code never reach the agent.

- **`browser.fill {tab?, login_name, login_uris, username?, password?, totp?}`.** The tab is the caller's own, one it
  may see, or the tab of Claude's call it is answering (tool scope). Its panel must be a web page that is up (not a
  mini-app or a plugin's page: `not_found`). The page's address must be a site of the login: its host or a subdomain
  of it (a leading `www.` of the login's host left out), `https` unless the login's address says `http`, the login's
  port when it names one, an IP address only itself; a bare `example.com` reads as `https://example.com`. Else
  `forbidden` with "this login is not for <host>". `login_name` is one clean line of at most 120 characters (the dialog
  shows it); `login_uris` 1 to 32 addresses; each value 1 to 1024 characters with no control character; at least one
  value; any other key is `bad_params`, and no error ever holds a value.
- **The dialog** (centred, in the plugin's look): its logo, "Fill <login>?", a card with the page's host and who asked in
  which tab ("asked by the agent in <tab>", or "asked by <plugin> in <tab>"), wide Deny and Fill, "Deny for this
  session" as a link, and Flare's line "<Plugin> fills it into the page; the agent never sees it.". Only your pointer
  presses Fill: no key does (Enter and Space press the Deny button that has the focus, Escape and a press outside are
  Deny), and it has no accessible default action. Fill takes no press for its first 0.7 s, so a click already on its way when the dialog appears never
  approves it. The web page hides under it. It waits a minute, then counts as Deny.
- **On Allow** the page's address is checked again (it may have moved while you read: `interrupted`), then Flare types
  the values with one `ExecuteScript`. The script first checks the document's `location` (scheme, host and port) with
  plain comparisons the page cannot replace, and stops (`interrupted`) before it reads any value when the tab has moved
  to another site; only then are the values read, as JSON data with `JSON.parse`, never as script text. The
  script finds the fields by their `autocomplete` (`username`, `email`, `current-password`, `one-time-code`) first, then
  the focused text field, the last text field before the password, a field named like a user or an email, and a code
  field named like `otp`, `2fa` or `verification`; it sets each value as typing does and fires `input` and `change`.
  Fields inside frames are not reached.
- **Answers.** `{filled, missing}` (kinds of field: `username`, `password`, `totp`), `{declined: true}`,
  `{declined: true, for_session: true}` (that plugin's fills are declined at once, with no dialog, until Flare quits),
  `{declined: true, timed_out: true}`, or an error. One dialog at a time across plugins (`busy`) and 6 asks a minute per
  plugin (`rate_limited`).
- **`browser.fill.page {tab?}`** gives `{tab, url, host}`: the address a plugin picks the login for.
- **`secret.ask {title, label, sub?, button?}`** shows Flare's masked field, centred in the plugin's look: its logo,
  `title` as the heading, `sub` under it (an account), one field with `label` as its placeholder and an eye that shows
  what is typed, a full-width button with `button` (`OK` without it), Cancel as a link, and Flare's line "What you
  type goes to <Plugin> only.". Enter submits. Answers `{text}` to that plugin only, `{cancelled: true}` or `{cancelled: true, timed_out: true}`. Flare keeps
  and logs none of it. It shares the 6 asks a minute.
- **A tool answered by a helper.** Claude's call to a plugin with a helper and neither a background page nor a panel
  reaches the helper as the `tool` event; while it waits the helper may fill into the caller's tab. Claude has 30 s for
  the whole call, so a fill inside a tool call leaves you about 25 s to answer.
- **Nothing is written down.** The one log line of a fill is `fill: plugin <id> on <host>: <outcome>`, of a secret
  `secret.ask: plugin <id>: answered|cancelled`. The agent never holds `fill`.

## Helper programs

- A plugin that declares `helper` and holds `process` gets its program started while it is on, and only then. The
  command passes the path checks again at start and is started by its canonical path, with the plugin folder as its
  current folder, no console window and stdin closed.
- **Environment:** Flare's own minus `FLARE_TAB_TOKEN` and `FLARE_RESUME_ID`, plus `FLARE_PLUGIN_TOKEN` (the plugin's
  token, set on the child only) and `FLARE_CTL_PIPE`. It speaks the channel like any client (`protocol.md`): `hello`
  with `token`, then the methods its grant allows, within the plugin's one budget. It has no tab of its own, so it names
  tabs (and needs `tabs` for that, or a click).
- **Contained:** it starts suspended and joins a kill-on-close job object before it runs, so it and everything it starts
  end when the plugin is turned off and when Flare ends. Its stdout and stderr go to `<profile>\plugins-logs\<id>.log`
  (moved to `.log.1` past 1 MB). It hears `tabs`, `agent` and `session` events as a background page would, and the
  `tool` event when it answers its plugin's tools (a plugin with no background page and no panel).
- A helper runs as you, with full access: it can leave things behind outside Flare, and its Access tile says Full control.

## Install, update, remove

- **Install from a file or an address** in Settings > Plugins. An address must be `https` on every redirect (at most
  5), on a public host; the download is at most 50 MB, 60 seconds, no cookies, no proxy from the environment, never the
  web panel's browser.
- **The zip** holds `plugin.json` (or `mod.json`) at the top or in exactly one top folder. Each entry is written by
  Flare with create-new into `plugins\.staging\<random>`; refused: links and reparse points, encrypted entries,
  absolute and drive paths, `:`, `\`, control and hidden formatting characters, `<>"|?*`, `.` and `..` parts, parts
  ending in a dot or space, Windows device names with any extension, names that differ only in case, paths over 200
  characters or more than 8 folders deep; 16 MB an entry and 100 MB in all as really decompressed, 2000 entries. The
  manifest and every file it names are checked on the staged copy, and its content hash taken, before anything
  reaches `plugins\<id>`.
- **Installed off.** Where it came from and the hash are recorded in `mods.json` (`installed`); the source goes into
  the approval when you turn it on.
- **Update** (the card's button): downloads again from the recorded address, or opens the file picker in the folder of
  the zip it was last installed from (`installed.folder` in `mods.json`, never part of the approval); the zip must
  hold the same id, and an older version is refused ("Remove it first"). A plugin that is on is paused, its views closed
  and its helper stopped and waited for, then the folders are swapped by rename and the old copy goes to the Recycle
  Bin. The same permissions and maker keep it on, as a new generation; more permissions, or another author, homepage,
  source or protocol, turn it off until approved, and another maker or source also wipes its storage and profile.
- **Remove** (the card's button): turns it off, waits for its helper, sends its folder to the Recycle Bin and forgets
  its approval, install record, storage, helper log, browser profiles, sidebar order and folds.
- Install, update, remove and the scan run one at a time on one worker queue. A folder copied into `plugins\` by hand
  works too, and is off until approved.

## Safety rules

- **Exact permissions (S5).** The token maps to the approved permissions and nothing else; a plugin that is off has
  no token; every call checks the live grant and the plugin's generation.
- **No spoofing.** Plugin badges sit in Flare's decor chip, never in the agent's ring or mark; no `check` or `dot`
  icon; tones never equal the state colours; every tooltip line starts with the plugin's name. An agent's icon motion
  is one of Flare's own, drawn on the agent's own artwork; a plugin sends no art for it. A plugin cannot style
  its sidebar section or draw over the tab strip.
- **Untrusted manifest and untrusted pages.** Every field is checked before use, every path on every use, every page
  message for shape and size before its method is read.
- **Typing is opt-in full control.** Only `tabs.input` reaches a shell, only with `input`, as one paste with no control
  characters; Claude never has it.
- **Network is opt-in and public only.** Without `network` a page has no way out (a sealed browser with no name
  resolution); with it, https to public hosts only.
- **A bad plugin fails alone.** Each folder is refused with its own reason; a dead page or helper is restarted a few
  times, then stopped and shown, and never takes Flare down.
- **Secrets stay in memory.** Tokens are never written, logged or shown; tab ids given to plugins are never the tab's
  secret. A login filled with `fill` and a secret typed for `secret.ask` are in no log, answer or store of Flare's.
- **A fill is yours to approve.** Flare's own dialog, Fill by the pointer only (or "Always allow", set by you on the
  plugin's details sheet and never by the plugin), the login's site only, one at a time and
  6 a minute, and "Deny for this session" to stop a plugin that asks too often.

## Claude's own surfaces (no plugin)

Claude in a Flare tab has the same decor for its own tab through `flare-mcp` (the agent grant), with no plugin:
`set_tab_badge {key, icon, tip?}`, `clear_tab_badge {key}`, `set_tab_progress {value?, label?}`,
`clear_tab_progress` and `show_audio {path | url, title?, autoplay?}` (`protocol.md`, section 9). Its pieces go when
the tab's Claude session changes, the agent leaves or the tab closes. The call-me skill puts
`set_tab_badge {key: "call-me", icon: "phone", tip: "Call-me is on"}` on the tab while call-me is on; a workflow reports
weighted progress with `set_tab_progress` (the `look` sample shows the same math as a plugin tool).

## For the Prism Terminal port

Port the rules, not the code: the manifest checks in the same order with the same sentences, the permission names and
words, the namespace table, the limits, the serving rule and the zip rules. The test vectors are the Rust tests named
above and in `mods.md`; the samples in `sample-plugins/` must load in both apps. The research behind the design:
`C:\Users\Admin\Documents\Claude\research\flare\2026-10-25-mod-support.md`.
