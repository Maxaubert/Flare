//! One client connection, from its first byte to its close: the transport-neutral half of the server.
//! The pipe code (`pipe.rs`) hands it a [`Wire`]; tests hand it an in-memory one.
//!
//! The order of checks on every request is part of the contract (`docs/surfaces/protocol.md`):
//! parse, rate limit, method name, hello first, token still valid, grant, then the dispatcher with a
//! timeout. Only the thread that runs [`serve_connection`] ever touches the wire; events published
//! by the app wait in the connection's outbox and are written from that thread.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::framing::{LineReader, ReadError, Wire, write_line};
use crate::grant::Grant;
use crate::method::{DEFAULT_TIMEOUT, Method, Namespace};
use crate::proto::{
    CtlError, Event, MAX_LINE, PROTOCOL, Request, Response, code, parse_request, to_line,
};

/// What the app implements. Every method may be called from any worker thread.
pub trait Dispatcher: Send + Sync + 'static {
    /// The grant of a mod token, or None when the token is unknown or its mod is disabled (S5). Asked
    /// again on every request, so disabling a mod kills its open connections at once.
    fn resolve_token(&self, token: &str) -> Option<Grant>;

    /// The grant of a caller with no token. Asked on every request, so the user's `eval` switch is live.
    fn agent_grant(&self) -> Grant {
        Grant::agent()
    }

    /// The tab that owns `tab_token` (the secret the app put in that tab's shell as
    /// `FLARE_TAB_TOKEN`), or None when no live tab has it. This is the only way a connection gets a
    /// tab: `hello` carries the token, never a tab name, so a process that was not started inside the
    /// tab cannot claim it. Asked again on every request, so a closed tab's token stops working at once.
    fn resolve_tab_token(&self, tab_token: &str) -> Option<String>;

    /// The methods this build implements; `hello` lists them (narrowed to the caller's grant).
    fn capabilities(&self) -> Vec<Method> {
        Method::ALL.to_vec()
    }

    /// Runs one request. The grant was checked; `params` is an object. Anything but `hello`.
    ///
    /// Runs on its own thread and may still be running, or start, after `Server::stop`: it must
    /// return quickly once the app is shutting down, and must not touch the UI thread in a way that
    /// can panic or block then (answer `unavailable` instead).
    fn call(&self, ctx: &CallContext, method: Method, params: Value) -> Result<Value, CtlError>;

    /// A connection ended (clean close, error or server stop).
    fn client_gone(&self, _ctx: &CallContext) {}
}

/// Who is calling.
#[derive(Clone, Debug, PartialEq)]
pub struct CallContext {
    /// A number unique among connections of this server run.
    pub conn: u64,
    pub client: String,
    pub client_version: String,
    pub grant: Grant,
    /// The tab whose token `hello` presented, while that tab exists.
    pub tab: Option<String>,
}

/// Server limits. The defaults are the protocol's.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub server_name: String,
    pub server_version: String,
    pub max_clients: usize,
    pub max_line: usize,
    /// A connection must say `hello` within this long.
    pub hello_timeout: Duration,
    /// Sustained requests per second per client.
    pub rate_per_sec: f64,
    /// Requests a client may send at once before the rate applies.
    pub rate_burst: f64,
    /// Replaces every method's timeout (tests only).
    pub timeout_override: Option<Duration>,
    /// Calls that timed out but are still running; past this the server refuses new ones.
    pub max_stuck_calls: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server_name: "flare".into(),
            server_version: env!("CARGO_PKG_VERSION").into(),
            max_clients: 64,
            max_line: MAX_LINE,
            hello_timeout: Duration::from_secs(10),
            rate_per_sec: 100.0,
            rate_burst: 200.0,
            timeout_override: None,
            max_stuck_calls: 32,
        }
    }
}

/// Token bucket. `now` is passed in so tests do not sleep.
pub struct Bucket {
    tokens: f64,
    last: Instant,
    rate: f64,
    burst: f64,
}

impl Bucket {
    pub fn new(rate: f64, burst: f64, now: Instant) -> Self {
        Self {
            tokens: burst,
            last: now,
            rate,
            burst,
        }
    }

    pub fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The part of a connection the app side can reach: its tab and the events waiting for it.
pub struct ConnHandle {
    pub id: u64,
    tab: Mutex<Option<String>>,
    said_hello: AtomicBool,
    outbox: Mutex<VecDeque<Vec<u8>>>,
    /// Unblocks a worker stuck in a write to a client that never reads (set by the pipe code).
    abort: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// Events queued per connection before the oldest is dropped (a client that never reads).
const OUTBOX_CAP: usize = 256;

impl ConnHandle {
    pub fn new(id: u64) -> Arc<Self> {
        Arc::new(Self {
            id,
            tab: Mutex::new(None),
            said_hello: AtomicBool::new(false),
            outbox: Mutex::new(VecDeque::new()),
            abort: Mutex::new(None),
        })
    }

    /// Sets (or, with None, clears) the hook that cancels this connection's blocked I/O. Clearing
    /// waits for a running hook, so the transport may close its handle right after.
    pub fn set_abort(&self, f: Option<Box<dyn Fn() + Send + Sync>>) {
        *self.abort.lock().unwrap_or_else(|e| e.into_inner()) = f;
    }

    fn abort(&self) {
        if let Some(f) = self
            .abort
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            f();
        }
    }

    fn push(&self, line: Vec<u8>) {
        let mut q = self.outbox.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= OUTBOX_CAP {
            q.pop_front();
        }
        q.push_back(line);
    }

    fn drain(&self) -> Vec<Vec<u8>> {
        let mut q = self.outbox.lock().unwrap_or_else(|e| e.into_inner());
        q.drain(..).collect()
    }

    /// Number of events waiting (tests).
    pub fn queued(&self) -> usize {
        self.outbox.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// Every live connection, for event fan-out.
#[derive(Default)]
pub struct Hub {
    conns: Mutex<Vec<Arc<ConnHandle>>>,
}

impl Hub {
    pub fn add(&self, c: Arc<ConnHandle>) {
        self.conns.lock().unwrap_or_else(|e| e.into_inner()).push(c);
    }

    pub fn remove(&self, id: u64) {
        self.conns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|c| c.id != id);
    }

    /// Cancels the blocked I/O of every connection (server stop).
    pub fn abort_all(&self) {
        let all: Vec<Arc<ConnHandle>> =
            self.conns.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for c in all {
            c.abort();
        }
    }

    pub fn len(&self) -> usize {
        self.conns.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Queues an event for every connection that said hello and, when `tab` is given, belongs to that
    /// tab. Never blocks on a client. Returns how many connections got it.
    pub fn publish(&self, event: &str, params: Value, tab: Option<&str>) -> usize {
        let line = to_line(&Event {
            event: event.into(),
            params,
        });
        let mut n = 0;
        for c in self.conns.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if !c.said_hello.load(Ordering::SeqCst) {
                continue;
            }
            if let Some(t) = tab
                && c.tab.lock().unwrap_or_else(|e| e.into_inner()).as_deref() != Some(t)
            {
                continue;
            }
            c.push(line.clone());
            n += 1;
        }
        n
    }
}

/// What every connection shares.
pub struct Shared {
    pub dispatcher: Arc<dyn Dispatcher>,
    pub config: ServerConfig,
    pub hub: Hub,
    /// Calls that passed their timeout and are still running (capped by `max_stuck_calls`).
    pub stuck: Arc<AtomicUsize>,
    /// Every call thread that has not finished, timed out or not (reported by `Server::stop`).
    pub in_flight: Arc<AtomicUsize>,
    pub stop: AtomicBool,
}

impl Shared {
    pub fn new(dispatcher: Arc<dyn Dispatcher>, config: ServerConfig) -> Arc<Self> {
        Arc::new(Self {
            dispatcher,
            config,
            hub: Hub::default(),
            stuck: Arc::new(AtomicUsize::new(0)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            stop: AtomicBool::new(false),
        })
    }
}

/// Where a call is: running in time, past its timeout and counted as stuck, or done.
const RUNNING: u8 = 0;
const STUCK: u8 = 1;
const DONE: u8 = 2;

/// Ends a call thread's bookkeeping, however it ends.
struct CallGuard {
    state: Arc<AtomicU8>,
    stuck: Arc<AtomicUsize>,
    in_flight: Arc<AtomicUsize>,
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if self.state.swap(DONE, Ordering::SeqCst) == STUCK {
            self.stuck.fetch_sub(1, Ordering::SeqCst);
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How often a waiting connection looks at the stop flag.
const STOP_SLICE: Duration = Duration::from_millis(50);

/// Runs the dispatcher on its own thread so a hung call cannot hold the connection past its timeout.
/// The thread is left to finish by itself; only calls past their timeout count as `stuck`, and that
/// count caps the damage. Slow calls still inside their timeout (a 60 s `browser.wait`) never do.
fn call_with_timeout(
    shared: &Shared,
    ctx: &CallContext,
    method: Method,
    params: Value,
) -> Result<Value, CtlError> {
    if shared.stuck.load(Ordering::SeqCst) >= shared.config.max_stuck_calls {
        return Err(CtlError::new(
            code::UNAVAILABLE,
            "too many earlier calls are still running",
        ));
    }
    let timeout = shared
        .config
        .timeout_override
        .unwrap_or_else(|| method.timeout());
    let (tx, rx) = mpsc::channel();
    let state = Arc::new(AtomicU8::new(RUNNING));
    shared.in_flight.fetch_add(1, Ordering::SeqCst);
    let guard = CallGuard {
        state: state.clone(),
        stuck: shared.stuck.clone(),
        in_flight: shared.in_flight.clone(),
    };
    let d = shared.dispatcher.clone();
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("flare-ctl-call".into())
        .spawn(move || {
            let _guard = guard;
            let r = d.call(&ctx, method, params);
            let _ = tx.send(r);
        });
    if spawned.is_err() {
        // The closure (and its guard) was dropped with the failed spawn.
        return Err(CtlError::new(code::INTERNAL, "could not start the call"));
    }
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.min(STOP_SLICE)) {
            Ok(r) => return r,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CtlError::new(
                    code::INTERNAL,
                    "the call failed unexpectedly",
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let stopping = shared.stop.load(Ordering::SeqCst);
                if !stopping && !left.is_zero() && Instant::now() < deadline {
                    continue;
                }
                // Count it as stuck unless it finished in the meantime.
                // (Counted first, so the finishing thread's decrement can never come before it.)
                shared.stuck.fetch_add(1, Ordering::SeqCst);
                if state
                    .compare_exchange(RUNNING, STUCK, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    shared.stuck.fetch_sub(1, Ordering::SeqCst);
                }
                if stopping {
                    return Err(CtlError::new(code::UNAVAILABLE, "Flare is shutting down"));
                }
                return Err(CtlError::new(
                    code::TIMEOUT,
                    format!(
                        "{} did not answer within {} ms",
                        method.name(),
                        timeout.as_millis()
                    ),
                ));
            }
        }
    }
}

fn reply(wire: &impl Wire, resp: &Response) -> io::Result<()> {
    write_line(wire, &to_line(resp), MAX_LINE)
}

/// What the `hello` answer says, given the grant.
fn hello_result(shared: &Shared, grant: &Grant, tab: Option<&str>) -> Value {
    let caps = shared.dispatcher.capabilities();
    let names: Vec<&str> = Method::ALL
        .into_iter()
        .filter(|m| caps.contains(m) && grant.may_call(*m))
        .map(Method::name)
        .collect();
    let mut grant_json = json!({"kind": grant.kind(), "permissions": grant.permission_names()});
    if let Grant::Mod { id, .. } = grant {
        grant_json["id"] = json!(id);
    }
    json!({
        "protocol": PROTOCOL,
        "server": {"name": shared.config.server_name, "version": shared.config.server_version},
        "grant": grant_json,
        "capabilities": names,
        "tab": tab,
        "limits": {
            "max_line": shared.config.max_line,
            "max_clients": shared.config.max_clients,
            "timeout_ms": DEFAULT_TIMEOUT.as_millis() as u64,
            "browser_timeout_ms": Method::BrowserOpen.timeout().as_millis() as u64,
            "rate_per_sec": shared.config.rate_per_sec,
        },
    })
}

struct Hello {
    ctx: CallContext,
    /// The mod token, kept to re-check on every request.
    token: Option<String>,
    /// The tab token, kept to re-check on every request.
    tab_token: Option<String>,
}

/// A grant from the app, checked to be one that may exist over the pipe: never `ui` (S3). A bug on
/// the app side (a manifest permission mapped to `ui`, a cached `ui` grant) must not open the `ui`
/// namespace to a socket.
fn pipe_grant(g: Grant, from_token: bool) -> Result<Grant, CtlError> {
    if matches!(g, Grant::Ui) {
        return Err(if from_token {
            CtlError::new(code::UNAUTHORIZED, "a token cannot hold the ui grant")
        } else {
            CtlError::new(code::INTERNAL, "the app gave a pipe caller the ui grant")
        });
    }
    Ok(g)
}

fn handle_hello(shared: &Shared, conn: u64, req: &Request) -> Result<Hello, CtlError> {
    let str_param = |k: &str| req.params.get(k).and_then(Value::as_str);
    let Some(client) = str_param("client") else {
        return Err(CtlError::new(code::BAD_PARAMS, "hello needs a client name"));
    };
    let Some(version) = str_param("version") else {
        return Err(CtlError::new(
            code::BAD_PARAMS,
            "hello needs a client version",
        ));
    };
    if let Some(p) = req.params.get("protocol")
        && p.as_u64() != Some(u64::from(PROTOCOL))
    {
        return Err(CtlError::new(
            code::UNSUPPORTED_PROTOCOL,
            format!("this server speaks protocol {PROTOCOL}"),
        ));
    }
    let token = match req.params.get("token") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) => Some(t.clone()),
        Some(_) => return Err(CtlError::new(code::BAD_PARAMS, "token must be a string")),
    };
    let grant = match &token {
        Some(t) => pipe_grant(
            shared
                .dispatcher
                .resolve_token(t)
                .ok_or_else(|| CtlError::new(code::UNAUTHORIZED, "unknown or disabled token"))?,
            true,
        )?,
        None => pipe_grant(shared.dispatcher.agent_grant(), false)?,
    };
    // A tab is named by its secret, never by its name: a free-form `tab` would let any process of the
    // user act as any tab.
    if req.params.get("tab").is_some_and(|t| !t.is_null()) {
        return Err(CtlError::new(
            code::BAD_PARAMS,
            "hello does not take a tab; send tab_token",
        ));
    }
    let tab_token = match req.params.get("tab_token") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) => Some(t.clone()),
        Some(_) => {
            return Err(CtlError::new(
                code::BAD_PARAMS,
                "tab_token must be a string",
            ));
        }
    };
    let tab = match &tab_token {
        Some(t) => Some(
            shared
                .dispatcher
                .resolve_tab_token(t)
                .ok_or_else(|| CtlError::new(code::UNAUTHORIZED, "unknown tab token"))?,
        ),
        None => None,
    };
    Ok(Hello {
        ctx: CallContext {
            conn,
            client: client.into(),
            client_version: version.into(),
            grant,
            tab,
        },
        token,
        tab_token,
    })
}

/// Serves one connection until it closes, errs, is told to stop or breaks the protocol badly.
pub fn serve_connection<W: Wire>(wire: &W, shared: &Shared, handle: &Arc<ConnHandle>) {
    let cap = shared.config.max_line;
    let mut reader = LineReader::new(cap);
    let mut hello: Option<Hello> = None;
    let started = Instant::now();
    let mut bucket = Bucket::new(
        shared.config.rate_per_sec,
        shared.config.rate_burst,
        started,
    );
    'conn: loop {
        let deadline = hello
            .is_none()
            .then(|| started + shared.config.hello_timeout);
        let line = {
            let mut flush = || -> io::Result<()> {
                for ev in handle.drain() {
                    write_line(wire, &ev, cap)?;
                }
                Ok(())
            };
            match reader.read_line_with(wire, deadline, &shared.stop, &mut flush) {
                Ok(l) => l,
                Err(ReadError::TooLong) => {
                    let e = CtlError::new(code::TOO_LARGE, "a message passed the size limit");
                    let _ = reply(wire, &Response::err(Value::Null, e));
                    break 'conn;
                }
                Err(_) => break 'conn,
            }
        };
        let req = match parse_request(&line) {
            Ok(r) => r,
            Err(f) => {
                let _ = reply(wire, &Response::err(f.id, f.error));
                break 'conn;
            }
        };
        let (resp, close) = answer(shared, handle, &mut hello, &mut bucket, &req);
        if reply(wire, &resp).is_err() || close {
            break 'conn;
        }
        // Events queued while the request ran go out right after its answer.
        for ev in handle.drain() {
            if write_line(wire, &ev, cap).is_err() {
                break 'conn;
            }
        }
    }
    if let Some(h) = hello {
        shared.dispatcher.client_gone(&h.ctx);
    }
}

/// Decides the answer to one parsed request. The bool is "close the client after it".
fn answer(
    shared: &Shared,
    handle: &Arc<ConnHandle>,
    hello: &mut Option<Hello>,
    bucket: &mut Bucket,
    req: &Request,
) -> (Response, bool) {
    let id = req.id.clone();
    let fail = |e: CtlError| {
        let close = code::closes_client(&e.code);
        (Response::err(id.clone(), e), close)
    };
    // Every request costs a token, forbidden and unknown ones and hello retries included, before the
    // app is asked anything: a spammer cannot drive unlimited callbacks into it.
    if !bucket.take(Instant::now()) {
        return fail(CtlError::new(
            code::RATE_LIMITED,
            "too many requests; slow down",
        ));
    }
    let Some(method) = Method::parse(&req.method) else {
        return fail(CtlError::new(
            code::UNKNOWN_METHOD,
            format!("no method named {}", req.method),
        ));
    };
    if method == Method::Hello {
        if hello.is_some() {
            return fail(CtlError::new(code::ALREADY_HELLO, "hello was already sent"));
        }
        return match handle_hello(shared, handle.id, req) {
            Ok(h) => {
                let result = hello_result(shared, &h.ctx.grant, h.ctx.tab.as_deref());
                *handle.tab.lock().unwrap_or_else(|e| e.into_inner()) = h.ctx.tab.clone();
                // A client that reads only while it waits for an answer says `events: false`: it is sent no
                // events, so a full pipe can never block this worker (and so deadlock a big request).
                let wants_events = req.params.get("events").and_then(Value::as_bool) != Some(false);
                handle.said_hello.store(wants_events, Ordering::SeqCst);
                *hello = Some(h);
                (Response::ok(id, result), false)
            }
            Err(e) => fail(e),
        };
    }
    let Some(h) = hello.as_mut() else {
        return fail(CtlError::new(code::HELLO_REQUIRED, "send hello first"));
    };
    // The grant is read again on every request: a disabled mod loses its connection, and the user's
    // eval switch takes effect without a reconnect.
    let regranted = match &h.token {
        Some(t) => match shared.dispatcher.resolve_token(t) {
            Some(g) => pipe_grant(g, true),
            None => Err(CtlError::new(
                code::UNAUTHORIZED,
                "the token is no longer valid",
            )),
        },
        None => pipe_grant(shared.dispatcher.agent_grant(), false),
    };
    match regranted {
        Ok(g) => h.ctx.grant = g,
        Err(e) => return fail(e),
    }
    // The tab is re-read too: a tab that was closed since hello is no tab any more.
    if let Some(t) = &h.tab_token {
        let now = shared.dispatcher.resolve_tab_token(t);
        if now != h.ctx.tab {
            *handle.tab.lock().unwrap_or_else(|e| e.into_inner()) = now.clone();
            h.ctx.tab = now;
        }
    }
    // The ui namespace is closed to the pipe whatever the grant says (belt and braces for S3).
    if method.namespace() == Namespace::Ui {
        return fail(
            h.ctx.grant.check(method).err().unwrap_or_else(|| {
                CtlError::new(code::FORBIDDEN, "ui methods are not on the pipe")
            }),
        );
    }
    if let Err(e) = h.ctx.grant.check(method) {
        return fail(e);
    }
    match call_with_timeout(shared, &h.ctx, method, req.params.clone()) {
        Ok(v) => (Response::ok(id, v), false),
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::testwire::{MemWire, pair};
    use std::sync::atomic::AtomicU32;

    /// (method, params, tab, grant kind)
    type Seen = (Method, Value, Option<String>, String);

    /// A dispatcher with a script: tabs `t1` and `t2`, token `good` is mod `m` with `view`.
    #[derive(Default)]
    pub struct Fake {
        pub calls: Mutex<Vec<Seen>>,
        pub gone: AtomicU32,
        pub revoked: AtomicBool,
        pub eval: AtomicBool,
        pub hang: AtomicBool,
        /// Token `ui` resolves to the ui grant; with `ui_agent` the agent grant is the ui grant.
        pub ui_agent: AtomicBool,
        pub promote: AtomicBool,
        /// Tab `t1` was closed: its token no longer resolves.
        pub t1_closed: AtomicBool,
    }

    impl Dispatcher for Fake {
        fn resolve_token(&self, token: &str) -> Option<Grant> {
            if token == "ui" || (token == "good" && self.promote.load(Ordering::SeqCst)) {
                return Some(Grant::Ui);
            }
            (token == "good" && !self.revoked.load(Ordering::SeqCst)).then(|| Grant::Mod {
                id: "m".into(),
                permissions: vec![crate::grant::Permission::View],
            })
        }
        fn agent_grant(&self) -> Grant {
            if self.ui_agent.load(Ordering::SeqCst) {
                return Grant::Ui;
            }
            Grant::Agent {
                eval: self.eval.load(Ordering::SeqCst),
            }
        }
        fn resolve_tab_token(&self, tab_token: &str) -> Option<String> {
            match tab_token {
                "tok1" if !self.t1_closed.load(Ordering::SeqCst) => Some("t1".into()),
                "tok2" => Some("t2".into()),
                _ => None,
            }
        }
        fn call(&self, ctx: &CallContext, m: Method, p: Value) -> Result<Value, CtlError> {
            if self.hang.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(400));
            }
            if p.get("slow").is_some() {
                std::thread::sleep(Duration::from_millis(150));
            }
            self.calls.lock().unwrap().push((
                m,
                p.clone(),
                ctx.tab.clone(),
                ctx.grant.kind().into(),
            ));
            match p.get("fail").and_then(Value::as_str) {
                Some(c) => Err(CtlError::new(c, "scripted")),
                None => Ok(json!({"ok": m.name()})),
            }
        }
        fn client_gone(&self, _: &CallContext) {
            self.gone.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub struct Rig {
        pub client: MemWire,
        pub fake: Arc<Fake>,
        pub shared: Arc<Shared>,
        pub thread: Option<std::thread::JoinHandle<()>>,
        reader: LineReader,
    }

    impl Rig {
        pub fn new(cfg: ServerConfig) -> Rig {
            let fake = Arc::new(Fake::default());
            let shared = Shared::new(fake.clone(), cfg);
            Rig::on(fake, shared, 1)
        }

        /// A connection on a server that other rigs share.
        pub fn on(fake: Arc<Fake>, shared: Arc<Shared>, id: u64) -> Rig {
            let (server_end, client) = pair();
            let handle = ConnHandle::new(id);
            shared.hub.add(handle.clone());
            let (s2, h2) = (shared.clone(), handle.clone());
            let thread = std::thread::spawn(move || {
                serve_connection(&server_end, &s2, &h2);
                server_end.close();
            });
            Rig {
                client,
                fake,
                shared,
                thread: Some(thread),
                reader: LineReader::new(MAX_LINE),
            }
        }

        pub fn send(&self, line: &str) {
            self.client
                .write_all(format!("{line}\n").as_bytes())
                .unwrap();
        }

        pub fn recv(&mut self) -> Value {
            let line = self
                .reader
                .read_line(
                    &self.client,
                    Some(Instant::now() + Duration::from_secs(5)),
                    &AtomicBool::new(false),
                )
                .expect("a line from the server");
            serde_json::from_slice(&line).unwrap()
        }

        pub fn ask(&mut self, line: &str) -> Value {
            self.send(line);
            self.recv()
        }

        pub fn hello(&mut self, extra: &str) -> Value {
            self.ask(&format!(
                r#"{{"id":0,"method":"hello","params":{{"client":"t","version":"1"{extra}}}}}"#
            ))
        }

        pub fn closed(&mut self) -> bool {
            self.reader
                .read_line(
                    &self.client,
                    Some(Instant::now() + Duration::from_secs(5)),
                    &AtomicBool::new(false),
                )
                .is_err()
        }

        pub fn finish(mut self) {
            self.client.close();
            self.thread.take().unwrap().join().unwrap();
        }
    }

    fn rig() -> Rig {
        Rig::new(ServerConfig::default())
    }

    fn code_of(v: &Value) -> &str {
        v["error"]["code"].as_str().unwrap_or("")
    }

    #[test]
    fn hello_reports_protocol_grant_and_capabilities() {
        let mut r = rig();
        let v = r.hello(r#","tab_token":"tok1""#);
        assert_eq!(v["result"]["protocol"], 1);
        assert_eq!(v["result"]["grant"]["kind"], "agent");
        assert_eq!(v["result"]["tab"], "t1");
        let caps: Vec<&str> = v["result"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert!(caps.contains(&"view.image"));
        assert!(!caps.contains(&"browser.eval"));
        assert!(!caps.contains(&"shell.input"));
        assert_eq!(v["result"]["limits"]["max_line"], MAX_LINE);
        r.finish();
    }

    #[test]
    fn a_request_before_hello_is_refused_and_the_client_stays() {
        let mut r = rig();
        let v = r.ask(r#"{"id":1,"method":"panel.close"}"#);
        assert_eq!(code_of(&v), code::HELLO_REQUIRED);
        assert_eq!(r.hello("")["result"]["protocol"], 1);
        r.finish();
    }

    #[test]
    fn hello_twice_is_refused() {
        let mut r = rig();
        r.hello("");
        let v = r.hello("");
        assert_eq!(code_of(&v), code::ALREADY_HELLO);
        r.finish();
    }

    #[test]
    fn hello_needs_client_and_version() {
        let mut r = rig();
        let v = r.ask(r#"{"id":1,"method":"hello","params":{"client":"x"}}"#);
        assert_eq!(code_of(&v), code::BAD_PARAMS);
        let v = r.ask(r#"{"id":2,"method":"hello","params":{"version":"1"}}"#);
        assert_eq!(code_of(&v), code::BAD_PARAMS);
        r.finish();
    }

    #[test]
    fn a_wrong_protocol_closes_the_client() {
        let mut r = rig();
        let v = r.hello(r#","protocol":2"#);
        assert_eq!(code_of(&v), code::UNSUPPORTED_PROTOCOL);
        assert!(r.closed());
        r.finish();
    }

    #[test]
    fn an_unknown_token_closes_the_client() {
        let mut r = rig();
        let v = r.hello(r#","token":"nope""#);
        assert_eq!(code_of(&v), code::UNAUTHORIZED);
        assert!(r.closed());
        r.finish();
    }

    #[test]
    fn an_unknown_tab_token_is_refused_and_closes_the_client() {
        let mut r = rig();
        let v = r.hello(r#","tab_token":"zzz""#);
        assert_eq!(code_of(&v), code::UNAUTHORIZED);
        assert!(r.closed());
        r.finish();
    }

    #[test]
    fn a_tab_name_is_refused_because_only_the_secret_names_a_tab() {
        // The ids `tabs.list` hands out are not tokens: naming a tab is not enough to act as it.
        let mut r = rig();
        for name in ["t1", "tok1", "t2"] {
            let v = r.hello(&format!(r#","tab":"{name}""#));
            assert_eq!(code_of(&v), code::BAD_PARAMS, "{name}");
        }
        let v = r.hello("");
        assert!(v["result"]["tab"].is_null());
        r.finish();
    }

    #[test]
    fn the_tab_token_must_be_a_string() {
        let mut r = rig();
        let v = r.hello(r#","tab_token":7"#);
        assert_eq!(code_of(&v), code::BAD_PARAMS);
        let v = r.hello(r#","tab_token":null"#);
        assert!(v["result"]["tab"].is_null());
        r.finish();
    }

    #[test]
    fn a_closed_tab_loses_its_connection_s_tab_at_once() {
        let mut r = rig();
        r.hello(r#","tab_token":"tok1""#);
        r.ask(r#"{"id":1,"method":"panel.state"}"#);
        assert_eq!(r.fake.calls.lock().unwrap()[0].2.as_deref(), Some("t1"));
        r.fake.t1_closed.store(true, Ordering::SeqCst);
        r.ask(r#"{"id":2,"method":"panel.state"}"#);
        assert_eq!(r.fake.calls.lock().unwrap()[1].2, None);
        assert_eq!(r.shared.hub.publish("x", json!({}), Some("t1")), 0);
        r.finish();
    }

    #[test]
    fn a_mod_token_gets_the_mod_grant() {
        let mut r = rig();
        let v = r.hello(r#","token":"good""#);
        assert_eq!(v["result"]["grant"]["kind"], "mod");
        assert_eq!(v["result"]["grant"]["id"], "m");
        let v = r.ask(r#"{"id":1,"method":"view.text","params":{"text":"x"}}"#);
        assert_eq!(v["result"]["ok"], "view.text");
        let v = r.ask(r#"{"id":2,"method":"panel.close"}"#);
        assert_eq!(code_of(&v), code::FORBIDDEN);
        r.finish();
    }

    #[test]
    fn disabling_a_mod_kills_its_open_connection() {
        let mut r = rig();
        r.hello(r#","token":"good""#);
        r.fake.revoked.store(true, Ordering::SeqCst);
        let v = r.ask(r#"{"id":1,"method":"view.text","params":{"text":"x"}}"#);
        assert_eq!(code_of(&v), code::UNAUTHORIZED);
        assert!(r.closed());
        assert!(r.fake.calls.lock().unwrap().is_empty());
        r.finish();
    }

    #[test]
    fn the_agent_cannot_reach_ui_methods_or_eval() {
        let mut r = rig();
        r.hello("");
        for m in [
            "shell.input",
            "settings.set",
            "mods.enable",
            "tools.respond",
            "browser.eval",
        ] {
            let v = r.ask(&format!(r#"{{"id":1,"method":"{m}"}}"#));
            assert_eq!(code_of(&v), code::FORBIDDEN, "{m}");
        }
        assert!(r.fake.calls.lock().unwrap().is_empty());
        r.fake.eval.store(true, Ordering::SeqCst);
        let v = r.ask(r#"{"id":2,"method":"browser.eval","params":{"expression":"1"}}"#);
        assert_eq!(v["result"]["ok"], "browser.eval");
        r.finish();
    }

    #[test]
    fn unknown_methods_are_answered_and_the_client_stays() {
        let mut r = rig();
        r.hello("");
        let v = r.ask(r#"{"id":1,"method":"browser.fly"}"#);
        assert_eq!(code_of(&v), code::UNKNOWN_METHOD);
        let v = r.ask(r#"{"id":2,"method":"panel.state"}"#);
        assert!(v["result"].is_object());
        r.finish();
    }

    #[test]
    fn a_call_reaches_the_dispatcher_with_the_tab_and_params() {
        let mut r = rig();
        r.hello(r#","tab_token":"tok1""#);
        let v = r.ask(r#"{"id":"abc","method":"view.image","params":{"path":"C:\\a.png"}}"#);
        assert_eq!(v["id"], "abc");
        assert_eq!(v["result"]["ok"], "view.image");
        let calls = r.fake.calls.lock().unwrap();
        assert_eq!(calls[0].0, Method::ViewImage);
        assert_eq!(calls[0].1["path"], "C:\\a.png");
        assert_eq!(calls[0].2.as_deref(), Some("t1"));
        assert_eq!(calls[0].3, "agent");
        drop(calls);
        r.finish();
    }

    #[test]
    fn dispatcher_errors_pass_through_unchanged() {
        let mut r = rig();
        r.hello("");
        let v = r.ask(r#"{"id":1,"method":"view.file","params":{"fail":"not_found"}}"#);
        assert_eq!(code_of(&v), code::NOT_FOUND);
        assert_eq!(v["error"]["message"], "scripted");
        r.finish();
    }

    #[test]
    fn malformed_json_closes_only_this_client() {
        let mut r = rig();
        r.hello("");
        r.send("{this is not json");
        let v = r.recv();
        assert_eq!(code_of(&v), code::PARSE_ERROR);
        assert_eq!(v["id"], Value::Null);
        assert!(r.closed());
        r.finish();
    }

    #[test]
    fn a_non_request_closes_the_client() {
        let mut r = rig();
        r.hello("");
        let v = r.ask("[1,2,3]");
        assert_eq!(code_of(&v), code::INVALID_REQUEST);
        assert!(r.closed());
        r.finish();
    }

    #[test]
    fn an_oversize_line_gets_too_large_and_closes() {
        let mut r = Rig::new(ServerConfig {
            max_line: 1000,
            ..ServerConfig::default()
        });
        r.hello("");
        r.send(&"x".repeat(5000));
        let v = r.recv();
        assert_eq!(code_of(&v), code::TOO_LARGE);
        assert!(r.closed());
        r.finish();
    }

    #[test]
    fn a_slow_call_times_out_and_the_connection_survives() {
        let mut r = Rig::new(ServerConfig {
            timeout_override: Some(Duration::from_millis(60)),
            ..ServerConfig::default()
        });
        r.hello("");
        r.fake.hang.store(true, Ordering::SeqCst);
        let v = r.ask(r#"{"id":1,"method":"panel.state"}"#);
        assert_eq!(code_of(&v), code::TIMEOUT);
        r.fake.hang.store(false, Ordering::SeqCst);
        let v = r.ask(r#"{"id":2,"method":"panel.state"}"#);
        assert!(v["result"].is_object());
        r.finish();
    }

    #[test]
    fn too_many_stuck_calls_refuse_new_ones() {
        let mut r = Rig::new(ServerConfig {
            timeout_override: Some(Duration::from_millis(30)),
            max_stuck_calls: 1,
            ..ServerConfig::default()
        });
        r.hello("");
        r.fake.hang.store(true, Ordering::SeqCst);
        assert_eq!(
            code_of(&r.ask(r#"{"id":1,"method":"panel.state"}"#)),
            code::TIMEOUT
        );
        assert_eq!(
            code_of(&r.ask(r#"{"id":2,"method":"panel.state"}"#)),
            code::UNAVAILABLE
        );
        r.finish();
    }

    #[test]
    fn the_rate_limit_answers_without_closing() {
        let mut r = Rig::new(ServerConfig {
            rate_burst: 4.0, // hello takes one
            rate_per_sec: 0.001,
            ..ServerConfig::default()
        });
        r.hello("");
        for i in 0..3 {
            let v = r.ask(&format!(r#"{{"id":{},"method":"panel.state"}}"#, i + 1));
            assert!(v["result"].is_object(), "call {i}");
        }
        let v = r.ask(r#"{"id":9,"method":"panel.state"}"#);
        assert_eq!(code_of(&v), code::RATE_LIMITED);
        r.finish();
    }

    #[test]
    fn forbidden_unknown_and_hello_requests_spend_rate_tokens_too() {
        let mut r = Rig::new(ServerConfig {
            rate_burst: 4.0,
            rate_per_sec: 0.001,
            ..ServerConfig::default()
        });
        r.hello("");
        for (i, m) in ["shell.input", "browser.fly", "hello"].iter().enumerate() {
            let v = r.ask(&format!(r#"{{"id":{i},"method":"{m}"}}"#));
            assert_ne!(code_of(&v), code::RATE_LIMITED, "{m}");
        }
        let v = r.ask(r#"{"id":9,"method":"shell.input"}"#);
        assert_eq!(code_of(&v), code::RATE_LIMITED);
        r.finish();
    }

    #[test]
    fn a_ui_token_is_refused_at_hello() {
        let mut r = rig();
        let v = r.hello(r#","token":"ui""#);
        assert_eq!(code_of(&v), code::UNAUTHORIZED);
        assert!(r.closed());
        r.finish();
    }

    #[test]
    fn a_ui_grant_for_the_agent_is_refused_at_hello() {
        let mut r = rig();
        r.fake.ui_agent.store(true, Ordering::SeqCst);
        assert_eq!(code_of(&r.hello("")), code::INTERNAL);
        r.finish();
    }

    #[test]
    fn a_grant_that_turns_into_ui_mid_connection_reaches_nothing() {
        let mut r = rig();
        r.hello(r#","token":"good""#);
        r.fake.promote.store(true, Ordering::SeqCst);
        let v = r.ask(r#"{"id":1,"method":"shell.input","params":{"data":"rm"}}"#);
        assert_eq!(code_of(&v), code::UNAUTHORIZED);
        assert!(r.closed());
        assert!(r.fake.calls.lock().unwrap().is_empty());
        r.finish();
        let mut r = rig();
        r.hello("");
        r.fake.ui_agent.store(true, Ordering::SeqCst);
        for m in ["shell.input", "settings.set", "mods.enable", "panel.state"] {
            let v = r.ask(&format!(r#"{{"id":1,"method":"{m}"}}"#));
            assert_eq!(code_of(&v), code::INTERNAL, "{m}");
        }
        assert!(r.fake.calls.lock().unwrap().is_empty());
        r.finish();
    }

    #[test]
    fn slow_calls_inside_their_timeout_are_not_stuck() {
        let fake = Arc::new(Fake::default());
        let shared = Shared::new(
            fake.clone(),
            ServerConfig {
                max_stuck_calls: 32,
                ..ServerConfig::default()
            },
        );
        let rigs: Vec<Rig> = (0..40)
            .map(|i| Rig::on(fake.clone(), shared.clone(), 100 + i))
            .collect();
        let workers: Vec<_> = rigs
            .into_iter()
            .map(|mut r| {
                std::thread::spawn(move || {
                    r.hello("");
                    let v = r.ask(r#"{"id":1,"method":"panel.state","params":{"slow":1}}"#);
                    let ok = v["result"].is_object();
                    r.finish();
                    ok
                })
            })
            .collect();
        for w in workers {
            assert!(w.join().unwrap(), "a slow call in time was refused");
        }
        assert_eq!(shared.stuck.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_stuck_call_stops_counting_when_it_finishes() {
        let mut r = Rig::new(ServerConfig {
            timeout_override: Some(Duration::from_millis(30)),
            ..ServerConfig::default()
        });
        r.hello("");
        r.fake.hang.store(true, Ordering::SeqCst);
        assert_eq!(
            code_of(&r.ask(r#"{"id":1,"method":"panel.state"}"#)),
            code::TIMEOUT
        );
        assert_eq!(r.shared.stuck.load(Ordering::SeqCst), 1);
        let t = Instant::now();
        while r.shared.in_flight.load(Ordering::SeqCst) > 0 && t.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(r.shared.stuck.load(Ordering::SeqCst), 0);
        r.finish();
    }

    #[test]
    fn a_waiting_call_gives_way_when_the_server_stops() {
        let mut r = Rig::new(ServerConfig::default());
        r.hello("");
        r.fake.hang.store(true, Ordering::SeqCst);
        // The call waits up to 15 s; the stop flag ends the wait within a slice.
        let t = Instant::now();
        r.send(r#"{"id":1,"method":"panel.state"}"#);
        std::thread::sleep(Duration::from_millis(100));
        r.shared.stop.store(true, Ordering::SeqCst);
        let _ = r.closed();
        assert!(t.elapsed() < Duration::from_secs(3));
        r.finish();
    }

    #[test]
    fn the_bucket_refills_with_time() {
        let t0 = Instant::now();
        let mut b = Bucket::new(10.0, 2.0, t0);
        assert!(b.take(t0));
        assert!(b.take(t0));
        assert!(!b.take(t0));
        assert!(b.take(t0 + Duration::from_millis(150)));
        assert!(!b.take(t0 + Duration::from_millis(150)));
    }

    #[test]
    fn silence_before_hello_closes_the_client() {
        let mut r = Rig::new(ServerConfig {
            hello_timeout: Duration::from_millis(80),
            ..ServerConfig::default()
        });
        assert!(r.closed());
        r.finish();
    }

    #[test]
    fn events_reach_only_the_matching_tab_and_after_hello() {
        let mut r = rig();
        assert_eq!(r.shared.hub.publish("panel.changed", json!({}), None), 0);
        r.hello(r#","tab_token":"tok1""#);
        assert_eq!(
            r.shared
                .hub
                .publish("panel.changed", json!({"k":1}), Some("t2")),
            0
        );
        assert_eq!(
            r.shared
                .hub
                .publish("panel.changed", json!({"k":2}), Some("t1")),
            1
        );
        let ev = r.recv();
        assert_eq!(ev["event"], "panel.changed");
        assert_eq!(ev["params"]["k"], 2);
        assert_eq!(r.shared.hub.publish("x", json!({}), None), 1);
        assert_eq!(r.recv()["event"], "x");
        r.finish();
    }

    #[test]
    fn a_connection_that_says_no_events_is_sent_none() {
        let mut r = rig();
        r.hello(r#","tab_token":"tok1","events":false"#);
        assert_eq!(
            r.shared.hub.publish("panel.changed", json!({}), Some("t1")),
            0
        );
        assert_eq!(r.shared.hub.publish("x", json!({}), None), 0);
        let v = r.ask(r#"{"id":1,"method":"tabs.list","params":{}}"#);
        assert_eq!(
            v["id"], 1,
            "the next line is the answer, no event came first"
        );
        r.finish();
    }

    #[test]
    fn the_outbox_drops_the_oldest_past_its_cap() {
        let h = ConnHandle::new(5);
        for i in 0..(OUTBOX_CAP + 10) {
            h.push(format!("{i}\n").into_bytes());
        }
        assert_eq!(h.queued(), OUTBOX_CAP);
        assert_eq!(h.drain()[0], b"10\n");
    }

    #[test]
    fn client_gone_is_called_once_after_hello_only() {
        let r = rig();
        let fake = r.fake.clone();
        r.finish();
        assert_eq!(fake.gone.load(Ordering::SeqCst), 0);
        let mut r = rig();
        let fake = r.fake.clone();
        r.hello("");
        r.finish();
        assert_eq!(fake.gone.load(Ordering::SeqCst), 1);
    }
}
