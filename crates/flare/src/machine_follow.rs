//! Finding a linked machine again after its address changed (rule 1373; Max, 2026-11-25: the router moved the Mac from
//! the address pairing saved, and the folder browser waited on "Loading..." with nothing to say).
//!
//! A paired machine is saved with the address it was reached at, the SSH host key it proved then (`host_key`) and
//! Flare's own known-hosts file, where that key is pinned for that address (rule 1259). A link to it
//! ([`connect`], called by `SshConnector`, so a remote tab, its reconnects and the folder browser all follow):
//!
//! 1. **The saved address first, briefly.** When it is an IP address, one plain TCP connect to ssh's port, at most
//!    [`PROBE_WAIT`]. Nothing there: ssh is not started at it at all. Something there: ssh as always.
//! 2. **Then its names.** Only when ssh could not reach it, could not find it, or met another key there (another device
//!    may have the address now): the machine's `.local` names are asked on the network itself (mDNS, at most
//!    [`LOOKUP_WAIT`], never the system's resolver, whose cache can hold an old address): the name saved when it was
//!    paired (`hostname`), `<name>.local` for a machine paired before that, and the name its own Flare answers while
//!    Computer sharing is on (`pairing::mdns_host`).
//! 3. **An address is followed only when the machine proves its key there.** ssh runs at the new address with
//!    `HostKeyAlias` set to the saved address's pin and strict checking, so ssh itself checks that the computer there
//!    holds the private half of the key pinned at pairing. A name, an announcement or an address is never proof. A
//!    computer with another key is never followed: the link fails with "Its identity changed. Pair again".
//! 4. **Proven: the new address is saved.** The key is pinned for the new address too, then `machines.json` takes the
//!    new address (read strict and written whole, audit #3 and rule 36). Nothing is written before the proof.
//!
//! A machine that is not paired (an alias from `~/.ssh/config`, a typed address with no pinned key) is never followed:
//! there is no key to prove. Every call here is BLOCKING: a link worker's (rule 34).

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::machines::{Machine, host_word};
use crate::pair_keys::{PublicKey, host_pattern, pin_host};
use crate::remote_link::{LinkError, ssh_argv};
use crate::ssh_problem::{NOT_REACHED, SshProblem, sort_down};

/// How long the saved address (and each new one) may take to accept a TCP connect on ssh's port.
pub const PROBE_WAIT: Duration = Duration::from_secs(3);
/// How long the network is asked for the machine's names.
pub const LOOKUP_WAIT: Duration = Duration::from_secs(3);
/// Once one name answered, how much longer the others may take.
const FOUND_GRACE: Duration = Duration::from_millis(400);
/// The most new addresses tried.
const ADDRS_MAX: usize = 6;

/// The network calls of following: a port, so a test needs no network.
pub trait Net: Send + Sync {
    /// BLOCKING (up to `wait`): whether `ip` accepts a TCP connect on `port`.
    fn reaches(&self, ip: IpAddr, port: u16, wait: Duration) -> bool;
    /// BLOCKING (up to `wait`): the addresses the network answers for `names` (each `<label>.local.`).
    fn lookup(&self, names: &[String], wait: Duration) -> Vec<IpAddr>;
}

/// The real network: a TCP connect, and mDNS through `mdns-sd` (a daemon of its own, so the Add a machine dialog's
/// browse is never touched).
pub struct RealNet;

impl Net for RealNet {
    fn reaches(&self, ip: IpAddr, port: u16, wait: Duration) -> bool {
        std::net::TcpStream::connect_timeout(&SocketAddr::new(ip, port), wait).is_ok()
    }

    fn lookup(&self, names: &[String], wait: Duration) -> Vec<IpAddr> {
        use mdns_sd::{HostnameResolutionEvent as Ev, RecvTimeoutError};
        if names.is_empty() {
            return Vec::new();
        }
        let d = match mdns_sd::ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                log::info!("machines: no mDNS to look a machine up: {e}");
                return Vec::new();
            }
        };
        let ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
        let mut asks: Vec<_> = names
            .iter()
            .filter_map(|n| d.resolve_hostname(n, Some(ms)).ok())
            .map(Some)
            .collect();
        let until = Instant::now() + wait;
        let mut first: Option<Instant> = None;
        let mut found: Vec<IpAddr> = Vec::new();
        while asks.iter().any(Option::is_some) {
            let end = first.map_or(until, |f| (f + FOUND_GRACE).min(until));
            if Instant::now() >= end {
                break;
            }
            for slot in &mut asks {
                let Some(rx) = slot else { continue };
                // sync-ok: a link worker looks the machine up, every wait capped by `wait`
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(Ev::AddressesFound(_, addrs)) => {
                        for a in addrs {
                            let ip = a.to_ip_addr();
                            if !found.contains(&ip) {
                                found.push(ip);
                            }
                        }
                        first.get_or_insert_with(Instant::now);
                    }
                    Ok(Ev::SearchTimeout(_) | Ev::SearchStopped(_))
                    | Err(RecvTimeoutError::Disconnected) => *slot = None,
                    Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                }
            }
        }
        let _ = d.shutdown();
        found
    }
}

/// Pure: whether a machine can be followed: it was paired (its host key pinned in Flare's own known-hosts file) and
/// is reached at an address of its own.
pub fn followable(m: &Machine) -> bool {
    m.host_key.is_some() && m.known_hosts.is_some() && m.host.is_some()
}

/// Pure: `<name>.local` when a computer's name is one DNS label (letters, digits and inner dashes, 63 at most), as a
/// Mac's or a PC's own computer name is (`Maxs-Mac-Mini`, `DESKTOP-4F2K`); none for a name with a space or an
/// apostrophe ("Max's PC"), which is no host name.
pub fn local_name(name: &str) -> Option<String> {
    let label = name.strip_suffix(".local").unwrap_or(name);
    let ok = !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| format!("{label}.local"))
}

/// Pure: a `.local` name as mDNS asks it: ending in `.local.`.
fn mdns_name(h: &str) -> String {
    if h.ends_with(".local.") {
        h.to_string()
    } else if h.ends_with(".local") {
        format!("{h}.")
    } else {
        format!("{}.local.", h.trim_end_matches('.'))
    }
}

/// Pure: the names a machine is asked for on the network, in order, each once: the `.local` name saved at pairing,
/// `<name>.local` (a machine paired before names were saved), and the name its own Flare answers while Computer
/// sharing is on.
pub fn lookup_names(m: &Machine) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |n: String| {
        if !out.contains(&n) {
            out.push(n);
        }
    };
    if let Some(h) = m.hostname.as_deref().filter(|h| host_word(h)) {
        add(mdns_name(h));
    }
    if let Some(l) = local_name(m.name.trim()) {
        add(mdns_name(&l));
    }
    if !m.name.trim().is_empty() {
        add(crate::pairing::mdns_host(&m.name));
    }
    out
}

/// Pure: the addresses worth trying, in order: none twice, not the saved one, no loopback, unspecified or link-local
/// IPv6 address (it needs a zone the lookup does not give), IPv4 first, at most `ADDRS_MAX`.
pub fn usable(found: Vec<IpAddr>, saved: Option<IpAddr>) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    for ip in found {
        let link_local_v6 = matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80);
        if Some(ip) == saved
            || out.contains(&ip)
            || ip.is_loopback()
            || ip.is_unspecified()
            || link_local_v6
        {
            continue;
        }
        out.push(ip);
    }
    out.sort_by_key(IpAddr::is_ipv6);
    out.truncate(ADDRS_MAX);
    out
}

/// Pure: the machine at `addr`: its address there, and its alias too when the alias was the old address.
pub fn moved(m: &Machine, addr: &str) -> Machine {
    let mut n = m.clone();
    if m.ssh == m.target() {
        n.ssh = addr.to_string();
    }
    n.host = Some(addr.to_string());
    n
}

/// Pure: the name the machine's key is pinned under in Flare's known-hosts file: its saved address (with the port
/// when it is not 22), as pairing pinned it.
pub fn pin_alias(m: &Machine) -> String {
    host_pattern(m.target(), m.port.unwrap_or(22))
}

/// Pure: ssh's argv for the machine at a new address, made to prove the pinned key there: the usual argv
/// (`ssh_argv`: Flare's key only, Flare's known-hosts file only, strict checking) plus `HostKeyAlias=<the saved
/// address's pin>`, so ssh checks the key it meets against that pin and nothing else, and `CheckHostIP=no`, so the new
/// address is never looked up or learned on its own.
pub fn trial_argv(ssh: &str, m: &Machine, addr: &str) -> Option<Vec<String>> {
    let mut argv = ssh_argv(ssh, &moved(m, addr))?;
    let at = argv.iter().position(|a| a == "BatchMode=yes")? + 1;
    argv.splice(
        at..at,
        [
            "-o".to_string(),
            format!("HostKeyAlias={}", pin_alias(m)),
            "-o".to_string(),
            "CheckHostIP=no".to_string(),
        ],
    );
    Some(argv)
}

/// Pure: what ssh said, sorted (none for a refusal of Flare's own, or words that are no case).
fn problem(e: &LinkError) -> Option<SshProblem> {
    match e {
        LinkError::Down(m) => sort_down(m),
        LinkError::Refused(_) => None,
    }
}

/// Pure: whether a failed link at the saved address is one to look further for: nothing answered, the name was not
/// found, or a computer with another key holds the address now.
pub fn may_follow(e: &LinkError) -> bool {
    matches!(
        problem(e),
        Some(SshProblem::NotAnswering | SshProblem::NotFound | SshProblem::IdentityChanged)
    )
}

/// BLOCKING (a link worker's): the machine's link, followed to a new address when the saved one no longer reaches it
/// (see the module text). `start` starts ssh with an argv and says hello. The machine as it should now be saved comes
/// back with the link when it was found at a new address.
pub fn connect<T>(
    m: &Machine,
    ssh: &str,
    net: &dyn Net,
    start: &mut dyn FnMut(&[String]) -> Result<T, LinkError>,
) -> Result<(T, Option<Machine>), LinkError> {
    let cannot = || LinkError::Down(format!("cannot build the ssh command for {}", m.id));
    let argv = ssh_argv(ssh, m).ok_or_else(cannot)?;
    if !followable(m) {
        return start(&argv).map(|t| (t, None));
    }
    let port = m.port.unwrap_or(22);
    let saved = m.target().parse::<IpAddr>().ok();
    let first = match saved {
        Some(ip) if !net.reaches(ip, port, PROBE_WAIT) => LinkError::Down(format!(
            "{NOT_REACHED}nothing answered at {ip} port {port} within {} s",
            PROBE_WAIT.as_secs()
        )),
        _ => match start(&argv) {
            Ok(t) => return Ok((t, None)),
            Err(e) if may_follow(&e) => e,
            Err(e) => return Err(e),
        },
    };
    let names = lookup_names(m);
    let found = usable(net.lookup(&names, LOOKUP_WAIT), saved);
    log::info!(
        "machines: {} did not answer at {}; looked up {names:?}: {found:?}",
        m.id,
        m.target()
    );
    let mut other_key: Option<LinkError> = None;
    for ip in found {
        if !net.reaches(ip, port, PROBE_WAIT) {
            continue;
        }
        let addr = ip.to_string();
        let Some(argv) = trial_argv(ssh, m, &addr) else {
            continue;
        };
        match start(&argv) {
            Ok(t) => return Ok((t, Some(moved(m, &addr)))),
            // Another computer, or another key: never followed.
            Err(e) if problem(&e) == Some(SshProblem::IdentityChanged) => {
                log::warn!(
                    "machines: {addr} answers for {} but does not prove its key",
                    m.id
                );
                other_key = Some(e);
            }
            Err(e) => log::info!("machines: {} not reached at {addr}: {e}", m.id),
        }
    }
    Err(other_key.unwrap_or(first))
}

/// Pure: Flare's known-hosts text with `key` (a saved `host_key`) pinned for `pattern` too, its type taken from the
/// line that pins it now. None when no line pins that key (nothing to copy).
pub fn repin(text: &str, pattern: &str, key: &str) -> Option<String> {
    let kind = text.lines().find_map(|l| {
        let mut w = l.split_ascii_whitespace();
        let (_, kind, k) = (w.next()?, w.next()?, w.next()?);
        (k == key).then(|| kind.to_string())
    })?;
    Some(pin_host(
        text,
        pattern,
        &PublicKey {
            kind,
            key: key.to_string(),
        },
    ))
}

/// BLOCKING (two reads and two whole-file writes, a link worker's): a machine proven at a new address is saved there:
/// its key pinned for the new address in Flare's known-hosts file, then `machines.json`'s entry with the same id and
/// the same host key takes the new address. A file that cannot be read whole is never saved over (audit #3).
pub fn save_moved(machines: &Path, was: &Machine, now: &Machine) -> Result<(), String> {
    let (Some(key), Some(known), Some(addr)) = (&was.host_key, &was.known_hosts, &now.host) else {
        return Err("only a paired machine is followed".into());
    };
    let known = Path::new(known);
    let text = std::fs::read_to_string(known).map_err(|e| e.to_string())?; // sync-ok: a link worker
    let pattern = host_pattern(addr, now.port.unwrap_or(22));
    let pinned = repin(&text, &pattern, key)
        .ok_or_else(|| "the machine's key is not pinned in Flare's known hosts".to_string())?;
    let mut list = crate::machines::load_machines(machines)?;
    let Some(entry) = list
        .iter_mut()
        .find(|e| e.id == was.id && e.host_key.as_deref() == Some(key.as_str()))
    else {
        return Err(format!("{} is no longer in machines.json", was.id));
    };
    entry.host = now.host.clone();
    entry.ssh = now.ssh.clone();
    flare_settings::atomic::write_atomic(known, &pinned).map_err(|e| e.to_string())?;
    flare_settings::atomic::write_atomic(machines, &crate::machines::machines_json(&list))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_tabs::tabs::Os;
    use std::collections::HashMap;
    use std::sync::Mutex;

    const KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIPinnedKeyOfTheMacMini0000000000000000";

    fn mac(known: &str) -> Machine {
        Machine {
            id: "maxs-mac-mini".into(),
            name: "Maxs-Mac-Mini".into(),
            kind: Some(Os::Mac),
            ssh: "192.0.2.242".into(),
            user: Some("max".into()),
            flare: "/Applications/Flare.app/Contents/MacOS/flare".into(),
            host: Some("192.0.2.242".into()),
            key: Some("/k/id_flare_ed25519".into()),
            known_hosts: Some(known.into()),
            host_key: Some(KEY.into()),
            ..Machine::default()
        }
    }

    /// A network in memory: the addresses that accept a connect, and what each name answers.
    #[derive(Default)]
    struct FakeNet {
        up: Vec<IpAddr>,
        names: HashMap<String, Vec<IpAddr>>,
        asked: Mutex<Vec<String>>,
    }

    impl Net for FakeNet {
        fn reaches(&self, ip: IpAddr, _port: u16, _wait: Duration) -> bool {
            self.up.contains(&ip)
        }
        fn lookup(&self, names: &[String], _wait: Duration) -> Vec<IpAddr> {
            self.asked.lock().unwrap().extend(names.iter().cloned());
            names
                .iter()
                .flat_map(|n| self.names.get(n).cloned().unwrap_or_default())
                .collect()
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// A stand-in for ssh: the address it was pointed at, and whether that computer proves the pinned key. A computer
    /// with another key answers as ssh does with strict checking.
    fn ssh_at<'a>(
        keys: &'a HashMap<&'static str, bool>,
    ) -> impl FnMut(&[String]) -> Result<String, LinkError> + 'a {
        move |argv: &[String]| {
            let at = argv[argv.len() - 2].clone();
            match keys.get(at.as_str()) {
                Some(true) => Ok(at),
                Some(false) => Err(LinkError::Down(
                    "the link ended; ssh said: Host key verification failed.".into(),
                )),
                None => Err(LinkError::Down(format!(
                    "the link ended; ssh said: ssh: connect to host {at} port 22: Connection timed out"
                ))),
            }
        }
    }

    /// Rule 1373: the saved address does not answer, the machine's `.local` name answers with a new address, and the
    /// computer there proves the pinned key: the link is made there and the moved machine comes back to be saved.
    #[test]
    fn the_same_key_at_a_new_address_is_followed() {
        let m = mac("/p/flare_known_hosts");
        let mut net = FakeNet {
            up: vec![ip("192.0.2.76")],
            ..FakeNet::default()
        };
        net.names.insert(
            "Maxs-Mac-Mini.local.".into(),
            vec![ip("fe80::1"), ip("192.0.2.251"), ip("192.0.2.76")],
        );
        let keys = HashMap::from([("192.0.2.76", true)]);
        let mut tried: Vec<Vec<String>> = Vec::new();
        let mut ssh = ssh_at(&keys);
        let (link, now) = connect(&m, "ssh", &net, &mut |a: &[String]| {
            tried.push(a.to_vec());
            ssh(a)
        })
        .unwrap();
        assert_eq!(link, "192.0.2.76");
        let now = now.expect("moved");
        assert_eq!(now.host.as_deref(), Some("192.0.2.76"));
        assert_eq!(now.ssh, "192.0.2.76", "the alias was the old address");
        assert_eq!(now.host_key, m.host_key);
        // The saved address was never handed to ssh (its short check failed); .251 does not answer at all.
        assert_eq!(tried.len(), 1, "{tried:?}");
        let argv = &tried[0];
        for want in [
            "HostKeyAlias=192.0.2.242",
            "CheckHostIP=no",
            "StrictHostKeyChecking=yes",
            "UserKnownHostsFile=/p/flare_known_hosts",
            "IdentitiesOnly=yes",
        ] {
            assert!(argv.iter().any(|a| a == want), "{want}: {argv:?}");
        }
        assert_eq!(
            *net.asked.lock().unwrap(),
            ["Maxs-Mac-Mini.local.", "Maxs-Mac-Mini-flare.local."]
        );
    }

    /// Rule 1373: a computer at the new address that does not prove the pinned key is never followed, even when it
    /// answers to the machine's name; the link fails with the identity line, and nothing comes back to be saved.
    #[test]
    fn another_key_at_a_new_address_is_refused() {
        let m = mac("/p/kh");
        let mut net = FakeNet {
            up: vec![ip("192.0.2.9")],
            ..FakeNet::default()
        };
        net.names
            .insert("Maxs-Mac-Mini.local.".into(), vec![ip("192.0.2.9")]);
        let keys = HashMap::from([("192.0.2.9", false)]);
        let mut ssh = ssh_at(&keys);
        let Err(e) = connect(&m, "ssh", &net, &mut ssh) else {
            panic!("another key is never followed");
        };
        assert_eq!(
            crate::machines::failure_line("Maxs-Mac-Mini", &e, 1),
            "Its identity changed. Pair again"
        );
    }

    /// The saved address answers and proves the key: nothing is looked up. A machine found nowhere is "Not answering"
    /// after the short checks; one that is not paired is never followed; a refusal of the host's own is the answer.
    #[test]
    fn the_saved_address_is_used_when_it_answers_and_nothing_else_is_tried_otherwise() {
        let m = mac("/p/kh");
        let net = FakeNet {
            up: vec![ip("192.0.2.242")],
            ..FakeNet::default()
        };
        let keys = HashMap::from([("192.0.2.242", true)]);
        let mut ssh = ssh_at(&keys);
        let (link, now) = connect(&m, "ssh", &net, &mut ssh).unwrap();
        assert_eq!((link.as_str(), now), ("192.0.2.242", None));
        assert!(net.asked.lock().unwrap().is_empty());
        // Found nowhere: Flare's own check says so, in the plain words.
        let off = FakeNet::default();
        let e = connect(&m, "ssh", &off, &mut ssh).unwrap_err();
        assert_eq!(
            crate::machines::failure_line("Maxs-Mac-Mini", &e, 1),
            "Not answering"
        );
        // An alias from ~/.ssh/config (no pinned key): ssh as always, nothing looked up.
        let alias = Machine {
            id: "nas".into(),
            name: "nas".into(),
            ssh: "nas".into(),
            flare: "flare".into(),
            ..Machine::default()
        };
        let quiet = FakeNet::default();
        assert!(connect(&alias, "ssh", &quiet, &mut ssh).is_err());
        assert!(quiet.asked.lock().unwrap().is_empty());
        // The host answered and refused: no other address is looked for.
        let refused = |_: &[String]| -> Result<String, LinkError> {
            Err(LinkError::Refused(flare_ctl::CtlError::new(
                "unsupported_protocol",
                "remote protocol 2 here, 1 there",
            )))
        };
        assert!(connect(&m, "ssh", &net, &mut { refused }).is_err());
        let net2 = FakeNet {
            up: vec![ip("192.0.2.242")],
            ..FakeNet::default()
        };
        let _ = connect(&m, "ssh", &net2, &mut { refused });
        assert!(net2.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn names_addresses_and_the_alias_are_read_plainly() {
        assert_eq!(
            local_name("Maxs-Mac-Mini").as_deref(),
            Some("Maxs-Mac-Mini.local")
        );
        assert_eq!(
            local_name("DESKTOP-4F2K.local").as_deref(),
            Some("DESKTOP-4F2K.local")
        );
        for bad in ["Max's PC", "", "-x", "a b", "æøå", &"x".repeat(64)] {
            assert_eq!(local_name(bad), None, "{bad}");
        }
        let mut m = mac("/p/kh");
        m.hostname = Some("studio.local".into());
        assert_eq!(
            lookup_names(&m),
            [
                "studio.local.",
                "Maxs-Mac-Mini.local.",
                "Maxs-Mac-Mini-flare.local."
            ]
        );
        m.name = "Max's PC".into();
        m.hostname = None;
        assert_eq!(lookup_names(&m), ["Max-s-PC-flare.local."]);
        assert_eq!(
            usable(
                vec![
                    ip("fd00::5"),
                    ip("192.0.2.242"),
                    ip("127.0.0.1"),
                    ip("fe80::1"),
                    ip("192.0.2.76"),
                    ip("192.0.2.76"),
                ],
                Some(ip("192.0.2.242"))
            ),
            [ip("192.0.2.76"), ip("fd00::5")]
        );
        // A port that is not 22: the pin's own pattern.
        m.port = Some(2222);
        assert_eq!(pin_alias(&m), "[192.0.2.242]:2222");
        let argv = trial_argv("ssh", &m, "192.0.2.76").unwrap();
        assert_eq!(
            &argv[..7],
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "HostKeyAlias=[192.0.2.242]:2222",
                "-o",
                "CheckHostIP=no"
            ]
        );
        assert_eq!(argv[argv.len() - 2], "192.0.2.76");
        // The alias is kept when it is a name of its own.
        let mut named = mac("/p/kh");
        named.ssh = "mini".into();
        assert_eq!(moved(&named, "192.0.2.76").ssh, "mini");
        assert!(!followable(&Machine::default()));
    }

    /// Rule 1373: a proven move is saved: the key pinned for the new address too (its type copied from the pin there
    /// is), and the entry with that id and that key takes the new address; other entries and lines stay as they were.
    #[test]
    fn a_proven_move_is_saved_and_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let known = dir.path().join("flare_known_hosts");
        std::fs::write(
            &known,
            format!("192.0.2.242 ssh-ed25519 {KEY}\n192.0.2.5 ssh-ed25519 AAAAother\n"),
        )
        .unwrap();
        let m = mac(&known.to_string_lossy());
        let other = Machine {
            id: "pc".into(),
            name: "PC".into(),
            ssh: "pc".into(),
            flare: "flare".into(),
            ..Machine::default()
        };
        let file = crate::machines::machines_file(dir.path());
        crate::machines::FileMachines::new(dir.path())
            .save(&[m.clone(), other.clone()])
            .unwrap();
        save_moved(&file, &m, &moved(&m, "192.0.2.76")).unwrap();
        let list = crate::machines::read_machines(&file);
        assert_eq!(list[0].host.as_deref(), Some("192.0.2.76"));
        assert_eq!(list[0].ssh, "192.0.2.76");
        assert_eq!(list[0].host_key.as_deref(), Some(KEY));
        assert_eq!(list[1], other);
        let text = std::fs::read_to_string(&known).unwrap();
        assert!(
            text.contains(&format!("192.0.2.76 ssh-ed25519 {KEY}\n")),
            "{text}"
        );
        assert!(text.contains("192.0.2.5 ssh-ed25519 AAAAother"), "{text}");
        // An entry whose key is not the one proven is never moved; a broken file is never saved over.
        let mut stranger = m.clone();
        stranger.host_key = Some("AAAAstranger".into());
        assert!(save_moved(&file, &stranger, &moved(&stranger, "192.0.2.99")).is_err());
        std::fs::write(&file, "{ not json").unwrap();
        assert!(save_moved(&file, &m, &moved(&m, "192.0.2.77")).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
        assert_eq!(repin("x ssh-ed25519 AAAAother\n", "y", KEY), None);
    }
}
