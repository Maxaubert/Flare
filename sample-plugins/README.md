# Sample plugins

Protocol 2 plugins (`plugin.json`) for authors to copy. Each folder here is loaded as a valid plugin by the
`flare-mods` tests (`discover::tests::the_sample_plugins_are_valid`), so a sample that breaks a manifest rule fails the
build. To try one, copy its folder into `%APPDATA%\Flare\plugins\` and turn it on in Settings > Plugins.

- `manifest-tour/`: uses every manifest key a page-only plugin can (author, homepage, background, panel, bar,
  contributes with a sidebar section, two actions and an icon of its own, and a tool). Its background page answers the
  `add_item` tool. The bar is checked now and runs in a later version; the sidebar section and the actions are drawn.
- `browser-badge/`: a background page only, with `tabs` and `decorate`. It hears every `tabs` event and puts a globe
  badge on a tab while that tab's browser panel is open (the tab's `panel` is `browser`), and takes it off when the
  panel closes.
- `todo/`: a background page with `tabs`, `sidebar`, `actions` and `tools` (plugins phase 4). Its sidebar section
  "To-dos" lists things to check per session tab, each with a checkbox and its tab's title; a click brings that tab to
  the front. Claude adds one with the `add_todo` tool (for the tab it runs in, unless it names another) and ticks it
  off with `todo_done`; ticking the box yourself does the same, and a ticked to-do leaves the list after a moment. A
  tab's menu has "Add to-do". The list is kept in the page's own `localStorage`.

The contract is `docs/surfaces/mods.md` ("Protocol 2"); the protocol 1 sample mod is `docs/surfaces/sample-mod/`.
