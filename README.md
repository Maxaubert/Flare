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

None yet beyond the workspace itself. Each dependency added later is listed here with its reason,
and gets an entry in `THIRD-PARTY-NOTICES.md` (generated with `cargo about generate about.hbs`) in
the same PR.

## Licence

Proprietary, see [LICENSE](LICENSE).
