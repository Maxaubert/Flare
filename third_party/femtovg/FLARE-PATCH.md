# femtovg 0.27.0, patched for Flare

This is femtovg 0.27.0 as published on crates.io (MIT OR Apache-2.0, see the two licence files), used through
`[patch.crates-io]` in the workspace `Cargo.toml`. Only `src/`, the licences, the README and a trimmed `Cargo.toml`
(no tests, examples or release profile) are kept.

## The one change (regression rule 1181)

`src/renderer/wgpu.rs`, the end of `WGPURenderer::render`: a cached GPU pipeline is dropped only after
`PIPELINE_IDLE_FLUSHES` (240) flushes in a row did not use it. Upstream dropped every pipeline the last flush did not
use. Slint's femtovg renderer flushes twice a frame when a rendering notifier is installed (Flare installs one): its
clear alone first, then the items. The clear-only flush dropped every other pipeline, so every frame built them all
again; on the Mac each is a naga to MSL translation and a Metal compile. Sampled in Max's Flare on 2026-11-12:
about 80 % of the main thread was drawing, 78 % of that in `create_render_pipeline`, 45 to 50 % CPU while agent marks
moved. A headless probe of three shapes: 4.9 ms a frame with two flushes, 0.6 ms with one.

Text changes: em-dashes in comments were replaced (the repository's no-em-dash rule).

Held by `crates/flare-render/tests/femtovg_pipelines.rs`. Drop this copy when Slint's femtovg stops evicting that way
(run that test against the upstream crate: it fails while the eviction is there).
