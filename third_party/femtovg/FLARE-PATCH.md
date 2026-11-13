# femtovg 0.27.0, patched for Flare

This is femtovg 0.27.0 as published on crates.io (MIT OR Apache-2.0, see the two licence files), used through
`[patch.crates-io]` in the workspace `Cargo.toml`. Only `src/`, the licences, the README and a trimmed `Cargo.toml`
(no tests, examples or release profile) are kept.

## Change 1: pipelines stay between flushes (regression rule 1181)

`src/renderer/wgpu.rs`, the end of `WGPURenderer::render`: a cached GPU pipeline is dropped only after
`PIPELINE_IDLE_FLUSHES` (240) flushes in a row did not use it. Upstream dropped every pipeline the last flush did not
use. Slint's femtovg renderer flushes twice a frame when a rendering notifier is installed (Flare installs one): its
clear alone first, then the items. The clear-only flush dropped every other pipeline, so every frame built them all
again; on the Mac each is a naga to MSL translation and a Metal compile. Sampled in Max's Flare on 2026-11-12:
about 80 % of the main thread was drawing, 78 % of that in `create_render_pipeline`, 45 to 50 % CPU while agent marks
moved. A headless probe of three shapes: 4.9 ms a frame with two flushes, 0.6 ms with one.

## Change 2: the concave cover quad takes the fringe on all four corners (regression rule 1203)

`src/lib.rs`, the cover quad at the end of `fill_path_internal`: its fourth corner (left, top) is
`bounds.miny - fringe_width`, as the other three take the fringe. Upstream left it at `bounds.miny`, so the quad's top
edge sloped and left a sliver of stencil counts above every concave fill; the stencil is kept between flushes, so later
fills there lost their anti-aliased edge (Clawd's hop drew other pixels for the same box). Held by
`crates/flare-render/tests/femtovg_clawd_hop.rs` (fails against the upstream crate).

Text changes: em-dashes in comments were replaced (the repository's no-em-dash rule).

Held by `crates/flare-render/tests/femtovg_pipelines.rs`. Drop this copy when Slint's femtovg stops evicting that way and fixes the cover quad (run both tests against the
upstream crate: they fail while either is there).
