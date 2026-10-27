//! Which addresses a networked page may reach (plugins phase 6, design 13): an `https` address on a public host.
//! A plugin that holds `network`, and Flare's own audio page for an address the agent gave, reach the internet and
//! nothing on this PC or its network: loopback, private and link-local ranges, `localhost` and the names that stay on
//! a local network (`.local`, `.localhost`, `.internal`, `.lan`, `.home.arpa`, a name without a dot) are refused.
//!
//! A Content-Security-Policy cannot name a range to leave out, so this is checked where a request is decided (the
//! browser glue's request wall for a plugin's page) and before Flare uses an address itself; the plugins' networked
//! browser also maps the local names and address ranges to "not found" in its resolver, the wall for a WebSocket,
//! which never meets the request wall (`modview::net_browser_arguments`). A public name that a resolver points at a
//! private address is not seen here (the browser's own private-network checks are the wall behind it).

use std::net::{Ipv4Addr, Ipv6Addr};

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

/// An IPv6 address on the internet: not loopback, unspecified, unique-local (fc00::/7), link-local (fe80::/10),
/// multicast, documentation, and an IPv4 one inside it only when that one is public.
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
}
