//! The authenticated link between Flare copies (Task 20, N26). Ported from Prism Terminal's
//! `core/main/dictationLink.ts` at e5848cb.
//!
//! One dictation for every copy: the copy whose key hook holds the lock is the SERVICE; every other copy is a
//! CLIENT on a local named pipe. The client answers for its own tabs, and pastes or sends into them through its
//! own checked path when the service asks. JSON, one message per line.
//!
//! BOTH ENDS PROVE THEY HOLD THE USER'S LINK KEY before anything else is said (a process that took the pipe's
//! name first could otherwise pose as the service and have a copy type into its Claude). The key is 32 random
//! bytes in a file in the dictation root; the pipe is named after its hash, never the key.
//!
//! ```text
//!   service -> client  {"t":"nonce","n":..}
//!   client -> service  {"t":"hello","pid":..,"name":..,"n":..,"mac":..}   HMAC(key, "client|" + service n + "|" + n)
//!   service -> client  {"t":"proof","mac":..}                             HMAC(key, "service|" + client n + "|" + service n)
//! ```
//!
//! A peer that fails, or has not proved itself in 5000 ms, is dropped and answered nothing. The pipe has a
//! current-user DACL and rejects remote clients. Everything blocking runs on this module's own threads; the
//! caller never waits (`tabs` and `send` answer on a channel).
//!
//! On macOS (port phase 7) the transport is a Unix socket instead (`macos/link_sock.rs`): in a 0700 folder of
//! this user's own under the per-user temp folder, mode 0600, one live service per name (an `flock` beside it),
//! and the peer's uid read from the kernel (`getpeereid`) on both ends, another user dropped before a byte is
//! read. The key file, the messages, the handshake and every rule below are the same code on both systems.
//!
//! Two deliberate strictnesses over Prism Terminal: an answer is matched to the peer that was asked (a rid from
//! another peer is ignored), and a single line over 64 KiB drops the peer (Prism Terminal ignored the line and
//! dropped only past 256 KiB buffered).

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
#[cfg(windows)]
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
#[cfg(windows)]
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_ANONYMOUS, SECURITY_SQOS_PRESENT,
};
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE,
};
#[cfg(windows)]
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use crate::hmac::{from_hex, hmac_sha256, nonce, random_bytes, same_mac, to_hex};
use crate::route::{RemoteCopy, SendDecision, SendTab};

/// The macOS transport: the Unix socket, its event, its connection and its acceptor.
#[cfg(not(windows))]
#[path = "macos/link_sock.rs"]
mod sock;
#[cfg(not(windows))]
use sock::{connect, Conn, Event};
#[cfg(not(windows))]
pub use sock::{peer_allowed, socket_dir};

/// A line from the other end, at most this long (UTF-16 units, as Prism Terminal counts).
pub const MAX_LINE: usize = 64 * 1024;
/// Dictated text, at most this long.
pub const MAX_TEXT: usize = 8000;
/// The tabs a copy may list.
pub const MAX_TABS: usize = 64;
/// A peer that has not proved itself in this long is dropped.
pub const PROOF_MS: u64 = 5000;
/// A tab list is asked with this limit.
pub const TABS_MS: u64 = 800;
/// A send is asked with this limit.
pub const SEND_MS: u64 = 15000;
/// A client tries again this often.
pub const RETRY_MS: u64 = 1000;
/// A write to a peer that does not read gives up after this long.
const WRITE_MS: u64 = 5000;

const NONCE_HEX: usize = 32;
const MAC_HEX: usize = 64;

// ---------------------------------------------------------------------------------------------
// The key and the pipe's name
// ---------------------------------------------------------------------------------------------

/// The user's link key: read, or made once (32 random bytes, hex). It is written whole to a temporary file and
/// put in place atomically, so a reader never sees a half-written key and two copies starting together agree
/// on one (the link that loses the race reads the winner's). A file that is there but is not a key (empty or
/// cut short by a crash) counts as missing and is replaced (audit L38). `None` when it can be neither read nor
/// made: then there is no link.
pub fn link_key(file: &Path) -> Option<[u8; 32]> {
    fn read(file: &Path) -> Option<[u8; 32]> {
        let text = std::fs::read_to_string(file).ok()?;
        let bytes = from_hex(text.trim(), 64)?;
        bytes.try_into().ok()
    }
    fn settle(file: &Path) -> Option<[u8; 32]> {
        // The other copy may be mid-way: give it a moment.
        for _ in 0..20 {
            if let Some(k) = read(file) {
                return Some(k);
            }
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: link worker, a first-run race with another copy
        }
        None
    }
    if let Some(k) = read(file) {
        return Some(k);
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut fresh = [0u8; 32];
    if !random_bytes(&mut fresh) {
        return settle(file);
    }
    let mut salt = [0u8; 4];
    let _ = random_bytes(&mut salt);
    let mut tmp_name = file.as_os_str().to_os_string();
    tmp_name.push(format!(".{}.{}.tmp", std::process::id(), to_hex(&salt)));
    let tmp = std::path::PathBuf::from(tmp_name);
    if write_private(&tmp, to_hex(&fresh).as_bytes()).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return settle(file);
    }
    // A file that is not there yet: link the finished one in (fails if another copy got there first).
    let linked = std::fs::hard_link(&tmp, file).is_ok();
    if !linked {
        // Something is there. A good key wins; a bad one waits a moment (an older build writes in place),
        // then is replaced in one atomic step.
        let found = settle(file);
        if found.is_none() {
            let _ = std::fs::rename(&tmp, file);
        }
    }
    let _ = std::fs::remove_file(&tmp);
    settle(file)
}
/// Writes a new file only this user can read: on macOS mode 0600 from the start (the umask would leave a key
/// readable by every account); on Windows the profile folder's ACL already keeps other users out.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(not(windows))]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)
    }
    #[cfg(windows)]
    {
        std::fs::write(path, bytes)
    }
}

/// The pipe's name: the key's hash, never the key.
#[cfg(windows)]
pub fn link_pipe(key: &[u8; 32]) -> String {
    let hash = Sha256::digest(key);
    format!(
        "\\\\.\\pipe\\{}{}",
        flare_win::names::DICTATION_PIPE_PREFIX,
        &to_hex(&hash)[..24]
    )
}

/// The socket's path: `<per-user temp folder>/flare-dictation/<24 hex of the key's hash>.sock`, never the key.
#[cfg(not(windows))]
pub fn link_pipe(key: &[u8; 32]) -> String {
    let hash = Sha256::digest(key);
    socket_dir()
        .join(format!("{}.sock", &to_hex(&hash)[..24]))
        .to_string_lossy()
        .into_owned()
}

/// A copy's name in the list: the installed app is "Flare", any other build "Flare (Dev)"; two of the same are
/// numbered by the service (D8).
pub fn copy_name(exe: &Path) -> String {
    if flare_win::packaged::is_packaged(exe) {
        flare_win::names::COPY_NAME.to_string()
    } else {
        flare_win::names::COPY_NAME_DEV.to_string()
    }
}

// ---------------------------------------------------------------------------------------------
// The messages
// ---------------------------------------------------------------------------------------------

/// What the service may say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToClient {
    Nonce { n: String },
    Proof { mac: String },
    TabsAsk { rid: u64 },
    Send { rid: u64, id: String, text: String },
    Paste { text: String },
}

/// What a client may say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToService {
    Hello {
        pid: u32,
        name: String,
        n: String,
        mac: String,
    },
    Tabs {
        rid: u64,
        tabs: Vec<SendTab>,
    },
    Sent {
        rid: u64,
        res: SendDecision,
    },
}

fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

fn is_hex(v: &Value, len: usize) -> Option<String> {
    let s = v.as_str()?;
    (s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| s.to_string())
}

fn str_max(v: &Value, max: usize) -> Option<String> {
    let s = v.as_str()?;
    (units(s) <= max).then(|| s.to_string())
}

fn int(v: &Value) -> Option<u64> {
    v.as_u64()
}

fn tab_of(v: &Value) -> Option<SendTab> {
    let o = v.as_object()?;
    let opt = |k: &str| o.get(k).and_then(|x| str_max(x, 256));
    Some(SendTab {
        id: str_max(o.get("id")?, 128)?,
        tab: str_max(o.get("tab")?, 128)?,
        label: str_max(o.get("label")?, 256)?,
        cwd: str_max(o.get("cwd")?, 1024)?,
        agent: opt("agent"),
        state: opt("state"),
        orchestrator: o.get("orchestrator") == Some(&Value::Bool(true)),
        session: opt("session"),
        copy: None,
    })
}

fn tab_json(t: &SendTab) -> Value {
    json!({
        "id": t.id, "tab": t.tab, "label": t.label, "cwd": t.cwd,
        "agent": t.agent, "state": t.state, "orchestrator": t.orchestrator, "session": t.session,
    })
}

fn parse_object(line: &str) -> Option<serde_json::Map<String, Value>> {
    if units(line) > MAX_LINE {
        return None;
    }
    match serde_json::from_str::<Value>(line).ok()? {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

/// A line from a client, checked field by field; anything else is `None`.
pub fn parse_to_service(line: &str) -> Option<ToService> {
    let m = parse_object(line)?;
    match m.get("t")?.as_str()? {
        "hello" => Some(ToService::Hello {
            pid: u32::try_from(int(m.get("pid")?)?).ok()?,
            name: str_max(m.get("name")?, 64)?,
            n: is_hex(m.get("n")?, NONCE_HEX)?,
            mac: is_hex(m.get("mac")?, MAC_HEX)?,
        }),
        "tabs" => Some(ToService::Tabs {
            rid: int(m.get("rid")?)?,
            tabs: m
                .get("tabs")?
                .as_array()?
                .iter()
                .take(MAX_TABS)
                .filter_map(tab_of)
                .collect(),
        }),
        "sent" => {
            let rid = int(m.get("rid")?)?;
            let r = m.get("res")?.as_object()?;
            if r.get("ok") == Some(&Value::Bool(true)) {
                let target = tab_of(r.get("target")?)?;
                // `entered: false`: typed, but the Enter did not send it. An older copy says nothing: sent.
                let res = if r.get("entered") == Some(&Value::Bool(false)) {
                    SendDecision::Typed(target)
                } else {
                    SendDecision::Ok(target)
                };
                Some(ToService::Sent { rid, res })
            } else {
                let reason = str_max(r.get("reason")?, 200)?;
                Some(ToService::Sent {
                    rid,
                    res: SendDecision::No(reason),
                })
            }
        }
        _ => None,
    }
}

/// A line from the service, checked; anything else is `None`.
pub fn parse_to_client(line: &str) -> Option<ToClient> {
    let m = parse_object(line)?;
    match m.get("t")?.as_str()? {
        "nonce" => Some(ToClient::Nonce {
            n: is_hex(m.get("n")?, NONCE_HEX)?,
        }),
        "proof" => Some(ToClient::Proof {
            mac: is_hex(m.get("mac")?, MAC_HEX)?,
        }),
        "tabs?" => Some(ToClient::TabsAsk {
            rid: int(m.get("rid")?)?,
        }),
        "send" => Some(ToClient::Send {
            rid: int(m.get("rid")?)?,
            id: str_max(m.get("id")?, 128)?,
            text: str_max(m.get("text")?, MAX_TEXT)?,
        }),
        "paste" => Some(ToClient::Paste {
            text: str_max(m.get("text")?, MAX_TEXT)?,
        }),
        _ => None,
    }
}

/// One line (with its newline) for a client.
pub fn encode_to_client(m: &ToClient) -> String {
    let v = match m {
        ToClient::Nonce { n } => json!({"t": "nonce", "n": n}),
        ToClient::Proof { mac } => json!({"t": "proof", "mac": mac}),
        ToClient::TabsAsk { rid } => json!({"t": "tabs?", "rid": rid}),
        ToClient::Send { rid, id, text } => {
            json!({"t": "send", "rid": rid, "id": id, "text": text})
        }
        ToClient::Paste { text } => json!({"t": "paste", "text": text}),
    };
    format!("{v}\n")
}

/// One line (with its newline) for the service.
pub fn encode_to_service(m: &ToService) -> String {
    let v = match m {
        ToService::Hello { pid, name, n, mac } => {
            json!({"t": "hello", "pid": pid, "name": name, "n": n, "mac": mac})
        }
        ToService::Tabs { rid, tabs } => {
            let tabs: Vec<Value> = tabs.iter().map(tab_json).collect();
            json!({"t": "tabs", "rid": rid, "tabs": tabs})
        }
        ToService::Sent { rid, res } => {
            let res = match res {
                SendDecision::Ok(t) => json!({"ok": true, "target": tab_json(t)}),
                // An older service reads only `ok`: to it this is a send, as it always was.
                SendDecision::Typed(t) => {
                    json!({"ok": true, "target": tab_json(t), "entered": false})
                }
                SendDecision::No(r) => json!({"ok": false, "reason": r}),
            };
            json!({"t": "sent", "rid": rid, "res": res})
        }
    };
    format!("{v}\n")
}

fn mac_of(key: &[u8; 32], what: &str) -> String {
    to_hex(&hmac_sha256(key, what.as_bytes()))
}

fn mac_matches(key: &[u8; 32], what: &str, theirs_hex: &str) -> bool {
    let Some(theirs) = from_hex(theirs_hex, MAC_HEX) else {
        return false;
    };
    same_mac(&hmac_sha256(key, what.as_bytes()), &theirs)
}

fn fresh_nonce_hex() -> String {
    to_hex(&nonce())
}

// ---------------------------------------------------------------------------------------------
// Lines out of a byte stream
// ---------------------------------------------------------------------------------------------

/// Cuts a byte stream into lines. A line longer than `MAX_LINE`, complete or not yet ended, is an error: the
/// peer is dropped.
#[derive(Default)]
struct LineReader {
    buf: Vec<u8>,
}

impl LineReader {
    fn push(&mut self, data: &[u8]) -> Result<Vec<String>, ()> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let line = &line[..line.len() - 1];
            if line.len() > MAX_LINE {
                return Err(());
            }
            let text = String::from_utf8_lossy(line);
            if !text.trim().is_empty() {
                out.push(text.into_owned());
            }
        }
        if self.buf.len() > MAX_LINE {
            return Err(());
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------------------------
// Overlapped pipes
// ---------------------------------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A manual-reset event.
#[cfg(windows)]
struct Event(usize);

#[cfg(windows)]
impl Event {
    fn new() -> Option<Event> {
        // SAFETY: default security, manual reset, not signalled, unnamed.
        let h = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        (!h.is_null()).then(|| Event(h as usize))
    }
    /// The stand-in for an event that could not be made: waiting on it never says set.
    fn none() -> Event {
        Event(0)
    }
    /// False for `none()`.
    fn is_real(&self) -> bool {
        self.0 != 0
    }
    fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
    fn set(&self) {
        // SAFETY: the handle is a live event owned by `self`.
        unsafe { SetEvent(self.raw()) };
    }
    /// Waits for the event, at most `ms` (`None` is forever); true when it was set.
    fn wait(&self, ms: Option<u64>) -> bool {
        let ms = ms.map_or(INFINITE, |m| m.min(u64::from(INFINITE - 1)) as u32);
        // SAFETY: the handle is a live event owned by `self`.
        let r =
            unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(self.raw(), ms) };
        r == WAIT_OBJECT_0
    }
}

#[cfg(windows)]
impl Drop for Event {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: the handle is closed exactly once, here.
            unsafe { CloseHandle(self.raw()) };
        }
    }
}

#[cfg(windows)]
enum Waited {
    Done,
    Cancelled,
    TimedOut,
}

#[cfg(windows)]
fn wait_io(io_event: HANDLE, cancel: HANDLE, deadline: Option<Instant>) -> Waited {
    let ms = match deadline {
        None => INFINITE,
        Some(d) => d
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u128::from(INFINITE - 1)) as u32,
    };
    let handles = [io_event, cancel];
    // SAFETY: two live handles; the array outlives the call.
    let r = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, ms) };
    if r == WAIT_OBJECT_0 {
        Waited::Done
    } else if r == WAIT_TIMEOUT {
        Waited::TimedOut
    } else {
        Waited::Cancelled
    }
}

#[cfg(windows)]
fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

/// One connected pipe end opened for overlapped I/O: a read and a write may be pending at once, from different
/// threads, and `cancel` ends whatever is pending.
#[cfg(windows)]
struct Conn {
    handle: usize,
    cancel: Event,
    write_lock: Mutex<()>,
}

#[cfg(windows)]
impl Conn {
    fn new(handle: HANDLE) -> Option<Arc<Conn>> {
        let cancel = match Event::new() {
            Some(c) => c,
            None => {
                // SAFETY: the handle is ours and not yet shared.
                unsafe { CloseHandle(handle) };
                return None;
            }
        };
        Some(Arc::new(Conn {
            handle: handle as usize,
            cancel,
            write_lock: Mutex::new(()),
        }))
    }

    fn h(&self) -> HANDLE {
        self.handle as HANDLE
    }

    fn cancel(&self) {
        self.cancel.set();
    }

    fn cancelled(&self) -> bool {
        self.cancel.wait(Some(0))
    }

    /// One overlapped call: `start` issues it; the wait ends on completion, on `cancel` or at the deadline.
    fn io(
        &self,
        deadline: Option<Instant>,
        start: impl FnOnce(&mut OVERLAPPED) -> i32,
    ) -> io::Result<u32> {
        let ev = Event::new().ok_or_else(io::Error::last_os_error)?;
        // SAFETY: OVERLAPPED is plain data; all-zero is its documented initial state.
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = ev.raw();
        if start(&mut ov) == 0 {
            // SAFETY: the last error of the call just made.
            let e = unsafe { GetLastError() };
            if e != ERROR_IO_PENDING {
                return Err(os_error(e));
            }
            match wait_io(ev.raw(), self.cancel.raw(), deadline) {
                Waited::Done => {}
                other => {
                    let mut n = 0u32;
                    // SAFETY: `ov` is the pending operation's; waiting for the cancelled call to finish
                    // keeps it alive until the kernel is done with it.
                    let done = unsafe {
                        CancelIoEx(self.h(), &ov);
                        GetOverlappedResult(self.h(), &ov, &mut n, 1)
                    };
                    if done != 0 && n > 0 {
                        return Ok(n);
                    }
                    return Err(match other {
                        Waited::TimedOut => io::ErrorKind::TimedOut.into(),
                        _ => io::ErrorKind::Interrupted.into(),
                    });
                }
            }
        }
        let mut n = 0u32;
        // SAFETY: `ov` completed (or the call finished at once); `n` is a valid out-pointer.
        if unsafe { GetOverlappedResult(self.h(), &ov, &mut n, 0) } == 0 {
            // SAFETY: the last error of the call just made.
            return Err(os_error(unsafe { GetLastError() }));
        }
        Ok(n)
    }

    /// Reads some bytes; `Ok(0)` is the end of the stream.
    fn read(&self, buf: &mut [u8], deadline: Option<Instant>) -> io::Result<usize> {
        let (ptr, len) = (buf.as_mut_ptr(), buf.len() as u32);
        let h = self.h();
        let r = self.io(deadline, |ov| {
            // SAFETY: `buf` outlives the call and the wait; `ov` is the live OVERLAPPED.
            unsafe { ReadFile(h, ptr.cast(), len, std::ptr::null_mut(), ov) }
        });
        match r {
            Ok(n) => Ok(n as usize),
            Err(e)
                if matches!(
                    e.raw_os_error().map(|c| c as u32),
                    Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA)
                ) =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }

    /// Writes every byte, one writer at a time, giving up after `WRITE_MS` or when cancelled.
    fn write(&self, data: &[u8]) -> io::Result<()> {
        let _one = lock(&self.write_lock);
        let deadline = Instant::now() + Duration::from_millis(WRITE_MS);
        let h = self.h();
        let mut done = 0usize;
        while done < data.len() {
            let rest = &data[done..];
            let (ptr, len) = (rest.as_ptr(), rest.len() as u32);
            let n = self.io(Some(deadline), |ov| {
                // SAFETY: `rest` outlives the call and the wait; `ov` is the live OVERLAPPED.
                unsafe { WriteFile(h, ptr.cast(), len, std::ptr::null_mut(), ov) }
            })?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            done += n as usize;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Conn {
    fn drop(&mut self) {
        // SAFETY: the handle is closed exactly once, when the last user lets go.
        unsafe { CloseHandle(self.h()) };
    }
}

/// The pipe's security descriptor: full access for the current user and nobody else.
#[cfg(windows)]
struct Descriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is a LocalAlloc'd block read only by `CreateNamedPipeW`, from one thread at a time.
#[cfg(windows)]
unsafe impl Send for Descriptor {}

#[cfg(windows)]
impl Descriptor {
    fn current_user() -> Result<Descriptor, String> {
        let who = flare_win::single_instance::current_identity()
            .ok_or("could not read the current user for the link pipe")?;
        let sddl = wide(&flare_win::single_instance::dacl_sddl(&who.sid));
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; `sd` receives the descriptor.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || sd.is_null() {
            return Err("could not build the link pipe's DACL".into());
        }
        Ok(Descriptor(sd))
    }
}

#[cfg(windows)]
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor came from the SDDL conversion (LocalAlloc) and is freed once.
        unsafe { LocalFree(self.0) };
    }
}

#[cfg(windows)]
fn create_instance(name: &[u16], sd: &Descriptor, first: bool) -> Result<HANDLE, String> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    let mut open = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        // The name is taken by someone else: fail rather than join them.
        open |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer and `attrs` points at a valid descriptor; both
    // outlive the call (the pipe keeps its own copy).
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &attrs,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: the last error of the call just made.
        return Err(format!("CreateNamedPipeW failed (error {})", unsafe {
            GetLastError()
        }));
    }
    Ok(h)
}

#[cfg(windows)]
enum Accepted {
    Connected,
    Stopped,
    Failed(u32),
}

/// Waits for a client on a new instance, or for `stop`.
#[cfg(windows)]
fn accept(h: HANDLE, stop: &Event) -> Accepted {
    let Some(ev) = Event::new() else {
        return Accepted::Failed(0);
    };
    // SAFETY: all-zero is OVERLAPPED's initial state.
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = ev.raw();
    // SAFETY: `h` is a live overlapped pipe instance and `ov` outlives the wait below.
    if unsafe { ConnectNamedPipe(h, &mut ov) } != 0 {
        return Accepted::Connected;
    }
    // SAFETY: the last error of the call just made.
    let e = unsafe { GetLastError() };
    if e == ERROR_PIPE_CONNECTED {
        return Accepted::Connected;
    }
    if e != ERROR_IO_PENDING {
        return Accepted::Failed(e);
    }
    match wait_io(ev.raw(), stop.raw(), None) {
        Waited::Done => {
            let mut n = 0u32;
            // SAFETY: the connect finished; `n` is a valid out-pointer.
            if unsafe { GetOverlappedResult(h, &ov, &mut n, 0) } != 0 {
                Accepted::Connected
            } else {
                // SAFETY: the last error of the call just made.
                Accepted::Failed(unsafe { GetLastError() })
            }
        }
        _ => {
            let mut n = 0u32;
            // SAFETY: cancel the pending connect and wait for it to finish before `ov` goes away.
            unsafe {
                CancelIoEx(h, &ov);
                GetOverlappedResult(h, &ov, &mut n, 1);
            }
            Accepted::Stopped
        }
    }
}

/// Opens the client end; `None` when there is no service (or every instance is busy: the retry finds one).
#[cfg(windows)]
fn connect(pipe: &str) -> Option<Arc<Conn>> {
    let name = wide(pipe);
    // SAFETY: `name` is NUL-terminated and outlives the call. Anonymous impersonation: an impostor that took
    // the name learns nothing about this process's token.
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        // ERROR_PIPE_BUSY (every instance taken for a moment) and "no such pipe" both mean: not now.
        return None;
    }
    Conn::new(h)
}

fn reap(threads: &mut Vec<JoinHandle<()>>) {
    let mut i = 0;
    while i < threads.len() {
        if threads[i].is_finished() {
            let _ = threads.swap_remove(i).join();
        } else {
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

struct Peer {
    id: u64,
    conn: Arc<Conn>,
    /// The service's nonce for this peer.
    nonce: String,
    copy: Mutex<Option<RemoteCopy>>,
    /// Lines for the peer's writer thread: nothing on the caller's thread waits on a pipe.
    out: Sender<String>,
}

impl Peer {
    fn named(&self) -> Option<RemoteCopy> {
        lock(&self.copy).clone()
    }
}

struct Shared {
    key: [u8; 32],
    stop: Event,
    peers: Mutex<Vec<Arc<Peer>>>,
    waits: Mutex<HashMap<(u64, u64), Sender<ToService>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    next_rid: AtomicU64,
    next_peer: AtomicU64,
    proof_ms: u64,
}

impl Shared {
    fn new(key: [u8; 32], stop: Event, proof_ms: u64) -> Arc<Shared> {
        Arc::new(Shared {
            key,
            stop,
            peers: Mutex::new(Vec::new()),
            waits: Mutex::new(HashMap::new()),
            threads: Mutex::new(Vec::new()),
            next_rid: AtomicU64::new(1),
            next_peer: AtomicU64::new(1),
            proof_ms,
        })
    }

    fn named(&self) -> Vec<Arc<Peer>> {
        lock(&self.peers)
            .iter()
            .filter(|p| p.named().is_some())
            .cloned()
            .collect()
    }

    /// Registers an answer for `peer` and queues the question.
    fn ask(
        &self,
        peer: &Arc<Peer>,
        make: impl FnOnce(u64) -> ToClient,
    ) -> Option<(u64, Receiver<ToService>)> {
        let rid = self.next_rid.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        lock(&self.waits).insert((peer.id, rid), tx);
        // A peer that hung up between being picked and now: its reader has cancelled the connection and
        // clears the waiters after that, so a waiter added after the clear would be kept (and its asker
        // held) for the whole timeout. Checked after the insert, the clear either sees this waiter or we do.
        if peer.conn.cancelled() || peer.out.send(encode_to_client(&make(rid))).is_err() {
            lock(&self.waits).remove(&(peer.id, rid));
            return None;
        }
        Some((rid, rx))
    }

    fn forget(&self, peer: u64, rid: u64) {
        lock(&self.waits).remove(&(peer, rid));
    }
}

/// The service's end: the copies connected now, and asking them.
pub struct LinkService {
    shared: Arc<Shared>,
    acceptor: Mutex<Option<JoinHandle<()>>>,
}

type OnError = Arc<dyn Fn(String) + Send + Sync>;
type AcceptHook = Arc<dyn Fn() + Send + Sync>;

/// A copy that was asked for its tabs: the peer's id, the copy, and the question's rid with its answer channel.
type Asked = (u64, RemoteCopy, Option<(u64, Receiver<ToService>)>);

impl LinkService {
    /// Takes the pipe's name and starts accepting. If the name is taken (or cannot be made) `on_error` hears
    /// why and the service is inert: it lists no copies.
    pub fn start(
        pipe: &str,
        key: [u8; 32],
        on_error: impl Fn(String) + Send + Sync + 'static,
    ) -> LinkService {
        Self::start_with_proof(pipe, key, on_error, PROOF_MS)
    }

    /// `start` with the time a peer has to prove itself (tests shorten it).
    #[doc(hidden)]
    pub fn start_with_proof(
        pipe: &str,
        key: [u8; 32],
        on_error: impl Fn(String) + Send + Sync + 'static,
        proof_ms: u64,
    ) -> LinkService {
        Self::start_inner(pipe, key, Arc::new(on_error), proof_ms, None)
    }

    /// `start` with a hook the acceptor runs between a connection completing and its peer being made (tests
    /// hold a connection there to make the close race deterministic).
    #[doc(hidden)]
    pub fn start_with_accept_hook(
        pipe: &str,
        key: [u8; 32],
        on_error: impl Fn(String) + Send + Sync + 'static,
        hook: impl Fn() + Send + Sync + 'static,
    ) -> LinkService {
        Self::start_inner(
            pipe,
            key,
            Arc::new(on_error),
            PROOF_MS,
            Some(Arc::new(hook)),
        )
    }

    #[cfg(not(windows))]
    fn start_inner(
        pipe: &str,
        key: [u8; 32],
        on_error: OnError,
        proof_ms: u64,
        hook: Option<AcceptHook>,
    ) -> LinkService {
        sock::start_service(pipe, key, on_error, proof_ms, hook)
    }

    #[cfg(windows)]
    fn start_inner(
        pipe: &str,
        key: [u8; 32],
        on_error: OnError,
        proof_ms: u64,
        hook: Option<AcceptHook>,
    ) -> LinkService {
        let stop = Event::new();
        let stop_failed = stop.is_none();
        let shared = Shared::new(key, stop.unwrap_or(Event(0)), proof_ms);
        if stop_failed {
            on_error("could not make the link's stop event".into());
            return Self::inert(shared);
        }
        let name = wide(pipe);
        let sd = match Descriptor::current_user() {
            Ok(sd) => sd,
            Err(e) => {
                on_error(e);
                return Self::inert(shared);
            }
        };
        let first = match create_instance(&name, &sd, true) {
            Ok(h) => h as usize,
            Err(e) => {
                on_error(e);
                return Self::inert(shared);
            }
        };
        let sh = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("flare-link-accept".into())
            .spawn(move || acceptor(sh, name, sd, first, on_error, hook));
        Self {
            shared,
            acceptor: Mutex::new(spawned.ok()),
        }
    }

    /// A service that accepts nobody: the name is taken, or nothing could be made.
    fn inert(shared: Arc<Shared>) -> LinkService {
        LinkService {
            shared,
            acceptor: Mutex::new(None),
        }
    }

    /// The copies that have proved themselves and are connected now.
    pub fn copies(&self) -> Vec<RemoteCopy> {
        self.shared
            .named()
            .iter()
            .filter_map(|p| p.named())
            .collect()
    }

    /// Every copy's tabs, answered on the channel once all have answered or `timeout` has passed; a copy
    /// that does not answer in time contributes no tabs.
    pub fn tabs(&self, timeout: Duration) -> Receiver<Vec<(RemoteCopy, Vec<SendTab>)>> {
        let (tx, rx) = mpsc::channel();
        let asked: Vec<Asked> = self
            .shared
            .named()
            .into_iter()
            .filter_map(|p| {
                let copy = p.named()?;
                let ask = self.shared.ask(&p, |rid| ToClient::TabsAsk { rid });
                Some((p.id, copy, ask))
            })
            .collect();
        if asked.is_empty() {
            let _ = tx.send(Vec::new());
            return rx;
        }
        let shared = Arc::clone(&self.shared);
        let deadline = Instant::now() + timeout;
        let work = move || {
            let mut out = Vec::new();
            for (peer, copy, ask) in asked {
                let mut tabs = Vec::new();
                if let Some((rid, rx)) = ask {
                    if let Ok(t) = wait_for(&rx, deadline, |m| match m {
                        ToService::Tabs { tabs, .. } => Some(tabs),
                        _ => None,
                    }) {
                        tabs = t;
                    }
                    shared.forget(peer, rid);
                }
                out.push((copy, tabs));
            }
            let _ = tx.send(out);
        };
        let _ = std::thread::Builder::new()
            .name("flare-link-tabs".into())
            .spawn(work);
        rx
    }

    /// Sends `text` into the tab `id` of the copy `pid`; the answer arrives on the channel.
    pub fn send(
        &self,
        pid: u32,
        id: &str,
        text: &str,
        timeout: Duration,
    ) -> Receiver<SendDecision> {
        let (tx, rx) = mpsc::channel();
        let closed = || SendDecision::No("that copy of the app has closed".into());
        let Some(peer) = self
            .shared
            .named()
            .into_iter()
            .find(|p| p.named().is_some_and(|c| c.pid == pid))
        else {
            let _ = tx.send(closed());
            return rx;
        };
        let (idc, textc) = (id.to_string(), text.to_string());
        let Some((rid, answers)) = self.shared.ask(&peer, |rid| ToClient::Send {
            rid,
            id: idc,
            text: textc,
        }) else {
            let _ = tx.send(closed());
            return rx;
        };
        let shared = Arc::clone(&self.shared);
        let deadline = Instant::now() + timeout;
        let _ = std::thread::Builder::new()
            .name("flare-link-send".into())
            .spawn(move || {
                let r = wait_for(&answers, deadline, |m| match m {
                    ToService::Sent { res, .. } => Some(res),
                    _ => None,
                });
                shared.forget(peer.id, rid);
                let _ = tx.send(match r {
                    Ok(res) => res,
                    Err(RecvTimeoutError::Timeout) => {
                        SendDecision::No("that copy of the app did not answer".into())
                    }
                    Err(RecvTimeoutError::Disconnected) => closed(),
                });
            });
        rx
    }

    /// Pastes `text` into the front tab of the copy `pid` (no Enter). False when that copy is not connected;
    /// the write itself happens on the peer's writer thread.
    pub fn paste(&self, pid: u32, text: &str) -> bool {
        let Some(peer) = self
            .shared
            .named()
            .into_iter()
            .find(|p| p.named().is_some_and(|c| c.pid == pid))
        else {
            return false;
        };
        peer.out
            .send(encode_to_client(&ToClient::Paste { text: text.into() }))
            .is_ok()
    }

    fn cancel_peers(&self) {
        for p in lock(&self.shared.peers).iter() {
            p.conn.cancel();
        }
    }

    /// Hangs up on every copy and stops accepting. Waits for the link's own threads.
    pub fn close(&self) {
        if self.shared.stop.is_real() {
            self.shared.stop.set();
        }
        self.cancel_peers();
        if let Some(a) = lock(&self.acceptor).take() {
            let _ = a.join();
        }
        // A connection that completed while this ran may have become a peer after the first pass.
        self.cancel_peers();
        let threads = std::mem::take(&mut *lock(&self.shared.threads));
        for t in threads {
            let _ = t.join();
        }
    }
}

impl Drop for LinkService {
    fn drop(&mut self) {
        self.close();
    }
}

/// Waits on `rx` until `pick` takes a message, the deadline passes or the channel closes.
fn wait_for<T>(
    rx: &Receiver<ToService>,
    deadline: Instant,
    pick: impl Fn(ToService) -> Option<T>,
) -> Result<T, RecvTimeoutError> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let m = rx.recv_timeout(left)?;
        if let Some(v) = pick(m) {
            return Ok(v);
        }
    }
}

#[cfg(windows)]
fn acceptor(
    shared: Arc<Shared>,
    name: Vec<u16>,
    sd: Descriptor,
    first: usize,
    on_error: OnError,
    hook: Option<AcceptHook>,
) {
    let mut handle = first as HANDLE;
    loop {
        match accept(handle, &shared.stop) {
            Accepted::Connected => {
                if let Some(h) = &hook {
                    h();
                }
                if let Some(conn) = Conn::new(handle) {
                    start_peer(&shared, conn);
                }
            }
            Accepted::Stopped => {
                // SAFETY: the instance is ours and nothing else holds it.
                unsafe { CloseHandle(handle) };
                return;
            }
            Accepted::Failed(code) => {
                // SAFETY: as above.
                unsafe { CloseHandle(handle) };
                on_error(format!("the link could not accept (error {code})"));
            }
        }
        // The next instance, retried every second while the service runs.
        loop {
            if shared.stop.wait(Some(0)) {
                return;
            }
            match create_instance(&name, &sd, false) {
                Ok(h) => {
                    handle = h;
                    break;
                }
                Err(e) => {
                    on_error(e);
                    if shared.stop.wait(Some(RETRY_MS)) {
                        return;
                    }
                }
            }
        }
    }
}

fn start_peer(shared: &Arc<Shared>, conn: Arc<Conn>) {
    let (out, lines) = mpsc::channel::<String>();
    let peer = Arc::new(Peer {
        id: shared.next_peer.fetch_add(1, Ordering::SeqCst),
        conn: Arc::clone(&conn),
        nonce: fresh_nonce_hex(),
        copy: Mutex::new(None),
        out,
    });
    let _ = peer.out.send(encode_to_client(&ToClient::Nonce {
        n: peer.nonce.clone(),
    }));
    lock(&shared.peers).push(Arc::clone(&peer));
    // Once the service is closing, no new peer lives: `close` cancels what it sees, and a peer that was
    // made after that is cancelled here (pushed first, so one of the two always sees it).
    if shared.stop.wait(Some(0)) {
        conn.cancel();
    }

    let writer_conn = Arc::clone(&conn);
    let writer = std::thread::Builder::new()
        .name("flare-link-write".into())
        .spawn(move || writer_loop(&writer_conn, &lines));
    let sh = Arc::clone(shared);
    let reader = std::thread::Builder::new()
        .name("flare-link-peer".into())
        .spawn(move || peer_loop(&sh, &peer));
    let mut threads = lock(&shared.threads);
    reap(&mut threads);
    threads.extend(writer.ok());
    threads.extend(reader.ok());
}

/// The peer's writer: lines out, one at a time, until the channel closes or the connection ends.
fn writer_loop(conn: &Conn, lines: &Receiver<String>) {
    // Ends with the channel, or with the connection: senders outlive a peer that has gone (a question that
    // picked it just before), so the channel alone may never close.
    while !conn.cancelled() {
        match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if conn.write(line.as_bytes()).is_err() {
                    conn.cancel();
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn peer_loop(shared: &Arc<Shared>, peer: &Arc<Peer>) {
    let started = Instant::now();
    let mut reader = LineReader::default();
    let mut buf = vec![0u8; 16 * 1024];
    'peer: loop {
        let deadline = if peer.named().is_some() {
            None
        } else {
            Some(started + Duration::from_millis(shared.proof_ms))
        };
        let n = match peer.conn.read(&mut buf, deadline) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let Ok(lines) = reader.push(&buf[..n]) else {
            break;
        };
        for l in lines {
            let Some(m) = parse_to_service(&l) else {
                continue;
            };
            match m {
                ToService::Hello { pid, name, n, mac } => {
                    let already = peer.named().is_some();
                    let what = format!("client|{}|{n}", peer.nonce);
                    if already || !mac_matches(&shared.key, &what, &mac) {
                        break 'peer;
                    }
                    *lock(&peer.copy) = Some(RemoteCopy { pid, name });
                    let proof = mac_of(&shared.key, &format!("service|{n}|{}", peer.nonce));
                    let _ = peer
                        .out
                        .send(encode_to_client(&ToClient::Proof { mac: proof }));
                }
                ToService::Tabs { rid, .. } | ToService::Sent { rid, .. } => {
                    if peer.named().is_some() {
                        let waiter = lock(&shared.waits).get(&(peer.id, rid)).cloned();
                        if let Some(w) = waiter {
                            let _ = w.send(m);
                        }
                    }
                }
            }
        }
    }
    peer.conn.cancel();
    lock(&shared.peers).retain(|p| p.id != peer.id);
    lock(&shared.waits).retain(|(p, _), _| *p != peer.id);
}

// ---------------------------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------------------------

/// What a client answers with. Called on the link's own threads, never the UI thread; a handler that needs
/// the UI posts to it and waits there.
pub trait ClientHandlers: Send + Sync + 'static {
    /// This copy's tabs.
    fn tabs(&self) -> Vec<SendTab>;
    /// Sends `text` into the tab `id` through this copy's checked path.
    fn send(&self, id: &str, text: &str) -> SendDecision;
    /// Pastes `text` into the front tab (no Enter). Must not block.
    fn paste(&self, text: &str);
}

struct ClientShared {
    stop: Event,
    closed: AtomicBool,
    current: Mutex<Option<Arc<Conn>>>,
}

/// A client's end: connects, and keeps trying while it is not the service.
pub struct LinkClient {
    shared: Arc<ClientShared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl LinkClient {
    /// `on_state` hears when this copy is connected to a service (proved both ways) and when it is not.
    pub fn start(
        pipe: &str,
        key: [u8; 32],
        me: RemoteCopy,
        handlers: Arc<dyn ClientHandlers>,
        retry_ms: u64,
        on_state: impl Fn(bool) + Send + Sync + 'static,
    ) -> LinkClient {
        let shared = Arc::new(ClientShared {
            stop: Event::new().unwrap_or_else(Event::none),
            closed: AtomicBool::new(false),
            current: Mutex::new(None),
        });
        let sh = Arc::clone(&shared);
        let pipe = pipe.to_string();
        let thread = std::thread::Builder::new()
            .name("flare-link-client".into())
            .spawn(move || client_loop(&sh, &pipe, &key, &me, &handlers, retry_ms, &on_state))
            .ok();
        LinkClient {
            shared,
            thread: Mutex::new(thread),
        }
    }

    /// Hangs up and stops trying. Waits for the client thread.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        if self.shared.stop.is_real() {
            self.shared.stop.set();
        }
        if let Some(c) = lock(&self.shared.current).as_ref() {
            c.cancel();
        }
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
    }
}

impl Drop for LinkClient {
    fn drop(&mut self) {
        self.close();
    }
}

fn client_loop(
    shared: &Arc<ClientShared>,
    pipe: &str,
    key: &[u8; 32],
    me: &RemoteCopy,
    handlers: &Arc<dyn ClientHandlers>,
    retry_ms: u64,
    on_state: &(impl Fn(bool) + Send + Sync),
) {
    let mut up = false;
    let mut set = |on: bool| {
        if on != up {
            up = on;
            on_state(on);
        }
    };
    while !shared.closed.load(Ordering::SeqCst) {
        if let Some(conn) = connect(pipe) {
            *lock(&shared.current) = Some(Arc::clone(&conn));
            if !shared.closed.load(Ordering::SeqCst) {
                session(shared, &conn, key, me, handlers, &mut set);
            }
            conn.cancel();
            *lock(&shared.current) = None;
        }
        set(false);
        if !shared.stop.is_real() {
            std::thread::sleep(Duration::from_millis(retry_ms)); // sync-ok: the client's own thread
        } else if shared.stop.wait(Some(retry_ms)) {
            break;
        }
    }
    set(false);
}

fn session(
    shared: &Arc<ClientShared>,
    conn: &Arc<Conn>,
    key: &[u8; 32],
    me: &RemoteCopy,
    handlers: &Arc<dyn ClientHandlers>,
    set: &mut impl FnMut(bool),
) {
    let started = Instant::now();
    let mine = fresh_nonce_hex();
    let mut theirs: Option<String> = None;
    let mut trusted = false;
    let mut reader = LineReader::default();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        // The same limit as the service gives a peer: a service that never proves itself is not one.
        let deadline = if trusted {
            None
        } else {
            Some(started + Duration::from_millis(PROOF_MS))
        };
        let n = match conn.read(&mut buf, deadline) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let Ok(lines) = reader.push(&buf[..n]) else {
            return;
        };
        for l in lines {
            let Some(m) = parse_to_client(&l) else {
                continue;
            };
            match m {
                ToClient::Nonce { n } => {
                    if theirs.is_some() {
                        return;
                    }
                    let mac = mac_of(key, &format!("client|{n}|{mine}"));
                    theirs = Some(n);
                    let hello = ToService::Hello {
                        pid: me.pid,
                        name: me.name.clone(),
                        n: mine.clone(),
                        mac,
                    };
                    if conn.write(encode_to_service(&hello).as_bytes()).is_err() {
                        return;
                    }
                }
                ToClient::Proof { mac } => {
                    // The service holds the key too, or it is not the service.
                    let Some(t) = theirs.as_deref() else { return };
                    if trusted || !mac_matches(key, &format!("service|{mine}|{t}"), &mac) {
                        return;
                    }
                    trusted = true;
                    set(true);
                }
                // Nothing is answered, typed or pasted for a peer that has not proved itself.
                _ if !trusted => return,
                ToClient::TabsAsk { rid } => {
                    let (h, c, s) = (Arc::clone(handlers), Arc::clone(conn), Arc::clone(shared));
                    let _ = std::thread::Builder::new()
                        .name("flare-link-tabs".into())
                        .spawn(move || {
                            let tabs = h.tabs();
                            if !s.closed.load(Ordering::SeqCst) {
                                let _ = c.write(
                                    encode_to_service(&ToService::Tabs { rid, tabs }).as_bytes(),
                                );
                            }
                        });
                }
                ToClient::Send { rid, id, text } => {
                    let (h, c, s) = (Arc::clone(handlers), Arc::clone(conn), Arc::clone(shared));
                    let _ = std::thread::Builder::new()
                        .name("flare-link-send".into())
                        .spawn(move || {
                            let res = h.send(&id, &text);
                            if !s.closed.load(Ordering::SeqCst) {
                                let _ = c.write(
                                    encode_to_service(&ToService::Sent { rid, res }).as_bytes(),
                                );
                            }
                        });
                }
                ToClient::Paste { text } => {
                    if !shared.closed.load(Ordering::SeqCst) {
                        handlers.paste(&text);
                    }
                }
            }
        }
        if conn.cancelled() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection with no pipe behind it: an event stands in for the handle, which is enough for everything
    /// that does not do I/O (cancel, cancelled, drop).
    #[cfg(windows)]
    fn dummy_conn() -> Arc<Conn> {
        let ev = Event::new().expect("an event");
        let h = ev.raw();
        std::mem::forget(ev); // the Conn closes the handle
        Conn::new(h).expect("a conn")
    }

    /// A connection to nobody who speaks: one end of a socket pair (the other end is kept open, then dropped).
    #[cfg(not(windows))]
    fn dummy_conn() -> Arc<Conn> {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
        std::mem::forget(b);
        Conn::new(a).expect("a conn")
    }

    fn dummy_peer(id: u64) -> (Arc<Peer>, Receiver<String>) {
        let (out, lines) = mpsc::channel();
        let peer = Arc::new(Peer {
            id,
            conn: dummy_conn(),
            nonce: fresh_nonce_hex(),
            copy: Mutex::new(None),
            out,
        });
        (peer, lines)
    }

    #[test]
    fn an_empty_or_garbled_key_file_is_replaced_not_kept_forever() {
        for junk in ["", "   \n", "not hex at all", "abcd"] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("link.key");
            std::fs::write(&file, junk).unwrap();
            let key = link_key(&file).expect("a crash-left file must not disable the link");
            assert_eq!(link_key(&file), Some(key), "the replacement is stable");
            let text = std::fs::read_to_string(&file).unwrap();
            assert_eq!(text.trim().len(), 64);
        }
    }

    #[test]
    fn a_good_key_file_is_left_alone_and_a_missing_one_is_made() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("link.key");
        let key = link_key(&file).expect("made");
        let before = std::fs::read(&file).unwrap();
        assert_eq!(link_key(&file), Some(key));
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }

    #[test]
    fn a_question_for_a_peer_that_has_hung_up_is_not_kept() {
        let shared = Shared::new([1u8; 32], Event::new().unwrap(), PROOF_MS);
        let (peer, _lines) = dummy_peer(1);
        peer.conn.cancel();
        let asked = shared.ask(&peer, |rid| ToClient::TabsAsk { rid });
        assert!(asked.is_none(), "asked a peer that had hung up");
        assert!(lock(&shared.waits).is_empty(), "a waiter was left behind");
    }

    #[test]
    fn the_writer_stops_when_its_connection_is_cancelled_even_if_the_channel_stays_open() {
        let (peer, lines) = dummy_peer(2);
        let conn = Arc::clone(&peer.conn);
        let t = std::thread::spawn(move || writer_loop(&conn, &lines));
        std::thread::sleep(Duration::from_millis(100));
        peer.conn.cancel();
        let end = Instant::now() + Duration::from_secs(3);
        while !t.is_finished() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(t.is_finished(), "the writer outlived its connection");
        drop(peer); // the sender lives until here
        let _ = t.join();
    }
}
