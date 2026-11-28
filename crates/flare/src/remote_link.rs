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
//! output flows, up to 250 ms apart when idle, and at once when the tab typed. Rule 1392: after typing the host took,
//! the polls come 3 ms apart until its echo arrives (100 ms at most), only while the link's call budget has room
//! (`remote_timing`, well under the host's 100 calls a second), and the status and conversation polls wait while a key
//! waits for its echo (half a second at most). Every call is timed by its method, and each key from when it was typed
//! to its echo; a summary goes to the diagnostics log as a `remote-typing` line. A broken link is tried again after 1 s,
//! then 2, 4 and so on up to 30 s, and the tab goes on from the output number it read: no byte twice. A tab that has
//! shown nothing of the host yet (a new tab, or one brought back by a restart) starts from `remote.snapshot`, as does
//! one whose number fell out of the host's ring (`gap`). What a reconnect catches up on is the past: the agent's
//! signals are taken out of it (rule 289).
//!
//! Rule 1210, the link's edge cases: an attach the host answers `unavailable` (a new tab's shell still starting) is
//! tried again on the same link every 250 ms for up to 10 s; `remote.open` carries the tab's open id, so an open whose
//! answer was lost is answered with the tab the first one opened; and typing while the link is down is kept (30 s and
//! 64 KB at most) and sent in order after the next attach, never into another host tab.
//!
//! Between the typing and the output poll the worker does the window's file work (remote sessions PR 6, `Errand`): a
//! file link's host file fetched in chunks (`remote.fetch`, rule 303), and dropped or pasted files copied to the host
//! (`remote.put`) with their host paths pasted as one paste (rule 304); a dropped folder goes whole, its folders and
//! files under it, and only its path is pasted (rule 1213). A snapshot is asked styled, so the host's colours show at
//! once; an older host's refusal is asked again plain (rule 1213).
//!
//! Closing the tab closes the host's session (`remote.close`, which asks as any close does); the quit and a dropped
//! link only detach (rule 288). Every wait here is on this worker: a dead link never holds the UI thread, a close or
//! the quit (rule 28).

use crate::remote_timing::LinkWatch;
use base64::Engine as _;
use flare_ctl::framing::Wire;
use flare_ctl::remote::{
    FILE_CHUNK, FILE_CHUNKS_MAX, FILE_MAX, FOLDER_DIRS_MAX, FOLDER_FILES_MAX, FOLDER_MAX, Intent,
    KeyIntent, MAX_INPUT_BYTES, MAX_INTENTS, MouseAct, MouseBtn, MouseIntent, OUTPUT_MAX,
    chunks_of, intent_weight, intents_json,
};
use flare_ctl::{
    Client, ClientError, CtlError, Method, RemoteHelloInfo, RemoteHelloParams, StdioWire, code,
};
use flare_tabs::tabs::Os;
use flare_term::handle::RemoteIntent;
use flare_term::handle::{RemoteMouse, RemoteMouseAct};
use flare_term::mouse_report::MouseButton;
use flare_term::pty::{PtyProcess, PtySpawner, SpawnSpec};
use flare_term::remote_pty::{
    Errand, ErrandFail, Fetched, Leaving, LinkState, RemoteIo, RemotePty, RemoteSpec, SignalStrip,
    Upload, UploadFolder, mouse_mode_bytes, plain_snapshot, styled_snapshot,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

// The registry is `crate::machines` (PR 5); the link reads it to build ssh's argv.
#[cfg(test)]
pub use crate::machines::parse_machines;
pub use crate::machines::{Machine, argv_word, machines_file, read_machines};

/// A variable a PC path may start with (rule 1207): a cmd login shell expands it on the host, so a candidate names the
/// user's own install without knowing the host's user name.
pub const PC_LOCAL_APP_DATA: &str = "%LOCALAPPDATA%";

/// The same folder in PowerShell's words (rule 1208). A PowerShell login shell drops the double quotes when it hands
/// `"%LOCALAPPDATA%\..."` to cmd, so cmd splits the expanded path at a space (`C:\Users\John Doe`); PowerShell expands
/// this one itself and quotes the result for cmd when it holds a space. A cmd login shell leaves it as text, which
/// names no file.
pub const PC_LOCAL_APP_DATA_PWSH: &str = "$env:LOCALAPPDATA";

/// Pure: a PC path without its leading variable, if it has one (rules 1207 and 1208).
fn pc_rest(flare: &str) -> &str {
    [PC_LOCAL_APP_DATA, PC_LOCAL_APP_DATA_PWSH]
        .iter()
        .find_map(|v| flare.strip_prefix(v))
        .unwrap_or(flare)
}

/// Pure: the command the host's login shell runs (rule 290: no token, no environment). A Mac's sh gets the path in
/// single quotes. A PC gets `cmd /c "<path>" --serve-remote` (rule 1207): sshd's DefaultShell there is cmd or, often,
/// PowerShell, which refuses a bare `"<path>" --serve-remote` and does not wait for the GUI-subsystem flare.exe with
/// `&`; `cmd /c` parses the same in both and waits. cmd keeps the quotes only when the path holds none of
/// `& < > ( ) @ ^ |`, so such a path is refused, and so is one with `" % ! $` or a backquote (expanded by cmd or by
/// PowerShell inside double quotes), except a leading `%LOCALAPPDATA%` or `$env:LOCALAPPDATA`, which the host's shell
/// expands. PowerShell also ends a double-quoted string at a curly quote (U+201C to U+201E), so every quote mark from
/// U+2018 to U+201F is refused too (rule 1208). None for a path that cannot be quoted safely there.
pub fn remote_command(kind: Option<Os>, flare: &str) -> Option<String> {
    if flare.is_empty() || flare.chars().any(char::is_control) {
        return None;
    }
    match kind {
        Some(Os::Pc) => {
            let rest = pc_rest(flare);
            if rest.contains([
                '"', '%', '!', '$', '`', '&', '<', '>', '(', ')', '@', '^', '|',
            ]) || rest.chars().any(|c| ('\u{2018}'..='\u{201F}').contains(&c))
            {
                return None;
            }
            Some(format!("cmd /c \"{flare}\" --serve-remote"))
        }
        _ => Some(format!("'{}' --serve-remote", flare.replace('\'', "'\\''"))),
    }
}

/// This machine's ssh: by its absolute path (rule 1).
pub(crate) fn ssh_exe() -> String {
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

/// How long ssh may take to reach a host (`ConnectTimeout`), so a host that is off never keeps Add waiting.
pub const CONNECT_TIMEOUT_S: u32 = 5;

/// Pure: an option value that is a file path, as ssh reads it: in double quotes when it holds a space (ssh splits an
/// option's value at a space; `path_word` already refused `"` and `%`).
fn ssh_path_value(path: &str) -> String {
    if path.contains(char::is_whitespace) {
        format!("\"{path}\"")
    } else {
        path.to_string()
    }
}

/// Pure: the options a machine's own fields add to every ssh it runs, the link's and the tunnel's (rule 1249): the
/// connect timeout; with Flare's key, that key only (`IdentitiesOnly`, `-i`); with Flare's known-hosts file, that file
/// only and strict checking (no question, no stranger trusted); the port. Each value is one argv item. None when a
/// field could be read as an option or break ssh's own parsing.
pub fn machine_opts(m: &Machine) -> Option<Vec<String>> {
    use crate::machines::path_word;
    // A user may hold a space (a Windows account, rule 1258): it is one argv item of its own after `-l`.
    if !argv_word(m.target())
        || m.user
            .as_deref()
            .is_some_and(|u| !crate::machines::user_word(u))
    {
        return None;
    }
    let mut a: Vec<String> = vec!["-o".into(), format!("ConnectTimeout={CONNECT_TIMEOUT_S}")];
    if let Some(k) = &m.key {
        if !path_word(k) {
            return None;
        }
        a.extend([
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-i".into(),
            k.clone(),
        ]);
    }
    if let Some(f) = &m.known_hosts {
        if !path_word(f) {
            return None;
        }
        a.extend([
            "-o".into(),
            format!("UserKnownHostsFile={}", ssh_path_value(f)),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
        ]);
    }
    if let Some(p) = m.port.filter(|p| *p > 0) {
        a.extend(["-p".into(), p.to_string()]);
    }
    Some(a)
}

/// Pure: the end of every ssh argv for a machine: its user, `--`, and where ssh connects (the address when there is
/// one, else the alias).
pub fn machine_dest(m: &Machine) -> Vec<String> {
    let mut a = Vec::new();
    if let Some(u) = &m.user {
        a.push("-l".into());
        a.push(u.clone());
    }
    a.push("--".into());
    a.push(m.target().to_string());
    a
}

/// Pure: ssh's argv for a machine, every part an item of its own: no shell on this side, no option smuggled in a
/// host name, no prompt (`BatchMode`), no terminal (`-T`), and the machine's own options (`machine_opts`).
pub fn ssh_argv(ssh: &str, m: &Machine) -> Option<Vec<String>> {
    let opts = machine_opts(m)?;
    let mut argv = vec![ssh.to_string(), "-o".into(), "BatchMode=yes".into()];
    argv.extend(opts);
    argv.push("-T".into());
    argv.extend(machine_dest(m));
    argv.push(remote_command(m.kind, &m.flare)?);
    Some(argv)
}

/// BLOCKING (one read of a file's access, rule 1277): the machine's key, when it is Flare's own, made private again
/// before ssh reads it; Windows' OpenSSH refuses a key that anyone else may read. Every real ssh a machine runs (a
/// tab's link, Settings > Machines' check, a Preview tunnel) calls it first, on its worker; the e2e's loopback runs no
/// ssh and skips it.
pub fn keep_key_private(m: &Machine) {
    if let Some(k) = &m.key {
        crate::pair_keys::keep_key_private(Path::new(k));
    }
}

/// BLOCKING: ssh's argv for a machine ([`ssh_argv`]), its key made private first ([`keep_key_private`]).
pub fn ssh_argv_ready(ssh: &str, m: &Machine) -> Option<Vec<String>> {
    keep_key_private(m);
    ssh_argv(ssh, m)
}

/// How long `remote.hello` may take once the link is up: the host may start its Flare hidden first and wait up to
/// `hidden_start::PIPE_WAIT` (60 s) for its channel (remote sessions PR 5), with time left for a slow login shell.
pub const HELLO_WAIT: Duration = Duration::from_secs(90);

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
        ssh_argv_ready(&ssh_exe(), &m)
            .ok_or_else(|| LinkError::Down(format!("cannot build the ssh command for {machine}")))
    }
}

/// The name this machine gives the host in `remote.hello`: the serve side's own rule (a Mac's host name when no
/// `HOSTNAME` is exported, rule 1199), else "viewer".
fn this_machine() -> String {
    Some(flare_ctl::session::default_machine_name())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "viewer".to_string())
}

/// BLOCKING (it starts the link and waits for `remote.hello`): the link `argv` names, said hello to. A worker's call.
fn start_link(argv: &[String]) -> Result<(ClientLink<StdioWire>, RemoteHelloInfo), LinkError> {
    let (exe, args) = argv
        .split_first()
        .ok_or_else(|| LinkError::Down("an empty link command".into()))?;
    let mut cmd = std::process::Command::new(exe); // sync-ok: the tab's link worker starts ssh, never the UI thread
    // ssh's stderr is kept (rule 1250): its last lines say why a link did not come up.
    cmd.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window, ever (tests never take the screen; nor does a link).
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| {
        let mark = if e.kind() == std::io::ErrorKind::NotFound {
            crate::ssh_problem::NO_CLIENT
        } else {
            ""
        };
        LinkError::Down(format!("{mark}could not start the link: {e}"))
    })?;
    let tail = child
        .stderr
        .take()
        .map(crate::ssh_problem::StderrTail::read);
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
    match hello_info(&mut link.client) {
        Ok(info) => Ok((link, info)),
        // A link that never came up says what ssh said: its last lines, once it ended (a moment at most).
        Err(LinkError::Down(m)) => {
            let said = tail.map(|t| t.text(STDERR_WAIT)).unwrap_or_default();
            drop(link);
            Err(LinkError::Down(if said.is_empty() {
                m
            } else {
                format!("{m}; {}{said}", crate::ssh_problem::SAID)
            }))
        }
        Err(e) => Err(e),
    }
}

/// How long a failed link waits for ssh's last words before it is ended.
const STDERR_WAIT: Duration = Duration::from_millis(1500);

impl Connector for SshConnector {
    fn connect(&self, machine: &str) -> Result<Box<dyn Link>, LinkError> {
        let (link, _) = start_link(&self.argv(machine)?)?;
        Ok(Box::new(link))
    }
}

/// BLOCKING: the link `argv` names (ssh, or the e2e's loopback) started, said hello to and let go: what the host
/// answered (Settings > Machines checks a link with it, remote sessions PR 7). The ssh child ends with the link.
pub fn hello_over(argv: &[String]) -> Result<RemoteHelloInfo, LinkError> {
    start_link(argv).map(|(_, info)| info)
}

/// `remote.hello` over a fresh client (the tests' links).
#[cfg(test)]
pub fn hello<W: Wire>(c: &mut Client<W>) -> Result<(), LinkError> {
    hello_info(c).map(|_| ())
}

/// `remote.hello` over a fresh client, and what the host answered.
pub fn hello_info<W: Wire>(c: &mut Client<W>) -> Result<RemoteHelloInfo, LinkError> {
    c.remote_hello(
        &RemoteHelloParams {
            machine: this_machine(),
            flare_version: crate::app_facts::version().to_string(),
        },
        HELLO_WAIT,
    )
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
        RemoteIntent::Mouse(m) => Intent::Mouse(wire_mouse(m)),
    }
}

/// The wire form of a mouse event (rule 1317).
fn wire_mouse(m: &RemoteMouse) -> MouseIntent {
    MouseIntent {
        act: match m.act {
            RemoteMouseAct::Press => MouseAct::Press,
            RemoteMouseAct::Release => MouseAct::Release,
            RemoteMouseAct::Motion => MouseAct::Motion,
            RemoteMouseAct::WheelUp => MouseAct::WheelUp,
            RemoteMouseAct::WheelDown => MouseAct::WheelDown,
        },
        button: m.button.map(|b| match b {
            MouseButton::Left => MouseBtn::Left,
            MouseButton::Middle => MouseBtn::Middle,
            MouseButton::Right => MouseBtn::Right,
        }),
        col: m.col,
        row: m.row,
        shift: m.mods.shift,
        alt: m.mods.alt,
        ctrl: m.mods.ctrl,
    }
}

/// The bytes an intent counts toward `MAX_INPUT_BYTES`, as the host counts them.
fn weight(i: &Intent) -> usize {
    intent_weight(i)
}

/// The host refused a call for its mouse intents: an older Flare that knows none (rule 1317) answers `bad_params`
/// naming the kind, and types nothing of the call.
fn refuses_mouse(e: &CtlError) -> bool {
    e.code == code::BAD_PARAMS && e.message.contains("no intent is called mouse")
}

/// The least time between two `remote.input` calls that carry only a pointer's motion (rule 1317): at most 25 a
/// second, half the host's 50 calls a second on a tab, so a dragged mouse never spends the typing's share.
pub const MOTION_GAP: Duration = Duration::from_millis(40);

/// Paces a remote tab's mouse motion (rule 1317): typing that is only a motion waits until [`MOTION_GAP`] after the
/// last motion sent, and a newer motion takes its place meanwhile. Anything else goes at once, with the motion held
/// before it, in order.
#[derive(Default)]
pub struct MotionPace {
    held: Option<RemoteIntent>,
    last: Option<Instant>,
}

fn is_motion(i: &RemoteIntent) -> bool {
    matches!(i, RemoteIntent::Mouse(m) if m.is_motion())
}

impl MotionPace {
    /// What to send now of `typed` (with the motion held before it, if any).
    pub fn pace(&mut self, typed: Vec<RemoteIntent>, now: Instant) -> Vec<RemoteIntent> {
        let mut all: Vec<RemoteIntent> = self.held.take().into_iter().collect();
        for i in typed {
            if is_motion(&i) && all.last().is_some_and(is_motion) {
                all.pop();
            }
            all.push(i);
        }
        let only_motion = !all.is_empty() && all.iter().all(is_motion);
        if only_motion
            && self
                .last
                .is_some_and(|t| now.saturating_duration_since(t) < MOTION_GAP)
        {
            self.held = all.pop();
            return Vec::new();
        }
        if all.iter().any(is_motion) {
            self.last = Some(now);
        }
        all
    }

    /// How long until the motion held may go, when one is held.
    pub fn due(&self, now: Instant) -> Option<Duration> {
        self.held.as_ref()?;
        let at = self.last? + MOTION_GAP;
        Some(at.saturating_duration_since(now))
    }

    /// The link went down: a motion held is dropped with it.
    pub fn clear(&mut self) {
        self.held = None;
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
/// refuses a call over either, and types nothing of it). A long text or paste is cut on character boundaries. A mouse
/// event over a cell the host would refuse (past `MAX_COLS` or `MAX_ROWS`) is left out, so it never costs the call.
pub fn input_batches(typed: &[RemoteIntent]) -> Vec<Vec<Intent>> {
    let mut all: Vec<Intent> = Vec::new();
    for i in typed {
        match wire_intent(i) {
            Intent::Text(t) => all.extend(pieces(&t).into_iter().map(Intent::Text)),
            Intent::Paste(p) => all.extend(pieces(&p).into_iter().map(Intent::Paste)),
            Intent::Mouse(m) if !m.in_bounds() => {}
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

// ---- files (remote sessions PR 6) ----

/// What a file errand's calls came to: the host's answer (`Ok`, the file or why not) or a broken link (`Err`).
type Answered<T> = Result<Result<T, ErrandFail>, LinkError>;

/// A refusal the person reads, or a broken link handed on.
fn refused<T>(e: LinkError) -> Answered<T> {
    match e {
        LinkError::Refused(r) => Ok(Err(ErrandFail::Failed(r.message))),
        down => Err(down),
    }
}

/// Fetches a host file by the first of `texts` the host has (rule 303), chunk by chunk, each chunk checked to be of
/// the same file. `NoSuchFile` when the host has none of them that it may send.
pub fn fetch_file(
    link: &mut dyn Link,
    tab: &str,
    texts: &[String],
    wait: Duration,
) -> Answered<Fetched> {
    for text in texts {
        let first = match link.call(
            Method::RemoteFetch,
            json!({"tab": tab, "path": text, "chunk": 0}),
            wait,
        ) {
            Ok(v) => v,
            Err(LinkError::Refused(r)) if r.code == code::NOT_FOUND => continue,
            Err(e) => return refused(e),
        };
        let size = first["size"].as_u64().unwrap_or(0);
        let chunks = first["chunks"].as_u64().unwrap_or(1);
        if size > FILE_MAX || chunks > u64::from(FILE_CHUNKS_MAX) {
            return Ok(Err(ErrandFail::Failed(
                "that file is too big to fetch".into(),
            )));
        }
        let name = first["name"].as_str().unwrap_or("file").to_string();
        let mut data = b64(&first);
        for c in 1..chunks {
            let v = match link.call(
                Method::RemoteFetch,
                json!({"tab": tab, "path": text, "chunk": c}),
                wait,
            ) {
                Ok(v) => v,
                Err(e) => return refused(e),
            };
            if v["size"].as_u64() != Some(size) || v["chunks"].as_u64() != Some(chunks) {
                return Ok(Err(ErrandFail::Failed(
                    "the file changed on the host while it was read".into(),
                )));
            }
            data.extend(b64(&v));
        }
        if data.len() as u64 != size {
            return Ok(Err(ErrandFail::Failed(
                "the file changed on the host while it was read".into(),
            )));
        }
        return Ok(Ok(Fetched { name, data }));
    }
    Ok(Err(ErrandFail::NoSuchFile))
}

/// Copies files to the tab's upload folder on the host, chunk by chunk (rule 304): the host paths, and every path as
/// the host quoted it for its shell, joined by a space (what is pasted).
pub fn put_files(
    link: &mut dyn Link,
    tab: &str,
    files: &[Upload],
    folders: &[UploadFolder],
    wait: Duration,
) -> Answered<(Vec<String>, String)> {
    if files.is_empty() && folders.is_empty() {
        return Ok(Err(ErrandFail::Failed("there is no file to copy".into())));
    }
    if let Err(why) = folders_fit(folders) {
        return Ok(Err(ErrandFail::Failed(why)));
    }
    let (mut paths, mut pastes) = (Vec::new(), Vec::new());
    // Rule 1215: whether the host still takes `exec`; an older one refuses it once, and this errand leaves it out.
    let mut exec_on = true;
    for f in files {
        if f.data.len() as u64 > FILE_MAX {
            return Ok(Err(ErrandFail::Failed(format!(
                "{} is over {} MB: copy it to the other machine another way",
                f.name,
                FILE_MAX / (1024 * 1024)
            ))));
        }
        let one = PutOne {
            under: None,
            name: &f.name,
            data: &f.data,
            exec: f.exec,
        };
        let last = match put_one(link, tab, &one, &mut exec_on, wait)? {
            Ok(v) => v,
            Err(why) => return Ok(Err(why)),
        };
        let (Some(p), Some(q)) = (last["host_path"].as_str(), last["paste"].as_str()) else {
            return Ok(Err(ErrandFail::Failed(
                "the host kept the file but named no path".into(),
            )));
        };
        paths.push(p.to_string());
        pastes.push(q.to_string());
    }
    for f in folders {
        match put_folder(link, tab, f, &mut exec_on, wait)? {
            Ok((p, q)) => {
                paths.push(p);
                pastes.push(q);
            }
            Err(why) => return Ok(Err(why)),
        }
    }
    Ok(Ok((paths, pastes.join(" "))))
}

/// Pure: whether dropped folders fit the caps together (rule 1213): at most `FOLDER_FILES_MAX` files, `FOLDER_MAX`
/// bytes and `FOLDER_DIRS_MAX` folders, each file at most `FILE_MAX`. The one line that says why not, otherwise.
pub fn folders_fit(folders: &[UploadFolder]) -> Result<(), String> {
    let files: usize = folders.iter().map(|f| f.files.len()).sum();
    let dirs: usize = folders.iter().map(|f| f.dirs.len() + 1).sum();
    let bytes: u64 = folders
        .iter()
        .flat_map(|f| f.files.iter().map(|x| x.size))
        .sum();
    let mb = |b: u64| b / (1024 * 1024);
    if files > FOLDER_FILES_MAX {
        return Err(format!(
            "the folder holds {files} files, over {FOLDER_FILES_MAX}: copy it to the other machine another way"
        ));
    }
    if bytes > FOLDER_MAX {
        return Err(format!(
            "the folder is {} MB, over {} MB: copy it to the other machine another way",
            mb(bytes),
            mb(FOLDER_MAX)
        ));
    }
    if dirs > FOLDER_DIRS_MAX {
        return Err(format!(
            "the folder holds over {FOLDER_DIRS_MAX} folders: copy it to the other machine another way"
        ));
    }
    if let Some(big) = folders
        .iter()
        .flat_map(|f| f.files.iter())
        .find(|x| x.size > FILE_MAX)
    {
        return Err(format!(
            "{} is over {} MB: copy the folder to the other machine another way",
            big.name,
            mb(FILE_MAX)
        ));
    }
    Ok(())
}

/// One file for [`put_one`]: the folder `under` names inside a dropped folder (none: the tab's upload folder), its
/// name, its bytes and whether it runs as a program on this machine (rule 1215).
struct PutOne<'a> {
    under: Option<&'a str>,
    name: &'a str,
    data: &'a [u8],
    exec: bool,
}

/// One file to the host in chunks (`remote.put`), into the tab's upload folder or the folder `under` names inside a
/// dropped folder: the host's answer to the last chunk. A runnable file's last chunk says `exec: true` (rule 1215); a
/// host that refuses the unknown param (`bad_params` naming it, an older Flare) takes the chunk again without it, and
/// `exec_on` goes false so the rest of the errand leaves it out.
fn put_one(
    link: &mut dyn Link,
    tab: &str,
    f: &PutOne<'_>,
    exec_on: &mut bool,
    wait: Duration,
) -> Answered<Value> {
    let enc = base64::engine::general_purpose::STANDARD;
    let data = f.data;
    let chunks = chunks_of(data.len() as u64);
    let mut name = f.name.to_string();
    let mut last = Value::Null;
    for c in 0..chunks {
        let from = (c as usize) * FILE_CHUNK;
        let piece = &data[from.min(data.len())..(from + FILE_CHUNK).min(data.len())];
        let mut p = json!({"tab": tab, "name": name, "chunk": c, "chunks": chunks, "data": enc.encode(piece)});
        if let Some(u) = f.under {
            p["under"] = json!(u);
        }
        let exec = f.exec && *exec_on && c + 1 == chunks;
        if exec {
            p["exec"] = json!(true);
        }
        let mut answer = link.call(Method::RemotePut, p.clone(), wait);
        if exec
            && let Err(LinkError::Refused(e)) = &answer
            && e.code == code::BAD_PARAMS
            && e.message.contains("exec")
        {
            log::info!("remote.put: an older host without exec, copying without it");
            *exec_on = false;
            if let Some(o) = p.as_object_mut() {
                o.remove("exec");
            }
            answer = link.call(Method::RemotePut, p, wait);
        }
        last = match answer {
            Ok(v) => v,
            Err(e) => return refused(e),
        };
        // The next chunks name the file as the host kept it.
        if let Some(n) = last["name"].as_str() {
            name = n.to_string();
        }
    }
    Ok(Ok(last))
}

/// One dropped folder to the host (rule 1213): the folder (`folder: true`), the folders in it, then each file, read
/// here as it goes. The folder's host path and that path quoted for the tab's shell. An older host that takes single
/// files only refuses the first call and nothing is copied.
fn put_folder(
    link: &mut dyn Link,
    tab: &str,
    f: &UploadFolder,
    exec_on: &mut bool,
    wait: Duration,
) -> Answered<(String, String)> {
    let top = match link.call(
        Method::RemotePut,
        json!({"tab": tab, "name": f.name, "folder": true}),
        wait,
    ) {
        Ok(v) => v,
        Err(LinkError::Refused(e))
            if e.code == code::BAD_PARAMS && e.message.contains("folder") =>
        {
            return Ok(Err(ErrandFail::Failed(
                "Flare on the other machine takes single files only: update it there to copy a folder"
                    .into(),
            )));
        }
        Err(e) => return refused(e),
    };
    let (Some(kept), Some(p), Some(q)) = (
        top["name"].as_str(),
        top["host_path"].as_str(),
        top["paste"].as_str(),
    ) else {
        return Ok(Err(ErrandFail::Failed(
            "the host made the folder but named no path".into(),
        )));
    };
    // Each folder's place on the host (`under`), by its parts here.
    let mut at: std::collections::HashMap<Vec<String>, String> = std::collections::HashMap::new();
    at.insert(Vec::new(), kept.to_string());
    for d in &f.dirs {
        let Some((name, parent)) = d.split_last() else {
            continue;
        };
        let Some(under) = at.get(parent).cloned() else {
            return Ok(Err(ErrandFail::Failed(
                "a folder came before the folder it is in".into(),
            )));
        };
        let v = match link.call(
            Method::RemotePut,
            json!({"tab": tab, "name": name, "folder": true, "under": under}),
            wait,
        ) {
            Ok(v) => v,
            Err(e) => return refused(e),
        };
        let made = v["name"].as_str().unwrap_or(name);
        at.insert(d.clone(), format!("{under}/{made}"));
    }
    for file in &f.files {
        let Some(under) = at.get(&file.dir).cloned() else {
            return Ok(Err(ErrandFail::Failed(format!(
                "{} is in a folder that was not copied",
                file.name
            ))));
        };
        // sync-ok: the tab's link worker reads a dropped folder's file as it sends it, never the UI thread
        let data = match std::fs::read(&file.path) {
            Ok(d) if d.len() as u64 <= FILE_MAX => d,
            Ok(_) => {
                return Ok(Err(ErrandFail::Failed(format!(
                    "{} grew over {} MB while it was copied",
                    file.name,
                    FILE_MAX / (1024 * 1024)
                ))));
            }
            Err(_) => {
                return Ok(Err(ErrandFail::Failed(format!(
                    "{} could not be read here",
                    file.name
                ))));
            }
        };
        let one = PutOne {
            under: Some(&under),
            name: &file.name,
            data: &data,
            exec: file.exec,
        };
        if let Err(why) = put_one(link, tab, &one, exec_on, wait)? {
            return Ok(Err(why));
        }
    }
    Ok(Ok((p.to_string(), q.to_string())))
}

// ---- the tab's worker ----

/// The waits of a tab's worker.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Between polls while output flows.
    pub poll_min: Duration,
    /// Between polls when idle.
    pub poll_max: Duration,
    /// Between polls while a key the host took waits for its echo (rule 1392).
    pub poll_fast: Duration,
    /// How long after typing the polls stay fast when no echo comes; zero: never fast.
    pub fast_for: Duration,
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
    /// Between two tries of an attach the host answered `unavailable` (a new tab's shell still starting, rule 1210).
    pub attach_retry: Duration,
    /// How long an attach is tried again on the same link before the link is dropped.
    pub attach_patience: Duration,
    /// The oldest typing kept while the link is down and sent after the next attach (rule 1210).
    pub typing_kept: Duration,
}

/// The most typing kept while the link is down, in the bytes `remote.input` counts (rule 1210).
pub const KEPT_TYPING_MAX: usize = 64 * 1024;

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll_min: Duration::from_millis(30),
            poll_max: Duration::from_millis(250),
            poll_fast: Duration::from_millis(3),
            fast_for: Duration::from_millis(100),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            call: Duration::from_secs(15),
            long_call: Duration::from_secs(60),
            end_call: Duration::from_secs(5),
            status_every: Duration::from_millis(1500),
            attach_retry: Duration::from_millis(250),
            attach_patience: Duration::from_secs(10),
            typing_kept: Duration::from_secs(30),
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
            convo: crate::remote_convo::ConvoMirror::new(),
            convo_next: None,
            convo_off: false,
            open_id_on: true,
            restarted: false,
            restart_line_at: None,
            kept: KeptTyping::default(),
            styled_on: true,
            mouse_on: true,
            pace: MotionPace::default(),
            watch: Arc::new(Mutex::new(LinkWatch::new(Instant::now()))),
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
    /// The host says the tab is gone (`no_tab`): the session asks `remote.find` once on the same link (rule 1307).
    /// Never leaves [`Worker::session`].
    Gone,
}

/// How long after the snapshot of a found session the restart line waits, at least, and then for a quiet output poll
/// (rule 1307).
const RESTART_LINE_QUIET: Duration = Duration::from_millis(500);

/// The line a remote tab shows when its session went on after the host's Flare restarted (rule 1307).
pub fn restarted_line(machine: &str) -> String {
    format!("Flare restarted on {machine}. The session goes on.")
}

/// The line a remote tab shows when its session ended on the host by itself, its shell exited (rule 1308).
pub fn ended_line(machine: &str) -> String {
    format!("This session was ended on {machine}.")
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
    /// The tab's conversation as the host numbered it (remote sessions PR 7, rule 297), polled only while the window
    /// wants it: when the next poll is due, and whether the host has no `remote.convo` (an older Flare).
    convo: crate::remote_convo::ConvoMirror,
    convo_next: Option<Instant>,
    convo_off: bool,
    /// Whether the host takes this tab's `remote.open` id (rule 1210; an older host refuses the param). The id itself
    /// is the tab's, kept in its live state and saved with it (rule 1307).
    open_id_on: bool,
    /// The host's Flare restarted and the tab found its session under a new id: the line is shown once the new
    /// snapshot is in and the host's screen went quiet, not before the time here (rule 1307).
    restarted: bool,
    restart_line_at: Option<Instant>,
    /// The typing kept while the link is down (rule 1210).
    kept: KeptTyping,
    /// Whether the host takes `remote.snapshot {styled}` (rule 1213; an older host refuses the param).
    styled_on: bool,
    /// Whether the host of this link takes mouse intents (rule 1317; an older host refuses the kind). Asked again on
    /// every new link, so a host that was updated meanwhile gets them.
    mouse_on: bool,
    /// The pace of the tab's mouse motion (rule 1317).
    pace: MotionPace,
    /// The link's timings and call budget (rule 1392), shared with the [`Watched`] link that records every call.
    watch: Arc<Mutex<LinkWatch>>,
}

/// How long the status and conversation polls wait at most while a key waits for its echo (rule 1392).
pub const PUT_OFF_MAX: Duration = Duration::from_millis(500);

/// A link whose every call is timed into the tab's [`LinkWatch`] (rule 1392).
struct Watched {
    link: Box<dyn Link>,
    watch: Arc<Mutex<LinkWatch>>,
}

fn watch_of(w: &Mutex<LinkWatch>) -> std::sync::MutexGuard<'_, LinkWatch> {
    w.lock().unwrap_or_else(|e| e.into_inner())
}

impl Link for Watched {
    fn call(
        &mut self,
        method: Method,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LinkError> {
        let t = Instant::now();
        let r = self.link.call(method, params, timeout);
        let now = Instant::now();
        watch_of(&self.watch).call(method, now.saturating_duration_since(t), now);
        r
    }
}

/// The most characters of one entry the fitted view shows (a long answer is cut, the view is for a glance).
pub const CONVO_LINE_MAX: usize = 4000;

/// Pure: the fitted view's lines of a mirrored conversation (remote sessions PR 7): who said it, the words (cut at
/// [`CONVO_LINE_MAX`] characters) and a tool's name.
pub fn convo_lines(m: &crate::remote_convo::ConvoMirror) -> Vec<flare_term::remote_pty::ConvoLine> {
    entry_lines(m.entries())
}

/// Pure: the fitted view's lines of numbered entries, as [`convo_lines`] makes them (the host's own view, rule 1211,
/// reads its entries the same way).
pub fn entry_lines(
    entries: &[(u64, flare_convo::said::Entry)],
) -> Vec<flare_term::remote_pty::ConvoLine> {
    use flare_convo::said::Role;
    entries
        .iter()
        .map(|(_, e)| flare_term::remote_pty::ConvoLine {
            role: match e.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            }
            .to_string(),
            text: e.text.chars().take(CONVO_LINE_MAX).collect(),
            tool: e.tool.clone(),
        })
        .collect()
}

/// Pure: `remote.open`'s params for a tab: the host folder, the session to resume there when one was asked for (the
/// host checks it again with `valid_resume`), and the tab's open id when the host takes one (rule 1210).
pub fn open_params(spec: &RemoteSpec, open_id: Option<&str>) -> Value {
    let mut p = json!({"folder": spec.folder});
    if let Some(r) = &spec.resume {
        p["resume"] = json!(r);
    }
    if let Some(o) = open_id {
        p["open_id"] = json!(o);
    }
    p
}

/// Pure: `remote.snapshot`'s params: the tab, and `styled` when the host takes it (rule 1213).
pub fn snapshot_params(tab: &str, styled: bool) -> Value {
    let mut p = json!({"tab": tab});
    if styled {
        p["styled"] = json!(true);
    }
    p
}

/// One typed thing kept while the link is down: when it was typed, and its weight toward [`KEPT_TYPING_MAX`].
type Kept = (Instant, RemoteIntent, usize);

/// The typing of a remote tab kept while its link is down (rule 1210), sent in order once the tab is attached again.
/// Typing older than the kept age is dropped, and so is what comes past [`KEPT_TYPING_MAX`]: once over it, the rest of
/// the outage's typing too, so what is sent has no hole in it. Either drop is told in the tab once.
#[derive(Default)]
pub struct KeptTyping {
    typed: std::collections::VecDeque<Kept>,
    bytes: usize,
    /// Over the cap since the last send: everything typed after is dropped.
    full: bool,
    /// Something was dropped since the last send.
    lost: bool,
}

impl KeptTyping {
    fn forget_old(&mut self, now: Instant, max_age: Duration) {
        while let Some((at, _, w)) = self.typed.front() {
            if now.saturating_duration_since(*at) <= max_age {
                break;
            }
            self.bytes -= w;
            self.typed.pop_front();
            self.lost = true;
        }
    }

    /// Keeps `intents`, typed at `at` (one time each, in order; now when missing).
    pub fn keep(
        &mut self,
        intents: Vec<RemoteIntent>,
        at: &[Instant],
        now: Instant,
        max_age: Duration,
    ) {
        self.forget_old(now, max_age);
        for (n, i) in intents.into_iter().enumerate() {
            // A mouse event is never kept (rule 1317): a click sent later would land on a screen that changed.
            if matches!(i, RemoteIntent::Mouse(_)) {
                continue;
            }
            let w = weight(&wire_intent(&i));
            if self.full || self.bytes + w > KEPT_TYPING_MAX {
                self.full = true;
                self.lost = true;
                continue;
            }
            self.bytes += w;
            self.typed
                .push_back((at.get(n).copied().unwrap_or(now), i, w));
        }
    }

    /// What is sent now, oldest first, and whether anything typed meanwhile was dropped. Empties the keep.
    pub fn take(&mut self, now: Instant, max_age: Duration) -> (Vec<RemoteIntent>, bool) {
        self.forget_old(now, max_age);
        let lost = self.lost;
        let typed = std::mem::take(&mut self.typed)
            .into_iter()
            .map(|(_, i, _)| i)
            .collect();
        *self = Self::default();
        (typed, lost)
    }

    /// The tab's host session is gone: nothing kept is ever typed into another tab.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
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
                // `Gone` never leaves the session; ended, as a `no_tab` would be, if it ever did.
                Step::Ended | Step::Gone => break,
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
        for e in live.end_errands() {
            e.fail(ErrandFail::Failed("this tab's session has ended".into()));
        }
    }

    /// File work asked while the link is down is told so at once, not held for a link that may take 30 s to return.
    fn refuse_errands(&self) {
        for e in self.spec.live.take_errands() {
            e.fail(ErrandFail::Failed(format!(
                "the link to {} is down; try again when it is back",
                self.spec.machine
            )));
        }
    }

    /// Waits `d` while the link is down (sizes and typing asked meanwhile are kept, rule 1210). False when the tab let
    /// go meanwhile: a close is still taken to the host, on a link of its own.
    fn wait(&mut self, d: Duration) -> bool {
        match self.hold(d, true) {
            Some(Leaving::Close) => {
                self.close_without_a_link();
                false
            }
            Some(Leaving::Detach) => false,
            None => true,
        }
    }

    /// Waits `d` with no live link: the last size asked is kept for the next attach, and the typing too (rule 1210).
    /// File work is refused at once when `refuse` (the link is down), else left for the polls. How the tab let go,
    /// when it did.
    fn hold(&mut self, d: Duration, refuse: bool) -> Option<Leaving> {
        let end = Instant::now() + d;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if refuse {
                self.refuse_errands();
            }
            let out = self
                .io
                .take_outgoing(left.min(self.t.poll_max.max(Duration::from_millis(1))));
            if let Some(s) = out.size {
                self.size = Some(s);
            }
            self.kept.keep(
                out.intents,
                &out.typed_at,
                Instant::now(),
                self.t.typing_kept,
            );
            if out.leaving.is_some() {
                return out.leaving;
            }
            if left.is_zero() {
                return None;
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

    /// The tab's host session is gone and nothing finds it: the tab ends as a shell that exits, and what was typed
    /// meanwhile is never typed into another tab.
    fn end_here(&mut self) -> Step {
        self.kept.clear();
        self.io.finish(None);
        Step::Ended
    }

    /// One life of the link: connect, open or attach, catch up, then the polls until it breaks or the tab goes. A
    /// `no_tab` asks `remote.find` once per life, on the same link (rule 1307).
    fn session(&mut self) -> Step {
        let mut link = match self.link_and_tab() {
            Ok(l) => l,
            Err(step) => return step,
        };
        let Some(mut tab) = self.spec.live.host_tab() else {
            return Step::Down;
        };
        let mut asked = false;
        loop {
            match self.follow(&mut *link, &tab) {
                Step::Gone if !asked => {
                    asked = true;
                    match self.find(&mut *link, &tab) {
                        Ok(t) => tab = t,
                        Err(step) => return step,
                    }
                }
                Step::Gone => return self.end_here(),
                step => return step,
            }
        }
    }

    /// Rule 1307: the host said the tab is gone. A tab that has its open id (and a host that takes one) asks the host
    /// once where its session is now:
    /// - `{tab}`: the host's Flare restarted and brought the session back under a new id. The tab follows it from a
    ///   fresh snapshot and says so in one line. The same id (only the follow was lost) goes on from the number read.
    /// - `no_tab`: the session ended on the host by itself (its shell exited, rule 1308). The tab stays, with one red
    ///   line, and its link ends.
    /// - `unknown_method` (an older host), or no open id (a tab saved before the key): the tab ends as before.
    /// - Any other refusal (`unavailable` while the host restores its tabs, `busy`, a timeout): asked again on this link
    ///   for up to `attach_patience`, then the link is tried again later. Never an end.
    ///
    /// The typing kept meanwhile is dropped in every case: it was typed for the lost tab.
    fn find(&mut self, link: &mut dyn Link, lost: &str) -> Result<String, Step> {
        self.kept.clear();
        let Some(open_id) = self.spec.live.open_id().filter(|_| self.open_id_on) else {
            return Err(self.end_here());
        };
        let end = Instant::now() + self.t.attach_patience;
        loop {
            match link.call(Method::RemoteFind, json!({"open_id": open_id}), self.t.call) {
                Ok(v) => {
                    let Some(found) = v["tab"].as_str().map(str::to_string) else {
                        return Err(Step::Down);
                    };
                    // Typing and a size asked during the find: the size is kept, the typing was for the lost tab.
                    let out = self.io.take_outgoing(Duration::ZERO);
                    if let Some(s) = out.size {
                        self.size = Some(s);
                    }
                    self.kept.clear();
                    if let Some(how) = out.leaving {
                        let m = match how {
                            Leaving::Close => Method::RemoteClose,
                            Leaving::Detach => Method::RemoteDetach,
                        };
                        let _ = link.call(m, json!({"tab": found}), self.t.end_call);
                        return Err(Step::Ended);
                    }
                    if found != lost {
                        log::info!("remote.find: the host's Flare restarted, the session goes on");
                        self.spec.live.set_host_tab(&found);
                        self.fresh = true;
                        self.restarted = true;
                        self.convo = crate::remote_convo::ConvoMirror::new();
                        self.convo_next = None;
                    }
                    return Ok(found);
                }
                Err(LinkError::Refused(e)) if e.code == code::NO_TAB => {
                    self.say(&ended_line(&self.spec.machine));
                    return Err(Step::Ended);
                }
                Err(LinkError::Refused(e)) if e.code == code::UNKNOWN_METHOD => {
                    log::info!("remote.find unknown to the host: the tab ends as before");
                    return Err(self.end_here());
                }
                Err(LinkError::Refused(e))
                    if e.code == code::UNAVAILABLE && Instant::now() < end =>
                {
                    log::info!("remote.find not yet ({}), asking again", e.message);
                    if self.hold(self.t.attach_retry, false).is_some() {
                        // The tab let go while its session was being looked for: nothing here to close.
                        return Err(Step::Ended);
                    }
                    self.kept.clear();
                }
                Err(LinkError::Refused(e)) => {
                    log::info!(
                        "remote.find refused ({}), the link tries again later",
                        e.code
                    );
                    return Err(Step::Down);
                }
                Err(LinkError::Down(_)) => return Err(Step::Down),
            }
        }
    }

    /// Connects, and opens the host tab when the tab has none yet. The link, or what the session comes to.
    fn link_and_tab(&mut self) -> Result<Box<dyn Link>, Step> {
        if let Some(how) = self.io.leaving() {
            if how == Leaving::Close {
                self.close_without_a_link();
            }
            return Err(Step::Ended);
        }
        let mut link: Box<dyn Link> = match self.connector.connect(&self.spec.machine) {
            Ok(l) => {
                watch_of(&self.watch).new_link(Instant::now());
                Box::new(Watched {
                    link: l,
                    watch: Arc::clone(&self.watch),
                })
            }
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
                return Err(Step::Down);
            }
        };
        if self.spec.live.host_tab().is_none() {
            let t = self.open(&mut *link)?;
            self.spec.live.set_host_tab(&t);
        }
        Ok(link)
    }

    /// Attaches to `tab`, catches up (a snapshot when fresh), sends the typing kept, then the polls until the link
    /// breaks, the tab goes, or the host says the tab is gone ([`Step::Gone`]).
    fn follow(&mut self, link: &mut dyn Link, tab: &str) -> Step {
        let live = self.spec.live.clone();
        // A new link asks again whether its host takes mouse intents; a motion held for the old one is gone (rule 1317).
        self.mouse_on = true;
        self.pace.clear();
        if let Some(step) = self.attach(link, tab) {
            return step;
        }
        // What the tab typed and sized while the link came up joins the typing kept meanwhile, so the age and the cap
        // hold for it too (rule 1210). A leave is still the polls' to take.
        let out = self.io.take_outgoing(Duration::ZERO);
        if let Some(s) = out.size {
            self.size = Some(s);
        }
        self.kept.keep(
            out.intents,
            &out.typed_at,
            Instant::now(),
            self.t.typing_kept,
        );
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
            self.snapshot(link, tab)
        } else {
            self.catch_up(link, tab)
        };
        if let Err(e) = caught {
            return self.failed(e);
        }
        self.fresh = false;
        // Rule 1307: said once the host's screen settled after the snapshot (the polls, `RESTART_LINE_QUIET`): a
        // Windows host's console can repaint the whole screen after the resize, which would wipe a line said at once.
        if std::mem::take(&mut self.restarted) {
            self.restart_line_at = Some(Instant::now() + RESTART_LINE_QUIET);
        }
        live.set_link(LinkState::Live);
        // The host's status is asked at once on every attach, and its first answer is always news (rule 294).
        self.status.attached();
        if let Err(e) = self.send_kept(link, tab) {
            return self.failed(e);
        }
        self.pump(link, tab)
    }

    /// `remote.open` for a tab with no host tab yet, with this tab's open id (rule 1210): an open whose answer a broken
    /// link lost goes again with the same id, and the host answers with the tab the first one opened. An older host
    /// refuses the unknown param (`bad_params` naming it): the open goes again without it, on this link, and later
    /// opens of this tab leave it out.
    fn open(&mut self, link: &mut dyn Link) -> Result<String, Step> {
        let open_id = self.open_id_on.then(|| self.spec.live.open_id_or_new());
        let id = open_id.as_deref();
        let mut answer = link.call(
            Method::RemoteOpen,
            open_params(&self.spec, id),
            self.t.long_call,
        );
        if id.is_some()
            && let Err(LinkError::Refused(e)) = &answer
            && e.code == code::BAD_PARAMS
            && e.message.contains("open_id")
        {
            log::info!("remote.open: an older host without open_id, opening without it");
            self.open_id_on = false;
            answer = link.call(
                Method::RemoteOpen,
                open_params(&self.spec, None),
                self.t.long_call,
            );
        }
        match answer {
            Ok(v) => v["tab"].as_str().map(str::to_string).ok_or(Step::Down),
            Err(LinkError::Refused(e)) => {
                self.say(&format!(
                    "Could not open {} on {}: {}",
                    self.spec.folder, self.spec.machine, e.message
                ));
                Err(Step::Ended)
            }
            Err(LinkError::Down(_)) => Err(Step::Down),
        }
    }

    /// `remote.attach`, tried again on the same link every `attach_retry` for up to `attach_patience` while the host
    /// answers `unavailable`: a tab opened a moment ago may not have its shell running yet (rule 1210). None once
    /// attached; else what the session comes to.
    fn attach(&mut self, link: &mut dyn Link, tab: &str) -> Option<Step> {
        let end = Instant::now() + self.t.attach_patience;
        loop {
            match link.call(Method::RemoteAttach, json!({"tab": tab}), self.t.call) {
                Ok(_) => return None,
                Err(LinkError::Refused(e)) if e.code == code::NO_TAB => {
                    // The host's tab is gone (ended there, or the host started again): the session looks for it once
                    // (rule 1307), and what was typed meanwhile is never typed into another tab.
                    self.kept.clear();
                    return Some(Step::Gone);
                }
                Err(LinkError::Refused(e))
                    if e.code == code::UNAVAILABLE && Instant::now() < end =>
                {
                    log::info!("remote.attach not yet ({}), trying again", e.message);
                    if let Some(how) = self.hold(self.t.attach_retry, false) {
                        if how == Leaving::Close {
                            let _ = link.call(
                                Method::RemoteClose,
                                json!({"tab": tab}),
                                self.t.end_call,
                            );
                        }
                        return Some(Step::Ended);
                    }
                }
                Err(LinkError::Refused(e)) => {
                    log::warn!("remote.attach refused: {}", e.message);
                    return Some(Step::Down);
                }
                Err(LinkError::Down(_)) => return Some(Step::Down),
            }
        }
    }

    /// The typing kept while the link was down, in the order typed, once the tab is attached again (rule 1210). What
    /// was too old or over the cap is told in the tab in one line. A call the link broke under loses its typing, as a
    /// live call does: it may have been typed on the host already.
    fn send_kept(&mut self, link: &mut dyn Link, tab: &str) -> Result<(), LinkError> {
        let (typed, lost) = self.kept.take(Instant::now(), self.t.typing_kept);
        if lost {
            self.say(&format!(
                "Some typing while the link to {} was down was not sent (older than {} s, or over {} KB).",
                self.spec.machine,
                self.t.typing_kept.as_secs(),
                KEPT_TYPING_MAX / 1024
            ));
        }
        for batch in input_batches(&typed) {
            match link.call(
                Method::RemoteInput,
                json!({"tab": tab, "intents": intents_json(&batch)}),
                self.t.call,
            ) {
                Ok(_) => {}
                Err(LinkError::Refused(e)) if e.code != code::NO_TAB => {
                    log::warn!("remote.input refused: {}", e.message);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// What a failed call means: the session gone on the host drops the typing kept for it and is looked for once
    /// (rule 1307, [`Step::Gone`]); anything else is a broken link.
    fn failed(&mut self, e: LinkError) -> Step {
        match e {
            LinkError::Refused(r) if r.code == code::NO_TAB => {
                self.kept.clear();
                Step::Gone
            }
            _ => Step::Down,
        }
    }

    /// The host's screen and history as text, and the number it holds (rule 289: text only, no mark rides on it).
    /// Rule 1213: the snapshot is asked `styled`, so the tab shows the host's colours at once; an older host refuses
    /// the param (`bad_params` naming it), and the snapshot goes again plain, on this link, and later ones of this tab
    /// leave it out. Only an answer that says `styled` keeps its SGRs.
    fn snapshot(&mut self, link: &mut dyn Link, tab: &str) -> Result<(), LinkError> {
        let mut v = link.call(
            Method::RemoteSnapshot,
            snapshot_params(tab, self.styled_on),
            self.t.long_call,
        );
        if self.styled_on
            && let Err(LinkError::Refused(e)) = &v
            && e.code == code::BAD_PARAMS
            && e.message.contains("styled")
        {
            log::info!("remote.snapshot: an older host without styled, asking plain");
            self.styled_on = false;
            v = link.call(
                Method::RemoteSnapshot,
                snapshot_params(tab, false),
                self.t.long_call,
            );
        }
        let v = v?;
        let seq = v["seq"]
            .as_u64()
            .ok_or_else(|| LinkError::Down("a snapshot with no seq".into()))?;
        let data = b64(&v);
        self.io
            .push_output(&if v["styled"].as_bool() == Some(true) {
                styled_snapshot(&data)
            } else {
                plain_snapshot(&data)
            });
        // Rule 1317: the mouse reports the host's program asked for, set here from the answer's word, so a click on a
        // restored or caught-up tab still reaches it. An older host names none: the modes here stay as they are.
        if let Some(word) = v["mouse"].as_str()
            && let Some(modes) = mouse_mode_bytes(word, v["sgr_mouse"].as_bool() == Some(true))
        {
            self.io.push_output(&modes);
        }
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

    /// The tab's conversation, every `status_every` while the window wants it (remote sessions PR 7): `remote.convo`
    /// from the last number held, each entry taken once (rule 297); a change goes to the window and wakes it. A host
    /// with no `remote.convo` is asked no more on this link; any other refusal (no session there yet) is asked again
    /// at the next poll.
    fn poll_convo(&mut self, link: &mut dyn Link, tab: &str) -> Result<(), LinkError> {
        let now = Instant::now();
        if self.convo_off
            || !self.spec.live.convo_wanted()
            || self.convo_next.is_some_and(|n| now < n)
        {
            return Ok(());
        }
        self.convo_next = Some(now + self.t.status_every);
        match link.call(
            Method::RemoteConvo,
            json!({"tab": tab, "since": self.convo.since()}),
            self.t.call,
        ) {
            Ok(v) => {
                if self.convo.apply(&v) {
                    self.spec.live.set_convo(convo_lines(&self.convo));
                    if let Some(w) = &self.wake {
                        w();
                    }
                }
                Ok(())
            }
            Err(LinkError::Refused(e)) if e.code == code::UNKNOWN_METHOD => {
                self.convo_off = true;
                Ok(())
            }
            Err(LinkError::Refused(e)) if e.code != code::NO_TAB => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// One piece of file work (remote sessions PR 6). The asker is always told; a broken link is handed back as the
    /// call's error, so the link is tried again as for any call.
    fn errand(&mut self, link: &mut dyn Link, tab: &str, e: Errand) -> Result<(), LinkError> {
        match e {
            Errand::Fetch { texts, done } => {
                match fetch_file(link, tab, &texts, self.t.long_call) {
                    Ok(f) => {
                        done(f);
                        Ok(())
                    }
                    Err(err) => {
                        done(Err(ErrandFail::Failed("the link broke".into())));
                        Err(err)
                    }
                }
            }
            Errand::Put {
                files,
                folders,
                done,
            } => match put_files(link, tab, &files, &folders, self.t.long_call) {
                Ok(Ok((paths, paste))) => {
                    // One paste of every path, quoted by the host for its shell (rules 1, 292, 304).
                    for batch in input_batches(&[RemoteIntent::Paste(paste)]) {
                        match link.call(
                            Method::RemoteInput,
                            json!({"tab": tab, "intents": intents_json(&batch)}),
                            self.t.call,
                        ) {
                            Ok(_) => {}
                            Err(LinkError::Refused(r)) => {
                                done(Err(ErrandFail::Failed(r.message.clone())));
                                return if r.code == code::NO_TAB {
                                    Err(LinkError::Refused(r))
                                } else {
                                    Ok(())
                                };
                            }
                            Err(err) => {
                                done(Err(ErrandFail::Failed("the link broke".into())));
                                return Err(err);
                            }
                        }
                    }
                    done(Ok(paths));
                    Ok(())
                }
                Ok(Err(why)) => {
                    done(Err(why));
                    Ok(())
                }
                Err(err) => {
                    done(Err(ErrandFail::Failed("the link broke".into())));
                    Err(err)
                }
            },
        }
    }

    /// The live polls: typing and sizes as they come, output in short polls, fast while a key waits for its echo
    /// (rule 1392).
    fn pump(&mut self, link: &mut dyn Link, tab: &str) -> Step {
        let mut idle = self.t.poll_min;
        // The slow polls' wait, kept apart from the fast one so a fast poll never shortens the idle back-off.
        let mut slow = self.t.poll_min;
        // Until when the polls are fast: a key the host took and whose echo has not come yet.
        let mut fast_until: Option<Instant> = None;
        // Since when the status and conversation polls wait for a key's echo.
        let mut put_off: Option<Instant> = None;
        // The next wait is a fast poll's.
        let mut fast_wait = false;
        loop {
            let out = if fast_wait {
                // A condvar's wait on Windows ends on the system's tick (about 15.6 ms), so a 3 ms wait would take five
                // times its length; a sleep keeps its time. Typing that comes meanwhile waits for it: 3 ms at most.
                std::thread::sleep(idle); // sync-ok: the tab's link worker, a few ms between fast polls
                self.io.take_outgoing(Duration::ZERO)
            } else {
                self.io.take_outgoing(idle)
            };
            fast_wait = false;
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
            // When the first key of this take was typed (a mouse event is no key: it has no echo to wait for).
            let key_at = out
                .intents
                .iter()
                .zip(&out.typed_at)
                .filter(|(i, _)| !matches!(i, RemoteIntent::Mouse(_)))
                .map(|(_, at)| *at)
                .min();
            let typed = self.pace.pace(out.intents, Instant::now());
            match self.send_typing(link, tab, typed, &mut busy) {
                Ok(true) => {
                    let now = Instant::now();
                    if !self.t.fast_for.is_zero() {
                        fast_until = Some(now + self.t.fast_for);
                    }
                    if let Some(at) = key_at {
                        watch_of(&self.watch).sent(at, now);
                    }
                }
                Ok(false) => {}
                Err(e) => return self.failed(e),
            }
            // File work (remote sessions PR 6) comes after the typing asked before it, so a dropped file's path is
            // pasted where the person was typing.
            let mut errands = self.spec.live.take_errands().into_iter();
            while let Some(e) = errands.next() {
                busy = true;
                if let Err(err) = self.errand(link, tab, e) {
                    for rest in errands {
                        rest.fail(ErrandFail::Failed("the link broke".into()));
                    }
                    return self.failed(err);
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
                    let got = !data.is_empty();
                    if got {
                        busy = true;
                        self.io.push_output(&self.strip.feed(&data, false));
                        // The key's echo (or what it made) is in: the polls go back to their pace.
                        fast_until = None;
                    }
                    watch_of(&self.watch).output(got, Instant::now());
                    self.spec.live.set_seq(v["to"].as_u64().unwrap_or(since));
                }
                Err(e) => return self.failed(e),
            }
            // Rule 1392: while a key waits for its echo, or typing waits to be sent, the status and conversation polls
            // wait (half a second at most), so typing never queues behind them. Asked before `due`, which spends the
            // slot.
            let now = Instant::now();
            let key_waits = fast_until.is_some_and(|u| now < u) || self.io.typing_waits();
            let hold_polls =
                key_waits && put_off.is_none_or(|t| now.saturating_duration_since(t) < PUT_OFF_MAX);
            if hold_polls {
                put_off.get_or_insert(now);
            } else {
                put_off = None;
            }
            if !hold_polls && let Err(e) = self.poll_status(link, tab) {
                return self.failed(e);
            }
            // A click or a key took the live terminal back on this screen (remote sessions PR 7): this tab's size goes
            // again, so the host's refits leave it to this window (rule 282, the last typist sets the size).
            if self.spec.live.take_retake()
                && let Some((cols, rows)) = self.size
                && let Err(e) = link.call(
                    Method::RemoteResize,
                    json!({"tab": tab, "cols": cols, "rows": rows}),
                    self.t.call,
                )
            {
                return self.failed(e);
            }
            if !hold_polls && let Err(e) = self.poll_convo(link, tab) {
                return self.failed(e);
            }
            // Rule 1307: the restart line, once the found session's screen is quiet.
            if !busy && self.restart_line_at.is_some_and(|at| Instant::now() >= at) {
                self.restart_line_at = None;
                self.io.push_output(
                    format!(
                        "\r\n\x1b[2m{}\x1b[0m\r\n",
                        restarted_line(&self.spec.machine)
                    )
                    .as_bytes(),
                );
            }
            slow = if busy {
                self.t.poll_min
            } else {
                next_backoff(slow, self.t.poll_max)
            };
            idle = slow;
            // Rule 1392: fast while a key waits for its echo, and only while the link's budget has room.
            let now = Instant::now();
            if fast_until.is_some_and(|u| now >= u) {
                fast_until = None;
            }
            if fast_until.is_some() {
                let mut w = watch_of(&self.watch);
                if w.fast_room(now) && self.t.poll_fast < idle {
                    w.fast_poll();
                    idle = self.t.poll_fast;
                    fast_wait = true;
                }
            }
            if let Some(fields) = watch_of(&self.watch).report(now) {
                flare_diag::writer::main().write("app", "remote-typing", fields);
            }
            // A motion held for its pace goes when its time comes, not at the next idle poll (rule 1317).
            if let Some(due) = self.pace.due(Instant::now()) {
                idle = idle.min(due.max(Duration::from_millis(1)));
            }
        }
    }

    /// The typing as `remote.input` calls. A host that knows no mouse intent (an older Flare, rule 1317) refuses the
    /// call whole: the call goes again without them, and this link sends none after. Any other refusal is logged and
    /// that call's typing dropped, as before; a broken link or a gone tab is handed back. True when the host took a
    /// call with a key, text or a paste in it: something whose echo the polls can wait for (rule 1392).
    fn send_typing(
        &mut self,
        link: &mut dyn Link,
        tab: &str,
        mut typed: Vec<RemoteIntent>,
        busy: &mut bool,
    ) -> Result<bool, LinkError> {
        let mut keyed = false;
        if !self.mouse_on {
            typed.retain(|i| !matches!(i, RemoteIntent::Mouse(_)));
        }
        for mut batch in input_batches(&typed) {
            if !self.mouse_on {
                batch.retain(|i| !matches!(i, Intent::Mouse(_)));
                if batch.is_empty() {
                    continue;
                }
            }
            *busy = true;
            loop {
                match link.call(
                    Method::RemoteInput,
                    json!({"tab": tab, "intents": intents_json(&batch)}),
                    self.t.call,
                ) {
                    Ok(_) => {
                        keyed |= batch.iter().any(|i| !matches!(i, Intent::Mouse(_)));
                        break;
                    }
                    Err(LinkError::Refused(e)) if self.mouse_on && refuses_mouse(&e) => {
                        log::info!("remote.input: an older host without mouse intents");
                        self.mouse_on = false;
                        batch.retain(|i| !matches!(i, Intent::Mouse(_)));
                        if batch.is_empty() {
                            break;
                        }
                    }
                    Err(LinkError::Refused(e)) if e.code != code::NO_TAB => {
                        log::warn!("remote.input refused: {}", e.message);
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(keyed)
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
