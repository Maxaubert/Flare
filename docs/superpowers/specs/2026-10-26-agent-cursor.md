# The agent's cursor in the web panel: spec and plan

Date: 2026-10-26. Owner ask (Max): "can it use it like Claude in Chrome where I can see a cursor". Style picked from six
animated mockups: **A, an orange arrow with a small name tag** ("Claude"), a ripple on each click.

## Why

An agent already clicks, types and scrolls in Flare's browser panel (Surfaces stage 4, `browser_*` tools, CDP
`Input.dispatchMouseEvent`). The person sees only the page react. A visible pointer shows where the agent is about to act,
so the person can follow it and take over in time, as with Claude in Chrome.

## What the person sees

- While an agent call acts on the page in front, an **arrow** in the accent orange (`#fe8f34`, white 1.6 px edge) with a
  **tag** beside its tip naming the agent (the ribbon's name: "Claude", "Codex", else "Agent"). Same shape as mockup A:
  `docs/surfaces/agent-cursor-mock.html`.
- Before a click (or a type into a ref, or a scroll over a ref) the arrow **glides** from where it was to the target in
  180 ms (ease-out), then the click is sent. The first appearance starts at the target itself (no glide from nowhere).
- On each press, a **ripple**: an orange disc at the tip, scale 0.3 to 1.4, fading out in 400 ms. A double click ripples twice.
- A key press or a look moves nothing. A scroll without a ref has no point and moves nothing.
- The arrow stays where it last acted while the ribbon is up (the call plus `RIBBON_HOLD_MS`, 1.5 s), then fades out in 200 ms.
- It is gone at once when the person takes over (`interrupted`), when the panel closes, its tab goes behind, a Flare menu or
  dialog covers the page (the still), or the window is minimised. It follows the window when it moves or resizes, and the
  panel when its splitter moves.

## Decisions (mine, for review)

1. **Drawn by Flare in a window of its own, never in the page.** A small layered popup (`WS_POPUP`, `WS_EX_LAYERED |
   WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`), owned by Flare's main window, so it sits just above Flare and
   under any other app's window, takes no clicks and no focus, and has no taskbar button. Why not a DOM element injected into
   the page: the page could see, move or hide it (a site could fake or bury the agent's pointer), it would be in
   `browser_look`'s screenshot and accessibility list, a navigation would wipe it, and iframes and CSP make it fragile.
   The page never learns the cursor exists.
2. **Pixels drawn in Rust**: the arrow as a polygon with analytic anti-aliasing (the taskbar disc's approach), the tag text
   with GDI into a coverage mask (as `taskbar::text_mask` does), composed premultiplied BGRA, shown with `UpdateLayeredWindow`.
   One bitmap per frame of the glide and ripple (60 fps at most, only while moving or rippling). No new dependency.
3. **Coordinates**: CDP points are CSS pixels of the visual viewport. Screen pixels = the panel's physical rectangle (already
   known, `set_bounds`) + point x `bounds.w / viewport.w`, offset by the window's client origin (`ClientToScreen`). A click by
   point asks `Page.getLayoutMetrics` first (a click by ref already does), so every point comes with its viewport.
4. **The glide delays the click by 180 ms.** Nothing waits (rule 34): the job gets a `Stage::Glide` that the agent tick
   (16 ms, already running during a call) ends. The `ACT_BUDGET_MS` timeout is unchanged.
5. **No setting.** It is part of the agent using the page, like the ribbon. (A switch can come later if wanted.)
6. **Under `--e2e`** the window is never made; a recording stand-in keeps the cursor's state (shown, position, ripples)
   for probes, as the taskbar badge's `RecordShell` does.

## Non-goals

- A cursor for the person's own mouse, or for mods/plugins driving the page.
- Showing the cursor in Claude in Chrome or any external browser.
- Moving the real system pointer.

## Plan

Built 2026-10-26 (steps 1 to 7); what was decided while building is in `docs/surfaces/decisions.md` ("The agent's cursor").

One branch `feat/agent-cursor`, one PR. Each step with its tests; fmt, clippy, the `flare` bin and `flare-surfaces`,
`flare-win` suites (known failures excepted).

1. **`flare-win::agent_cursor` (pixels, pure):** `arrow_coverage(scale)`, `compose_cursor(scale, label, ripple: Option<f32>)
   -> Bitmap` (premultiplied BGRA, size, tip offset). Tests: tip pixel opaque orange, edge anti-aliased, white rim, tag
   present and readable width, ripple alpha falls with t, every scale 1 to 3.
2. **`flare-win::agent_cursor` (window, Windows only):** `CursorWindow::new(owner)`, `show_at(screen_x, screen_y, bitmap)`,
   `hide()`. Layered, transparent to input, owned, never activated. A trait `CursorShell` with the real one and
   `RecordCursor` for tests and e2e.
3. **`flare-surfaces::agent_cursor` (state, pure):** where the arrow is, the glide (from, to, start, 180 ms ease-out), the
   ripples, the fade; `frame(now) -> Option<CursorFrame>` and `needs_tick(now)`. Tests: glide midpoint, first show has no
   glide, ripple timing, fade after the hold, hide is immediate.
4. **Job points:** `agent_job` reports the screen-bound point of each pointer action before its events (`Out::Point { x, y,
   vp_w, vp_h }` or a field on the stage), adds `Page.getLayoutMetrics` to a click by point, and a `Glide` stage of 180 ms
   before the events go. Tests in `agent_job.rs`: the order (metrics, point, wait, events), a double click, scroll by ref,
   type into ref, no point for a key or a look.
5. **`web_ctl` wiring:** map the point to screen pixels with the panel's bounds and the client origin; drive the cursor from
   the agent tick; hide it on takeover, panel close, tab behind, still, minimise; reposition on window move, resize and
   splitter. Tests with `RecordCursor`: shown during a click, where, hidden on `interrupted`, hidden while a menu covers the
   page, gone after the ribbon.
6. **e2e:** the `agent_hooks`-style probe reads the stand-in; a scenario clicks a button by ref and sees the cursor at its
   middle, then gone after the hold.
7. **Docs:** `docs/surfaces/decisions.md` (the decisions above), `docs/features.md` (the web panel section), the mockup
   copied to `docs/surfaces/agent-cursor-mock.html`, and the by-hand list (the real window over the panel at 100 and 150 %).
8. **Live check:** a release build with its own profile, a page opened with `open_page`, a `browser_click` by ref; Max looks.

## Update 2026-10-27: it never jumps, a border while the agent works

Max's decisions (recorded in `docs/surfaces/decisions.md`, "The agent's cursor never jumps"):

- **Never jump.** After its fade the arrow remembers where it rested; the next action on the same page shows it there and
  glides on. Only the first action on a page, one after a navigation or a viewport size change, and one after a hide (a
  takeover, the page out of sight) appear at the target (held 180 ms so the arrow is seen before the press).
- **The glide is clearly visible:** 350 ms plus 0.5 ms a CSS pixel, at most 700 ms, ease-in-out (cubic). The job holds
  its events exactly that long (`Job::glide_for`, the window says the length once it takes the point).
- **A light orange border** round the page from the agent's first call until the ribbon's hold ends, fading with the arrow
  in 200 ms, gone at once on a takeover: the cursor's orange at 55 %, 2 px at 100 %, square like the page's corners. A second
  layered popup the size of the page (Slint cannot draw over the browser's child window), transparent to clicks; a fade
  changes only its constant alpha.
- **Animation effects off:** no glide and no fade; the arrow is placed at once, the border shows and goes at once.
