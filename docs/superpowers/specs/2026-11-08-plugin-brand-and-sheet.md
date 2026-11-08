# Plugin brand, details sheet and themed prompts (2026-11-08)

Max's picks from the mockup `2026-11-08-bitwarden-mock.html` (next to this file): plugin card B4, unlock B, fill
approval B, tab menu A. Rule from Max: any plugin UI is mocked in the browser and approved first.

## Goals

1. **One-row plugin card** in Settings > Plugins: logo, name and version, one-line description, switch, Update, an
   **i** button (plain, not coloured) and a **trash** button (Remove). No Show panel button, no Made by / Allowed to /
   Full control rows.
2. **Details sheet** (the i opens it, a modal in the plugin's brand): header with logo, name, `version · author`, close
   X; then a 2-column tile grid, one tile per permission the plugin asks for:
   - multi-choice permissions get a **dropdown** in the value (Access: Full control / Off; Fills logins: Ask each
     time / Always allow / Off), Full control drawn amber, Ask each time in the brand colour;
   - on/off permissions get a **toggle** (Agent tool, Tab menu, Panel, and the rest of the catalog);
   - facts without a choice are plain values (Program: helper name).
3. **The user's choices are enforced.** Turning a permission off withholds it (the call answers FORBIDDEN, the same as a
   permission the plugin never asked for). Access Off stops the helper. Fills "Always allow" skips the approval for
   that plugin (it is said in the tile as amber, like Full control). Choices live in the plugin switch file next to
   the on/off, survive updates, and a new permission still turns the plugin off until approved (today's rule).
4. **Brand** in `plugin.json` (optional, protocol 2): `brand {color, icon}`: `color` an sRGB hex for buttons, focus
   ring and the header, `icon` a file in the plugin (SVG path data or the existing icon path rules). Without it Flare
   uses its own look and the chip icon. Flare draws the brand only on surfaces the plugin owns (the sheet, its
   prompts, its menu item), never on Flare's own chrome. The Bitwarden palette (navy `#1f242e`, card `#2a303c`,
   line `#353d4c`, text white, muted `#bac0ce`) is a fixed "dark brand" surface; `color` is the only input.
5. **Themed prompts.**
   - `secret.ask {title, label, sub?, button?}`: centred logo, `title` as the heading ("Your vault is locked"),
     `sub` under it (the account email), one masked field with `label` as the placeholder ("Master password") and an
     eye to show it, a full-width button with `button` ("Unlock"), Cancel as a link. Enter submits.
   - Fill approval: centred logo, "Fill <login name>?", a card with the site and "asked by the agent in <tab>", wide
     Deny / Fill, "Deny for this session" as a link.
6. **Menu.** A plugin with exactly one `tab-menu` action shows it flat in the tab menu with the plugin's logo (no
   flyout); with more, the flyout's entry carries the logo. Bitwarden ships one action whose title it sets with
   `actions.set` to "Lock Bitwarden" or "Unlock Bitwarden" by state.

## Non-goals

- No theming of Flare's own dialogs, settings page or menus beyond the plugin's own item.
- No new permission kinds. No favicon fetching (the site card shows a letter tile).

## Anti-goals

- A secret, fill or the user's choices must never reach the agent, logs, argv or tool results (unchanged rule).
- "Always allow" and Full control must never look neutral: amber, every time.

## Plan

**Flare (one PR, branch `feat/plugin-brand-sheet`, worktree `.claude\worktrees\plugrow`, shared target `target`)**
1. Manifest: `brand {color, icon}` parse and checks (`flare-mods`), docs in `plugins.md`.
2. Per-permission grants: store, enforcement in the hub's permission check, Access Off stops the helper, Fills Always
   skips the ask. Tests per permission.
3. Settings card: one row (the work started on this branch: icon buttons in `SetRowButton`, `Button.icon/tip`).
   The i opens the sheet instead of a tooltip.
4. Details sheet: Slint modal (tiles, dropdown, toggle), wired to the grants.
5. `secret.ask` and fill approval: new Slint layouts with the brand; `sub` and `button` fields.
6. Tab menu: flat single action with logo; logo on the flyout entry.
7. Tests: unit, `ui_tests`, offscreen e2e for the sheet, prompts and menu; screenshots at 100/150/225 percent.

**flare-plugins (one PR)**: `brand` with Bitwarden blue `#175DDC` and its logo (simple-icons path), one `toggle`
action titled by state, `secret.ask` with `sub` = account email and `button` = "Unlock", version 0.2.0, new zip.

Merges ask first, as always. Install on Stable only after Max says sessions are idle.

## General rules (Flare, 2026-11-08)

Everything Flare gained here is a plugin feature, never a Bitwarden one: no Bitwarden name, colour, id or special case
is in Flare's code. Another plugin gets each part by what its manifest says and the calls it makes:

- **Brand.** Any protocol 2 plugin may add `brand {color, icon?}`: a `#rrggbb` dark enough for white words (3:1) and
  an optional filled SVG logo (the icon path rules, 8 KB). Flare draws it only on the plugin's own surfaces (its card
  logo, its sheet, its questions, its tab-menu item) on one fixed dark surface; without a brand they use Flare's theme
  and the chip.
- **Details sheet.** Drawn for every plugin and protocol 1 mod from its manifest and Flare's permission catalog
  (`flare_mods::grants`): one tile per permission asked for, a dropdown for the permissions with levels (`process`,
  `input`: Full control / Off; `fill`: Ask each time / Always allow / Off), a toggle for every other one, what each
  covers under it (tools by name, menu items, panel title, sections), then the facts (program, homepage) and the
  warnings. A long list wraps into more rows. A plugin asks for nothing extra to get it.
- **Choices.** The person may turn any asked permission off; it leaves the live grant (answered `forbidden` like one
  never asked for). They are kept with the switch, survive updates, and never widen the grant. "Always allow" applies
  to any plugin's fills.
- **Questions.** Any plugin's `secret.ask` may add `sub` and `button`; every fill approval names the login, the site
  and the tab, centred in the plugin's look, with Flare's own line naming the plugin.
- **Tab menu.** Any plugin with exactly one `tab-menu` action gets it flat with its logo; with more, a flyout whose
  entry wears the logo. It titles the item by state with `actions.set`.

Flare's surfaces and the contract: `docs/surfaces/plugins.md` ("Brand", "The details sheet", "Filling logins").
Screenshots (offscreen e2e `pluginBrand`, 100 and 150 percent): `docs/screenshots/plugin-brand/`.
