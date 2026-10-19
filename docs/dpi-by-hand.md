# Display scaling, by hand

The owner's two-monitor checklist (M10, Task 72, Q6). `dpiMatrix` and `monitorMove` prove the
numbers on a parked window at every scaling; they cannot drag a window across a real seam. Do this
once per release with two monitors of different scalings (for example 100% and 225%).

- [ ] Drag the window between the monitors at rest.
- [ ] Drag it between the monitors during output (`type` a big file, or a `for /L` loop).
- [ ] The tab strip hairlines look the same weight on both monitors.
- [ ] The sidebar corner and edge are clean on both.
- [ ] The find bar, a menu, a dialog and Settings look right on both.
- [ ] The grid is sharp (no soft or doubled glyph edges) on both.
- [ ] The prompt line survives the move: type after it and the text lands at the prompt.
- [ ] Win+Shift+Arrow moves it to the other monitor and it settles the same way.
- [ ] No black frame while it moves.
- [ ] Remote desktop at 150% looks right too.
