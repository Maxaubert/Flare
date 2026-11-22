//! `flare --install-staged [--check]` (rule 1326): a command that asks the Flare running here to install its staged
//! build the way a click on "Build ready" does, armed until every agent is idle, prints the answer and exits.
//!
//! It runs before the e2e set-up, the single-instance lock and any window (`main.rs`), like `flare open --on`. It finds
//! this profile's Flare on its pipe and never starts one: with none running there is nothing to install into. It says
//! `hello` with no token (the agent grant: the same reach as the agent's `install_staged_build` tool, no more) and
//! calls `install.staged`. The answer is one line (`flare_ctl::install::describe`); the exit code is 0 for armed,
//! installing or staged, 2 for nothing staged, 3 for refused and 1 when Flare could not be asked.

use std::path::Path;
use std::time::Duration;

use flare_ctl::framing::Wire;
use flare_ctl::install::describe;
use flare_ctl::{Client, ClientError, HelloParams, Method};
use serde_json::json;

/// How long the connect and the `hello` may take.
const CONNECT_WAIT: Duration = Duration::from_secs(5);

/// Says `hello` (no token) and asks. The exit code and the line to print.
pub fn install_with<W: Wire>(c: &mut Client<W>, check: bool) -> (i32, String) {
    if let Err(e) = c.hello(
        &HelloParams {
            client: "flare-install-staged".into(),
            version: crate::app_facts::version().into(),
            token: None,
            tab_token: None,
            no_events: true,
        },
        CONNECT_WAIT,
    ) {
        return (
            1,
            format!("the running Flare did not take the command: {e}"),
        );
    }
    let m = Method::InstallStaged;
    let params = if check {
        json!({"check": true})
    } else {
        json!({})
    };
    match c.call(m, params, m.timeout() + Duration::from_secs(5)) {
        Ok(answer) => {
            let (state, line) = describe(&answer);
            (state.exit_code(), line)
        }
        Err(ClientError::Remote(e)) => (3, format!("refused: {}.", e.message)),
        Err(e) => (1, format!("Flare did not answer: {e}")),
    }
}

/// Runs the command.
pub fn run(check: bool, user_data_dir: Option<&Path>) -> i32 {
    let (_, pipe) = crate::serve_remote::profile_and_pipe(user_data_dir);
    let Some(pipe) = pipe else {
        eprintln!("flare: no pipe name for this user");
        return 1;
    };
    let mut c = match flare_ctl::connect(&pipe, CONNECT_WAIT) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "flare: no Flare runs on this profile, so there is nothing to install into ({e})"
            );
            return 1;
        }
    };
    let (code, line) = install_with(&mut c, check);
    if code == 0 {
        println!("{line}");
    } else {
        eprintln!("flare: {line}");
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_ctl::framing::testwire::pair;
    use flare_ctl::session::{ConnHandle, Shared, serve_connection};
    use flare_ctl::{CallContext, CtlError, Dispatcher, Grant, ServerConfig};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    /// The running Flare as the command meets it: no token is the agent grant, which is checked as the server does.
    struct Running {
        seen: Mutex<Vec<(Method, Value, String)>>,
        answer: Value,
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
                .push((m, p, ctx.grant.kind().into()));
            Ok(self.answer.clone())
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

    fn running(answer: Value) -> Arc<Running> {
        Arc::new(Running {
            seen: Mutex::new(Vec::new()),
            answer,
        })
    }

    /// The command speaks as the agent (no token), arms with no params and prints the answer with commit and signer.
    #[test]
    fn the_command_asks_as_the_agent_and_prints_the_answer() {
        let d = running(
            json!({"state": "armed", "short": "65273b3", "signer": "Developer ID (WV7BY8CUM8)"}),
        );
        let mut c = client_to(d.clone());
        let (code, line) = install_with(&mut c, false);
        assert_eq!(code, 0);
        assert!(
            line.starts_with("armed: build 65273b3 (Developer ID (WV7BY8CUM8))"),
            "{line}"
        );
        let seen = d.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, Method::InstallStaged);
        assert_eq!(seen[0].1, json!({}));
        assert_eq!(seen[0].2, "agent");
    }

    #[test]
    fn check_nothing_staged_and_a_refusal_have_their_own_exit_codes() {
        let d = running(json!({"state": "staged", "short": "65273b3"}));
        let mut c = client_to(d.clone());
        assert_eq!(install_with(&mut c, true).0, 0);
        assert_eq!(d.seen.lock().unwrap()[0].1, json!({"check": true}));
        let mut c = client_to(running(
            json!({"state": "none", "why": "no build is staged"}),
        ));
        assert_eq!(
            install_with(&mut c, false),
            (2, "none: no build is staged.".to_string())
        );
        let mut c = client_to(running(
            json!({"state": "refused", "why": "the staged build is ad hoc, not signed with Developer ID"}),
        ));
        let (code, line) = install_with(&mut c, false);
        assert_eq!(code, 3);
        assert!(line.contains("not signed with Developer ID"), "{line}");
    }
}
