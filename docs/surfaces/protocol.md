# Control channel protocol, version 1

The contract between an app (Flare today, Prism Terminal later) and everything that drives its
panels: the app's own UI, the MCP server (`flare-mcp`), and mods. It is written so a Node server can
implement it from this file alone. The Rust reference is `crates/flare-ctl` (types, grants, framing,
the connection state machine, the client) and `crates/flare-mcp` (a client). Where this file and the
code disagree, that is a bug in one of them: fix both in the same commit.

Status of the methods in Flare: the framework (transport, hello, grants, errors, limits, timeouts,
events) is built and tested in `flare-ctl` (stage 2, library part). The app wiring and the behaviour of
`panel.*` and `view.*` land with the panels (stage 2, proper), `browser.*` with the web panel (stage 3)
and the agent driving it (stage 4), `convo.state` with the conversation reader (stage 6),
`tools.respond` with mods (stage 8). Each method
below says which stage owns its behaviour; the names, params and grants are fixed now.

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
- **Discovery for a client**: environment variable `FLARE_CTL_PIPE` when set and non-empty, else the
  user's default name. The app sets `FLARE_CTL_PIPE` and `FLARE_TAB_ID` in every shell it starts, so a
  child finds the right app even when the owner runs two profiles.
- **Who may connect**: the pipe's security descriptor is `D:P(A;;GA;;;<user SID>)`, a protected DACL
  with one ACE, so only the current user (S1). Remote clients are rejected
  (`PIPE_REJECT_REMOTE_CLIENTS`). The server creates the first instance with
  `FILE_FLAG_FIRST_PIPE_INSTANCE`: if the name already exists it does not start, and it never serves on
  a pipe somebody else made. It never listens on a network.
- **Client check**: before sending a byte a client reads the server process id of the pipe
  (`GetNamedPipeServerProcessId`) and refuses a server that runs as another user. It opens the pipe
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
5. **Re-resolve the grant**: a connection with a token asks the app for the token's grant again; a
   token that is gone or whose mod is disabled answers `unauthorized` and closes (rule S5: a disabled
   mod's token is dead, also for connections already open). A connection with no token takes the
   app's current agent grant, so the user's `eval` switch is live without a reconnect. **A grant of
   kind `ui` from the app is never accepted over the pipe**: a token resolving to `ui` answers
   `unauthorized` (closes), an agent grant of `ui` answers `internal`. Belt and braces for S3: the
   library, not only the app, keeps the `ui` namespace off the pipe.
6. Grant allows the method? No: `forbidden` (stays). The reason text names the rule. A method in the
   `ui` namespace is refused here whatever the grant says.
7. Run the method with its **timeout**: 15 s, and 60 s for every `browser.*` method. The app's
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

**The tab is advisory, and a grant is not a sandbox.** `tab` in `hello` is asserted by the client: any
client of this user can name any tab (`tabs.list` hands the ids out), and the agent can run Bash as
that same user anyway. Grants limit what the MCP tool surface and mods can do; they do not defend one
process of the user against another. A method must therefore act only on the connection's own `tab`
(`ctx.tab`) and never on a `tab` field in its params, for the agent and mod grants alike.

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
refuses it, and never runs anything. A relative path is relative to the app's idea of the tab's folder,
so clients send absolute paths (`flare-mcp` makes them absolute).

`PanelInfo` is `{"kind": "image"|"text"|"web"|..., "title": string, "path"?: string, "url"?: string}`.

### hello

Params: `client` (string, required), `version` (string, required), `protocol` (number, optional, must
be 1 if present), `token` (string, optional), `tab` (string, optional, an existing tab id).

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
**and** this grant may call; a client uses it to decide which tools to offer. `tab` is null when the
client named none.

Errors: `bad_params` (missing client or version, token not a string), `unsupported_protocol` (closes),
`unauthorized` (unknown or disabled token; closes), `no_tab` (the named tab does not exist; the client
may retry hello), `already_hello`.

### tabs.list

Params: none. Result: `{"tabs": [{"id": string, "title": string, "active": bool}]}`. Ids and titles
only: never screen content (S3). Stage 2.

### panel.open

Params: `kind` (string, required; v1 accepts `"web"`), `url` (string, optional, http or https).
Opens the tab's panel (replacing an open one). For images, files and text use `view.*`.
Result: `{"panel": PanelInfo}`. Errors: `no_tab`, `bad_params`, `unavailable` (kind not built yet, or no
WebView2 runtime). Stage 2 (shell of the panel), stage 3 (`web`).

### panel.close

Params: none. Result: `{"closed": bool}` (`false` when none was open; not an error). Errors: `no_tab`.

### panel.state

Params: none. Result: `{"open": bool, "panel"?: PanelInfo, "width"?: number}` where `width` is the
slot's fraction of the window. Errors: `no_tab`.

### view.image

Params: `path` (string, required), `title` (string, optional). Shows an image in the tab's panel.
Result: `{"panel": PanelInfo}`. Errors: `bad_params` (no path, or not an image the viewer can decode),
`not_found`, `no_tab`, `forbidden` is not used here (a path the user cannot open answers `not_found`,
so a probe learns nothing). Stage 2.

### view.file

Params: `path` (string, required), `line` (number, optional, 1-based line to scroll to), `title`.
Shows a text or code file, read-only. A binary file answers `bad_params`. A file over 8 MB shows its
first 8 MB and says so in the panel. Result and other errors as `view.image`. Stage 2.

### view.text

Params: `text` (string, required, at most 4 MB), `title`, `language` (string, optional syntax hint).
Result `{"panel": PanelInfo}`. Errors: `bad_params`, `no_tab`. Stage 2.

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
| `panel.changed` | `{"tab": string, "open": bool, "panel"?: PanelInfo}` | the tab's panel opened, closed or changed |
| `tabs.changed` | `{}` | a tab was added, removed or retitled |

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
connection), says `hello` with `tab` from `FLARE_TAB_ID`, then calls:

| Tool | Params | Channel method |
|---|---|---|
| `show_image` | `path` | `view.image` |
| `show_file` | `path` | `view.file` |
| `close_panel` | none | `panel.close` |

(`show_diagram`, `show_html`, `open_page` follow in stage 5, `browser_*` in stages 3 and 4.) A tool
call waits for the method's own timeout plus 5 s (`call_wait`), not a fixed 20 s, so a 60 s
`browser.wait` is not given up on early; 20 s bounds only connecting and `hello`. Results are MCP
content: text, and image content (`{"type":"image","data","mimeType"}`) reserved for `browser_look`.
Outside Flare (no `FLARE_TAB_ID`, no pipe, no server) every tool answers `isError: true` with "Flare is
not running this session ...". A channel error `forbidden` or `not_found` becomes `isError` text
("Flare refused: ... (code)"), so the model sees it. A bad argument is `isError`, not a JSON-RPC error;
an unknown tool name is JSON-RPC `-32602`. An implementation in another app only needs the same tool
names and the same method mapping, so Prism Terminal can reuse the binary unchanged if its server
speaks this protocol.
