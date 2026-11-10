# Plan: browser panel pop-ups, app links and logging

Spec: `docs/superpowers/specs/2026-11-10-web-popups-design.md`. Three PRs, one at a time, logging first so the other two
can use it. Each: worktree, tests first for the pure parts, gate, independent review, PR, CI against main's known failures.

## 1. Logging (`feat/web-log`)
1. `flare-surfaces/web.rs`: `site_of` (host only, never the raw string), `web_error_name`, permission and process words; tests.
2. `flare-web/backend.rs`: `WebEvent::Note` carrying sanitised fields; fake backend.
3. `flare-web/host.rs`: emit at new-window, navigation cancel, load failure, process failure, close request, permission,
   certificate error (log only, never `SetAction`); degrade when a newer interface is missing.
4. `flare-diag::web_diag` with the rate gate; `web_ctl.rs` maps events to lines; reader groups.
5. `neutral.rs` text keys, `privacy.rs`, `PRIVACY.md`, `docs/diagnostics.md`, rule 110.

## 2. Pop-ups (`feat/web-popups`)
1. `flare-surfaces/web.rs`: pure `popup_plan` (in place, window, refuse) with limits; tests.
2. `flare-web/host.rs`: deferral, a second controller in the main window at the panel's bounds on the same environment,
   `SetNewWindow`, `configure_popup`, the opener hidden but alive, a stack per tab, close paths, bounds follow the panel;
   every path completes the deferral.
3. `ui/panel.slint`: the "Pop-up from <site>" strip with Close, in Flare's look; `web_ctl.rs` reserves its height.
3. `WebEvent::Popups`; `web_ctl.rs` state; agent input tools pause; `browser_state` line.
4. e2e: a trusted click opens exactly one window that closes on `window.close()`; a named form post arrives as a POST.
   Hidden or off-screen only. Rule 189, `docs/e2e.md`, `docs/features.md`, `docs/surfaces/README.md` parity note.

## 3. App links (`feat/web-applinks`)
1. Probe: does `NavigationStarting` fire for a custom scheme? Wire through it, `LaunchingExternalUriScheme`, or both.
2. `flare-surfaces/web.rs`: `external_scheme` classifier with the never list as data; tests.
3. `app_ask.rs` dialog modelled on `download_ask.rs`, Cancel default, 0.3 s guard, limits, agent suppression.
4. `flare_win::shell_open::open_app_uri` on a bare link after the person's Open.
5. Same-site `blob:` for a page's own content; rule and docs.
