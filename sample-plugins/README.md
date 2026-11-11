# Sample plugins

Protocol 2 plugins (`plugin.json`) for authors to copy. Each folder here is loaded as a valid plugin by the
`flare-mods` tests (`discover::tests::the_sample_plugins_are_valid`), so a sample that breaks a manifest rule fails the
build, and each is turned on and driven through one round trip of its protocol by
`appctl::tests::mods_tests::plugin_samples` on Windows and on a Mac. To try one, copy its folder into
`%APPDATA%\Flare\plugins\` (Windows) or `~/Library/Application Support/Flare/plugins/` (macOS) and turn it on in
Settings > Plugins. They are pages only, so nothing in them differs by system. A plugin with a helper program names it
per system: `helper` (or `helpers.windows`) for the Windows `.exe`, `helpers.macos` for the Mac program
(`docs/surfaces/plugins.md`, "The manifest").

- `manifest-tour/`: uses every manifest key a page-only plugin can (author, homepage, background, panel, bar,
  contributes with a sidebar section, two actions and an icon of its own, and a tool). Its background page answers the
  `add_item` tool. The sidebar section and the actions are drawn; the bar is declared but nothing calls `bar.show`
  (`sonara-bar` shows one).
- `browser-badge/`: a background page only, with `tabs` and `decorate`. It hears every `tabs` event and puts a globe
  badge on a tab while that tab's browser panel is open (the tab's `panel` is `browser`), and takes it off when the
  panel closes.
- `todo/`: a background page with `tabs`, `sidebar`, `actions` and `tools` (plugins phase 4). Its sidebar section
  "To-dos" lists things to check per session tab, each with a checkbox and its tab's title; a click brings that tab to
  the front. Claude adds one with the `add_todo` tool (for the tab it runs in, unless it names another) and ticks it
  off with `todo_done`; ticking the box yourself does the same, and a ticked to-do leaves the list after a moment. A
  tab's menu has "Add to-do". The list is kept in the page's own `localStorage`.

- `what-next/`: a background page with `sidebar` and `tools`. Its section "Next up" lists what is left to do, such as a
  repository's next GitHub issues. As shipped it has no network: Claude fills it with `set_next {source?, items}` (for
  example from what `gh issue list` printed). The other path: set `REPO` in `background.html` and add `network` to the
  permissions, and the page fetches the open issues from `api.github.com` itself every 10 minutes; Flare asks you again
  for the new permission ("Use the internet"). The list is kept in the plugin's own store (`storage.*`).
- `sonara-bar/`: a bar under a tab's terminal with `bar`, `actions` and `tools`: play and pause, a seek slider and the
  title in the bar's page, and back and skip as `session-bar` actions (chips Flare draws, sent to the background page
  as `action`). The source is FAKE: nothing plays and nothing talks to Sonara; Claude says what "plays" with
  `sonara_now {title, pos, len, playing}`, kept in the plugin's store, which the bar reads once a second and writes on a
  seek. Open it from a tab's menu ("Show the player"). A real player would add a helper program (`process`).
- `session-buttons/`: `tabs`, `actions`, `input` and `bar`. "Compact" (the tab's hover button, its menu and the bar)
  types `/compact` with `tabs.input` and presses Enter; "Tell it to carry on" types one preset line (change it at the
  top of `background.html`). **`input` is full control:** a plugin with it can type and run any command in your
  terminals, and Settings says so. This one types only into tabs where Claude runs (that is what it reads `tabs` for).
- `look/`: `tabs`, `decorate` and `tools`. A star badge marks a tab whose agent stopped working while another tab was
  in front, until you bring that tab back. Claude's `set_steps {steps: [{name, weight?, done}]}` puts a weighted
  progress chip on its tab: the weights done over all the weights, so 9 of 10 steps done reads 50 % when the last one
  is half the work. (A tab's own icon and effects, `tabs.icon.*` and `tabs.effect.*` in the design, are not built.)
- `agent-moves/`: `agent-looks`, `panel` and `actions`. The agents' tab icons move while they work: Claude hops and
  blinks (`hop-blink`), Codex turns while it fades (`spin-fade`), Antigravity's rainbow flows along its arch
  (`rainbow-flow`), set with `agents.motion.set` when its background page starts. A tab's menu has "Agent motions",
  a panel with one picker per agent (kept in the plugin's store). Turning it off makes the icons still again; so does
  Windows' "animation effects" switch, or "Reduce motion" on a Mac. On a Mac: copy the folder to
  `~/Library/Application Support/Flare/plugins/agent-moves/`, or zip it (`cd sample-plugins && zip -r
  agent-moves.zip agent-moves`) and use Settings > Plugins > "Install from file"; then turn it on there.

The contract is `docs/surfaces/plugins.md`; the protocol 1 sample mod is `docs/surfaces/sample-mod/`.
