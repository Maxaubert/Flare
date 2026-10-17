# pt-shot

Screenshots the installed Prism Terminal wearing each theme, for the theme matrix beside Flare.

    node tools/pt-shot/pt-shot.mjs --out <folder> --scales 1,1.5,2.25 --themes all|id,id --size 1000x640
    node tools/pt-shot/pt-shot.mjs --selftest

Drives `C:\Users\Admin\AppData\Local\Programs\PrismTerminal\PrismTerminal.exe` through Playwright's
Electron driver (from the Prism repo's `node_modules`), in a throwaway `--user-data-dir` under `%TEMP%`,
with `--e2e` (window parked offscreen, unfocusable). It prints each theme's sample from
`docs/theme-sample.txt` and writes `<id>-<pct>.png` plus `VERSION.txt`.

It never touches `%APPDATA%\PrismTerminal*`. It exits 1 if the profile is not removed, a real profile
folder's timestamp changed, or an image is not `round(1000*s) x round(640*s)` or is blank.
`--themes all` reads the ids from `tools/oracle/out/wall_order.json` if present.
`--sample <file>` and `--exe <path>` override the defaults.
