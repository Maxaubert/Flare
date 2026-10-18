//! The pipe client the harness talks to the app with.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{
    decode_reply, encode, pipe_name, Button, PointerAction, Reply, Request, MAX_LINE_BYTES,
};
use serde::Deserialize;

const GONE: &str = "the app is gone";

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
    #[serde(default)]
    pub rect: Rect,
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

/// The connection to one app's test hook.
#[derive(Debug)]
pub struct Session {
    writer: File,
    reader: BufReader<File>,
    pub profile: PathBuf,
}

impl Session {
    /// Poll the profile's pipe every 25 ms until it opens or `timeout` passes.
    pub fn connect(profile: &Path, timeout: Duration) -> Result<Session, String> {
        let name = pipe_name(profile);
        let start = Instant::now();
        loop {
            match OpenOptions::new().read(true).write(true).open(&name) {
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
        let line = encode(r);
        self.writer
            .write_all(line.as_bytes())
            .map_err(|_| GONE.to_string())?;
        self.writer.flush().map_err(|_| GONE.to_string())?;
        let mut reply = String::new();
        match self
            .reader
            .by_ref()
            .take(MAX_LINE_BYTES as u64 + 2)
            .read_line(&mut reply)
        {
            Ok(0) | Err(_) => return Err(GONE.to_string()),
            Ok(_) => {}
        }
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
