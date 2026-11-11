//! The pipe client the harness talks to the app with.

#[cfg(windows)]
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{
    decode_reply, encode, pipe_name, Button, PointerAction, Reply, Request, MAX_LINE_BYTES,
};
use serde::Deserialize;

const GONE: &str = "the app is gone";

/// Reads one reply line. A line over the limit is read to its end and thrown away (the next
/// reply stays in step) and comes back as an error (L56).
fn read_reply_line<R: BufRead>(r: &mut R) -> Result<String, String> {
    let mut reply = String::new();
    let mut over = false;
    loop {
        let mut chunk = String::new();
        match r
            .by_ref()
            .take(MAX_LINE_BYTES as u64 + 2)
            .read_line(&mut chunk)
        {
            Ok(0) | Err(_) => return Err(GONE.to_string()),
            Ok(_) => {}
        }
        let done = chunk.ends_with('\n');
        if !over {
            reply.push_str(&chunk);
            over = reply.len() > MAX_LINE_BYTES + 1;
        }
        if done {
            break;
        }
    }
    if over {
        return Err(format!(
            "the reply is over the {MAX_LINE_BYTES} byte line limit"
        ));
    }
    Ok(reply)
}

/// macOS (rule 1106): the request with every chord in the Mac app's spelling (`chord::mac_app_chord`),
/// so a scenario written with the Windows keys presses the same command on a Mac.
#[cfg(target_os = "macos")]
fn mac_keys(r: &Request) -> Request {
    use crate::chord::mac_app_chord;
    let mut r = r.clone();
    match &mut r {
        Request::Key { chord, .. }
        | Request::KeyPress { chord }
        | Request::Latency { chord, .. }
        | Request::KeyAtPrompt { chord, .. } => *chord = mac_app_chord(chord),
        _ => {}
    }
    r
}

/// The app's `Hello` reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Hello {
    pub proto: u32,
    pub version: String,
    pub renderer: String,
    pub scale: f32,
    pub pid: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rect {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub w: f32,
    #[serde(default)]
    pub h: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TabState {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub front: bool,
    #[serde(default)]
    pub kind: String,
    /// The tab's box in the strip. Down the side (the default since 2026-11-11) the probe answers
    /// `null`, which reads as an empty box: before, one `null` failed the whole `app.state`, so a
    /// vertical window seemed to have no tabs at all.
    #[serde(default, deserialize_with = "null_is_empty")]
    pub rect: Rect,
}

fn null_is_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Rect, D::Error> {
    Ok(Option::<Rect>::deserialize(d)?.unwrap_or_default())
}

/// A typed view of the `app.state` probe: the fields scenarios use most. Unknown fields are ignored.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub renderer: String,
    #[serde(default)]
    pub scale: f32,
    #[serde(default)]
    pub front: Option<String>,
    #[serde(default)]
    pub tabs: Vec<TabState>,
    #[serde(default)]
    pub quitting: bool,
}

/// The hook's connection: the named pipe opened as a file on Windows, the Unix socket on macOS (rule 1103).
#[cfg(windows)]
type Conn = File;
#[cfg(not(windows))]
type Conn = std::os::unix::net::UnixStream;

/// Opens the hook's endpoint once (no wait).
#[cfg(windows)]
fn open_conn(name: &str) -> std::io::Result<Conn> {
    OpenOptions::new().read(true).write(true).open(name)
}

#[cfg(not(windows))]
fn open_conn(name: &str) -> std::io::Result<Conn> {
    Conn::connect(name)
}

/// The connection to one app's test hook.
#[derive(Debug)]
pub struct Session {
    writer: Conn,
    reader: BufReader<Conn>,
    pub profile: PathBuf,
}

impl Session {
    /// Poll the profile's pipe every 25 ms until it opens or `timeout` passes.
    pub fn connect(profile: &Path, timeout: Duration) -> Result<Session, String> {
        let name = pipe_name(profile);
        let start = Instant::now();
        loop {
            match open_conn(&name) {
                Ok(file) => {
                    let reader = file.try_clone().map_err(|e| format!("pipe clone: {e}"))?;
                    return Ok(Session {
                        writer: file,
                        reader: BufReader::new(reader),
                        profile: profile.to_path_buf(),
                    });
                }
                Err(e) => {
                    if start.elapsed() >= timeout {
                        return Err(format!(
                            "the test hook pipe {name} did not open within {} ms ({e})",
                            timeout.as_millis()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }

    /// Write one request line, read one reply line. An `Err` reply becomes `Err(message)`.
    pub fn call(&mut self, r: &Request) -> Result<Reply, String> {
        #[cfg(target_os = "macos")]
        let mac = mac_keys(r);
        #[cfg(target_os = "macos")]
        let r = &mac;
        self.call_as_is(r)
    }

    /// As `call`, with every chord sent as written: no Mac spelling (rule 1112). For the keys a scenario
    /// means as the program's own, which on a Mac are the Ctrl keys, while Cmd is the app's.
    pub fn call_as_is(&mut self, r: &Request) -> Result<Reply, String> {
        let line = encode(r);
        self.writer
            .write_all(line.as_bytes())
            .map_err(|_| GONE.to_string())?;
        self.writer.flush().map_err(|_| GONE.to_string())?;
        let reply = read_reply_line(&mut self.reader)?;
        match decode_reply(&reply)? {
            Reply::Err { message } => Err(message),
            other => Ok(other),
        }
    }

    fn ok(&mut self, r: &Request) -> Result<(), String> {
        match self.call(r)? {
            Reply::Ok => Ok(()),
            other => Err(format!("expected Ok, got {other:?}")),
        }
    }

    pub fn hello(&mut self) -> Result<Hello, String> {
        match self.call(&Request::Hello)? {
            Reply::Hello {
                proto,
                version,
                renderer,
                scale,
                pid,
            } => Ok(Hello {
                proto,
                version,
                renderer,
                scale,
                pid,
            }),
            other => Err(format!("expected Hello, got {other:?}")),
        }
    }

    pub fn key(&mut self, chord: &str) -> Result<(), String> {
        self.ok(&Request::KeyPress {
            chord: chord.to_string(),
        })
    }

    /// Presses `chord` exactly as written, on every system (see `call_as_is`).
    pub fn key_as_is(&mut self, chord: &str) -> Result<(), String> {
        match self.call_as_is(&Request::KeyPress {
            chord: chord.to_string(),
        })? {
            Reply::Ok => Ok(()),
            other => Err(format!("expected Ok, got {other:?}")),
        }
    }

    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.ok(&Request::Text {
            text: text.to_string(),
        })
    }

    pub fn click(&mut self, x: f32, y: f32) -> Result<(), String> {
        self.ok(&Request::Pointer {
            x,
            y,
            action: PointerAction::Click {
                button: Button::Left,
                count: 1,
            },
        })
    }

    pub fn right_click(&mut self, x: f32, y: f32) -> Result<(), String> {
        self.ok(&Request::Pointer {
            x,
            y,
            action: PointerAction::Click {
                button: Button::Right,
                count: 1,
            },
        })
    }

    pub fn probe(
        &mut self,
        name: &str,
        arg: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match self.call(&Request::Probe {
            name: name.to_string(),
            arg,
        })? {
            Reply::Value { value } => Ok(value),
            Reply::Ok => Ok(serde_json::Value::Null),
            other => Err(format!("expected Value, got {other:?}")),
        }
    }

    pub fn probe_str(&mut self, name: &str) -> Result<serde_json::Value, String> {
        self.probe(name, serde_json::Value::Null)
    }

    pub fn snapshot(&mut self, path: &Path) -> Result<(u32, u32), String> {
        let req = Request::Snapshot {
            path: path.to_string_lossy().into_owned(),
            mode: None,
        };
        match self.call(&req)? {
            Reply::Png { w, h, .. } => Ok((w, h)),
            other => Err(format!("expected Png, got {other:?}")),
        }
    }

    pub fn state(&mut self) -> Result<AppState, String> {
        let value = self.probe_str("app.state")?;
        serde_json::from_value(value)
            .map_err(|e| format!("app.state is not the expected shape: {e}"))
    }
}

#[cfg(test)]
mod reply_line_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_short_reply_is_one_line() {
        let mut r = BufReader::new(Cursor::new(b"{\"a\":1}\nnext\n".to_vec()));
        assert_eq!(read_reply_line(&mut r).unwrap(), "{\"a\":1}\n");
        assert_eq!(read_reply_line(&mut r).unwrap(), "next\n");
    }

    #[test]
    fn an_over_long_reply_is_an_error_and_the_next_reply_stays_in_step() {
        let mut data = vec![b'a'; MAX_LINE_BYTES + 500];
        data.push(b'\n');
        data.extend_from_slice(b"{\"ok\":true}\n");
        let mut r = BufReader::new(Cursor::new(data));
        let e = read_reply_line(&mut r).unwrap_err();
        assert!(e.contains("over"), "{e}");
        assert_eq!(read_reply_line(&mut r).unwrap(), "{\"ok\":true}\n");
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn a_vertical_window_s_tabs_read_with_an_empty_box() {
        let v = serde_json::json!({
            "scale": 1.0,
            "tabs": [
                { "id": "t1", "front": true, "rect": null },
                { "id": "t2", "rect": { "x": 4.0, "y": 0.0, "w": 90.0, "h": 36.0 } },
                { "id": "t3" }
            ]
        });
        let s: AppState = serde_json::from_value(v).expect("a null rect is not an error");
        assert_eq!(s.tabs.len(), 3);
        assert_eq!(s.tabs[0].rect.w, 0.0);
        assert_eq!(s.tabs[1].rect.w, 90.0);
        assert_eq!(s.tabs[2].rect.w, 0.0);
    }
}
