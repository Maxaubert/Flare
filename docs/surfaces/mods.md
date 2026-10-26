# Mods

A mod is a small folder that adds a panel to a tab and, optionally, tools the agent can call. It is
written by the user or by a mod author, it runs on the user's PC, and it gets exactly the permissions
its manifest asks for and the user accepts. This file is the contract: for mod authors, and for the
Prism Terminal port, which must reproduce the same rules.

- Sample: `sample-mod/notes/` (a notes panel the agent can append to with `append_note`). The tests
  in `crates/flare-mods` read it, so it is always a valid mod.
- The Rust reference: `crates/flare-mods` (manifest, paths, discover, enabled, tokens, tools).
- The control channel the bridge speaks: `protocol.md` (`tools.respond`, and the grants).
- Design: `../superpowers/specs/2026-10-21-surfaces-design.md`, section "Mods", and rules S5 and S6.
- **Status (2026-10-22):** built, library and app: the manifest, discovery, the switch store, tokens, routing
  (`crates/flare-mods`), and in Flare Settings > Mods, a mod's panel served per request, the `window.flare`
  bridge, the `tool` events, `tools.respond`, `mods.tools` / `mods.call` and the `flare-mcp` routing, with the
  e2e `modsSample` on the sample mod. One thing is a documented limit, not a bug: a mod's own browser panel
  (`browser.*`) is not built (see "Which tab a mod acts on"). The Electron port reproduces the rules here, not
  the code.

## Where mods live and when they run

- Folder: `%APPDATA%\Flare\plugins\<id>\`, the one folder for every plugin, protocol 1 (`mod.json`) and protocol 2
  (`plugin.json`) alike (plugins phase 2c). The folder name is the id. Settings > Plugins has an "Open plugins folder"
  button (the folder is made at start and each time the page opens, on the scan worker).
- The old folder, `%APPDATA%\Flare\mods\`, is no longer read. At start, before the first scan and on its worker,
  Flare moves once every folder in it whose name is a legal id into `plugins\` (a rename on the same volume). A name
  `plugins\` already holds stays in `mods\` and Settings > Plugins names it in one sentence; a folder whose name is not
  an id is left alone; an emptied `mods\` is removed, one that still holds something keeps it. A mod's switch and
  approval (`mods.json`, whose name stays) and its storage (the profile `flare-mod-<id>`) are tied to its id, never to
  its folder, so a moved mod keeps all three (`flare_mods::migrate`).
- A mod is OFF until the user turns it on in Settings > Mods, which shows its permissions first.
- Turning it on mints the mod's token. Turning it off kills the token. Nothing is stored about tokens: a
  restart mints new ones for the mods still on.
- Mods are not fetched from anywhere. There is no store, no signing and no network install.

## The manifest (`mod.json`)

One JSON object, at most 64 KB. Unknown keys are an error at every level, so a typo is refused, never
ignored. Every refusal is one sentence that names the field; Settings > Mods shows it for the folder.

| Field | Type | Rule |
|---|---|---|
| `id` | text | Required. 3 to 40 characters: `a-z`, `0-9`, `-`. Not a Windows device name (`con`, `nul`, `com1` to `com9`, `lpt1` to `lpt9`, `aux`, `prn`). Must equal the folder name exactly. |
| `name` | text | Required. Not blank, at most 80 characters, no control characters or line breaks. |
| `version` | text | Required. Semantic version: `1.0.0`, `1.0.0-rc.1`, `1.0.0+build.5`. No leading zeros. |
| `protocol` | number | Required. Must be `1`. |
| `description` | text | Required. Not blank, at most 500 characters. |
| `permissions` | list of names | Required (may be `[]`). Each name once. See below. |
| `panel` | object | Optional. `{"entry": "index.html", "title": "Notes"}`. Needs the `panel` permission, and the `panel` permission needs a panel. |
| `tools` | list of objects | Optional. At most 32. Needs the `tools` permission and a `panel` (the page answers them). |

### Permissions

| Name | What it allows the mod | Notes |
|---|---|---|
| `panel` | Show its panel in a tab. | Required with a `panel`. `panel.open` from a mod takes only its OWN `mod:<id>`: any other kind (`web`, `changes`, another mod) is `forbidden`, because a web panel would reach the network in the user's shared profile (S4/S5). `panel.close` and `panel.state` are allowed. |
| `view` | Show content through `view.*` (image, file, text, html) in its own panel or the user's. | Same checks as the agent (rule 13, 14). `view.html` with `allow_network: true` is `forbidden` for a mod: the user's mini-app network switch is consent for the AGENT's pages, never a mod's. |
| `browser` | Drive the mod's own browser panel through `browser.*` (open, navigate, look, click, type, press, scroll, wait). | Never the user's Edge profile (S4), and never the shared web profile of the user's tabs. Its own profile (see "Which tab a mod acts on"). **v1 limit:** a mod that holds `browser` passes the grant, and the call is then answered `unavailable` ("a mod's own browser panel is not built yet"): the panel slot of a tab holds one panel, which for a mod is its page. A mod without `browser` is refused every `browser.*` method with `forbidden`, before anything runs. |
| `convo` | Read `convo.state` (the session's changes). | |
| `tools` | Answer tool calls through `tools.respond`. | Required with a `tools` list. |

A permission not in this list is refused with a reason. `eval` (`browser.eval`) is the user's own switch
(rule S4), so a mod can never ask for it, and a mod's token is never minted with it (`Tokens::mint` refuses
it, and `mod_grant` drops it even when a caller passes it). A mod gets EXACTLY its listed permissions (S5);
`hello` and `tabs.list` are open to every connection, `tabs.list` answers by the rule in protocol.md, and
nothing else is open.

### Panel

`entry` is a path inside the mod folder to an `.html` file. Refused when it:

- is empty, longer than 200 characters, or has a control character;
- contains `:` (a drive or a stream), or starts with `/` or `\` (absolute or a network path);
- has a `..` part, or an empty or `.` part;
- does not end in `.html` or `.htm`;
- does not exist in the folder, or, once resolved, is not a file inside the canonical mod folder (a link
  or a junction that points out is refused).

### Tools

```json
{
  "name": "append_note",
  "description": "Adds one line to the end of the Notes panel.",
  "input_schema": {
    "type": "object",
    "properties": { "text": { "type": "string", "minLength": 1, "maxLength": 500 } },
    "required": ["text"],
    "additionalProperties": false
  }
}
```

- `name`: `a-z`, `0-9`, `_`, 1 to 40 characters, unique in the mod.
- `description`: what the agent reads to decide when to call it. Not blank, at most 500 characters.
- `input_schema`: a JSON Schema object whose `"type"` MUST be `"object"` (the MCP tool listing needs an object
  schema), at most 8 KB serialised. Flare checks the shape, not the full JSON Schema: the page checks the
  input itself.
- The name the agent sees is `mod_<id>_<name>` (see Tool routing), and it must fit in 52 characters, so the
  whole MCP name `mcp__flare__mod_...` fits the 64-character limit of the model API and MCP clients. A longer
  name is left out of the agent's tools and reported in Settings > Mods, like a clash. (A 40-character id and
  a 40-character tool name is 85 characters: refused by the table, pinned by a test.)

### A complete example

`sample-mod/notes/mod.json` is the reference. Copy it, change the id (it must equal the folder name), and
the tests in `crates/flare-mods` show every refusal message.

## The bridge (`window.flare`)

The host injects `window.flare` into the mod's page before any of the page's own scripts run. The page
never sees the token: the host adds it to every call.

```js
// Tool calls from the agent. One handler; it may be registered once.
window.flare.on('tool', function (call) {
  // call = { callId: "c1", tool: "append_note", input: { text: "..." } }
  window.flare.respond(call.callId, { count: 3 });            // success: the result (any JSON)
  // window.flare.respond(call.callId, null, "text is empty"); // failure: a reason string
});

// A channel method the mod's permissions allow. Resolves with the result.
window.flare.call('tabs.list', {})
  .then(function (result) { /* result.tabs = [{ id, title, active }] */ })
  .catch(function (err) { /* err = { code, message }, codes from protocol.md section 6 */ });
```

| Member | Meaning |
|---|---|
| `on('tool', handler)` | Called for each tool call the agent makes on one of this mod's tools. Only `'tool'` exists in v1. |
| `respond(callId, result, error?)` | Answers one call. `result` is any JSON value; with `error` (a string), the call fails with that reason. Sent as `tools.respond`. |
| `call(method, params)` | A channel method, such as `tabs.list` or `convo.state`. Returns a promise. Refused by the host unless the mod's grant allows it (`forbidden`; a mod that was turned off is `unauthorized`). `tools.respond` and `hello` are not sent this way (`bad_params`): use `respond`. At most 16 calls of one mod run at once (`rate_limited`). |

Rules for the page:

- A call that is not answered within 30 seconds fails for the agent with `timeout` (the mod's name and the tool
  are in the message). Answer quickly, and answer every call, even with an error. A call that arrives before the
  page has loaded waits for it; a page that never loads, a panel that is closed, a page whose process ends and a
  mod that is turned off all fail the call at once with `unavailable`. After a page's process ends the panel stays
  and the next call makes a new page. A reply is given only to the document that asked: when the page reloads,
  calls already handed to the old document fail (`unavailable`, "the mod's page reloaded") and a late reply is
  dropped, because call ids restart in every document.
- What a tool returns is cleaned before the agent sees it: escape sequences, control characters (except a line
  break and a tab) and invisible or direction-changing characters are removed.
- `window.flare` is frozen and cannot be replaced. It holds three functions and no data; the token is not in the
  page (the host keeps it and looks the mod's live grant up for every call).
- `callId` is only good for the call it names. Answering it twice is an error.
- Everything the host sends is data. A tool's `input` is what the agent wrote, so validate it.
- The page is served from the mod folder under the Serving rule below: no network, no loading from
  elsewhere. Keep scripts and styles inline or in the folder.

**The `respond` wire form.** The bridge maps `callId` to the wire's `call_id`.
When `error` is a string, the bridge sends `{call_id, error}` only. Otherwise it sends
`{call_id, result}`, with `result` set to `null` when the page passed `undefined` or nothing. A present
`result` key whose value is `null` is a success (`protocol.md`, `tools.respond`); a call with neither is
never sent, because the bridge always sends one of the two.

## Serving rule

A mod's page is NOT served by a folder mapping (`SetVirtualHostNameToFolderMapping` in WebView2, or any
static root). A mapping serves every file under the folder and follows the junctions and links inside it, so
a junction `data -> C:\Users\<me>` would let the page fetch `https://<mod-host>/data/.ssh/id_rsa`, and a
meta CSP written by the page's author does not stop it. Instead:

- Every request goes through `WebResourceRequested` (Electron: `protocol.handle`). The host answers with
  `flare_mods::paths::serve(folder, url_path)`, which runs on EVERY request, not once at the scan: the
  request path (percent-decoded, leading `/` optional) is syntax-checked (no `..`, no empty part, no `:`,
  no `\`, no control character), joined to the canonical mod folder, canonicalised, and must still start with
  the canonical folder and be a file. A junction or link that points out is refused, and so is a file that
  was swapped for one after the scan (`paths::tests::a_folder_swapped_for_a_junction_after_the_scan...`).
  The page's own entry is served for `/`.
- Each answer carries a header CSP:
  `default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:`.
  A meta tag is not enough to rely on, and the header is the one the host controls. The page's own meta policy
  is applied as well, so the page gets the INTERSECTION: the sample's own meta (`default-src 'none'`, inline
  script and style only) leaves it no `img-src`, and a second page of the folder that wants an image says
  `img-src 'self'` itself. Because `default-src 'none'` also covers `connect-src`, a page cannot `fetch` or
  `XMLHttpRequest` even its own address: it loads scripts, styles and images from its folder, keeps its data in
  `localStorage` (a profile of its own that survives a restart) and asks Flare through the bridge.
- A request is routed by `flare_surfaces::modview::route_request`: a path of the mod's own host is SERVED (the
  address parser has already resolved dot segments; `serve` checks the rest), `data:` and `blob:` pass (they never
  touch the network), everything else (https to another host, http, a bad `%` escape in the mod's own path) is
  refused 403. A file over 16 MB is 413, a miss or an escape 404, and more than 8 reads at once 503.
- Every other request of the view is also answered by this handler, never by the network: the view runs in the
  mini-apps' sealed browser (WebRTC off, a proxy that is not there, no name resolves), so the page has no way
  out even if a rule here missed something.
- Navigation: only the mod's own host may be navigated to (`NavigationStarting` in WebView2, `will-navigate` in
  Electron). Any other URL is cancelled. New windows are denied (`NewWindowRequested` / `setWindowOpenHandler`
  returns deny). A mod's page cannot open a link, a window or a download.
- Serving reads the disk, so it runs on a worker, never the UI thread (rule 34): the browser's request is held
  with a deferral, a worker reads the file, and the UI thread hands the answer back.

## Which tab a mod acts on

A mod is bound to ONE tab: the tab whose panel shows the mod, and the bridge is injected into that panel only.
No mod call names a tab, so there is no tab argument to aim at another one.

- `view.*` shows content in the bound tab's panel slot, never another tab's. A tab has ONE panel slot, which for
  a mod is its page, so a mod's `view.text` REPLACES the mod's own page (the page ends and the tab shows the
  text). v1 accepts this; `view` is mostly for a helper process that holds the mod's token and no page.
- `browser.*` is meant to drive a browser panel that belongs to the mod, in its OWN WebView2 profile, never the
  shared web profile that holds the user's logged-in tabs (S4 and "One profile per mod" below). **Not built:** it
  needs a second slot beside the mod's page, so a mod that holds `browser` is answered `unavailable`. The
  profile rule is already kept for the page itself: a mod's page runs in its own profile `flare-mod-<id>`
  (not private, so its `localStorage` survives a restart), inside the mods' and plugins' own sealed browser (apart
  from the mini-apps' since plugins phase 2b), under `<profile>\WebView2-app-sealed` (decisions.md, stage 8). The agent's `browser_*` tools do not drive a mod's page
  either (`unavailable`: the panel belongs to a mod).
- A mod that is not shown in any tab has no bound tab: its `view.*` calls and its tool calls need one. A tool
  call opens the mod's panel in the CALLER's tab (a tab behind gets its page made hidden), so a call never fails
  because the panel was closed; it fails with `unavailable` when the page cannot be made (no WebView2 runtime).
- `tabs.list` for a mod gives its own tab only (id and title), as for the agent. Listing every tab would show titles (folders, commands, task names) that "Show its panel in a tab" never said (2026-10-22 review).
  Nothing in it reads page content.

Why: the agent's rule is "its own tab" (decisions.md, stage 2). A mod is not the agent, but the same rule
stops a mod from reading a page the user is logged into in another tab.

## Tool routing (how the agent reaches a mod)

- The agent sees each tool of each ENABLED mod as `mod_<id with - as _>_<tool>`. `notes` with
  `append_note` is `mod_notes_append_note`. The `mod_` prefix keeps mod tools clear of the built-in agent
  tools.
- The mapping can clash: `ab-c` with tool `d_e` and `ab-c-d` with tool `e` both give `mod_ab_c_d_e`. Both
  sides are LEFT OUT of the agent's tools, and Settings > Mods shows the clash. Nothing is guessed.
- A name over 52 characters is LEFT OUT too (`ToolTable::build` returns it as `TooLong`), and Settings > Mods
  says so. See the length rule under Tools.
- A call goes: agent (through `flare-mcp`) -> the channel (`mods.call`) -> the host finds the mod by name, opens
  the mod's panel in the caller's tab if it is not showing -> the host sends a `tool` event to the mod's page with
  `{callId, tool, input}` (queued until the page has loaded) -> the page calls `respond` -> `tools.respond` -> the
  agent gets the result. A disabled mod has no tools and no token, so nothing routes. The agent lists the tools
  with `mods.tools` (`flare-mcp` does this at every `tools/list`), and hears `mods.changed` when the set changes,
  which `flare-mcp` turns into `notifications/tools/list_changed`.
- At most 8 calls wait on one mod. A mod answers only the calls routed to it, once each.
- A mod can only answer calls for its own tools. The host sends a tool call only to the mod that declared
  it.

## Security rules

- **S5, exact permissions.** A mod's token maps to its manifest permissions, and nothing else. A disabled
  mod's token is dead at once.
- **Tokens are secrets and live in memory.** 256 bits from the system random generator, 64 hex digits.
  Never written to a file, never logged, never shown. The lookup compares every live token in constant
  time.
- **One profile per mod.** A mod's WebView2 profile is its own (`flare-mod-<id>`, kept between runs so its
  `localStorage` survives a restart), inside the mods' and plugins' own sealed browser, under
  `%LOCALAPPDATA%\Flare\WebView2-app-sealed` (with a `--user-data-dir`, beside that profile's `WebView2` folder; the
  folder name is older than the split from the mini-apps in plugins phase 2b and must not change, or every mod's
  storage is lost);
  never the web panel's profile and never the user's Edge profile. A profile name is the only per-mod name WebView2
  allows inside one browser, so the folder is not `...\plugins\<id>\profile`.
- **No network from a mod.** The page is served from its folder with a strict policy. A mod that needs
  the internet is not a v1 mod. A protocol 2 plugin reaches it only with an approved `network` (plugins phase 6,
  "Session bar, network and audio" below).
- **The manifest is untrusted.** Every field is checked before anything is used. A path is checked three
  times: its syntax when read, its canonical location on disk (links and junctions included) when the mod is
  found, and the same location again on every request its page makes (the Serving rule).
- **Discovery is bounded.** At most 64 mod folders are read (only folders whose names are legal mod ids count;
  each other folder is reported by name), at most 4096 directory entries are looked at (one more is an error,
  and the rest are not looked at), and a `mod.json` over 64 KB is refused before it is parsed.
- **Text a person reads is clean.** Names, titles and descriptions refuse control characters and the Unicode
  format characters (zero-width spaces, bidi overrides, the BOM): what Settings shows is what the agent reads.
- **A bad mod does not stop the others.** Each folder fails alone, with its reason.
- **Enabled state is stored atomically** (`%APPDATA%\Flare\mods.json`, `{"version":1,"mods":{"id":{"on":true,"permissions":["panel"]}}}`: the switch and the permissions the person
  approved with it; an on entry without that list reads as off, and a scan that finds the manifest asking for more
  turns the mod off until it is switched on again, regression rule 212).
  Unknown entries are dropped and never written back. A damaged file (not JSON, no `version`, no `mods`
  object) is moved to `mods.json.bad` and the switches start empty. A `version` that is present and is not
  exactly 1 is read as empty and left alone, and the store is read-only for the session.

## For the Prism Terminal port

Port the rules, not the code. The table in `README.md` says what each Rust piece becomes in Electron.

- The manifest checks are the same list, in the same order, with the same sentences (they are the
  contract test). Reject unknown keys at every level.
- Containment, at discovery: `fs.realpath` on the mod folder and on the entry, then `startsWith` of the canonical
  folder. A junction and a symlink are followed by `realpath`, so they are caught the same way.
- Containment, on EVERY request: the page is served by `protocol.handle` through `serve`'s rule
  (`realpath` of the joined request path, `startsWith` the canonical folder, `isFile`), with the header CSP. Never
  by a folder mapping or `protocol.registerFileProtocol` of the mod folder.
- Tokens: `crypto.randomBytes(32).toString('hex')`. Look them up with `crypto.timingSafeEqual` over every
  live token, without an early exit.
- The bridge: a preload script with `contextBridge.exposeInMainWorld('flare', ...)` that sends `tool`
  events and `tools.respond` over `ipcRenderer` to the main process. The page never gets the token.
- Routing: the same `mod_<id with - as _>_<tool>` names, and the same clash rule (both sides out).
- Test vectors that must match: the sample mod is valid; `ab-c`/`d_e` with `ab-c-d`/`e` clashes;
  `notes`/`append_note` is `mod_notes_append_note`; `con` is refused as an id; `../x.html` and
  `C:x.html` are refused as entries; a 40-character id with a 40-character tool is too long (85 > 52);
  `/data/secret.txt` through a junction `data -> <outside folder>` is refused by `serve`; a `tabs.list`
  answer for a mod without `panel` lists only its own tab; `mint(["browser","eval"])` is refused;
  `"version": "2"` in `mods.json` is left alone and `"not json"` is moved to `mods.json.bad`.

## Protocol 2 (`plugin.json`), plugins phase 2

A plugin is a mod folder whose manifest is `plugin.json` with `"protocol": 2` (design:
`../superpowers/specs/2026-10-25-plugin-system-design.md`, sections 3, 4 and 13). Every rule above holds, and these
are added. `mod.json` must say protocol 1 and `plugin.json` protocol 2; a folder with both is refused. A protocol 1 mod
keeps exactly its rules, its five permissions and its Settings rows. Sample: `../../sample-plugins/manifest-tour/`
(loaded as valid by `flare_mods::discover::tests::the_sample_plugins_are_valid`).

- **Where.** `<profile>\plugins\<id>\`, the same folder as a protocol 1 mod; the folder holds one of the two files.
- **Status.** The keys below are parsed and validated, and every file they name must exist in the folder. Background
  pages run (phase 3, see "Background pages and events" below); the bar, the sidebar sections, the actions and the
  helper are not started yet (later phases). A plugin's tools are offered when it has a background page (which then
  answers them) or a panel.

| Field | Rule |
|---|---|
| `author` | Optional. The text rule of `name` (80 characters). |
| `homepage` | Optional. `https://` and a host, no space or user name, at most 200 characters. |
| `background` | Optional. An `.html` path in the folder. Needs no permission. Tools may then go without a panel. |
| `bar` | Optional. `{"entry": "bar.html", "height": 48}`, height 24 to 160. Goes with the `bar` permission, both ways. |
| `contributes.sidebar` | At most 2 `{key, title}`; keys `a-z0-9-`, 1 to 24, unique. Goes with `sidebar`, both ways. |
| `contributes.actions` | At most 8 `{key, title, icon?, where}`; `where` from `tab-menu`, `tab-hover` (one per plugin), `top-bar` (one per plugin), `session-bar` (needs a `bar`). `icon` is a built-in badge or one of `contributes.icons`. Goes with `actions`, both ways. |
| `contributes.icons` | At most 32 `name: path`; a name not a built-in badge; the path starts with `M`/`m`, at most 2 KB, at most 256 commands, every number within -1000 to 1000, path commands, numbers, signs, dots, commas and spaces only. |
| `helper` | Optional. `{"command": "bin\\helper.exe", "args": []}`: an `.exe` in the folder (the entry path rules), at most 16 args of 200 characters, no control characters. Goes with `process`, both ways. |

Permissions a `plugin.json` may also ask for, and the words Settings shows:

| Name | Words | Warning on the card |
|---|---|---|
| `tabs` | See all your tabs and what their agents are doing | with `network`: can send what your tabs show to the internet |
| `decorate` | Put badges and progress on tabs | |
| `sidebar` | Add a section to the sidebar | |
| `actions` | Add buttons to tabs and menus | |
| `bar` | Show a bar under the terminal | |
| `sessions` | Read what you and Claude say in sessions | with `input`: can read and steer Claude; with `network`: can send what you and Claude say to the internet |
| `input` | Type and run commands in your terminals (full control) | Full control |
| `network` | Use the internet | can change what it does from the internet |
| `process` | Run its own program on this PC (full access) | Full control, and it can leave things behind outside Flare |

In the channel each new method family has its own namespace opened by exactly its permission (`sidebar.*`,
`actions.*`, `bar.*`, `session.*`, `tabs.input`); nothing rides on the open `tabs.list`; `network` and `process` open no
method. `tabs` opens no method either: it lets a call name another tab (`Grant::may_act_on_any_tab`) and widens
`tabs.list` (phase 3, below). A plugin with no permission reaches only `hello`, its own-tab `tabs.list` and its own
store (`storage.*`, phase 5 below), which holds only what it put there; the agent never reaches a plugin's store.

The approval in `mods.json` (same file, version 1) keeps, for a plugin, `author`, `homepage`, `source` (once install
exists) and `hash` (`sha256:` over the folder's files). A plugin whose author or homepage changes is turned off until
the person switches it on again, as one that asks for more permissions is.

## Background pages and events (plugins phase 3)

Design sections 6, 10 and 13. Sample: `../../sample-plugins/browser-badge/` (a globe on every tab whose browser panel
is open).

- **The background page.** A protocol 2 plugin that is on and declares `background` gets one hidden view of it for as
  long as it is on: in the plugins' browser and the plugin's own profile (`flare-mod-<id>`, the storage its panel
  has), tied to no tab, never shown and never given the keyboard, sealed as a background view (no script dialog, no
  context menu, no browser keys, no download). At most 6 such plugins are on at once; turning on a seventh is refused
  with the reason. A page that ends (its process, its browser, a failed load) is started again at once, at most 3 times
  in 5 minutes; the next end stops it, and Settings shows "Stopped" under its switch until it is turned off and on.
  Every 10 seconds the host pings it (`{"t":"ping"}`, answered by the bridge itself); a ping with no answer after 10
  seconds closes the page and counts as an end, and so does a page still loading 10 seconds after it began (a script
  that never lets it finish). Everything the plugin drew on tabs goes when the page ends.
- **Turning off.** Every view of the plugin closes (its background page and its panels), its waiting calls fail and
  its decor goes before Settings shows it off; its generation moves on, so nothing started before is applied after
  (a badge, progress or `panel.open` checked before the switch is refused with `unauthorized`, even once it is on
  again).
- **Tools.** A plugin with a background page has its tools answered there (`window.flare.on('tool', ...)` in the
  background page); no panel is opened for the call. A protocol 1 mod, and a plugin with no background page, keep the
  panel routing of stage 8.
- **Events.** `window.flare.on(name, fn)` takes `tool`, `tabs`, `agent` and `plugin`, one handler each per page. The
  host hands an event to every page of the plugin that registered for it, at most 32 a page in one round (a round is
  one window refresh, which also coalesces what changed). `tabs` is `{kind, tab}` with `kind` one of `opened`,
  `closed`, `focused`, `renamed`, `moved`, `panel` (what the tab's panel shows changed) and `tab` the object of the wide
  `tabs.list` (`protocol.md`). `agent` is `{tab, kind, state, session}`. A page of a plugin that holds `tabs` hears of
  every tab; any other page (a protocol 1 mod's too) only of the tab it sits in (a background page, of none), and then
  only what own-tab `tabs.list` gives: `tab` is `{id, title, active}`, and no `agent` event comes (the folder, the
  agent and the session are for `tabs`). `plugin` is `{kind: "on"}`, said
  once when the handler is registered, and `{kind: "off"}` as the plugin is turned off.
- **Any tab.** With `tabs` a call may name any tab by its public id (`protocol.md`, section 4); without it, only its
  own tab, and a background page none. `panel.open` on a tab that is not the caller's own takes the panel only when it
  is closed or shows the plugin already; the person's web page or another panel there answers `busy`.
- **The budget.** One limiter per plugin, shared by all its pages (and later its helper): 50 messages a second with a
  burst of 100, 2 MB a second, and no message over 256 KB. A page's message over it is dropped before it is copied out
  of the page; a pipe connection's call over it answers `rate_limited`. Decor is 30 changes a second per plugin across
  its tabs, on top of 10 per tab. Protocol 1 mods keep the limits they had.
- **Safe mode.** Two starts in a row that end without closing properly while plugins are on make the next start keep
  every plugin off, with a row at the top of Settings > Plugins; `--no-plugins` and Shift held at launch do the same for
  one start. The switches are untouched: the start after has them back. The record is `<profile>\plugins-run.json`.

## Sidebar sections and actions (plugins phase 4)

Design sections 5.3, 5.4 and 13. Sample: `../../sample-plugins/todo/` (to-dos per session tab, ticked off by Claude or
by you).

- **Sections.** Each `contributes.sidebar` entry of a plugin that is on (and holds `sidebar`) is a section Flare draws
  under the tab list in the vertical tab layout, in the sidebar's look, under a header with the plugin's name (and the
  section's title when it differs). The plugin cannot style it. At most 6 sections are drawn in all, in the order you
  give them (the header's menu, a right-click or its dots: Move up, Move down, Fold), then the rest in the order the
  plugins were found; a click on the header folds it. The order and the folds are preferences
  (`plugins.sidebar.order`, `plugins.sidebar.folded`). The sections take at most 45 percent of the sidebar's height and
  scroll inside. In the horizontal layout a "Plugins" button in the title bar (shown while any section is drawn) opens
  the same sections as a dropdown.
- **`sidebar.set {key, items}`** replaces a section's items: `key` is one of the plugin's `contributes.sidebar` keys;
  `items` is a list of at most 50 objects `{id, text, sub?, icon?, checked?, tab?}` and nothing else. `id` (1 to 64
  characters, unique in the list), `text` and `sub` (at most 120 characters) are clean lines (no control or hidden
  formatting character); `icon` is a built-in badge or one of the plugin's `contributes.icons`; `checked` (true or
  false) draws a checkbox; `tab` is a tab the plugin may see (its own, any with `tabs`, or one a click gave it), and a
  click on the item brings that tab to the front. At most 5 calls a second for one section (`rate_limited`).
- **`actions.set {key, tab?, enabled?, checked?, title?}`** changes one of the plugin's `contributes.actions`: on the
  tab it names (one it may act on), on the caller's own tab when it names none, and on every tab when a background
  page names none (the top bar's button reads that one). A field left out keeps what it was; a tab's own state wins
  over the every-tab one field by field. `enabled: false` draws the action faint and its click does nothing; `checked`
  draws it ticked (the accent); `title` (at most 80 characters) replaces its words. At most 20 calls a second for one
  plugin.
- **Where actions show.** `tab-menu`: in a shell tab's right-click menu, one flyout per plugin under its name holding
  its actions. `tab-hover`: a small button in Flare's outlined chip on a tab while it is hovered (one per plugin, two on
  a tab at most; named tabs of 110 px or more and sidebar rows), left of the badges and never over the agent's ring.
  `top-bar`: a button left of the window buttons (one per plugin, 3 in all), kept apart from them by a gap. An action
  without an icon shows its title's first letter. `session-bar`: a chip at the right end of the plugin's own bar under
  a tab's terminal, while its bar shows there (plugins phase 6).
- **Events.** `window.flare.on('action', fn)` hears `{key, tab}` when you click an action (`tab` is the tab it was
  clicked on; for a top-bar button, the shell tab in front, or null). It goes to the plugin's background page when that
  page listens, else to every page of the plugin that does. `window.flare.on('sidebar.item', fn)` hears `{key, item}`
  for a click on an item and `{key, item, checked}` for a tick on its box (the box shows the new state at once; the
  plugin may set the list again). A tool call answered by a background page also says `tab`, the tab the agent called
  from.
- **Click scope.** A click on a plugin's action or item on tab X (a tab's menu or hover button, an item that leads to
  X, a top-bar button while X is in front) lets that plugin act on X as if it held `tabs` for that one tab, until X
  closes or the plugin is turned off. So a plugin needs `tabs` only for tabs nobody clicked.
- **When it stops.** A plugin turned off takes its sections, its actions and everything it set with it, and its click
  scopes; a background page that ends takes what it set (it sets it again when it starts). A closed tab takes the
  action state set for it.

## Sessions, typing and storage (plugins phase 5)

Design sections 6, 7 and 13.

- **`session` events (`sessions`).** `window.flare.on('session', fn)` hears what is said in the Claude session of a tab
  the plugin may see (its own, any with `tabs`, or one a click gave it): one event per entry, `{tab, session, role,
  text, tool?, at?, dropped?}`. `role` is `user` (a prompt), `assistant` (the reply's words) or `tool` (a tool call:
  `tool` is its name and `text` the one argument that says what it does, such as the command or the file, else its
  input as JSON). Tool results, the assistant's thinking, Claude Code's own notes and the lines of subagents are not
  entries. `text` is cleaned (no control or invisible character, line breaks kept as `\n`) and at most 2000 characters
  (a cut ends in `...`); `at` is the transcript's own time. Only what is said after the page began to listen comes
  (the earlier part is `session.read`). A plugin hears at most 50 entries a second; what is over is dropped and the
  next event it hears says how many in `dropped`.
- **The follower.** A tab's transcript (`<Claude folder>\projects\<folder>\<session>.jsonl`, the one the Changes
  panel reads) is followed only while a page of a plugin that holds `sessions` listens and may see that tab, and the
  tab's Claude session is known; it starts at the file's end and reads on a worker of its own. Nobody listening, the
  plugin turned off, a new session in the tab or the tab closed: it stops.
- **`session.read {tab?}` (`sessions`).** The last 200 entries of the tab's session (read from the transcript's last
  4 MB on the call's worker), oldest first, each `{role, text, tool?, at?}`: `{tab, session, entries}`. A tab with no
  known session answers `session: null` and no entries. Once a second per tab per plugin.
- **`tabs.input {tab?, text, enter?}` (`input`).** Types `text` into the shell of a tab the plugin may see, the way
  your paste goes: one bracketed paste when the program asked for one (Claude Code does), else the lines as a paste
  sends them, with no final Enter. `enter: true` presses Enter 300 ms later, only if the tab is still open and the
  plugin still on. `text` is 1 to 4096 bytes; any control character but a tab and line breaks is refused (so nothing
  can end the paste early or press a key of its own). At most 2 a second per tab. Answers `{typed, enter}`. It is the
  only way into a shell from outside Flare's own interface (regression rules 2 and 188): the agent never has it.
- **`storage.get {key?}` and `storage.set {key?, value}` (no permission, plugins only).** The plugin's own JSON object
  in `<profile>\plugins-storage\<id>.json`. `get` with a key gives `{value}` (null when the key is not there), with
  none the whole object; `set` with a key sets it (a null value removes it), with none `value` replaces the whole
  object (it must be one). Keys are 1 to 128 characters with no control character. The store is at most 1 MB as
  written; a set past that answers `too_large` and changes nothing. A set is answered at once; the file is written on a
  worker, at most twice a second (sets in between land together), to a `.tmp` file renamed over the old one. A file
  that is not a JSON object reads as empty. A protocol 1 mod has no store (`forbidden`), and neither has the agent.

## Session bar, network and audio (plugins phase 6)

Design sections 5.5, 5.6, 8 and 13.

- **`bar.show {tab?}` and `bar.hide {tab?}` (`bar`).** The plugin's strip (its manifest's `bar.entry`) under the
  terminal of a tab it may see (its own, any with `tabs`, or one a click gave it; a background page names one), as tall
  as `bar.height` (24 to 160, kept within those). `show` answers `{tab, height}`, `hide` `{hidden}` (false when the
  tab showed no bar of this plugin; another plugin's stays). One bar a tab: while another plugin's bar shows there,
  `show` is `busy` with that plugin's name. Each is at most once a second per plugin per tab (`rate_limited`).
- **The terminal gives up the height.** The grid is laid out shorter by the bar's height and the shell is resized once,
  the same way a window resize goes; nothing is animated, so no frame shows it half way. The bar shows only while its
  tab is in front with its terminal (not over Settings or a placeholder) and goes when the plugin stops (turned off, or
  its background page ended) or the tab closes; a bar page that ends takes its bar off the tab (the plugin may show it
  again).
- **The bar's page.** A browser view in the plugins' browser and the plugin's own profile, served like its other pages
  (the Serving rule), sealed as a bar: no script dialog, no right-click menu, no browser keys, no download, no new
  window, no permission, own frames only, and never the keyboard (a click that gives it focus hands the keyboard back
  to the terminal at once). It is made the first time its tab is in front with the bar, hidden while the tab is
  behind or a Flare menu or dialog is open (a browser view paints over Flare), and kept for at most two tabs a plugin
  (an older one behind is closed and made again when its tab comes back). It has the bridge like any page of the
  plugin: its calls act on the tab it sits in, and it hears the events it listens to.
- **`session-bar` actions.** Drawn by Flare as chips at the right end of the bar row (at most 4), with the action's
  icon or first letter and `actions.set` state; the page's place leaves them their room. A click is `action`
  `{key, tab}` and gives click scope on the tab.
- **`network`.** The plugin's pages (panel, bar, background) run in a networked browser of the plugins' own
  (`%LOCALAPPDATA%\Flare\WebView2-plugin-net`, its profile `flare-mod-<id>` there, so a plugin that gains or loses
  `network` starts with empty page storage): the mini-apps' WebRTC policy and no dead proxy, and a resolver for which
  `localhost`, `*.localhost`, `*.local`, `*.lan`, `*.internal` and `*.home.arpa` are not found. Every answer of its
  pages carries `default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src
  'self' data: https:; connect-src 'self' https:; media-src 'self' data: blob: https:; frame-src 'none'; form-action
  'self'; base-uri 'none'`: it may fetch, and load pictures and sound, over https, never plain http, scripts from the
  internet, frames or a form that posts elsewhere. The request wall lets a request out only to an https address on a
  public host: loopback, the private, shared and link-local ranges (IPv4 and IPv6), a host without a dot and the local
  names above are refused (`flare_surfaces::net`). It still cannot navigate away from its own address or open a window.
  Without `network` the policy is the sealed one above. A protocol 1 mod never has it.
- **`view.audio {path | url, title?, autoplay?}` (`view`).** Flare's own audio player in the plugin's panel (the agent's
  is `show_audio`): a sound file (mp3, wav, ogg, oga, opus, m4a, aac, flac, webm, weba; checked like `view.file` and
  copied beside the page, at most 100 MB), or a public https address (refused as above), which needs `network` too.
  Play, pause and seek; Space, Left and Right work in the page. It never loops, and plays by itself only with
  `autoplay: true`, once (a reload does not play it again). The page is Flare's: its policy lets in its own track or
  that address's origin only.

## Tab bar and sidebar contributions (review 2026-10-25)

Mods cannot draw on tabs, sidebar rows or the strip yet; the only screen area is the mod's panel page.
The review of what is missing, how VS Code, WezTerm, iTerm2, Kitty and Zellij do it, and a phased design
(first: an agent and mod tab indicator, `tabs.indicator` plus a `set_tab_indicator` MCP tool) is in
`C:\Users\Admin\Documents\Claude\research\flare\2026-10-25-mod-support.md`.
