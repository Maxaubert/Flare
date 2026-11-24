# Panel launcher and "Open in"

Status: mockups approved by Max, 2026-11-24 (placement 3 "ghost button that blooms", icon 6 "Layers"). Plan:
`docs/superpowers/plans/2026-11-24-panel-launcher.md`. Rules 1372 to 1376.

Mockups: `2026-11-24-panel-launcher-mockup.html` (the first flow, with "Open in"), `2026-11-24-panel-launcher-10-mockups.html`
(the ten placements), `2026-11-24-panel-launcher-ghost-button-mockup.html` (the pick, with the ten icons).

Why: a closed side panel has no way in except a file link, an agent, or a chord. The person cannot open the browser, the
device or a file of their own choice in the panel with the pointer, and cannot hand what the panel shows to the app that
owns it.

Anything marked **Claude's call** was decided by the builder, not by Max.

## 1. The launcher, per tab

- While the tab's side panel is **closed**, a round ghost button sits in the top right of that tab's terminal: 8 px
  from the top, 10 px from the right, 26 px round, a 1 px divider border, the menu colour (`side-flat`) at 60% alpha,
  and the Layers icon (three stacked rhombi, 14 px, stroke 1.6).
- Idle it is at 45% opacity, so terminal text under it stays readable. It is solid while hovered or while its menu is
  open.
- It is hidden while the tab's panel is open, and wherever a panel cannot open: Settings or the start screen in front,
  a window too narrow for a slot. **Claude's call:** also hidden while a remote tab's fitted conversation view or the
  resume skeleton covers the terminal.
- One button per window, standing for the tab in front; switching tabs closes its menu.
- **Claude's call:** the Preview chip (stage 7) lives in the same corner. While the launcher shows, the chip moves left
  of it (8 px apart).
- **Claude's call:** no tooltip on the button. It is icon-only, but its hover opens a labelled menu right where a tip
  would go. Screen readers get "Open in the side panel" and the expanded state.

## 2. Hover rules (Max's words)

"It must appear only when the cursor is over the button, not wherever the menu would be; and it must not disappear
while the cursor goes from the button to the menu across the small gap."

- The menu opens only on the pointer entering the **button**, on a click, or from the keys.
- A closed menu takes no pointer input at all: it is not drawn and its touch area is off.
- The 6 px gap between the button and the menu is covered by an invisible bridge that belongs to the menu.
- Leaving the button or the menu starts a 250 ms grace. Entering either cancels it. After 250 ms off both, it closes.
- Escape closes it. Picking a row closes it. Arrow keys move inside the menu.
- **Claude's call (keys):** Flare has no Tab key focus over the terminal (Tab belongs to the shell), so the button gets
  "keyboard focus" by a click or its accessible action. A menu opened that way holds Up, Down, Home, End, Enter and
  Escape; any other key closes it and goes where it would have gone. A menu opened by the pointer alone takes no key:
  Escape closes it and still reaches the shell, so an agent's Escape is never swallowed by a menu the pointer brushed.
- **Claude's call (click):** a click on a menu the pointer opened keeps it open and gives it the keys (the mockup
  closed it, which reads as "the click did nothing" after a hover). A click on a keyed menu closes it.

## 3. Menu rows

Top to bottom, labelled, with the mockup's stroke icons (globe, phone, file, clock):

| Row | What it calls | When shown |
|---|---|---|
| Browser | `AppCtl::browser_open(tab, "about:blank")`, then `focus_address` (the address field holds the keys) | always |
| Device | `AppCtl::device_open(tab, None)` (the device list) | only where `DeviceCtl::offered()` |
| Open a file... | the system Open dialog (`flare_win::pick::pick_any_file`) in the tab's folder; the file goes through `term_ui::previewable` on a worker, then `TermMsg::Preview { asked: true }`, the same path a file link's "Show in side panel" takes (`show_preview_asked`) | local tabs only |
| (rule) Last panel | `PanelCtl::reopen(tab)` (what `toggle_front_panel` brings back) | when the tab's closed panel showed a page, a device or a file |

- A picked file the panel cannot show is said in the panel ("Flare cannot show this kind of file in the side panel."),
  as a refused preview is. **Claude's call:** new words, because the existing refusal speaks of "since the menu
  opened". A cancel does nothing.
- **Claude's call:** Browser always opens a new, empty page (the "new-tab state"), even when the tab's panel hid a page.
  That page comes back through Last panel instead. This is unlike Ctrl+Shift+L, which brings a hidden page back.
- **Claude's call:** Last panel is left out (not dimmed) when there is nothing to bring back. "Nothing" also covers an
  empty page that never went anywhere (`about:blank`), a note or text handed over, and the Changes view.
- **Claude's call:** Open a file... is left out on a remote tab: its folder is on another machine.
- Rows that do not apply are left out, so the arrows never land on a dead row.

## 4. "Open in" in the panel header

A labelled button with an external-link icon, no tooltip (it is labelled).

| Panel | Words | Action |
|---|---|---|
| A file on the click allowlist (rule 14) | "Open in <app>" when the default app's name is known, else "Open in default app" | the path openers' `open_path` (ShellExecute "open" on Windows, `NSWorkspace` on a Mac), through `open_term_path` on a worker |
| A file not on the allowlist (a script an agent showed) | "Show in Explorer" / "Show in Finder" | `reveal_path`, never run. **Claude's call.** |
| A web page (http or https) | "Open in browser" | `deps.open_url` (the links' opener) |
| Device, Changes, a mini-app or diagram, a mod's page, text handed over, an empty page | none | |

- The default app's name: `AssocQueryStringW(ASSOCSTR_FRIENDLYAPPNAME)` on a worker, once per extension, cleaned
  (control characters out, at most 32 characters, a bare `.exe` name counts as none). **Claude's call:** on a Mac the
  name is not looked up in this change (it says "Open in default app"); under `--e2e` and in tests nothing is looked
  up, so the words are the same on every machine.
- Narrow panels: the header gives the button only the room the title can spare (the title keeps 80 px): the full words, else "Open" (or "Show"), else no button. In a web page's bar it sits left of
  "Draw arrow" and the close, says "Open" below 440 px and gives way below 280 px so the address keeps room.
  **Claude's call** (the numbers).

## 5. Both systems, themes, sizes

- Windows and macOS: the same Slint drawing and controller. The Open dialog is `rfd` on both. The file opens through
  the existing path openers on both.
- Every theme, light and dark: only theme colours (`side-flat`, `divider`, `hover`, `text`, `text-soft`).
- Small and portrait windows: the button hides when no slot fits; the menu is about 160 px tall and is clipped by the
  terminal box in a window shorter than that. **Claude's call:** no special layout for that case.
- DPI: whole device pixels (`Px.snap`), checked at 100, 150 and 225 percent.

## Not in this change

- An e2e scenario that drives the real window (the ui tests drive the real controller and window headless instead).
- A Mac app-name lookup.
- A hotkey or a setting to hide the launcher (ideas in the mockup's notes).
