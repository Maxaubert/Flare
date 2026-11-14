//! Machine pairing PR 3 (rules 1242 to 1246): pairing two computers with a 6-digit code.
//!
//! Spec `docs/superpowers/specs/2026-11-14-machine-pairing-design.md` ("What happens underneath") and the approved
//! mockup (`2026-11-14-machine-pairing-mockup.html`, "Add a machine", "The code", "Linked"):
//!
//! - **The host** (Computer sharing on): announces itself on the network as `_flare-pair._tcp` (mDNS; TXT `name`, `os`,
//!   `version`, `proto`, no secret) and listens on a random TCP port, the one the announcement names. A viewer that
//!   connects and says hello makes the host show the Flare dialog "<viewer> wants to connect" with a fresh 6-digit code
//!   and Decline, wherever the person is in Flare. One request at a time: another connection meanwhile is told `busy`;
//!   after three wrong codes the window closes, and after any request that ended with a wrong code and no pairing
//!   (rule 1247), no request is taken for [`LOCKOUT`].
//! - **The viewer**: while the Add a machine dialog is open it looks for those announcements; they are the drop-down's
//!   first choices. Picking one connects and says hello; the dialog asks for the "Code shown on <name>". Add runs the
//!   SPAKE2 exchange (`pair_wire`). A wrong code is the red line "Wrong code".
//! - **Over the confirmed channel** the viewer sends Flare's own public key (`pair_keys`, made on first need) and its
//!   name; the host adds it to the right `authorized_keys` (rule 1244) and answers with its SSH host key, its user, its
//!   Flare path, its system and version. The viewer pins the host key in Flare's own known-hosts file, saves the machine
//!   with its address, user, key and known-hosts file (machine pairing PR 1's fields), and checks the link as usual. The
//!   host keeps the viewer in its own list of computers that reach it (`machines_in.json`), so both show the other in
//!   Linked machines.
//!
//! Every network and disk call runs on a worker (rule 34) and answers with a [`PairMsg`]. The network is behind
//! [`PairNet`], so the tests pair two pages over in-memory pipes and never open a socket.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use flare_tabs::tabs::Os;
use serde_json::{Value, json};

use crate::appctl::{Post, UiMsg};
use crate::machines::{Incoming, Machine, argv_word, machine_id};
use crate::machines_page::{MachinesIo, MachinesMsg};
use crate::pair_keys::{HOST_KEY_TYPES, PairFiles, authorized_line, parse_public_key};
use crate::pair_wire::{self as wire, AttemptEnd, HostEnd, PROTOCOL};
use crate::settings_ctl::SettingsMsg;
use crate::sharing::SharingOs;

/// The mDNS service type a computer with Computer sharing on announces.
pub const SERVICE: &str = "_flare-pair._tcp.local.";
/// How long a host takes no request after three wrong codes.
pub const LOCKOUT: Duration = Duration::from_secs(30);
/// How long either end waits for the other: the person reads the code and types it in this time.
pub const WINDOW: Duration = Duration::from_secs(300);

/// What a computer says about itself, in its hello and its announcement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Me {
    pub name: String,
    pub os: Option<Os>,
    pub version: String,
}

/// The word for a system on the wire and in the announcement.
pub fn os_word(os: Option<Os>) -> &'static str {
    match os {
        Some(Os::Mac) => "mac",
        Some(Os::Pc) => "pc",
        None => "",
    }
}

impl Me {
    /// This computer: its name (a Mac's `.local` left off), its system and Flare's version.
    pub fn this_computer() -> Self {
        let name = flare_ctl::session::default_machine_name();
        let name = name.trim_end_matches(".local").to_string();
        Self {
            name: if name.is_empty() {
                "Computer".into()
            } else {
                name
            },
            os: if cfg!(target_os = "macos") {
                Some(Os::Mac)
            } else if cfg!(windows) {
                Some(Os::Pc)
            } else {
                None
            },
            version: crate::app_facts::version().to_string(),
        }
    }
}

/// A computer with Computer sharing on, found on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nearby {
    /// Its announcement's full name: the drop-down's id after `NEARBY_PREFIX`.
    pub id: String,
    pub name: String,
    pub os: Option<Os>,
    pub version: String,
    /// Its addresses, IPv4 first.
    pub addrs: Vec<IpAddr>,
    /// Its pairing port.
    pub port: u16,
}

/// Pure: a nearby computer from an announcement's parts (its TXT `name`, `os`, `version`). None when it speaks another
/// pairing protocol or names no port.
pub fn nearby_of(
    fullname: &str,
    txt: &dyn Fn(&str) -> Option<String>,
    mut addrs: Vec<IpAddr>,
    port: u16,
) -> Option<Nearby> {
    if port == 0 || txt("proto").as_deref() != Some("1") {
        return None;
    }
    let instance = fullname
        .strip_suffix(SERVICE)
        .map(|s| s.trim_end_matches('.'))
        .unwrap_or(fullname);
    let name = txt("name")
        .map(|n| clean_name(&n))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| clean_name(instance));
    addrs.sort_by_key(|a| (a.is_ipv6(), a.to_string()));
    addrs.retain(|a| !a.is_loopback());
    Some(Nearby {
        id: fullname.to_string(),
        name,
        os: txt("os").as_deref().and_then(Os::parse),
        version: txt("version").map(|v| clean_name(&v)).unwrap_or_default(),
        addrs,
        port,
    })
}

/// Pure: a name from the other computer, as Flare shows it: no control character, 60 characters at most, trimmed.
pub fn clean_name(s: &str) -> String {
    clean_upto(s, 60)
}

/// Pure: a line from the other computer (why it could not add this one): no control character, 200 at most.
pub fn clean_line(s: &str) -> String {
    clean_upto(s, 200)
}

fn clean_upto(s: &str, n: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(n)
        .collect::<String>()
        .trim()
        .to_string()
}

// ---- the network, behind a trait --------------------------------------------------------------------------------

/// Ends a connection from another thread: a read that waits there returns.
pub type Closer = Box<dyn Fn() + Send + Sync>;

/// One connection: its bytes both ways, and a way to end it from another thread.
pub trait Duplex: Read + Write + Send {
    fn closer(&self) -> Closer;
}

impl Duplex for std::net::TcpStream {
    fn closer(&self) -> Closer {
        match self.try_clone() {
            Ok(s) => Box::new(move || {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }),
            Err(_) => Box::new(|| {}),
        }
    }
}

/// The host's listener.
pub trait PairListener: Send {
    fn port(&self) -> u16;
    /// A connection and the address it came from, or None after waiting `wait`.
    fn accept(&self, wait: Duration) -> Option<(Box<dyn Duplex>, String)>;
}

/// The network calls of pairing. BLOCKING where noted: a worker's.
pub trait PairNet: Send + Sync {
    /// Starts announcing `me` with the pairing `port`, or stops (None).
    fn announce(&self, me: Option<(&Me, u16)>);
    /// Starts looking for announcements (each change calls `found` with them all, on a worker), or stops.
    fn browse(&self, on: bool, found: Arc<dyn Fn(Vec<Nearby>) + Send + Sync>);
    /// A listener on a random port of the local network.
    fn listen(&self) -> Result<Box<dyn PairListener>, String>;
    /// BLOCKING: a connection to a nearby computer's pairing port, and the address that answered.
    fn connect(&self, to: &Nearby) -> Result<(Box<dyn Duplex>, String), String>;
}

/// No pairing (scenes, `--e2e`, tests that never pair): nothing is announced or found, and no port is opened.
pub struct NoPairNet;

impl PairNet for NoPairNet {
    fn announce(&self, _me: Option<(&Me, u16)>) {}
    fn browse(&self, _on: bool, _found: Arc<dyn Fn(Vec<Nearby>) + Send + Sync>) {}
    fn listen(&self) -> Result<Box<dyn PairListener>, String> {
        Err("no pairing here".into())
    }
    fn connect(&self, _to: &Nearby) -> Result<(Box<dyn Duplex>, String), String> {
        Err("no pairing here".into())
    }
}

// ---- the host's side of one connection --------------------------------------------------------------------------

/// What a host session tells the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// Show "<viewer> wants to connect" with this code.
    Ask {
        id: u64,
        viewer: String,
        code: String,
    },
    /// A wrong code; `left` tries remain.
    Strike { id: u64, left: u32 },
    /// The request ended: `linked` with the computers that reach this one now (and whether the key went into the
    /// admin file), or not.
    Done {
        id: u64,
        linked: Option<(Vec<Incoming>, bool)>,
        too_many: bool,
    },
}

/// What a host session needs.
pub struct HostCtx {
    pub me: Me,
    pub os: Arc<dyn SharingOs>,
    pub io: Arc<dyn MachinesIo>,
    /// Flare's own program here, which the viewer runs over ssh.
    pub flare: String,
}

/// BLOCKING: one request on the host (rules 1243 and 1244), from the viewer's hello to the end. `events` reaches the
/// UI thread.
pub fn host_session(
    s: &mut dyn Duplex,
    peer: &str,
    ctx: &HostCtx,
    id: u64,
    events: &dyn Fn(HostEvent),
) {
    let done = |linked, too_many| {
        events(HostEvent::Done {
            id,
            linked,
            too_many,
        })
    };
    let Ok(hello) = wire::recv(s) else {
        return done(None, false);
    };
    if wire::kind(&hello) != "hello" {
        return done(None, false);
    }
    if hello.get("proto").and_then(Value::as_u64) != Some(PROTOCOL) {
        let _ = wire::send(s, &json!({"t": "proto", "proto": PROTOCOL}));
        return done(None, false);
    }
    let viewer = clean_name(
        hello
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let viewer = if viewer.is_empty() {
        "A computer".to_string()
    } else {
        viewer
    };
    let Ok(code) = wire::fresh_code() else {
        return done(None, false);
    };
    events(HostEvent::Ask {
        id,
        viewer: viewer.clone(),
        code: code.clone(),
    });
    if wire::send(s, &json!({"t": "shown"})).is_err() {
        return done(None, false);
    }
    let mut ch = match wire::host_attempts(s, &code, |left| events(HostEvent::Strike { id, left }))
    {
        Ok(ch) => ch,
        Err(HostEnd::TooMany) => return done(None, true),
        Err(HostEnd::Gone(why)) => {
            log::info!("pairing: the request from {peer} ended: {why}");
            return done(None, false);
        }
    };
    let refuse = |ch: &mut wire::Channel, s: &mut dyn Duplex, line: &str| {
        let _ = ch.send(s, &json!({"ok": false, "error": line}));
    };
    let Ok(said) = ch.recv(s) else {
        return done(None, false);
    };
    let key = said.get("key").and_then(Value::as_str).unwrap_or_default();
    let name = clean_name(said.get("name").and_then(Value::as_str).unwrap_or_default());
    let Some(line) = authorized_line(key, &name) else {
        refuse(&mut ch, s, "The key that came is not one Flare takes.");
        return done(None, false);
    };
    let admin = match crate::sharing::install_key(&*ctx.os, &line) {
        Ok(admin) => admin,
        Err(why) => {
            refuse(&mut ch, s, &why);
            return done(None, false);
        }
    };
    let Some(host_key) = ctx
        .os
        .ssh_host_key()
        .ok()
        .and_then(|k| parse_public_key(k.trim(), &HOST_KEY_TYPES))
    else {
        refuse(&mut ch, s, "The SSH server's key could not be read there.");
        return done(None, false);
    };
    let reply = json!({
        "ok": true,
        "host_key": host_key.line(),
        "user": ctx.os.user_name(),
        "flare": ctx.flare,
        "os": os_word(ctx.me.os),
        "name": ctx.me.name,
        "version": ctx.me.version,
        "port": crate::sharing::SSH_PORT,
    });
    if ch.send(s, &reply).is_err() {
        return done(None, false);
    }
    // This computer keeps the viewer among the computers that reach it (the same key again replaces its entry).
    let key_field = parse_public_key(key, &[crate::pair_keys::ED25519])
        .map(|k| k.key)
        .unwrap_or_default();
    let mut list = ctx.io.list_in();
    let old = list.iter().position(|m| m.key == key_field);
    let taken: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
    let entry = Incoming {
        id: old
            .map(|i| list[i].id.clone())
            .unwrap_or_else(|| machine_id(&name, &taken)),
        name: if name.is_empty() { viewer } else { name },
        kind: said.get("os").and_then(Value::as_str).and_then(Os::parse),
        address: peer.to_string(),
        version: said
            .get("version")
            .and_then(Value::as_str)
            .map(clean_name)
            .unwrap_or_default(),
        key: key_field,
    };
    match old {
        Some(i) => list[i] = entry,
        None => list.push(entry),
    }
    if let Err(e) = ctx.io.save_in(&list) {
        log::warn!("pairing: {e}");
    }
    done(Some((list, admin)), false)
}

// ---- the viewer's side of one connection ------------------------------------------------------------------------

/// What a viewer session tells the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewerEvent {
    /// The host shows its code: the code can be typed.
    Shown,
    /// The code was wrong; tries remain.
    Wrong,
    /// Paired: the saved list and the new machine's id.
    Linked { list: Vec<Machine>, id: String },
    /// It ended: the dialog's red line.
    Failed(String),
}

/// What a viewer session needs.
pub struct ViewerCtx {
    pub me: Me,
    pub io: Arc<dyn MachinesIo>,
    pub files: PairFiles,
}

/// Pure: a Flare path the host named, kept only when ssh can carry it as one word of the remote command.
pub fn flare_path_ok(p: &str) -> bool {
    !p.trim().is_empty()
        && p.len() <= 1024
        && !p.chars().any(|c| c.is_control() || c == '"' || c == '\'')
}

/// BLOCKING: one pairing on the viewer (rules 1243 and 1245), from the hello to the saved machine. The typed codes come
/// through `codes` (a closed channel is Cancel); `events` reaches the UI thread.
pub fn viewer_session(
    s: &mut dyn Duplex,
    address: &str,
    to: &Nearby,
    ctx: &ViewerCtx,
    codes: &Receiver<String>,
    events: &dyn Fn(ViewerEvent),
) {
    let name = to.name.as_str();
    let fail = |line: String| events(ViewerEvent::Failed(line));
    let hello = json!({
        "t": "hello",
        "proto": PROTOCOL,
        "name": ctx.me.name,
        "os": os_word(ctx.me.os),
        "version": ctx.me.version,
    });
    let answer = wire::send(s, &hello).and_then(|()| wire::recv(s));
    match answer.as_ref().map(wire::kind) {
        Ok("shown") => events(ViewerEvent::Shown),
        Ok("busy") => {
            return fail(format!(
                "{name} is busy with another request. Try again in a minute."
            ));
        }
        Ok("proto") => return fail("Update Flare on both computers, then try again.".into()),
        Ok(other) => {
            log::info!("pairing: {name} said {other}");
            return fail("Not answering".into());
        }
        Err(e) => {
            log::info!("pairing: {name} did not answer: {e}");
            return fail("Not answering".into());
        }
    }
    let mut ch = loop {
        let Ok(code) = codes.recv() else {
            // sync-ok: the viewer's worker waits for the typed code
            return;
        };
        match wire::viewer_attempt(s, &code) {
            Ok(ch) => break ch,
            Err(AttemptEnd::Wrong { left }) if left > 0 => events(ViewerEvent::Wrong),
            Err(AttemptEnd::Wrong { .. }) => {
                return fail(format!(
                    "Wrong code three times. Pick {name} again for a new code."
                ));
            }
            Err(AttemptEnd::Unproved) => {
                return fail(format!("{name} could not prove the code. Try again."));
            }
            Err(AttemptEnd::Gone(why)) => {
                log::info!("pairing: {name} ended the request: {why}");
                return fail(format!("{name} declined."));
            }
        }
    };
    let public = match ctx.files.ensure_key(&ctx.me.name) {
        Ok(k) => k,
        Err(e) => return fail(e),
    };
    let said = json!({
        "key": public,
        "name": ctx.me.name,
        "os": os_word(ctx.me.os),
        "version": ctx.me.version,
    });
    let reply = ch.send(s, &said).and_then(|()| ch.recv(s));
    let reply = match reply {
        Ok(r) => r,
        Err(e) => {
            log::info!("pairing: no answer from {name}: {e}");
            return fail(format!("{name} stopped answering."));
        }
    };
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        let why = reply
            .get("error")
            .and_then(Value::as_str)
            .map(clean_line)
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| format!("{name} could not add this computer."));
        return fail(why);
    }
    let field = |k: &str| reply.get(k).and_then(Value::as_str).unwrap_or_default();
    let Some(host_key) = parse_public_key(field("host_key"), &HOST_KEY_TYPES) else {
        return fail(format!("{name} sent no SSH key Flare can pin."));
    };
    let user = field("user");
    let flare = field("flare");
    if !argv_word(user) || !flare_path_ok(flare) || !argv_word(address) {
        return fail(format!("{name} answered in a way Flare cannot use."));
    }
    let port = reply
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p > 0)
        .unwrap_or(22);
    let known = match ctx.files.pin(address, port, &host_key) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let host_name = clean_name(field("name"));
    let machine_name = if host_name.is_empty() {
        name.to_string()
    } else {
        host_name
    };
    let mut list = ctx.io.list();
    // Pairing a machine again keeps its id (and its tabs).
    let old = list
        .iter()
        .position(|m| m.host.as_deref() == Some(address) || m.name == machine_name);
    let taken: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
    let id = old
        .map(|i| list[i].id.clone())
        .unwrap_or_else(|| machine_id(&machine_name, &taken));
    let machine = Machine {
        id: id.clone(),
        name: machine_name,
        kind: Os::parse(field("os")).or(to.os),
        ssh: address.to_string(),
        user: Some(user.to_string()),
        flare: flare.to_string(),
        host: Some(address.to_string()),
        port: (port != 22).then_some(port),
        key: Some(ctx.files.key_path().to_string_lossy().into_owned()),
        known_hosts: Some(known.to_string_lossy().into_owned()),
    };
    match old {
        Some(i) => list[i] = machine,
        None => list.push(machine),
    }
    if let Err(e) = ctx.io.save(&list) {
        return fail(e);
    }
    events(ViewerEvent::Linked { list, id });
}

// ---- the UI thread's state ----------------------------------------------------------------------------------------

/// A worker's answer for pairing.
#[derive(Debug)]
pub enum PairMsg {
    /// The nearby computers now.
    Nearby(Vec<Nearby>),
    /// The viewer run `run` said something.
    Viewer { run: u64, event: ViewerEvent },
    /// A host request said something.
    Host(HostEvent),
}

/// The code dialog on the host: who asks, and the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub id: u64,
    pub viewer: String,
    pub code: String,
}

/// The words of the host's code dialog (mockup, "The code").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptWords {
    pub title: String,
    pub code: String,
    pub line: String,
}

/// Pure: what the host's code dialog says.
pub fn prompt_words(p: &Prompt) -> PromptWords {
    PromptWords {
        title: format!("{} wants to connect", p.viewer),
        code: wire::code_shown(&p.code),
        line: format!("Enter this code on {}", p.viewer),
    }
}

/// The viewer's request that runs now.
struct ViewerRun {
    run: u64,
    to: Nearby,
    codes: Sender<String>,
    /// Cancelled, and how to end the connection once there is one.
    close: Arc<Mutex<(bool, Option<Closer>)>>,
}

/// Pure (rule 1247): whether a request that ended locks the host for [`LOCKOUT`]: three wrong codes, or any wrong code
/// in a request that ended without pairing. Without the second, a stranger could guess once or twice, hang up and ask
/// again for a fresh code at once, with no limit on its guesses.
pub fn locks_out(too_many: bool, linked: bool, struck: bool) -> bool {
    too_many || (struck && !linked)
}

/// The host's listener and its requests.
struct HostRun {
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<HostShared>>,
}

#[derive(Default)]
struct HostShared {
    /// The request that shows its code now, and how to end it.
    active: Option<(u64, Closer)>,
    /// No request until then (three wrong codes).
    locked_until: Option<Instant>,
    next: u64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `f` on a named worker; a failure to start is logged.
fn worker(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        log::warn!("could not start {name}: {e}");
    }
}

/// Pairing's state on the UI thread: the nearby computers and the viewer's request (the Add a machine dialog), the
/// host's listener and announcement (while Computer sharing is on), and the host's code dialog.
pub struct Pairing {
    net: Arc<dyn PairNet>,
    os: Arc<dyn SharingOs>,
    files: Option<PairFiles>,
    me: Me,
    post: Post,
    nearby: Vec<Nearby>,
    browsing: bool,
    /// The browse's number: answers from a browse that ended are dropped.
    browse_gen: Arc<AtomicU64>,
    viewer: Option<ViewerRun>,
    runs: u64,
    host: Option<HostRun>,
    prompt: Option<Prompt>,
}

impl Pairing {
    /// Pairing over `net`, keeping Flare's key and known hosts in `files` (None: no pairing at all).
    pub fn new(
        net: Arc<dyn PairNet>,
        os: Arc<dyn SharingOs>,
        files: Option<PairFiles>,
        me: Me,
        post: Post,
    ) -> Self {
        Self {
            net,
            os,
            files,
            me,
            post,
            nearby: Vec::new(),
            browsing: false,
            browse_gen: Arc::new(AtomicU64::new(0)),
            viewer: None,
            runs: 0,
            host: None,
            prompt: None,
        }
    }

    /// The same pairing with the switch's OS calls.
    pub fn set_os(&mut self, os: Arc<dyn SharingOs>) {
        self.os = os;
    }

    fn say(post: &Post, m: PairMsg) {
        post(UiMsg::Settings(SettingsMsg::Machines(MachinesMsg::Pair(m))));
    }

    pub fn nearby(&self) -> &[Nearby] {
        &self.nearby
    }

    pub fn nearby_by_id(&self, id: &str) -> Option<&Nearby> {
        self.nearby.iter().find(|n| n.id == id)
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// The Add a machine dialog opened (start looking) or closed (stop).
    pub fn browse(&mut self, on: bool) {
        if on == self.browsing || self.files.is_none() {
            return;
        }
        self.browsing = on;
        let n = self.browse_gen.fetch_add(1, Ordering::SeqCst) + 1;
        if !on {
            self.nearby.clear();
            self.net.browse(false, Arc::new(|_| {}));
            return;
        }
        let (post, generation) = (Arc::clone(&self.post), Arc::clone(&self.browse_gen));
        self.net.browse(
            true,
            Arc::new(move |list| {
                if generation.load(Ordering::SeqCst) == n {
                    Self::say(&post, PairMsg::Nearby(list));
                }
            }),
        );
    }

    /// A nearby computer was picked: connect and say hello (the host shows its code). An older request ends.
    pub fn start_viewer(&mut self, id: &str, io: Arc<dyn MachinesIo>) -> bool {
        self.end_viewer();
        let (Some(to), Some(files)) = (self.nearby_by_id(id).cloned(), self.files.clone()) else {
            return false;
        };
        self.runs += 1;
        let run = self.runs;
        let (tx, rx) = channel::<String>();
        let close: Arc<Mutex<(bool, Option<Closer>)>> = Arc::default();
        self.viewer = Some(ViewerRun {
            run,
            to: to.clone(),
            codes: tx,
            close: Arc::clone(&close),
        });
        let (net, post) = (Arc::clone(&self.net), Arc::clone(&self.post));
        let ctx = ViewerCtx {
            me: self.me.clone(),
            io,
            files,
        };
        worker("flare-pair-viewer", move || {
            let events = |event| Self::say(&post, PairMsg::Viewer { run, event });
            let (mut s, address) = match net.connect(&to) {
                Ok(c) => c,
                Err(e) => {
                    log::info!("pairing: could not reach {}: {e}", to.name);
                    return events(ViewerEvent::Failed("Not answering".into()));
                }
            };
            {
                let mut c = lock(&close);
                // Cancelled while it connected: the connection ends here.
                if c.0 {
                    return;
                }
                c.1 = Some(s.closer());
            }
            viewer_session(&mut *s, &address, &to, &ctx, &rx, &events);
        });
        true
    }

    /// The viewer's request that runs now: its number and the computer.
    pub fn viewer_to(&self) -> Option<(u64, &Nearby)> {
        self.viewer.as_ref().map(|v| (v.run, &v.to))
    }

    /// Add: the typed code goes to the request. False when no request waits for it.
    pub fn send_code(&mut self, digits: String) -> bool {
        self.viewer
            .as_ref()
            .is_some_and(|v| v.codes.send(digits).is_ok())
    }

    /// Ends the viewer's request (Cancel, another pick, the dialog closing): the host's dialog closes too.
    pub fn end_viewer(&mut self) {
        if let Some(v) = self.viewer.take() {
            let mut c = lock(&v.close);
            c.0 = true;
            if let Some(close) = c.1.take() {
                close();
            }
        }
    }

    /// A viewer run ended by itself (paired or failed): it is forgotten when it is the one running.
    pub fn viewer_ended(&mut self, run: u64) {
        if self.viewer.as_ref().is_some_and(|v| v.run == run) {
            self.viewer = None;
        }
    }

    /// Computer sharing is on (listen and announce) or off (stop both, end any request).
    pub fn host(&mut self, on: bool, io: &Arc<dyn MachinesIo>) {
        if on == self.host.is_some() || self.files.is_none() {
            return;
        }
        if !on {
            if let Some(h) = self.host.take() {
                h.stop.store(true, Ordering::SeqCst);
                if let Some((_, close)) = lock(&h.shared).active.take() {
                    close();
                }
            }
            self.prompt = None;
            self.net.announce(None);
            return;
        }
        let listener = match self.net.listen() {
            Ok(l) => l,
            Err(e) => {
                log::warn!("pairing: no listener: {e}");
                return;
            }
        };
        self.net.announce(Some((&self.me, listener.port())));
        let stop = Arc::new(AtomicBool::new(false));
        let shared: Arc<Mutex<HostShared>> = Arc::default();
        self.host = Some(HostRun {
            stop: Arc::clone(&stop),
            shared: Arc::clone(&shared),
        });
        let ctx = Arc::new(HostCtx {
            me: self.me.clone(),
            os: Arc::clone(&self.os),
            io: Arc::clone(io),
            flare: std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
        let post = Arc::clone(&self.post);
        worker("flare-pair-listen", move || {
            while !stop.load(Ordering::SeqCst) {
                let Some((mut s, peer)) = listener.accept(Duration::from_millis(200)) else {
                    continue;
                };
                let id = {
                    let mut sh = lock(&shared);
                    let locked = sh.locked_until.is_some_and(|t| Instant::now() < t);
                    if sh.active.is_some() || locked {
                        drop(sh);
                        let _ = wire::send(&mut *s, &json!({"t": "busy"}));
                        continue;
                    }
                    sh.next += 1;
                    let id = sh.next;
                    sh.active = Some((id, s.closer()));
                    id
                };
                let (ctx, post, shared) =
                    (Arc::clone(&ctx), Arc::clone(&post), Arc::clone(&shared));
                worker("flare-pair-host", move || {
                    // A wrong code in this request: leaving without pairing still locks (rule 1247).
                    let struck = std::cell::Cell::new(false);
                    let events = |e: HostEvent| {
                        match &e {
                            HostEvent::Strike { .. } => struck.set(true),
                            HostEvent::Done {
                                linked, too_many, ..
                            } => {
                                let mut sh = lock(&shared);
                                sh.active = None;
                                if locks_out(*too_many, linked.is_some(), struck.get()) {
                                    sh.locked_until = Some(Instant::now() + LOCKOUT);
                                }
                            }
                            HostEvent::Ask { .. } => {}
                        }
                        Self::say(&post, PairMsg::Host(e));
                    };
                    host_session(&mut *s, &peer, &ctx, id, &events);
                });
            }
        });
    }

    /// Decline (or Escape) in the host's code dialog: the request ends and the viewer hears so.
    pub fn decline(&mut self) {
        let Some(p) = self.prompt.take() else {
            return;
        };
        if let Some(h) = &self.host {
            let mut sh = lock(&h.shared);
            if sh.active.as_ref().is_some_and(|(id, _)| *id == p.id)
                && let Some((_, close)) = sh.active.take()
            {
                close();
            }
        }
    }

    /// A host event on the UI thread: the code dialog shows, stays, or closes.
    pub fn on_host(&mut self, e: &HostEvent) {
        match e {
            HostEvent::Ask { id, viewer, code } => {
                self.prompt = Some(Prompt {
                    id: *id,
                    viewer: viewer.clone(),
                    code: code.clone(),
                });
            }
            HostEvent::Strike { .. } => {}
            HostEvent::Done { id, .. } => {
                if self.prompt.as_ref().is_some_and(|p| p.id == *id) {
                    self.prompt = None;
                }
            }
        }
    }

    /// The nearby computers, as a browse found them.
    pub fn set_nearby(&mut self, list: Vec<Nearby>) {
        if self.browsing {
            self.nearby = list;
        }
    }

    /// Computer sharing is announced and listening (the tests).
    #[cfg(test)]
    pub fn hosting(&self) -> bool {
        self.host.is_some()
    }
}

impl Drop for Pairing {
    fn drop(&mut self) {
        self.end_viewer();
        if let Some(h) = self.host.take() {
            h.stop.store(true, Ordering::SeqCst);
        }
    }
}

// ---- the real network -------------------------------------------------------------------------------------------

/// The real network: mDNS through `mdns-sd`, plain TCP for the pairing port.
#[derive(Default)]
pub struct RealPairNet {
    daemon: Mutex<Option<mdns_sd::ServiceDaemon>>,
    /// The full name this computer announces now (its own announcement is never a nearby computer).
    mine: Arc<Mutex<Option<String>>>,
    browse_gen: Arc<AtomicU64>,
}

impl RealPairNet {
    fn daemon(&self) -> Option<mdns_sd::ServiceDaemon> {
        let mut d = lock(&self.daemon);
        if d.is_none() {
            match mdns_sd::ServiceDaemon::new() {
                Ok(n) => *d = Some(n),
                Err(e) => log::warn!("pairing: no mDNS: {e}"),
            }
        }
        d.clone()
    }
}

/// Pure: the mDNS host name for a computer name: letters, digits and dashes, then `.local.`.
pub fn mdns_host(name: &str) -> String {
    let mut h: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(40)
        .collect();
    h = h.trim_matches('-').to_string();
    if h.is_empty() {
        h = "flare".into();
    }
    format!("{h}.local.")
}

/// Pure: the TXT record of an announcement (no secret: the name, the system, Flare's version, the protocol).
pub fn txt_of(me: &Me) -> Vec<(String, String)> {
    vec![
        ("name".into(), me.name.clone()),
        ("os".into(), os_word(me.os).into()),
        ("version".into(), me.version.clone()),
        ("proto".into(), PROTOCOL.to_string()),
    ]
}

struct TcpPairListener(std::net::TcpListener);

impl PairListener for TcpPairListener {
    fn port(&self) -> u16 {
        self.0.local_addr().map(|a| a.port()).unwrap_or(0)
    }
    fn accept(&self, wait: Duration) -> Option<(Box<dyn Duplex>, String)> {
        match self.0.accept() {
            Ok((s, at)) => {
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(WINDOW));
                let _ = s.set_nodelay(true);
                Some((Box::new(s), at.ip().to_string()))
            }
            Err(_) => {
                std::thread::sleep(wait); // sync-ok: the listener's worker waits between two looks
                None
            }
        }
    }
}

impl PairNet for RealPairNet {
    fn announce(&self, me: Option<(&Me, u16)>) {
        let Some(d) = self.daemon() else {
            return;
        };
        if let Some(old) = lock(&self.mine).take() {
            let _ = d.unregister(&old);
        }
        let Some((me, port)) = me else {
            log::info!("pairing: no longer announced");
            return;
        };
        let txt = txt_of(me);
        let txt: Vec<(&str, &str)> = txt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        match mdns_sd::ServiceInfo::new(SERVICE, &me.name, &mdns_host(&me.name), (), port, &txt[..])
        {
            Ok(info) => {
                let info = info.enable_addr_auto();
                let full = info.get_fullname().to_string();
                match d.register(info) {
                    Ok(()) => {
                        log::info!("pairing: announced on port {port}");
                        *lock(&self.mine) = Some(full);
                    }
                    Err(e) => log::warn!("pairing: could not announce: {e}"),
                }
            }
            Err(e) => log::warn!("pairing: could not announce: {e}"),
        }
    }

    fn browse(&self, on: bool, found: Arc<dyn Fn(Vec<Nearby>) + Send + Sync>) {
        let n = self.browse_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(d) = self.daemon() else {
            return;
        };
        let _ = d.stop_browse(SERVICE);
        if !on {
            return;
        }
        let rx = match d.browse(SERVICE) {
            Ok(rx) => rx,
            Err(e) => {
                log::warn!("pairing: could not look for computers: {e}");
                return;
            }
        };
        let (generation, mine) = (Arc::clone(&self.browse_gen), Arc::clone(&self.mine));
        worker("flare-pair-browse", move || {
            let mut seen: HashMap<String, Nearby> = HashMap::new();
            while generation.load(Ordering::SeqCst) == n {
                let Ok(ev) = rx.recv_timeout(Duration::from_millis(500)) else {
                    // sync-ok: the browse worker waits for the next announcement
                    continue;
                };
                let changed = match ev {
                    mdns_sd::ServiceEvent::ServiceResolved(info) => {
                        let full = info.get_fullname().to_string();
                        if lock(&mine).as_deref() == Some(full.as_str()) {
                            continue;
                        }
                        let txt = |k: &str| info.get_property_val_str(k).map(str::to_string);
                        let addrs = info
                            .get_addresses()
                            .iter()
                            .map(mdns_sd::ScopedIp::to_ip_addr)
                            .collect();
                        match nearby_of(&full, &txt, addrs, info.get_port()) {
                            Some(nb) => {
                                let changed = seen.get(&full) != Some(&nb);
                                seen.insert(full, nb);
                                changed
                            }
                            None => false,
                        }
                    }
                    mdns_sd::ServiceEvent::ServiceRemoved(_, full) => seen.remove(&full).is_some(),
                    _ => false,
                };
                if changed {
                    let mut list: Vec<Nearby> = seen.values().cloned().collect();
                    list.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
                    found(list);
                }
            }
        });
    }

    fn listen(&self) -> Result<Box<dyn PairListener>, String> {
        let l = std::net::TcpListener::bind(("0.0.0.0", 0)).map_err(|e| e.to_string())?;
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Box::new(TcpPairListener(l)))
    }

    fn connect(&self, to: &Nearby) -> Result<(Box<dyn Duplex>, String), String> {
        let mut last = String::from("no address");
        for ip in &to.addrs {
            let at = std::net::SocketAddr::new(*ip, to.port);
            match std::net::TcpStream::connect_timeout(&at, Duration::from_secs(5)) {
                Ok(s) => {
                    let _ = s.set_read_timeout(Some(WINDOW));
                    let _ = s.set_nodelay(true);
                    return Ok((Box::new(s), ip.to_string()));
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(last)
    }
}

#[cfg(test)]
#[path = "pairing_fake.rs"]
pub(crate) mod fake;

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;
