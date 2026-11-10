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
- Panels: a click on a path to a picture or a text file (or "Show in side panel" in its right-click menu) shows it beside the terminal, per tab, behind a draggable splitter (fit, click for 1:1, scroll, a checkerboard under transparency; long text lines wrap, with a button in the header to turn that off). Ctrl+Shift+P shows or hides it and Ctrl+Shift+O moves the keys; Settings > Terminal > Links turns it off.
- Agent tools: Claude in a Flare tab can show an image, a file or some text in that tab's panel (`show_image`, `show_file`, `show_text`, `open_page`, `close_panel`) through a local, per-user, per-tab control channel and the bundled `flare-mcp.exe`; no setup, nothing leaves the PC. The contract is `docs/surfaces/protocol.md`.
- Web panel: a real browser beside the terminal (Microsoft Edge WebView2, already on Windows 11), per tab, with an address bar, back, forward and reload, the page title in the header, local video at full speed and the same splitter. It hides behind Flare's own menus and dialogs (a still of the page stays in its place), the page gets its keys with Ctrl+Shift+O and gives them back with a second Escape, a page cannot open a window or ask for a permission, a download waits for your yes before anything is saved, and only http and https addresses open. Claude opens a page with `open_page` and then uses it as you would: `browser_look` gives it a screenshot and a numbered list of the page's controls and text, and `browser_click`, `browser_type`, `browser_press`, `browser_scroll`, `browser_wait` and `browser_navigate` act on those numbers, in Flare's own browser instead of your Chrome. A ribbon in the panel's head says "Claude is using this page" while it works, and your own click or key in the page stops it at once (a quick tap too). It also works in a tab that is behind, and hidden characters in a page's text (Unicode tag characters and the like) never reach the agent. Running scripts in the page (`browser_eval`) stays off until you switch on "Let agents run scripts in pages" in Settings. Type `/browser` (or `/browser docs.rs`, `/browser rust lifetimes`) in Claude to open a page or a Google search there and browse it yourself; Claude is told which page the panel shows when you next write to it, and `browser_state` lets it check. Without the runtime the panel shows the download link and nothing else changes.
- Changes panel: on a tab that runs Claude, the right-click menu (or Claude itself, with `panel.open`) shows the files it edited, each edit as a diff, beside the terminal. Flare reads the session's transcript on a worker only while that panel is open, nothing is read otherwise, and nothing leaves your PC. `convo.state` on the control channel gives Claude its own tab's summary.
- Live preview and notifications: when a tab starts a local site (`http://localhost:5173/`), a Preview chip opens it in the web panel; a question from an agent you are not looking at shows a Windows notification with the tab's name and the question. Each has a switch in Settings (Offer previews of local servers, Notify me when an agent asks); nothing leaves your PC.
- Mini-apps and diagrams: Claude can show a small page it writes (a chart, a calculator, an explainer, a 3D viewer) with `show_html`, or draw a Mermaid diagram with `show_diagram`, in the panel beside the terminal. A page runs in a sandbox of its own: a browser profile of its own that keeps nothing, a host name of its own, a policy that blocks everything that would leave the page, no navigation and no new windows. It has no network unless Claude asks and you switch on "Let mini apps use the network" in Settings (off by default). Claude can look at its own page with `browser_look` and use it. A diagram that does not parse comes back to Claude as Mermaid's own error. Closing the panel ends the page and removes its files.
- Plugins: anyone can add to Flare with a folder and a `plugin.json`: badges and progress on tabs, sidebar sections, buttons on tabs and in menus, a bar under the terminal, a panel, a hidden background page, tools Claude can call and, if you allow it, a program of its own. Install one from a zip file or an `https` address in Settings > Plugins, where it arrives off and shows what it asks for in plain words (typing into your terminals and running a program are marked full control); update and remove it there. Pages run in a sealed browser profile per plugin with no network unless it asks for `network`. Samples are in `sample-plugins/` (to-dos, a browser badge, what next, a player bar, session buttons, weighted progress); the contract is [docs/surfaces/plugins.md](docs/surfaces/plugins.md). Protocol 1 mods (`mod.json`) keep working ([docs/surfaces/mods.md](docs/surfaces/mods.md)). Claude itself can put a badge or a progress on its own tab with no plugin (`set_tab_badge`, `set_tab_progress`).

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

The end-to-end suite drives the built release exe (offscreen, one profile per scenario; see [docs/e2e.md](docs/e2e.md)):

```
cargo build --release -p flare --target-dir <TARGET>
cargo run -p flare-e2e --release --target-dir <TARGET> -- --exe <TARGET>\release\flare.exe [names]
cargo run -p flare-e2e --release --target-dir <TARGET> -- perf all --exe <TARGET>\release\flare.exe
```
The performance bar, its method and the M10 verdict are in [docs/performance.md](docs/performance.md); the scaling images are in [docs/screenshots/m10/dpi/README.md](docs/screenshots/m10/dpi/README.md).

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
- `image` 0.25.10 (MIT OR Apache-2.0; default features off, only png, jpeg, gif, bmp, webp and ico): decodes the pictures a panel shows and, since Surfaces stage 3, the still of a web page (a PNG) that stands in for the page while a menu is open, on a worker thread (Surfaces stage 1). `png` and `jpeg` were already in the build through Slint; the others add `gif` and its small helpers.
- `webview2-com` 0.39.1 (MIT; with its `-sys` and `-macros` crates; released 2026-03-11): the COM bindings to WebView2, the browser engine of the web panel (Surfaces stage 3). The `-sys` crate links Microsoft's static loader (`WebView2LoaderStatic.lib`, BSD 3-Clause, listed under "Bundled binaries" in the notices), so no `WebView2Loader.dll` ships. Flare never installs or updates the runtime; Windows 11 has it, and without it the panel shows a download link. It uses the `windows` crate already in the build (`=0.62.2`).
- `url` 2.5.8 is now also a direct dependency of `flare-surfaces`: the web panel's address checks (scheme, host, no login in the address).
- Mermaid 11.17.2 (MIT, released 2026-08-25; `assets/mermaid/mermaid.min.js`, the official npm build kept unchanged, with its licence and `assets/mermaid/README.md`): draws the diagrams `show_diagram` asks for (Surfaces stage 5). It is a vendored JavaScript file, not a crate: compiled into `flare.exe` with `include_bytes!`, never fetched, run only inside a mini-app's sandbox with no network. It bundles Mermaid's own dependencies (d3, dagre, cytoscape, DOMPurify and others); the hand entry in `THIRD-PARTY-NOTICES.md` (from `about.hbs`) names them. `sha2` is now also a dev dependency of `flare`, only for the test that holds the file's hash.
- three.js 0.170.0 (MIT, released 2024-10-31; `assets/three/viewer.js`, built once with esbuild 0.24.0 from `tools/model-viewer/`, with its licence and `assets/three/README.md`): draws the models `show_model` asks for (issue #36). Compiled into `flare.exe` with `include_bytes!`, never fetched, run only inside a mini-app's sandbox with no network.
- `pollster`: blocks on wgpu's async setup from synchronous startup code.
- `bytemuck`: casts glyph and vertex data to bytes for GPU upload.
- `log`: the logging facade used across the crates.
- `env_logger`: prints the log to the console when run from a terminal (`RUST_LOG`).
- `windows-sys`: the Win32 calls Flare makes directly (console, process memory, threads, the startup error box; M3 adds the named pipe and its DACL for the single-instance hand-off, the security identity checks, the clipboard, the shell open verb, DPI and the monitors for the window position; M4 adds the DWM backdrop and border calls and the Windows build number; M6 adds `Wdk_System_Threading`, the native process snapshot with command lines that the agent poll reads, off the UI thread).
- `windows` 0.62.2: the taskbar overlay badge (`ITaskbarList3`); already in the build graph through Slint's accessibility stack; M8 reuses it for GSMTC. Surfaces stage 7 adds its `UI_Notifications` and `Data_Xml_Dom` features (no new crate, nothing new in `Cargo.lock`) for the Windows notification a question from an agent shows; the check whether a local dev server listens uses `windows-sys`' `Win32_NetworkManagement_IpHelper`, already enabled.
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
- `ureq` 3.4.2: `velopack`'s HTTP source (compiled in only with `live-updates`), and since M8 also a direct dependency of `flare-dictation`, which downloads a speech model or engine only after the user asks for it. Nothing else makes a request.
- `zip` 9.0.0 (2026-10-09, `deflate` only): the GPU pack's unzip (M8 D5). The oldest release patched for RUSTSEC-2026-0367/0368/0369; the 14-day age rule gave way to the advisory rule because no fixed release is that old yet. `velopack` still pulls its own `zip` 8.6, ignored in `deny.toml` for the updater only.
- `libloading` 0.8.9: loads `ggml.dll` and `parakeet.dll` inside `flare-parakeet-host.exe` (M8 D3), the resident Parakeet host. It runs out of process, so a CUDA crash cannot take the app down.
- whisper.cpp engine b5130 (MIT, `LICENSE-whisper.cpp.txt`, ggml-org): `whisper-server.exe`, `parakeet-cli.exe` and their DLLs, plus Microsoft's Visual C++ runtime DLLs (copied only when Authenticode says Microsoft). `tools/fetch-whisper.ps1` fetches the pinned zip at build time into `vendor/whisper` (gitignored) after a size and SHA-256 check; `tools/release/pack.ps1` ships it as `whisper\` beside `Flare.exe`. Nothing in the repository is compiled from it.
- `flare-convo` (Surfaces stage 6) adds no crate: it uses `regex`, `serde_json`, `log`, `flare-agent` (for the transcript slug) and `windows-sys`, whose `GetFileInformationByHandle` (`Win32_Storage_FileSystem`, already enabled) gives a transcript's file index.
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
