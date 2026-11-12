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
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
///
/// Two ways up to the running Flare (rule 286). The anchor connection carries the calls that follow a tab or change it
/// (`on_anchor`), one at a time and in turn: the running Flare keys a follow by its connection, so this one is never
/// dropped for a slow call. Every other call (the polls, the snapshot, the tab list, a folder read, an open) takes an
/// idle pooled connection or a new one, so a slow folder read never holds up typing or the output polls. A call's
/// time counts from when the link handed it over, its wait for its turn included: a call whose turn comes after its
/// time is answered `timeout` and never sent, so nothing is typed late.
pub struct BridgeDispatcher<W: Wire + Send + 'static> {
    connect: Connect<W>,
    key_of: KeyOf,
    /// How long a relayed call may take, its wait for its turn included (tests shorten it).
    budget: fn(Method) -> Duration,
    lanes: Mutex<Lanes<W>>,
    /// Signalled when the anchor connection is free again.
    turn: Condvar,
}

/// The bridge's connections to the running Flare.
struct Lanes<W: Wire> {
    /// The connection that follows tabs, opened at its first call and again after it broke.
    anchor: Option<Client<W>>,
    /// A call is using the anchor connection.
    anchor_busy: bool,
    /// Pooled connections no call is using (they follow nothing).
    idle: Vec<Client<W>>,
    /// The link closed: a connection coming back from a call is dropped, not kept.
    closed: bool,
}

/// At most this many pooled connections are kept between calls.
const IDLE_KEPT: usize = 4;

fn unavailable(msg: impl Into<String>) -> CtlError {
    CtlError::new(code::UNAVAILABLE, msg)
}

fn late(method: Method) -> CtlError {
    CtlError::new(
        code::TIMEOUT,
        format!("Flare did not answer {} in time", method.name()),
    )
}

/// What a relayed call may take: its link's time less the margin.
fn default_budget(method: Method) -> Duration {
    method.timeout().saturating_sub(RELAY_MARGIN)
}

/// Whether a relayed call rides the anchor connection: the calls that follow a tab or change it, in turn.
fn on_anchor(m: Method) -> bool {
    matches!(
        m,
        Method::RemoteAttach
            | Method::RemoteDetach
            | Method::RemoteInput
            | Method::RemoteResize
            | Method::RemoteClose
    )
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
            | Method::RemoteStatus
            | Method::RemoteConvo
    )
}

/// The anchor connection's turn, held by one call; given back (and the next call woken) when it ends, a panic too.
struct AnchorTurn<'a, W: Wire + Send + 'static> {
    d: &'a BridgeDispatcher<W>,
    conn: Option<Client<W>>,
}

impl<W: Wire + Send + 'static> Drop for AnchorTurn<'_, W> {
    fn drop(&mut self) {
        let mut st = self.d.lk();
        st.anchor_busy = false;
        if !st.closed {
            st.anchor = self.conn.take();
        }
        drop(st);
        self.d.turn.notify_one();
    }
}

impl<W: Wire + Send + 'static> BridgeDispatcher<W> {
    pub fn new(connect: Connect<W>, key_of: KeyOf) -> Self {
        Self {
            connect,
            key_of,
            budget: default_budget,
            lanes: Mutex::new(Lanes {
                anchor: None,
                anchor_busy: false,
                idle: Vec::new(),
                closed: false,
            }),
            turn: Condvar::new(),
        }
    }

    #[cfg(test)]
    fn with_budget(mut self, budget: fn(Method) -> Duration) -> Self {
        self.budget = budget;
        self
    }

    fn lk(&self) -> MutexGuard<'_, Lanes<W>> {
        self.lanes.lock().unwrap_or_else(|e| e.into_inner())
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

    /// Relays one call by `deadline`, on the anchor connection or a pooled one.
    fn relay(&self, method: Method, params: Value) -> Result<Value, CtlError> {
        let deadline = Instant::now() + (self.budget)(method);
        if on_anchor(method) {
            self.relay_on_anchor(method, params, deadline)
        } else {
            let conn = self.lk().idle.pop();
            let (r, back) = self.send(conn, method, params, deadline);
            if let Some(c) = back {
                let mut st = self.lk();
                if !st.closed && st.idle.len() < IDLE_KEPT {
                    st.idle.push(c);
                }
            }
            r
        }
    }

    /// Waits for the anchor connection's turn until `deadline`; a call whose turn comes too late is never sent.
    fn relay_on_anchor(
        &self,
        method: Method,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, CtlError> {
        let st = self.lk();
        let left = deadline.saturating_duration_since(Instant::now());
        let (mut st, _) = self
            .turn
            .wait_timeout_while(st, left, |s| s.anchor_busy) // sync-ok: a link worker waits for its turn, capped by its own time
            .unwrap_or_else(|e| e.into_inner());
        if st.anchor_busy {
            return Err(late(method));
        }
        st.anchor_busy = true;
        let conn = st.anchor.take();
        drop(st);
        let mut turn = AnchorTurn {
            d: self,
            conn: None,
        };
        let (r, back) = self.send(conn, method, params, deadline);
        turn.conn = back;
        drop(turn);
        r
    }

    /// Sends one call on `conn` (or a new connection), within `deadline`. Returns the answer and the connection to keep:
    /// None when it broke or the running Flare closed it. A call that timed out keeps its connection: a late answer is
    /// skipped by its id, and dropping the anchor would stop every tab it follows.
    fn send(
        &self,
        conn: Option<Client<W>>,
        method: Method,
        params: Value,
        deadline: Instant,
    ) -> (Result<Value, CtlError>, Option<Client<W>>) {
        let mut conn = conn;
        for attempt in 0..2 {
            let mut c = match conn.take() {
                Some(c) => c,
                None => match self.open() {
                    Ok(c) => c,
                    Err(e) => return (Err(e), None),
                },
            };
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return (Err(late(method)), Some(c));
            }
            match c.call(method, params.clone(), left) {
                Ok(v) => return (Ok(v), Some(c)),
                // The running Flare's own answer, error or not, is the link's.
                Err(ClientError::Remote(e)) => {
                    let keep = (!code::closes_client(&e.code)).then_some(c);
                    return (Err(e), keep);
                }
                Err(ClientError::Timeout) => return (Err(late(method)), Some(c)),
                Err(e) => {
                    if attempt == 0 && may_retry(method) {
                        log::info!("serve-remote: the pipe broke ({e}); connecting again");
                        continue;
                    }
                    return (
                        Err(unavailable(format!("the running Flare went away: {e}"))),
                        None,
                    );
                }
            }
        }
        (Err(unavailable("the running Flare went away")), None)
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
        let mut st = self.lk();
        st.closed = true;
        st.anchor = None;
        st.idle.clear();
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
        /// While above 0, the window answers `remote.open` and `remote.attach` this many ms late (on its own thread).
        slow_ms: Arc<AtomicU64>,
    }

    impl Host {
        fn new() -> Arc<Host> {
            let ops = Arc::new(Mutex::new(Vec::new()));
            let seen = ops.clone();
            let slow_ms = Arc::new(AtomicU64::new(0));
            let slow = slow_ms.clone();
            let post: Post = Arc::new(move |m: UiMsg| {
                let UiMsg::Ctl(boxed) = m else {
                    return;
                };
                seen.lock().unwrap().push(format!("{boxed:?}"));
                let req = boxed.0;
                let answer = match &req.op {
                    CtlOp::RemoteOpen { .. } => Ok(json!({"tab": "t2"})),
                    CtlOp::RemoteTabs => Ok(json!({"tabs": [{"tab": "t1", "cwd": "C:\\w"}]})),
                    CtlOp::RemoteAttach { tab, .. } => {
                        Ok(json!({"tab": tab, "cols": 80, "rows": 24, "seq": 0}))
                    }
                    _ => Err(CtlError::new(code::NO_TAB, "that tab is closed")),
                };
                let late = slow.load(Ordering::SeqCst);
                if late > 0
                    && matches!(
                        req.op,
                        CtlOp::RemoteOpen { .. } | CtlOp::RemoteAttach { .. }
                    )
                {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(late));
                        let _ = req.reply.send(answer);
                    });
                    return;
                }
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
                slow_ms,
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

    fn remote_ctx() -> CallContext {
        CallContext {
            conn: 1,
            client: "viewer".into(),
            client_version: "9".into(),
            grant: Grant::Remote,
            tab: None,
            cancel: flare_ctl::session::CancelFlag::default(),
        }
    }

    fn bridge_to(host: &Arc<Host>) -> BridgeDispatcher<MemWire> {
        let h = host.clone();
        BridgeDispatcher::new(
            Box::new(move || h.connect()),
            Box::new(|| Some(KEY.to_string())),
        )
    }

    fn seen(host: &Host, what: &str) -> bool {
        host.ops.lock().unwrap().iter().any(|o| o.contains(what))
    }

    fn wait_for(what: &str, ok: impl Fn() -> bool) {
        let end = Instant::now() + WAIT;
        while !ok() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Rule 286: a slow `remote.open` (or folder read) holds no lock the other calls need: the polls and typing are
    /// answered while it runs.
    #[test]
    fn a_slow_open_never_holds_up_typing_or_the_polls() {
        let host = Host::new();
        host.slow_ms.store(3000, Ordering::SeqCst);
        let bridge = Arc::new(bridge_to(&host));
        let b = bridge.clone();
        let here = std::env::temp_dir().to_string_lossy().into_owned();
        let open = std::thread::spawn(move || {
            b.call(&remote_ctx(), Method::RemoteOpen, json!({"folder": here}))
        });
        wait_for("the open to reach the window", || seen(&host, "RemoteOpen"));
        let t0 = Instant::now();
        let tabs = bridge
            .call(&remote_ctx(), Method::RemoteTabs, json!({}))
            .expect("remote.tabs");
        assert_eq!(tabs["tabs"][0]["tab"], "t1");
        let typed = bridge.call(
            &remote_ctx(),
            Method::RemoteInput,
            json!({"tab": "t1", "intents": [{"text": "x"}]}),
        );
        assert_eq!(code_of(typed.map_err(ClientError::Remote)), code::NO_TAB);
        assert!(
            t0.elapsed() < Duration::from_millis(2000),
            "the calls waited {:?} behind the open",
            t0.elapsed()
        );
        assert_eq!(open.join().unwrap().expect("remote.open")["tab"], "t2");
    }

    /// Rule 286: a call whose turn on the anchor connection comes after its own time is answered `timeout` and never
    /// sent, and a call that timed out keeps the anchor connection (the running Flare keys its follows by it).
    #[test]
    fn a_call_late_for_its_turn_is_never_sent_and_a_timeout_keeps_the_anchor() {
        let host = Host::new();
        host.slow_ms.store(2500, Ordering::SeqCst);
        let bridge = Arc::new(bridge_to(&host).with_budget(|m| match m {
            Method::RemoteInput => Duration::from_millis(200),
            _ => Duration::from_millis(1000),
        }));
        let b = bridge.clone();
        let attach = std::thread::spawn(move || {
            b.call(&remote_ctx(), Method::RemoteAttach, json!({"tab": "t1"}))
        });
        wait_for("the attach to reach the window", || {
            seen(&host, "RemoteAttach")
        });
        let opened = host.next.load(Ordering::SeqCst);
        let typed = bridge.call(
            &remote_ctx(),
            Method::RemoteInput,
            json!({"tab": "t1", "intents": [{"text": "x"}]}),
        );
        assert_eq!(code_of(typed.map_err(ClientError::Remote)), code::TIMEOUT);
        assert_eq!(
            code_of(attach.join().unwrap().map_err(ClientError::Remote)),
            code::TIMEOUT
        );
        // The attach's late answer comes and goes; the typing never reached the window.
        std::thread::sleep(Duration::from_millis(2000));
        assert!(!seen(&host, "RemoteInput"), "a late call was typed");
        // The next anchor call rides the same connection: none was dropped or opened again.
        host.slow_ms.store(0, Ordering::SeqCst);
        bridge
            .call(&remote_ctx(), Method::RemoteDetach, json!({"tab": "t1"}))
            .expect("remote.detach");
        assert_eq!(host.next.load(Ordering::SeqCst), opened);
    }
}
