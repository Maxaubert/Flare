# Material spike results (Task 19)

Date: 2026-10-17. OS: Windows 11 build 10.0.26300 (`support()` = Ok, so acrylic and system backdrop are available).
Adapter: NVIDIA GeForce RTX 5090, DX12. Tool: `tools/material-spike` (`--mode ...`), window 400 x 300 logical,
900 x 675 physical at this PC's 225% scale, bottom right corner, inactive, always on top, off the taskbar.
Pictures: `docs/screenshots/m4/material/*.png`.

How it was measured: the window is captured with GDI from the screen DC 1.5 s after the `flare_win` calls. For the glass
modes a plain white window (the predefined STATIC class, `SS_WHITERECT`) sits right under the test window, so there is
something bright to show through (the desktop in that corner is near black, where a see-through window is
indistinguishable from an opaque one). The plan's E1 asks for a non-flat difference to the closed capture; over a flat white
backdrop the difference is flat by construction, so E1 is judged by the centre pixel against the closed capture and
against the ground instead (a deviation from the plan's `stddev > 0`).

Capture noise: other things on this PC (other agents' windows and a cursor overlay) intermittently cover that corner, so a
capture sometimes shows their pixels instead of ours. The runs below are the ones where a plausibility check held (closed
capture is the white backdrop, border ground is the distinct `#2a3b4c`, premul centre is not the desktop), retried up to
7 times; `centre_series_r` (the centre red value every 250 ms) was recorded with each run.

## E0: alpha modes

`probe`: the `visual` swapchain (`DxgiFromVisual`) offers `[Auto, Inherit, Opaque, PostMultiplied, PreMultiplied]`; the
`hwnd` swapchain offers `[Opaque]`. Both as expected: transparency needs the visual swapchain.

## E1: glass works with the visual swapchain

`glass-gpu --level 40`, ground `#171717` at alpha 0.6, acrylic on: centre `[47,47,47]` over a white backdrop, which is
`[255,255,255]` with the window closed. Not the ground (differs by 24) and not the backdrop: glass works. With acrylic off
the same window shows `[116,116,116]` = 0.6 x 23 + 0.4 x 255, so the swapchain's own alpha is honoured, and acrylic then
adds its dark frosted layer under the ground (acrylic layer about 82). Pass.

## E2: alpha 1.0 looks like an opaque window

`glass-gpu --level 0`: centre `[23,23,23]` = `#171717` exactly, with acrylic on and off. A transparent surface at alpha 1.0
is identical to an opaque one (D8 holds). Pass.

## E3: premultiplied texture

`premul`: a 64 x 64 `Rgba8Unorm` texture of `(128,0,0,128)` over a white ground, through `Image::try_from(wgpu::Texture)`.
Centre `[191,127,127]`: R 191 means Slint reads the texture as STRAIGHT alpha, not premultiplied (premultiplied would give
R 255, G 127). The plan's assumption is wrong: Task 20b is needed.

## E4: DWM border on the frameless window

`border`: `set_border(hwnd, "#ff00ff")` returned true, but the left column (and the next one) of the capture is the
window's own colour (`[42,59,76]`), never `#ff00ff`. The DWM border does not show on the frameless window: Task 25's
Slint-drawn edge step is required.

## E5: software renderer with the transparent attribute

`glass-software --renderer software --level 40`: with acrylic off the centre is `[115,115,115]` over white (real alpha
blending, like the GPU path); with acrylic on it is `[47,47,47]`; at level 0 it is `[23,23,23]`. So the software
renderer DOES show through on this PC; decision D5's expectation (solid or black) did not hold here. The software renderer
stays a fallback; whether to offer glass on it is a product decision (the See-through row can stay GPU only on purpose).

## E6: read back

`readback`: set backdrop = Acrylic, border = `#ff00ff`, dark = true (all three calls returned true). Read back:
attribute 38 = 3 (equal), attribute 20 = 1 (equal), attribute 34 (border colour) = no value: `DwmGetWindowAttribute` does
not return it (null). So rule 73's read-back works for 38 and 20 and cannot be done for 34.

## E7: cost

10 fresh processes per swapchain, visual (transparent, acrylic) vs hwnd, same window and same ground:

| | cold start to first frame (median) | mean of 300 forced redraws (median) |
|---|---|---|
| visual | 1131 ms | 6.995 ms |
| hwnd | 1139 ms | 7.003 ms |

No measurable cost for the visual swapchain (start differs by 8 ms in the other direction, inside the run-to-run spread of
about 60 ms; the frame is the display's refresh, about 143 Hz). The cold start is dominated by wgpu and Slint start-up in
either case.

## Decisions

Transparent surface: yes
Premultiplied texture: no
DWM border visible on the frameless window: no
Software renderer glass: other (it shows through like the GPU path: 115 at level 40 without acrylic, 47 with it)
Visual swapchain cost vs hwnd: +0 ms start, +0 ms frame
DwmExtendFrameIntoClientArea needed: no
