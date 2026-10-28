# /browser and the page the agent is told about: spec

Date: 2026-10-28. Owner: Max.

## What

1. **`/browser [url or search words]`**, a slash command in the bundled `flare-status` Claude Code plugin
   (`crates/flare-agent/claude-plugin/commands/browser.md`). Claude opens the page in Flare's browser panel beside the tab
   with `open_page`, looks once with `browser_look`, tells the user in one line that the page is open and that they can
   browse it, and then waits for the user. No more browsing unless asked.
   - No arguments: `https://www.google.com/`.
   - An address (with a scheme, or a host with an optional path such as `docs.rs/serde` or `localhost:3000`): passed as
     written; `open_page` (`browser.open`, `flare_surfaces::web::normalize_url`) adds `https://`, or `http://` for a local
     host. Only http, https and `about:blank` open.
   - Anything else is search words: `https://www.google.com/search?q=<words, URL-encoded>`.
   - `allowed-tools` lists `mcp__plugin_flare-status_flare__open_page` and `..._browser_look` only, so the command runs
     without a permission prompt and nothing else is pre-allowed.
2. **The agent hears which page is open.** `flare-mcp --hook-context` runs as a second `UserPromptSubmit` hook. It connects
   with `FlareChannel::from_env()` (1 s for the connect, 1 s for the answer), asks `panel.state` for its tab and, when an
   open web panel's address differs from the last one it reported for this tab and Claude session, prints
   `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"..."}}`. The context says the browser
   panel beside the tab shows a web page, gives the title and address between the page-content markers (cleaned, quoted:
   `browser::page_block`, the same rules as a look), and says `browser_look` and the other browser tools can see and use
   it, only when the request is about the page. When the page has gone since the last report it says so once. Otherwise it
   prints nothing.
   - State: `%TEMP%\flare-mcp\page-<16 hex>.json` holding `{"url": ...}`. The name is an FNV-1a hash of the tab token and
     the hook input's `session_id` (only a plain id of letters, digits, `-` and `_` is used), so the token is never
     written down and a new Claude session in the tab hears of the page afresh.
   - A failed call (no Flare, a refused token, a timeout) prints nothing, remembers nothing and exits 0. Without
     `FLARE_TAB_TOKEN` it returns before reading stdin.
3. **`hooks.json`** gets the hook as its own group under `UserPromptSubmit`, with `@@FLARE_MCP_EXE@@` as its program and
   `["--hook-context"]` as its args (timeout 5). `plugin::render_hooks_with` writes the absolute `flare-mcp.exe` there (the
   same path the manifest's `mcpServers` gets), or drops the hook (and a group or event left empty) when there is no
   program. `render_hooks` is `render_hooks_with(.., None)`. The 25 `hook.cmd` hooks are unchanged.
4. **`browser_state`**, a ninth browser tool: `panel.state` in words (no panel; a web page with its title and address as
   page data; a mini-app; another kind).

Plugin version 1.2.0.

## My calls (Max can change these)

- Only a web panel counts as a page for the hook. A mini-app is the agent's own page, so it is not announced.
- The address alone decides "changed": a title that arrives later for the same address is not a second report.
- The hook says "the user may have opened it or be browsing it", not "the user opened it": the agent's own `open_page`
  also changes the page, and the hook cannot tell the two apart.
- State files are a few bytes each in `%TEMP%` and are never cleaned up by Flare; Windows' own temp cleanup takes them.
