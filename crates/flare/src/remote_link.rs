//! The viewer's end of a remote link (remote sessions PR 3): one worker per remote tab that starts the link, opens or
//! attaches the host's tab, and then moves typing, sizes and output between the tab's `RemotePty` and the host.
//!
//! The link is `ssh <host> <flare> --serve-remote` (`SshConnector`), speaking the control protocol over the child's
//! stdio (`StdioWire::from_child`). The host and the user are argv items of their own, the remote command is built for
//! the host's login shell (sh on a Mac, cmd on a PC), and no token or environment of this machine is on the line
//! (rule 290). Tests join a `Link` to an in-process host instead; under `--e2e` the command can be a loopback
//! `flare --serve-remote` of another profile (`FLARE_E2E_REMOTE_COMMAND`), never a real ssh.
//!
//! Output is polled (`remote.output {since}`), never pushed, in short polls: a call at a time, 30 ms apart while
//! output flows, up to 250 ms apart when idle, and at once when the tab typed. A broken link is tried again after 1 s,
//! then 2, 4 and so on up to 30 s, and the tab goes on from the output number it read: no byte twice. A tab that has
//! shown nothing of the host yet (a new tab, or one brought back by a restart) starts from `remote.snapshot`, as does
//! one whose number fell out of the host's ring (`gap`). What a reconnect catches up on is the past: the agent's
//! signals are taken out of it (rule 289).
//!
//! Closing the tab closes the host's session (`remote.close`, which asks as any close does); the quit and a dropped
//! link only detach (rule 288). Every wait here is on this worker: a dead link never holds the UI thread, a close or
//! the quit (rule 28).

use base64::Engine as _;
use flare_ctl::framing::Wire;
use flare_ctl::remote::{
    Intent, KeyIntent, MAX_INPUT_BYTES, MAX_INTENTS, OUTPUT_MAX, intents_json,
};
use flare_ctl::{Client, ClientError, CtlError, Method, RemoteHelloParams, StdioWire, code};
use flare_tabs::tabs::Os;
use flare_term::handle::RemoteIntent;
use flare_term::pty::{PtyProcess, PtySpawner, SpawnSpec};
use flare_term::remote_pty::{
    Leaving, LinkState, RemoteIo, RemotePty, RemoteSpec, SignalStrip, plain_snapshot,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Why a call over the link failed.
#[derive(Debug)]
pub enum LinkError {
    /// The link broke or never came up (ssh ended, a timeout, a line that is no protocol): try again later.
    Down(String),
    /// The host answered with an error (`no_tab`, `bad_params`, a version refusal).
    Refused(CtlError),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Down(m) => write!(f, "{m}"),
            LinkError::Refused(e) => write!(f, "{}: {}", e.code, e.message),
        }
    }
}

fn of_client(e: ClientError) -> LinkError {
    match e {
        ClientError::Remote(e) => LinkError::Refused(e),
        other => LinkError::Down(other.to_string()),
    }
}

/// One link to one host: a call at a time.
pub trait Link: Send {
    fn call(
        &mut self,
        method: Method,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LinkError>;
}

/// Starts links. BLOCKING (it starts ssh and waits for `remote.hello`): only a tab's worker calls it.
pub trait Connector: Send + Sync {
    fn connect(&self, machine: &str) -> Result<Box<dyn Link>, LinkError>;
}

/// A link over a control protocol client, and the process that carries it (ssh), ended with the link.
pub struct ClientLink<W: Wire> {
    client: Client<W>,
    child: Option<std::process::Child>,
}

impl<W: Wire> ClientLink<W> {
    pub fn new(client: Client<W>, child: Option<std::process::Child>) -> Self {
        Self { client, child }
    }
}

impl<W: Wire + Send> Link for ClientLink<W> {
    fn call(
        &mut self,
        method: Method,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LinkError> {
        self.client.call(method, params, timeout).map_err(of_client)
    }
}

impl<W: Wire> Drop for ClientLink<W> {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait(); // sync-ok: the tab's link worker, after a kill: the child is gone at once
        }
    }
}

// ---- machines ----

/// A machine a remote tab can run on, as `machines.json` in the profile names it (hand-edited until PR 7's page):
/// `{"machines":[{"id":"mac","name":"Mac Mini","kind":"mac","ssh":"mac-mini","user":"max","flare":"/Applications/..."}]}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    pub id: String,
    pub name: String,
    /// `mac` or `pc`; anything else is no kind and reads as a Mac's shell (rule 40, never written back).
    pub kind: Option<Os>,
    /// The ssh host (an alias of `~/.ssh/config` or a name).
    pub ssh: String,
    pub user: Option<String>,
    /// Flare's executable on the host; `flare` (on the host's PATH) when not given.
    pub flare: String,
}

/// A word that goes to ssh as one argv item: printable, no space, never read as an option.
fn argv_word(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_graphic())
}

/// Pure: the machines a `machines.json` names. A machine whose id, host or user is not a plain word is left out.
pub fn parse_machines(raw: &str) -> Vec<Machine> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(list) = v.get("machines").and_then(Value::as_array) else {
        return Vec::new();
    };
    let s = |m: &Value, k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
    list.iter()
        .filter_map(|m| {
            let id = s(m, "id").filter(|i| argv_word(i))?;
            let ssh = s(m, "ssh").filter(|h| argv_word(h))?;
            let user = match m.get("user") {
                None | Some(Value::Null) => None,
                Some(_) => Some(s(m, "user").filter(|u| argv_word(u))?),
            };
            Some(Machine {
                name: s(m, "name")
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| id.clone()),
                kind: s(m, "kind").as_deref().and_then(Os::parse),
                flare: s(m, "flare")
                    .filter(|f| !f.trim().is_empty())
                    .unwrap_or_else(|| "flare".to_string()),
                id,
                ssh,
                user,
            })
        })
        .collect()
}

/// `machines.json` in the profile. BLOCKING (a file read): a worker's call.
pub fn machines_file(profile: &Path) -> PathBuf {
    profile.join("machines.json")
}

pub fn read_machines(path: &Path) -> Vec<Machine> {
    std::fs::read_to_string(path) // sync-ok: the tab's link worker or a test, never the UI thread
        .map(|raw| parse_machines(&raw))
        .unwrap_or_default()
}

/// Pure: the command the host's login shell runs (rule 290: no token, no environment). A Mac's sh gets the path in
/// single quotes; a PC's cmd in double quotes, with no `&&` and nothing cmd would expand. None for a path that cannot
/// be quoted safely there.
pub fn remote_command(kind: Option<Os>, flare: &str) -> Option<String> {
    if flare.is_empty() || flare.chars().any(char::is_control) {
        return None;
    }
    match kind {
        Some(Os::Pc) => {
            if flare.contains(['"', '%', '^', '!']) {
                return None;
            }
            Some(format!("\"{flare}\" --serve-remote"))
        }
        _ => Some(format!("'{}' --serve-remote", flare.replace('\'', "'\\''"))),
    }
}

/// This machine's ssh: by its absolute path (rule 1).
fn ssh_exe() -> String {
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        format!("{root}\\System32\\OpenSSH\\ssh.exe")
    }
    #[cfg(not(windows))]
    {
        "/usr/bin/ssh".to_string()
    }
}

/// Pure: ssh's argv for a machine, every part an item of its own: no shell on this side, no option smuggled in a
/// host name, no prompt (`BatchMode`), no terminal (`-T`).
pub fn ssh_argv(ssh: &str, m: &Machine) -> Option<Vec<String>> {
    if !argv_word(&m.ssh) || m.user.as_deref().is_some_and(|u| !argv_word(u)) {
        return None;
    }
    let mut argv = vec![
        ssh.to_string(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-T".into(),
    ];
    if let Some(u) = &m.user {
        argv.push("-l".into());
        argv.push(u.clone());
    }
    argv.push("--".into());
    argv.push(m.ssh.clone());
    argv.push(remote_command(m.kind, &m.flare)?);
    Some(argv)
}

/// How long `remote.hello` may take once the link is up (the host may start its Flare first, PR 5).
const HELLO_WAIT: Duration = Duration::from_secs(30);

/// The real connector: ssh to the machine `machines.json` names, or, under `--e2e`, the loopback command the scenario
/// gives (`FLARE_E2E_REMOTE_COMMAND`, a JSON list: an argv).
pub struct SshConnector {
    machines: PathBuf,
    loopback: Option<Vec<String>>,
}

impl SshConnector {
    pub fn new(profile: &Path, loopback: Option<Vec<String>>) -> Self {
        Self {
            machines: machines_file(profile),
            loopback,
        }
    }

    fn argv(&self, machine: &str) -> Result<Vec<String>, LinkError> {
        if let Some(l) = &self.loopback {
            return Ok(l.clone());
        }
        let m = read_machines(&self.machines)
            .into_iter()
            .find(|m| m.id == machine)
            .ok_or_else(|| {
                LinkError::Down(format!("no machine called {machine} in machines.json"))
            })?;
        ssh_argv(&ssh_exe(), &m)
            .ok_or_else(|| LinkError::Down(format!("cannot build the ssh command for {machine}")))
    }
}

/// The name this machine gives the host in `remote.hello`.
fn this_machine() -> String {
    std::env::var(if cfg!(windows) {
        "COMPUTERNAME"
    } else {
        "HOSTNAME"
    })
    .ok()
    .filter(|n| !n.is_empty())
    .unwrap_or_else(|| "viewer".to_string())
}

impl Connector for SshConnector {
    fn connect(&self, machine: &str) -> Result<Box<dyn Link>, LinkError> {
        let argv = self.argv(machine)?;
        let (exe, args) = argv
            .split_first()
            .ok_or_else(|| LinkError::Down("an empty link command".into()))?;
        let mut cmd = std::process::Command::new(exe); // sync-ok: the tab's link worker starts ssh, never the UI thread
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window, ever (tests never take the screen; nor does a link).
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| LinkError::Down(format!("could not start the link: {e}")))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(LinkError::Down("the link has no stdio".into()));
        };
        let wire = match StdioWire::from_child(stdin, stdout) {
            Ok(w) => w,
            Err(e) => {
                let _ = child.kill();
                return Err(LinkError::Down(format!("a wire over the link: {e}")));
            }
        };
        let mut link = ClientLink::new(Client::new(wire), Some(child));
        hello(&mut link.client)?;
        Ok(Box::new(link))
    }
}

/// `remote.hello` over a fresh client.
pub fn hello<W: Wire>(c: &mut Client<W>) -> Result<(), LinkError> {
    c.remote_hello(
        &RemoteHelloParams {
            machine: this_machine(),
            flare_version: crate::app_facts::version().to_string(),
        },
        HELLO_WAIT,
    )
    .map(|_| ())
    .map_err(of_client)
}

// ---- typing ----

/// The wire form of one typed thing.
fn wire_intent(i: &RemoteIntent) -> Intent {
    match i {
        RemoteIntent::Key(k) => Intent::Key(KeyIntent {
            key: k.key.clone(),
            code: k.code.clone(),
            text: k.text.clone(),
            ctrl: k.ctrl,
            shift: k.shift,
            alt: k.alt,
            meta: k.meta,
        }),
        RemoteIntent::Text(t) => Intent::Text(t.clone()),
        RemoteIntent::Paste(p) => Intent::Paste(p.clone()),
    }
}

/// The bytes an intent counts toward `MAX_INPUT_BYTES`, as the host counts them.
fn weight(i: &Intent) -> usize {
    match i {
        Intent::Key(k) => k.key.len() + k.code.len() + k.text.as_ref().map_or(0, String::len),
        Intent::Text(s) | Intent::Paste(s) => s.len(),
    }
}

/// The longest piece a long text or paste is cut into: a call carries a few of them, well under the cap.
const PIECE: usize = 16 * 1024;

fn pieces(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if cur.len() + c.len_utf8() > PIECE {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Pure: the typing as `remote.input` calls, in order, each within `MAX_INTENTS` and `MAX_INPUT_BYTES` (the host
/// refuses a call over either, and types nothing of it). A long text or paste is cut on character boundaries.
pub fn input_batches(typed: &[RemoteIntent]) -> Vec<Vec<Intent>> {
    let mut all: Vec<Intent> = Vec::new();
    for i in typed {
        match wire_intent(i) {
            Intent::Text(t) => all.extend(pieces(&t).into_iter().map(Intent::Text)),
            Intent::Paste(p) => all.extend(pieces(&p).into_iter().map(Intent::Paste)),
            key => all.push(key),
        }
    }
    let mut out: Vec<Vec<Intent>> = Vec::new();
    let mut cur: Vec<Intent> = Vec::new();
    let mut bytes = 0;
    for i in all {
        let w = weight(&i);
        if !cur.is_empty() && (cur.len() >= MAX_INTENTS || bytes + w > MAX_INPUT_BYTES) {
            out.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        bytes += w;
        cur.push(i);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---- the tab's worker ----

/// The waits of a tab's worker.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Between polls while output flows.
    pub poll_min: Duration,
    /// Between polls when idle.
    pub poll_max: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// One call's time: a poll, typing, a size, an attach (the host's default 15 s).
    pub call: Duration,
    /// `remote.open` and `remote.snapshot`, which take `REMOTE_CALL`'s 60 s on the host.
    pub long_call: Duration,
    /// The close or detach sent as the tab goes.
    pub end_call: Duration,
    /// Between two `remote.status` polls (remote sessions PR 4).
    pub status_every: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll_min: Duration::from_millis(30),
            poll_max: Duration::from_millis(250),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            call: Duration::from_secs(15),
            long_call: Duration::from_secs(60),
            end_call: Duration::from_secs(5),
            status_every: Duration::from_millis(1500),
        }
    }
}

/// Pure: the wait after `prev`: doubled, at most `max`.
pub fn next_backoff(prev: Duration, max: Duration) -> Duration {
    (prev * 2).min(max)
}

/// Remote tabs' pties, each with its worker (the spawner's remote side, `RoutingSpawner`).
pub struct RemoteSpawner {
    connector: Arc<dyn Connector>,
    timing: Timing,
    /// Wakes the window when a tab's link heard a new status from the host (remote sessions PR 4).
    wake: Option<Wake>,
}

/// Wakes the UI thread (the window's `HostWake`).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

impl RemoteSpawner {
    pub fn new(connector: Arc<dyn Connector>, timing: Timing) -> Self {
        Self {
            connector,
            timing,
            wake: None,
        }
    }

    /// The window is woken when a remote tab's status changes on the host.
    pub fn with_wake(mut self, wake: Wake) -> Self {
        self.wake = Some(wake);
        self
    }
}

impl PtySpawner for RemoteSpawner {
    /// Never waits for the other machine: the worker connects.
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn PtyProcess>, String> {
        let remote = spec.remote.clone().ok_or("not a remote tab")?;
        let io = RemoteIo::new();
        let pty = RemotePty::new(Arc::clone(&io));
        let worker = Worker {
            connector: Arc::clone(&self.connector),
            t: self.timing,
            spec: remote,
            io,
            size: Some((spec.cols, spec.rows)),
            strip: SignalStrip::new(),
            fresh: true,
            told: false,
            wake: self.wake.clone(),
            status: crate::remote_status::StatusPoll::new(self.timing.status_every),
        };
        std::thread::Builder::new()
            .name("flare-remote-link".into())
            .spawn(move || worker.run())
            .map_err(|e| format!("could not start the link worker: {e}"))?;
        Ok(Box::new(pty))
    }
}

enum Step {
    /// The tab is done with the link (closed, left, ended on the host, refused).
    Ended,
    /// The link broke: wait, then try again.
    Down,
}

struct Worker {
    connector: Arc<dyn Connector>,
    t: Timing,
    spec: RemoteSpec,
    io: Arc<RemoteIo>,
    /// The tab's last size, sent at every attach.
    size: Option<(u16, u16)>,
    strip: SignalStrip,
    /// Nothing of the host shown yet: the first attach starts from a snapshot.
    fresh: bool,
    /// A refusal of the link was shown in the tab once already.
    told: bool,
    wake: Option<Wake>,
    /// The `remote.status` polls of this tab (remote sessions PR 4).
    status: crate::remote_status::StatusPoll,
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(v["data"].as_str().unwrap_or_default())
        .unwrap_or_default()
}

impl Worker {
    fn run(mut self) {
        let live = self.spec.live.clone();
        let mut backoff = self.t.backoff_min;
        loop {
            live.set_link(LinkState::Connecting);
            match self.session() {
                Step::Ended => break,
                Step::Down => {
                    // A link that was up starts the back-off again from its shortest wait.
                    if live.link() == LinkState::Live {
                        backoff = self.t.backoff_min;
                    }
                    live.set_link(LinkState::Down);
                    if !self.wait(backoff) {
                        // sync-ok: the tab's link worker waits out its back-off
                        break;
                    }
                    backoff = next_backoff(backoff, self.t.backoff_max);
                }
            }
        }
        live.set_link(LinkState::Ended);
    }

    /// Waits `d` (sizes asked meanwhile are kept, typing while the link is down is not). False when the tab let go
    /// meanwhile: a close is still taken to the host, on a link of its own.
    fn wait(&mut self, d: Duration) -> bool {
        let end = Instant::now() + d;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let out = self.io.take_outgoing(left);
            if let Some(s) = out.size {
                self.size = Some(s);
            }
            if let Some(how) = out.leaving {
                if how == Leaving::Close {
                    self.close_without_a_link();
                }
                return false;
            }
            if left.is_zero() {
                return true;
            }
        }
    }

    /// The tab was closed while the link was down: one try to tell the host, so the close ends the session there too.
    fn close_without_a_link(&self) {
        let Some(tab) = self.spec.live.host_tab() else {
            return;
        };
        if let Ok(mut link) = self.connector.connect(&self.spec.machine) {
            let _ = link.call(Method::RemoteClose, json!({"tab": tab}), self.t.end_call);
        }
    }

    /// Text in the tab's terminal, before the host's screen is in it (a refusal the person should read).
    fn say(&self, text: &str) {
        self.io
            .push_output(format!("\r\n\x1b[31m{text}\x1b[0m\r\n").as_bytes());
    }

    /// One life of the link: connect, open or attach, catch up, then the polls until it breaks or the tab goes.
    fn session(&mut self) -> Step {
        if let Some(how) = self.io.leaving() {
            if how == Leaving::Close {
                self.close_without_a_link();
            }
            return Step::Ended;
        }
        let mut link = match self.connector.connect(&self.spec.machine) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("remote link to {}: {e}", self.spec.machine);
                if self.fresh
                    && !self.told
                    && let LinkError::Refused(r) = &e
                {
                    // A version refusal names both versions in one line (rule 278): show it once.
                    self.say(&r.message);
                    self.told = true;
                }
                return Step::Down;
            }
        };
        let live = self.spec.live.clone();
        let tab = match live.host_tab() {
            Some(t) => t,
            None => match link.call(
                Method::RemoteOpen,
                json!({"folder": self.spec.folder}),
                self.t.long_call,
            ) {
                Ok(v) => match v["tab"].as_str() {
                    Some(t) => {
                        live.set_host_tab(t);
                        t.to_string()
                    }
                    None => return Step::Down,
                },
                Err(LinkError::Refused(e)) => {
                    self.say(&format!(
                        "Could not open {} on {}: {}",
                        self.spec.folder, self.spec.machine, e.message
                    ));
                    return Step::Ended;
                }
                Err(LinkError::Down(_)) => return Step::Down,
            },
        };
        match link.call(Method::RemoteAttach, json!({"tab": tab}), self.t.call) {
            Ok(_) => {}
            Err(LinkError::Refused(e)) if e.code == code::NO_TAB => {
                // The host's tab is gone (closed there, or the host started again): the tab ends as a shell that exits.
                self.io.finish(None);
                return Step::Ended;
            }
            Err(LinkError::Refused(e)) => {
                log::warn!("remote.attach refused: {}", e.message);
                return Step::Down;
            }
            Err(LinkError::Down(_)) => return Step::Down,
        }
        if let Some((cols, rows)) = self.size
            && let Err(e) = link.call(
                Method::RemoteResize,
                json!({"tab": tab, "cols": cols, "rows": rows}),
                self.t.call,
            )
        {
            return self.failed(e);
        }
        let caught = if self.fresh {
            self.snapshot(&mut *link, &tab)
        } else {
            self.catch_up(&mut *link, &tab)
        };
        if let Err(e) = caught {
            return self.failed(e);
        }
        self.fresh = false;
        live.set_link(LinkState::Live);
        // The host's status is asked at once on every attach, and its first answer is always news (rule 294).
        self.status.attached();
        self.pump(&mut *link, &tab)
    }

    /// What a failed call means: the session gone on the host ends the tab; anything else is a broken link.
    fn failed(&self, e: LinkError) -> Step {
        match e {
            LinkError::Refused(r) if r.code == code::NO_TAB => {
                self.io.finish(None);
                Step::Ended
            }
            _ => Step::Down,
        }
    }

    /// The host's screen and history as text, and the number it holds (rule 289: text only, no mark rides on it).
    fn snapshot(&mut self, link: &mut dyn Link, tab: &str) -> Result<(), LinkError> {
        let v = link.call(
            Method::RemoteSnapshot,
            json!({"tab": tab}),
            self.t.long_call,
        )?;
        let seq = v["seq"]
            .as_u64()
            .ok_or_else(|| LinkError::Down("a snapshot with no seq".into()))?;
        self.io.push_output(&plain_snapshot(&b64(&v)));
        self.spec.live.set_seq(seq);
        Ok(())
    }

    /// After a reconnect: everything since the number read, as a replay (its signals taken out); a snapshot when the
    /// number fell out of the host's ring.
    fn catch_up(&mut self, link: &mut dyn Link, tab: &str) -> Result<(), LinkError> {
        loop {
            let since = self.spec.live.seq();
            let v = link.call(
                Method::RemoteOutput,
                json!({"tab": tab, "since": since, "max": OUTPUT_MAX}),
                self.t.call,
            )?;
            if v["gap"].as_bool() == Some(true) {
                return self.snapshot(link, tab);
            }
            let data = b64(&v);
            self.io.push_output(&self.strip.feed(&data, true));
            self.spec.live.set_seq(v["to"].as_u64().unwrap_or(since));
            if data.len() < OUTPUT_MAX {
                return Ok(());
            }
        }
    }

    /// The host's status of the tab, every `status_every` (remote sessions PR 4, rule 294): a change is handed to the
    /// window and wakes it. A host with no `remote.status` (an older Flare, `unknown_method`) is asked no more on this
    /// link; any other refusal (a host UI wait that timed out, a busy or rate refusal) is passing and the next poll asks
    /// again (rule 298). A broken link ends the polls as any call's does.
    fn poll_status(&mut self, link: &mut dyn Link, tab: &str) -> Result<(), LinkError> {
        if !self.status.due(Instant::now()) {
            return Ok(());
        }
        match link.call(Method::RemoteStatus, json!({}), self.t.call) {
            Ok(v) => {
                if let Some(s) = self.status.news(&v, tab) {
                    self.spec.live.set_status(s);
                    if let Some(w) = &self.wake {
                        w();
                    }
                }
                Ok(())
            }
            Err(LinkError::Refused(e)) if e.code == code::UNKNOWN_METHOD => {
                log::info!("remote.status unknown to the host, not asked again on this link");
                self.status.off();
                Ok(())
            }
            Err(LinkError::Refused(e)) => {
                log::info!(
                    "remote.status refused, asked again at the next poll: {}",
                    e.code
                );
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// The live polls: typing and sizes as they come, output in short polls.
    fn pump(&mut self, link: &mut dyn Link, tab: &str) -> Step {
        let mut idle = self.t.poll_min;
        loop {
            let out = self.io.take_outgoing(idle);
            if let Some(how) = out.leaving {
                let m = match how {
                    Leaving::Close => Method::RemoteClose,
                    Leaving::Detach => Method::RemoteDetach,
                };
                let _ = link.call(m, json!({"tab": tab}), self.t.end_call);
                return Step::Ended;
            }
            let mut busy = false;
            if let Some((cols, rows)) = out.size {
                self.size = Some((cols, rows));
                if let Err(e) = link.call(
                    Method::RemoteResize,
                    json!({"tab": tab, "cols": cols, "rows": rows}),
                    self.t.call,
                ) {
                    return self.failed(e);
                }
            }
            for batch in input_batches(&out.intents) {
                busy = true;
                match link.call(
                    Method::RemoteInput,
                    json!({"tab": tab, "intents": intents_json(&batch)}),
                    self.t.call,
                ) {
                    Ok(_) => {}
                    Err(LinkError::Refused(e)) if e.code != code::NO_TAB => {
                        log::warn!("remote.input refused: {}", e.message);
                    }
                    Err(e) => return self.failed(e),
                }
            }
            let since = self.spec.live.seq();
            match link.call(
                Method::RemoteOutput,
                json!({"tab": tab, "since": since}),
                self.t.call,
            ) {
                Ok(v) if v["gap"].as_bool() == Some(true) => {
                    if let Err(e) = self.snapshot(link, tab) {
                        return self.failed(e);
                    }
                    busy = true;
                }
                Ok(v) => {
                    let data = b64(&v);
                    if !data.is_empty() {
                        busy = true;
                        self.io.push_output(&self.strip.feed(&data, false));
                    }
                    self.spec.live.set_seq(v["to"].as_u64().unwrap_or(since));
                }
                Err(e) => return self.failed(e),
            }
            if let Err(e) = self.poll_status(link, tab) {
                return self.failed(e);
            }
            idle = if busy {
                self.t.poll_min
            } else {
                next_backoff(idle, self.t.poll_max)
            };
        }
    }
}

/// The e2e's loopback command (`FLARE_E2E_REMOTE_COMMAND`, a JSON list of strings), read only under `--e2e`.
pub fn e2e_loopback(raw: Option<&str>) -> Option<Vec<String>> {
    let v: Vec<String> = serde_json::from_str(raw?).ok()?;
    (!v.is_empty()).then_some(v)
}

/// The e2e's remote tab to open at start (`FLARE_E2E_REMOTE_OPEN`: `{"machine":..,"folder":..}`), read only under
/// `--e2e`: the way a scenario opens a remote tab before PR 5's command and PR 7's menu exist.
pub fn e2e_open(raw: Option<&str>) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(raw?).ok()?;
    let machine = v["machine"].as_str().filter(|m| argv_word(m))?;
    let folder = v["folder"].as_str().filter(|f| !f.is_empty())?;
    Some((machine.to_string(), folder.to_string()))
}

#[cfg(test)]
#[path = "remote_link_tests.rs"]
mod tests;
