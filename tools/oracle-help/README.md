# Flare help oracle

Dev-only. Nothing here ships. It makes Flare's command-help catalogue and its
search golden vectors from Prism Terminal's own code, so the Rust port is
checked against what Prism Terminal answers, not against what we remember.

## What it does

- `copy-sources.mjs` copies Prism Terminal's `core/shared/help/*.ts` (11 files)
  into `src/`. The only edits: a header line, and `'./x'` import specifiers
  get `.ts` so Node 24 can strip the types. `--check` exits 1 if a copy
  differs from the source. Source folder: `--from <dir>` or `PT_HELP_SRC`.
- `dump.mjs` writes the seven catalogue files as data
  (`crates/flare-help/data/<shell>.json`, one entry per line), and
  `tests/fixtures/meta.json` and `blankless.json` (what a row draws and copies).
  It refuses a total outside Prism Terminal's own 250 to 600 gate.
- `flare.mjs` holds what Flare adds over Prism Terminal's catalogue (the
  Antigravity `agy-start` and `agy-continue` entries, and the agents category
  named "Claude Code, Codex and Antigravity", 2026-10-26). The copies in
  `src/` stay Prism Terminal's; `dump.mjs` and `golden.mjs` read the
  catalogue through this file, so the golden vectors are still Prism
  Terminal's own `searchHelp`, run over what Flare ships.
- `golden.mjs` writes `tests/fixtures/golden_search.json` (every query in
  `queries.json` for three shells and for none, plus generated queries from one
  entry in three, with Prism Terminal's `searchHelp` ids and scores) and
  `tests/fixtures/words.json` (tokenise, meaningful words, stems, synonym
  expansion, one-slip matches).

Node 24.20.0 runs the TypeScript directly; there is no install step.

## Regenerate

    node copy-sources.mjs --from <PT/core/shared/help>
    node dump.mjs
    node golden.mjs
    node --test oracle.test.mjs

Set `PT_HELP_SRC` to also run `copy_check_passes` against the source.

## Facts

- 330 catalogue entries (powershell 96, cmd 48, bash 72, git 42, agents 40,
  packages 22, wsl 10): Prism Terminal v0.55.6's 328 and Flare's 2.
- `golden_search.json`: 2482 queries, about 1.2 MB, not sampled.
- `queries.json`: 379 strings, the literals Prism Terminal's tests pass to the
  search (`search.test.ts`, the `QUESTIONS` table of `catalogue.test.ts`) and
  the edge cases the plan names. Two stray literals were dropped because they
  are not queries.
- The synonym table is read from the copy as text (27 groups), not evaluated.

## Rules

- A golden vector is never edited by hand. If the Rust search disagrees with
  it, the Rust search is wrong.
- No em-dashes in any output or script (`oracle.test.mjs` checks the outputs).
