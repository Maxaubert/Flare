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

- Folder: `%APPDATA%\Flare\mods\<id>\`. The folder name is the id.
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
| `tools` | list of objects | Optional. At most 32. Needs the `tools` permission. |

### Permissions

| Name | What it allows the mod | Notes |
|---|---|---|
| `panel` | Show its panel in a tab, and list every tab (id and title). | Required with a `panel`. `panel.open` from a mod takes only its OWN `mod:<id>`: any other kind (`web`, `changes`, `tasks`, `board`, another mod) is `forbidden`, because a web panel would reach the network in the user's shared profile (S4/S5). `panel.close` and `panel.state` are allowed. |
| `view` | Show content through `view.*` (image, file, text, html) in its own panel or the user's. | Same checks as the agent (rule 13, 14). `view.html` with `allow_network: true` is `forbidden` for a mod: the user's mini-app network switch is consent for the AGENT's pages, never a mod's. |
| `browser` | Drive the mod's own browser panel through `browser.*` (open, navigate, look, click, type, press, scroll, wait). | Never the user's Edge profile (S4), and never the shared web profile of the user's tabs. Its own profile (see "Which tab a mod acts on"). **v1 limit:** a mod that holds `browser` passes the grant, and the call is then answered `unavailable` ("a mod's own browser panel is not built yet"): the panel slot of a tab holds one panel, which for a mod is its page. A mod without `browser` is refused every `browser.*` method with `forbidden`, before anything runs. |
| `convo` | Read `convo.state` (the session's tasks and changes). | |
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
  (not private, so its `localStorage` survives a restart), inside the mini-apps' sealed browser, under
  `<profile>\WebView2-app-sealed` (decisions.md, stage 8). The agent's `browser_*` tools do not drive a mod's page
  either (`unavailable`: the panel belongs to a mod).
- A mod that is not shown in any tab has no bound tab: its `view.*` calls and its tool calls need one. A tool
  call opens the mod's panel in the CALLER's tab (a tab behind gets its page made hidden), so a call never fails
  because the panel was closed; it fails with `unavailable` when the page cannot be made (no WebView2 runtime).
- `tabs.list` for a mod follows protocol.md: every tab (id and title) when the mod holds `panel`, else its own.
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
  `localStorage` survives a restart), inside the sealed browser the mini-apps use, under
  `%LOCALAPPDATA%\Flare\WebView2-app-sealed` (with a `--user-data-dir`, beside that profile's `WebView2` folder);
  never the web panel's profile and never the user's Edge profile. A profile name is the only per-mod name WebView2
  allows inside one browser, so the folder is not `...\mods\<id>\profile`.
- **No network from a mod.** The page is served from its folder with a strict policy. A mod that needs
  the internet is not a v1 mod.
- **The manifest is untrusted.** Every field is checked before anything is used. A path is checked three
  times: its syntax when read, its canonical location on disk (links and junctions included) when the mod is
  found, and the same location again on every request its page makes (the Serving rule).
- **Discovery is bounded.** At most 64 mod folders are read (only folders whose names are legal mod ids count;
  each other folder is reported by name), at most 4096 directory entries are looked at (one more is an error,
  and the rest are not looked at), and a `mod.json` over 64 KB is refused before it is parsed.
- **Text a person reads is clean.** Names, titles and descriptions refuse control characters and the Unicode
  format characters (zero-width spaces, bidi overrides, the BOM): what Settings shows is what the agent reads.
- **A bad mod does not stop the others.** Each folder fails alone, with its reason.
- **Enabled state is stored atomically** (`%APPDATA%\Flare\mods.json`, `{"version":1,"mods":{"id":true}}`).
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
