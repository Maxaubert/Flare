# Browser panel: real pop-ups, asking before apps, and logging

Status: proposed for approval, 2026-11-10. Plan: `docs/superpowers/plans/2026-11-10-web-popups.md`.

Why: a card's 2FA check (3-D Secure) failed in the browser panel and worked in Chrome. The panel loads every pop-up into
itself, ignores `window.close()`, silently blocks links to apps, and logs nothing about pages. A certificate was not the
cause: Flare uses WebView2's default certificate handling.

Scope: the plain browser panel only. Mini-apps and plugin pages keep opening nothing.

## 1. Real pop-ups

A page's pop-up opens as a real second browser view, so the bank page keeps its link to the shop page
(`window.opener`), its window name and its form data.

**When a request opens a pop-up.** Only after a click or key by the person (WebView2's `IsUserInitiated`), as Chrome's
blocker does. And only when the request looks like a pop-up: it gives a size or position, names a window, or opens
`about:blank` for the page to write into. A plain link with `target=_blank` still opens in the panel, as today.

| Case | Result |
|---|---|
| Plain `target=_blank` link | Opens in the panel, as today |
| `window.open(url, name, "width=500,height=600")` after a click | Pop-up in the panel |
| A form posting into a named window | Pop-up in the panel, with the POST intact |
| A pop-up the page opens with no click by the person | Refused, with a "Pop-up blocked" note |
| More than 2 pop-ups for a tab, or 4 in total | Refused |

Known gap: WebView2 doesn't say whether a request is a form post. A form posting to an unnamed `_blank` still opens in
the panel as a plain page. If 2FA still fails, every new-window request after a click opens as a pop-up instead.

**Where it shows (owner, 2026-11-10: "in the side panel as part of Flare ... instead of a free window").** The pop-up
shows inside the side panel, not as a separate window:
- It covers the page in the panel. The page that opened it keeps running behind it, hidden, so the link back still works.
- A strip at the top, in Flare's own look, says "Pop-up from bank.example" (the site, never the page's title) with a
  Close button.
- The page's requested size and position are ignored: the pop-up fills the panel.
- It closes when the page calls `window.close()`, when you press Close, when the panel or tab closes, and when Flare quits.
  Then the page that opened it shows again.
- A second pop-up from the pop-up stacks on top, with the same strip.

**What carries over.** The panel's profile, saved passwords and autofill, the permission denial, and the page-address
rules. Downloads from a pop-up are cancelled in this version.

**Agents.** An agent's browser tools can't see or reach a pop-up, so card and code pages never go to the agent. While a
pop-up is open, the agent's input tools (click, type, press, navigate, eval) answer "a pop-up is open, the person must
finish it". Reading the panel still works, and `browser_state` says a pop-up is open.

## 2. Ask before opening apps

A page link to an app, such as `bankid:`, `swish:`, `mailto:` or `tel:`, shows a Flare dialog instead of being blocked:
"example.com wants to open bankid on this PC." with **Open** and **Cancel**. Flare opens the app itself; the page never does.

- Cancel is the default: Enter, Space, Escape and a click outside all cancel. Open needs a click, or Tab then Enter, and
  ignores clicks in the first 0.3 s.
- Never offered, cancelled with a note as today: `javascript:`, `data:`, `file:`, `vbscript:` and a list of known
  dangerous Windows handlers (`ms-msdt`, `search-ms`, `ms-officecmd` and others). Apps with no handler on the PC get a note
  "Nothing on this PC opens X:".
- A click by the person is not required, because bank apps are often opened by a script after a form.
- Limits: one dialog per tab at a time, one ask per site and app every 10 s, Cancel mutes that pair until the page changes
  site, 5 asks a minute in total.
- While an agent is driving the panel, the dialog never appears, the open is cancelled, and the agent's call answers "only
  the person can allow opening an app".
- A page may open its own `blob:` content (for example a receipt) in the panel. Other sites' `blob:` stays blocked.

## 3. Log browser problems

New lines in Flare's diagnostics log (`diag.jsonl`), with the **site name only**: never addresses, paths, queries, form
data, page titles, cookies or certificate contents.

| Kind | When |
|---|---|
| `web-refused` | A navigation, app link, pop-up or frame was refused, and why |
| `web-popup` | A pop-up opened, closed or went to the panel |
| `web-load-failed` | A page failed to load, with the error name (`cert-expired`, `name-not-resolved` and so on) |
| `web-process` | The browser process failed or restarted |
| `web-cert` | A certificate error, logged only; behaviour is unchanged |
| `web-permission` | A permission the page asked for was denied |
| `web-nav` | The chain of sites a page went through. Only in verbose mode |

Each line has the tab, the site and the opening page's site, since a 2FA failure is a hand-off from shop to bank.
Repeats are limited to one per 10 s per site and problem. `PRIVACY.md` and `docs/diagnostics.md` say what is kept.

## Rules changed

Rule 189 and the e2e check that "window.open goes to the same panel" change to the table above. Mini-app and plugin
rules stay. Rule 110 (the log holds no typed or page text) is extended with the site-only rule. Rule 188 is untouched:
nothing a page does reaches a shell, and Flare opens apps with the Windows shell on a bare link, never a command.

## Not in this version

Agent access to pop-ups, downloads from pop-ups, "always allow this site for this app", a padlock or certificate view, the
same pop-up change for mini-apps and plugins.
