# Maybe issues

Things seen once that could not be reproduced yet. Each one waits for a second sighting before anyone works on it.
When one comes back, add what was seen and open a fix branch. When it is fixed or ruled out, remove it.

## Double-click on the title bar gives a broken maximize (2026-11-18)

- **Seen:** Windows, build fe2354b (Dev or Stable). Max double-clicked the title bar. The window went into a sort of
  pseudo fullscreen: it covered the taskbar, the settings cog was gone and the new tab button looked odd.
- **Not reproduced:** after that, maximize looked fine again.
- **Guess:** Flare draws its own title bar, so a double-click may maximize through a different path than the
  maximize button, one that does not keep to the monitor's work area (taskbar excluded). The fe2354b build changed
  no window or maximize code, so it may be older.
- **Next time:** a screenshot, whether it was a double-click or the maximize button, and which monitor.
