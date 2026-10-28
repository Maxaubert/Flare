# Networked pages checked after the name lookup (issue #97): spec

Date: 2026-10-28. Issue: #97 ("Plugins: networked pages can reach local addresses through DNS (rebinding)").

## The gap

A plugin page with `network` (`EnvKind::PluginNet`) and a mini-app that was granted `https:` (`EnvKind::AppNet`, the
audio page too) were walled by the address as written: the request wall (`modview::route_request_net`,
`net::is_public_https`) and, for the plugins' browser, `--host-resolver-rules` mapping local names and numeric ranges to
"not found". A public name whose DNS answer is `127.0.0.1`, a `192.168.x` address or another private one passed every
check. A WebSocket never meets the request wall, so for `wss:` the resolver rules were the only line, and `AppNet` had
none at all.

## The fix

1. **Pure checks** (`flare_surfaces::net`): `parse_authority` reads a `CONNECT` target (host and port, port required,
   IPv6 in brackets, nothing else); `allowed_addresses(host, port, resolve)` refuses by the host's text first
   (`refusal`: local names, a numeric local address), takes a numeric address as it is, and otherwise looks the name up
   with the given resolver and keeps only the addresses `ip_refusal` passes. None left is a refusal. A mixed answer
   keeps its public addresses only.
2. **The proxy** (`flare_surfaces::netproxy`, std only): a `TcpListener` on `127.0.0.1:0`, started once a process
   (`OnceLock`), so its port is fixed for the process life. A thread a connection, at most 256 at once. The request
   head is read within 8 KB and 10 s (else closed with no answer). Only `CONNECT` is served (any other method 403, a
   bad target 400). The rule is applied, then `connect_timeout` (10 s) to one of the addresses it gave, in order: the
   tunnel is pinned to an address that was checked, and the name is never looked up again. Then 200 and a copy both
   ways on two threads; one side's end of file half-closes the other, an error closes both, and 5 minutes with no byte
   either way closes both. Bytes sent right after the head are forwarded.
3. **Browser arguments**: `PluginNet` and `AppNet` get `--proxy-server=http://127.0.0.1:<port>
   --proxy-bypass-list=<-loopback>`. `<-loopback>` removes Chromium's implicit bypass of loopback and link-local
   addresses (it works with a fixed proxy, `net/docs/proxy.md`, "Implicit bypass rules"), so those go to the proxy and
   are refused there. With a fixed proxy there is no fall back to a direct route. The plugins' resolver rules stay as
   defence in depth, preceded by `EXCLUDE 127.0.0.1`: they match a numeric host too (`127.*`), and without the
   exclusion the browser could not reach its own proxy. The web panel (`EnvKind::Web`) is unchanged, and the sealed
   kinds keep the dead proxy on port 9.
4. **Fail closed**: `host::env_arguments` asks `netproxy::port()` first; if the proxy could not start, the networked
   browser is not made and its views fail with the reason, never started unproxied.

Pages that never use the network still load: a plugin's own `https://flare-mod-<id>.local/` is answered in
`WebResourceRequested` and a mini-app's folder is a host mapping; both are handled before the network stack (measured
for the sealed browsers, which already have a proxy that is not there: `docs/surfaces/lessons.md`).

## Left as it was

- The URL install of a plugin (`flare_mods::fetch`, the `Walled` resolver) already checks every resolved address.
- The proxy listens on loopback with no password: any program on this PC can use it, and it reaches only public
  addresses, which that program can reach anyway.
- Not measured on a live WebView2 yet (no windowed run in this change). The e2e `miniApp` "webrtc" WebSocket check
  now expects a loopback listener to hear nothing for a networked app too (it was the control that reached it); the
  next e2e run measures that, and that the `EXCLUDE 127.0.0.1` rule lets the browser reach its proxy.

## Tests

`flare_surfaces::net` (`a_connect_target_is_a_host_and_a_port_and_nothing_else`,
`a_public_name_pointed_at_this_pc_is_refused_after_the_lookup`,
`an_address_written_as_numbers_or_a_local_name_is_refused_without_a_lookup`), `flare_surfaces::netproxy` (the head
parser, a fixed port), `crates/flare-surfaces/tests/netproxy.rs` (8: a relay both ways, early bytes, GET 403 with nothing
dialled, a rebinding name and numeric local targets 403 with nothing dialled, a bad target 400, an oversized head
closed, a slow head closed, an idle tunnel closed), `modview` and `app` (the arguments), `flare_web::host`
(`every_browser_kind_gets_the_same_arguments_each_time_it_is_made`,
`the_networked_browsers_go_through_the_filtering_proxy_and_the_web_panel_does_not`).
