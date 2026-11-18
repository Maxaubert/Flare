//! Which system an SSH host of the Add a machine drop-down runs, so its row draws the Windows, Linux or Apple logo (rule
//! 1278; Max, 2026-11-17: "id like it to show either the windows, linux or mac icon automatically depending on the
//! device").
//!
//! Two looks, each with a short time limit ([`STEP_TIMEOUT`]); a host that answers neither has no logo:
//! 1. The SSH server's greeting: a TCP connect to the host's `HostName` and `Port` from the ssh config, and its first
//!    `SSH-` line read. Nothing is sent and nothing logs in. `OpenSSH_for_Windows` is a PC; a greeting that names a
//!    Linux distribution (`Ubuntu`, `Debian`, `Raspbian`, ...) is Linux. A plain `OpenSSH_9.8` says nothing.
//! 2. Only then, the user's own ssh: `ssh -o BatchMode=yes -o ConnectTimeout=3 <host> uname -s`. BatchMode never asks
//!    for a password or a new host key (the host key rule is the user's own config), so it answers only where a key
//!    already logs in. `Darwin` is a Mac, `Linux` is Linux.
//!
//! What was found is kept in the profile's `host_os.json` per host (its name and `HostName`) with the time it was
//! found, so the logo shows at once the next time; the looks run again in the background while the dialog is open. A
//! look that fails (a timeout, a VPN in the way) keeps what was found before. Nothing found here is ever a machine's
//! `kind`: that word says how a login shell is quoted (rules 1207, 1208), and only the host's own `remote.hello` sets it.
//!
//! Everything here that touches the network or the disk is BLOCKING: a worker's call.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_settings::page::machines::{MachineOs, SshHost};
use serde_json::{Value, json};

/// Each look's time limit: the greeting's connect and its read, and ssh's connect.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(3);
/// How long ssh may run in all (a login that hangs after the connect is ended there).
pub const SSH_LIMIT: Duration = Duration::from_secs(6);
/// The most hosts looked at at once.
pub const WORKERS: usize = 8;
/// The file in the profile folder.
pub const CACHE_FILE: &str = "host_os.json";
/// The most bytes read for a greeting.
const BANNER_MAX: usize = 255;

/// A host as the cache keys it: its name and the computer it reaches.
pub type HostKey = (String, String);

/// The cache key of a host.
pub fn key_of(h: &SshHost) -> HostKey {
    (h.alias.clone(), h.host_name.clone())
}

/// Pure: the system an SSH server's greeting names, from its first `SSH-` line. A PC says `OpenSSH_for_Windows`; a
/// greeting whose words name a Linux distribution is Linux. A plain OpenSSH (a Mac, or a Linux that does not say), any
/// other system (FreeBSD) and anything that is no greeting say nothing.
pub fn os_of_banner(text: &str) -> Option<MachineOs> {
    const LINUX: [&str; 12] = [
        "ubuntu", "debian", "raspbian", "fedora", "centos", "rhel", "redhat", "alpine", "arch",
        "suse", "gentoo", "linux",
    ];
    let line = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .find(|l| l.starts_with("SSH-"))?;
    // `SSH-2.0-<software> <comments>`: the software and the comments, without case.
    let soft = line.splitn(3, '-').nth(2)?.to_ascii_lowercase();
    if soft.starts_with("openssh_for_windows") {
        return Some(MachineOs::Pc);
    }
    soft.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| LINUX.contains(&w))
        .then_some(MachineOs::Linux)
}

/// Pure: the system `uname -s` printed. `Darwin` is a Mac, `Linux` is Linux, an MSYS, MinGW or Cygwin shell is a PC.
pub fn os_of_uname(text: &str) -> Option<MachineOs> {
    let word = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let up = word.to_ascii_uppercase();
    match up.as_str() {
        "DARWIN" => Some(MachineOs::Mac),
        "LINUX" => Some(MachineOs::Linux),
        _ if ["MINGW", "MSYS", "CYGWIN", "WINDOWS_NT"]
            .iter()
            .any(|p| up.starts_with(p)) =>
        {
            Some(MachineOs::Pc)
        }
        _ => None,
    }
}

/// The two looks, so the tests can stand in for the network and ssh.
pub trait OsLookup: Send + Sync {
    /// BLOCKING: what the SSH server at `host`:`port` says first, None when it does not answer in time.
    fn banner(&self, host: &str, port: u16) -> Option<String>;
    /// BLOCKING: what `uname -s` prints over the user's ssh to `alias`, None when it cannot log in without asking.
    fn uname(&self, alias: &str) -> Option<String>;
}

/// BLOCKING: the system a host runs: its greeting first, then `uname -s` over ssh. None when neither says.
pub fn find_os(look: &dyn OsLookup, h: &SshHost) -> Option<MachineOs> {
    if let Some(os) = look
        .banner(&h.host_name, h.port.unwrap_or(22))
        .as_deref()
        .and_then(os_of_banner)
    {
        return Some(os);
    }
    look.uname(&h.alias).as_deref().and_then(os_of_uname)
}

/// The real looks: a TCP connect for the greeting, the user's own ssh for `uname -s`.
pub struct RealLookup;

impl OsLookup for RealLookup {
    fn banner(&self, host: &str, port: u16) -> Option<String> {
        use std::net::{TcpStream, ToSocketAddrs};
        let start = Instant::now();
        let addrs = (host, port).to_socket_addrs().ok()?;
        for addr in addrs.take(4) {
            let left = STEP_TIMEOUT.saturating_sub(start.elapsed());
            if left.is_zero() {
                return None;
            }
            let Ok(mut s) = TcpStream::connect_timeout(&addr, left) else {
                continue;
            };
            let _ = s.set_read_timeout(Some(STEP_TIMEOUT));
            let mut got = Vec::new();
            let mut buf = [0u8; 64];
            while got.len() < BANNER_MAX && start.elapsed() < STEP_TIMEOUT * 2 {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
                let text = String::from_utf8_lossy(&got);
                if text.lines().any(|l| l.starts_with("SSH-")) && text.contains('\n') {
                    break;
                }
            }
            return Some(String::from_utf8_lossy(&got).into_owned());
        }
        None
    }

    fn uname(&self, alias: &str) -> Option<String> {
        let mut cmd = std::process::Command::new(crate::remote_link::ssh_exe()); // sync-ok: the system lookup's worker
        let timeout = format!("ConnectTimeout={}", STEP_TIMEOUT.as_secs());
        cmd.args([
            "-o",
            "BatchMode=yes",
            "-o",
            &timeout,
            "-o",
            "UpdateHostKeys=no",
            "--",
            alias,
            "uname",
            "-s",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window, ever.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().ok()?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(st)) if st.success() => break,
                Ok(Some(_)) | Err(_) => return None,
                Ok(None) if start.elapsed() > SSH_LIMIT => {
                    let _ = child.kill();
                    let _ = child.wait(); // sync-ok: the system lookup's worker reaps the ssh it ended
                    return None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)), // sync-ok: the system lookup's worker
            }
        }
        let mut out = String::new();
        child.stdout.take()?.read_to_string(&mut out).ok()?; // sync-ok: ssh has ended; its few bytes are read
        Some(out)
    }
}

/// The cache file in a profile folder.
pub fn cache_file(profile: &Path) -> PathBuf {
    profile.join(CACHE_FILE)
}

/// Pure: the systems a cache file holds, with the time each was found (seconds since 1970). An entry that is not
/// whole, or names no system, is left out.
pub fn parse_cache(raw: &str) -> HashMap<HostKey, (MachineOs, u64)> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return HashMap::new();
    };
    v.get("hosts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let alias = e.get("alias")?.as_str()?;
            let host_name = e.get("host_name")?.as_str()?;
            let os = MachineOs::from_word(e.get("os")?.as_str()?)?;
            let at = e.get("found_at").and_then(Value::as_u64).unwrap_or(0);
            Some(((alias.to_string(), host_name.to_string()), (os, at)))
        })
        .collect()
}

/// Pure: the cache file's text, its hosts in name order.
pub fn cache_json(map: &HashMap<HostKey, (MachineOs, u64)>) -> String {
    let mut list: Vec<_> = map.iter().collect();
    list.sort_by(|a, b| a.0.cmp(b.0));
    let hosts: Vec<Value> = list
        .into_iter()
        .map(|((alias, host_name), (os, at))| {
            json!({"alias": alias, "host_name": host_name, "os": os.word(), "found_at": at})
        })
        .collect();
    serde_json::to_string_pretty(&json!({ "hosts": hosts })).unwrap_or_default()
}

/// BLOCKING (a file read): the cache, empty when there is none.
pub fn read_cache(path: &Path) -> HashMap<HostKey, (MachineOs, u64)> {
    std::fs::read_to_string(path) // sync-ok: the Machines page's worker reads the cache
        .map(|raw| parse_cache(&raw))
        .unwrap_or_default()
}

/// BLOCKING (a file read and write): the cache with what was just found added, at `now`; what was not found again
/// keeps its entry. Written whole at once.
pub fn remember(path: &Path, found: &[(HostKey, MachineOs)], now: u64) -> Result<(), String> {
    if found.is_empty() {
        return Ok(());
    }
    let mut map = read_cache(path);
    for (key, os) in found {
        map.insert(key.clone(), (*os, now));
    }
    flare_settings::atomic::write_atomic(path, &cache_json(&map))
        .map_err(|e| format!("{CACHE_FILE} could not be saved: {e}"))
}

/// Seconds since 1970, now.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Rule 1278: the greeting of each kind of server, as the real ones say it.
    #[test]
    fn a_greeting_names_windows_or_a_linux_or_nothing() {
        let table: [(&str, Option<MachineOs>); 12] = [
            ("SSH-2.0-OpenSSH_for_Windows_9.5\r\n", Some(MachineOs::Pc)),
            ("SSH-2.0-OpenSSH_for_Windows_8.1", Some(MachineOs::Pc)),
            (
                "SSH-2.0-OpenSSH_8.9p1 Ubuntu-3ubuntu0.10\r\n",
                Some(MachineOs::Linux),
            ),
            (
                "SSH-2.0-OpenSSH_9.2p1 Debian-2+deb12u3\r\n",
                Some(MachineOs::Linux),
            ),
            (
                "SSH-2.0-OpenSSH_7.9p1 Raspbian-10+deb10u2\r\n",
                Some(MachineOs::Linux),
            ),
            // A line before the greeting is allowed (RFC 4253).
            (
                "Welcome\r\nSSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13\r\n",
                Some(MachineOs::Linux),
            ),
            // A Mac, or a Linux that does not say: a plain OpenSSH says nothing.
            ("SSH-2.0-OpenSSH_9.8\r\n", None),
            ("SSH-2.0-OpenSSH_9.7 FreeBSD-20240318\r\n", None),
            // A word that only holds a distribution's name is not it.
            ("SSH-2.0-OpenSSH_9.8 archive\r\n", None),
            ("HTTP/1.1 400 Bad Request\r\n", None),
            ("\u{0}\u{1}garbage", None),
            ("", None),
        ];
        for (text, want) in table {
            assert_eq!(os_of_banner(text), want, "{text:?}");
        }
    }

    #[test]
    fn uname_names_a_mac_a_linux_or_a_windows_shell() {
        assert_eq!(os_of_uname("Darwin\n"), Some(MachineOs::Mac));
        assert_eq!(os_of_uname("\r\nLinux\r\n"), Some(MachineOs::Linux));
        assert_eq!(os_of_uname("MINGW64_NT-10.0-26100\n"), Some(MachineOs::Pc));
        assert_eq!(os_of_uname("MSYS_NT-10.0\n"), Some(MachineOs::Pc));
        assert_eq!(os_of_uname("FreeBSD\n"), None);
        assert_eq!(os_of_uname(""), None);
        assert_eq!(os_of_uname("'uname' is not recognized"), None);
    }

    /// The looks a test gives: a greeting or ssh's answer per host, and what was asked.
    #[derive(Default)]
    struct Looks {
        banners: HashMap<(String, u16), String>,
        unames: HashMap<String, String>,
        asked: Mutex<Vec<String>>,
    }

    impl OsLookup for Looks {
        fn banner(&self, host: &str, port: u16) -> Option<String> {
            self.asked
                .lock()
                .unwrap()
                .push(format!("banner {host}:{port}"));
            self.banners.get(&(host.to_string(), port)).cloned()
        }
        fn uname(&self, alias: &str) -> Option<String> {
            self.asked.lock().unwrap().push(format!("uname {alias}"));
            self.unames.get(alias).cloned()
        }
    }

    /// The greeting is read at the host's HostName and Port; ssh is asked only when it says nothing, by the host's
    /// name; a host that answers neither has no system.
    #[test]
    fn the_greeting_comes_first_and_ssh_only_when_it_says_nothing() {
        let mut looks = Looks::default();
        looks.banners.insert(
            ("192.168.0.139".into(), 2222),
            "SSH-2.0-OpenSSH_for_Windows_9.5\r\n".into(),
        );
        looks.banners.insert(
            ("192.168.0.120".into(), 22),
            "SSH-2.0-OpenSSH_9.8\r\n".into(),
        );
        looks.unames.insert("mac".into(), "Darwin\n".into());
        let evo = SshHost {
            alias: "evo-x2".into(),
            host_name: "192.168.0.139".into(),
            port: Some(2222),
            os: None,
        };
        let mac = SshHost {
            alias: "mac".into(),
            host_name: "192.168.0.120".into(),
            ..SshHost::default()
        };
        assert_eq!(find_os(&looks, &evo), Some(MachineOs::Pc));
        assert_eq!(find_os(&looks, &mac), Some(MachineOs::Mac));
        assert_eq!(find_os(&looks, &SshHost::plain("nas")), None);
        assert_eq!(
            *looks.asked.lock().unwrap(),
            [
                "banner 192.168.0.139:2222",
                "banner 192.168.0.120:22",
                "uname mac",
                "banner nas:22",
                "uname nas",
            ]
        );
    }

    /// The cache keeps each host by its name and HostName, with when it was found; a new find adds to it and leaves
    /// the others as they were; a broken file or entry is no system.
    #[test]
    fn the_cache_is_written_whole_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_file(dir.path());
        assert!(read_cache(&path).is_empty(), "no file, no systems");
        let evo: HostKey = ("evo-x2".into(), "192.168.0.139".into());
        let pi: HostKey = ("pi".into(), "pi".into());
        remember(&path, &[(evo.clone(), MachineOs::Pc)], 100).unwrap();
        remember(&path, &[(pi.clone(), MachineOs::Linux)], 200).unwrap();
        let got = read_cache(&path);
        assert_eq!(got.get(&evo), Some(&(MachineOs::Pc, 100)));
        assert_eq!(got.get(&pi), Some(&(MachineOs::Linux, 200)));
        // Found again: the time moves on. Nothing found: the file is left as it was.
        remember(&path, &[(evo.clone(), MachineOs::Pc)], 300).unwrap();
        remember(&path, &[], 400).unwrap();
        let got = read_cache(&path);
        assert_eq!(got.get(&evo), Some(&(MachineOs::Pc, 300)));
        assert_eq!(got.len(), 2);
        // The same name reaching another computer is another entry.
        assert!(!got.contains_key(&("evo-x2".to_string(), "10.0.0.9".to_string())));
        assert!(parse_cache("not json").is_empty());
        let odd = r#"{"hosts":[{"alias":"a","host_name":"a","os":"amiga"},{"alias":"b","os":"pc"},
            {"alias":"c","host_name":"c","os":"mac"}]}"#;
        let got = parse_cache(odd);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got.get(&("c".to_string(), "c".to_string())),
            Some(&(MachineOs::Mac, 0))
        );
    }
}
