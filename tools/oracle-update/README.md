# Oracle: Prism Terminal's answers as Flare's golden fixtures

Dev tool, not shipped. It runs Prism Terminal's own TypeScript (v0.55.6, read-only) on a seeded corpus
and writes the fixtures the Flare tests compare against.

    node tools/oracle-update/gen.mjs

Plain Node 24 (type stripping is built in), no dependency. `PT_REF` overrides the Prism Terminal folder.
The run is idempotent: seeds and orders are fixed, so a second run leaves `git status` clean.

Never edit a fixture by hand. If a fixture is wrong, fix the generator. If Flare disagrees with a fixture,
Flare is wrong, unless the case is marked (see below).

## What each file feeds

| Fixture | Written from | Read by |
|---|---|---|
| `crates/flare-update/tests/fixtures/version.json` | `newerVersion` (cut from `src/main/update.ts`, types stripped) | Task 4 |
| `.../notes.json` | `parseReleaseNotes`: 39 named PT test cases (`name` = the PT title), 17 edge bodies, seeded bodies, 450 in all | Task 5 |
| `.../groups.json` | `groupReleaseNotes` on the first 150 note bodies, and `classifyEntry` on 40 titles | Task 6 |
| `.../flow.json` | `updateFlow` on 200 seeded action sequences, the state after every step | Task 7 |
| `.../preview.json` | `previewVersion`, `wantsPreview`, `SAMPLE_NOTES` and its parse, `runPreviewInstall` traces | Task 4 |
| `crates/flare-diag/tests/fixtures/summary.json` | `capString`, `summariseValue`, `summariseArgs`, `errorFields`, `cleanFields` | Task 17 |
| `.../gate.json` | `createDiagGate(10000, 10)` on 60 seeded sequences | Task 17 |
| `.../reader/{quiet,stall,agent}/diag.jsonl` | hand-written log fixtures in the writer's line shape | Task 23 |
| `.../reader_goldens.json` | stdout of `tools/diag.mjs` (TZ=UTC) on those folders, 7 cases | Task 23 |
| `crates/flare-win/tests/fixtures/names.json` | Prism Terminal's identifiers, regex-read by `names.mjs` (left column of Appendix B) | Task 35 |

## Encodings (JSON cannot say these)

- A note body is a string, or `{"repeat": [s, n]}` (`s` repeated `n` times), or `{"concat": [...]}` (the
  pieces joined), or `{"undefined": true}`, or a plain JSON value (null, a number, an object).
- Summary inputs and outputs: `{"undefined": true}`, `{"number": "NaN"|"Infinity"}`, `{"bigint": "123"}`,
  `{"fn": true}`, `{"symbol": "s"}`, `{"$bytes": n}`, `{"$arraybuffer": n}`, and
  `{"$error": {"message", "stack"?}}` (no `stack` key means the error has none).
- Flow actions: `progress` carries `"NaN"` or `"Infinity"` as strings where the number is not finite.
- `reader` goldens: `args` are the exact arguments, `--dir reader/<name>` first. The folders are
  `reader/<name>/diag.jsonl` because `diag.mjs` reads a folder, not a file.

## Flagged cases (not a bug in the oracle)

- `version.json`, three pairs with `"flare": "zero"`: Prism Terminal's `Number()` reads `1e2`, `0x10` and
  ` 5 ` as numbers. Flare reads a part that is not all digits as 0. `flare_newer` is Flare's answer.
- `groups.json`, entries with `"flare": "skip"`: the `terminal core: prism-term-core` line is Prism Terminal's
  own pin bump. Flare has no such line, so Flare's classifier does not file it under the hood.
- `summary.json`, records with `"lone_surrogate": true`: Prism Terminal cuts a string in the middle of a
  surrogate pair and keeps the lone half. JSON for Rust cannot hold that, so `output` is `null` and
  `flare_output` is the same text with the lone half removed (Flare's cut drops the pair whole).

## Corpus

`corpus.mjs` holds the seeded fragments (every sentence shape the Prism Terminal tests use, plus the hostile
ones: tags hidden in tags, bidi overrides, control characters, CRLF, surrogates at the caps), the 17 edge bodies
and the 40 titles. `names.mjs` reads Prism Terminal's identifiers from its source and fails loudly when one
is missing.
