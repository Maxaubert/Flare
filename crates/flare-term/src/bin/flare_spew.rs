//! Test helper: writes lines to stdout until `MB * 1_000_000` bytes have gone (`MB = 0` means
//! forever), then `SPEW-DONE`. Each line is 111 bytes, written in 56 KB chunks. A write error (the
//! reader went away) ends it quietly.

use std::io::Write;

const BODY: &str = "The quick brown fox jumps over the lazy dog 0123456789 The quick brown fox jumps over the lazy dog 0123456789";

fn main() {
    let mb: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let forever = mb == 0;
    let mut chunk = String::with_capacity(500 * 120);
    for i in 0..500 {
        chunk.push_str(&format!("{i:07} {BODY}\r\n"));
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let target = mb.saturating_mul(1_000_000);
    let mut written: u64 = 0;
    while forever || written < target {
        if out.write_all(chunk.as_bytes()).is_err() {
            return;
        }
        written += chunk.len() as u64;
    }
    if out.write_all(b"SPEW-DONE\r\n").is_err() {
        return;
    }
    let _ = out.flush();
}
