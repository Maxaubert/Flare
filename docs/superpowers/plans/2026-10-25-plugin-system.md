# Flare plugins: implementation plan

Design: `../specs/2026-10-25-plugin-system-design.md`. One PR per phase, each merged on the owner's yes, in order.
Every phase keeps protocol 1 mods, the e2e `modsSample` and the existing tests green, and adds tests for what it adds.
Rules that hold throughout: no UI-thread I/O (rule 34), every animation through `Motion.d`, unknown manifest keys
refused, CRLF files keep their endings, `cargo fmt`, `cargo clippy --all-targets` clean.

## Phase 1: tab badges and progress for Claude's own tab (no plugins yet)

Gives call-me's phone badge and a workflow's progress bar right away, and builds the drawing every later phase uses.

Design section 13 overrides details below where they differ (it was written after the review).

1. `flare-tabs`: a `Decor` model per tab: badges keyed `(source, key)` with icon, tip and tone (agent slot reserved,
   at most 4 keys per source per tab), and one `progress: Option<{source, key, value: Option<f32>, label}>`. Pure,
   unit tested: replace by key, clear by key, clear by source, caps, which chips are drawn for a tab width.
   (Icon override, effects and plugin path icons wait for the plugin phases.)
2. Icons: extend the existing glyph table `menus_ui::glyph_path` (`crates/flare/src/menus_ui.rs`) and `glyphs.slint`
   with the badge set on the 24-unit box (phone, globe, bell, mic, eye, moon, lock, bolt, star, plus, pin; no check or
   dot), with its pin test.
3. Slint: `TabRow` gains FLAT fields (no nested model, so `sync_rows`' `!=` diff stays cheap): `chip1-cmd`,
   `chip1-tone`, `chip2-cmd`, `chip2-tone`, `chip-count`, `prog` (-1 none, 0..1, -2 unknown drawn static dotted), and
   the badge text merged into the tab's tooltip. Strip tab: chips in an outlined pill before `TabEnd`, the name width
   reduced by what is drawn, 2 chips at 150 px or more, 1 below, none under 110 px or on icon tabs. Sidebar row: chips
   left of the ring. Progress is a pill chip, never at the tab foot (the agent `MarkBar`). No animation. Update every
   `TabRow { .. }` literal (tabs_ui.rs and the ui tests).
4. Channel: `Namespace::Decor` with methods `tabs.badge.set`, `tabs.badge.clear`, `tabs.progress.set`,
   `tabs.progress.clear` (protocol v1, additive). `Method::ALL` grows; the prefix test gets `Decor => "tabs"`; a
   `Permission::Decorate` exists now (mods cannot ask for it yet: `ASKABLE` unchanged), the agent's
   `permission_names` gains `decorate` (update the round-trip and hello tests). The agent may call them for its own
   tab only. Add the methods to `capabilities()`.
5. `ctl_app.rs`: parse and check params on the worker (key `a-z0-9-` 1 to 24, tip through the manifest's
   `clean_text`, 80 chars, icon in the built-in table, value 0..1), rate limit on the worker (10/s per tab per source)
   before posting; new `CtlOp` variants with their `Debug` arms and both appctl matches; AppCtl applies to a
   `decor: HashMap<tab, Decor>` and refreshes.
6. Clear agent decor: AgentHub reports "session changed for tab X" (Some(a) to Some(b)) and "agent left tab X";
   AppCtl clears the agent source on those and on tab close.
7. `flare-mcp`: tools `set_tab_badge {key, icon, tip?}`, `clear_tab_badge {key}`, `set_tab_progress {value?, label?}`,
   `clear_tab_progress` (icon enum of the built-ins), and the pinned tools list updated.
8. Tests: decor model units; grant table incl. the exhaustive "no permission plugin may call only hello and tabs.list";
   ctl dispatch (own tab only, bad key, bad icon, cleaned tip, rate limit); an appctl test from the channel op to the
   drawn row; Slint ui tests that chips draw, the name narrows, chips hide on narrow and icon tabs, and that a chip
   never overlaps the `MarkBar`; MCP list and call against a fake channel; the session-change and agent-exit clears.
   Docs: `docs/surfaces/protocol.md`, `docs/features.md`.
9. Live check after install, in a NEW Claude session (a running one keeps its tool list): `set_tab_badge phone`,
   `set_tab_progress 0.5`, both visible in the strip and the sidebar.

## Phase 2: protocol 2 manifest, permissions, Settings > Plugins

1. `flare-mods`: `plugin.json` with `protocol: 2` and the new keys (`author`, `homepage`, `background`, `bar`,
   `contributes{sidebar, actions, icons}`, `helper`); protocol 1 unchanged. Every new key checked like the old ones,
   with one-sentence refusals and tests. Read `plugins\` and `mods\`; ids unique across both.
2. Permissions: add `tabs`, `decorate`, `sidebar`, `actions`, `bar`, `sessions`, `input`, `network`, `process` to the
   manifest's askable list and to `flare-ctl` `Permission`; grant rules per section 7 of the design (each new
   namespace maps to its permission; `tabs` widens the tab scope).
3. The switch store stays `mods.json` (same shape; protocol 2 entries added, plus author, homepage, source and hash
   with the approval), so the e2e `modsSample` and rule 212 hold and an older Flare still reads it.
4. Settings: the page keeps its key `mods` and is titled Plugins; the row label of a protocol 1 mod is unchanged;
   protocol 2 rows add author, homepage, permissions in plain words (design table), full-control
   warning for `process`, the `input`+`sessions` note.
5. Tests: manifest vectors for every new key; grant table; store migration; settings text.

## Phase 2b: views keyed by slot

`WebHost` (`flare-web/src/host.rs`) and `ModCtl` (`mods_ctl.rs`) key views and page state by tab. Re-key them by
slot `{owner, kind}` (kind: panel, bar, background) with generations, keeping every current behaviour (the panel per
tab, protocol 1 routing, the e2e `modsSample`). One sealing function for plugin views (design 13), and the plugin
WebView2 environment apart from the web panel and mini-apps. Tests: the existing web and mod tests unchanged, plus a
view-kind wall test.

## Phase 2c: one plugins folder

Asked for by the owner after 2 and 2b; it amends design 13 ("Compatibility"): there is one folder, `<profile>\plugins\`,
for protocol 1 and 2 alike, and `mods\` is no longer read. On start, before the first scan and on its worker, every
folder of `mods\` whose name is a legal id and that `plugins\` does not hold is moved there (a rename); a name already
in `plugins\` stays and is named in Settings in one sentence; an emptied `mods\` is removed. Storage (`flare-mod-<id>`)
and the switch store (`mods.json`) are by id and unchanged. Settings: one "Open plugins folder" row. The e2e
`modsSample` writes the sample into `plugins\`. Tests: the move (moved, clash kept and reported, empty `mods\` removed,
a non-id folder left alone, nothing moved twice), discovery from `plugins\` only, the e2e scenario compiling.

## Phase 3: background pages, events, decorate any tab

1. A background view per enabled plugin with `background` (hidden WebView2 in the plugin's profile, not tied to a
   tab), cap 6, restart policy (3 in 5 minutes), "stopped" state in Settings.
2. Bridge: `on()` accepts `tabs`, `agent`, `plugin` (and later names), one handler each; the host pushes events to
   every page of the plugin that registered. Tool calls may be answered by the background page.
3. Tab scope: a plugin call with `tab` acts on that tab if it holds `tabs`, else only the bound tab (the background
   page has no bound tab: it needs `tabs` to name one, else `forbidden`).
4. Decorate for plugins (`decorate`): the Phase 1 methods with `source = plugin:<id>`, plugin icons from
   `contributes.icons`; all of a plugin's decor cleared when it stops.
5. Events from the app: tab opened/closed/focused/renamed/moved, agent state changes (from the hub), plugin on/off.
6. Tests: background lifecycle with a fake web host; event fan-out and permission filtering; decorate scope.

## Phase 4: sidebar sections and actions

1. Sidebar: a section list under the tab list in vertical mode (collapsible, user order stored), rows from
   `sidebar.set` (cap 50, text caps), checkbox and click events (`sidebar.item`), jump-to-tab items. Horizontal mode:
   a "Plugins" top-bar button with the same lists.
2. Actions: `tab-menu` items in the tab right-click menu, `tab-hover` button, `top-bar` button (caps per design),
   `actions.set` state per tab, click events to the background page or helper.
3. Tests: Slint ui tests for the section and buttons, appctl tests for events and caps.

## Phase 5: sessions and input

1. `sessions`: a per-tab transcript follower (reuse `flare-convo` tail) started only while a plugin with `sessions`
   listens; `session` events (role, text capped, tool name), `session.read` for the last 200 entries; rate cap 50 a
   second with a dropped count.
2. `input`: `tabs.input {tab, text, enter?}` through the same path as a paste (bracketed paste rules), text caps,
   control characters refused.
3. `storage.get/set` per plugin (1 MB JSON, atomic, off the UI thread).
4. Tests: follower start and stop with listeners, event shape, input path, storage limits.

Built 2026-10-26, with these calls by the builder (the owner may overrule):
- Session text is cut at 2000 characters (design 6 says 16 KB): a plugin reads a session, it does not mirror it, and
  50 events a second of 16 KB is 800 KB a second through the page bridge. `session.read` reads the transcript's last
  4 MB. Tool results, thinking, `isMeta` notes and subagent lines are not entries.
- `storage.*` needs no permission (design 7) and so a plugin with none reaches `hello`, own-tab `tabs.list` and its own
  store; design 13's exhaustive test now pins those four methods. It is kept to protocol 2 plugins (a protocol 1 mod
  keeps exactly what it had) and closed to the agent. The file is `<profile>\plugins-storage\<id>.json`.
- `tabs.input` lets a tab and line breaks through (design 7 said a tab only): a newline goes as a paste carries it,
  which is rule 1's set. Its Enter is written 300 ms after the paste (the checked send's wait), checked again then.

## Phase 6: session bar, network, audio

1. Bar slot under a tab's terminal: `bar.show/hide`, height from the manifest, terminal reflows, one per tab, `busy`.
2. `network`: a networked mini-app profile for the plugin's pages; CSP with https `connect-src`, `img-src`,
   `media-src`; still no navigation away and no new windows.
3. `show_audio` (agent and `view.audio` for plugins): an audio player page in the panel.
4. Tests: bar layout and reflow, CSP per permission, audio view.

Built 2026-10-26, with these calls by the builder (the owner may overrule):
- No pin yet, so a second plugin asking for a tab's bar is always `busy` (design 5.5's "unless the user pinned it").
- The bar's page hides under a Flare overlay instead of showing a still, and a click that focuses it hands the keyboard
  back to the terminal at once. At most two bar views are kept a plugin; an older one behind is made again later.
- `session-bar` actions are Flare's chips at the bar row's right end, at most 4.
- A plugin with `network` has its profile in a browser folder of its own (`WebView2-plugin-net`): an update that adds or
  drops `network` starts its page storage empty. Local hosts are refused by the request wall and the resolver; a public
  name that resolves to a private address is left to the browser.
- `view.audio` copies a file beside Flare's player page (100 MB at most); an address needs `network` from a plugin and
  plays whatever the mini-app network switch says (the page is Flare's, its policy lets in one origin's media).

## Phase 7: helper process, install and update

1. Helper: start `helper.command` from inside the plugin folder (path checks as entries), in a job object, env
   `FLARE_PLUGIN_TOKEN` and `FLARE_CTL_PIPE`, logs rotated, restart policy; channel notifications for events.
2. Install from zip and from an https URL (download on a worker, limits, safe extraction, manifest checked first,
   installed off); update (stop, replace, permission check); remove (stop, Recycle Bin, forget approval and storage).
3. Settings buttons: Install from file, Install from URL, Update, Remove.
4. Tests: zip limits and traversal refusals, URL scheme refusals, update with more permissions turns off, helper env
   and job.

## Phase 8: samples, call-me, docs

1. `sample-plugins/`: `todo`, `browser-badge`, `what-next`, `sonara-bar` (UI with a fake source), `session-buttons`,
   `look`; each loads in the tests as a valid plugin.
2. The call-me skill: set and clear the agent badge (outside the repo: `~/.claude/skills/call-me/SKILL.md`).
3. `docs/surfaces/plugins.md` (the contract, replacing mods.md, which keeps a pointer), features, README section.

## Order and checkpoints

Phases 1 to 8 in order; each ends with a PR, a merge question, a release build and an install, and a live check of
what it adds. A phase that grows past its scope is split, not stretched.
