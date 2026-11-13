//! The client library: connect, `hello`, call with a timeout, collect events. Used by `flare-mcp` and
//! by mods' helper processes. Blocking, one request in flight at a time.

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::framing::{LineReader, ReadError, Wire, write_line};
use crate::method::Method;
use crate::proto::{
    CtlError, Event, Incoming, MAX_LINE, PROTOCOL, REMOTE_PROTOCOL, Request, code, parse_incoming,
    to_line,
};
use crate::session::{CancelFlag, remote_mismatch};

#[derive(Debug)]
pub enum ClientError {
    /// The pipe could not be opened or broke.
    Io(io::Error),
    /// No answer within the time given.
    Timeout,
    /// The server closed the connection.
    Closed,
    /// The server sent something that is not protocol v1.
    Protocol(String),
    /// The server answered with an error.
    Remote(CtlError),
    /// The stop flag ([`Client::with_stop`]) was raised while the call waited.
    Cancelled,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Io(e) => write!(f, "{e}"),
            ClientError::Timeout => f.write_str("timed out waiting for Flare"),
            ClientError::Closed => f.write_str("Flare closed the connection"),
            ClientError::Protocol(m) => write!(f, "unexpected answer from Flare: {m}"),
            ClientError::Remote(e) => write!(f, "{e}"),
            ClientError::Cancelled => f.write_str("the call was cancelled"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Events a client keeps while it waits for answers (the oldest go first).
const EVENTS_KEPT: usize = 256;

/// What `hello` sends.
#[derive(Clone, Debug, Default)]
pub struct HelloParams {
    pub client: String,
    pub version: String,
    pub token: Option<String>,
    /// The secret of the tab this client runs in (`FLARE_TAB_TOKEN`). The tab itself is never named.
    pub tab_token: Option<String>,
    /// True for a client that reads only while it waits for an answer: the server sends it no events.
    pub no_events: bool,
}

/// What `hello` answered.
#[derive(Clone, Debug, PartialEq)]
pub struct HelloInfo {
    pub protocol: u32,
    pub server_name: String,
    pub server_version: String,
    pub grant_kind: String,
    pub permissions: Vec<String>,
    pub capabilities: Vec<String>,
    pub tab: Option<String>,
    pub raw: Value,
}

impl HelloInfo {
    fn from_value(v: Value) -> Result<HelloInfo, ClientError> {
        let bad = |what: &str| ClientError::Protocol(format!("hello answer has no {what}"));
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let protocol = v["protocol"].as_u64().ok_or_else(|| bad("protocol"))? as u32;
        Ok(HelloInfo {
            protocol,
            server_name: v["server"]["name"].as_str().unwrap_or_default().into(),
            server_version: v["server"]["version"].as_str().unwrap_or_default().into(),
            grant_kind: v["grant"]["kind"]
                .as_str()
                .ok_or_else(|| bad("grant"))?
                .into(),
            permissions: strings(&v["grant"]["permissions"]),
            capabilities: strings(&v["capabilities"]),
            tab: v["tab"].as_str().map(str::to_string),
            raw: v,
        })
    }

    /// Whether the server lists `method` for this grant.
    pub fn can(&self, method: Method) -> bool {
        self.capabilities.iter().any(|c| c == method.name())
    }
}

/// What `remote.hello` sends: this end's machine and Flare version (the remote protocol is [`REMOTE_PROTOCOL`]).
#[derive(Clone, Debug, Default)]
pub struct RemoteHelloParams {
    pub machine: String,
    pub flare_version: String,
}

/// What `remote.hello` answered: the other end's remote protocol, machine, Flare version and methods.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteHelloInfo {
    pub remote_protocol: u32,
    pub machine: String,
    pub flare_version: String,
    /// The host's system (`mac`, `pc`, `other`; remote sessions PR 7), "" from a host that does not say it.
    pub os: String,
    pub capabilities: Vec<String>,
    pub raw: Value,
}

/// Lines that are no message a link may carry before the `remote.hello` answer (a login banner, a shell's greeting)
/// before the client gives up on it.
const MAX_NOISE_LINES: usize = 200;

pub struct Client<W: Wire> {
    wire: W,
    reader: LineReader,
    next_id: u64,
    events: VecDeque<Event>,
    stop: CancelFlag,
}

impl<W: Wire> Client<W> {
    pub fn new(wire: W) -> Self {
        Self {
            wire,
            reader: LineReader::new(MAX_LINE),
            next_id: 1,
            events: VecDeque::new(),
            stop: CancelFlag::default(),
        }
    }

    /// A flag another thread can raise to end a call in flight: the call answers [`ClientError::Cancelled`], and
    /// the caller drops the client so the server ends the job too. A blocking client cannot do this any other way.
    pub fn with_stop(mut self, stop: CancelFlag) -> Self {
        self.stop = stop;
        self
    }

    /// The wire, for tests that must send bytes the library never would.
    pub fn wire(&self) -> &W {
        &self.wire
    }

    /// Says hello. Must be the first call.
    pub fn hello(&mut self, h: &HelloParams, timeout: Duration) -> Result<HelloInfo, ClientError> {
        let mut params = json!({"client": h.client, "version": h.version, "protocol": PROTOCOL});
        if let Some(t) = &h.token {
            params["token"] = json!(t);
        }
        if let Some(t) = &h.tab_token {
            params["tab_token"] = json!(t);
        }
        if h.no_events {
            params["events"] = json!(false);
        }
        let v = self.call(Method::Hello, params, timeout)?;
        HelloInfo::from_value(v)
    }

    /// Says `remote.hello` on a remote link (the ssh child's stdio). Must be the first call. Lines before the answer
    /// that are no message (a login banner, a shell's greeting) are skipped. A peer of another remote protocol is
    /// [`ClientError::Remote`] with `unsupported_protocol` and the one line that names both versions (rule 278),
    /// whichever end noticed.
    pub fn remote_hello(
        &mut self,
        h: &RemoteHelloParams,
        timeout: Duration,
    ) -> Result<RemoteHelloInfo, ClientError> {
        let params = json!({
            "remote_protocol": REMOTE_PROTOCOL,
            "machine": h.machine,
            "flare_version": h.flare_version,
        });
        let v = self.exchange(Method::RemoteHello.name(), params, timeout, true)?;
        let theirs = v["remote_protocol"].as_u64().ok_or_else(|| {
            ClientError::Protocol("remote.hello answer has no remote_protocol".into())
        })?;
        if theirs != u64::from(REMOTE_PROTOCOL) {
            return Err(ClientError::Remote(CtlError::new(
                code::UNSUPPORTED_PROTOCOL,
                remote_mismatch(u64::from(REMOTE_PROTOCOL), theirs),
            )));
        }
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(RemoteHelloInfo {
            remote_protocol: REMOTE_PROTOCOL,
            machine: v["machine"].as_str().unwrap_or_default().into(),
            flare_version: v["flare_version"].as_str().unwrap_or_default().into(),
            os: v["os"].as_str().unwrap_or_default().into(),
            capabilities: strings(&v["capabilities"]),
            raw: v,
        })
    }

    /// Calls a method and waits for its answer.
    pub fn call(
        &mut self,
        method: Method,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        self.call_named(method.name(), params, timeout)
    }

    /// `call` by wire name, for a method this crate's enum does not know (a newer server).
    pub fn call_named(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        self.exchange(method, params, timeout, false)
    }

    /// One request and its answer. With `skip_noise`, lines that are no message are skipped (up to
    /// [`MAX_NOISE_LINES`]) instead of failing the call: only `remote.hello` asks for it.
    fn exchange(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        skip_noise: bool,
    ) -> Result<Value, ClientError> {
        let mut noise = 0usize;
        let id = self.next_id;
        self.next_id += 1;
        let line = to_line(&Request::new(id, method, params));
        if let Err(e) = write_line(&self.wire, &line, MAX_LINE) {
            // A Unix socket whose peer has already closed it can say ENOTCONN rather than EPIPE (macOS, seen on
            // the CI runner): the same hang-up, with the refusal still waiting to be read.
            let hung_up = e.kind() == io::ErrorKind::BrokenPipe
                || (cfg!(unix) && e.kind() == io::ErrorKind::NotConnected);
            if !hung_up {
                return Err(ClientError::Io(e));
            }
            // The server hung up first. If it left a reason (a refusal sent just before it closed),
            // that is the answer; else the connection is simply closed.
            let deadline = Some(Instant::now() + Duration::from_millis(500));
            if let Ok(line) = self
                .reader
                .read_line(&self.wire, deadline, self.stop.as_atomic())
                && let Ok(Incoming::Response(r)) = parse_incoming(&line)
                && let Some(err) = r.error
            {
                return Err(ClientError::Remote(err));
            }
            return Err(ClientError::Closed);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let line = self
                .reader
                .read_line(&self.wire, Some(deadline), self.stop.as_atomic())
                .map_err(read_error)?;
            let incoming = match parse_incoming(&line) {
                Ok(i) => i,
                Err(_) if skip_noise && noise < MAX_NOISE_LINES => {
                    noise += 1;
                    continue;
                }
                Err(e) => return Err(ClientError::Protocol(e)),
            };
            match incoming {
                Incoming::Event(e) => {
                    // A client that never takes them must not grow without bound.
                    if self.events.len() >= EVENTS_KEPT {
                        self.events.pop_front();
                    }
                    self.events.push_back(e);
                }
                Incoming::Response(r) => {
                    // A null id is the server refusing the connection (parse error, too large).
                    let ours = r.id == json!(id) || (r.id.is_null() && r.error.is_some());
                    if !ours {
                        continue; // the late answer of a request that timed out
                    }
                    return match (r.result, r.error) {
                        (Some(v), _) => Ok(v),
                        (None, Some(e)) => Err(ClientError::Remote(e)),
                        (None, None) => Err(ClientError::Protocol("empty response".into())),
                    };
                }
            }
        }
    }

    /// Events received so far (while waiting for answers, or by `wait_event`).
    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }

    /// Waits up to `timeout` for the next event.
    pub fn wait_event(&mut self, timeout: Duration) -> Result<Option<Event>, ClientError> {
        if let Some(e) = self.events.pop_front() {
            return Ok(Some(e));
        }
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .reader
                .read_line(&self.wire, Some(deadline), self.stop.as_atomic())
            {
                Err(ReadError::Timeout) => return Ok(None),
                Err(e) => return Err(read_error(e)),
                Ok(line) => match parse_incoming(&line).map_err(ClientError::Protocol)? {
                    Incoming::Event(e) => return Ok(Some(e)),
                    Incoming::Response(_) => continue,
                },
            }
        }
    }
}

fn read_error(e: ReadError) -> ClientError {
    match e {
        ReadError::Closed => ClientError::Closed,
        ReadError::Timeout => ClientError::Timeout,
        ReadError::TooLong => ClientError::Protocol("an answer passed the size limit".into()),
        ReadError::Stopped => ClientError::Cancelled,
        ReadError::Io(e) => ClientError::Io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::testwire::pair;

    fn serve_script(server: crate::framing::testwire::MemWire, answers: Vec<String>) {
        std::thread::spawn(move || {
            let mut r = LineReader::new(MAX_LINE);
            let stop = std::sync::atomic::AtomicBool::new(false);
            for a in answers {
                if r.read_line(
                    &server,
                    Some(Instant::now() + Duration::from_secs(5)),
                    &stop,
                )
                .is_err()
                {
                    return;
                }
                server.write_all(format!("{a}\n").as_bytes()).unwrap();
            }
        });
    }

    #[test]
    fn hello_parses_the_answer() {
        let (s, c) = pair();
        serve_script(
            s,
            vec![r#"{"id":1,"result":{"protocol":1,"server":{"name":"flare","version":"9"},"grant":{"kind":"agent","permissions":["view"]},"capabilities":["view.image"],"tab":"t1"}}"#.into()],
        );
        let mut cl = Client::new(c);
        let info = cl
            .hello(
                &HelloParams {
                    client: "x".into(),
                    version: "1".into(),
                    token: None,
                    tab_token: Some("tok".into()),
                    no_events: false,
                },
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(info.protocol, 1);
        assert_eq!(info.tab.as_deref(), Some("t1"));
        assert!(info.can(Method::ViewImage));
        assert!(!info.can(Method::ViewText));
    }

    #[test]
    fn events_before_the_answer_are_kept() {
        let (s, c) = pair();
        serve_script(
            s,
            vec![format!(
                "{}\n{}",
                r#"{"event":"a","params":{}}"#, r#"{"id":1,"result":{"v":1}}"#
            )],
        );
        let mut cl = Client::new(c);
        let v = cl
            .call(Method::PanelState, json!({}), Duration::from_secs(2))
            .unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(cl.take_events()[0].event, "a");
    }

    #[test]
    fn events_nobody_takes_are_capped_and_the_newest_stay() {
        let (s, c) = pair();
        let mut script = String::new();
        for i in 0..(EVENTS_KEPT + 50) {
            script.push_str(&format!(
                "{{\"event\":\"e\",\"params\":{{\"i\":{i}}}}}
"
            ));
        }
        script.push_str(r#"{"id":1,"result":{"v":1}}"#);
        serve_script(s, vec![script]);
        let mut cl = Client::new(c);
        cl.call(Method::PanelState, json!({}), Duration::from_secs(5))
            .unwrap();
        let got = cl.take_events();
        assert_eq!(got.len(), EVENTS_KEPT);
        assert_eq!(got.last().unwrap().params["i"], EVENTS_KEPT + 49);
    }

    #[test]
    fn remote_errors_map_to_remote() {
        let (s, c) = pair();
        serve_script(
            s,
            vec![r#"{"id":1,"error":{"code":"not_found","message":"no file"}}"#.into()],
        );
        let mut cl = Client::new(c);
        match cl.call(Method::ViewFile, json!({}), Duration::from_secs(2)) {
            Err(ClientError::Remote(e)) => assert_eq!(e.code, "not_found"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_null_id_error_is_the_answer_to_the_current_call() {
        let (s, c) = pair();
        serve_script(
            s,
            vec![r#"{"id":null,"error":{"code":"too_large","message":"x"}}"#.into()],
        );
        let mut cl = Client::new(c);
        match cl.call(Method::PanelState, json!({}), Duration::from_secs(2)) {
            Err(ClientError::Remote(e)) => assert_eq!(e.code, "too_large"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_stale_answer_is_skipped() {
        let (s, c) = pair();
        serve_script(
            s,
            vec![format!(
                "{}\n{}",
                r#"{"id":99,"result":{"old":1}}"#, r#"{"id":1,"result":{"new":1}}"#
            )],
        );
        let mut cl = Client::new(c);
        let v = cl
            .call(Method::PanelState, json!({}), Duration::from_secs(2))
            .unwrap();
        assert_eq!(v["new"], 1);
    }

    #[test]
    fn no_answer_times_out() {
        let (_s, c) = pair();
        let mut cl = Client::new(c);
        assert!(matches!(
            cl.call(Method::PanelState, json!({}), Duration::from_millis(50)),
            Err(ClientError::Timeout)
        ));
    }

    #[test]
    fn a_raised_stop_flag_ends_a_call_in_flight() {
        let (_s, c) = pair();
        let stop = CancelFlag::default();
        let mut cl = Client::new(c).with_stop(stop.clone());
        stop.set();
        assert!(matches!(
            cl.call(Method::PanelState, json!({}), Duration::from_secs(5)),
            Err(ClientError::Cancelled)
        ));
    }

    #[test]
    fn a_closed_server_is_closed() {
        let (s, c) = pair();
        s.close();
        let mut cl = Client::new(c);
        assert!(matches!(
            cl.call(Method::PanelState, json!({}), Duration::from_secs(1)),
            Err(ClientError::Closed)
        ));
    }

    #[test]
    fn garbage_from_the_server_is_a_protocol_error() {
        let (s, c) = pair();
        serve_script(s, vec!["not json".into()]);
        let mut cl = Client::new(c);
        assert!(matches!(
            cl.call(Method::PanelState, json!({}), Duration::from_secs(2)),
            Err(ClientError::Protocol(_))
        ));
    }

    #[test]
    fn remote_hello_skips_a_banner_before_the_answer() {
        let (s, c) = pair();
        serve_script(
            s,
            vec![format!(
                "Welcome to the Mac mini\n\nLast login: Thu Nov 12\r\n{{oops\n{}",
                r#"{"id":1,"result":{"remote_protocol":1,"machine":"mini","flare_version":"0.4.0","os":"mac","capabilities":["remote.hello"]}}"#
            )],
        );
        let mut cl = Client::new(c);
        let info = cl
            .remote_hello(
                &RemoteHelloParams {
                    machine: "pc".into(),
                    flare_version: "0.4.0".into(),
                },
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(info.remote_protocol, REMOTE_PROTOCOL);
        assert_eq!(info.machine, "mini");
        assert_eq!(info.flare_version, "0.4.0");
        // The host's system (remote sessions PR 7); an older host that does not say it reads as "".
        assert_eq!(info.os, "mac");
        assert!(info.capabilities.iter().any(|c| c == "remote.hello"));
    }

    /// Rule 278 from the viewer's side: a host that answered with another remote protocol is refused with the one
    /// line, this side's version first.
    #[test]
    fn a_remote_hello_answer_of_another_version_is_refused_naming_both() {
        let (s, c) = pair();
        serve_script(
            s,
            vec![
                r#"{"id":1,"result":{"remote_protocol":5,"machine":"m","flare_version":"9"}}"#
                    .into(),
            ],
        );
        let mut cl = Client::new(c);
        match cl.remote_hello(&RemoteHelloParams::default(), Duration::from_secs(2)) {
            Err(ClientError::Remote(e)) => {
                assert_eq!(e.code, code::UNSUPPORTED_PROTOCOL);
                assert_eq!(e.message, "remote protocol 1 here, 5 there");
            }
            other => panic!("{other:?}"),
        }
    }

    /// Only `remote.hello` skips noise: an ordinary call still fails on a line that is no message.
    #[test]
    fn noise_is_skipped_only_for_remote_hello_and_only_so_much() {
        let (s, c) = pair();
        serve_script(s, vec!["banner".into()]);
        let mut cl = Client::new(c);
        assert!(matches!(
            cl.call(Method::PanelState, json!({}), Duration::from_secs(2)),
            Err(ClientError::Protocol(_))
        ));
        let (s, c) = pair();
        serve_script(s, vec!["noise\n".repeat(MAX_NOISE_LINES + 1)]);
        let mut cl = Client::new(c);
        assert!(matches!(
            cl.remote_hello(&RemoteHelloParams::default(), Duration::from_secs(2)),
            Err(ClientError::Protocol(_))
        ));
    }

    #[test]
    fn wait_event_returns_none_on_quiet() {
        let (_s, c) = pair();
        let mut cl = Client::new(c);
        assert!(cl.wait_event(Duration::from_millis(30)).unwrap().is_none());
    }
}
