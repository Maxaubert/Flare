//! Which addresses a networked page may reach (plugins phase 6, design 13): an `https` address on a public host.
//! A plugin that holds `network`, and Flare's own audio page for an address the agent gave, reach the internet and
//! nothing on this PC or its network: loopback, private and link-local ranges, `localhost` and the names that stay on
//! a local network (`.local`, `.localhost`, `.internal`, `.lan`, `.home.arpa`, a name without a dot) are refused.
//!
//! A Content-Security-Policy cannot name a range to leave out, so this is checked where a request is decided (the
//! browser glue's request wall for a plugin's page) and before Flare uses an address itself; the plugins' networked
//! browser also maps the local names and address ranges to "not found" in its resolver, the wall for a WebSocket,
//! which never meets the request wall (`modview::net_browser_arguments`). A public name that a resolver points at a
//! private address (DNS rebinding, issue #97) passes the text checks, so those browsers send every connection through
//! Flare's own proxy (`crate::netproxy`), which looks the name up itself, keeps only the addresses
//! [`allowed_addresses`] lets through and connects to one of them.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Name endings that stay on a local network.
const LOCAL_SUFFIXES: [&str; 6] = [
    ".local",
    ".localhost",
    ".internal",
    ".lan",
    ".home.arpa",
    ".localdomain",
];

/// Why `uri` may not be reached by a networked page, or None when it is an `https` address on a public host with no
/// user name or password.
pub fn refusal(uri: &str) -> Option<&'static str> {
    let Ok(u) = url::Url::parse(uri) else {
        return Some("that is not an address");
    };
    if u.scheme() != "https" {
        return Some("only https addresses are reached");
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Some("an address with a user name or password is refused");
    }
    match u.host() {
        None => Some("the address has no host"),
        Some(url::Host::Ipv4(ip)) => {
            (!public_v4(ip)).then_some("a local or private address is refused")
        }
        Some(url::Host::Ipv6(ip)) => {
            (!public_v6(ip)).then_some("a local or private address is refused")
        }
        Some(url::Host::Domain(d)) => local_name(d).then_some("a local host name is refused"),
    }
}

/// Why an address a name resolved to may not be reached, or None for an address on the internet (the same ranges as
/// [`refusal`]). Flare's own download of a plugin zip (plugins phase 7) checks each resolved address with it, so a public
/// name pointed at this PC or its network is not reached.
pub fn ip_refusal(ip: std::net::IpAddr) -> Option<&'static str> {
    let public = match ip {
        std::net::IpAddr::V4(v4) => public_v4(v4),
        std::net::IpAddr::V6(v6) => public_v6(v6),
    };
    (!public).then_some("a local or private address is refused")
}

/// The host and port of a proxy's `CONNECT` target (`example.com:443`, `[2606:4700::1111]:443`), or None when it is
/// not one: the port is required, an IPv6 address must be in brackets, and nothing but a host and a port is allowed
/// (no user part, path, space or control character). The host comes back as written, without its brackets.
pub fn parse_authority(authority: &str) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        (h, tail.strip_prefix(':')?)
    } else {
        let (h, p) = authority.rsplit_once(':')?;
        if h.contains(':') {
            return None;
        }
        (h, p)
    };
    if host.is_empty()
        || port.is_empty()
        || port.len() > 5
        || !port.bytes().all(|b| b.is_ascii_digit())
        || host
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')))
    {
        return None;
    }
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    Some((host.to_string(), port))
}

/// Where a `CONNECT` to `host` (as [`parse_authority`] gave it) may go. An address written as numbers is checked as it
/// is. A name is refused by [`refusal`]'s rules for its text, then looked up with `resolve`, and only the addresses
/// [`ip_refusal`] has no reason for are kept, so a public name pointed at this PC or its network reaches nothing there;
/// none left is a refusal. The caller connects to one of the addresses it got and never looks the name up again.
pub fn allowed_addresses<R>(
    host: &str,
    port: u16,
    resolve: R,
) -> Result<Vec<SocketAddr>, &'static str>
where
    R: FnOnce(&str, u16) -> std::io::Result<Vec<SocketAddr>>,
{
    let uri = if host.contains(':') {
        format!("https://[{host}]/")
    } else {
        format!("https://{host}/")
    };
    if let Some(why) = refusal(&uri) {
        return Err(why);
    }
    let parsed = url::Url::parse(&uri).map_err(|_| "that is not an address")?;
    let ip = match parsed.host() {
        Some(url::Host::Ipv4(v4)) => IpAddr::V4(v4),
        Some(url::Host::Ipv6(v6)) => IpAddr::V6(v6),
        Some(url::Host::Domain(name)) => {
            let found = resolve(name, port).map_err(|_| "the name was not found")?;
            let kept: Vec<SocketAddr> = found
                .into_iter()
                .filter(|a| ip_refusal(a.ip()).is_none())
                .collect();
            return if kept.is_empty() {
                Err("the name points at no public address")
            } else {
                Ok(kept)
            };
        }
        None => return Err("the address has no host"),
    };
    match ip_refusal(ip) {
        Some(why) => Err(why),
        None => Ok(vec![SocketAddr::new(ip, port)]),
    }
}

/// Whether a networked page may reach `uri` ([`refusal`] has no reason).
pub fn is_public_https(uri: &str) -> bool {
    refusal(uri).is_none()
}

/// A name that stays on this PC or its network.
fn local_name(name: &str) -> bool {
    let n = name.trim_end_matches('.').to_ascii_lowercase();
    n.is_empty()
        || n == "localhost"
        || !n.contains('.')
        || LOCAL_SUFFIXES.iter().any(|s| n.ends_with(s))
}

/// An IPv4 address on the internet: not loopback, private, shared (100.64/10), link-local, unspecified, broadcast,
/// multicast, documentation or reserved.
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19))
        || a >= 240)
}

/// An IPv6 address on the internet: not loopback, unspecified, unique-local (fc00::/7), link-local (fe80::/10), the
/// old site-local range (fec0::/10, which still stays on a site's network), multicast, documentation, and an IPv4 one
/// inside it only when that one is public.
fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let s = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xffc0) == 0xfec0
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        // The IPv4-compatible and NAT64 forms carry an IPv4 address the check above did not see.
        || (s[0..6] == [0, 0, 0, 0, 0, 0])
        || (s[0] == 0x0064 && s[1] == 0xff9b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_https_address_passes() {
        for ok in [
            "https://example.com/a.mp3",
            "https://api.example.org:8443/x?y=1",
            "https://8.8.8.8/",
            "https://[2606:4700::1111]/",
            "https://Example.COM./",
        ] {
            assert_eq!(refusal(ok), None, "{ok}");
        }
    }

    #[test]
    fn loopback_private_ranges_local_names_and_other_schemes_are_refused() {
        for bad in [
            "http://example.com/",
            "ftp://example.com/",
            "file:///C:/x",
            "https://localhost/",
            "https://LOCALHOST./",
            "https://app.localhost/",
            "https://printer.local/",
            "https://nas.lan/",
            "https://box.home.arpa/",
            "https://db.internal/",
            "https://intranet/",
            "https://127.0.0.1/",
            "https://127.1/",
            "https://0x7f000001/",
            "https://10.1.2.3/",
            "https://172.16.0.1/",
            "https://192.168.1.10/",
            "https://169.254.169.254/latest",
            "https://100.64.0.1/",
            "https://0.0.0.0/",
            "https://[::1]/",
            "https://[::]/",
            "https://[fd00::1]/",
            "https://[fe80::1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://[::ffff:192.168.0.1]/",
            "https://[64:ff9b::a00:1]/",
            "https://user:pw@example.com/",
            "not an address",
        ] {
            assert!(refusal(bad).is_some(), "{bad}");
            assert!(!is_public_https(bad), "{bad}");
        }
    }

    #[test]
    fn a_resolved_address_is_refused_by_the_same_ranges() {
        for bad in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(ip_refusal(bad.parse().unwrap()).is_some(), "{bad}");
        }
        for ok in ["8.8.8.8", "2606:4700::1111"] {
            assert_eq!(ip_refusal(ok.parse().unwrap()), None, "{ok}");
        }
    }

    #[test]
    fn a_connect_target_is_a_host_and_a_port_and_nothing_else() {
        assert_eq!(
            parse_authority("example.com:443"),
            Some(("example.com".into(), 443))
        );
        assert_eq!(
            parse_authority("[2606:4700::1111]:8443"),
            Some(("2606:4700::1111".into(), 8443))
        );
        assert_eq!(
            parse_authority("8.8.8.8:443"),
            Some(("8.8.8.8".into(), 443))
        );
        for bad in [
            "example.com",
            "example.com:",
            "example.com:0",
            "example.com:65536",
            "example.com:44a",
            ":443",
            "::1:443",
            "[::1]",
            "[::1]443",
            "user@example.com:443",
            "example.com/x:443",
            "exa mple.com:443",
            "example.com:+443",
            "",
        ] {
            assert_eq!(parse_authority(bad), None, "{bad}");
        }
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn a_public_name_pointed_at_this_pc_is_refused_after_the_lookup() {
        let to = |addrs: Vec<SocketAddr>| move |_: &str, _: u16| Ok(addrs);
        assert!(allowed_addresses("rebind.example", 443, to(vec![sa("127.0.0.1:443")])).is_err());
        for private in [
            "10.0.0.5:443",
            "100.64.1.1:443",
            "169.254.169.254:443",
            "192.168.1.1:443",
            "[fc00::1]:443",
            "[fe80::1]:443",
            "[fec0::1]:443",
            "[::1]:443",
            "[::ffff:127.0.0.1]:443",
        ] {
            assert!(
                allowed_addresses("rebind.example", 443, to(vec![sa(private)])).is_err(),
                "{private}"
            );
        }
        assert!(
            allowed_addresses("rebind.example", 443, to(vec![])).is_err(),
            "no address is a refusal"
        );
        assert_eq!(
            allowed_addresses(
                "mixed.example",
                443,
                to(vec![
                    sa("192.168.0.2:443"),
                    sa("93.184.215.14:443"),
                    sa("[::1]:443")
                ])
            ),
            Ok(vec![sa("93.184.215.14:443")]),
            "only the public address is kept"
        );
        let gone = |_: &str, _: u16| -> std::io::Result<Vec<SocketAddr>> {
            Err(std::io::Error::other("not found"))
        };
        assert!(allowed_addresses("gone.example", 443, gone).is_err());
    }

    #[test]
    fn an_address_written_as_numbers_or_a_local_name_is_refused_without_a_lookup() {
        let never =
            |h: &str, _: u16| -> std::io::Result<Vec<SocketAddr>> { panic!("{h} was looked up") };
        for bad in [
            "::ffff:7f00:1",
            "127.0.0.1",
            "127.1",
            "10.1.2.3",
            "100.64.0.1",
            "169.254.1.1",
            "fc00::1",
            "fe80::1",
            "localhost",
            "printer.local",
            "intranet",
        ] {
            assert!(allowed_addresses(bad, 443, never).is_err(), "{bad}");
        }
        assert_eq!(
            allowed_addresses("8.8.8.8", 443, never),
            Ok(vec![sa("8.8.8.8:443")])
        );
        assert_eq!(
            allowed_addresses("2606:4700::1111", 443, never),
            Ok(vec![sa("[2606:4700::1111]:443")])
        );
        let (h, p) = parse_authority("[::ffff:7f00:1]:443").unwrap();
        assert!(allowed_addresses(&h, p, never).is_err());
    }
}
