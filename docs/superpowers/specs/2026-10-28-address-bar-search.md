# The web panel's address field searches Google: spec

Date: 2026-10-28. Owner: Max.

## The problem

Enter in the address field sent the text through `normalize_url`, which puts `https://` in front of anything. `youtube`
opened `https://youtube/`, a host that does not exist, and the page stayed blank with nothing said; `cat videos` was
refused for its space.

## Owner decision (2026-10-28)

Anything typed that is not an address shows Google results: `https://www.google.com/search?q=<the text, form-encoded>`.

## What is an address

`flare_surfaces::web::address_or_search`, used only by the person's address field (`WebCtl::submit`). The text is trimmed
first; an empty field does nothing (no navigation, no note).

- A scheme of `http`, `https` or `about:blank`: an address, through `normalize_url` as before.
- A scheme that cannot open here (`file:`, `javascript:`, `data:`, `vbscript:`, `blob:`, `ftp:`, `ws:`, `wss:`, `chrome:`,
  `edge:`, `view-source:`, and any `x://`) is still refused with the reason in the dim line; it is not sent to Google.
  Any other `word:word` (`note:this`) is a search.
- With no space and no scheme: an address when the host (before the first `/`, `?` or `#`) has a dot between non-empty
  parts and a last part of letters (`youtube.com`, `a.b/path`, `bücher.de`), is `localhost`, an IPv4 address, an IPv6
  literal in brackets (`[::1]`, `[::1]:80`), or `host:port` where the host is localhost, an IP or a name with a letter (`myserver:8080`; `10:30` and `3:16` are searches). It then goes through `normalize_url`; if that refuses it
  (`me@example.com`, a login), it is a search.
- Everything else is a search: one word (`youtube`), text with a space (`cat videos`, even `http://x.com/a b`), words with
  `?`, `#`, `&`, `%`, letters of any script. A search longer than 2048 characters once encoded is refused as too long.

The agents' `browser.open` / `browser.navigate`, `panel.open` and a stored panel keep `normalize_url`, which never turns
text into a search (`a_caller_and_a_stored_panel_never_get_a_search`).

## A site that cannot be reached

When the latest navigation the page started (`started_nav`) ends with `ok = false`, the address row's dim line says
"Could not open <host>" (`web::failed_load_notice`), unless a note is already there (a refused navigation). Only the
WebView2 statuses that mean the site was not reached count: certificate errors (1 to 5), server unreachable, timeout,
bad response (6 to 8), reset, disconnected, cannot connect, name not resolved (10 to 13), redirect failed (15),
unexpected (16). A stopped or replaced navigation (14), a stream handed to the player or a download (9), the page's own
error page (0) and a login asked for (17, 18) say nothing. The next navigation that starts clears it, as every note is
cleared.

## Tests

- `web.rs`: `the_address_field_opens_an_address_as_an_address`, `the_address_field_searches_for_anything_else`,
  `the_address_field_still_refuses_a_scheme_and_does_not_search_it`, `a_caller_and_a_stored_panel_never_get_a_search`,
  `a_site_that_cannot_be_reached_is_named_and_the_rest_say_nothing`.
- `appctl/tests/web_tests.rs` (the fake browser): `the_address_field_searches_google_for_what_is_not_an_address`,
  `a_site_that_cannot_be_reached_is_said_in_the_address_row_until_the_next_load`; `two words` left
  `the_address_field_refuses_what_a_panel_may_not_open`, which now also holds that blank text sends nothing.
