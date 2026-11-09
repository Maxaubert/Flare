# Pictures in the terminal grid (#35): probe result

Measured 2026-11-09 with the bundled ConPTY, OpenConsole 1.25.2603.03002 (`1.25.260303002-preview`), by
`crates/flare-term/tests/apc_conpty.rs`. The spec's gate is in
`docs/superpowers/specs/2026-11-09-surfaces-later-design.md`.

**Verdict: no-go by the spec's table.** No picture route survives a ConPTY repaint. The other two questions came out
better than the spec feared, so the decision below is a real one for the owner. No image layer was built and #35 stays
open.

## What was measured

The program under ConPTY is the test binary itself, printing fixed bytes and calling the console API. No shell runs.
Flare has no picture layer, so the place tests replay the bytes ConPTY delivered into an `Emulator` that writes the
picture's cells at the sequence. ConPTY gets no feedback from the front end beyond the startup replies, so that replay
sees what a picture-aware Flare would see.

### 1. Does it arrive?

| Sequence | Arrives | Where |
|---|---|---|
| APC (kitty, `ESC _ G ... ESC \`) | Yes, byte-exact | Between the text around it, nothing in between |
| DCS (sixel `ESC P q`, and an unknown `ESC P z`) | Yes, byte-exact | Same |
| OSC 1337, small | Yes, byte-exact | Same |
| OSC 1337, 2 000 000 bytes | Yes, byte-exact, in 0.66 s | Same |

Answers questions 1 and 2: this ConPTY forwards all of them, in order.

### 2. Does text printed after it land below it?

ConPTY's buffer has no idea the picture took rows, so it depends on what makes room:

| What makes room | Text after it | A prompt the console places |
|---|---|---|
| Nothing, the cursor moves past the picture (kitty's default, `imgcat`) | Below | **On the picture.** ConPTY sends `ESC[3;1H` for row 2, inside the picture's rows 1 to 4 |
| The terminal scrolls the rows above the cursor up and paints there, so the cursor never moves | Below | Below |
| The program prints blank rows, moves back, sends the picture, moves down | Below | Below |
| The program prints the cells as text (kitty's Unicode placeholders, `U+10EEEE`) | Below | Below |

The prompt case is `SetConsoleCursorPosition` in the console's own coordinates, the way a line editor can place itself.

The second row is what Flare could do on its own for a plain `imgcat`. Its limit, reasoned and not measured: a picture
taller than the rows above the cursor (for example right after `cls`) starts in the scrollback.

### 3. Does it survive a resize and a repaint?

| Event | Result |
|---|---|
| Four resizes (60x24 to 48x24, 72x24, 72x16, 72x30), plus one more with the picture in the scrollback (measured while writing the probe, not kept in it) | **ConPTY sends zero bytes.** The picture stays. The program confirms the console really is 72x30 |
| 40 lines of output | The picture scrolls into the history, whole, for every row-making method above |
| A VT alternate screen (`ESC[?1049h` and `l`) | Passes through. The picture is still there afterwards |
| **`SetConsoleActiveScreenBuffer` to another buffer and back** | **ConPTY repaints the whole screen from its own buffer (5188 bytes), and the picture is gone** |

What the repaint leaves:

| Placement | After the repaint |
|---|---|
| Scrolled above the cursor | Picture gone. Rows 0 to 7 repaint as ConPTY knows them, so `EARLIER 0` to `3` now also sit in the scrollback, duplicated |
| In rows the program reserved | Picture gone, the rows blank. Everything else is correct |
| Cells printed by the program | Each cell is replaced by `U+FFFD`, one row of replacement characters per picture row |

Measured once and not kept in the test: a full-screen `FillConsoleOutputCharacter` followed by a cursor home also
repaints every row, which clears the picture as a clear should.

Not measured: other things that might repaint, such as `SetConsoleScreenBufferSize`, `SetConsoleWindowInfo` or `mode
con`; how often real programs swap buffers (this probe only shows the swap does it).

## Passthrough mode

Measured inside the probe only, never adopted. `CreatePseudoConsole` called directly on the bundled DLL, flags `0x6`
against `0xE` (`PSEUDOCONSOLE_PASSTHROUGH_MODE`, which portable-pty cannot pass):

- The APC arrives in both.
- The buffer swap repaints in both, with the same 5188 bytes.

In this build the flag changes nothing that matters here. What it would cost is therefore all cost and no gain:
replacing portable-pty's pty opening, re-testing `resize.rs` and the regression rules that lean on it, and still not
fixing the repaint.

## The decision left for the owner

The repaint is the only condition that fails, and it fails on one trigger the probe found. If that loss is acceptable,
a kitty front-end with cells that scroll, plus a documented limit that a buffer swap erases pictures on screen, can be
built on what is above. Stopping is what the spec's table says to do, and relaxing it is not this probe's call.
