//! The pipe client against a fake server thread speaking the proto over a real named pipe.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use flare_e2e::chord;
use flare_e2e::session::Session;
use flare_e2e_proto::{decode_request, encode_reply, pipe_name, Reply, Request};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};

fn profile(tag: &str) -> PathBuf {
    PathBuf::from(format!(
        r"C:\flare-e2e-session-test\{tag}-{}",
        std::process::id()
    ))
}

/// Create the pipe for `profile` and wait for one client; hand its file to `serve`.
fn serve<F: FnOnce(File) + Send + 'static>(
    profile: &std::path::Path,
    delay: Duration,
    serve: F,
) -> std::thread::JoinHandle<()> {
    let name: Vec<u16> = pipe_name(profile).encode_utf16().chain(Some(0)).collect();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        // SAFETY: `name` is a NUL-terminated wide string that outlives the call.
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                65536,
                65536,
                0,
                std::ptr::null(),
            )
        };
        assert!(h != INVALID_HANDLE_VALUE, "CreateNamedPipeW failed");
        // SAFETY: `h` is the valid pipe handle made above; a client connecting first is fine.
        unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) };
        // SAFETY: `h` is an owned, valid handle; the File closes it on drop.
        serve(unsafe { File::from_raw_handle(h as _) });
    })
}

fn answer_with(file: File, mut reply: impl FnMut(Request) -> Option<Reply>) {
    let mut writer = file.try_clone().expect("clone");
    let mut reader = BufReader::new(file);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let Ok(req) = decode_request(&line) else {
            return;
        };
        match reply(req) {
            Some(r) => {
                let _ = writer.write_all(encode_reply(&r).as_bytes());
            }
            None => return,
        }
    }
}

#[test]
fn connect_waits_for_the_pipe_then_succeeds() {
    let p = profile("wait");
    let server = serve(&p, Duration::from_millis(200), |f| {
        answer_with(f, |_| Some(Reply::Ok))
    });
    let start = Instant::now();
    let session = Session::connect(&p, Duration::from_secs(5));
    assert!(session.is_ok(), "{:?}", session.err());
    assert!(start.elapsed() >= Duration::from_millis(100));
    drop(session);
    server.join().expect("server");
}

#[test]
fn connect_times_out_with_a_message() {
    let p = profile("timeout");
    let start = Instant::now();
    let err = Session::connect(&p, Duration::from_millis(150)).expect_err("no pipe");
    assert!(err.contains("did not open"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn call_round_trips_a_probe() {
    let p = profile("probe");
    let server = serve(&p, Duration::ZERO, |f| {
        answer_with(f, |req| match req {
            Request::Probe { name, arg } => Some(Reply::Value {
                value: serde_json::json!({ "name": name, "arg": arg }),
            }),
            _ => Some(Reply::Ok),
        })
    });
    let mut s = Session::connect(&p, Duration::from_secs(5)).expect("connect");
    let v = s
        .probe("chrome", serde_json::json!({"k": 1}))
        .expect("probe");
    assert_eq!(v["name"], "chrome");
    assert_eq!(v["arg"]["k"], 1);
    assert!(s.probe_str("x").expect("null arg")["arg"].is_null());
    drop(s);
    server.join().expect("server");
}

#[test]
fn an_err_reply_is_an_err_result() {
    let p = profile("err");
    let server = serve(&p, Duration::ZERO, |f| {
        answer_with(f, |_| {
            Some(Reply::Err {
                message: "no such probe".into(),
            })
        })
    });
    let mut s = Session::connect(&p, Duration::from_secs(5)).expect("connect");
    assert_eq!(s.key("Enter").expect_err("err"), "no such probe");
    assert_eq!(s.probe_str("nope").expect_err("err"), "no such probe");
    drop(s);
    server.join().expect("server");
}

#[test]
fn a_dropped_pipe_is_the_app_is_gone() {
    let p = profile("gone");
    let server = serve(&p, Duration::ZERO, |f| answer_with(f, |_| None));
    let mut s = Session::connect(&p, Duration::from_secs(5)).expect("connect");
    assert_eq!(s.hello().expect_err("gone"), "the app is gone");
    assert_eq!(
        s.click(1.0, 2.0).expect_err("still gone"),
        "the app is gone"
    );
    server.join().expect("server");
}

#[test]
fn chord_parse_matches_the_app_grammar() {
    let good = [
        "Ctrl+T",
        "Ctrl+Shift+KeyC",
        "Shift+Enter",
        "Alt+b",
        "F11",
        "Ctrl+,",
        "Ctrl+Tab",
        "ArrowUp",
        "Ctrl+Alt+q",
        "?",
        "Ctrl+Shift+T",
        "KeyW",
        "Digit1",
        "Ctrl+Digit1",
        "F5",
        "Escape",
        "Backspace",
        "Delete",
        "PageUp",
        "Home",
        "End",
        "Numpad0",
        "Ctrl+KeyC=\u{441}",
        "ctrl+shift+Backquote",
        "Meta+Space",
    ];
    assert_eq!(good.len(), 25);
    for c in good {
        assert!(
            chord::parse(c).is_ok(),
            "{c} should parse: {:?}",
            chord::parse(c)
        );
    }
    for bad in [
        "Ctrl+",
        "Foo+T",
        "",
        "Ctrl+Nope",
        "Ctrl+Ctrl+T",
        "Ctrl+KeyC=",
    ] {
        assert!(chord::parse(bad).is_err(), "{bad:?} should not parse");
    }
    let (mods, key) = chord::parse("Ctrl+Shift+T").expect("parse");
    assert_eq!((mods.len(), key.as_str()), (2, "T"));
    assert_eq!(chord::label("ctrl+shift+KeyT"), "Ctrl+Shift+T");
    assert_eq!(chord::label("Alt+b"), "Alt+B");
    assert_eq!(chord::label("Ctrl+Digit1"), "Ctrl+1");
}
