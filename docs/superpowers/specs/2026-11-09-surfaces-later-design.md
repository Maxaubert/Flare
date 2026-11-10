# Surfaces, later items: ask the agent, image thumbnails, inline pictures, 3D viewer

Status: owner picked these on 2026-11-09 for an overnight build ("plan and spec first ... dont ask about
merges ... ill test in the morning"). Plan: `docs/superpowers/plans/2026-11-09-surfaces-later.md`.
Issues: #33, #17, #35, #36. Closed the same day as not wanted: #64, #32, #34.

These are Flare core features, not plugin UI, so the plugin mockup-first rule does not apply. The look of each
new element is written down here so it can be judged in the morning.

## Shared rules

- Every feature is general and data-driven. No special case for one agent, plugin or file.
- Nothing an agent or a page does can type into a shell (rule 188). Only a person's gesture can.
- Paths from terminal output or from an agent go through the same vetting as a click (rules 13, 14), off the
  UI thread (rule 34).
- Tests run headless. Nothing takes the screen.

## #33 Ask the agent about a selection

A person selects something and sends it to the agent in that tab. It lands in the prompt and is **never
submitted**: the person reads it and presses Enter.

**Gesture.** A small "Ask agent" button in the panel header. It is Flare chrome, so a page cannot fake it. It is
visible only when the tab has an agent (`hub.agent_here`), whichever agent that is. A row "Ask agent about this
selection" is added to the terminal right-click menu for a grid selection, under the same gate.

**What gets sent, by panel kind.**

| Panel | Sent |
|---|---|
| Web | The page's selection, read with `window.getSelection().toString()`. With no selection: a toast, nothing typed |
| Text, Image | The file's quoted path. These panels have no selection today |
| Terminal grid | The selected text |

**Text gate.** A new pure function in `crates/flare/src/ask_agent.rs` strips invisible characters (`handed_text`)
and controls (`sanitize_paste`), caps at 4 KB with a visible `[cut]` marker, and frames the text as a quote with
its source: `About this from <label>:` then `> ` per line. The label is the host or file name, sanitised and
capped. Page text is untrusted, so the framing is visible, not hidden.

**Delivery.** `handle.paste` (bracketed when the program asked; unbracketed never ends in CR). Then focus the
terminal. Refused with a toast when the tab has no live agent or a question box is up, so a paste can't answer a
question by accident. A web result that arrives after the panel navigated is dropped.

**Not in this version.** A row in the page's own right-click menu (no `ContextMenuRequested` path exists),
selection inside the Text panel, any auto-submit, any ctl, MCP or plugin entry point.

## #17 Image thumbnails on hover

Rows cannot be added under a line without changing the agent's text, reflow, selection and scrollback. The
preview is therefore a floating popover over the grid, anchored to the path.

**Behaviour.** Point at an image path for 350 ms and a thumbnail appears, above or below the line so it never
covers it. The look follows the tooltip bubble: border, shadow, 120 ms fade, longest side 256 px, caption with
file name and pixel size. It ignores the pointer. It hides when the pointer leaves, the link under the cell
changes (scrolling under a still pointer included, rechecked every tick), or on a key, tab switch, resize, DPI
change, Find or a menu.

**Loading.** One decode at a time on a worker, sharing the panel's decode lock. Vetted exactly like a click
(`resolve_openable_file`, `classify_resolved`, `loadable`). Limits 16 MB and 25 MP, below the panel's. Cache in
memory only, 32 entries and about 8 MB, keyed by path, length and mtime so an edited picture refreshes. Only the
hovered path is ever decoded, so a huge output costs nothing.

**Setting.** "Show thumbnails when hovering image paths", Settings > Terminal, on by default. Off means no state
and no worker.

**Not in this version.** Always-on thumbnails, web image URLs (network on hover), SVG (scripts), video, PDF,
animation (first frame only), a disk cache.

## #35 Pictures inside the terminal grid

**Go or no-go first.** Every shell runs through ConPTY, which re-renders from its own buffer. Whether a graphics
sequence reaches Flare at all is unknown, and a sequence that arrives is not enough: ConPTY doesn't know the
picture took up rows, so its next repaint or cursor move may land on top of it.

A headless probe through the real bundled ConPTY (`crates/flare-term/tests/apc_conpty.rs`) must answer:

1. Does an APC (kitty) sequence arrive? Does a DCS? Does a 2 MB OSC 1337 (iTerm2 style)?
2. If one arrives, is it in the right order relative to the text around it?
3. Can the picture hold its place: text printed after it lands below it, and it survives a ConPTY repaint and a
   resize?

| Result | What ships |
|---|---|
| APC arrives and holds its place | Kitty front-end |
| Only OSC 1337 arrives and holds its place | OSC 1337 front-end on the same image layer |
| Neither | **Stop.** No code merged. `docs/graphics.md` records the evidence and #35 stays open |

`PSEUDOCONSOLE_PASSTHROUGH_MODE` is out of scope for tonight. It needs a replacement for portable-pty's pty
opening and changes the cursor and resize behaviour that `resize.rs` and about 20 regression rules rest on.
`docs/perf/heavy-output.md` records it as the owner's call. The probe may measure it, only inside the test.

**Design if it is a go.** One image layer, independent of the protocol.

- `flare-term/src/graphics.rs`: parser, chunk assembly, PNG and raw 24/32 decode, an `ImageStore` with caps (16 MP
  per image, 64 MB total, LRU). Direct transmission only, so an escape sequence can never read a file.
- `scan.rs`: a new body state for the chosen sequence with its own cap, cancel handling and split-chunk safety.
- Placement by cells: the picture is written as placeholder cells, so scrollback, alt screen, clear and resize
  follow the text. All text consumers (find, copy, link scan, row text) filter the placeholder to a blank at one
  choke point.
- `Grid.tiles` and `Scene.pictures`, painted after glyphs and before overlays by both renderers. Tile edges come
  from `cell_origin` so fractional advance leaves no seams. The GPU gets a textured-quad pipeline with a capped
  texture cache and memory accounting.
- Decoding is capped and never happens while holding the emulator lock.

**Not in this version.** Animation, composition, z-index, file or shared-memory transmission, sixel.

## #36 3D model viewer

A new `show_model {path, title?}` agent tool opens `.glb`, `.gltf` or `.stl` in a panel. It follows `show_audio`
exactly: a Flare-authored mini-app page, so no new panel kind.

**Flow.** `view.model` method, resolved with rules 13 and 14, extension checked on both the typed and the real
name, bounded read with a 64 MB cap, then `index.html`, `viewer.js` and `model.<ext>` are written to the app
folder. No network.

**Page.** `default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src 'self' data: blob:; connect-src
'self'; worker-src 'none'`, with frames, objects and forms none. No eval and no WASM. The viewer fetches the file
itself, so model bytes never pass through the page source.

**Viewer.** Orbit, zoom, pan, double-click reset, wireframe and grid toggles, a neutral studio light, the camera
fitted to the model, background following light or dark. A model over 5M triangles or a corrupt file shows a clear
message instead of a blank panel. `window.__flareModel` settles for tests.

**Bundle.** three.js, pinned to a release at least two weeks old, built once with esbuild from
`tools/model-viewer/`. The output is committed at `assets/three/viewer.js` with README, MIT licence, notices entry
and a pinned-hash test, as Mermaid is. A table of formats makes a new format one row plus a loader.

**Not in this version.** Click a path to preview (needs a new click outcome and a way to restore app panels),
`.gltf` with separate `.bin` or texture files (the tool text says to use `.glb`), Draco or other WASM decoders,
OBJ, FBX, animation.

## Merging

The owner waived the per-PR merge question for these four PRs only. A PR merges when local fmt, clippy and tests
pass, an independent review has no open must-fix finding, and CI fails nothing beyond what main already fails. A PR
that misses this stays open and is reported. Flare is not installed overnight: a release build is prepared and the
install is asked about in the morning.
