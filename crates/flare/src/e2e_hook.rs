//! The test hook's pipe server, started only under --e2e (Task 7).
//!
//! One server thread owns ONE pipe handle (`FILE_FLAG_FIRST_PIPE_INSTANCE`, one instance, a DACL for
//! the current user only). It accepts a client, reads one JSON request per line and answers each in
//! lockstep. `Hello` is answered by the server thread itself, so a harness can tell a hung UI thread
//! from a missing app; every other request goes to the UI thread through the event loop and is
//! answered there through a `Responder`. The server thread is test code: it may block.

#![allow(dead_code)] // the dispatcher (Task 8) and the harness use the rest

use std::cell::RefCell;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use flare_e2e_proto::{MAX_LINE_BYTES, PROTO_VERSION, Reply, Request};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE, LocalFree,
};
#[cfg(windows)]
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
#[cfg(windows)]
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
#[cfg(windows)]
use windows_sys::Win32::System::IO::CancelSynchronousIo;
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

/// How long the server thread waits for the UI thread's answer, in milliseconds. A static so a
/// test can lower it.
pub static UI_REPLY_LIMIT: AtomicU64 = AtomicU64::new(10_000);

/// How long past its own timeout the hook waits for a `Latency` answer: the app's timer answers
/// with an error at `timeout_ms`, and that error must get through.
pub const LATENCY_MARGIN_MS: u64 = 2_000;

/// How long the server thread waits for the UI thread's answer to `req`. A `Latency` request waits
/// for a frame that holds some text (a 100 MB flood takes tens of seconds), so it may wait as long
/// as its own timeout; every other request keeps `base`.
pub fn reply_limit(req: &Request, base: Duration) -> Duration {
    match req {
        Request::Latency { timeout_ms, .. } => base.max(Duration::from_millis(
            timeout_ms.saturating_add(LATENCY_MARGIN_MS),
        )),
        _ => base,
    }
}

const BUFFER: u32 = 64 * 1024;

#[derive(Debug)]
pub enum HookError {
    NotEnabled,
    Pipe(String),
}

/// Completes a request once. `Send`, so the UI thread can answer later (a frame, a latency probe).
pub struct Responder(Sender<Reply>);

impl Responder {
    pub fn reply(self, r: Reply) {
        let _ = self.0.send(r);
    }
}

/// A responder and the receiver its answer arrives on.
pub fn responder() -> (Responder, Receiver<Reply>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (Responder(tx), rx)
}

pub type UiHandler = Rc<dyn Fn(Request, Responder)>;

thread_local! {
    static UI_HANDLER: RefCell<Option<UiHandler>> = const { RefCell::new(None) };
}

/// Registers the UI-thread handler; replaces a previous one. Call it on the UI thread.
pub fn register_ui_handler(h: UiHandler) {
    UI_HANDLER.with(|slot| *slot.borrow_mut() = Some(h));
}

/// What `Hello` reports besides the constants; the window glue fills it once it knows.
static HELLO: Mutex<(String, f32)> = Mutex::new((String::new(), 1.0));

pub fn set_hello_info(renderer: &str, scale: f32) {
    if let Ok(mut h) = HELLO.lock() {
        *h = (renderer.to_string(), scale);
    }
}

/// The window's scale changed after the renderer was told (the software window reads 1.0 until winit
/// delivers the monitor's scale): keeps Hello's scale the window's own (audit V6).
pub fn set_hello_scale(scale: f32) {
    if let Ok(mut h) = HELLO.lock() {
        h.1 = scale;
    }
}

/// What Hello reports besides the constants: the renderer word and the scale, empty until the window glue says.
pub fn hello_info() -> (String, f32) {
    HELLO.lock().map(|h| h.clone()).unwrap_or_default()
}

/// The grid host's cell size in device pixels, noted by the render path under `--e2e` (0 until
/// the first frame), so `dpi.state` answers the exact cell and not a share of the leftover pixels.
static CELL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn set_cell(w: u32, h: u32) {
    CELL.store(
        (u64::from(w) << 32) | u64::from(h),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The noted cell size (width, height), None before a frame was drawn.
pub fn cell() -> Option<(u32, u32)> {
    let v = CELL.load(std::sync::atomic::Ordering::Relaxed);
    (v != 0).then_some(((v >> 32) as u32, v as u32))
}

/// Hands one request to something that will answer it. Production posts it to the UI thread; the
/// unit tests pass their own, because a unit test has no event loop.
type Dispatch = Arc<dyn Fn(Request, Responder) + Send + Sync>;

fn ui_dispatch() -> Dispatch {
    Arc::new(|req, responder| {
        let _ = slint::invoke_from_event_loop(move || {
            let handler = UI_HANDLER.with(|slot| slot.borrow().clone());
            match handler {
                Some(h) => h(req, responder),
                None if matches!(req, Request::Quit { .. }) => {
                    let _ = slint::quit_event_loop();
                }
                None => responder.reply(Reply::Err {
                    message: "no UI handler is registered".into(),
                }),
            }
        });
        // A failed post drops the closure and with it the responder; the server answers Err.
    })
}

pub struct HookServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub pipe: String,
}

/// Starts the server for an `--e2e` run. Err(NotEnabled) when the flag is off.
pub fn start(profile: &Path) -> Result<HookServer, HookError> {
    start_with(crate::e2e::global().on(), profile)
}

fn start_with(enabled: bool, profile: &Path) -> Result<HookServer, HookError> {
    start_with_dispatch(enabled, profile, ui_dispatch())
}

fn start_with_dispatch(
    enabled: bool,
    profile: &Path,
    dispatch: Dispatch,
) -> Result<HookServer, HookError> {
    if !enabled {
        return Err(HookError::NotEnabled);
    }
    let pipe = flare_e2e_proto::pipe_name(profile);
    let handle = create_pipe(&pipe)?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let raw = handle as usize;
    let thread = std::thread::Builder::new()
        .name("flare-e2e-hook".into())
        .spawn(move || serve(raw, &flag, &dispatch))
        .map_err(|e| HookError::Pipe(format!("could not start the hook thread: {e}")))?;
    Ok(HookServer {
        stop,
        thread: Some(thread),
        pipe,
    })
}

impl Drop for HookServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            // SAFETY: the thread handle is valid while the JoinHandle lives; this wakes a blocking
            // ReadFile or ConnectNamedPipe on that thread.
            #[cfg(windows)]
            unsafe {
                CancelSynchronousIo(thread.as_raw_handle() as _)
            };
            // A connect unblocks an accept that began after the cancel; failure just means no waiter.
            let _ = std::fs::OpenOptions::new() // sync-ok: test hook teardown, only under --e2e
                .read(true)
                .write(true)
                .open(&self.pipe);
            // Joined only once the thread has said it is finished: this drop also runs in a
            // thread-local destructor during process exit, when the OS has already ended the
            // thread and a plain join panics (the app then exits 101, not 0). A thread that
            // does not finish in time is left to the process exit.
            for _ in 0..50 {
                if thread.is_finished() {
                    let _ = thread.join(); // sync-ok: test hook teardown, only under --e2e
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5)); // sync-ok: test hook teardown, only under --e2e
            }
        }
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The SDDL of the pipe's protected DACL: one ACE, full access for the current user only.
pub fn pipe_sddl(sid: &str) -> String {
    flare_win::single_instance::dacl_sddl(sid)
}

#[cfg(windows)]
fn create_pipe(name: &str) -> Result<usize, HookError> {
    let who = flare_win::single_instance::current_identity()
        .ok_or_else(|| HookError::Pipe("could not read the current user".into()))?;
    let sddl = wide(&pipe_sddl(&who.sid));
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
        return Err(HookError::Pipe("could not build the pipe's DACL".into()));
    }
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let wname = wide(name);
    // SAFETY: `wname` is NUL-terminated and `attrs` points at a valid descriptor; both outlive the call.
    let handle = unsafe {
        CreateNamedPipeW(
            wname.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER,
            BUFFER,
            0,
            &attrs,
        )
    };
    // SAFETY: last error of the call above, read before anything can change it.
    let err = unsafe { GetLastError() };
    // SAFETY: the descriptor came from the SDDL conversion; the pipe keeps its own copy.
    unsafe { LocalFree(sd) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(HookError::Pipe(format!(
            "CreateNamedPipeW failed (error {err})"
        )));
    }
    Ok(handle as usize)
}

#[cfg(windows)]
fn serve(raw: usize, stop: &AtomicBool, dispatch: &Dispatch) {
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        // SAFETY: valid pipe handle owned by this thread; no OVERLAPPED, so it blocks.
        let connected = unsafe { ConnectNamedPipe(raw as _, std::ptr::null_mut()) }; // sync-ok: the hook thread waits for a client
        // SAFETY: last error of the call above.
        let err = unsafe { GetLastError() };
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if connected != 0 || err == ERROR_PIPE_CONNECTED || err == ERROR_NO_DATA {
            talk(raw, stop, dispatch);
        }
        // SAFETY: valid pipe handle; makes the instance ready for the next client.
        unsafe { DisconnectNamedPipe(raw as _) };
    }
    // SAFETY: the handle is closed exactly once, by the thread that owns it.
    unsafe { CloseHandle(raw as _) };
}

#[cfg(windows)]
fn read_chunk(raw: usize, buf: &mut [u8]) -> usize {
    let mut read = 0u32;
    // SAFETY: valid pipe handle and buffer; blocks until data, a close or a cancel.
    let ok = unsafe {
        ReadFile(
            raw as _,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &mut read,
            std::ptr::null_mut(),
        )
    }; // sync-ok: the hook thread waits for the client's next request
    if ok == 0 { 0 } else { read as usize }
}

#[cfg(windows)]
fn write_all(raw: usize, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        let mut written = 0u32;
        // SAFETY: valid pipe handle and buffer; blocks until the client has room.
        let ok = unsafe {
            WriteFile(
                raw as _,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        }; // sync-ok: the hook thread answers the client
        if ok == 0 || written == 0 {
            return false;
        }
        bytes = &bytes[written as usize..];
    }
    // SAFETY: valid pipe handle.
    unsafe { FlushFileBuffers(raw as _) };
    true
}

/// One reply as a wire line. A reply the client could not take whole (over the line limit) is
/// replaced by an error, so the client never reads half a line and falls out of step (L56).
fn reply_line(reply: &Reply) -> String {
    let mut line = flare_e2e_proto::encode_reply(reply);
    if line.len() > MAX_LINE_BYTES {
        line = flare_e2e_proto::encode_reply(&Reply::Err {
            message: format!(
                "the reply was {} bytes, over the {MAX_LINE_BYTES} byte line limit",
                line.len()
            ),
        });
    }
    if !line.ends_with('\n') {
        line.push('\n');
    }
    line
}

fn send(raw: usize, reply: &Reply) -> bool {
    write_all(raw, reply_line(reply).as_bytes())
}

fn talk(raw: usize, stop: &AtomicBool, dispatch: &Dispatch) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; BUFFER as usize];
    loop {
        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            let text = text.trim_end_matches('\r');
            if !handle_line(raw, text, dispatch) {
                return;
            }
        }
        if pending.len() > MAX_LINE_BYTES {
            let _ = send(
                raw,
                &Reply::Err {
                    message: "a request line is over one MiB".into(),
                },
            );
            return;
        }
        let n = read_chunk(raw, &mut buf);
        if n == 0 || stop.load(Ordering::SeqCst) {
            return;
        }
        pending.extend_from_slice(&buf[..n]);
    }
}

fn hello() -> Reply {
    let (renderer, scale) = HELLO
        .lock()
        .map(|h| h.clone())
        .unwrap_or_else(|_| (String::new(), 1.0));
    Reply::Hello {
        proto: PROTO_VERSION,
        version: crate::app_facts::version().to_string(),
        renderer,
        scale,
        pid: std::process::id(),
    }
}

/// Answers one line. False when the client is gone.
fn handle_line(raw: usize, text: &str, dispatch: &Dispatch) -> bool {
    let request = match flare_e2e_proto::decode_request(text) {
        Ok(r) => r,
        Err(message) => return send(raw, &Reply::Err { message }),
    };
    match request {
        Request::Hello => send(raw, &hello()),
        Request::Quit { force } => {
            // Answered first: the app is about to go away.
            let alive = send(raw, &Reply::Ok);
            let (responder, _unused) = responder();
            dispatch(Request::Quit { force }, responder);
            alive
        }
        other => {
            let (responder, rx) = responder();
            let base = Duration::from_millis(UI_REPLY_LIMIT.load(Ordering::SeqCst));
            let limit = reply_limit(&other, base);
            dispatch(other, responder);
            let reply = match rx.recv_timeout(limit) {
                // sync-ok: the hook thread waits for the UI thread's answer
                Ok(r) => r,
                Err(_) => {
                    let message = format!(
                        "the UI thread did not answer in {} s",
                        limit.as_secs_f64().max(0.0)
                    );
                    log::warn!("e2e hook: {message}");
                    Reply::Err { message }
                }
            };
            send(raw, &reply)
        }
    }
}

// ---- macOS: the hook is a Unix socket (rule 1103), with the named pipe's guarantees ----
//
// The socket is `flare_e2e_proto::pipe_name(profile)`, in the control channel's private folder (0700,
// this user's), bound 0600 under the control channel's first-instance lock, so a second `--e2e` launch on
// the same profile cannot take it over (`FILE_FLAG_FIRST_PIPE_INSTANCE`). A client of another uid is
// dropped before a byte is read (the pipe's DACL). Every wait is a short `poll` that looks at the stop
// flag, so dropping the server never hangs on a client that is still connected (`CancelSynchronousIo`).

#[cfg(target_os = "macos")]
struct MacHook {
    listener: std::os::unix::net::UnixListener,
    _lock: std::fs::File, // sync-ok: a type, held open for the lock; no call
    path: std::path::PathBuf,
}

#[cfg(target_os = "macos")]
thread_local! {
    /// The client this thread talks to now (one at a time, as the pipe has one instance).
    static MAC_CLIENT: RefCell<Option<std::os::unix::net::UnixStream>> = const { RefCell::new(None) };
    /// The server's stop flag (its address) while `serve` runs on this thread, so a read gives up
    /// when it is raised. `serve` borrows the flag for its whole run, so the address stays valid.
    static MAC_STOP: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How long one wait of the Mac hook lasts before it looks at the stop flag again, in milliseconds.
#[cfg(target_os = "macos")]
const MAC_POLL_MS: i32 = 50;

/// Waits up to `MAC_POLL_MS` for `fd` to be readable. True when it is (or is closed: the read says).
#[cfg(target_os = "macos")]
fn mac_readable(fd: std::os::fd::RawFd) -> bool {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the stated count; the fd stays open for the call.
    let n = unsafe { libc::poll(&mut p, 1, MAC_POLL_MS) }; // sync-ok: the hook thread waits for a client
    n > 0
}

/// Whether the server this thread serves was told to stop.
#[cfg(target_os = "macos")]
fn mac_stopped() -> bool {
    let addr = MAC_STOP.with(std::cell::Cell::get);
    // SAFETY: a non-zero address is the `stop` flag `serve` borrows for as long as it runs.
    addr != 0 && unsafe { &*(addr as *const AtomicBool) }.load(Ordering::SeqCst)
}

#[cfg(target_os = "macos")]
fn create_pipe(name: &str) -> Result<usize, HookError> {
    let path = std::path::PathBuf::from(name);
    let (listener, lock) = flare_ctl::pipe::bind_private_socket(&path)
        .map_err(|e| HookError::Pipe(format!("could not serve the hook socket: {e}")))?;
    let hook = Box::new(MacHook {
        listener,
        _lock: lock,
        path,
    });
    Ok(Box::into_raw(hook) as usize)
}

#[cfg(target_os = "macos")]
fn serve(raw: usize, stop: &AtomicBool, dispatch: &Dispatch) {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileTypeExt;
    // SAFETY: `raw` came from `Box::into_raw` in `create_pipe` and is taken back exactly once, here.
    let hook = unsafe { Box::from_raw(raw as *mut MacHook) };
    MAC_STOP.with(|s| s.set(std::ptr::from_ref(stop) as usize));
    let me = flare_ctl::pipe::own_uid();
    while !stop.load(Ordering::SeqCst) {
        if !mac_readable(hook.listener.as_raw_fd()) {
            continue;
        }
        let Ok((stream, _)) = hook.listener.accept() else {
            continue;
        };
        if !flare_ctl::pipe::peer_allowed(me, flare_ctl::pipe::peer_uid(stream.as_raw_fd())) {
            log::warn!("e2e hook: a client of another user was refused");
            continue;
        }
        if stream.set_nonblocking(false).is_err() {
            continue;
        }
        MAC_CLIENT.with(|c| *c.borrow_mut() = Some(stream));
        talk(0, stop, dispatch);
        MAC_CLIENT.with(|c| *c.borrow_mut() = None);
    }
    MAC_STOP.with(|s| s.set(0));
    // Removed only while it is still a socket, under the lock `hook` holds.
    // sync-ok: test hook teardown, only under --e2e
    if std::fs::symlink_metadata(&hook.path).is_ok_and(|m| m.file_type().is_socket()) {
        let _ = std::fs::remove_file(&hook.path); // sync-ok: test hook teardown, only under --e2e
    }
    drop(hook);
}

#[cfg(target_os = "macos")]
fn read_chunk(_raw: usize, buf: &mut [u8]) -> usize {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    loop {
        if mac_stopped() {
            return 0;
        }
        let Some(fd) = MAC_CLIENT.with(|c| c.borrow().as_ref().map(AsRawFd::as_raw_fd)) else {
            return 0;
        };
        if !mac_readable(fd) {
            continue;
        }
        return MAC_CLIENT.with(|c| match c.borrow_mut().as_mut() {
            Some(stream) => stream.read(buf).unwrap_or(0), // sync-ok: the hook thread reads a request poll said is there
            None => 0,
        });
    }
}

#[cfg(target_os = "macos")]
fn write_all(_raw: usize, bytes: &[u8]) -> bool {
    use std::io::Write;
    MAC_CLIENT.with(|c| match c.borrow_mut().as_mut() {
        Some(stream) => stream
            .write_all(bytes)
            .and_then(|()| stream.flush())
            .is_ok(), // sync-ok: the hook thread answers the client
        None => false,
    })
}

// ---- any other system: no hook ----

#[cfg(not(any(windows, target_os = "macos")))]
fn create_pipe(_name: &str) -> Result<usize, HookError> {
    Err(HookError::Pipe(
        "the test hook is not built on this platform".into(),
    ))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn serve(_raw: usize, _stop: &AtomicBool, _dispatch: &Dispatch) {}

#[cfg(not(any(windows, target_os = "macos")))]
fn read_chunk(_raw: usize, _buf: &mut [u8]) -> usize {
    0
}

#[cfg(not(any(windows, target_os = "macos")))]
fn write_all(_raw: usize, _bytes: &[u8]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[cfg(windows)]
    type Conn = std::fs::File;
    #[cfg(target_os = "macos")]
    type Conn = std::os::unix::net::UnixStream;

    fn profile() -> tempfile::TempDir {
        tempfile::tempdir().unwrap() // sync-ok: test code only
    }

    fn echo_dispatch() -> Dispatch {
        Arc::new(|req, responder| {
            let value = serde_json::to_value(&req).unwrap();
            responder.reply(Reply::Value { value });
        })
    }

    #[cfg(windows)]
    fn open(pipe: &str) -> Conn {
        std::fs::OpenOptions::new() // sync-ok: test code only
            .read(true)
            .write(true)
            .open(pipe)
            .expect("the pipe opens")
    }

    #[cfg(target_os = "macos")]
    fn open(pipe: &str) -> Conn {
        Conn::connect(pipe).expect("the socket opens") // sync-ok: test code only
    }

    fn connect(pipe: &str) -> (Conn, BufReader<Conn>) {
        let f = open(pipe);
        let r = BufReader::new(f.try_clone().unwrap());
        (f, r)
    }

    fn ask(w: &mut Conn, r: &mut BufReader<Conn>, req: &Request) -> Reply {
        w.write_all(flare_e2e_proto::encode(req).as_bytes())
            .unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        flare_e2e_proto::decode_reply(line.trim_end()).unwrap()
    }

    // L56: a reply over the line limit would be cut by the client and desync every later call.
    #[test]
    fn an_over_long_reply_becomes_an_error_line() {
        let big = Reply::Value {
            value: serde_json::Value::String("a".repeat(MAX_LINE_BYTES + 100)),
        };
        let line = reply_line(&big);
        assert!(line.len() <= MAX_LINE_BYTES, "{}", line.len());
        assert!(line.ends_with('\n'));
        match flare_e2e_proto::decode_reply(&line).unwrap() {
            Reply::Err { message } => assert!(message.contains("over"), "{message}"),
            other => panic!("expected Err, got {other:?}"),
        }
        let small = reply_line(&Reply::Ok);
        assert_eq!(flare_e2e_proto::decode_reply(&small).unwrap(), Reply::Ok);
    }

    #[test]
    fn start_without_the_flag_is_not_enabled_and_creates_no_pipe() {
        let dir = profile();
        assert!(matches!(
            start_with(false, dir.path()),
            Err(HookError::NotEnabled)
        ));
        // `e2e::init` never ran in this test binary, so the plain `start` is off too.
        assert!(matches!(start(dir.path()), Err(HookError::NotEnabled)));
        let name = flare_e2e_proto::pipe_name(dir.path());
        assert!(std::fs::OpenOptions::new().read(true).open(name).is_err()); // sync-ok: test code only
    }

    #[test]
    fn hello_scale_follows_a_later_scale_change_and_keeps_the_renderer() {
        set_hello_info("software", 1.0);
        set_hello_scale(2.25);
        match hello() {
            Reply::Hello {
                renderer, scale, ..
            } => {
                assert_eq!(renderer, "software");
                assert_eq!(scale, 2.25);
            }
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_hello_round_trips_without_the_ui_thread() {
        let dir = profile();
        // No UI handler is registered and the dispatch would fail: Hello never reaches it.
        let server = start_with(true, dir.path()).unwrap();
        let (mut w, mut r) = connect(&server.pipe);
        match ask(&mut w, &mut r, &Request::Hello) {
            Reply::Hello { proto, pid, .. } => {
                assert_eq!(proto, PROTO_VERSION);
                assert_eq!(pid, std::process::id());
            }
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_request_reaches_the_dispatch_and_its_answer_comes_back() {
        let dir = profile();
        let server = start_with_dispatch(true, dir.path(), echo_dispatch()).unwrap();
        let (mut w, mut r) = connect(&server.pipe);
        let req = Request::KeyPress {
            chord: "Ctrl+T".into(),
        };
        match ask(&mut w, &mut r, &req) {
            Reply::Value { value } => assert_eq!(value["chord"], "Ctrl+T"),
            other => panic!("expected Value, got {other:?}"),
        }
        // A bad line is an error reply, and the pipe stays usable.
        w.write_all(b"{\"op\":\"nope\"}\n").unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(matches!(
            flare_e2e_proto::decode_reply(line.trim_end()),
            Ok(Reply::Err { .. })
        ));
        assert!(matches!(
            ask(&mut w, &mut r, &Request::Hello),
            Reply::Hello { .. }
        ));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_second_server_on_the_same_profile_cannot_take_the_pipe() {
        let dir = profile();
        let first = start_with(true, dir.path()).unwrap();
        assert!(matches!(
            start_with(true, dir.path()),
            Err(HookError::Pipe(_))
        ));
        drop(first);
    }

    #[cfg(windows)]
    #[test]
    fn the_pipe_is_refused_to_another_user() {
        let sddl = pipe_sddl("S-1-5-21-1-2-3-1001");
        assert_eq!(sddl, "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)");
        assert_eq!(sddl.matches("(A;").count(), 1, "exactly one ACE");
        assert!(
            !sddl.contains("WD") && !sddl.contains("BU"),
            "no world or users ACE"
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn an_unanswered_request_times_out_with_an_error() {
        let dir = profile();
        let drop_it: Dispatch = Arc::new(|_req, responder| drop(responder));
        let server = start_with_dispatch(true, dir.path(), drop_it).unwrap();
        UI_REPLY_LIMIT.store(200, Ordering::SeqCst);
        let (mut w, mut r) = connect(&server.pipe);
        let started = std::time::Instant::now();
        let reply = ask(
            &mut w,
            &mut r,
            &Request::Probe {
                name: "x".into(),
                arg: serde_json::Value::Null,
            },
        );
        UI_REPLY_LIMIT.store(10_000, Ordering::SeqCst);
        assert!(matches!(reply, Reply::Err { .. }));
        assert!(started.elapsed() < Duration::from_secs(11));
    }

    fn latency(timeout_ms: u64) -> Request {
        Request::Latency {
            chord: "x".into(),
            expect: "x".into(),
            tab: None,
            timeout_ms,
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_latency_request_may_wait_as_long_as_its_own_timeout() {
        let base = Duration::from_secs(10);
        // A flood measure waits up to 120 s for its text; the hook must not cut it at 10 s.
        assert_eq!(
            reply_limit(&latency(120_000), base),
            Duration::from_millis(120_000 + LATENCY_MARGIN_MS)
        );
        // A short timeout never lowers the base limit, and other requests keep it.
        assert_eq!(reply_limit(&latency(100), base), base);
        let probe = Request::Probe {
            name: "x".into(),
            arg: serde_json::Value::Null,
        };
        assert_eq!(reply_limit(&probe, base), base);
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_slow_latency_answer_is_delivered_not_cut_off() {
        let dir = profile();
        // The UI answers the latency request after 400 ms, longer than the lowered base limit.
        let slow: Dispatch = Arc::new(|_req, responder| {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400)); // sync-ok: test code only
                responder.reply(Reply::Latency { ms: 400.0 });
            });
        });
        let server = start_with_dispatch(true, dir.path(), slow).unwrap();
        UI_REPLY_LIMIT.store(200, Ordering::SeqCst);
        let (mut w, mut r) = connect(&server.pipe);
        let reply = ask(&mut w, &mut r, &latency(5_000));
        UI_REPLY_LIMIT.store(10_000, Ordering::SeqCst);
        assert_eq!(reply, Reply::Latency { ms: 400.0 });
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn dropping_the_server_returns_even_with_a_client_connected() {
        let dir = profile();
        let server = start_with(true, dir.path()).unwrap();
        let (_w, _r) = connect(&server.pipe);
        let started = std::time::Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    // Rule 1103: the Mac hook is a 0600 socket in a 0700 folder of this user's, and it is gone when the
    // server stops (the next launch on the profile binds it again).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_socket_is_this_users_alone_and_goes_with_the_server() {
        use std::os::unix::fs::PermissionsExt;
        let dir = profile();
        let server = start_with(true, dir.path()).unwrap();
        let path = std::path::PathBuf::from(&server.pipe);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode(); // sync-ok: test code only
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        let folder = std::fs::metadata(path.parent().unwrap()).unwrap(); // sync-ok: test code only
        assert_eq!(
            folder.permissions().mode() & 0o077,
            0,
            "the folder is private"
        );
        drop(server);
        assert!(
            !path.exists(),
            "the socket is removed when the server stops"
        );
        let again = start_with(true, dir.path()).unwrap();
        assert_eq!(again.pipe, path.to_string_lossy());
    }
}
