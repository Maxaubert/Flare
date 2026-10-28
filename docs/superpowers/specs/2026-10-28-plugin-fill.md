# Plugin fill: `fill`, `browser.fill`, `secret.ask` and tool calls for helpers (2026-10-28)

Status: built (branch `feat/plugin-fill-api`). This is Part A of the Bitwarden design, which lives with the plugin:
`Maxaubert/flare-plugins`, `docs/2026-10-28-bitwarden-design.md` (Part B there is the `bitwarden` plugin itself). This
file records what Flare built and the choices left to the builder; the contract is `../../surfaces/plugins.md` ("Filling
logins") and `../../surfaces/protocol.md` (section 5, "browser.fill, browser.fill.page, secret.ask").

## Intent

An agent driving a sign-in page in Flare's web panel can have a password manager plugin sign it in with the owner's
saved login, approved by the owner in a Flare dialog, and the user name, password and code never reach the agent (what
the agent reads leaves the PC). Flare holds no password manager: it gets a generic fill API any plugin can use.

## What Flare built

1. **Permission `fill`** (protocol 2 only, never the agent's). Words in Settings: "Fill logins into your web panel".
   Warning: "Can type passwords into pages, after you approve each one."; with `network` also "It can send the passwords
   you type for it to the internet.". Namespace `Fill` holds `browser.fill`, `browser.fill.page` and `secret.ask`.
2. **`browser.fill {tab?, login_name, login_uris, username?, password?, totp?}`**:
   - the tab: the caller's own, one it may see (`tabs` or a click there), or the tab an agent's call to this plugin came
     from while that call waits for the plugin's answer ("tool scope", new);
   - the tab's web page (not a mini-app, not a plugin's page, and up in a browser), else `not_found`;
   - the site rule (no public suffix list in Flare): the page's host is the login's host or a subdomain of it, a leading
     `www.` of the login's host left out; `https` unless the login's address says `http` (an `http` login may fill its
     `https` page); the login's port when it names one; an IP address only itself; a bare `example.com` reads as
     `https://example.com`. No match: `forbidden`, "this login is not for <host>";
   - Flare's own dialog: "<Plugin> wants to fill a login" / "Fill <login> into <host>?" with Deny for this session, Deny
     and Allow. Allow takes the pointer only: no key presses it (Enter and Space press the focused Deny button), and it
     has no accessible default action. Escape and a press outside are Deny. The web page hides under the dialog;
   - on Allow the page's site is checked again (it may have moved: `interrupted`), then one `ExecuteScript`: the values
     are one JSON text, encoded again as a JS string literal by `serde_json` and read with `JSON.parse`, so a value is
     never script text. The script checks `location` once more, finds the fields (`autocomplete` `username`, `email`,
     `current-password`, `one-time-code` first; then the focused text field, the last text field before the password, a
     field named like a user or email, and a code field named like `otp`, `2fa`, `verification`), sets each value with
     the native setter and fires `input` and `change`;
   - answers `{filled: [...], missing: [...]}` (kinds of field: `username`, `password`, `totp`), `{declined: true}`,
     `{declined: true, for_session: true}`, `{declined: true, timed_out: true}`, or an error;
   - one question at a time across plugins (`busy`), at most 6 asks a minute per plugin (fills and secret asks together,
     `rate_limited`), a minute to answer; "Deny for this session" declines that plugin's fills until Flare quits.
3. **`browser.fill.page {tab?}`** answers `{tab, url, host}`: the address the plugin picks a login for (a helper cannot
   use `panel.state`, which acts on the caller's own tab only).
4. **`secret.ask {title, label}`**: Flare's masked field titled "<Plugin>: <title>"; answers `{text}` to that plugin only,
   `{cancelled: true}`, or `{cancelled: true, timed_out: true}`.
5. **Tool calls for a helper.** A plugin with a helper and neither a background page nor a panel has its tools listed
   and answered by its helper: Flare sends the `tool` event `{call_id, tool, tab, input}` to the connections that said
   hello with the plugin's token, and the helper answers with `tools.respond`. With no helper connected, the call fails
   at once (`unavailable`). A manifest may now have tools with only a helper.
6. **Nothing logged.** The values live in `flare_mods::fill::Secret` (no `Debug` of the value, written over on drop); an
   error names a key, never a value; the one log line of a fill is `fill: plugin <id> on <host>: <outcome>` and of a
   secret `secret.ask: plugin <id>: answered|cancelled`.

## Left to the builder, and what was chosen

- Field heuristics: as above. Login forms inside frames are not reached (the script runs in the main frame).
- Several matching logins: the plugin's to pick (Part B); `browser.fill` names one login.
- Base domain: exact host or subdomain, with `www.` left out of the login's host, since Flare has no public suffix list.
- The time a person has: the dialog waits 60 s, but an agent's tool call has 30 s in all, so a plugin that fills inside
  a tool call leaves the person about 25 s. Raising the tool budget while a Flare dialog of that plugin is up is a
  possible follow-up.

## Tests

`flare_mods::fill` (site rule, rate, session deny, params, the script's data encoding, the answer filter, the log line),
`flare_ctl::grant` and `method` (the namespace and its timeouts), `flare_mods::hub` (words, warnings, tool scope),
`flare_mods::manifest` (tools with a helper), `appctl::tests::mods_tests::plugin_fill` (the whole flow with the fake
browser), `plugin_ask` (the dialog's words) and the offscreen e2e `pluginFill` (a fake helper, a local sign-in page,
the dialog pressed by the pointer, Enter that does not allow, a mismatch refused, no value in the logs or the results).
