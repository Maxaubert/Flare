# M6 screenshots: the marks at every scaling (Task 34)

Made by `flare.exe --scene <name> --scale <s> --size 1000x640 --renderer software --screenshot <name>-<pct>.png`
(release build) for the eleven mark scenes (`marks-ring`, `marks-bar`, `marks-bar-flow`, `marks-sidebar`,
`marks-sidebar-bar`, `marks-icons`, `marks-overflow`, `marks-tip`, `marks-skeleton`, `marks-close`, `badge`) at
100, 125, 150, 175, 200, 225, 250 and 300 percent (88 images), plus `--renderer gpu` for `marks-ring`, `marks-bar`
and `marks-sidebar` at 100, 150, 225 and 300 (`<name>-gpu-<pct>.png`, 12 images). `marks-overflow` is 700 px wide.
`badge` is not a window: one sheet of the taskbar disc for counts 1 to 9 and `9+` (columns) at the eight scalings'
sizes 16 to 48 px (rows), each magnified 4x on `#1f1f1f`; the eight `badge-<pct>.png` files are the same sheet.

## How they were reviewed (2026-10-17)

- Read with the image reader, whole and cropped at 2x to 10x: the strip's ring slots (spinner, dot, check, cross,
  loading ring) at 100, 125, 175 and 250; the first three tabs' feet on Bar at 125, 175 and 250; the icons (Clawd,
  OpenAI, prompt, cog) at 100, 125, 175 and 250; the sidebar at 175; every other scene at one or two scales.
- Measured with scripts at every scale: the Bar band thickness (`round(3 x scale)`, the Task 33 test), the rainbow's
  largest step between neighbouring pixels (no seam), ground pixels inside a line (none after the fix), the skeleton's
  pitch (11 px lines, 21 px line pitch, 37 px paragraph pitch, 16 px inset, the 9% logo column), the badge digits'
  ink centre (within 0.5 px after the fix), software against GPU (no pixel more than 1 unit apart).
- Against Prism Terminal's source (`core/renderer/components/TabMark.tsx`, `styles/marks.css`, `AgentIcon.tsx`,
  `ResumeSkeleton.tsx`, `src/renderer/src/components/TabStrip.tsx`, `TabSidebar.tsx`, `tabParts.tsx`,
  `lib/taskbarBadge.ts`).

## Defects found and fixed

Each got a failing test first (it failed on the old build for the reason shown), then the fix, then a re-shoot.

| defect | test first | fix |
|---|---|---|
| The front working tab showed its spinner AND the close X (Prism Terminal hides the X beside a ring until hover); the scene pinned the motion before the X's 150 ms slide-out ran | `headless::tests::a_ringed_front_tab_shows_its_ring_and_no_close_at_every_scale` | the mark scenes let the clock run 200 ms before pinning the phase |
| The Codex tab wore Clawd (the hook signal arrived before any poll said Codex) and `marks-icons` had no shell tab: the OpenAI mark and the prompt were never shown | `headless::tests::the_icons_scene_shows_clawd_openai_and_the_prompt` | the scenes answer the process poll for the Codex tab first, as the real poll does; `docs` (a shell) added to `marks-icons` |
| `marks-overflow` showed `+8` with no dots: every tab that needed you fitted as an icon | `headless::tests::the_overflow_button_shows_the_mark_colours_of_hidden_tabs` | thirteen of the sixteen tabs carry a state, so tabs that need you are hidden |
| With four dots the +N button kept the plan's 72 px and its count touched the left edge (Prism Terminal's button is sized by its content, `px-2`) | `headless::tests::the_overflow_button_keeps_its_padding_with_four_dots` (all 8 scales), `strip_plan::tests::the_more_button_grows_with_its_dots_and_never_under_72` | `strip_plan::more_button_css` and `strip_layout::layout_strip_with`: the button grows with its dots; the plan is made again with that much less room |
| `marks-close` showed the resume skeleton behind the question while the tab already worked | `headless::tests::the_close_scene_has_no_skeleton_behind_the_question` | the scene's tab is saved as a plain shell (the hook makes it Claude) |
| The sidebar's rainbow had a ground-coloured column at 175% where two of its 42 pieces met mid-pixel | `headless::tests::the_rainbow_line_has_no_gap_at_any_scale` (strip and sidebar, 8 scales) | `ui/tabmark.slint`: the ribbon and every piece start and end on whole device pixels |
| The badge digits sat 2 to 5 px below the disc's middle (GDI's `DT_VCENTER` centres the whole cell; Prism Terminal's canvas uses the em middle nudged 4%) | `taskbar::tests::the_digits_sit_centred_on_the_disc_at_every_size` | `taskbar::centre_ink_vertically` moves the rasterised ink onto the middle; the 4% nudge is gone |

## Looked at and kept

- Clawd at 100% and 125% has soft edges: the 17-unit grid does not land on whole pixels at 18 px and Slint has no
  `crispEdges` (the plan accepted this in Task 24). No gaps between its pixels at any scale.
- The sidebar's Bar line stops a whole device pixel inside the card's corner curve (flat ends) where Prism Terminal's
  rounded `overflow-hidden` card clips the line's ends to the curve: the software renderer ignores a clip's radius
  (Task 25's decision, `ui/sidebar.slint`). The difference is at most 3 css px at the line's top row.
- The `1` on the badge sits up to 2 px left of the middle: its ink is off its advance, which both apps centre.

## Side by side with Prism Terminal

`side-by-side-100.png`, `-150.png`, `-225.png`: Prism Terminal 0.58.0.0 (left, captured by `tools/pt-shot` in a
throwaway profile, offscreen) beside Flare's `marks-icons` (right); under them Prism Terminal's strip, then Flare's
`marks-ring` and `marks-bar` strips. `pt-shot` cannot drive stand-in agents, so Prism Terminal's side shows one plain
shell tab and no marks: the tab's geometry (icon at the left, name centred, X, top rule), the `+` and the title bar
match. The marks were judged against Prism Terminal's source instead (above). Owner: a side by side of the marks
themselves needs Prism Terminal with live agents; say if you want it.

## The real taskbar overlay

Not seen: it needs a visible window, which no test opens. The owner is asked at the PR to look at the badge once on
his display (a visible `flare.exe` with a stand-in agent).

## Verdicts

`read` = looked at with the image reader; `measured` = checked by script only. Every row passes after the fixes.

| scene | scale | renderer | verdict | checked | what was fixed |
|---|---|---|---|---|---|
| `marks-ring` | 100 | software | ok: 12 px spinner, crisp top arc, round 7 px dot, check and cross as heavy as the close X, 9 px loading ring; the front tab shows its spinner and no X until hovered | read | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 125 | software | ok: 12 px spinner, crisp top arc, round 7 px dot, check and cross as heavy as the close X, 9 px loading ring; the front tab shows its spinner and no X until hovered | read | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 150 | software | ok: 12 px spinner, crisp top arc, round 7 px dot, check and cross as heavy as the close X, 9 px loading ring; the front tab shows its spinner and no X until hovered | measured | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 175 | software | ok: 12 px spinner, crisp top arc, round 7 px dot, check and cross as heavy as the close X, 9 px loading ring; the front tab shows its spinner and no X until hovered | read | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 200 | software | ok: 12 px spinner, crisp top arc, round 7 px dot, check and cross as heavy as the close X, 9 px loading ring; the front tab shows its spinner and no X until hovered | measured | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 225 | software | ok: 12 px spinner, crisp top arc, round 7 px dot, check and cross as heavy as the close X, 9 px loading ring; the front tab shows its spinner and no X until hovered | measured | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 250 | software | ok: 12 px spinner, crisp top arc, round 7 px dot, check and cross as heavy as the close X, 9 px loading ring; the front tab shows its spinner and no X until hovered | read | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 300 | software | ok: 12 px spinner, crisp top arc, round 7 px dot, check and cross as heavy as the close X, 9 px loading ring; the front tab shows its spinner and no X until hovered | measured | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 100 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 150 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 225 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured | the front tab's close X no longer shows beside its ring |
| `marks-ring` | 300 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured | the front tab's close X no longer shows beside its ring |
| `marks-bar` | 100 | software | ok: lines round(3 x scale) device px on every marked tab, flush with the tab edges, the divider drawn over the line; run bar rounded and 42% wide; rainbow smooth; top rule and foot line on the front tab | read |  |
| `marks-bar` | 125 | software | ok: lines round(3 x scale) device px on every marked tab, flush with the tab edges, the divider drawn over the line; run bar rounded and 42% wide; rainbow smooth; top rule and foot line on the front tab | read |  |
| `marks-bar` | 150 | software | ok: lines round(3 x scale) device px on every marked tab, flush with the tab edges, the divider drawn over the line; run bar rounded and 42% wide; rainbow smooth; top rule and foot line on the front tab | measured |  |
| `marks-bar` | 175 | software | ok: lines round(3 x scale) device px on every marked tab, flush with the tab edges, the divider drawn over the line; run bar rounded and 42% wide; rainbow smooth; top rule and foot line on the front tab | read |  |
| `marks-bar` | 200 | software | ok: lines round(3 x scale) device px on every marked tab, flush with the tab edges, the divider drawn over the line; run bar rounded and 42% wide; rainbow smooth; top rule and foot line on the front tab | measured |  |
| `marks-bar` | 225 | software | ok: lines round(3 x scale) device px on every marked tab, flush with the tab edges, the divider drawn over the line; run bar rounded and 42% wide; rainbow smooth; top rule and foot line on the front tab | measured |  |
| `marks-bar` | 250 | software | ok: lines round(3 x scale) device px on every marked tab, flush with the tab edges, the divider drawn over the line; run bar rounded and 42% wide; rainbow smooth; top rule and foot line on the front tab | read |  |
| `marks-bar` | 300 | software | ok: lines round(3 x scale) device px on every marked tab, flush with the tab edges, the divider drawn over the line; run bar rounded and 42% wide; rainbow smooth; top rule and foot line on the front tab | measured |  |
| `marks-bar` | 100 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured |  |
| `marks-bar` | 150 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured |  |
| `marks-bar` | 225 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured |  |
| `marks-bar` | 300 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured |  |
| `marks-bar-flow` | 100 | software | ok: the rainbow half a tile on, no seam where the tiles meet, nothing else moved | measured |  |
| `marks-bar-flow` | 125 | software | ok: the rainbow half a tile on, no seam where the tiles meet, nothing else moved | measured |  |
| `marks-bar-flow` | 150 | software | ok: the rainbow half a tile on, no seam where the tiles meet, nothing else moved | measured |  |
| `marks-bar-flow` | 175 | software | ok: the rainbow half a tile on, no seam where the tiles meet, nothing else moved | read |  |
| `marks-bar-flow` | 200 | software | ok: the rainbow half a tile on, no seam where the tiles meet, nothing else moved | measured |  |
| `marks-bar-flow` | 225 | software | ok: the rainbow half a tile on, no seam where the tiles meet, nothing else moved | measured |  |
| `marks-bar-flow` | 250 | software | ok: the rainbow half a tile on, no seam where the tiles meet, nothing else moved | measured |  |
| `marks-bar-flow` | 300 | software | ok: the rainbow half a tile on, no seam where the tiles meet, nothing else moved | measured |  |
| `marks-sidebar` | 100 | software | ok: Clawd, the OpenAI mark, the prompt, the cog; spinner, dot, check, cross in the 18 px end box | read | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 125 | software | ok: Clawd, the OpenAI mark, the prompt, the cog; spinner, dot, check, cross in the 18 px end box | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 150 | software | ok: Clawd, the OpenAI mark, the prompt, the cog; spinner, dot, check, cross in the 18 px end box | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 175 | software | ok: Clawd, the OpenAI mark, the prompt, the cog; spinner, dot, check, cross in the 18 px end box | read | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 200 | software | ok: Clawd, the OpenAI mark, the prompt, the cog; spinner, dot, check, cross in the 18 px end box | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 225 | software | ok: Clawd, the OpenAI mark, the prompt, the cog; spinner, dot, check, cross in the 18 px end box | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 250 | software | ok: Clawd, the OpenAI mark, the prompt, the cog; spinner, dot, check, cross in the 18 px end box | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 300 | software | ok: Clawd, the OpenAI mark, the prompt, the cog; spinner, dot, check, cross in the 18 px end box | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 100 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 150 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 225 | gpu | ok: identical to software (no pixel more than 1 unit apart) | read | the Codex tab wears the OpenAI mark |
| `marks-sidebar` | 300 | gpu | ok: identical to software (no pixel more than 1 unit apart) | measured | the Codex tab wears the OpenAI mark |
| `marks-sidebar-bar` | 100 | software | ok: lines inside the rounded cards, rainbow without gaps (fixed), run bar 42% | measured | the Codex tab wears the OpenAI mark; no ground column in the rainbow (175) |
| `marks-sidebar-bar` | 125 | software | ok: lines inside the rounded cards, rainbow without gaps (fixed), run bar 42% | measured | the Codex tab wears the OpenAI mark; no ground column in the rainbow (175) |
| `marks-sidebar-bar` | 150 | software | ok: lines inside the rounded cards, rainbow without gaps (fixed), run bar 42% | measured | the Codex tab wears the OpenAI mark; no ground column in the rainbow (175) |
| `marks-sidebar-bar` | 175 | software | ok: lines inside the rounded cards, rainbow without gaps (fixed), run bar 42% | read | the Codex tab wears the OpenAI mark; no ground column in the rainbow (175) |
| `marks-sidebar-bar` | 200 | software | ok: lines inside the rounded cards, rainbow without gaps (fixed), run bar 42% | measured | the Codex tab wears the OpenAI mark; no ground column in the rainbow (175) |
| `marks-sidebar-bar` | 225 | software | ok: lines inside the rounded cards, rainbow without gaps (fixed), run bar 42% | measured | the Codex tab wears the OpenAI mark; no ground column in the rainbow (175) |
| `marks-sidebar-bar` | 250 | software | ok: lines inside the rounded cards, rainbow without gaps (fixed), run bar 42% | measured | the Codex tab wears the OpenAI mark; no ground column in the rainbow (175) |
| `marks-sidebar-bar` | 300 | software | ok: lines inside the rounded cards, rainbow without gaps (fixed), run bar 42% | measured | the Codex tab wears the OpenAI mark; no ground column in the rainbow (175) |
| `marks-icons` | 100 | software | ok: Clawd, OpenAI (Codex tab), prompt (shell tab), cog (Settings) crisp in the ink; Clawd orange exact | read | the Codex tab wears the OpenAI mark; a shell tab added |
| `marks-icons` | 125 | software | ok: Clawd, OpenAI (Codex tab), prompt (shell tab), cog (Settings) crisp in the ink; Clawd orange exact | read | the Codex tab wears the OpenAI mark; a shell tab added |
| `marks-icons` | 150 | software | ok: Clawd, OpenAI (Codex tab), prompt (shell tab), cog (Settings) crisp in the ink; Clawd orange exact | read | the Codex tab wears the OpenAI mark; a shell tab added |
| `marks-icons` | 175 | software | ok: Clawd, OpenAI (Codex tab), prompt (shell tab), cog (Settings) crisp in the ink; Clawd orange exact | read | the Codex tab wears the OpenAI mark; a shell tab added |
| `marks-icons` | 200 | software | ok: Clawd, OpenAI (Codex tab), prompt (shell tab), cog (Settings) crisp in the ink; Clawd orange exact | measured | the Codex tab wears the OpenAI mark; a shell tab added |
| `marks-icons` | 225 | software | ok: Clawd, OpenAI (Codex tab), prompt (shell tab), cog (Settings) crisp in the ink; Clawd orange exact | read | the Codex tab wears the OpenAI mark; a shell tab added |
| `marks-icons` | 250 | software | ok: Clawd, OpenAI (Codex tab), prompt (shell tab), cog (Settings) crisp in the ink; Clawd orange exact | read | the Codex tab wears the OpenAI mark; a shell tab added |
| `marks-icons` | 300 | software | ok: Clawd, OpenAI (Codex tab), prompt (shell tab), cog (Settings) crisp in the ink; Clawd orange exact | measured | the Codex tab wears the OpenAI mark; a shell tab added |
| `marks-overflow` | 100 | software | ok: icon tabs with corner marks, `+8` with four round dots in the mark colours, the list with Needs you rows and rings | read | dots shown; the +N button grows to keep its padding |
| `marks-overflow` | 125 | software | ok: icon tabs with corner marks, `+8` with four round dots in the mark colours, the list with Needs you rows and rings | measured | dots shown; the +N button grows to keep its padding |
| `marks-overflow` | 150 | software | ok: icon tabs with corner marks, `+8` with four round dots in the mark colours, the list with Needs you rows and rings | read | dots shown; the +N button grows to keep its padding |
| `marks-overflow` | 175 | software | ok: icon tabs with corner marks, `+8` with four round dots in the mark colours, the list with Needs you rows and rings | measured | dots shown; the +N button grows to keep its padding |
| `marks-overflow` | 200 | software | ok: icon tabs with corner marks, `+8` with four round dots in the mark colours, the list with Needs you rows and rings | measured | dots shown; the +N button grows to keep its padding |
| `marks-overflow` | 225 | software | ok: icon tabs with corner marks, `+8` with four round dots in the mark colours, the list with Needs you rows and rings | measured | dots shown; the +N button grows to keep its padding |
| `marks-overflow` | 250 | software | ok: icon tabs with corner marks, `+8` with four round dots in the mark colours, the list with Needs you rows and rings | read | dots shown; the +N button grows to keep its padding |
| `marks-overflow` | 300 | software | ok: icon tabs with corner marks, `+8` with four round dots in the mark colours, the list with Needs you rows and rings | measured | dots shown; the +N button grows to keep its padding |
| `marks-tip` | 100 | software | ok: two lines, centred under the failed tab, the cross ring and the slid-in X on the hovered tab | measured |  |
| `marks-tip` | 125 | software | ok: two lines, centred under the failed tab, the cross ring and the slid-in X on the hovered tab | measured |  |
| `marks-tip` | 150 | software | ok: two lines, centred under the failed tab, the cross ring and the slid-in X on the hovered tab | read |  |
| `marks-tip` | 175 | software | ok: two lines, centred under the failed tab, the cross ring and the slid-in X on the hovered tab | measured |  |
| `marks-tip` | 200 | software | ok: two lines, centred under the failed tab, the cross ring and the slid-in X on the hovered tab | measured |  |
| `marks-tip` | 225 | software | ok: two lines, centred under the failed tab, the cross ring and the slid-in X on the hovered tab | measured |  |
| `marks-tip` | 250 | software | ok: two lines, centred under the failed tab, the cross ring and the slid-in X on the hovered tab | measured |  |
| `marks-tip` | 300 | software | ok: two lines, centred under the failed tab, the cross ring and the slid-in X on the hovered tab | read |  |
| `marks-skeleton` | 100 | software | ok: 11 px lines, 10 px gaps, 26 px between paragraphs, the logo block header; loading ring on both tabs | measured |  |
| `marks-skeleton` | 125 | software | ok: 11 px lines, 10 px gaps, 26 px between paragraphs, the logo block header; loading ring on both tabs | measured |  |
| `marks-skeleton` | 150 | software | ok: 11 px lines, 10 px gaps, 26 px between paragraphs, the logo block header; loading ring on both tabs | read |  |
| `marks-skeleton` | 175 | software | ok: 11 px lines, 10 px gaps, 26 px between paragraphs, the logo block header; loading ring on both tabs | measured |  |
| `marks-skeleton` | 200 | software | ok: 11 px lines, 10 px gaps, 26 px between paragraphs, the logo block header; loading ring on both tabs | measured |  |
| `marks-skeleton` | 225 | software | ok: 11 px lines, 10 px gaps, 26 px between paragraphs, the logo block header; loading ring on both tabs | measured |  |
| `marks-skeleton` | 250 | software | ok: 11 px lines, 10 px gaps, 26 px between paragraphs, the logo block header; loading ring on both tabs | read |  |
| `marks-skeleton` | 300 | software | ok: 11 px lines, 10 px gaps, 26 px between paragraphs, the logo block header; loading ring on both tabs | measured |  |
| `marks-close` | 100 | software | ok: `Stop the agent and close?`, Claude, `working for 2 minutes`, Prism; the shell behind | measured | no resume skeleton behind the question |
| `marks-close` | 125 | software | ok: `Stop the agent and close?`, Claude, `working for 2 minutes`, Prism; the shell behind | read | no resume skeleton behind the question |
| `marks-close` | 150 | software | ok: `Stop the agent and close?`, Claude, `working for 2 minutes`, Prism; the shell behind | read | no resume skeleton behind the question |
| `marks-close` | 175 | software | ok: `Stop the agent and close?`, Claude, `working for 2 minutes`, Prism; the shell behind | measured | no resume skeleton behind the question |
| `marks-close` | 200 | software | ok: `Stop the agent and close?`, Claude, `working for 2 minutes`, Prism; the shell behind | measured | no resume skeleton behind the question |
| `marks-close` | 225 | software | ok: `Stop the agent and close?`, Claude, `working for 2 minutes`, Prism; the shell behind | measured | no resume skeleton behind the question |
| `marks-close` | 250 | software | ok: `Stop the agent and close?`, Claude, `working for 2 minutes`, Prism; the shell behind | measured | no resume skeleton behind the question |
| `marks-close` | 300 | software | ok: `Stop the agent and close?`, Claude, `working for 2 minutes`, Prism; the shell behind | measured | no resume skeleton behind the question |
| `badge` | 100 | software | ok: discs sharp, every count centred within half a pixel up and down (fixed) | read (the sheet holds all eight sizes) | digits centred (they sat 2 to 5 px low) |
| `badge` | 125 | software | ok: discs sharp, every count centred within half a pixel up and down (fixed) | same sheet as 100 | digits centred (they sat 2 to 5 px low) |
| `badge` | 150 | software | ok: discs sharp, every count centred within half a pixel up and down (fixed) | same sheet as 100 | digits centred (they sat 2 to 5 px low) |
| `badge` | 175 | software | ok: discs sharp, every count centred within half a pixel up and down (fixed) | same sheet as 100 | digits centred (they sat 2 to 5 px low) |
| `badge` | 200 | software | ok: discs sharp, every count centred within half a pixel up and down (fixed) | same sheet as 100 | digits centred (they sat 2 to 5 px low) |
| `badge` | 225 | software | ok: discs sharp, every count centred within half a pixel up and down (fixed) | same sheet as 100 | digits centred (they sat 2 to 5 px low) |
| `badge` | 250 | software | ok: discs sharp, every count centred within half a pixel up and down (fixed) | same sheet as 100 | digits centred (they sat 2 to 5 px low) |
| `badge` | 300 | software | ok: discs sharp, every count centred within half a pixel up and down (fixed) | same sheet as 100 | digits centred (they sat 2 to 5 px low) |
