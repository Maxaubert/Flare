# The memory diet (M10 Task 66)

Taken 2026-10-19 on this PC (release build of `m10-parity`, `cmd`, one tab at the prompt, 10 s after
the first prompt, median of 3, the CIM query `tools/pt-perf` uses). B2 counts the private working
set of the app tree minus the shells. Prism Terminal 0.58.0: 126.5 MB, so the bar is 42.2 MB for the
GPU renderer and 126.5 MB for the software renderer.

## Step 1: the breakdown

`memory.breakdown` (probe) and a `parts:` line in the F12 overlay (`crates/flare/src/memory.rs`).
`perf memory` writes the table into the report detail. It reads the private working set the way Task
Manager does (`QueryWorkingSet`, pages without the Shared bit) and splits it by kind of memory.

At idle with one tab, before the diet (GPU renderer, window 2655 x 1674 device px, 225%):

| Part | GPU | Software | In the private working set? |
|---|---|---|---|
| Glyph atlas, CPU side | 0.01 MB | 0 | yes (heap) |
| Glyph atlas texture | 16.0 MB | 0 | no: the driver commits a texture's pages when they are first touched; an atlas shrunk 2048 to 512 left the working set where it was (78.1 vs 78.7 MB) |
| Grid texture | 17.0 MB (exactly the grid area) | 0 | yes: every pixel is drawn each frame, so the pages are resident (a 16.9 MB private region) |
| CPU frame buffer | 3.7 MB (the first CPU frame, kept for ever) | 18.0 MB | yes (heap) |
| Glyph bitmap cache | 0.06 MB | 0.06 MB | yes (heap) |
| Scrollback (one tab, empty) | 0 | 0 | yes (heap); 10,000 lines of 80 columns would be `10000 * 80 * size_of::<Cell>()` |
| Font database (68 faces, file sizes) | 209.7 MB | 209.7 MB | no: memory-mapped files |
| Help index | 1.07 MB | 1.07 MB | yes (heap) |
| Settings catalogue | 0.005 MB | 0.005 MB | no: static data |
| Process heap allocated | 40.0 MB (after the OpenGL fix: 38) | 52.4 MB | yes |
| Private working set | 135 MB | 72.5 MB (in-app), 69.1 MB (CIM) | |
| Unaccounted | Slint, the graphics driver and the rest of the heap; the largest private regions are 17.1, 14.4, 11.0, 7.7, 4.0, 4.0, 3.9 MB | Slint's software frame (window-sized) and the heap | |

## Step 2: the candidates, in the plan's order

Rule: kept only if it removes at least 3% of the private working set. B2 median in MB, release,
this PC. "Before" is the number measured in the same session just before the change.

| Candidate | Result | Kept? |
|---|---|---|
| Not in the plan: the Task 64 prewarm started every wgpu backend, OpenGL and Vulkan included, so the driver stacks of all GPUs were loaded (`nvoglv64`, `amdvlk64`, `nvrtum64`). First OpenGL out, then DX12 alone (the glass swapchain is DX12-only anyway). | GPU 135 -> 115.7 (OpenGL out) -> 82.4 (DX12 alone). B1 GPU 1636 -> 986 ms. Software unchanged (no prewarm). Test `the_prewarm_instance_starts_dx12_alone_unless_a_backend_is_named`. | yes, 39% |
| The help index is built on first use and dropped when the panel has been closed for 60 s | GPU 78.5 -> 76.5 (2.6%), software 69.1 -> 66.5 (3.8%). The warm-up is gone from start-up; opening the panel warms the index on a worker. Tests `the_index_is_built_on_first_use`, `the_index_is_released_after_the_panel_has_been_closed_a_minute` (fake clock), `an_open_panel_keeps_its_index`, `the_help_index_is_not_built_at_start`. The catalogue itself (about 0.9 MB) stays once built: it is a `&'static` list the panel reads. | yes (3% on the software row, 2.6% on GPU) |
| Scrollback row cost | Parity of lines kept (the reference terminal's xterm has `scrollback: 10000`); alacritty's `Cell` cannot be made smaller. `scrollback_matches_the_reference_terminals_line_count`. | n/a, left alone as the plan says |
| Grid texture sized to the window, not the 8192 limit; CPU frame buffer only on the software painter | The grid texture was already the size of the grid area (`the_grid_texture_matches_the_window`). The CPU buffer of the first frames stayed for ever on the GPU renderer; it is now dropped at the first GPU frame (`no_cpu_buffer_after_the_switch_on_gpu`): 82.9 -> 80.9 MB (2.3%, the process heap fell 3.4 MB). | kept below the 3% line: the buffer is the size of the first frame's window (3.5 MB in this harness, 33 MB for a saved 4K window) |
| Glyph atlas starts at 1024 and grows by doubling | Not done. The atlas texture is not resident: a 512 x 512 atlas left the working set unchanged (78.1 vs 78.7 MB). | no |
| wgpu `MemoryHints::MemoryUsage`, smaller `max_buffer_size` | The prewarmed device already asks for `MemoryUsage` (it is Slint's own default too; `the_device_asks_for_the_memory_usage_hint`). `max_buffer_size` is a limit, not an allocation. | already met, no change |
| cosmic-text shape and swash caches capped | The glyph cache is 0.06 MB with a full prompt screen of glyphs; nothing to cap. | no |
| Hidden tabs release their CPU frame buffers | One `GridHost` serves every tab (rule 25: only the visible grid is drawn), so a hidden tab never held a frame buffer. | already met |
| `mimalloc` as the global allocator (trial) | Worse: GPU 76.5 -> 83 to 84 MB, software 66.5 -> 109 MB (CIM). The reserved segments cost more than the Windows heap's fragmentation. Reverted; no dependency was added. | no |

## Where B2 stands

| | Before the diet | After | Bar | Verdict |
|---|---|---|---|---|
| GPU | 135 MB (115 MB before Task 64) | 75.8 MB | 42.2 MB (126.5 / 3) | MISS |
| Software | 69.1 MB | 66.6 MB | 126.5 MB | pass |

The GPU row cannot reach 42 MB with what the app owns. Of the 75.8 MB, 71.7 MB are plain
allocations: the process heap holds 18 MB allocated (24 MB committed), the grid texture is a 17 MB
region (resident because every pixel is drawn every frame), and the rest is made of large regions
(14.4, 11.0, 7.7 MB ...) that the breakdown cannot name: they are either Windows heap segments or
the window-sized buffers of Slint's renderer, the swapchain and the graphics driver (`nvgpucomp64`
and `nvwgf2umx` hold another 2 MB of private image data). What the numbers do show: even the software renderer, which has no GPU memory at all, sits at 66.6 MB (its CPU frame buffer alone is 18 MB), so 42 MB is out of reach for the GPU path unless
the grid texture and the window-sized buffers go, which is not a diet. This is a D8 / Q1 input for the verdict, not something
Task 66 can move further. The owner's choices: (a) accept the bar as missed with this floor, (b)
change the bar to "at most half of Prism Terminal's" (63 MB, which the GPU path still misses by about
13 MB at this window size), or (c) draw the grid straight into Slint's frame instead of an imported
texture (removes the 17 MB grid texture; a rendering-architecture change that is the owner's call).
## How to read it again

    flare-e2e perf memory --exe <target>\release\flare.exe --renderer both --baseline docs/perf/pt-baseline-2026-10-19.json --out <json>

The `detail.breakdown` object of each measure is the table above (bytes), including the image /
mapped / private split, the process heap and the ten largest private allocations. In the app, F12
shows the same parts in one line under `memory:`.
