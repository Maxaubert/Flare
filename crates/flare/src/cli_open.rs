//! `flare open --on <machine> <folder> [--resume <id>]` (remote sessions PR 5): a command that asks the Flare running
//! here to open a tab that shows a session on another machine, then exits.
//!
//! It runs before the e2e set-up, the single-instance lock and any window (`main.rs`), like `--serve-remote`. It finds
//! this profile's Flare on its pipe; when none runs it starts one hidden (`--start-hidden`, rule 301) and waits for the
//! pipe. Then it says `hello` with no token (the agent grant: the same reach as the agent's `open_remote_session`
//! tool, no more) and calls `open.remote` with `front` so the new tab comes to the front. The folder is the host's: it
//! is never statted or opened here, and `--on`'s value never reaches path parsing (rule 300). The other machine's link
//! is the new tab's own worker, so the command never waits for ssh.

use std::path::Path;
use std::time::Duration;

use flare_ctl::framing::Wire;
use flare_ctl::{Client, ClientError, HelloParams, Method};
use serde_json::{Value, json};

use crate::config::OpenOn;

/// How long the connect and the `hello` may take.
const CONNECT_WAIT: Duration = Duration::from_secs(5);

/// Pure: `open.remote`'s params for the command: always in front.
pub fn open_params(o: &OpenOn) -> Value {
    let mut p = json!({"machine": o.machine, "folder": o.folder, "front": true});
    if let Some(r) = &o.resume {
        p["resume"] = json!(r);
    }
    p
}

/// Says `hello` (no token) and asks for the tab. The line to print on success; the reason on failure.
pub fn open_with<W: Wire>(c: &mut Client<W>, o: &OpenOn) -> Result<String, String> {
    c.hello(
        &HelloParams {
            client: "flare-open".into(),
            version: crate::app_facts::version().into(),
            token: None,
            tab_token: None,
            no_events: true,
        },
        CONNECT_WAIT,
    )
    .map_err(|e| format!("the running Flare did not take the command: {e}"))?;
    let m = Method::OpenRemoteSession;
    match c.call(m, open_params(o), m.timeout() + Duration::from_secs(5)) {
        Ok(_) => Ok(format!("Opened a tab on {} in {}.", o.machine, o.folder)),
        Err(ClientError::Remote(e)) => Err(format!("Flare refused: {}", e.message)),
        Err(e) => Err(format!("Flare did not answer: {e}")),
    }
}

/// Runs the command. The exit code: 0 when the tab is open, 1 otherwise (one line on stderr says why).
pub fn run(o: &OpenOn, user_data_dir: Option<&Path>, e2e: bool) -> i32 {
    let (_, pipe) = crate::serve_remote::profile_and_pipe(user_data_dir);
    let Some(pipe) = pipe else {
        eprintln!("flare: no pipe name for this user");
        return 1;
    };
    let starter = crate::hidden_start::SystemStarter {
        pipe: Some(pipe.clone()),
        user_data_dir: user_data_dir.map(Path::to_path_buf),
        e2e,
    };
    if let Err(e) = crate::hidden_start::ensure_running(
        &starter,
        crate::hidden_start::PIPE_WAIT,
        crate::hidden_start::PIPE_POLL,
    ) {
        eprintln!("flare: {}", e.message);
        return 1;
    }
    let mut c = match flare_ctl::connect(&pipe, CONNECT_WAIT) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("flare: could not reach the running Flare: {e}");
            return 1;
        }
    };
    match open_with(&mut c, o) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(why) => {
            eprintln!("flare: {why}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_ctl::framing::testwire::pair;
    use flare_ctl::session::{ConnHandle, Shared, serve_connection};
    use flare_ctl::{CallContext, CtlError, Dispatcher, Grant, ServerConfig, code};
    use std::sync::{Arc, Mutex};

    /// The running Flare as the command meets it: no token is the agent grant; `open.remote` is recorded.
    #[derive(Default)]
    struct Running {
        seen: Mutex<Vec<(Method, Value, String)>>,
    }

    impl Dispatcher for Running {
        fn resolve_token(&self, _: &str) -> Option<Grant> {
            None
        }
        fn resolve_tab_token(&self, _: &str) -> Option<String> {
            None
        }
        fn call(&self, ctx: &CallContext, m: Method, p: Value) -> Result<Value, CtlError> {
            self.seen
                .lock()
                .unwrap()
                .push((m, p.clone(), ctx.grant.kind().into()));
            if p["machine"] == "nas" {
                return Err(CtlError::new(code::NOT_FOUND, "no machine called nas"));
            }
            Ok(json!({"tab": "t3"}))
        }
    }

    fn client_to(d: Arc<Running>) -> Client<flare_ctl::framing::testwire::MemWire> {
        let shared = Shared::new(d, ServerConfig::default());
        let (server_end, client) = pair();
        let handle = ConnHandle::new(1);
        shared.hub.add(handle.clone());
        std::thread::spawn(move || {
            serve_connection(&server_end, &shared, &handle);
            server_end.close();
        });
        Client::new(client)
    }

    fn mac(resume: Option<&str>) -> OpenOn {
        OpenOn {
            machine: "mac".into(),
            folder: "/Users/max/proj".into(),
            resume: resume.map(str::to_string),
        }
    }

    /// The command speaks as the agent (no token) and asks for the tab in front; the host folder goes as it was typed.
    #[test]
    fn the_command_asks_the_running_flare_as_the_agent_for_a_tab_in_front() {
        let d = Arc::new(Running::default());
        let mut c = client_to(d.clone());
        let id = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";
        let line = open_with(&mut c, &mac(Some(id))).unwrap();
        assert_eq!(line, "Opened a tab on mac in /Users/max/proj.");
        let seen = d.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, Method::OpenRemoteSession);
        assert_eq!(
            seen[0].1,
            json!({"machine": "mac", "folder": "/Users/max/proj", "front": true, "resume": id})
        );
        assert_eq!(seen[0].2, "agent");
    }

    #[test]
    fn a_refusal_is_one_line_with_the_running_flares_reason() {
        let d = Arc::new(Running::default());
        let mut c = client_to(d);
        let o = OpenOn {
            machine: "nas".into(),
            ..mac(None)
        };
        let why = open_with(&mut c, &o).unwrap_err();
        assert_eq!(why, "Flare refused: no machine called nas");
        assert!(
            !open_params(&mac(None))
                .as_object()
                .unwrap()
                .contains_key("resume")
        );
    }
}
