# Conversation transcripts: the format as Flare reads it

What `crates/flare-convo` expects in Claude Code's session file, so Prism Terminal can read the same file the same way.
Every claim is marked **known** (the code relies on it and a test pins it) or **guessed** (the reader tolerates it being
absent or different; nothing breaks, but the feature it feeds is empty). **Known** here means the author's understanding of
Claude Code's format, pinned by synthetic fixtures; no live transcript was opened in stage 6, so a structure-only check against one
is the first task of the next stage. Fixtures are synthetic
(`crates/flare-convo/tests/fixtures/session.jsonl`); no real transcript content was copied into the repository.

## Where the file is

- **Known.** `<home>\.claude\projects\<slug>\<session-id>.jsonl`, one JSON object per line, appended as the session runs.
  `<home>` is the user's profile folder (`%USERPROFILE%`, what `flare_agent::resume::claude_home` returns).
- **Known.** The slug is the tab's folder with every UTF-16 unit that is not `[A-Za-z0-9]` replaced by `-`
  (`C:\Users\Admin\my project` becomes `C--Users-Admin-my-project`). Flare calls `flare_agent::resume::encode_cwd`; do not copy it.
- **Known.** The session id is a UUID. `path::valid_session_id` refuses anything else (a separator, a dot, a space, over 64
  bytes), so an id from a shell can never name a file outside the projects folder.
- **Fallback.** `find_transcript` searches every project folder for `<session-id>.jsonl`, for a tab whose folder moved.
  The first match wins; folders are not sorted.

## Reading it while it grows

- **Known (by design, tested).** `tail::Tail::poll` reads at most 4 MB from its byte offset. It returns only complete
  lines (a partial last line waits for its newline; a trailing `\r` is stripped; empty lines are dropped). `more` says
  bytes remain, so a caller can poll again at once.
- **Known.** A line longer than 8 MB (no newline inside that many bytes) is dropped while it is read, and counted in
  `skipped_long`. Nothing holds it in memory.
- **Restart.** The file is read from byte 0 again (and the batch says `restarted`, so the consumer clears its model) when:
  the file is shorter than the offset (truncated or rewritten in place), or its **file index and volume serial** changed
  (replaced by a new file). Use the file index, not the creation time: NTFS tunnelling keeps a file's creation time when
  it is deleted and made again within a few seconds, so the creation time missed a real replacement in a test.
- **Missing file.** An empty batch, not an error. The transcript may not exist yet.
- **Worker.** `tail::subscribe` polls every 500 ms only while the `Subscription` lives; dropping it sets a stop flag and the
  worker exits within one 25 ms slice. A failed send (the receiver gone) also ends it.

## Lines

Each line is a JSON object. The reader looks at these keys only; everything else is ignored.

| Key | Use | Status |
|---|---|---|
| `type` | `"assistant"` and `"user"` are folded; every other value is ignored | known (`queue-operation`, `last-prompt`, `file-history-snapshot`, `summary`, `system` appear and are ignored) |
| `isSidechain` | `true` for a subagent's line; its edits and tasks are tagged or kept apart | known for Claude Code's subagent lines |
| `timestamp` | ISO 8601 string, kept as written, on each edit | known |
| `message.content` | an array of blocks (or, rarely, a plain string: an assistant string is scanned for file paths, a user string is ignored) | known |

Lines that are not a JSON object count as `bad_lines` and are skipped. A JSON object of an unknown `type` is not bad; it is
just not used.

## Content blocks

- **`type: "tool_use"`** in an assistant message: `id` (the call's id), `name`, `input` (an object).
- **`type: "tool_result"`** in a user message: `tool_use_id`, `is_error` (boolean, default false), and `content`, which is
  a string or an array of `{type:"text", text}` blocks. **Known** for both shapes (a test uses each).
- **`type: "text"`** in an assistant message: `text`. Scanned for named files only.

## The tools Flare folds

| Tool | Input Flare reads | Status |
|---|---|---|
| `Edit` | `file_path`, `old_string`, `new_string` | known (Claude Code's Edit tool) |
| `MultiEdit` | `file_path`, `edits: [{old_string, new_string}]` (one change per entry) | known |
| `Write` | `file_path`, `content` (a whole file; the text it replaced is not in the transcript, so its diff is all added) | known |
| `NotebookEdit` | `notebook_path`, `new_source` (one cell's new source, as a whole); `cell_id`, `edit_mode`, `cell_type` ignored | known for `new_source`; the delete mode is not distinguished |
| `TodoWrite` | `todos: [{content, status, activeForm}]`; each call **replaces** the whole list | known |
| `TaskCreate` | `subject` (or `content`, or `description`) for the title, `activeForm` | guessed field names (the tool family is newer; a missing title makes the create a no-op) |
| `TaskUpdate` | `taskId` (a string or a number), and optionally `status`, `subject`/`content`, `activeForm`; `status: "deleted"` removes the task | guessed |
| `TaskCreate` result | Flare looks for a JSON `id` or `task.id`, else the first `#<digits>` in the text, and keeps it as the task's id for `TaskUpdate` | guessed (the result format is not pinned by any sample Flare has seen) |

Statuses: `pending`, `in_progress`, `completed` are known. Any other word is `TaskStatus::Other` and is kept as it is not lost.

Edits: `Edit` and `MultiEdit` and `Write` and `NotebookEdit` need a file path; an `Edit` with no `new_string`, or a
`MultiEdit` with no usable entries, is dropped. A result with `is_error: true` marks the edit `Failed`; a success marks it
`Applied`; no result leaves it `Pending`. A failed edit stays in the list, marked, because the agent did attempt it.

## Files the assistant named

- **Known rule.** An absolute Windows path in an assistant's text: a drive letter, `:`, a slash or backslash, then characters
  that cannot end a path in prose (no whitespace, quotes, `< > | * ? ( ) [ ] { }`). Trailing `. , ; : ! ?` are cut.
- It is kept only when its last segment has an extension (a folder such as `C:\Users\x\project` is not a file).
- Forward slashes become backslashes. Duplicates are matched case-insensitively and kept once, first mention first.
- Picture extensions (`png jpg jpeg gif bmp webp ico`) set `image`.

## Subagents

Lines with `isSidechain: true` are folded like any other, with `sidechain` set on the edit or file. A subagent's
`TodoWrite` never replaces the main list: it goes to `side_tasks`. Task-tool creates and updates from a subagent go to
`side_tasks` too.

## Diff

`diff::diff(old, new)` is a line LCS with no dependency. Hunks carry three lines of context and merge when they touch. The
output is capped at 2000 lines in all (`truncated` says so). A pair with more than 4,000,000 cells in its table is shown as
all removed then all added, uncapped in shape but still capped at 2000 lines. `EditOp::diff` picks the pair: a replacement
diffs old to new, a whole file diffs from nothing.

## What is not known

- The exact shape of `TaskCreate` and `TaskUpdate` inputs and results (guessed above).
- Whether Claude Code writes a tool result's `is_error` as `true` only, or sometimes as a string. Flare takes only a boolean.
- Other transcript types (images in messages, compaction summaries) are ignored; nothing here depends on them.
- Codex transcripts: a later adapter, not read at all.

## For Prism Terminal

Read this file, the code in `crates/flare-convo/src` (`path.rs`, `tail.rs`, `fold.rs`, `diff.rs`) and the tests. The rules
that need care in Node: read by byte offset with a 4 MB cap, split on `\n` only (not on `\r`), carry the partial line,
restart on shrink or replacement (an `fs.stat` `ino` plus `dev`, never `birthtime`, is the replacement test on Windows),
and skip a line longer than 8 MB without holding it.
