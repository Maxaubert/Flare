# settings-matrix

Flare's Settings tab beside Prism Terminal's, at every display scaling that matters.

    node tools/pt-shot/pt-settings.mjs --out docs/screenshots/m7/pt
    powershell -File tools\settings-matrix\run.ps1 -Exe <flare.exe>
    powershell -File tools\settings-matrix\compose.ps1
    powershell -File tools\settings-matrix\check.ps1

`tools/pt-shot/pt-settings.mjs` is the one source of the image list (`--list` prints it as JSON, and
the three scripts here read it). The images are `<page>-<variant>-<pct>.png`:

- the six pages (`plain`) at 100, 150 and 225 percent;
- Appearance with `picker`, `editor`, `ask`, `help` and `wall-expanded`, and Terminal with `select`
  and `help`, at the same three scalings (Appearance has no select row, so it has no select image);
- a Find result (`find`, the text `colour`) for Appearance at the same three;
- Appearance and Terminal (`plain`) at all eight scalings: 100, 125, 150, 175, 200, 225, 250, 300.

A name two groups share is made once, so the list is 52 images.

## The two sides

Both sides show the same fixed state: theme `pt-default`, font `cascadia`, size 100 percent, edges
`hairline`, tab layout `horizontal`, new tab mode `folder`, everything else default, the window
1000 by 640 css px, and the Settings tab alone in the strip.

- **Flare**: `flare.exe --scene settings --settings-page <page> [--settings-open <word>]
  [--settings-find <text>] --scale <s> --size 1000x640 --renderer software --screenshot <png>`.
  The scene has a fixed fake environment (shells "PowerShell 7", "Windows PowerShell", "Command
  Prompt", "Ubuntu"; the Explorer verb off; version `0.0.0-scene`; glass supported; the log folder
  `C:\Users\You\AppData\Roaming\Flare\logs`; no NVIDIA) over an in-memory store. It reads and
  writes no profile folder. Open overlays: `select` (the first select row of the page), `picker` (the
  first colour row), `editor` (the chosen theme's pencil), `ask` (the accent is changed so the theme is
  dirty, then another card is chosen), `help` (F1) and `wall-expanded` (Show all, the pane then scrolled
  330 css px). A control below the fold is scrolled to the middle first, as a click does.
- **Prism Terminal**: the installed app, driven by Playwright's Electron driver from the Prism repo's
  `node_modules`, in a throwaway `--user-data-dir` under `%TEMP%`, with `--e2e` (offscreen, unfocusable).
  It never touches `%APPDATA%\PrismTerminal*`: it compares their timestamps before and after and exits 1
  on any change, or on an image that is not `round(1000*s)` by `round(640*s)` or is blank.
  `--selftest` takes one Appearance image at scale 1 and prints `ok`.

## The scripts

- `run.ps1`: the Flare images. A non-zero exit of the scene stops the run.
- `compose.ps1`: `magick pt flare +append` side-by-sides captioned "Prism Terminal <version>" and
  "Flare", and a contact sheet per main scaling.
- `check.ps1`: a table of image sizes against `round(1000 * s)` by `round(640 * s)`, a blank check, and
  for each pair the share of the page body where the two differ by more than 12 percent in luminance
  after both are shrunk to the same height. That number is for the reader of the images; it is not a gate.
  Exit 1 on a missing, wrongly sized or blank image.
