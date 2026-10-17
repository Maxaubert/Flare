//! Single instance (Task 15, decision D4): a named mutex says who is first, a named pipe carries the
//! second launch's folders to the first. Both names are keyed by a hash of the profile folder, so an
//! e2e run (its own profile) never hands its folders to the owner's running Flare.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND, ReadFile,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

pub const MAX_HANDOFF: usize = 65_536;
pub const MAX_ARGS: usize = 256;
pub const MAX_ARG_LEN: usize = 32_768;

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

/// Mutex `Local\Flare.Single.<hash>`, pipe `\\.\pipe\Flare.Handoff.<hash>`.
pub fn instance_names(profile: &Path) -> InstanceNames {
    let hash = profile_hash(profile);
    InstanceNames {
        mutex: format!("Local\\Flare.Single.{hash}"),
        pipe: format!("\\\\.\\pipe\\Flare.Handoff.{hash}"),
    }
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

pub fn acquire(names: &InstanceNames) -> Result<Acquired, String> {
    let name = wide(&names.mutex);
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer that outlives the call; no security attributes.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    // SAFETY: reads the calling thread's last error; nothing ran between the call above and this.
    let err = unsafe { GetLastError() };
    if handle.is_null() {
        return Err(format!("CreateMutexW failed (error {err})"));
    }
    if err == ERROR_ALREADY_EXISTS {
        // SAFETY: the handle is valid and owned by us.
        unsafe { CloseHandle(handle) };
        return Ok(Acquired::Second);
    }
    Ok(Acquired::First(InstanceGuard {
        handle: handle as usize,
    }))
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
        // Connect once so the blocked ConnectNamedPipe returns.
        let _ = std::fs::OpenOptions::new().write(true).open(&self.pipe);
        let _ = thread.join(); // sync-ok: stopping the worker at quit
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
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer that outlives the call; no security attributes.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            65_536,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: reads the thread's last error right after the failed call.
        let err = unsafe { GetLastError() };
        return Err(format!("CreateNamedPipeW failed (error {err})"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let raw = handle as usize;
    let thread = std::thread::Builder::new()
        .name("flare-handoff".into())
        .spawn(move || {
            loop {
                // SAFETY: valid pipe handle owned by this thread; no OVERLAPPED, so it blocks.
                let connected = unsafe { ConnectNamedPipe(raw as _, std::ptr::null_mut()) }; // sync-ok: worker thread waits for a client
                // SAFETY: last error of the call above.
                let err = unsafe { GetLastError() };
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                if (connected != 0 || err == ERROR_PIPE_CONNECTED)
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

pub fn send_handoff(
    names: &InstanceNames,
    h: &Handoff,
    total_wait: Duration,
) -> Result<(), String> {
    // SAFETY: plain call with a constant; lets the first instance raise its window.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let started = Instant::now();
    let bytes = encode_handoff(h);
    loop {
        match std::fs::OpenOptions::new().write(true).open(&names.pipe) {
            Ok(mut file) => {
                return file
                    .write_all(&bytes)
                    .map_err(|e| format!("hand-off write failed: {e}"));
            }
            Err(e) => {
                let retry = e.kind() == std::io::ErrorKind::NotFound
                    || e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32);
                if !retry {
                    return Err(format!("hand-off open failed: {e}"));
                }
                if started.elapsed() >= total_wait {
                    return Err(format!("no running Flare answered: {e}"));
                }
                std::thread::sleep(Duration::from_millis(50)); // sync-ok: second instance, nothing else runs
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

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
        assert!(!a.mutex.contains("Prism") && !a.pipe.contains("Prism"));
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
}
