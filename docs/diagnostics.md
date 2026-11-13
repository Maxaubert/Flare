# Diagnostics

Flare keeps a small log of what happened, so a stall, a crash or an odd agent state can be read
afterwards. It is written by `crates/flare-diag` and read with `cargo diag`. The log is local: nothing
in it leaves the PC (see `PRIVACY.md`).

## Where the log is

- The log: `%APPDATA%\Flare\logs\diag.jsonl`. When it reaches 2 MB it rotates to `diag.jsonl.1`, and
  up to `diag.jsonl.4` are kept, so there are never more than five files.
- The verbose switch: `%APPDATA%\Flare\diag.json`, written as `{"verbose":true}` (or false). It survives
  a relaunch.
- A test profile (`--user-data-dir=<folder>`) keeps its own log under `<folder>\logs\`.

The log starts when a session opens and ends with a `quit` line. A quit flushes the queue on the
calling thread, so the last lines are on disk before the process exits. A crash flushes the queue
from the panic path. If the disk is slow, the writer never blocks the UI: lines queue (at most 2000)
and the newest are dropped and counted when the queue is full.

## Reading it

    cargo diag                       the newest problems, each with the crumbs before it
    cargo diag -- --since 10m        only the last ten minutes (ms, s, m, h, d; default minutes)
    cargo diag -- --all              every line, not only the problems
    cargo diag -- --kinds main-lag,fs-slow   only these kinds
    cargo diag -- --agent            the agent lines only
    cargo diag -- --agent --tab <id> the agent lines of one tab
    cargo diag -- --web              the web panel lines only (add --tab <id> for one tab)
    cargo diag -- --dir <folder>     any folder that holds a diag.jsonl
    cargo diag -- --profile <folder> that profile's logs folder
    cargo diag -- --n 50             at most 50 entries
    cargo diag -- --utc              print times in UTC, not local time

The reader sorts by `t`, shows up to five crumbs within 120 seconds before each problem, and escapes
terminal and bidi control characters, so a hostile log line cannot write to the console.

## How to read a stall

A stall is a line that says the app was slow, and the lines around it say where.

- **`main-lag`**: the UI thread's 50 ms heartbeat arrived 100 ms or more late. The line carries the
  work items that were running or ended inside the lag (`inflight`). A work item that ended inside
  the lag is named `done`, so a single long call is easy to spot.
- **`ui-stack`**: the UI thread missed two seconds of heartbeats. It names the work items running then,
  and a sampled stack when the sampler is on.
- **`frame-slow`**: one frame (update and paint) took 200 ms or more. It carries the last five crumbs.
- **`work-slow`**: a timed UI-thread work item ran over 500 ms (a synchronous one over 100 ms). Items
  that wait on the user are never slow.
- **`fs-slow`**: a worker thread stats the profile folder every five seconds; one stat that took 500 ms
  or more is logged.
- **`mem-high`**: a process passed 1024 MB (the app) or 2048 MB (the GPU, or the children). It is logged
  again at each doubling, and a memory crumb is written every five minutes.
- **`unresponsive` and `responsive`**: Windows says the window stopped answering (`IsHungAppWindow`)
  and then answered again, with how long it took.

## The kinds

`src` is `main` for the app's own events and `app` for crumbs, marks and memory. There is no page:
Flare is one process. Every line also has `t` (ISO UTC), `up` (monotonic ms) and `k` (the kind), and
a field can never overwrite those four.

| Kind | When | Source |
|---|---|---|
| `session` | a session opens (app, version, runtime, graphics backend, windows, pid, verbose, cpus, ram, `e2e` under test) | `session.rs` |
| `quit` | the app is quitting; always the last line of a session | `session.rs` |
| `session-end` | the session closed; the queue is flushed at once | `session.rs` |
| `main-lag` | the UI heartbeat was 100 ms or more late | `stall.rs` |
| `ui-stack` | the UI thread missed 2 s of heartbeats | `stall.rs` |
| `frame-slow` | a frame ran 200 ms or more, with the last 5 crumbs | `stall.rs` |
| `work-slow`, `work-error` | a timed work item over 500 ms, or a sync one over 100 ms; or one that failed | `stall.rs` |
| `fs-slow` | a stat of the profile folder took 500 ms or more | `stall.rs` |
| `mem-high` | a process passed its threshold, then at each doubling | `mem.rs` |
| `panic` | a Rust panic: the message and a stack, capped | `crash.rs` |
| `render-error` | the renderer reported an error | `crash.rs` |
| `gone` | a shell or the GPU device ended, with its reason and exit code | `crash.rs` |
| `unresponsive`, `responsive` | Windows reports the window hung, then answering again | `stall.rs` |
| `update-check` | an update check: offered, fetch and offer times, rss | `flare-update` |
| `crumb` | a crumb (below) | `mem.rs`, `diag_ctl.rs` |
| `mark` | a user mark (no note; see below) | `control.rs` |
| `agent-hook`, `agent-title`, `agent-mark`, `agent-restore` | the agent indicator (M6), see below | `agent_diag.rs` |
| `agent-poll-failing` | the agent poll's process query failed three times in a row, so agent marks go missing (a problem line) | `agent_hub.rs` |
| `web-refused`, `web-popup`, `web-close-ignored`, `web-permission` | the browser panel refused a navigation or a pop-up, opened a pop-up or sent a new-tab link to the panel, ignored `window.close()` or denied a permission, see below | `web_diag.rs`, `web_ctl.rs` |
| `web-load-failed`, `web-process`, `web-cert` | a page failed to load, a browser process failed, or a certificate error (problem lines), see below | `web_diag.rs`, `web_ctl.rs` |
| `web-nav` | the chain of sites a page went through; verbose only | `web_diag.rs`, `web_ctl.rs` |
| `logger-error`, `logger-dropped` | the log could not write, or dropped lines (said once, then counted; past twice the 2 MB cap, while rotation keeps failing, lines are dropped and counted too) | `writer.rs` |

The kinds the page version wrote have no native equivalent, so they are not written:

- `page-stall` becomes `frame-slow`; `page-stack` becomes `ui-stack`.
- `ipc-slow` and `ipc-error` become `work-slow` and `work-error`.
- `page-error` and `main-error` become `panic` (the hook also writes `render-error`).
- `page-rejection` and `main-rejection` are dropped: Rust has no unhandled-rejection event.
- `page-task`, `page-batch` and the page's text sources are dropped.

**`mark` carries no note.** The page version took a typed note. Flare does not: the `mark` line is
`{}`, so the log holds no typed text at all, with no exception. `mark` is a bare signal.

## The crumbs

A crumb is a small line the app writes as things happen, so a later problem has its context. They are
kept in a ring of 50 and written as `k = crumb` with a field `a`:

| `a` | Fields | Written by |
|---|---|---|
| `tab-open` | `id`, `cwd`, `resume` | AppCtl, when a tab is added (restore, Ctrl+T, the + button, a hand-off, a drop) |
| `tab-close` | `id`, `kind` | AppCtl close flow |
| `tab-switch` | `id` | AppCtl front change |
| `settings-page` | `page` | SettingsCtl |
| `update-open`, `update-install` | `version`, `preview` | UpdateCtl |
| `update-check` | `offered`, `fetchMs`, `offerMs`, `rssMb` | the engine's crumb |
| `dictation` | `phase` (`listening`, `transcribing`, `idle`) | the dictation controller |
| `shell-spawn` | `id`, `pid`, `shell`, `cwd`, `resume`, `ms`, `warm` | `flare-term` host event |
| `shell-exit` | `id`, `pid`, `exitCode` | same |
| `shell-spawn-failed` | `id`, `shell`, `cwd`, `err` | same |
| `kill-stuck` | `pid`, `ms` | the kill path |
| `memory` | `appMb`, `gpuMb`, `childrenMb`, `workingSetMb` | `MemWatch`, every 5 min |
| `first-run` | `version` | the Velopack first-run hook |
| `path-click` | `how` (`click`, `menu-open`, `menu-reveal`), `route` (`panel`, `opened`, `revealed`, `missing`, `refused`), `ext`, `ms` | TermUi, once per file link click or menu Open (owner, 2026-11-13: a click that opened nothing left no trace) |
| `open-failed` | `via` (`file`, `folder`, `reveal`), `ext`, `err` (the opener's own fixed text, at most 80 characters) | `WinOpeners`, when Windows refused the open |

`ext` is a fixed word, never the screen text: an extension from the click allowlist, `none` or `other`
(`flare_term::path_open::ext_label`). No crumb at all after a click means Flare saw no file link there.
`opened` means the path was handed to Windows' default app; whether that app then showed it is the app's own log.

## The agent kinds (M6)

The agent indicator writes one line each time an agent's state moves: `agent-hook` (a hook signal
arrived), `agent-title` (the tab's title meaning changed), `agent-mark` (the tab mark moved, with the
rule that moved it) and `agent-restore` (a restored tab, and how long after the start). Repeated
signals are folded, a flood is capped per tab, and the next line says how many were left out. Free
text from the agent never reaches the log: a hook value is kept only when it is in the allowed list,
and anything else is written as `other`.

## The web kinds (rules 247, 248)

The browser panel writes a line when something goes wrong in it, so a failed hand-off (a shop's pop-up to a bank's 2FA page)
can be read afterwards. Every line has `id` (the tab), `view` (`web` for the browser panel, `app` for a mini-app, `mod` for a
plugin's page), `site` and, when there was a page before it, `from` (that page's site).

| Kind | Fields besides those | A problem |
|---|---|---|
| `web-refused` | `why`: `scheme` (the browser panel opens http, https and about:blank only), `not-its-page` (a mini-app), `not-its-host` (a plugin), `frame` (a mini-app's or plugin's frame to another site), `app-link` (a mini-app's or plugin's `mailto:` or app link, refused; or the browser panel's, which the browser never opens and Flare asks the person about, site and scheme only, never the link), `popup` (a window of an address that may not open, or any window of a mini-app or plugin), `popup-blocked` (no click of the person's asked for it), `popup-limit` (2 pop-ups a tab or 4 in all are open) | no |
| `web-popup` | `to: panel` (a plain new-tab link opened in the panel) or `to: popup` (a pop-up opened over the page, rule 249), `user` (the person's click opened it) | no |
| `web-close-ignored` | none: the page called `window.close()` | no |
| `web-load-failed` | `error`: the WebView2 error's name (`cert-expired`, `name-not-resolved`, `timeout` ...). A navigation Flare cancelled and a stream handed to the player are not failures | yes |
| `web-process` | `process` (`browser`, `render`, `gpu` ...), and `reason` (`crashed`, `out-of-memory` ...) and `exit` when the Runtime says them | yes |
| `web-cert` | `error`: the certificate error's name. Logged only: the browser's own handling is unchanged | yes |
| `web-permission` | `permission` (`camera`, `microphone`, `location`, `notifications` ...), `user` | no |
| `web-nav` | none: one step of the chain of sites. Verbose only | no |

The gate: one line per kind, site and word per 10 seconds, the copies counted and written as `repeats` once the 10 seconds have
passed (`repeats` is always the events a line stands for besides itself); at most 10 lines per kind per 10 seconds, and the held lines wait for their kind's next 10 seconds. `web-process` and `web-cert` always write their first line, even over that. `web-nav` lines are never folded: a hop back to a site seen a moment ago is its own line, in order.
The page events (`web-nav`, `web-load-failed`, `web-cert`) are the browser panel's only.

## Wiring and the quiet level

- The writer is a thread with a queue of 2000 lines and a flush every 250 ms. A write on the calling
  thread never does I/O, and a poisoned lock is recovered, so the log cannot stop the app.
- The error gate: the same error once per 10 seconds, with a `repeats` count; at most 10 lines per
  kind per 10 seconds; 200 distinct keys are tracked, and a quiet key is forgotten after six windows.
- The quiet level writes nothing while the app is idle: an idle control writes no line. Verbose
  (`diag.json`) logs every item, including the fast ones.
- The stall watchdog is native: a UI heartbeat every 50 ms, and a worker thread that stats the profile
  folder. It takes no lock the UI thread holds.
- The stack sampler (`thread_stack.rs`) is optional; without it `ui-stack` names the work items running.

## Privacy

The log holds only numbers, times, ids, kinds, short capped strings (paths, shell names, versions) and
the messages of the app's own errors. It never holds typed text, clipboard content, audio, screen text,
tab titles or a conversation. `mark` has no note, and the agent kinds drop free text. The web kinds keep a page's site
only: the host in lower case, never its address, port, login, path, query, fragment, title, cookies, form data or certificate
(`flare_surfaces::web::site_of`; an address with no host is its scheme, and anything else is `odd` or `unparsed`). The sites are
made where the browser's event arrives (`flare_web::WebNote`), so an address never reaches the log code. `PRIVACY.md`
says the same, and the scan in `crates/flare/tests/privacy.rs` and the text-key scan in
`crates/flare-diag/tests/neutral.rs` hold it: a line with a text key (`text`, `title`, `url`, `uri`, `query`, `path`, `cookie` and
the rest) fails the build.
