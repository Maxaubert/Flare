# File links by marker, and no more path guessing: spec and plan (2026-11-11)

Research: `C:\Users\Admin\Documents\Claude\research\flare\2026-11-11-terminal-links-research.md`.

## Why

Max, 2026-11-11: "the link system we have is poor. many links dont work. maybe we should scrap it entirely and rethink
it into a flare hook or something so the session can send a link with a shorthand, otherwise we just handle it like
claude normally since its so inconsistent."

Flare finds file paths by guessing from screen text. Claude Code breaks long lines into separate rows itself, indents
them, and prints paths relative to its own folder, so the guess fails often (the screenshot: a path over two rows was
no link). Claude Code prints no real links (OSC 8) in Flare, because it does not see a terminal it knows, and
`file://` links are plain text in Claude Code in any case.

## Owner decisions (2026-11-11)

1. Rethink the links: research first, then a spec. (owner)
2. Guessing stays only for web addresses (http and https). File-path guessing goes. (owner)
3. A session sends a file link as **shorthand markers**: the path between a start mark and an end mark. Flare reads
   what is between them exactly, also over line breaks. Rule 11 stays: Flare never sets `FORCE_HYPERLINK`. (owner)
4. A click on a file link does what it does today (text and images in the side panel, other files in their default
   app, unknown or runnable files shown in Explorer), and the panel goes to the line the link names. (owner)

## Goals

- A file path that a Claude session writes in markers is always a link, at any width, wrapped or not, indented or not.
- A click opens it as today, at its line in the panel.
- Every Claude session in Flare is told to write file paths this way, with no setup by Max.
- Less code: the path guesser, its caches and its retry table go.

## Non-goals

- Real OSC 8 file links from Claude Code (needs `FORCE_HYPERLINK`, which rule 11 forbids).
- Telling Codex or Antigravity sessions about markers (they have no Flare plugin). The reader works for any program
  that prints markers, so a user can still ask them to.
- Opening files in VS Code or another editor.

## Anti-goals

- A plain shell's output (`dir`, `git status`, a compiler error) must not turn into a wall of guessed links again.
- No link may run a program: the allowlist (rule 134) stays.

## Design

### The markers

- Start `⟦` (U+27E6) and end `⟧` (U+27E7): one cell wide, rare in real output, and readable in a terminal that knows
  nothing about them. *(My call: these two characters. Any rare pair works; these read as brackets.)*
- Content: an absolute path, optionally with `:line` or `:line:col` at the end. Windows (`C:\...`, `C:/...`) and
  macOS (`/...`, `~/...`). A relative path is resolved against the tab's folder (OSC 9;9 / OSC 7), and is a link
  only when that folder is known.
- Example: `⟦C:\Users\Admin\Documents\Claude\Github\web-application-project\app\src\app\auth\form-state.ts:17⟧`.

### Reading them (flare-term)

- A new `file_marks` reader finds each `⟦` and reads up to the next `⟧` in the same logical line or in the rows below
  it (at most 8 rows and 1,024 characters, so a lone `⟦` never eats the screen).
- Across a row break, the next row's leading spaces are dropped (Claude Code indents its continuation rows). A row
  marked wrapped by the terminal is joined with nothing dropped.
- The trailing `:line[:col]` is taken off and kept.
- The whole span, marks included, is one link, painted as a link on each row it covers. There is no existence check
  before it lights: the session wrote it on purpose. The click checks, and says "Flare found no such file" in the
  same status line a failed open uses today.
- A path that contained a space where the row broke: if the path with nothing between the rows does not exist and the
  one with one space does, the click opens that one. *(My call: the click tries both, in that order.)*

### Web addresses

- http and https guessing stays as it is, including its join over rows that Claude Code broke by hand (review #37).
  *(My call: it is guessing, but it is what Max kept, and it works.)*
- OSC 8 links stay as they are (http and https only).

### The click

- `LinkTarget::Path` now comes only from markers. It carries `line` and `col`.
- The click goes through the same `open_term_path` and allowlist as today. Text and images open in the side panel
  with "Open previews in Flare" on (the default). The text preview scrolls to `line` and marks that line. Other files
  open in their default app, which gets no line.
- The right-click menu keeps Open, Show in Explorer (Finder on macOS) and Show in side panel for a marker link.
- Hover thumbnails keep working for a marker link to an image.

### Telling the session

- The Flare Claude plugin gets a `SessionStart` hook (on start, resume, clear and compact). It runs
  `flare-mcp --hook-context --session-start` and returns `additionalContext` with a short instruction:

  > You are running inside Flare. When you mention a file in your reply, write its absolute path between ⟦ and ⟧, with
  > :line when a line matters, for example ⟦C:\repo\src\main.rs:42⟧. Flare turns it into a link. Do not put the marks
  > in code blocks, commands or files you write.

- The MCP server's `instructions` (initialize reply) gets the same sentence, so a session that skips the hook still
  sees it.
- `show_file` gets an optional `line`, the same scroll and mark as a click.

### What goes

- `paths.rs`, `path_links.rs` (the `PathLinks` cache, anchors, taught folders), the path-guessing branch and retry
  table of `link_scan.rs`, the path parts of `path_grammar` that only the guesser uses, and the `pathLinks` e2e.
- `cells.rs` row joining stays for web addresses and markers.
- `path_open.rs` (allowlist, `open_term_path`, `StatGate`) stays: dropped files, the panel and marker links use it.
  Whatever only the guesser used goes.
- Regression rules that hold the guesser (13, 237, and the guess parts of the macOS path rules) are marked replaced by
  the new rules, with the date. Parity lines for Prism Terminal's path links become a deliberate difference.

## Plan

One branch `feat/file-link-markers`, one PR. Implement, then an independent review, then fixes (the usual workflow).

1. **Reader.** `crates/flare-term/src/file_marks.rs`: find marker spans in a logical line plus the rows below, the
   joining and indentation rules, `:line[:col]`, the limits. Unit tests: one row; wrapped by the terminal; broken by
   hand with indentation; a space at the break; no end mark; two links on one row; marks inside a web address;
   Windows and POSIX paths; relative with and without a known folder.
2. **Scan and click.** `link_scan.rs` gives `LinkTarget::Path { text, line, col }` from markers only; the guess branch,
   its cache and retries go. `link_target.rs` and `term_ui.rs` pass the line on. The space-at-break fallback is in the
   click's worker.
3. **Line in the panel.** The text preview scrolls to and marks a line; `show_file` takes `line`. Tests in the
   preview and `flare-mcp`.
4. **Telling the session.** The `SessionStart` hook in `crates/flare-agent/claude-plugin/hooks/hooks.json`, the
   `--session-start` mode in `crates/flare-mcp/src/hook.rs`, the MCP `instructions`. Tests that the text is returned
   once per start and that the plugin template is valid.
5. **Remove.** The guesser's files and tests, the `pathLinks` e2e, settings or menu items that only the guesser used.
6. **E2E.** A hidden `markerLinks` scenario: a shell prints a marker path broken over two rows with indentation and one
   with a line; a click opens the panel at that line; a plain `dir` listing shows no file links.
7. **Docs.** `docs/features.md` (Links, Paths), new regression rules, the replaced ones marked, `parity.md`,
   `decisions.md`, `CLAUDE.md` pointer if needed.

Checks before the PR: `cargo fmt`, clippy, the workspace tests, the hidden `markerLinks` and `links`/`osc8Links` e2e.

## Left to the builder

The exact status sentence for a missing file, how the panel marks the line (the selection colour is a fine default),
and the names of the new rules and tests.
