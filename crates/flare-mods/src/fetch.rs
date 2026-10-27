//! Install from an address (plugins phase 7, design 13): the one place plugins reach the network from Flare itself.
//!
//! Only when the person pastes an `https` address in Settings > Plugins and presses Install. The zip is downloaded on
//! the install worker to a file in the staging folder, then installed exactly as a file would be. The rules:
//!
//! - Every address, the first and each redirect (at most [`MAX_REDIRECTS`]), must pass the caller's address rule (the
//!   app gives `flare_surfaces::net::refusal`, phase 6's: `https` only, a public host, no user name or password). A
//!   redirect is followed here, one hop at a time, so none is taken without the rule.
//! - The resolver refuses a name that resolves to a loopback, private or link-local address (the caller's address
//!   rule decides each address), so a public name pointed at this PC or its network is never reached.
//! - The bytes are counted while they stream: more than [`crate::install::MAX_ZIP_BYTES`] stops the download. The
//!   whole download has [`TIMEOUT`].
//! - No cookies (ureq keeps none without its `cookies` feature), no proxy from the environment, not the web panel's
//!   browser or profile. The address is recorded with the approval as the plugin's source.

use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

/// Redirects followed at most.
pub const MAX_REDIRECTS: usize = 5;
/// The whole download, redirects included.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// Why an address may not be reached, or None.
pub type AddressRule = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// Why a resolved address may not be reached, or None.
pub type IpRule = Arc<dyn Fn(IpAddr) -> Option<String> + Send + Sync>;

/// The rules an address and a resolved address must pass: each answers why not, or None.
#[derive(Clone)]
pub struct Rules {
    pub address: AddressRule,
    pub ip: IpRule,
    pub max_bytes: u64,
    pub timeout: Duration,
}

impl std::fmt::Debug for Rules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rules")
            .field("max_bytes", &self.max_bytes)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

/// The resolver that hands ureq only the addresses the rules allow.
struct Walled {
    inner: DefaultResolver,
    ip: IpRule,
}

impl std::fmt::Debug for Walled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Walled")
    }
}

impl Resolver for Walled {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let all = self.inner.resolve(uri, config, timeout)?;
        let mut kept = self.inner.empty();
        for a in all.iter() {
            match (self.ip)(a.ip()) {
                None => kept.push(*a),
                Some(why) => {
                    log::info!("plugin download: an address of the host is refused: {why}")
                }
            }
        }
        if kept.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(kept)
    }
}

/// Downloads `url` into `dest` (a new file) under `rules`, following redirects one hop at a time. A failure is one
/// sentence for Settings; a partial file is the caller's to delete.
pub fn download(url: &str, dest: &Path, rules: &Rules) -> Result<(), String> {
    let config = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .proxy(None)
        .timeout_global(Some(rules.timeout))
        .build();
    let agent = ureq::Agent::with_parts(
        config,
        DefaultConnector::default(),
        Walled {
            inner: DefaultResolver::default(),
            ip: Arc::clone(&rules.ip),
        },
    );
    let mut at = url.to_string();
    let mut hops = 0;
    let response = loop {
        if let Some(why) = (rules.address)(&at) {
            return Err(if hops == 0 {
                format!("That address is refused: {why}.")
            } else {
                format!("The address redirected to one that is refused: {why}.")
            });
        }
        let r = agent.get(&at).call().map_err(|e| match e {
            ureq::Error::HostNotFound => {
                "The host was not found, or it is on this PC or its local network.".to_string()
            }
            ureq::Error::Timeout(_) => "The download took longer than a minute.".to_string(),
            other => format!("The download failed: {other}."),
        })?;
        let status = r.status().as_u16();
        if (300..400).contains(&status) {
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(format!(
                    "The address redirected more than {MAX_REDIRECTS} times."
                ));
            }
            let Some(next) = r
                .headers()
                .get("location")
                .and_then(|l| l.to_str().ok())
                .and_then(|l| url::Url::parse(&at).ok()?.join(l).ok())
            else {
                return Err("The address redirected nowhere.".to_string());
            };
            at = next.to_string();
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(format!("The server answered {status}."));
        }
        break r;
    };
    if let Some(len) = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        && len > rules.max_bytes
    {
        return Err(too_large(rules.max_bytes));
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest) // sync-ok: the install worker
        .map_err(|e| format!("The download cannot be saved: {e}."))?;
    let mut body = response
        .into_body()
        .into_with_config()
        .limit(rules.max_bytes + 1)
        .reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut got: u64 = 0;
    loop {
        let n = match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if got >= rules.max_bytes => {
                log::debug!("plugin download stopped at the limit: {e}");
                return Err(too_large(rules.max_bytes));
            }
            Err(e) => return Err(format!("The download failed: {e}.")),
        };
        got += n as u64;
        if got > rules.max_bytes {
            return Err(too_large(rules.max_bytes));
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("The download cannot be saved: {e}."))?;
    }
    file.flush()
        .map_err(|e| format!("The download cannot be saved: {e}."))?;
    Ok(())
}

fn too_large(max: u64) -> String {
    format!("The file is larger than {} MB.", max / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Rules for a loopback test server: `http` allowed, every address allowed.
    fn open_rules(max: u64) -> Rules {
        Rules {
            address: Arc::new(|_: &str| None),
            ip: Arc::new(|_: IpAddr| None),
            max_bytes: max,
            timeout: Duration::from_secs(10),
        }
    }

    /// A loopback server that answers each of `replies` to one request, in order.
    fn serve(replies: Vec<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for reply in replies {
                if let Ok((mut s, _)) = listener.accept() {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(&reply);
                }
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn ok_body(body: &[u8]) -> Vec<u8> {
        let mut r = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        r.extend_from_slice(body);
        r
    }

    fn redirect(to: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 302 Found\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn a_body_is_saved_and_one_over_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(vec![ok_body(b"PK-zip-bytes")]);
        let dest = dir.path().join("a.zip");
        download(&format!("{base}/a.zip"), &dest, &open_rules(1024)).expect("downloaded");
        assert_eq!(std::fs::read(&dest).unwrap(), b"PK-zip-bytes");

        let base = serve(vec![ok_body(&[7u8; 4096])]);
        let e = download(
            &format!("{base}/b.zip"),
            &dir.path().join("b.zip"),
            &open_rules(1000),
        )
        .unwrap_err();
        assert!(e.contains("larger than"), "{e}");

        // No length given: the bytes are counted as they come.
        let mut chunked = b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_vec();
        chunked.extend_from_slice(&[1u8; 5000]);
        let base = serve(vec![chunked]);
        let e = download(
            &format!("{base}/c.zip"),
            &dir.path().join("c.zip"),
            &open_rules(1000),
        )
        .unwrap_err();
        assert!(e.contains("larger than"), "{e}");
    }

    #[test]
    fn every_redirect_meets_the_address_rule_and_at_most_five_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        // A redirect to a refused address is not followed.
        let base = serve(vec![redirect("http://refused.example/x.zip")]);
        let mut rules = open_rules(1024);
        rules.address =
            Arc::new(|u: &str| u.contains("refused").then(|| "not this one".to_string()));
        let e = download(&format!("{base}/a"), &dir.path().join("a.zip"), &rules).unwrap_err();
        assert!(e.contains("redirected to one that is refused"), "{e}");

        // Six redirects in a row are one too many.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for i in 0..7 {
                if let Ok((mut s, _)) = listener.accept() {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(&redirect(&format!("/r{i}")));
                }
            }
        });
        let e = download(
            &format!("http://127.0.0.1:{port}/start"),
            &dir.path().join("b.zip"),
            &open_rules(1024),
        )
        .unwrap_err();
        assert!(e.contains("more than 5 times"), "{e}");

        // A relative redirect on the same host is followed.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for reply in [redirect("/real.zip"), ok_body(b"zip")] {
                if let Ok((mut s, _)) = listener.accept() {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(&reply);
                }
            }
        });
        let dest = dir.path().join("c.zip");
        download(
            &format!("http://127.0.0.1:{port}/a"),
            &dest,
            &open_rules(1024),
        )
        .unwrap();
        assert_eq!(std::fs::read(dest).unwrap(), b"zip");
    }

    #[test]
    fn a_host_whose_every_address_is_refused_is_not_reached() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(vec![ok_body(b"never")]);
        let mut rules = open_rules(1024);
        rules.ip = Arc::new(|ip: IpAddr| ip.is_loopback().then(|| "loopback".to_string()));
        let e = download(&format!("{base}/a.zip"), &dir.path().join("a.zip"), &rules).unwrap_err();
        assert!(e.contains("not found"), "{e}");
    }

    #[test]
    fn a_server_error_is_one_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(vec![
            b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
        ]);
        let e = download(
            &format!("{base}/a.zip"),
            &dir.path().join("a.zip"),
            &open_rules(10),
        )
        .unwrap_err();
        assert_eq!(e, "The server answered 404.");
    }
}
