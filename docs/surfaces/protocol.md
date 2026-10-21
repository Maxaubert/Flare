# Control channel protocol, version 1

The contract between an app (Flare today, Prism Terminal later) and everything that drives its
panels: the app's own UI, the MCP server (`flare-mcp`), and mods. It is written so a Node server can
implement it from this file alone. The Rust reference is `crates/flare-ctl` (types, grants, framing,
the connection state machine, the client) and `crates/flare-mcp` (a client). Where this file and the
code disagree, that is a bug in one of them: fix both in the same commit.

Status of the methods in Flare (stage 2 is done): the framework (transport, hello, grants, errors,
limits, timeouts, events) is built and tested in `flare-ctl`. Flare starts the server at launch and
implements `hello`, `tabs.list`, `panel.close`, `panel.state` and `view.image|file|text`; `panel.open`
answers (`unavailable` for `web`, `bad_params` for anything else) but is not listed in `capabilities`.
`browser.*` lands with the web panel (stage 3) and the agent driving it (stage 4), `convo.state` with the
conversation reader (stage 6), `tools.respond` with mods (stage 8); until then each answers
`unavailable`. Each method below says which stage owns its behaviour; the names, params and grants are
fixed now.

Design: `../superpowers/specs/2026-10-21-surfaces-design.md` (section "Control channel").

## 1. Transport

- A **named pipe** (Windows). Node: `net.createServer().listen('\\\\.\\pipe\\<name>')`.
- **Name**: `\\.\pipe\flare-ctl-<h>` where `h` is 16 lower-case hex digits of FNV-1a 64 over the
  lower-cased string form of the current user's SID (`S-1-5-21-...`). With a **scope** (a profile
  folder, an e2e run) append `-<s>`: 8 hex digits of the low 32 bits of FNV-1a 64 over the lower-cased
  scope. FNV-1a 64: offset `0xcbf29ce484222325`, prime `0x100000001b3`, per byte
  `h = (h ^ byte) * prime` modulo 2^64. Known answers (a port pastes these into its own test; they are
  also asserted in `crates/flare-ctl/tests/pipe.rs`, `the_pipe_name_has_known_answers`):

  | Input | Result |
  |---|---|
  | FNV-1a 64 of `""` | `cbf29ce484222325` |
  | FNV-1a 64 of `"a"` | `af63dc4c8601ec8c` |
  | SID `S-1-5-21-1` | `\\.\pipe\flare-ctl-81be4daa4a464916` |
  | SID `S-1-5-21-1`, scope `e2e` | `\\.\pipe\flare-ctl-81be4daa4a464916-f1179b09` |
  | SID `S-1-5-21-1111111111-2222222222-3333333333-1001` | `\\.\pipe\flare-ctl-df660900cac14fc2` |

  `s-1-5-21-1` (lower case) and scope `E2E` give the same names as `S-1-5-21-1` and `e2e`.
- **Which scope Flare uses**: none for the default profile (`%APPDATA%\Flare`), so any client of the same
  user finds the plain name with no environment; for any other profile (an e2e run, a second profile of
  the owner) the scope is the profile folder as a string, so that Flare never meets the owner's. The shell
  gets the whole name in `FLARE_CTL_PIPE`, so a client never has to compute it.
- **Discovery for a client**: environment variable `FLARE_CTL_PIPE` when set and non-empty, else the
  user's default name. The app sets four variables in every shell it starts (the `pty_env` function,
  regression rule 12): `FLARE_CTL_PIPE` (this instance's pipe), `FLARE_CTL_PID` (the process id of the app
  that serves it), `FLARE_TAB_TOKEN` (the tab's secret, section 4) and `FLARE_MCP_EXE` (the absolute path of
  `flare-mcp.exe`, when it sits beside the app). **Only once the server is really serving**: if it could not
  start (the pipe name is taken, no SID) no shell is given a secret or a pipe name, and a copy the user
  already had is dropped too. A value the user
  already had under any of these names is replaced, and none is inherited. So a child finds the right app
  even when the owner runs two profiles, and a Flare started inside a Flare tab does not pass on the outer
  tab's secret. `FLARE_TAB_ID` still exists, for hook lines and Sonara; it is public and opens nothing.
- **Who may connect**: the pipe's security descriptor is `D:P(A;;GA;;;<user SID>)`, a protected DACL
  with one ACE, so only the current user (S1). Remote clients are rejected
  (`PIPE_REJECT_REMOTE_CLIENTS`). The server creates the first instance with
  `FILE_FLAG_FIRST_PIPE_INSTANCE`: if the name already exists it does not start, and it never serves on
  a pipe somebody else made. It never listens on a network.
- **Client check**: before sending a byte a client reads the server process id of the pipe
  (`GetNamedPipeServerProcessId`) and refuses a server that runs as another user, **and, when it was told
  which process serves the pipe (`FLARE_CTL_PID`), a server that is any other process**. The second rule
  closes the squatter: another process of the same user, at low integrity or under a restricted token (both
  still carry the user's SID), could create the predictable pipe name before the app starts, be handed the
  tab's secret and answer with any text that a tool result would carry to the model. `flare-mcp` requires
  `FLARE_CTL_PID`; without it, or with another process serving, it says Flare is not running and sends
  nothing. A client outside a Flare tab (no pid) may still use the plain check. It opens the pipe
  with `SECURITY_IDENTIFICATION`, so a server can learn who the client is but never act as it.
- **Reading**: the server never parks in a blocking read: it asks the pipe how many bytes are ready
  (`PeekNamedPipe`) and sleeps a few milliseconds when none are. A Node port gets the same from its
  event loop. While half a request line is buffered the server does not write queued events, so a big
  request and a big event write can never block each other on full pipe buffers.
- Byte mode, duplex, up to 255 instances; the app keeps one waiting and makes the next before it
  serves the one that just connected.

## 2. Framing

- **JSON lines**: one JSON object per line, UTF-8, terminated by `\n`. A trailing `\r` is ignored;
  blank lines are ignored. A message never contains a raw newline (JSON escapes it).
- **Size cap**: 16 MB (`16 * 1024 * 1024`) per line, without the newline, both directions. Past it
  the receiver answers `too_large` with a null id and closes that connection. A sender refuses to
  write a larger message instead of sending it.
- Request: `{"id": <number|string>, "method": "<name>", "params": {...}}`. `params` may be absent or
  null (read as `{}`) but if present must be an object.
- Response: `{"id": <same>, "result": {...}}` or `{"id": <same>, "error": {"code": "...", "message": "..."}}`.
  Exactly one of the two. `code` is a stable machine string (section 6); `message` is for people.
- Event (server push): `{"event": "<name>", "params": {...}}`, no id (section 7).
- A client tells the two apart by the `event` key. Responses to a connection's requests come in the
  order they were sent: the server handles one request at a time per connection.
- A client sends at most one request at a time and waits for its answer (the reference client does).
  Pipelining is not part of v1.

## 3. Connection life

1. The client connects and sends `hello` within **10 s**, or the server closes it.
2. Any other method before `hello` answers `hello_required`; the connection stays open.
3. After `hello`, requests run through the **order of checks** below.
4. The connection ends when either side closes, on an error marked "closes" in section 6, or when the
   server stops. Nothing survives a reconnect: a client says `hello` again.

### Order of checks for every request

1. Parse the line. Not JSON: `parse_error` (null id, closes). Not an object, bad id, no method,
   `params` not an object: `invalid_request` (closes).
2. **Rate limit**: a token bucket per connection, **100 requests/s sustained, burst 200**. Every
   request takes a token, refused ones and `hello` retries included, and it is checked before the app
   is asked anything, so a spammer cannot drive unlimited callbacks into the app. Empty bucket:
   `rate_limited` (stays).
3. Method name in the protocol? No: `unknown_method` (stays).
4. `hello` again: `already_hello` (stays). `hello` itself resolves the grant (below) and answers.
   Any other method before `hello`: `hello_required` (stays).
5. **Re-resolve the grant and the tab**: a connection with a tab token asks the app which tab owns it
   again (a closed tab's token stops resolving, and the connection then has no tab: `no_tab`). A
   connection with a token asks the app for the token's grant again; a
   token that is gone or whose mod is disabled answers `unauthorized` and closes (rule S5: a disabled
   mod's token is dead, also for connections already open). A connection with no token takes the
   app's current agent grant, so the user's `eval` switch is live without a reconnect. **A grant of
   kind `ui` from the app is never accepted over the pipe**: a token resolving to `ui` answers
   `unauthorized` (closes), an agent grant of `ui` answers `internal`. Belt and braces for S3: the
   library, not only the app, keeps the `ui` namespace off the pipe.
6. Grant allows the method? No: `forbidden` (stays). The reason text names the rule. A method in the
   `ui` namespace is refused here whatever the grant says.
7. Run the method with its **timeout**: 15 s, and 60 s for every `browser.*` method. The app's own
   waits on the UI thread end earlier (10 s, one deadline shared by every step of a call, `view.*` included),
   and the request carries that deadline: a request the window picks up after it changes nothing, so a
   call that was answered `timeout` never acts later. The app's
   handler runs off the connection's thread; when the timeout passes the client gets `timeout` and the
   late result is discarded. Only handlers **past their timeout** that are still running count as
   stuck: if more than 32 are, new calls answer `unavailable` until they finish. Slow calls still
   inside their timeout (many agents in a `browser.wait`) never count. When the server stops, a
   waiting call answers `unavailable` at once.

The handler (`Dispatcher::call`) may still be running, or start, after the server is told to stop. The
app must make it return quickly then, and must not touch its UI thread in a way that can panic or
block (answer `unavailable`). `Server::stop` reports how many handlers are still running so the app
can wait on them inside its quit cap.

### Limits

| Limit | Value |
|---|---|
| Line size | 16 MB |
| Clients | 64 at once. The 65th is accepted, sent `too_many_clients` (null id), and closed. |
| First hello | 10 s |
| Request timeout | 15 s; 60 s for `browser.*` |
| Rate | 100/s, burst 200, per connection |
| Queued events per connection | 256, then the oldest is dropped |
| Stuck handlers | 32 |

`hello` returns the limits in force, so a client does not hard-code them.

## 4. Grants (who may call what)

Three kinds. A grant is decided at `hello` and re-read on every request (check 4).

| Grant | Who | How it is issued |
|---|---|---|
| `agent` | Claude Code through `flare-mcp`, any MCP client, anything with no token | `hello` without a token |
| `mod` | A mod's helper or page | `hello` with the mod's token; the grant is the mod manifest's permissions |
| `ui` | The app's own code | **in process only**, never over the pipe. No token can mean `ui`. |

Permissions of a `mod` grant: `panel`, `view`, `browser`, `eval`, `convo`, `tools`. The `agent` grant
holds `panel`, `view`, `browser` and `convo`, plus `eval` only when the user has turned `browser_eval`
on (S4). The `permissions` list in the `hello` answer always agrees with what the grant can call
(`permission_names_agree_with_what_each_grant_can_call`).

**A tab is named by its secret (rule S1).** Flare makes a random token for every tab when it starts the
tab's shell (64 lower-case hex digits from two random UUIDs, the OS generator: 244 bits) and puts it in
that shell's environment as `FLARE_TAB_TOKEN`; never on a command line, never logged, never sent to
Sonara, never in a hook line. `hello` presents it as `tab_token`; the server maps token to tab through
the app and the connection's tab is that tab. There is no way to name a tab any other way: a `tab` param
in `hello` is refused (`bad_params`), the ids `tabs.list` returns are not secrets and open nothing, and
an unknown token is `unauthorized` (closes). A token lives as long as its shell: a restored tab gets a new
one at every launch, a closed tab's stops working at once, a warm spare shell belongs to no tab until one
adopts it. Compared in constant time. **A grant is still not a sandbox**: the agent can run Bash as the
same user, so a process that can read another process's environment can still read a token; the token
keeps unrelated processes of the user (a script, a browser tab, another app) from acting as a tab, and
keeps the tab's identity out of anything that only sees `tabs.list`. A method acts only on the
connection's own tab (`ctx.tab`) and never on a `tab` field in its params, for the agent and mod grants
alike.

| Method | agent | mod needs | ui |
|---|---|---|---|
| `hello`, `tabs.list` | yes | (always) | yes |
| `panel.open`, `panel.close`, `panel.state` | yes | `panel` | yes |
| `view.image`, `view.file`, `view.text` | yes | `view` | yes |
| `browser.open` ... `browser.wait` | yes | `browser` | yes |
| `browser.eval` | only if the user switched eval on | `browser` and `eval` | yes |
| `convo.state` | yes (its own tab) | `convo` | yes |
| `tools.respond` | no | `tools` | yes |
| `shell.input`, `settings.set`, `mods.enable` | **no** | **no** | yes |

**Rule S3** is the last row and the `agent` column: the agent can show content and drive its panel
browser. It cannot type into a shell, change settings, enable mods, answer as a mod, or read other
tabs' screens. The three methods in the last row are reserved names that exist only so the grant table
can pin their refusal: nothing over the pipe reaches them, whatever token or permission is held, and
the in-process UI path does not use the channel for them at all in v1.

The pure function is `Grant::may_call` in `crates/flare-ctl/src/grant.rs`; its tests are the table
above. A port must keep the same table and the same tests.

## 5. Methods

`tab` below is the tab of the connection (from `hello`). Methods that act on a tab fail with `no_tab`
when the connection has none. Paths are **resolved again by the backend** (S2): the app opens only
files the user could already open, shows a runnable file (an `.exe`, `.bat`, a script) as text or
refuses it, and never runs anything. A relative path on the channel is relative to the app's idea of
the **tab's** folder (its tracked cwd); an absolute path never asks the UI for a folder. `flare-mcp`
makes a path absolute against **its own** working folder (the agent's, where Claude Code started it)
before it sends it, and its tool text says so; a client that wants the tab's folder sends the path as it
is, from its own connection.

`PanelInfo` is `{"kind": "image"|"text"|"web"|..., "title": string, "path"?: string, "url"?: string}`.

### hello

Params: `client` (string, required), `version` (string, required), `protocol` (number, optional, must
be 1 if present), `token` (string, optional, a mod's token), `tab_token` (string, optional, the tab's secret
from `FLARE_TAB_TOKEN`). A `tab` param, whatever its value, is refused with `bad_params`.

Result:

```json
{"protocol": 1,
 "server": {"name": "flare", "version": "0.1.0"},
 "grant": {"kind": "agent", "permissions": ["panel", "view", "browser"]},
 "capabilities": ["hello", "tabs.list", "panel.open", "view.image"],
 "tab": "tab-7",
 "limits": {"max_line": 16777216, "max_clients": 64, "timeout_ms": 15000,
            "browser_timeout_ms": 60000, "rate_per_sec": 100}}
```

`grant` gains `"id": "<mod id>"` for a mod. `capabilities` is the methods this build implements
**and** this grant may call; a client uses it to decide which tools to offer. `tab` is the id of the tab
the token belongs to, null when the client presented none.

Errors: `bad_params` (missing client or version, a token or tab_token that is not a string, a `tab`
param; the client may retry hello), `unsupported_protocol` (closes), `unauthorized` (unknown or disabled
mod token, unknown tab token; closes), `already_hello`.

### tabs.list

Params: none. Result: `{"tabs": [{"id": string, "title": string, "active": bool}]}`. Ids and titles
only: never screen content (S3). The agent grant gets **its own tab only** (an empty list when the
connection has no tab); a mod gets every tab when it holds `panel`, else its own. `title` is the label the
tab strip shows, `active` is "in front". Stage 2.

### panel.open

Params: `kind` (string, required; v1 accepts `"web"`), `url` (string, optional, http or https).
Opens the tab's panel (replacing an open one). For images, files and text use `view.*`.
Result: `{"panel": PanelInfo}`. Errors: `no_tab`, `bad_params` (no kind, or a kind other than `web`),
`unavailable` (`web` is not built yet, or there is no WebView2 runtime). In Flare today `web` always
answers `unavailable`; stage 3 builds it.

### panel.close

Params: none. Result: `{"closed": bool}` (`false` when none was open; not an error). Errors: `no_tab`.
The panel keeps what it showed, so the user's toggle chord (Ctrl+Shift+P) brings it back. If the user had
moved the keys into the panel they go back to the terminal. Stage 2.

### panel.state

Params: none. Result: `{"open": bool, "panel"?: PanelInfo, "width"?: number}` where `width` is the
slot's fraction of the page (the user's splitter position, else the default 0.4). Both are absent when
`open` is false (a closed panel is not reported). Errors: `no_tab`. Stage 2.

### view.image

Params: `path` (string, required), `title` (string, optional, one line, cut at 200 characters, control
characters become spaces; the header shows it instead of the file's name). Shows an image in the tab's
panel: png, jpg, gif, bmp, webp or ico (not svg: a vector file can carry script). Result:
`{"panel": PanelInfo}`, answered once the panel holds the request (the decode follows on a worker; a file
that turns out unreadable is shown as a reason in the panel, the call has already answered).

Rules (S2, regression rule 187): the path is resolved again by the app on the call's own thread, a
relative one against the tab's folder, with the same function and the same click allowlist as a click on
a path on the screen. Errors: `bad_params` (no path, a path with a NUL or over 4096 characters, a
`title` that is not a string, or a file that is not a picture: "show_file shows it"), `not_found` (the
file is missing, is a folder, is on a share, or is not on the allowlist, a program for instance: all the
same answer, so a probe learns nothing about the disk), `no_tab`, `unavailable` (Flare is shutting down),
`timeout` (the window did not answer in 10 s). A file whose name says picture but which the file system
names something else (a link to a program) is `bad_params`, never shown. The explicit ask is not held
back by the "Open previews in Flare" switch (that switch governs clicks only). A call for a tab that is
not in front stores that tab's panel and does **not** switch tabs; it shows when the tab comes to the
front. Stage 2.

### view.file

Params: `path` (string, required), `line` (whole number from 1, optional), `title`. Shows a text or code
file, read-only, in the panel: the types in `flare_surfaces::preview::TEXT_EXTS` (txt, md, json, yaml,
toml, rs, ts, and so on; no script: a runnable file is refused, never shown as text and never run, rule
14). The first 4 MB and 100 000 lines are shown and the panel says so under the text; a file with a NUL
in its first 8 KB (UTF-16 with a BOM excepted) shows "This file is not text." in the panel. `line` is
validated and **ignored** in Flare today (scrolling to a line is not built). Result and errors as
`view.image` (a picture through `view.file` is `bad_params`: "show_image shows it"). Stage 2.

### view.text

Params: `text` (string, required, at most 4 MB = 4194304 bytes; an empty string is allowed on the wire,
the MCP tool refuses it), `title` (as `view.image`; default "Text"), `language` (string, optional syntax
hint, checked and ignored today). Shows the text in the same read-only viewer a file gets (tabs become
four spaces, long lines are cut, at most 100 000 lines), replacing what the panel showed. The text lives
in memory for this run: it is **not** written to `panels.json` and is not restored. Result
`{"panel": PanelInfo}` with no `path`. Errors: `bad_params` (no text, not a string, over 4 MB, a title
or language that is not a string), `no_tab`, `unavailable`, `timeout`. Stage 2.

### browser.* (stage 3 for the panel, stage 4 for the agent driving it)

All act on the tab's web panel and take up to 60 s. They answer `unavailable` when no web panel is open
(except `browser.open`, which opens it) or no WebView2 runtime is installed. While one runs the panel
shows a ribbon that the agent is using the page; the user can take over at any time.

| Method | Params | Result |
|---|---|---|
| `browser.open` | `url` | `{"panel": PanelInfo, "url": string, "title": string}` |
| `browser.navigate` | `url`, or `action`: `back`, `forward`, `reload` | `{"url", "title"}` |
| `browser.look` | none | `{"url", "title", "image": {"mime": "image/png", "data": base64, "width", "height"}, "elements": [Element]}` |
| `browser.click` | `ref`, or `x` and `y` (page pixels) | `{}` |
| `browser.type` | `text`, `ref` (optional; focuses it first) | `{}` |
| `browser.press` | `key` (`Enter`, `Tab`, `Escape`, `Control+a`, ...) | `{}` |
| `browser.scroll` | `dx`, `dy` (page pixels, default 0), `ref` (optional) | `{}` |
| `browser.wait` | one of `text`, `selector`, `ms`; `timeout_ms` (max 60000) | `{"met": bool}` |
| `browser.eval` | `expression` | `{"value": any}` (grant-gated, S4) |

`Element` is `{"ref": string, "role": string, "name": string, "rect": [x, y, w, h]}`; `ref`s are valid
until the next `browser.look` or navigation, and a stale one answers `not_found`. Only `http` and
`https` URLs open (and `about:blank`); `file:`, `javascript:` and custom schemes answer `bad_params`.
The web panel never uses the user's own browser profile (S4).

### convo.state (stage 6, reserved)

Params: none. Result: `{"session"?: string, "tasks": [...], "changes": [...]}`. Until the reader
exists it answers `unavailable`.

### tools.respond (stage 8, reserved)

A mod answers a tool call the agent made through `flare-mcp`. UI-routed in v1; named now so the grant
table is complete.

### shell.input, settings.set, mods.enable (never over the pipe)

Reserved names. Every grant answers `forbidden` ("is for the app's own interface only").

## 6. Errors

| Code | Meaning | Closes? |
|---|---|---|
| `parse_error` | line is not JSON | yes |
| `invalid_request` | JSON but not a request (no id, bad method or params shape) | yes |
| `too_large` | line passed 16 MB | yes |
| `too_many_clients` | server is at 64 clients | yes |
| `unauthorized` | token unknown or mod disabled | yes |
| `unsupported_protocol` | `hello.protocol` is not 1 | yes |
| `unknown_method` | not a method of the protocol | no |
| `forbidden` | the grant does not allow it (S3, S5, S4) | no |
| `hello_required` | first request was not `hello` | no |
| `already_hello` | second `hello` | no |
| `bad_params` | params do not fit the method | no |
| `no_tab` | the method needs a tab and there is none | no |
| `not_found` | a path, ref or panel does not exist (or may not be seen) | no |
| `unavailable` | known method, cannot be done now (no runtime, nothing open, reserved, overloaded) | no |
| `timeout` | not answered within its timeout | no |
| `rate_limited` | too many requests | no |
| `internal` | the server failed | no |

On a closing error the server sends the error response first (with the request's id when it could be
read, else `null`) and then closes. A client that sent a request and gets a response with `"id": null`
and an `error` treats it as the answer to that request. Because the server may close right after
writing, a client whose write fails with a broken pipe should still try to read one line before
reporting "closed" (the reference client does, for up to 500 ms). Codes may be added within v1; a
client treats an unknown code as a plain failure.

## 7. Events

Pushed by the server, never answered. Delivered only to connections that have said hello; an event
about a tab goes to connections of that tab. Best effort: the queue per connection is 256 and drops the
oldest, so an event is a hint to ask again (`panel.state`), not the only record.

| Event | Params | When |
|---|---|---|
| `panel.changed` | `{"tab": string, "open": bool, "panel"?: PanelInfo}` | the tab's panel opened, closed or changed: by a `view.*` or `panel.close` call, a click on a path, the close button or the toggle chord |
| `tabs.changed` | `{}` | a tab was added, removed or retitled (**reserved**: Flare does not send it yet) |

## 8. Versioning

`protocol` is 1. Within v1 the server may add methods, params, result fields, events and error codes;
a client ignores what it does not know. Anything else (renaming, removing, changing meaning, tightening
a grant for an existing method) is v2, and a v2 server still answers a `protocol: 1` hello with v1
behaviour or `unsupported_protocol`.

## 9. MCP server (`flare-mcp`)

A client of this channel, not part of it. A stdio MCP server (JSON-RPC 2.0, one message per line,
protocol versions `2025-06-18`, `2025-03-26`, `2024-11-05`; the client's version is echoed when listed,
else the newest). Methods: `initialize`, `notifications/initialized` (no reply), `ping`, `tools/list`,
`tools/call`. It starts with or without Flare. A tool call connects (once, reconnecting after a stale
connection), says `hello` with `tab_token` from `FLARE_TAB_TOKEN` (the pipe from `FLARE_CTL_PIPE`, and
only if `FLARE_CTL_PID` is the process that serves it), then
calls:

| Tool | Params | Channel method |
|---|---|---|
| `show_image` | `path` | `view.image` |
| `show_file` | `path` | `view.file` |
| `show_text` | `text` (non-empty), `title` (optional) | `view.text` |
| `close_panel` | none | `panel.close` |

(`show_diagram`, `show_html`, `open_page` follow in stage 5, `browser_*` in stages 3 and 4.) A tool
call waits for the method's own timeout plus 5 s (`call_wait`), not a fixed 20 s, so a 60 s
`browser.wait` is not given up on early; 20 s bounds only connecting and `hello`. Results are MCP
content: text, and image content (`{"type":"image","data","mimeType"}`) reserved for `browser_look`.
Outside Flare (no `FLARE_TAB_TOKEN`, no `FLARE_CTL_PID`, no pipe, no server) every tool answers `isError: true` with "Flare is
not running this session ...". A channel error `forbidden` or `not_found` becomes `isError` text
("Flare refused: ... (code)"), so the model sees it. The words of a refusal are cleaned first (one
line, control characters turned to spaces, at most 200 characters), because the text came over a pipe. A bad argument is `isError`, not a JSON-RPC error;
an unknown tool name is JSON-RPC `-32602`. An implementation in another app only needs the same tool
names and the same method mapping, so Prism Terminal can reuse the binary unchanged if its server
speaks this protocol.

### How Claude Code finds it (the bundled plugin)

The `flare-status` plugin (`crates/flare-agent/claude-plugin`, handed to Claude Code through
`CLAUDE_CODE_PLUGIN_DIRS` in every Flare shell) declares the server in its `plugin.json`, so Claude in a
Flare tab has the four tools with no setup. The shipped `plugin.json` is a **template** (like
`hooks.json`): its server's `command` is `@@FLARE_MCP_EXE@@`, and the install step writes the absolute path of
`flare-mcp.exe` beside the running app into the installed copy:

```json
"mcpServers": {"flare": {"command": "C:\\Path\\To\\Flare\\flare-mcp.exe", "args": []}}
```

When that file is not beside the app (a development build, a package without it) the `mcpServers` block is
left out, so Claude Code has nothing to fail on and the status hooks in the same plugin are unaffected.
Nothing relies on Claude Code expanding `${VAR}` in a manifest. The tab's secret, the pipe and the server's
process id reach the server through Claude Code's own environment (a stdio server inherits it); they are never
in `args` and never in the manifest. The installed plugin copy (`%LOCALAPPDATA%\Flare\claude-plugin`)
is refreshed from the template at every start, so a new manifest reaches existing installs and a program that
appears later is written in.
