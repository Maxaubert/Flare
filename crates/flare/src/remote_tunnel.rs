//! The Preview chip of a remote tab (rule 1213): a local server address a remote tab prints (`http://localhost:5173`)
//! is the host's, so a click opens an ssh tunnel to the host first and the side panel shows the page through it.
//!
//! The tunnel is `ssh -N -o BatchMode=yes -o ExitOnForwardFailure=yes -L 127.0.0.1:<local>:localhost:<port> -- <host>`,
//! built by [`tunnel_argv`] from the same machine entry the link uses: every part an argv item of its own, never a
//! shell string, no prompt, nothing run on the host. The local port is a free one on this machine's loopback only.
//! One tunnel per remote tab and host port, reused while the chip stands; it closes when the tab closes or the chip
//! goes (the window drops its [`Tunnel`]). A tunnel that does not come up in [`TUNNEL_WAIT`] is closed and said in one
//! line, never waited on by the UI thread (rule 34): the window opens it on a worker.
//!
//! Under `--e2e` no ssh is ever run: [`open_loopback`] stands in, a forwarder on this machine's loopback to the same
//! port, so a scenario's own server shows through the same code path.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::machines::{Machine, argv_word};

/// How long a tunnel may take to come up (ssh's login included) before it is closed and said.
pub const TUNNEL_WAIT: Duration = Duration::from_secs(20);

/// An open tunnel: the local port it listens on. Dropping it closes it (off the UI thread).
pub struct Tunnel {
    pub local_port: u16,
    closer: Option<Box<dyn FnOnce() + Send>>,
}

impl Tunnel {
    pub fn new(local_port: u16, closer: Box<dyn FnOnce() + Send>) -> Self {
        Self {
            local_port,
            closer: Some(closer),
        }
    }
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tunnel({})", self.local_port)
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Some(c) = self.closer.take() {
            c();
        }
    }
}

/// What a tunnel is opened to: the machine of the remote tab and the port its server prints on the host.
#[derive(Clone, Debug)]
pub struct TunnelAsk {
    pub machine: Machine,
    pub host_port: u16,
}

/// Opens a tunnel. BLOCKING: a worker's call. Production: [`open_ssh`]; `--e2e`: [`open_loopback`]; the tests a fake.
pub type TunnelOpener = Arc<dyn Fn(&TunnelAsk) -> Result<Tunnel, String> + Send + Sync>;

/// Pure: ssh's argv for the tunnel (rule 1213): the link's own words for the machine (`argv_word` host and user, `--`
/// before the host), no command, no prompt, the forward on this machine's loopback only, and ssh ends when the
/// forward cannot be made. None for a machine or a port that cannot be one.
pub fn tunnel_argv(ssh: &str, m: &Machine, local_port: u16, host_port: u16) -> Option<Vec<String>> {
    if !argv_word(&m.ssh) || m.user.as_deref().is_some_and(|u| !argv_word(u)) {
        return None;
    }
    if local_port == 0 || host_port == 0 {
        return None;
    }
    let mut argv = vec![
        ssh.to_string(),
        "-N".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-L".into(),
        format!("127.0.0.1:{local_port}:localhost:{host_port}"),
    ];
    if let Some(u) = &m.user {
        argv.push("-l".into());
        argv.push(u.clone());
    }
    argv.push("--".into());
    argv.push(m.ssh.clone());
    Some(argv)
}

/// Pure: the address the side panel opens for a chip's address once its tunnel listens on `local_port`: the same
/// scheme, path and query on this machine's loopback. None for anything that is no http or https address.
pub fn tunnel_url(chip_url: &str, local_port: u16) -> Option<String> {
    let (scheme, rest) = chip_url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let cut = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[..cut].is_empty() || local_port == 0 {
        return None;
    }
    let tail = &rest[cut..];
    let tail = if tail.is_empty() || !tail.starts_with('/') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    Some(format!(
        "{}://127.0.0.1:{local_port}{tail}",
        scheme.to_ascii_lowercase()
    ))
}

/// A free port on this machine's loopback, as the system hands one out.
pub fn free_port() -> std::io::Result<u16> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(l.local_addr()?.port())
}

/// Whether something listens on this machine's loopback at `port`.
fn listens(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(200),
    )
    .is_ok()
}

/// BLOCKING (it starts ssh and waits for the forward): the real tunnel. A worker's call.
pub fn open_ssh(ask: &TunnelAsk, wait: Duration) -> Result<Tunnel, String> {
    let name = &ask.machine.name;
    let local = free_port().map_err(|e| format!("no free port here for the tunnel: {e}"))?;
    let argv = tunnel_argv(
        &crate::remote_link::ssh_exe(),
        &ask.machine,
        local,
        ask.host_port,
    )
    .ok_or_else(|| format!("cannot build the ssh command for {name}"))?;
    let (exe, args) = argv.split_first().expect("an argv has its program");
    let mut cmd = std::process::Command::new(exe); // sync-ok: a worker starts the tunnel, never the UI thread
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window, ever.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start ssh for the tunnel to {name}: {e}"))?;
    let end = Instant::now() + wait;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the tunnel to {name} did not open (ssh ended: {status})"
            ));
        }
        if listens(local) {
            break;
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait(); // sync-ok: a worker, after a kill: the child is gone at once
            return Err(format!(
                "the tunnel to {name} did not open in {} s",
                wait.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(100)); // sync-ok: a worker waits for the tunnel, capped by `wait`
    }
    Ok(Tunnel::new(
        local,
        Box::new(move || {
            // The kill is at once (so a quit leaves no ssh behind); only the wait for its end is on a thread.
            let _ = child.kill();
            let _ = std::thread::Builder::new()
                .name("flare-tunnel-close".into())
                .spawn(move || {
                    let _ = child.wait(); // sync-ok: a thread of its own, after a kill
                });
        }),
    ))
}

/// Copies one way until either end closes.
fn pump(mut from: TcpStream, mut to: TcpStream) {
    let mut buf = [0u8; 16 * 1024];
    while let Ok(n) = from.read(&mut buf) {
        if n == 0 || to.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(std::net::Shutdown::Write);
}

/// BLOCKING (it binds a port): the `--e2e` stand-in for ssh (rule 1213): a forwarder on this machine's loopback from a
/// free port to `host_port` on the same loopback. Never a real ssh, never another machine.
pub fn open_loopback(ask: &TunnelAsk) -> Result<Tunnel, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| format!("no free port here for the tunnel: {e}"))?;
    let local = listener
        .local_addr()
        .map_err(|e| format!("no free port here for the tunnel: {e}"))?
        .port();
    let to = ask.host_port;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("flare-tunnel-loopback".into())
        .spawn(move || {
            for conn in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(client) = conn else { continue };
                let Ok(server) = TcpStream::connect((Ipv4Addr::LOCALHOST, to)) else {
                    continue;
                };
                if let (Ok(c2), Ok(s2)) = (client.try_clone(), server.try_clone()) {
                    std::thread::spawn(move || pump(client, server));
                    std::thread::spawn(move || pump(s2, c2));
                }
            }
        })
        .map_err(|e| format!("could not start the tunnel: {e}"))?;
    Ok(Tunnel::new(
        local,
        Box::new(move || {
            stop.store(true, Ordering::SeqCst);
            // Wakes the accept so the forwarder sees the stop and ends.
            let _ = TcpStream::connect_timeout(
                &SocketAddr::from((Ipv4Addr::LOCALHOST, local)),
                Duration::from_millis(200),
            );
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(user: Option<&str>) -> Machine {
        Machine {
            id: "mac".into(),
            name: "Mac Mini".into(),
            kind: Some(flare_tabs::tabs::Os::Mac),
            ssh: "mini".into(),
            user: user.map(str::to_string),
            flare: "flare".into(),
        }
    }

    /// Rule 1213: the tunnel's argv is the link's machine entry, every part an item of its own: no command, no prompt,
    /// the forward on the loopback only, `--` before the host; a host or a user that could be an option is refused.
    #[test]
    fn the_tunnel_argv_is_the_links_machine_and_never_a_shell_string() {
        assert_eq!(
            tunnel_argv("ssh.exe", &mac(Some("max")), 50123, 5173).unwrap(),
            [
                "ssh.exe",
                "-N",
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-L",
                "127.0.0.1:50123:localhost:5173",
                "-l",
                "max",
                "--",
                "mini"
            ]
        );
        let argv = tunnel_argv("ssh", &mac(None), 1, 2).unwrap();
        assert!(!argv.contains(&"-l".to_string()));
        assert_eq!(argv.last().map(String::as_str), Some("mini"));
        let mut bad = mac(None);
        bad.ssh = "-oProxyCommand=calc".into();
        assert_eq!(tunnel_argv("ssh", &bad, 1, 2), None);
        assert_eq!(tunnel_argv("ssh", &mac(Some("a b")), 1, 2), None);
        assert_eq!(tunnel_argv("ssh", &mac(None), 0, 2), None);
        assert_eq!(tunnel_argv("ssh", &mac(None), 1, 0), None);
    }

    /// Rule 1213: the panel opens the chip's address on this machine's loopback at the tunnel's port, path and query
    /// kept; anything that is no web address maps to nothing.
    #[test]
    fn a_chips_address_maps_to_the_tunnels_port_on_the_loopback() {
        assert_eq!(
            tunnel_url("http://localhost:5173/", 50123).as_deref(),
            Some("http://127.0.0.1:50123/")
        );
        assert_eq!(
            tunnel_url("http://localhost:5173", 50123).as_deref(),
            Some("http://127.0.0.1:50123/")
        );
        assert_eq!(
            tunnel_url("HTTPS://127.0.0.1:8443/app?x=1#top", 7).as_deref(),
            Some("https://127.0.0.1:7/app?x=1#top")
        );
        assert_eq!(
            tunnel_url("http://[::1]:3000?q", 9).as_deref(),
            Some("http://127.0.0.1:9/?q")
        );
        for bad in [
            "file:///C:/x",
            "localhost:5173",
            "http:///x",
            "javascript://x",
        ] {
            assert_eq!(tunnel_url(bad, 9), None, "{bad}");
        }
        assert_eq!(tunnel_url("http://localhost:1/", 0), None);
    }

    /// Rule 1213's e2e stand-in: the loopback tunnel carries a request to the server's port and back, and once dropped
    /// it takes no new connection.
    #[test]
    fn the_loopback_tunnel_carries_a_request_and_closes_when_dropped() {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = server.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = server.accept() {
                let mut buf = [0u8; 5];
                let _ = s.read_exact(&mut buf);
                let _ = s.write_all(b"pong:");
                let _ = s.write_all(&buf);
            }
        });
        let t = open_loopback(&TunnelAsk {
            machine: mac(None),
            host_port: port,
        })
        .unwrap();
        let mut c = TcpStream::connect((Ipv4Addr::LOCALHOST, t.local_port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"hello").unwrap();
        let mut got = [0u8; 10];
        c.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"pong:hello");
        let local = t.local_port;
        drop(t);
        let end = Instant::now() + Duration::from_secs(5);
        while listens(local) {
            assert!(Instant::now() < end, "the tunnel still listens");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
