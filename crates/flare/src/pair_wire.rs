//! Machine pairing PR 3 (rule 1243): the pairing wire. A viewer and a host talk over one TCP connection in frames (a
//! 4-byte big-endian length, then at most [`MAX_FRAME`] bytes of JSON). The 6-digit code never crosses it:
//!
//! 1. viewer: `{"t":"hello","proto":1,"name","os","version"}`; host: `{"t":"shown"}` once its dialog shows a fresh code
//!    (or `busy`, `declined`, `proto`, and the connection ends).
//! 2. One attempt: viewer `{"t":"spake","msg"}` (SPAKE2 side A with the typed code), host `{"t":"spake","msg"}` (side
//!    B with its code). The identities are bound to the protocol label ([`ID_VIEWER`], [`ID_HOST`]). Both hash the
//!    shared key with HKDF-SHA256 into two confirmation keys and two channel keys. The viewer sends
//!    `{"t":"confirm","mac"}` (HMAC-SHA256 over both SPAKE2 messages); a wrong one is a wrong code: the host answers
//!    `{"t":"wrong","left"}` and the attempt is over. After [`MAX_TRIES`] wrong codes the host ends the connection
//!    (its window closes). A right one is answered with the host's own confirmation, which the viewer checks.
//! 3. From then on every frame is sealed with ChaCha20-Poly1305 (a key per direction, the frame's number as nonce).
//!
//! Everything here works on any `Read + Write`, so the tests run it over an in-memory pipe.

use std::io::{Read, Write};

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

/// The pairing protocol's number: both ends must speak the same one.
pub const PROTOCOL: u64 = 1;
/// The SPAKE2 identities, bound to the protocol.
pub const ID_VIEWER: &[u8] = b"flare-pair-v1 viewer";
pub const ID_HOST: &[u8] = b"flare-pair-v1 host";
/// The largest frame either end reads.
pub const MAX_FRAME: usize = 16 * 1024;
/// Wrong codes before the host closes its window.
pub const MAX_TRIES: u32 = 3;
/// The code's digits.
pub const CODE_LEN: usize = 6;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Writes one frame.
pub fn write_frame<W: Write + ?Sized>(w: &mut W, bytes: &[u8]) -> std::io::Result<()> {
    if bytes.len() > MAX_FRAME {
        return Err(std::io::Error::other("a frame too long"));
    }
    let mut buf = Vec::with_capacity(4 + bytes.len());
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
    w.write_all(&buf)?;
    w.flush()
}

/// Reads one frame; a length over [`MAX_FRAME`] ends the connection.
pub fn read_frame<R: Read + ?Sized>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(std::io::Error::other("a frame too long"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn send<W: Write + ?Sized>(w: &mut W, v: &Value) -> std::io::Result<()> {
    write_frame(w, v.to_string().as_bytes())
}

pub fn recv<R: Read + ?Sized>(r: &mut R) -> std::io::Result<Value> {
    let f = read_frame(r)?;
    serde_json::from_slice(&f).map_err(|_| std::io::Error::other("a frame that is no JSON"))
}

/// A frame's kind (`t`).
pub fn kind(v: &Value) -> &str {
    v.get("t").and_then(Value::as_str).unwrap_or_default()
}

/// Pure: the digits of what was typed (spaces and the like dropped), when there are exactly six.
pub fn code_digits(typed: &str) -> Option<String> {
    let d: String = typed.chars().filter(char::is_ascii_digit).collect();
    let other = typed
        .chars()
        .any(|c| !c.is_ascii_digit() && !c.is_whitespace() && c != '-');
    (d.len() == CODE_LEN && !other).then_some(d)
}

/// Pure: a code as the host shows it, `482 913`.
pub fn code_shown(code: &str) -> String {
    if code.len() == CODE_LEN {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// A fresh code from the OS's random bytes: six digits, every value as likely as another.
pub fn fresh_code() -> Result<String, String> {
    loop {
        let mut b = [0u8; 4];
        getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
        let n = u32::from_be_bytes(b);
        // 4_294_000_000 is a multiple of 1_000_000: a draw above it is drawn again, so no code is likelier.
        if n < 4_294_000_000 {
            return Ok(format!("{:06}", n % 1_000_000));
        }
    }
}

/// The keys one confirmed attempt gives both ends.
struct Keys {
    confirm_viewer: [u8; 32],
    confirm_host: [u8; 32],
    seal_viewer: [u8; 32],
    seal_host: [u8; 32],
}

fn keys_of(shared: &[u8]) -> Keys {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(b"flare-pair-v1"), shared);
    let out = |info: &[u8]| {
        let mut k = [0u8; 32];
        hk.expand(info, &mut k).expect("32 bytes is a valid length");
        k
    };
    Keys {
        confirm_viewer: out(b"confirm viewer"),
        confirm_host: out(b"confirm host"),
        seal_viewer: out(b"seal viewer"),
        seal_host: out(b"seal host"),
    }
}

fn mac_of(key: &[u8; 32], transcript: &[u8]) -> Vec<u8> {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
    m.update(transcript);
    m.finalize().into_bytes().to_vec()
}

/// Checks a MAC in constant time.
fn mac_ok(key: &[u8; 32], transcript: &[u8], got: &[u8]) -> bool {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
    m.update(transcript);
    m.verify_slice(got).is_ok()
}

fn b64_field(v: &Value, k: &str) -> Option<Vec<u8>> {
    v.get(k)
        .and_then(Value::as_str)
        .and_then(|s| B64.decode(s).ok())
}

fn bad(what: &str) -> std::io::Error {
    std::io::Error::other(what.to_string())
}

/// The sealed channel after a confirmed attempt: every frame is sealed with this end's key and opened with the other
/// end's, numbered from 0 each way (a frame replayed, dropped or moved fails to open).
pub struct Channel {
    seal: ChaCha20Poly1305,
    open: ChaCha20Poly1305,
    sent: u64,
    got: u64,
}

fn nonce(n: u64) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_be_bytes());
    b
}

impl Channel {
    fn new(seal: &[u8; 32], open: &[u8; 32]) -> Self {
        Self {
            seal: ChaCha20Poly1305::new(Key::from_slice(seal)),
            open: ChaCha20Poly1305::new(Key::from_slice(open)),
            sent: 0,
            got: 0,
        }
    }

    pub fn send<W: Write + ?Sized>(&mut self, w: &mut W, v: &Value) -> std::io::Result<()> {
        let sealed = self
            .seal
            .encrypt(
                Nonce::from_slice(&nonce(self.sent)),
                v.to_string().as_bytes(),
            )
            .map_err(|_| bad("could not seal"))?;
        self.sent += 1;
        write_frame(w, &sealed)
    }

    pub fn recv<R: Read + ?Sized>(&mut self, r: &mut R) -> std::io::Result<Value> {
        let f = read_frame(r)?;
        let plain = self
            .open
            .decrypt(Nonce::from_slice(&nonce(self.got)), f.as_ref())
            .map_err(|_| bad("a frame that does not open"))?;
        self.got += 1;
        serde_json::from_slice(&plain).map_err(|_| bad("a frame that is no JSON"))
    }
}

/// How one viewer attempt ended, when it did not give a channel.
#[derive(Debug, PartialEq, Eq)]
pub enum AttemptEnd {
    /// The host said the code was wrong; `left` tries remain (0: the host closed its window).
    Wrong { left: u32 },
    /// The host's own confirmation did not check out: no channel (someone in between, or a broken host).
    Unproved,
    /// The connection ended or said something else.
    Gone(String),
}

/// One viewer attempt with the typed `code` (rule 1243): SPAKE2 side A, then the confirmations.
pub fn viewer_attempt<S: Read + Write + ?Sized>(
    s: &mut S,
    code: &str,
) -> Result<Channel, AttemptEnd> {
    let gone = |e: std::io::Error| AttemptEnd::Gone(e.to_string());
    let (state, msg_a) = Spake2::<Ed25519Group>::start_a(
        &Password::new(code.as_bytes()),
        &Identity::new(ID_VIEWER),
        &Identity::new(ID_HOST),
    );
    send(s, &json!({"t": "spake", "msg": B64.encode(&msg_a)})).map_err(gone)?;
    let reply = recv(s).map_err(gone)?;
    if kind(&reply) != "spake" {
        return Err(AttemptEnd::Gone(format!("the host said {}", kind(&reply))));
    }
    let msg_b = b64_field(&reply, "msg").ok_or(AttemptEnd::Unproved)?;
    // A message that does not finish is the host's fault, not a wrong code; it still gives no key.
    let shared = state.finish(&msg_b).map_err(|_| AttemptEnd::Unproved)?;
    let keys = keys_of(&shared);
    let transcript = [msg_a.as_slice(), msg_b.as_slice()].concat();
    let mine = mac_of(&keys.confirm_viewer, &transcript);
    send(s, &json!({"t": "confirm", "mac": B64.encode(mine)})).map_err(gone)?;
    let answer = recv(s).map_err(gone)?;
    match kind(&answer) {
        "wrong" => Err(AttemptEnd::Wrong {
            left: answer
                .get("left")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(u64::from(MAX_TRIES)) as u32,
        }),
        "confirm" => {
            let theirs = b64_field(&answer, "mac").ok_or(AttemptEnd::Unproved)?;
            if mac_ok(&keys.confirm_host, &transcript, &theirs) {
                Ok(Channel::new(&keys.seal_viewer, &keys.seal_host))
            } else {
                Err(AttemptEnd::Unproved)
            }
        }
        other => Err(AttemptEnd::Gone(format!("the host said {other}"))),
    }
}

/// How the host's attempts ended, when they gave no channel.
#[derive(Debug, PartialEq, Eq)]
pub enum HostEnd {
    /// [`MAX_TRIES`] wrong codes: the window closes.
    TooMany,
    /// The connection ended (the viewer left, or Decline shut it).
    Gone(String),
}

/// The host's side of the attempts with its `code` (rule 1243): up to [`MAX_TRIES`] SPAKE2 runs; each wrong code is
/// answered `wrong` with the tries left and calls `strike` with them; the right one gives the channel.
pub fn host_attempts<S: Read + Write + ?Sized>(
    s: &mut S,
    code: &str,
    mut strike: impl FnMut(u32),
) -> Result<Channel, HostEnd> {
    let gone = |e: std::io::Error| HostEnd::Gone(e.to_string());
    let mut wrong = 0;
    loop {
        let first = recv(s).map_err(gone)?;
        if kind(&first) != "spake" {
            return Err(HostEnd::Gone(format!("the viewer said {}", kind(&first))));
        }
        let msg_a = b64_field(&first, "msg").ok_or_else(|| HostEnd::Gone("no message".into()))?;
        let (state, msg_b) = Spake2::<Ed25519Group>::start_b(
            &Password::new(code.as_bytes()),
            &Identity::new(ID_VIEWER),
            &Identity::new(ID_HOST),
        );
        send(s, &json!({"t": "spake", "msg": B64.encode(&msg_b)})).map_err(gone)?;
        let confirm = recv(s).map_err(gone)?;
        let theirs = b64_field(&confirm, "mac").unwrap_or_default();
        let transcript = [msg_a.as_slice(), msg_b.as_slice()].concat();
        // A message that does not finish counts as a wrong code: it is one more guess.
        let keys = state.finish(&msg_a).ok().map(|k| keys_of(&k));
        match keys {
            Some(keys)
                if kind(&confirm) == "confirm"
                    && mac_ok(&keys.confirm_viewer, &transcript, &theirs) =>
            {
                let mine = mac_of(&keys.confirm_host, &transcript);
                send(s, &json!({"t": "confirm", "mac": B64.encode(mine)})).map_err(gone)?;
                return Ok(Channel::new(&keys.seal_host, &keys.seal_viewer));
            }
            _ => {
                wrong += 1;
                let left = MAX_TRIES.saturating_sub(wrong);
                strike(left);
                send(s, &json!({"t": "wrong", "left": left})).map_err(gone)?;
                if left == 0 {
                    return Err(HostEnd::TooMany);
                }
            }
        }
    }
}

/// An in-memory pipe for the tests: two ends, each reads what the other wrote; dropping an end (or `close`) is the
/// other end's EOF.
#[cfg(test)]
pub(crate) mod pipe {
    use std::io::{Read, Write};
    use std::sync::mpsc::{Receiver, Sender, channel};

    pub(crate) struct End {
        tx: Sender<Vec<u8>>,
        rx: Receiver<Vec<u8>>,
        /// A sender into this end's own reader: `close` wakes a read that waits.
        me: Sender<Vec<u8>>,
        buf: Vec<u8>,
        closed: bool,
    }

    pub(crate) fn pair() -> (End, End) {
        let (a_tx, a_rx) = channel();
        let (b_tx, b_rx) = channel();
        (
            End {
                tx: b_tx.clone(),
                rx: a_rx,
                me: a_tx.clone(),
                buf: Vec::new(),
                closed: false,
            },
            End {
                tx: a_tx,
                rx: b_rx,
                me: b_tx,
                buf: Vec::new(),
                closed: false,
            },
        )
    }

    impl End {
        /// What wakes this end's read with an EOF from another thread.
        pub(crate) fn waker(&self) -> Sender<Vec<u8>> {
            self.me.clone()
        }
    }

    impl Drop for End {
        fn drop(&mut self) {
            // The other end's read sees an EOF.
            let _ = self.tx.send(Vec::new());
        }
    }

    impl Read for End {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.buf.is_empty() && !self.closed {
                match self.rx.recv() {
                    // An empty chunk is the closer's EOF.
                    Ok(c) if c.is_empty() => self.closed = true,
                    Ok(c) => self.buf = c,
                    Err(_) => self.closed = true,
                }
            }
            let n = out.len().min(self.buf.len());
            out[..n].copy_from_slice(&self.buf[..n]);
            self.buf.drain(..n);
            Ok(n)
        }
    }

    impl Write for End {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if b.is_empty() {
                return Ok(0);
            }
            self.tx
                .send(b.to_vec())
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 1243: frames are a length and the bytes; a frame over the limit is refused both ways.
    #[test]
    fn a_frame_is_a_length_and_its_bytes_and_a_long_one_is_refused() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"abc").unwrap();
        assert_eq!(buf, [0, 0, 0, 3, b'a', b'b', b'c']);
        assert_eq!(read_frame(&mut buf.as_slice()).unwrap(), b"abc");
        assert!(write_frame(&mut Vec::new(), &vec![0u8; MAX_FRAME + 1]).is_err());
        let mut long = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
        long.extend(vec![0u8; 8]);
        assert!(read_frame(&mut long.as_slice()).is_err());
        // A cut frame is an error, never a short read.
        assert!(read_frame(&mut [0u8, 0, 0, 5, 1, 2].as_slice()).is_err());
        let mut j = Vec::new();
        send(&mut j, &json!({"t": "hello", "proto": 1})).unwrap();
        assert_eq!(kind(&recv(&mut j.as_slice()).unwrap()), "hello");
        assert!(recv(&mut [0u8, 0, 0, 1, b'{'].as_slice()).is_err());
    }

    #[test]
    fn a_code_is_six_digits_shown_in_two_threes() {
        assert_eq!(code_digits("482 913").as_deref(), Some("482913"));
        assert_eq!(code_digits(" 482-913 ").as_deref(), Some("482913"));
        assert_eq!(code_digits("48291"), None);
        assert_eq!(code_digits("4829134"), None);
        assert_eq!(code_digits("48291a"), None);
        assert_eq!(code_shown("482913"), "482 913");
        for _ in 0..50 {
            let c = fresh_code().unwrap();
            assert_eq!(c.len(), 6);
            assert!(c.chars().all(|c| c.is_ascii_digit()));
        }
    }

    /// What the host's thread ends with: its answer, the tries left at each strike, and its end of the pipe.
    type HostRun = std::thread::JoinHandle<(Result<Channel, HostEnd>, Vec<u32>, pipe::End)>;

    /// Runs the host's attempts on a thread over a pipe; the viewer's end and the host's answer come back.
    fn host_on_pipe(code: &'static str) -> (pipe::End, HostRun) {
        let (v, mut h) = pipe::pair();
        let t = std::thread::spawn(move || {
            let mut strikes = Vec::new();
            let r = host_attempts(&mut h, code, |left| strikes.push(left));
            (r, strikes, h)
        });
        (v, t)
    }

    /// Rule 1243: the right code gives both ends the same sealed channel; a frame sealed one way opens the other way
    /// only, and the code never crosses the pipe.
    #[test]
    fn the_right_code_gives_both_ends_one_sealed_channel() {
        let (mut v, t) = host_on_pipe("482913");
        let mut ch = viewer_attempt(&mut v, "482913").expect("the right code");
        ch.send(&mut v, &json!({"key": "ssh-ed25519 AAAA"}))
            .unwrap();
        let (r, strikes, mut h) = t.join().unwrap();
        let mut hc = r.expect("the host's channel");
        assert!(strikes.is_empty());
        let got = hc.recv(&mut h).unwrap();
        assert_eq!(got["key"], "ssh-ed25519 AAAA");
        hc.send(&mut h, &json!({"ok": true})).unwrap();
        assert_eq!(ch.recv(&mut v).unwrap()["ok"], true);
    }

    /// Rule 1243: a wrong code ends that attempt with the tries left; the right code after it still pairs.
    #[test]
    fn a_wrong_code_ends_the_attempt_and_the_right_one_after_it_pairs() {
        let (mut v, t) = host_on_pipe("482913");
        assert_eq!(
            viewer_attempt(&mut v, "482914").err(),
            Some(AttemptEnd::Wrong { left: 2 })
        );
        assert!(viewer_attempt(&mut v, "482913").is_ok());
        let (r, strikes, _) = t.join().unwrap();
        assert!(r.is_ok());
        assert_eq!(strikes, [2]);
    }

    /// Rule 1243: three wrong codes close the host's window: the third answer says none is left and the host ends.
    #[test]
    fn three_wrong_codes_close_the_window() {
        let (mut v, t) = host_on_pipe("482913");
        for left in [2, 1, 0] {
            assert_eq!(
                viewer_attempt(&mut v, "000000").err(),
                Some(AttemptEnd::Wrong { left })
            );
        }
        let (r, strikes, h) = t.join().unwrap();
        assert_eq!(r.err(), Some(HostEnd::TooMany));
        assert_eq!(strikes, [2, 1, 0]);
        drop(h);
        // The right code now finds no one.
        assert!(matches!(
            viewer_attempt(&mut v, "482913"),
            Err(AttemptEnd::Gone(_))
        ));
    }

    /// Rule 1243: a host that does not know the code cannot prove it: its confirmation fails on the viewer, which
    /// gets no channel (a stranger in between learns nothing it can use).
    #[test]
    fn a_host_that_does_not_know_the_code_is_not_trusted() {
        let (mut v, mut h) = pipe::pair();
        let t = std::thread::spawn(move || {
            // A host that answers every confirmation, whatever the code.
            let first = recv(&mut h).unwrap();
            let msg_a = b64_field(&first, "msg").unwrap();
            let (state, msg_b) = Spake2::<Ed25519Group>::start_b(
                &Password::new(b"111111"),
                &Identity::new(ID_VIEWER),
                &Identity::new(ID_HOST),
            );
            send(&mut h, &json!({"t": "spake", "msg": B64.encode(&msg_b)})).unwrap();
            let _ = recv(&mut h).unwrap();
            let keys = keys_of(&state.finish(&msg_a).unwrap());
            let transcript = [msg_a.as_slice(), msg_b.as_slice()].concat();
            let mac = mac_of(&keys.confirm_host, &transcript);
            send(&mut h, &json!({"t": "confirm", "mac": B64.encode(mac)})).unwrap();
        });
        assert_eq!(
            viewer_attempt(&mut v, "482913").err(),
            Some(AttemptEnd::Unproved)
        );
        t.join().unwrap();
    }

    /// Rule 1243: a sealed frame that is replayed or changed does not open.
    #[test]
    fn a_replayed_or_changed_sealed_frame_does_not_open() {
        let k = keys_of(b"shared secret");
        let mut a = Channel::new(&k.seal_viewer, &k.seal_host);
        let mut b = Channel::new(&k.seal_host, &k.seal_viewer);
        let mut wire = Vec::new();
        a.send(&mut wire, &json!({"n": 1})).unwrap();
        let first = wire.clone();
        assert_eq!(b.recv(&mut first.as_slice()).unwrap()["n"], 1);
        assert!(b.recv(&mut first.as_slice()).is_err(), "a replay");
        let mut c = Channel::new(&k.seal_host, &k.seal_viewer);
        let mut changed = first.clone();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert!(c.recv(&mut changed.as_slice()).is_err(), "a changed frame");
        // A frame sealed by the host does not open as the host's own.
        let mut wire2 = Vec::new();
        b.send(&mut wire2, &json!({"n": 2})).unwrap();
        let mut d = Channel::new(&k.seal_viewer, &k.seal_viewer);
        assert!(d.recv(&mut wire2.as_slice()).is_err());
    }
}
