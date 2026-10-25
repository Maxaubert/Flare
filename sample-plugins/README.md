# Sample plugins

Protocol 2 plugins (`plugin.json`) for authors to copy. Each folder here is loaded as a valid plugin by the
`flare-mods` tests (`discover::tests::the_sample_plugins_are_valid`), so a sample that breaks a manifest rule fails the
build. To try one, copy its folder into `%APPDATA%\Flare\plugins\` and turn it on in Settings > Plugins.

- `manifest-tour/`: uses every manifest key a page-only plugin can (author, homepage, background, panel, bar,
  contributes with a sidebar section, two actions and an icon of its own, and a tool). Its background page answers the
  `add_item` tool. The bar, the sidebar section and the actions are checked now and run in a later version.
- `browser-badge/`: a background page only, with `tabs` and `decorate`. It hears every `tabs` event and puts a globe
  badge on a tab while that tab's browser panel is open (the tab's `panel` is `browser`), and takes it off when the
  panel closes.

The contract is `docs/surfaces/mods.md` ("Protocol 2"); the protocol 1 sample mod is `docs/surfaces/sample-mod/`.
