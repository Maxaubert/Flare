# Flare

A fast native Windows terminal for AI CLIs, written in Rust with Slint. A port of Prism Terminal.

Work in progress. Design: [docs/superpowers/specs/2026-10-17-flare-design.md](docs/superpowers/specs/2026-10-17-flare-design.md).
Plan: [docs/superpowers/plans/2026-10-17-flare-roadmap.md](docs/superpowers/plans/2026-10-17-flare-roadmap.md).
What "done" means: [docs/parity.md](docs/parity.md). Rules: [docs/regression-rules.md](docs/regression-rules.md).
Test mapping: [docs/test-port.md](docs/test-port.md).

## Features

- Terminal tabs with a shell per tab, restore, single instance and 39 themes.
- Updates (one click, per user, off until the repository is public), the Explorer verb "Open in Flare", and a local diagnostics log (`cargo diag`; nothing leaves the PC).
- Selection on the grid: character, word, line and block (Alt drag), drag scroll and Shift to extend.
- Copy (Ctrl+C with a selection, the Copied badge, OSC 52), paste (Ctrl+V, Ctrl+Shift+V, right-click Paste) and file drop (a quoted path, never Enter).
- Find in scrollback (Ctrl+F, Ctrl+Shift+F) with highlights, the count and wrap-around.
- Links: web links and OSC 8 links, and existing paths, painted and opened by a left click through an allowlist.
- A terminal right-click menu that fits what was clicked.

Details and the tests that hold each one: [docs/features.md](docs/features.md).

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

To read the diagnostics log after a run: `cargo diag` (the newest problems, with the crumbs before each), or `cargo diag -- --since 10m --all`. The flags and what each stall kind means are in [docs/diagnostics.md](docs/diagnostics.md).

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
- `windows-sys`: the Win32 calls Flare makes directly (console, process memory, threads, the startup error box; M3 adds the named pipe and its DACL for the single-instance hand-off, the security identity checks, the clipboard, the shell open verb, DPI and the monitors for the window position; M4 adds the DWM backdrop and border calls and the Windows build number; M6 adds `Wdk_System_Threading`, the native process snapshot with command lines that the agent poll reads, off the UI thread).
- `windows` 0.62.2: the taskbar overlay badge (`ITaskbarList3`); already in the build graph through Slint's accessibility stack; M8 reuses it for GSMTC.
- `alacritty_terminal`: terminal state, the escape-sequence parser, the grid, scrollback and reflow (owner decision, parity with Prism Terminal's xterm.js).
- `portable-pty`: the ConPTY child process and its pipes, one pty per shell tab.
- `regex`: the ported link and path finders (`find_links`, `find_paths`).
- `memchr`: the fast scan for escape bytes in the output stream.
- `base64`: decodes and encodes OSC 52 clipboard payloads.
- `url`: compares the host of an OSC 8 link, so only web links open.
- `rusty-leveldb` 4.0.1 (MIT): reads Prism Terminal's Chromium Local Storage once, from a copy made in a temp folder, for the first-run import (M7). Never writes into Prism Terminal's folders.
- `unicode-normalization` 0.1.25 (MIT OR Apache-2.0): the accent folding of Find a setting and the command help search, as JavaScript's `normalize('NFKD')` does (M7).
- `fancy-regex` (dev only, 0.19.2): the lookbehind the command help catalogue gate needs (M7).
- `uuid`: the per-shell random tab token (`FLARE_TAB_ID`).
- `serde`: derives the (de)serialisation of the state files (`tabs.json`, `window.json`, the settings file).
- `serde_json`: reads and writes those JSON state files, and encodes the single-instance hand-off message.
- `rfd`: the native folder picker for "Ask" new tabs (MIT; default features off, so on Windows it pulls only `log`, `raw-window-handle` and `windows-sys`).
- `raw-window-handle`: hands the window's handle to `rfd`, so the picker is parented to Flare's window (an unparented picker can open behind a fullscreen window).
- `sha2`: checks the bundled ConPTY files against their pinned hashes at start.
- `tempfile` (dev only): temporary folders for the tests; never shipped.
- Bundled ConPTY 1.25.260303002 (`conpty.dll`, `OpenConsole.exe`): MIT licence, (c) Microsoft Corporation, https://github.com/microsoft/terminal. Loaded by absolute path before the first pty opens.
- `velopack`: the installer, the one-click per-user update and the first-run and uninstall hooks (`flare-update`, `crates/flare`). Its update source is switched off until the repository is public: the `live-updates` cargo feature is off by default (M9 D1, O28).
- `ureq`: pulled in by `velopack` for its HTTP source; it is compiled in only with `live-updates`, so an ordinary build makes no request.
- `embed-resource` (build only): compiles the icon and the version resource into `Flare.exe` from `crates/flare/build.rs` (M9, Task 26).

M4 adds no new crate: the theme, fonts and material need only what M1 to M3 already ship. The
Win32 features `Win32_Graphics_Dwm`, `Win32_UI_Controls`, `Wdk_System_SystemServices` and
`Win32_System_SystemInformation` are added to the existing `windows-sys` entry (the DWM backdrop,
the window border and the Windows build number).

## Tools

Development tools in `tools/`, never shipped in the installer:

- `tools/oracle`: Node (v24, runs the TypeScript of Prism Terminal's theme modules through its
  built-in type stripping, no install step). It writes the fixtures in `crates/flare-theme/tests/fixtures`
  and the generated `presets_data.rs` and `fonts_data.rs`. Its README says how to regenerate them.
- `tools/pt-shot`: Playwright (from Prism's `node_modules`) drives the installed Prism Terminal to
  take the reference screenshots.
- `tools/theme-matrix`: PowerShell scripts that take the theme screenshots of every theme at 100,
  150 and 225 percent, compose the contact sheets and compare the two renderers.
- `tools/material-spike`: the Windows 11 material spike (acrylic, the z-order and the sampling); its
  results are in `docs/material.md`.
- `tools/oracle-help`: Node (v24, no install step). It generates Flare's command help catalogue and the
  search golden vectors from Prism Terminal's own help code; `node --test tools/oracle-help/` checks them.
- `tools/pt-metrics`: Node. Measures the installed Prism Terminal's Settings page (rects and computed
  style) at display scales 1 and 1.5 for the layout test (M7).
- `tools/pt-shot/pt-settings.mjs`: Playwright (from Prism's `node_modules`) takes Prism Terminal's
  Settings screenshots for the comparison (M7).
- `tools/settings-matrix`: PowerShell scripts that take Flare's Settings screenshots beside Prism
- `tools/make-icon.py`: builds the icon files (`build/icon.ico`, `build/icon-source.png`, `docs/assets/flare-icon.png`) from the owner's artwork, `assets/icon/flare-color.svg`. Regenerate with `python tools/make-icon.py` (needs ImageMagick, whose SVG reader is librsvg); `python -I tools/test_make_icon.py` checks it.
  Terminal's at every display scaling and compose them (M7).

## Release

A push to `main` with a new `version` in the root `Cargo.toml` publishes a GitHub release `v<version>` (the workflow is `.github/workflows/release.yml`). The steps, the gates, the installer and the signing state are in [docs/shipping.md](docs/shipping.md).

## Licence

Proprietary, see [LICENSE](LICENSE).
