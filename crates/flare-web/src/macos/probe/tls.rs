//! An HTTPS server on loopback with a self-signed certificate (made for the run by the system's `openssl`, which
//! listens on nothing), so the certificate check sees the browser's error page, the log's `web-cert` note and that
//! no override lets the request through (rule 1172): the server counts every request that got past the handshake.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub struct TlsServer {
    pub port: u16,
    /// Requests read after a finished handshake (a browser that refused the certificate sends none).
    pub served: Arc<AtomicU64>,
}

impl TlsServer {
    pub fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.port)
    }
}

/// Makes the certificate and its key in `dir` and starts the server.
pub fn start(dir: &Path) -> Result<TlsServer, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (cert, key, cnf) = (
        dir.join("cert.pem"),
        dir.join("key.pem"),
        dir.join("req.cnf"),
    );
    // A version 3 certificate for the address (rustls takes no other), signed by itself.
    std::fs::write(
        &cnf,
        "[req]\ndistinguished_name=dn\nx509_extensions=v3\nprompt=no\n[dn]\nCN=127.0.0.1\n\
         [v3]\nsubjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\n",
    )
    .map_err(|e| e.to_string())?;
    let made = Command::new("/usr/bin/openssl")
        .args([
            "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-config",
        ])
        .arg(&cnf)
        .arg("-keyout")
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("openssl: {e}"))?;
    if !made.success() {
        return Err(format!("openssl req ended {made}"));
    }
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&cert)
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let key = PrivateKeyDer::from_pem_file(&key).map_err(|e| e.to_string())?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let served = Arc::new(AtomicU64::new(0));
    let count = Arc::clone(&served);
    std::thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let (config, count) = (Arc::clone(&config), Arc::clone(&count));
            std::thread::spawn(move || {
                let _ = tcp.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let Ok(conn) = rustls::ServerConnection::new(config) else {
                    return;
                };
                let mut tls = rustls::StreamOwned::new(conn, tcp);
                let mut line = String::new();
                if BufReader::new(&mut tls).read_line(&mut line).is_err() || line.is_empty() {
                    return;
                }
                count.fetch_add(1, Ordering::SeqCst);
                let body = "<!doctype html><title>Let through</title>";
                let _ = write!(
                    tls,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.flush();
            });
        }
    });
    Ok(TlsServer { port, served })
}
