# Conversation transcripts: the format as Flare reads it

What `crates/flare-convo` expects in Claude Code's session file, so Prism Terminal can read the same file the same way.
Every claim is marked **known** (the code relies on it and a test pins it) or **guessed** (the reader tolerates it being
absent or different; nothing breaks, but the feature it feeds is empty). **Known** here means the author's understanding of
Claude Code's format, pinned by synthetic fixtures. The review of stage 6 ran a structure-only check against one local
session (no content copied); it found the subagent layout below. Fixtures are synthetic
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
- **Subagents (layout checked by structure only, 2026-10-21).** A subagent's lines are **not** in the session file. They
  are in `<home>\.claude\projects\<slug>\<session-id>\subagents\agent-<id>.jsonl`, one file per subagent, every line with
  `isSidechain: true`. The session file holds only `isSidechain: false` lines in that check. `path::subagent_dir` returns
  the folder. The check read one local session by structure only (no content was copied); the fixture
  `tests/fixtures/agent-abc.jsonl` is synthetic and follows that layout.

## In Flare (stage 6)

Flare tails a session only while a Changes panel is open on its tab. The Claude folder is `CLAUDE_CONFIG_DIR` when it is an absolute path, else `<profile>\.claude` (`path::claude_dir`, `transcript_path_in`). A restart of any file re-reads every file from 0 and starts a new model. The row model the panels draw is `view.rs`; the worker is `track.rs`.

## Reading it while it grows

- **Known (by design, tested).** `tail::Tail::poll` reads at most 4 MB from its byte offset. It returns only complete
  lines (a partial last line waits for its newline; a trailing `\r` is stripped; empty lines are dropped). `more` says
  bytes remain, so a caller can poll again at once.
- **Known.** A line longer than 8 MB (no newline inside that many bytes) is dropped while it is read, and counted in
  `skipped_long`. Nothing holds it in memory.
- **Restart.** The file is read from byte 0 again (and the batch says `restarted`, so the consumer clears its model) when:
  the file is shorter than the offset, or its **file index and volume serial** changed (replaced by a new file), or the
  last 64 bytes before the offset no longer read the same (rewritten in place and grown past the offset). Use the file
  index, not the creation time: NTFS tunnelling keeps a file's creation time when it is deleted and made again within a
  few seconds, so the creation time missed a real replacement in a test.
- **Missing file.** An empty batch, not an error. The transcript may not exist yet.
- **Worker.** `tail::subscribe` (one file) and `tail::subscribe_session` (the session file plus every `agent-*.jsonl` in
  the subagent folder, listed on each poll, so a later subagent is picked up) poll every 500 ms only while the
  `Subscription` lives. Each batch names its file in `source` (`""` for the session file). The channel holds two batches:
  a full channel parks the worker, so a slow consumer slows the reads and memory stays bounded. Dropping the subscription
  sets a stop flag and the worker exits within one 25 ms slice, even while parked (held by a test that reads `exit_flag`).
  A failed send (the receiver gone) also ends it.

## Lines

Each line is a JSON object. The reader looks at these keys only; everything else is ignored.

| Key | Use | Status |
|---|---|---|
| `type` | `"assistant"` and `"user"` are folded; every other value is ignored | known (`queue-operation`, `last-prompt`, `file-history-snapshot`, `summary`, `system` appear and are ignored) |
| `isSidechain` | `true` for a subagent's line; its edits are tagged. Lines of a subagent's own file are sidechain whatever this says | known (subagent files, see Where the file is); inline `true` lines in the session file are read the same way (a fixture only) |
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

The task tools (`TodoWrite`, `TaskCreate`, `TaskUpdate`) were folded into a task list for the Tasks panel until that
panel was removed (owner, 2026-10-26: sessions never used it). They are now tools the fold ignores
(`the_task_tools_fold_into_nothing`). The shapes Flare read (`TodoWrite`'s `todos: [{content, status, activeForm}]`
replacing the whole list, and the guessed `TaskCreate`/`TaskUpdate` fields) are in this file's history before that date.

Edits: `Edit` and `MultiEdit` and `Write` and `NotebookEdit` need a file path; an `Edit` with no `new_string`, or a
`MultiEdit` with no usable entries, is dropped. A result with `is_error: true` marks the edit `Failed`; a success marks it
`Applied`; no result leaves it `Pending`. A failed edit stays in the list, marked, because the agent did attempt it.

Caps (`fold::MAX_TEXT`, `MAX_EDITS`, `MAX_EDIT_BYTES`): each old, new or whole-file text keeps its first 64 KB (the edit
is then `truncated`, and its diff covers only the kept part). The model keeps the newest 2000 edits and at most 32 MB of
edit text; the oldest go first, and a result that arrives for a dropped edit is ignored. Named files are not
capped yet.

## Files the assistant named

- **Known rule.** An absolute Windows path in an assistant's text: a drive letter, `:`, a slash or backslash, then characters
  that cannot end a path in prose (no whitespace, quotes, `< > | * ? ( ) [ ] { }`). Trailing `. , ; : ! ?` are cut. The
  drive letter may not follow a letter, digit, `_`, `/`, a backslash or `:`, so `https://x.com/a.png` and
  `file:///C:/a.png` name no file (held by `a_url_or_a_file_url_in_the_assistants_words_names_no_file`).
- It is kept only when its last segment has an extension (a folder such as `C:\Users\x\project` is not a file).
- Forward slashes become backslashes. Duplicates are matched case-insensitively and kept once, first mention first.
- Picture extensions (`png jpg jpeg gif bmp webp ico`) set `image`.

## Subagents

A subagent's lines come from its own file (see Where the file is). `Convo::fold_from(source, line)` takes the file name
(`agent-<id>.jsonl`) with each line, and `fold_line` is the session file (`source ""`). Lines of a subagent file are
sidechain, so its edits and named files are tagged `sidechain`.

On a `restarted` batch from a subagent file, the consumer must clear that file's part of the model. The reader keeps no
per-file state beyond its tails.

## Diff

`diff::diff(old, new)` is a line LCS with no dependency. Hunks carry three lines of context and merge when they touch. The
output is capped at 2000 lines in all (`truncated` says so). A pair with more than 4,000,000 cells in its table is shown as
all removed then all added, uncapped in shape but still capped at 2000 lines. `EditOp::diff` picks the pair: a replacement
diffs old to new, a whole file diffs from nothing.

Line numbers: `Hunk::old_start` and `new_start` count from the first line of the text the edit carried (the snippet), not
from the top of the file. An edit in the middle of a file shows line 1 here. The panel adds the file's offset itself if it
reads the file. Fold and diff are not cheap: run them on the tail worker or a background thread, never the UI thread
(rule 34; the table can be 16 MB).

## What is not known

- Whether Claude Code writes a tool result's `is_error` as `true` only, or sometimes as a string. Flare takes only a boolean.
- Other transcript types (images in messages, compaction summaries) are ignored; nothing here depends on them.
- Codex transcripts: a later adapter, not read at all.

## For Prism Terminal

Read this file, the code in `crates/flare-convo/src` (`path.rs`, `tail.rs`, `fold.rs`, `diff.rs`) and the tests. The rules
that need care in Node: read by byte offset with a 4 MB cap, split on `\n` only (not on `\r`), carry the partial line,
restart on shrink or replacement (an `fs.stat` `ino` plus `dev`, never `birthtime`, is the replacement test on Windows),
and skip a line longer than 8 MB without holding it. Also: read the subagent folder for `agent-*.jsonl` each poll and tail
each file the same way, keyed by file name; keep the last 64 bytes before the offset and compare them each poll (a rewrite
in place that ends longer must restart); bound the queue to two batches so a slow consumer parks the reader.
