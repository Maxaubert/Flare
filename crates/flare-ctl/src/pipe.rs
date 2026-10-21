//! The Windows named pipe under the channel: the name, the owner-only security descriptor, the
//! accept loop with one worker thread per client, and the client's connect.
//!
//! Reads are probes plus short sleeps, never a parked `ReadFile` (see `framing.rs`). The pipe is
//! byte mode, duplex, remote clients rejected, created first-instance so a squatter's pipe of the
//! same name fails the server's start instead of receiving its clients.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
    GetLastError, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
};
use windows_sys::Win32::System::IO::CancelIoEx;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe,
};

use crate::client::{Client, ClientError};
use crate::framing::{Wire, write_line};
use crate::proto::{CtlError, MAX_LINE, Response, code, to_line};
use crate::session::{ConnHandle, Dispatcher, ServerConfig, Shared, serve_connection};
use crate::winid::{current_user_sid, sid_of_pid};

/// Environment variable naming the pipe. Flare sets it in the shells it starts (with
/// `FLARE_TAB_ID`), so a child finds the right Flare even when the owner runs two profiles.
pub const ENV_PIPE: &str = "FLARE_CTL_PIPE";

const BUFFER: u32 = 64 * 1024;
/// How long `Server::stop` waits for workers to leave.
const STOP_WAIT: Duration = Duration::from_secs(2);

fn fnv1a(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.to_lowercase().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `\\.\pipe\flare-ctl-<16 hex of FNV-1a(SID)>`; with a `scope` (a profile folder, an e2e run) the
/// scope's hash is appended, so two profiles of one user never share a pipe.
pub fn pipe_name(sid: &str, scope: Option<&str>) -> String {
    match scope {
        None => format!("\\\\.\\pipe\\flare-ctl-{:016x}", fnv1a(sid)),
        Some(s) => format!(
            "\\\\.\\pipe\\flare-ctl-{:016x}-{:08x}",
            fnv1a(sid),
            fnv1a(s) as u32
        ),
    }
}

/// The current user's SID string.
pub fn user_sid() -> Option<String> {
    current_user_sid()
}

/// The pipe of the current user (and `scope`), or None when Windows will not say who we are.
pub fn user_pipe_name(scope: Option<&str>) -> Option<String> {
    current_user_sid().map(|sid| pipe_name(&sid, scope))
}

/// The pipe a client should use: `FLARE_CTL_PIPE` when set and non-empty, else the user's default.
pub fn default_pipe_name() -> Option<String> {
    match std::env::var(ENV_PIPE) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => user_pipe_name(None),
    }
}

/// One end of a connected pipe.
pub struct PipeEnd(File);

impl Wire for PipeEnd {
    fn poll_read(&self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        let mut avail: u32 = 0;
        // SAFETY: the handle is the open pipe; only the "bytes available" out-pointer is used.
        let ok = unsafe {
            PeekNamedPipe(
                self.0.as_raw_handle() as _,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut avail,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            if avail == 0 {
                return Ok(Some(0));
            }
            // Data is ready, so this read returns at once.
            let want = (avail as usize).min(buf.len());
            return (&self.0).read(&mut buf[..want]).map(Some);
        }
        // SAFETY: last error of the call above.
        let err = unsafe { GetLastError() };
        if err == ERROR_BROKEN_PIPE || err == ERROR_PIPE_NOT_CONNECTED {
            // The peer closed. What it wrote before closing may still be readable; the peek cannot
            // see it, a read can, and with nothing left it fails at once instead of waiting.
            return match (&self.0).read(buf) {
                Ok(0) => Ok(None),
                Ok(n) => Ok(Some(n)),
                Err(e) if is_closed(&e) => Ok(None),
                Err(e) => Err(e),
            };
        }
        Err(io::Error::from_raw_os_error(err as i32))
    }

    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        (&self.0).write_all(bytes)
    }
}

/// The pipe's descriptor: a protected DACL with one ACE, full access for `sid` and nobody else.
pub fn dacl_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

fn is_closed(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(c) if c == ERROR_BROKEN_PIPE as i32 || c == ERROR_PIPE_NOT_CONNECTED as i32
    )
}

/// A security descriptor from SDDL, freed on drop.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn owner_only(sid: &str) -> io::Result<Descriptor> {
        let sddl: Vec<u16> = dacl_sddl(sid)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
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
            return Err(io::Error::other(
                "could not build the pipe's security descriptor",
            ));
        }
        Ok(Descriptor(sd))
    }
}

// SAFETY: the descriptor is plain LocalAlloc'd memory that is only read after creation.
unsafe impl Send for Descriptor {}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor came from the SDDL conversion and is freed once.
        unsafe { LocalFree(self.0) };
    }
}

fn create_instance(name: &[u16], sd: &Descriptor, first: bool) -> io::Result<RawHandle> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    let open_mode = PIPE_ACCESS_DUPLEX
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
    // SAFETY: `name` is NUL-terminated and `attrs` points at a live descriptor; both outlive the call.
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            255,
            BUFFER,
            BUFFER,
            0,
            &attrs,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(h as RawHandle)
}

/// The running server. Dropping it stops it.
pub struct Server {
    shared: Arc<Shared>,
    name: String,
    accept: Option<JoinHandle<()>>,
    active: Arc<AtomicUsize>,
}

impl Server {
    /// Creates the pipe `name` for the current user and starts accepting. Fails when a pipe of that
    /// name exists already (another Flare, or a squatter).
    pub fn start(
        name: &str,
        config: ServerConfig,
        dispatcher: Arc<dyn Dispatcher>,
    ) -> io::Result<Server> {
        let sid = current_user_sid()
            .ok_or_else(|| io::Error::other("could not read the current user"))?;
        let sd = Descriptor::owner_only(&sid)?;
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let first = create_instance(&wide, &sd, true)?;
        let shared = Shared::new(dispatcher, config);
        let active = Arc::new(AtomicUsize::new(0));
        let accept = {
            let (shared, active) = (shared.clone(), active.clone());
            let first = first as usize;
            std::thread::Builder::new()
                .name("flare-ctl-accept".into())
                .spawn(move || accept_loop(first as RawHandle, &wide, sd, &shared, &active))?
        };
        Ok(Server {
            shared,
            name: name.into(),
            accept: Some(accept),
            active,
        })
    }

    /// Queues an event for every client that said hello (and, with `tab`, belongs to that tab).
    /// Never blocks on a client. Returns how many got it.
    pub fn publish(&self, event: &str, params: Value, tab: Option<&str>) -> usize {
        self.shared.hub.publish(event, params, tab)
    }

    /// Clients connected now.
    pub fn client_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    pub fn pipe(&self) -> &str {
        &self.name
    }

    /// Stops accepting, tells every worker to leave (cancelling writes that block on a client that
    /// never reads) and waits briefly for them and for in-flight calls. Returns how many dispatcher
    /// calls are still running, so the app can wait on them inside its own quit cap.
    pub fn stop(mut self) -> usize {
        self.shutdown()
    }

    fn shutdown(&mut self) -> usize {
        let Some(accept) = self.accept.take() else {
            return self.shared.in_flight.load(Ordering::SeqCst);
        };
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.hub.abort_all();
        // A connect unblocks the accept thread's ConnectNamedPipe.
        let started = Instant::now();
        while !accept.is_finished() && started.elapsed() < STOP_WAIT {
            let _ = std::fs::OpenOptions::new() // sync-ok: quit, bounded by STOP_WAIT
                .read(true)
                .write(true)
                .open(&self.name);
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: quit, bounded by STOP_WAIT
        }
        if accept.is_finished() {
            let _ = accept.join(); // sync-ok: the accept thread has finished
        } else {
            log::warn!("flare-ctl: the accept thread did not stop; leaving it behind");
        }
        // A cancel only hits I/O that is blocked right now, so it repeats until the workers are out.
        while self.active.load(Ordering::SeqCst) > 0 && started.elapsed() < STOP_WAIT {
            self.shared.hub.abort_all();
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: quit, bounded by STOP_WAIT
        }
        while self.shared.in_flight.load(Ordering::SeqCst) > 0 && started.elapsed() < STOP_WAIT {
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: quit, bounded by STOP_WAIT
        }
        let left = self.shared.in_flight.load(Ordering::SeqCst);
        if left > 0 {
            log::warn!("flare-ctl: {left} call(s) still running at stop");
        }
        left
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

fn accept_loop(
    first: RawHandle,
    name: &[u16],
    sd: Descriptor,
    shared: &Arc<Shared>,
    active: &Arc<AtomicUsize>,
) {
    let mut pending = first;
    loop {
        // SAFETY: `pending` is a pipe instance this thread owns; no OVERLAPPED, so it blocks here.
        let ok = unsafe { ConnectNamedPipe(pending as _, std::ptr::null_mut()) }; // sync-ok: accept thread waits for a client
        // SAFETY: last error of the call above.
        let err = unsafe { GetLastError() };
        if shared.stop.load(Ordering::SeqCst) {
            // SAFETY: closing the instance this thread owns, once.
            drop(unsafe { File::from_raw_handle(pending) });
            return;
        }
        let connected = ok != 0 || err == ERROR_PIPE_CONNECTED;
        let conn = pending;
        // Listen again before serving, so the next client never meets a missing pipe.
        pending = loop {
            match create_instance(name, &sd, false) {
                Ok(h) => break h,
                Err(e) => {
                    log::warn!("flare-ctl: could not create the next pipe instance: {e}");
                    if shared.stop.load(Ordering::SeqCst) {
                        // SAFETY: closing the instance this thread owns, once.
                        drop(unsafe { File::from_raw_handle(conn) });
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100)); // sync-ok: accept thread, retrying
                }
            }
        };
        if !connected {
            // SAFETY: the instance never connected; closing it once.
            drop(unsafe { File::from_raw_handle(conn) });
            continue;
        }
        // SAFETY: `conn` is a connected pipe instance, now owned by the File.
        let end = PipeEnd(unsafe { File::from_raw_handle(conn) });
        admit(end, shared, active);
    }
}

/// Starts a worker for a connected client, or refuses it when the server is full.
fn admit(end: PipeEnd, shared: &Arc<Shared>, active: &Arc<AtomicUsize>) {
    if active.fetch_add(1, Ordering::SeqCst) >= shared.config.max_clients {
        active.fetch_sub(1, Ordering::SeqCst);
        let e = CtlError::new(
            code::TOO_MANY_CLIENTS,
            "Flare serves too many clients already",
        );
        let _ = write_line(&end, &to_line(&Response::err(Value::Null, e)), MAX_LINE);
        return; // dropping the handle closes it; the client still reads what was written
    }
    let (shared, active2) = (shared.clone(), active.clone());
    let raw = end.0.as_raw_handle() as usize;
    let spawned = std::thread::Builder::new()
        .name("flare-ctl-client".into())
        .spawn(move || {
            let handle = ConnHandle::new(NEXT_CONN.fetch_add(1, Ordering::SeqCst));
            shared.hub.add(handle.clone());
            handle.set_abort(Some(Box::new(move || {
                // SAFETY: the hook is cleared (waiting for a running call) before `end` closes the
                // handle, so `raw` is a live pipe handle whenever this runs. No OVERLAPPED: cancel all
                // of the pipe's I/O issued by any thread, which fails a blocked write at once.
                unsafe { CancelIoEx(raw as _, std::ptr::null()) };
            })));
            serve_connection(&end, &shared, &handle);
            handle.set_abort(None);
            shared.hub.remove(handle.id);
            active2.fetch_sub(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Opens the pipe as a client and checks that the process serving it runs as this user.
fn open_verified(name: &str, timeout: Duration) -> Result<PipeEnd, ClientError> {
    let started = Instant::now();
    let file = loop {
        // SECURITY_IDENTIFICATION: the server can learn who we are, never act as us.
        match std::fs::OpenOptions::new() // sync-ok: a client process's connect, bounded by its timeout
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(name)
        {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                if started.elapsed() >= timeout {
                    return Err(ClientError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(20)); // sync-ok: a client process, bounded
            }
            Err(e) => return Err(ClientError::Io(e)),
        }
    };
    let mut pid: u32 = 0;
    // SAFETY: the handle is the open pipe; `pid` outlives the call.
    if unsafe { GetNamedPipeServerProcessId(file.as_raw_handle() as _, &mut pid) } == 0 {
        return Err(ClientError::Protocol(
            "could not tell who serves the pipe".into(),
        ));
    }
    let me = current_user_sid();
    if me.is_none() || sid_of_pid(pid) != me {
        return Err(ClientError::Protocol(
            "the pipe is served by another user".into(),
        ));
    }
    Ok(PipeEnd(file))
}

/// Connects to the pipe `name`. `timeout` bounds waiting for a busy pipe.
pub fn connect(name: &str, timeout: Duration) -> Result<Client<PipeEnd>, ClientError> {
    open_verified(name, timeout).map(Client::new)
}

/// Connects to the pipe from `FLARE_CTL_PIPE` or the user's default.
pub fn connect_default(timeout: Duration) -> Result<Client<PipeEnd>, ClientError> {
    let name = default_pipe_name()
        .ok_or_else(|| ClientError::Io(io::Error::other("could not work out the pipe name")))?;
    connect(&name, timeout)
}
