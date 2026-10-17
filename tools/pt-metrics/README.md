# pt-metrics

Measures the installed Prism Terminal's Settings page (rects and computed style) at display scales 1 and
1.5, so Flare's settings layout is held to the measure (Appendix B of the M7 plan, Tasks 24 and 45).

    node tools/pt-metrics/pt-metrics.mjs [--out <json>] [--scales 1,1.5]
    node tools/pt-metrics/pt-metrics.mjs --selftest

Output: `crates/flare/tests/fixtures/pt_settings_metrics.json`, `{ pt_version, scales: { "1": {...}, "1.5": {...} } }`.
Per scale: one object per rail page (`appearance`, `terminal`, `agents`, `dictation`, `diagnostics`, `about`) and
one each for `colourPopover`, `shellSelect`, `themeEditor` and `helpPanel`. Each entry is
`{ x, y, width, height, style }`. Page entries are relative to `[data-settings-page]`; popup entries are
absolute in the window. Numbers are CSS px to 2 decimals.

Runs the INSTALLED `PrismTerminal.exe` in a throwaway `--user-data-dir` under `%TEMP%` with `--e2e` (window
parked offscreen, unfocusable), the same launch block as `tools/pt-shot`. It never touches
`%APPDATA%\PrismTerminal*`; it exits 1 if a real profile folder's timestamp changed, the throwaway profile
was not removed, a selector was missing, or a self-check failed (rail 244 wide, row at least 58 high, tile
32 by 32, switch 36 by 20, popover 226 wide, help panel 620 wide).

Selectors that exist only in a state the script does not reach are left out on purpose: the "Reset" link
(`[data-follow-theme]`, shown only for a custom colour) and a third popover slider (the popover has two
`role=slider` bars; the square is not one).

Note: the installed version at capture time was 0.58.0.0 (the plan's source reference is v0.55.6); the
JSON records the version it was taken from.
