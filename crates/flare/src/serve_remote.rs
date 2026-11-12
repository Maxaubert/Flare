//! `flare --serve-remote` (remote sessions PR 2): the host end of a remote link, run by ssh for the viewing Flare
//! (`ssh <host> <flare> --serve-remote`).
//!
//! A headless bridge. It owns no tab, opens no window and takes no single-instance lock (rule 283): `main.rs` branches
//! here before the e2e set-up and the instance code. It answers the link's `remote.hello` itself (the remote grant
//! comes from its own stdio, rule 276) and relays every other `remote.*` call to the Flare that runs on this machine
//! under the same profile, over that Flare's pipe, as the bridge: `hello` with the bridge key the running Flare wrote
//! into the profile (rule 284). When no Flare runs, the hello still answers and every relayed call is `unavailable`
//! (PR 5 starts one hidden instead).
//!
//! Stdout carries only protocol lines (rule 277): `StdioWire::from_process` is its one writer, and logs go to stderr.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use flare_ctl::framing::Wire;
use flare_ctl::{
    CallContext, Client, ClientError, ConnOrigin, CtlError, Dispatcher, Grant, HelloParams, Method,
    ServerConfig, code,
};
use serde_json::Value;

/// How long the bridge waits for the running Flare's pipe to take a connection.
const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// What the bridge leaves of a method's own time for the link: the relayed call ends first, so the answer is the
/// bridge's (with a reason) and not the serve side's plain `timeout`.
const RELAY_MARGIN: Duration = Duration::from_secs(2);

/// Opens a connection to the running Flare (the real one: its pipe; a test: an in-memory wire).
pub type Connect<W> = Box<dyn Fn() -> Result<Client<W>, ClientError> + Send + Sync>;
/// Reads the bridge key the running Flare wrote (the real one: the profile's file).
pub type KeyOf = Box<dyn Fn() -> Option<String> + Send + Sync>;

/// The serve process's dispatcher: the remote grant for its own link, every `remote.*` call but the hello relayed.
pub struct BridgeDispatcher<W: Wire + Send + 'static> {
    connect: Connect<W>,
    key_of: KeyOf,
    /// One connection to the running Flare, opened at the first relayed call and again after it broke.
    upstream: Mutex<Option<Client<W>>>,
}

fn unavailable(msg: impl Into<String>) -> CtlError {
    CtlError::new(code::UNAVAILABLE, msg)
}

/// Whether a relayed call may be sent again on a fresh connection when the first one broke: the calls that change
/// nothing twice. Typing, opening and closing are never sent twice.
fn may_retry(m: Method) -> bool {
    matches!(
        m,
        Method::RemoteLs
            | Method::RemoteTabs
            | Method::RemoteAttach
            | Method::RemoteDetach
            | Method::RemoteResize
            | Method::RemoteOutput
            | Method::RemoteSnapshot
    )
}

impl<W: Wire + Send + 'static> BridgeDispatcher<W> {
    pub fn new(connect: Connect<W>, key_of: KeyOf) -> Self {
        Self {
            connect,
            key_of,
            upstream: Mutex::new(None),
        }
    }

    fn lk(&self) -> MutexGuard<'_, Option<Client<W>>> {
        self.upstream.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A connection that said `hello` with the bridge key and was given the bridge grant.
    fn open(&self) -> Result<Client<W>, CtlError> {
        let not_running = || unavailable("Flare is not running on this machine");
        let key = (self.key_of)().ok_or_else(not_running)?;
        let mut c = (self.connect)().map_err(|_| not_running())?;
        let info = c
            .hello(
                &HelloParams {
                    client: "flare-serve-remote".into(),
                    version: crate::app_facts::version().into(),
                    token: Some(key),
                    tab_token: None,
                    no_events: true,
                },
                CONNECT_WAIT,
            )
            .map_err(|e| match e {
                // A key from an earlier run of that Flare: it is not running with this one any more.
                ClientError::Remote(e) if e.code == code::UNAUTHORIZED => not_running(),
                e => unavailable(format!("the running Flare did not take the bridge: {e}")),
            })?;
        if info.grant_kind != "bridge" {
            return Err(CtlError::new(
                code::INTERNAL,
                format!(
                    "the running Flare gave the bridge the {} grant",
                    info.grant_kind
                ),
            ));
        }
        Ok(c)
    }

    /// Relays one call. A connection that broke is opened again; a call that changes nothing is sent once more on it.
    fn relay(&self, method: Method, params: Value) -> Result<Value, CtlError> {
        let wait = method.timeout().saturating_sub(RELAY_MARGIN);
        let mut up = self.lk();
        for attempt in 0..2 {
            if up.is_none() {
                *up = Some(self.open()?);
            }
            let c = up.as_mut().expect("just opened");
            match c.call(method, params.clone(), wait) {
                Ok(v) => return Ok(v),
                // The running Flare's own answer, error or not, is the link's.
                Err(ClientError::Remote(e)) => {
                    if code::closes_client(&e.code) {
                        *up = None;
                    }
                    return Err(e);
                }
                Err(ClientError::Timeout) => {
                    // A late answer would arrive out of turn on this connection: start a fresh one next time.
                    *up = None;
                    return Err(CtlError::new(
                        code::TIMEOUT,
                        format!("Flare did not answer {} in time", method.name()),
                    ));
                }
                Err(e) => {
                    *up = None;
                    if attempt == 0 && may_retry(method) {
                        log::info!("serve-remote: the pipe broke ({e}); connecting again");
                        continue;
                    }
                    return Err(unavailable(format!("the running Flare went away: {e}")));
                }
            }
        }
        Err(unavailable("the running Flare went away"))
    }
}

impl<W: Wire + Send + 'static> Dispatcher for BridgeDispatcher<W> {
    /// The link is the only caller and has no token.
    fn resolve_token(&self, _token: &str) -> Option<Grant> {
        None
    }

    fn resolve_tab_token(&self, _tab_token: &str) -> Option<String> {
        None
    }

    /// The `remote.*` methods; whether the running Flare answers them is its own matter.
    fn capabilities(&self) -> Vec<Method> {
        Method::ALL
            .into_iter()
            .filter(|m| m.namespace() == flare_ctl::Namespace::Remote)
            .collect()
    }

    /// This process's own stdio is the link (rule 276).
    fn remote_grant(&self, origin: ConnOrigin) -> Option<Grant> {
        (origin == ConnOrigin::Stdio).then_some(Grant::Remote)
    }

    fn call(&self, ctx: &CallContext, method: Method, params: Value) -> Result<Value, CtlError> {
        if ctx.grant != Grant::Remote || method == Method::RemoteHello {
            return Err(CtlError::new(
                code::FORBIDDEN,
                "the serve process relays remote calls only",
            ));
        }
        self.relay(method, params)
    }

    /// The link closed: the bridge's connection goes too, so the running Flare hears the viewer is gone (its tabs stop
    /// being followed and take the window's size again; their shells go on, rule 280).
    fn client_gone(&self, _ctx: &CallContext) {
        self.lk().take();
    }
}

/// This machine's name for the other end: the computer's name.
fn machine_name() -> String {
    ServerConfig::default().machine
}

/// The profile folder and pipe the running Flare of this profile uses (the same rules as a launch).
pub fn profile_and_pipe(user_data_dir: Option<&Path>) -> (PathBuf, Option<String>) {
    let appdata = flare_win::paths::roaming_app_data();
    let profile = crate::startup::profile_dir(user_data_dir, appdata.as_deref());
    let pipe = crate::ctl_app::pipe_for(user_data_dir, &profile);
    (profile, pipe)
}

/// Runs the serve process until the link closes. The exit code: 0 when the link ended, 2 when this process has no
/// stdio to serve (a start with no stdin or stdout handle, which ssh never makes).
pub fn run(user_data_dir: Option<&Path>) -> i32 {
    let wire = match flare_ctl::StdioWire::from_process() {
        Ok(w) => w,
        Err(e) => {
            // Stderr only: stdout belongs to the link (rule 277).
            eprintln!("flare: --serve-remote has no stdio to serve: {e}");
            return 2;
        }
    };
    let (profile, pipe) = profile_and_pipe(user_data_dir);
    log::info!(
        "serve-remote: relaying to the Flare of {}",
        profile.display()
    );
    let connect: Connect<flare_ctl::PipeEnd> = Box::new(move || {
        let pipe = pipe
            .clone()
            .ok_or_else(|| ClientError::Io(std::io::Error::other("no pipe name for this user")))?;
        flare_ctl::connect(&pipe, CONNECT_WAIT)
    });
    let key_of: KeyOf = Box::new(move || crate::remote_host::read_bridge_key(&profile));
    let dispatcher = Arc::new(BridgeDispatcher::new(connect, key_of));
    let shared = flare_ctl::session::Shared::new_stdio(
        dispatcher,
        ServerConfig {
            machine: machine_name(),
            ..ServerConfig::stdio()
        },
    );
    flare_ctl::serve_stdio(&wire, &shared);
    log::info!("serve-remote: the link closed");
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appctl::{Post, UiMsg};
    use crate::ctl_app::{AppDispatcher, CtlOp};
    use crate::remote_host::RemoteHost;
    use flare_ctl::framing::testwire::{MemWire, pair};
    use flare_ctl::session::{ConnHandle, Shared, serve_connection};
    use flare_ctl::{RemoteHelloParams, StdioWire};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    const KEY: &str = "the-bridge-key-of-this-run";
    const WAIT: Duration = Duration::from_secs(10);

    /// A running Flare's dispatcher whose window answers `remote.tabs` and `remote.attach` at once, served on in-memory
    /// pipe connections (each `connect` is a new one, as the real pipe's accept gives).
    struct Host {
        shared: Arc<Shared>,
        ops: Arc<Mutex<Vec<String>>>,
        next: AtomicU64,
        up: Arc<AtomicBool>,
    }

    impl Host {
        fn new() -> Arc<Host> {
            let ops = Arc::new(Mutex::new(Vec::new()));
            let seen = ops.clone();
            let post: Post = Arc::new(move |m: UiMsg| {
                let UiMsg::Ctl(boxed) = m else {
                    return;
                };
                seen.lock().unwrap().push(format!("{boxed:?}"));
                let req = boxed.0;
                let answer = match &req.op {
                    CtlOp::RemoteTabs => Ok(json!({"tabs": [{"tab": "t1", "cwd": "C:\\w"}]})),
                    CtlOp::RemoteAttach { tab, .. } => {
                        Ok(json!({"tab": tab, "cols": 80, "rows": 24, "seq": 0}))
                    }
                    _ => Err(CtlError::new(code::NO_TAB, "that tab is closed")),
                };
                let _ = req.reply.send(answer);
            });
            let d = AppDispatcher::new(
                post,
                Arc::new(|_: &str| None),
                crate::ctl_app::real_resolve(),
                "C:\\".into(),
                Arc::new(AtomicBool::new(false)),
            )
            .with_remote(RemoteHost::with_key(KEY.into()));
            Arc::new(Host {
                shared: Shared::new(Arc::new(d), ServerConfig::default()),
                ops,
                next: AtomicU64::new(1),
                up: Arc::new(AtomicBool::new(true)),
            })
        }

        /// A new pipe connection to the running Flare, or a refusal while it is "not running".
        fn connect(self: &Arc<Self>) -> Result<Client<MemWire>, ClientError> {
            if !self.up.load(Ordering::SeqCst) {
                return Err(ClientError::Io(std::io::Error::other("no pipe")));
            }
            let (server_end, client) = pair();
            let id = self.next.fetch_add(1, Ordering::SeqCst);
            let handle = ConnHandle::new(id);
            self.shared.hub.add(handle.clone());
            let shared = self.shared.clone();
            std::thread::spawn(move || {
                serve_connection(&server_end, &shared, &handle);
                server_end.close();
            });
            Ok(Client::new(client))
        }
    }

    /// The serve process's link over a loopback pair: the bridge in front of `host`, its key `key`.
    fn link(
        host: &Arc<Host>,
        key: Option<&'static str>,
    ) -> (Client<MemWire>, std::thread::JoinHandle<()>) {
        let h = host.clone();
        let bridge = BridgeDispatcher::new(
            Box::new(move || h.connect()),
            Box::new(move || key.map(str::to_string)),
        );
        let shared = Shared::new_stdio(
            Arc::new(bridge),
            ServerConfig {
                machine: "host-pc".into(),
                ..ServerConfig::stdio()
            },
        );
        let (server_end, viewer) = pair();
        let t = std::thread::spawn(move || {
            flare_ctl::serve_stdio(&server_end, &shared);
            server_end.close();
        });
        (Client::new(viewer), t)
    }

    fn hello(c: &mut Client<MemWire>) {
        let info = c
            .remote_hello(
                &RemoteHelloParams {
                    machine: "viewer".into(),
                    flare_version: "9".into(),
                },
                WAIT,
            )
            .expect("remote.hello");
        assert_eq!(info.machine, "host-pc");
        assert!(info.capabilities.iter().any(|c| c == "remote.output"));
    }

    fn code_of(r: Result<Value, ClientError>) -> String {
        match r {
            Err(ClientError::Remote(e)) => e.code,
            Err(e) => format!("other: {e}"),
            Ok(_) => "ok".into(),
        }
    }

    /// The whole relay: the link's hello is the serve process's own, `remote.*` reaches the running Flare as the
    /// bridge, and anything else is refused before it leaves the serve process.
    #[test]
    fn the_bridge_relays_remote_calls_and_refuses_everything_else() {
        let host = Host::new();
        let (mut c, t) = link(&host, Some(KEY));
        hello(&mut c);
        let tabs = c.call(Method::RemoteTabs, json!({}), WAIT).unwrap();
        assert_eq!(tabs["tabs"][0]["tab"], "t1");
        let at = c
            .call(Method::RemoteAttach, json!({"tab": "t1"}), WAIT)
            .unwrap();
        assert_eq!(at["cols"], 80);
        for m in [
            Method::Hello,
            Method::TabsList,
            Method::PanelState,
            Method::ShellInput,
            Method::TabsInput,
            Method::BrowserOpen,
        ] {
            assert_eq!(
                code_of(c.call(m, json!({}), WAIT)),
                code::FORBIDDEN,
                "{}",
                m.name()
            );
        }
        // The running Flare saw the two relayed calls and nothing else.
        assert_eq!(host.ops.lock().unwrap().len(), 2);
        c.wire().close();
        t.join().unwrap();
    }

    /// With no Flare running (no key, or no pipe), the hello still answers and every relayed call says so in one line.
    #[test]
    fn with_no_flare_running_the_hello_answers_and_the_calls_are_unavailable() {
        let host = Host::new();
        let (mut c, t) = link(&host, None);
        hello(&mut c);
        let r = c.call(Method::RemoteTabs, json!({}), WAIT);
        assert_eq!(code_of(r), code::UNAVAILABLE);
        c.wire().close();
        t.join().unwrap();
        host.up.store(false, Ordering::SeqCst);
        let (mut c, t) = link(&host, Some(KEY));
        hello(&mut c);
        match c.call(Method::RemoteTabs, json!({}), WAIT) {
            Err(ClientError::Remote(e)) => {
                assert_eq!(e.code, code::UNAVAILABLE);
                assert!(e.message.contains("not running"), "{}", e.message);
            }
            other => panic!("{other:?}"),
        }
        c.wire().close();
        t.join().unwrap();
    }

    /// Rule 284: a key of another run is no bridge; the running Flare refuses it, and the link hears "not running".
    #[test]
    fn a_wrong_key_is_no_bridge() {
        let host = Host::new();
        let (mut c, t) = link(&host, Some("a-key-of-an-earlier-run"));
        hello(&mut c);
        assert_eq!(
            code_of(c.call(Method::RemoteTabs, json!({}), WAIT)),
            code::UNAVAILABLE
        );
        assert!(host.ops.lock().unwrap().is_empty());
        c.wire().close();
        t.join().unwrap();
    }

    /// The running Flare restarted between two polls: a call that changes nothing goes again on a fresh connection.
    #[test]
    fn a_broken_pipe_is_opened_again_for_a_call_that_changes_nothing() {
        assert!(may_retry(Method::RemoteOutput) && may_retry(Method::RemoteTabs));
        assert!(!may_retry(Method::RemoteInput));
        assert!(!may_retry(Method::RemoteOpen) && !may_retry(Method::RemoteClose));
    }

    /// Rule 277 over real OS pipes: the serve side's stdout is protocol lines and nothing else, a banner before the
    /// greeting is skipped, and the relayed answer comes back whole.
    #[test]
    fn the_serve_side_answers_over_os_pipes_with_protocol_lines_only() {
        let host = Host::new();
        let h = host.clone();
        let bridge = BridgeDispatcher::new(
            Box::new(move || h.connect()),
            Box::new(|| Some(KEY.to_string())),
        );
        let shared = Shared::new_stdio(Arc::new(bridge), ServerConfig::stdio());
        let (viewer_read, host_write) = std::io::pipe().unwrap();
        let (host_read, mut viewer_write) = std::io::pipe().unwrap();
        let serve = std::thread::spawn(move || {
            let wire = StdioWire::new(host_read, host_write).unwrap();
            flare_ctl::serve_stdio(&wire, &shared);
        });
        use std::io::Write;
        viewer_write.write_all(b"Welcome to the host\r\n").unwrap();
        let wire = StdioWire::new(viewer_read, viewer_write).unwrap();
        let mut c = Client::new(wire);
        hello_on(&mut c);
        let tabs = c.call(Method::RemoteTabs, json!({}), WAIT).unwrap();
        assert_eq!(tabs["tabs"][0]["cwd"], "C:\\w");
        drop(c);
        serve.join().unwrap();
    }

    fn hello_on<W: Wire>(c: &mut Client<W>) {
        c.remote_hello(
            &RemoteHelloParams {
                machine: "viewer".into(),
                flare_version: "9".into(),
            },
            WAIT,
        )
        .expect("remote.hello over OS pipes");
    }
}
