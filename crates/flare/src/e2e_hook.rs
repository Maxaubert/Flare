//! The test hook's pipe server, started only under --e2e (Task 7).
//!
//! One server thread owns ONE pipe handle (`FILE_FLAG_FIRST_PIPE_INSTANCE`, one instance, a DACL for
//! the current user only). It accepts a client, reads one JSON request per line and answers each in
//! lockstep. `Hello` is answered by the server thread itself, so a harness can tell a hung UI thread
//! from a missing app; every other request goes to the UI thread through the event loop and is
//! answered there through a `Responder`. The server thread is test code: it may block.

#![allow(dead_code)] // the dispatcher (Task 8) and the harness use the rest

use std::cell::RefCell;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use flare_e2e_proto::{MAX_LINE_BYTES, PROTO_VERSION, Reply, Request};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

/// How long the server thread waits for the UI thread's answer, in milliseconds. A static so a
/// test can lower it.
pub static UI_REPLY_LIMIT: AtomicU64 = AtomicU64::new(10_000);

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

/// What Hello reports besides the constants: the renderer word and the scale, empty until the window glue says.
pub fn hello_info() -> (String, f32) {
    HELLO.lock().map(|h| h.clone()).unwrap_or_default()
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
            unsafe { CancelSynchronousIo(thread.as_raw_handle() as _) };
            // A connect unblocks an accept that began after the cancel; failure just means no waiter.
            let _ = std::fs::OpenOptions::new() // sync-ok: test hook teardown, only under --e2e
                .read(true)
                .write(true)
                .open(&self.pipe);
            let _ = thread.join(); // sync-ok: test hook teardown, only under --e2e
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The SDDL of the pipe's protected DACL: one ACE, full access for the current user only.
pub fn pipe_sddl(sid: &str) -> String {
    flare_win::single_instance::dacl_sddl(sid)
}

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

fn send(raw: usize, reply: &Reply) -> bool {
    let mut line = flare_e2e_proto::encode_reply(reply);
    if !line.ends_with('\n') {
        line.push('\n');
    }
    write_all(raw, line.as_bytes())
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
        version: env!("CARGO_PKG_VERSION").to_string(),
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
            dispatch(other, responder);
            let limit = Duration::from_millis(UI_REPLY_LIMIT.load(Ordering::SeqCst));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::{BufRead, BufReader, Write};

    fn profile() -> tempfile::TempDir {
        tempfile::tempdir().unwrap() // sync-ok: test code only
    }

    fn echo_dispatch() -> Dispatch {
        Arc::new(|req, responder| {
            let value = serde_json::to_value(&req).unwrap();
            responder.reply(Reply::Value { value });
        })
    }

    fn connect(pipe: &str) -> (File, BufReader<File>) {
        let f = std::fs::OpenOptions::new() // sync-ok: test code only
            .read(true)
            .write(true)
            .open(pipe)
            .expect("the pipe opens");
        let r = BufReader::new(f.try_clone().unwrap());
        (f, r)
    }

    fn ask(w: &mut File, r: &mut BufReader<File>, req: &Request) -> Reply {
        w.write_all(flare_e2e_proto::encode(req).as_bytes())
            .unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        flare_e2e_proto::decode_reply(line.trim_end()).unwrap()
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

    #[test]
    fn dropping_the_server_returns_even_with_a_client_connected() {
        let dir = profile();
        let server = start_with(true, dir.path()).unwrap();
        let (_w, _r) = connect(&server.pipe);
        let started = std::time::Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
