# Flare

A fast native Windows terminal for AI CLIs, written in Rust with Slint. A port of Prism Terminal.

Work in progress. Design: [docs/superpowers/specs/2026-10-17-flare-design.md](docs/superpowers/specs/2026-10-17-flare-design.md).
Plan: [docs/superpowers/plans/2026-10-17-flare-roadmap.md](docs/superpowers/plans/2026-10-17-flare-roadmap.md).
What "done" means: [docs/parity.md](docs/parity.md). Rules: [docs/regression-rules.md](docs/regression-rules.md).
Test mapping: [docs/test-port.md](docs/test-port.md).

## Build

```
cargo build
```

## Test

```
cargo nextest run --workspace
```

Also run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo deny check` before a PR.

## Dependencies

Each dependency is listed here with its reason, and has an entry in `THIRD-PARTY-NOTICES.md`
(generated with `cargo about generate about.hbs`) in the same PR.

- `slint`: the window, frameless chrome and widgets; its femtovg-wgpu and software renderers show the grid.
- `slint-build`: compiles the `.slint` UI files at build time.
- `wgpu`: the GPU painter; the same wgpu version Slint's `unstable-wgpu-30` uses, so one device is shared.
- `cosmic-text`: text shaping and layout for the glyph rasteriser (brings in `swash` for glyph bitmaps).
- `unicode-width`: the display width of each character, so wide cells take two columns.
- `png`: writes the `--screenshot` image.
- `pollster`: blocks on wgpu's async setup from synchronous startup code.
- `bytemuck`: casts glyph and vertex data to bytes for GPU upload.
- `log`: the logging facade used across the crates.
- `env_logger`: prints the log to the console when run from a terminal (`RUST_LOG`).
- `windows-sys`: the Win32 calls Flare makes directly (console, process memory, threads).

## Licence

Proprietary, see [LICENSE](LICENSE).
