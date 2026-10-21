//! Single instance (Task 15, decision D4): a named mutex says who is first, a named pipe carries the
//! second launch's folders to the first. Both names are keyed by the user SID, the session id and a
//! hash of the profile folder, so an e2e run (its own profile) never hands its folders to the owner's
//! running Flare, and another user or session cannot squat the name. The pipe is created first-instance
//! with a DACL for the current user only; the client checks the server's user and session before it
//! writes a byte, and lets only that verified pid take the foreground (never ASFW_ANY).

use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND, ReadFile, SECURITY_IDENTIFICATION,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

pub const MAX_HANDOFF: usize = 65_536;
pub const MAX_ARGS: usize = 256;
pub const MAX_ARG_LEN: usize = 32_768;
/// How long `PipeServer::stop` tries to reach its own pipe before it leaves the thread behind.
const STOP_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceNames {
    pub mutex: String,
    pub pipe: String,
}

/// FNV-1a 64 of the profile path, normalised: `/` becomes `\`, trailing `\` trimmed, lower-cased.
/// 16 lower-case hex digits.
pub fn profile_hash(profile: &Path) -> String {
    let s = profile.to_string_lossy().replace('/', "\\");
    let s = s.trim_end_matches('\\').to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Who a process is: its user SID (string form) and its session id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub sid: String,
    pub session: u32,
}

/// Where identities come from. The real source asks Windows; a test supplies a fake one.
pub trait IdentitySource {
    /// The identity of the process `pid`, or None when it cannot be read (then it is not trusted).
    fn of_pid(&self, pid: u32) -> Option<Identity>;
}

/// The real source: the process token's user and `ProcessIdToSessionId`.
pub struct WindowsIdentity;

impl IdentitySource for WindowsIdentity {
    fn of_pid(&self, pid: u32) -> Option<Identity> {
        let mut session: u32 = 0;
        // SAFETY: `session` outlives the call.
        if unsafe { ProcessIdToSessionId(pid, &mut session) } == 0 {
            return None;
        }
        // SAFETY: plain call; the handle (if any) is closed below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }
        let sid = token_sid_string(process);
        // SAFETY: the handle came from OpenProcess and is closed once.
        unsafe { CloseHandle(process) };
        Some(Identity { sid: sid?, session })
    }
}

/// The identity of this process.
pub fn current_identity() -> Option<Identity> {
    // SAFETY: GetCurrentProcessId has no preconditions.
    let pid = unsafe { GetCurrentProcessId() };
    WindowsIdentity.of_pid(pid)
}

fn token_sid_string(process: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    let mut token = std::ptr::null_mut();
    // SAFETY: `token` outlives the call; `process` is a valid process handle.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let result = (|| {
        let mut len: u32 = 0;
        // SAFETY: the size probe: a null buffer of length 0 and a valid `len`.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len) };
        if len == 0 {
            return None;
        }
        // u64 elements keep the buffer 8-byte aligned for TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` is at least `len` bytes; `len` is a valid out-pointer.
        let ok = unsafe {
            GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
        };
        if ok == 0 {
            return None;
        }
        // SAFETY: the call above filled `buf` with a TOKEN_USER.
        let user = unsafe { &*(buf.as_ptr().cast::<TOKEN_USER>()) };
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: the SID is valid for the life of `buf`; `text` receives a LocalAlloc'd string.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 || text.is_null() {
            return None;
        }
        let mut n = 0;
        // SAFETY: `text` is NUL-terminated, as ConvertSidToStringSidW promises.
        while unsafe { *text.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: `n` units were just walked.
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, n) });
        // SAFETY: freeing the string the call allocated, once.
        unsafe { LocalFree(text.cast()) };
        Some(s)
    })();
    // SAFETY: the token came from OpenProcessToken and is closed once.
    unsafe { CloseHandle(token) };
    result
}

/// The server is trusted only when it runs as the same user in the same session.
pub fn verify_peer(me: &Identity, peer: Option<&Identity>) -> Result<(), String> {
    match peer {
        None => Err("the hand-off server's identity could not be read".into()),
        Some(p) if p.sid != me.sid => Err("the hand-off server runs as another user".into()),
        Some(p) if p.session != me.session => {
            Err("the hand-off server runs in another session".into())
        }
        Some(_) => Ok(()),
    }
}

/// Mutex `Local\Flare.Single.<sid>.<session>.<hash>`, pipe `\\.\pipe\Flare.Handoff.<sid>.<session>.<hash>`.
/// `Local\` is per-session already; the pipe namespace is not, so both carry the user and the session.
pub fn instance_names_for(profile: &Path, who: &Identity) -> InstanceNames {
    let hash = profile_hash(profile);
    let tag = format!("{}.{}.{hash}", who.sid, who.session);
    InstanceNames {
        mutex: format!("Local\\Flare.Single.{tag}"),
        pipe: format!("\\\\.\\pipe\\Flare.Handoff.{tag}"),
    }
}

/// The names for this user and session. If Windows will not say who we are the names still work (they
/// carry `unknown`), but `serve` then refuses to build the pipe, since it cannot make the DACL.
pub fn instance_names(profile: &Path) -> InstanceNames {
    let who = current_identity().unwrap_or(Identity {
        sid: "unknown".into(),
        session: 0,
    });
    instance_names_for(profile, &who)
}

/// The SDDL of a protected DACL with one ACE: full access for `sid` and nobody else.
pub fn dacl_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

/// The SDDL of the lock mutex: a protected DACL with one ACE (full access for `sid`) and an explicit
/// medium integrity label. Without it the mutex takes the creator's token defaults: an elevated first
/// instance owns it as Administrators and a normal launch of the same user is refused (audit L11).
pub fn mutex_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})S:(ML;;NW;;;ME)")
}

/// Builds a security descriptor from SDDL (LocalAlloc'd; free with LocalFree).
fn descriptor_from_sddl(sddl: &str) -> Result<PSECURITY_DESCRIPTOR, String> {
    let sddl = wide(sddl);
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
        return Err("could not build a security descriptor".into());
    }
    Ok(sd)
}

/// Builds the pipe's security descriptor (LocalAlloc'd; free with LocalFree).
fn security_descriptor(sid: &str) -> Result<PSECURITY_DESCRIPTOR, String> {
    descriptor_from_sddl(&dacl_sddl(sid))
        .map_err(|_| "could not build the hand-off pipe's DACL".to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handoff {
    pub cwd: String,
    pub args: Vec<String>,
}

pub fn encode_handoff(h: &Handoff) -> Vec<u8> {
    let cwd = serde_json::to_string(&h.cwd).unwrap_or_else(|_| "\"\"".into());
    let args = serde_json::to_string(&h.args).unwrap_or_else(|_| "[]".into());
    format!("{{\"v\":1,\"cwd\":{cwd},\"args\":{args}}}").into_bytes()
}

pub fn decode_handoff(bytes: &[u8]) -> Option<Handoff> {
    if bytes.len() > MAX_HANDOFF {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let obj = value.as_object()?;
    if obj.get("v")?.as_u64()? != 1 {
        return None;
    }
    let cwd = obj.get("cwd")?.as_str()?.to_string();
    let list = obj.get("args")?.as_array()?;
    if list.len() > MAX_ARGS {
        return None;
    }
    let mut args = Vec::with_capacity(list.len());
    for item in list {
        let s = item.as_str()?;
        if s.chars().count() > MAX_ARG_LEN {
            return None;
        }
        args.push(s.to_string());
    }
    Some(Handoff { cwd, args })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct InstanceGuard {
    handle: usize,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateMutexW and is closed exactly once, here.
        unsafe { CloseHandle(self.handle as _) };
    }
}

pub enum Acquired {
    First(InstanceGuard),
    Second,
}

/// What `CreateMutexW` said.
#[derive(Debug, PartialEq, Eq)]
enum Created {
    /// This call made the mutex.
    Fresh,
    /// The mutex is there already: another instance holds the lock.
    Held,
}

/// Reads the result of `CreateMutexW`. ACCESS_DENIED means the mutex exists and this token may not
/// open it (an older elevated instance, or a squatter): a running instance, not a missing lock.
fn classify_create(null_handle: bool, err: u32) -> Result<Created, String> {
    if null_handle {
        if err == ERROR_ACCESS_DENIED {
            return Ok(Created::Held);
        }
        return Err(format!("CreateMutexW failed (error {err})"));
    }
    Ok(if err == ERROR_ALREADY_EXISTS {
        Created::Held
    } else {
        Created::Fresh
    })
}

pub fn acquire(names: &InstanceNames) -> Result<Acquired, String> {
    let name = wide(&names.mutex);
    // An explicit descriptor, so an elevated first instance still lets a normal launch see the lock.
    // If it cannot be built the default one is used (the lock still works for same-level launches).
    let sd = current_identity()
        .ok_or_else(|| "no identity".to_string())
        .and_then(|who| descriptor_from_sddl(&mutex_sddl(&who.sid)))
        .map_err(|e| log::warn!("the lock mutex keeps the default descriptor: {e}"))
        .ok();
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.unwrap_or(std::ptr::null_mut()),
        bInheritHandle: 0,
    };
    let attrs_ptr: *const SECURITY_ATTRIBUTES = if sd.is_some() {
        &attrs
    } else {
        std::ptr::null()
    };
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer and `attrs` (when used) points at a valid
    // descriptor; both outlive the call.
    let handle = unsafe { CreateMutexW(attrs_ptr, 0, name.as_ptr()) };
    // SAFETY: reads the calling thread's last error; nothing ran between the call above and this.
    let err = unsafe { GetLastError() };
    if let Some(sd) = sd {
        // SAFETY: the descriptor came from the SDDL conversion; the mutex keeps its own copy.
        unsafe { LocalFree(sd) };
    }
    match classify_create(handle.is_null(), err)? {
        Created::Held => {
            if !handle.is_null() {
                // SAFETY: the handle is valid and owned by us.
                unsafe { CloseHandle(handle) };
            }
            Ok(Acquired::Second)
        }
        Created::Fresh => Ok(Acquired::First(InstanceGuard {
            handle: handle as usize,
        })),
    }
}
pub struct PipeServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pipe: String,
}

impl PipeServer {
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.stop.store(true, Ordering::SeqCst);
        // Connect once so the blocked ConnectNamedPipe returns. The one pipe instance may be busy
        // with a client (a second launch mid-write, or one that connected and never closes): try
        // for a bounded time, and never join a thread that was not unblocked, or the quit hangs.
        let started = Instant::now();
        let unblocked = loop {
            match std::fs::OpenOptions::new().write(true).open(&self.pipe) {
                // sync-ok: quit, a bounded connect that unblocks the pipe thread
                Ok(_) => break true,
                // The server already closed its pipe: the thread is on its way out.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break true,
                Err(_) if started.elapsed() < STOP_WAIT => {
                    std::thread::sleep(Duration::from_millis(20)); // sync-ok: quit, bounded by STOP_WAIT
                }
                Err(_) => break thread.is_finished(),
            }
        };
        if unblocked {
            let _ = thread.join(); // sync-ok: stopping the worker at quit; it was unblocked
        } else {
            log::warn!("the hand-off server is held by a client; leaving its thread behind");
        }
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn read_message(pipe: usize) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = Vec::new();
    let mut overflow = false;
    let mut buf = [0u8; 4096];
    loop {
        let mut read: u32 = 0;
        // SAFETY: `buf` is a valid writable buffer of the stated length; `read` outlives the call;
        // the pipe handle is valid for the life of the worker thread.
        let ok = unsafe {
            ReadFile(
                pipe as _,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        }; // sync-ok: worker thread, blocks until the client writes or closes
        if ok == 0 || read == 0 {
            break;
        }
        if !overflow {
            if data.len() + read as usize > MAX_HANDOFF {
                overflow = true;
                data.clear();
            } else {
                data.extend_from_slice(&buf[..read as usize]);
            }
        }
    }
    if overflow { None } else { Some(data) }
}

pub fn serve(
    names: &InstanceNames,
    on_handoff: impl Fn(Handoff) + Send + 'static,
) -> Result<PipeServer, String> {
    let name = wide(&names.pipe);
    let who = current_identity().ok_or("could not read the current user for the hand-off pipe")?;
    let sd = security_descriptor(&who.sid)?;
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer and `attrs` points at a valid descriptor; both
    // outlive the call.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            65_536,
            0,
            &attrs,
        )
    };
    // SAFETY: reads the thread's last error right after the call, before anything can change it.
    let err = unsafe { GetLastError() };
    // SAFETY: the descriptor came from the SDDL conversion; the pipe keeps its own copy.
    unsafe { LocalFree(sd) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!("CreateNamedPipeW failed (error {err})"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let raw = handle as usize;
    let thread = std::thread::Builder::new()
        .name("flare-handoff".into())
        .spawn(move || {
            #[cfg(test)]
            {
                let ms = tests::FIRST_CONNECT_DELAY_MS.load(Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(ms)); // sync-ok: test seam, the server thread
            }
            loop {
                // SAFETY: valid pipe handle owned by this thread; no OVERLAPPED, so it blocks.
                let connected = unsafe { ConnectNamedPipe(raw as _, std::ptr::null_mut()) }; // sync-ok: worker thread waits for a client
                // SAFETY: last error of the call above.
                let err = unsafe { GetLastError() };
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                // ERROR_NO_DATA: the client connected, wrote and closed before this call; what it
                // wrote is still in the pipe and is read like any other message.
                if (connected != 0 || err == ERROR_PIPE_CONNECTED || err == ERROR_NO_DATA)
                    && let Some(h) = read_message(raw).as_deref().and_then(decode_handoff)
                {
                    on_handoff(h);
                }
                // SAFETY: valid pipe handle; makes the instance ready for the next client.
                unsafe { DisconnectNamedPipe(raw as _) };
            }
            // SAFETY: the handle is closed exactly once, by the thread that owns it.
            unsafe { CloseHandle(raw as _) };
        })
        .map_err(|e| format!("could not start the hand-off thread: {e}"))?;
    Ok(PipeServer {
        stop,
        thread: Some(thread),
        pipe: names.pipe.clone(),
    })
}

/// Why a hand-off did not go through.
#[derive(Debug, PartialEq, Eq)]
pub enum HandoffError {
    /// A server answered the name but is not this user in this session: nothing was written, and the
    /// caller refuses to start a second instance on the same profile (audit L41).
    Untrusted(String),
    /// No server, or the write failed.
    Failed(String),
}

impl std::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Untrusted(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

pub fn send_handoff(
    names: &InstanceNames,
    h: &Handoff,
    total_wait: Duration,
) -> Result<(), HandoffError> {
    send_handoff_with(names, h, total_wait, &WindowsIdentity)
}

/// `send_handoff` with the identity source made explicit, so a test can stand in another user.
pub fn send_handoff_with(
    names: &InstanceNames,
    h: &Handoff,
    total_wait: Duration,
    source: &dyn IdentitySource,
) -> Result<(), HandoffError> {
    let me = current_identity()
        .ok_or_else(|| HandoffError::Failed("could not read the current user".into()))?;
    let started = Instant::now();
    let bytes = encode_handoff(h);
    loop {
        // SECURITY_IDENTIFICATION: a server can learn who we are but never act as us.
        match std::fs::OpenOptions::new() // sync-ok: hand-off to the first instance, bounded by its deadline
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(&names.pipe)
        {
            Ok(mut file) => {
                let mut pid: u32 = 0;
                // SAFETY: the handle is the open pipe; `pid` outlives the call.
                let got =
                    unsafe { GetNamedPipeServerProcessId(file.as_raw_handle() as _, &mut pid) };
                if got == 0 {
                    return Err(HandoffError::Untrusted(
                        "could not tell which process serves the hand-off pipe".into(),
                    ));
                }
                verify_peer(&me, source.of_pid(pid).as_ref()).map_err(HandoffError::Untrusted)?;
                // The verified pid only, never ASFW_ANY: lets that process raise its window.
                // SAFETY: plain call with a pid.
                unsafe { AllowSetForegroundWindow(pid) };
                return file
                    .write_all(&bytes)
                    .map_err(|e| HandoffError::Failed(format!("hand-off write failed: {e}")));
            }
            Err(e) => {
                let retry = e.kind() == std::io::ErrorKind::NotFound
                    || e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32);
                if !retry {
                    // Access denied is what a pipe made by another user answers with.
                    return Err(HandoffError::Untrusted(format!(
                        "hand-off open refused: {e}"
                    )));
                }
                if started.elapsed() >= total_wait {
                    return Err(HandoffError::Failed(format!(
                        "no running Flare answered: {e}"
                    )));
                }
                std::thread::sleep(Duration::from_millis(50)); // sync-ok: second instance, nothing else runs
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc;

    /// How long the server thread waits before its first `ConnectNamedPipe` (tests only; each
    /// nextest test is its own process, so one test setting it touches no other).
    pub(super) static FIRST_CONNECT_DELAY_MS: AtomicU64 = AtomicU64::new(0);

    /// Review fix: a client that connects, writes and closes before the server thread first
    /// reaches `ConnectNamedPipe` makes it answer ERROR_NO_DATA. The message is still in the pipe
    /// and must be read, not dropped (a second launch right after the first lost its folder).
    #[test]
    fn a_handoff_written_before_the_server_listens_is_not_lost() {
        FIRST_CONNECT_DELAY_MS.store(400, Ordering::SeqCst);
        let n = names_for_test();
        let (tx, rx) = mpsc::channel();
        let _server = serve(&n, move |h| {
            let _ = tx.send(h);
        })
        .unwrap();
        let h = sample();
        send_handoff(&n, &h, Duration::from_secs(3)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), h);
    }

    fn names_for_test() -> InstanceNames {
        let dir = tempfile::tempdir().unwrap();
        instance_names(dir.path())
    }

    fn sample() -> Handoff {
        Handoff {
            cwd: "C:\\x".into(),
            args: vec!["a".into(), "b".into()],
        }
    }

    fn raw_write(names: &InstanceNames, bytes: &[u8]) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            match std::fs::OpenOptions::new().write(true).open(&names.pipe) {
                Ok(mut f) => {
                    let _ = f.write_all(bytes);
                    return;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    #[test]
    fn profile_hash_matches_the_fnv_vectors() {
        assert_eq!(profile_hash(Path::new("")), "cbf29ce484222325");
        assert_eq!(profile_hash(Path::new("A")), "af63dc4c8601ec8c");
    }

    #[test]
    fn names_differ_per_profile_and_ignore_case_and_slash_direction() {
        let a = instance_names(Path::new("C:\\a\\Flare"));
        let b = instance_names(Path::new("C:\\a\\Flare-e2e"));
        assert_ne!(a, b);
        assert_eq!(
            instance_names(Path::new("c:/A/flare/")),
            instance_names(Path::new("C:\\a\\Flare"))
        );
        assert!(a.mutex.starts_with("Local\\Flare.Single."));
        assert!(a.pipe.starts_with("\\\\.\\pipe\\Flare.Handoff."));
        let other = ["Pri", "sm"].concat();
        assert!(!a.mutex.contains(&other) && !a.pipe.contains(&other));
    }

    #[test]
    fn a_handoff_round_trips() {
        let h = sample();
        let bytes = encode_handoff(&h);
        assert!(bytes.starts_with(b"{\"v\":1"));
        assert_eq!(decode_handoff(&bytes), Some(h));
    }

    #[test]
    fn a_hostile_handoff_is_refused() {
        assert!(decode_handoff(b"junk").is_none());
        assert!(decode_handoff(br#"{"v":2,"cwd":"","args":[]}"#).is_none());
        let many = format!(
            r#"{{"v":1,"cwd":"","args":[{}]}}"#,
            vec!["\"a\""; 257].join(",")
        );
        assert!(decode_handoff(many.as_bytes()).is_none());
        let long = format!(r#"{{"v":1,"cwd":"","args":["{}"]}}"#, "a".repeat(32_769));
        assert!(decode_handoff(long.as_bytes()).is_none());
        assert!(decode_handoff(br#"{"v":1,"cwd":"x","args":[1]}"#).is_none());
        assert!(decode_handoff(&vec![b' '; 65_537]).is_none());
    }

    #[test]
    fn the_first_instance_wins_and_the_second_knows() {
        let n = names_for_test();
        let Acquired::First(g) = acquire(&n).unwrap() else {
            panic!("expected First");
        };
        assert!(matches!(acquire(&n).unwrap(), Acquired::Second));
        drop(g);
        assert!(matches!(acquire(&n).unwrap(), Acquired::First(_)));
    }

    #[test]
    fn a_handoff_reaches_the_running_instance() {
        let n = names_for_test();
        let (tx, rx) = mpsc::channel();
        let _server = serve(&n, move |h| {
            let _ = tx.send(h);
        })
        .unwrap();
        let h = sample();
        send_handoff(&n, &h, Duration::from_secs(3)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), h);
        let h2 = Handoff {
            cwd: "D:\\y".into(),
            args: vec!["c".into()],
        };
        send_handoff(&n, &h, Duration::from_secs(3)).unwrap();
        send_handoff(&n, &h2, Duration::from_secs(3)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), h);
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), h2);
    }

    #[test]
    fn a_handoff_waits_for_a_server_that_starts_late() {
        let n = names_for_test();
        let (tx, rx) = mpsc::channel();
        let (keep_tx, keep_rx) = mpsc::channel();
        let n2 = n.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let server = serve(&n2, move |h| {
                let _ = tx.send(h);
            })
            .unwrap();
            let _ = keep_tx.send(server);
        });
        let h = sample();
        send_handoff(&n, &h, Duration::from_secs(3)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), h);
        t.join().unwrap();
        drop(keep_rx.recv().unwrap());
    }

    #[test]
    fn a_handoff_gives_up_without_a_server() {
        let n = names_for_test();
        let started = Instant::now();
        assert!(send_handoff(&n, &sample(), Duration::from_millis(200)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn an_oversized_or_junk_write_is_dropped_and_the_server_keeps_serving() {
        let n = names_for_test();
        let (tx, rx) = mpsc::channel();
        let _server = serve(&n, move |h| {
            let _ = tx.send(h);
        })
        .unwrap();
        let n2 = n.clone();
        std::thread::spawn(move || raw_write(&n2, &vec![b'x'; 70_000]))
            .join()
            .unwrap();
        raw_write(&n, b"junk");
        let h = sample();
        send_handoff(&n, &h, Duration::from_secs(3)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), h);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn a_second_server_on_the_same_name_fails_loudly() {
        let n = names_for_test();
        let _first = serve(&n, |_| {}).unwrap();
        assert!(serve(&n, |_| {}).is_err());
    }

    #[test]
    fn stop_ends_the_server_thread() {
        let n = names_for_test();
        let server = serve(&n, |_| {}).unwrap();
        let started = Instant::now();
        server.stop();
        assert!(started.elapsed() < Duration::from_secs(2));
        let again = serve(&n, |_| {}).unwrap();
        again.stop();
    }

    #[test]
    fn stop_never_hangs_on_a_client_that_holds_the_pipe() {
        let n = names_for_test();
        let server = serve(&n, |_| {}).unwrap();
        // A client connects and never writes or closes: the one instance stays busy.
        let started = Instant::now();
        let held = loop {
            match std::fs::OpenOptions::new().write(true).open(&n.pipe) {
                Ok(f) => break f,
                Err(_) if started.elapsed() < Duration::from_secs(3) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("could not connect: {e}"),
            }
        };
        let started = Instant::now();
        server.stop();
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(held);
    }

    fn fake(sid: &str, session: u32) -> Identity {
        Identity {
            sid: sid.into(),
            session,
        }
    }

    struct Fake(Option<Identity>);
    impl IdentitySource for Fake {
        fn of_pid(&self, _pid: u32) -> Option<Identity> {
            self.0.clone()
        }
    }

    #[test]
    fn names_carry_the_user_sid_and_the_session() {
        let p = Path::new("C:\\a\\Flare");
        let a = instance_names_for(p, &fake("S-1-5-21-1", 1));
        assert!(a.mutex.contains("S-1-5-21-1.1."));
        assert!(a.pipe.contains("S-1-5-21-1.1."));
        assert_ne!(a, instance_names_for(p, &fake("S-1-5-21-2", 1)));
        assert_ne!(a, instance_names_for(p, &fake("S-1-5-21-1", 2)));
        let real = current_identity().expect("the test process has an identity");
        assert!(instance_names(p).pipe.contains(&real.sid));
        assert!(
            instance_names(p)
                .pipe
                .contains(&format!(".{}.", real.session))
        );
    }

    #[test]
    fn a_server_of_another_user_or_session_is_refused() {
        let me = fake("S-1-5-21-1", 1);
        assert!(verify_peer(&me, Some(&fake("S-1-5-21-1", 1))).is_ok());
        assert!(verify_peer(&me, Some(&fake("S-1-5-21-9", 1))).is_err());
        assert!(verify_peer(&me, Some(&fake("S-1-5-21-1", 2))).is_err());
        assert!(verify_peer(&me, None).is_err());
    }

    #[test]
    fn nothing_is_written_to_a_server_that_is_another_user() {
        let n = names_for_test();
        let (tx, rx) = mpsc::channel();
        let _server = serve(&n, move |h| {
            let _ = tx.send(h);
        })
        .unwrap();
        let stranger = Fake(Some(fake("S-1-5-21-9999", 1)));
        let r = send_handoff_with(&n, &sample(), Duration::from_secs(3), &stranger);
        assert!(matches!(r, Err(HandoffError::Untrusted(_))), "{r:?}");
        let unreadable = Fake(None);
        let r = send_handoff_with(&n, &sample(), Duration::from_secs(3), &unreadable);
        assert!(matches!(r, Err(HandoffError::Untrusted(_))), "{r:?}");
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn the_dacl_is_one_ace_for_the_current_sid() {
        let sid = current_identity().unwrap().sid;
        let sddl = dacl_sddl(&sid);
        assert_eq!(sddl, format!("D:P(A;;GA;;;{sid})"));
        assert_eq!(sddl.matches('(').count(), 1);
        let sd = security_descriptor(&sid).unwrap();
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl: *mut windows_sys::Win32::Security::ACL = std::ptr::null_mut();
        // SAFETY: `sd` is a valid descriptor; the out-pointers outlive the call.
        let ok = unsafe {
            windows_sys::Win32::Security::GetSecurityDescriptorDacl(
                sd,
                &mut present,
                &mut acl,
                &mut defaulted,
            )
        };
        assert!(ok != 0 && present != 0 && !acl.is_null());
        // SAFETY: the ACL lives inside `sd`, still allocated.
        assert_eq!(unsafe { (*acl).AceCount }, 1);
        // SAFETY: freeing the descriptor once.
        unsafe { LocalFree(sd) };
    }

    /// L11 (audit 2026-10-20): the mutex had the creating token's default DACL. An elevated first
    /// instance owns it as Administrators, a normal launch of the same user then gets ACCESS_DENIED
    /// from `CreateMutexW`, and used to run a second full instance. The mutex now carries one
    /// explicit ACE for the user's SID and a medium integrity label, whoever creates it.
    #[test]
    fn the_mutex_sddl_is_one_ace_for_the_user_and_a_medium_label() {
        let sddl = mutex_sddl("S-1-5-21-1-2-3-1001");
        assert_eq!(sddl, "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)S:(ML;;NW;;;ME)");
        let sid = current_identity().unwrap().sid;
        let sd = descriptor_from_sddl(&mutex_sddl(&sid)).unwrap();
        let (mut present, mut defaulted) = (0, 0);
        let mut acl: *mut windows_sys::Win32::Security::ACL = std::ptr::null_mut();
        // SAFETY: `sd` is a valid descriptor; the out-pointers outlive the calls.
        unsafe {
            assert!(
                windows_sys::Win32::Security::GetSecurityDescriptorDacl(
                    sd,
                    &mut present,
                    &mut acl,
                    &mut defaulted
                ) != 0
                    && present != 0
                    && !acl.is_null()
            );
            assert_eq!((*acl).AceCount, 1);
            let mut sacl: *mut windows_sys::Win32::Security::ACL = std::ptr::null_mut();
            assert!(
                windows_sys::Win32::Security::GetSecurityDescriptorSacl(
                    sd,
                    &mut present,
                    &mut sacl,
                    &mut defaulted
                ) != 0
                    && present != 0
                    && !sacl.is_null(),
                "the integrity label is there"
            );
            assert_eq!((*sacl).AceCount, 1);
            LocalFree(sd);
        }
    }

    #[test]
    fn a_mutex_this_user_cannot_open_means_a_running_instance_not_a_missing_lock() {
        use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
        assert!(matches!(
            classify_create(true, ERROR_ACCESS_DENIED),
            Ok(Created::Held)
        ));
        assert!(classify_create(true, 0).is_err());
        assert!(matches!(
            classify_create(false, ERROR_ALREADY_EXISTS),
            Ok(Created::Held)
        ));
        assert!(matches!(classify_create(false, 0), Ok(Created::Fresh)));
    }

    #[test]
    fn the_lock_still_works_with_the_explicit_label() {
        let n = names_for_test();
        let Acquired::First(g) = acquire(&n).unwrap() else {
            panic!("the first wins");
        };
        assert!(matches!(acquire(&n).unwrap(), Acquired::Second));
        drop(g);
        assert!(matches!(acquire(&n).unwrap(), Acquired::First(_)));
    }
}
