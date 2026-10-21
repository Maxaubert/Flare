# Theme oracle

The oracle is Prism Terminal's own theme code, run under Node (v24 strips TypeScript types natively, so
nothing is installed). It answers the questions the Rust tests ask, so the expected values in
`crates/flare-theme/tests/fixtures/` are Prism Terminal's answers, not hand-typed numbers.

- `prep.mjs` copies the pure Prism Terminal theme modules flat into `src/` and rewrites their imports.
- `resolve.mjs`, `gen.mjs`, `gen_vectors.mjs` run them and write the JSON fixtures.
- `gen_rust.mjs` writes `crates/flare-theme/src/presets_data.rs` (generated, never edited by hand).
- `snapcheck.mjs` parses Prism Terminal's vitest `.snap` files and compares them with the oracle output.

`src/` and `out/` are scratch output (see `.gitignore`).

## Commands

Run from this folder. `<PT>` is the Prism Terminal export path (the only argument that varies).

```
node prep.mjs "<PT>"
node --no-warnings gen.mjs ..\..\crates\flare-theme\tests\fixtures
node --no-warnings gen_vectors.mjs ..\..\crates\flare-theme\tests\fixtures
node --no-warnings gen_rust.mjs ..\..\crates\flare-theme\tests\fixtures ..\..\crates\flare-theme\src\presets_data.rs
node --no-warnings snapcheck.mjs "<PT>"
```

The last command must print `every preset equal: true`, `opaque custom equal: true`,
`picked grounds: snap keys 80 oracle keys 80 equal: true`, `alpha sweep diffs: 0` and
`preset chrome diffs: 0 of 156`.

The fixtures are committed; regenerate only when the parity target moves.
