//! Machine pairing PR 3 (rules 1256 to 1260): pairing two computers with a 6-digit code.
//!
//! Spec `docs/superpowers/specs/2026-11-14-machine-pairing-design.md` ("What happens underneath") and the approved
//! mockup (`2026-11-14-machine-pairing-mockup.html`, "Add a machine", "The code", "Linked"):
//!
//! - **The host** (Computer sharing on): announces itself on the network as `_flare-pair._tcp` (mDNS; TXT `name`, `os`,
//!   `version`, `proto`, no secret) and listens on the first free port of `sharing::PAIRING_PORTS` (the ports Flare's
//!   firewall rule names), the one the announcement names. A viewer that
//!   connects and says hello within [`HELLO_WAIT`] makes the host show the Flare dialog "<viewer> wants to connect"
//!   with a fresh 6-digit code and Decline, wherever the person is in Flare. One request at a time, taken only after a
//!   valid hello: another connection meanwhile is told `busy`. After three wrong codes the window closes. Wrong codes
//!   and requests that ended unpaired after the code showed are counted per address and in all, across connections
//!   ([`Guard`], rule 1261): each such request locks the host for longer ([`LOCKOUTS`]), and a cap per hour stops
//!   guessing for good.
//! - **The viewer**: while the Add a machine dialog is open it looks for those announcements; they are the drop-down's
//!   first choices. Picking one connects and says hello; the dialog asks for the "Code shown on <name>". Add runs the
//!   SPAKE2 exchange (`pair_wire`). A wrong code is the red line "Wrong code".
//! - **Over the confirmed channel** the viewer sends Flare's own public key (`pair_keys`, made on first need) and its
//!   name; the host adds it to the right `authorized_keys` (rule 1258) and answers with its SSH host key, its user, its
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
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use flare_settings::kv::PrefStore;
use flare_tabs::tabs::Os;
use serde_json::{Value, json};

use crate::appctl::{Post, UiMsg};
use crate::machines::{Incoming, Machine, argv_word, machine_id, user_word};
use crate::machines_page::{MachinesIo, MachinesMsg};
use crate::pair_keys::{HOST_KEY_TYPES, PairFiles, authorized_line, parse_public_key};
use crate::pair_wire::{self as wire, AttemptEnd, HostEnd, PROTOCOL};
use crate::settings_ctl::SettingsMsg;
use crate::sharing::{PAIRING_PORTS, SharingOs};

/// The mDNS service type a computer with Computer sharing on announces.
pub const SERVICE: &str = "_flare-pair._tcp.local.";
/// How long the host locks after a request that ended unpaired once the code showed: the first such request in the
/// last hour, the second, the third and later (rule 1261).
pub const LOCKOUTS: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(120),
    Duration::from_secs(600),
];
/// The time over which wrong codes and failed requests are counted.
pub const STRIKE_WINDOW: Duration = Duration::from_secs(3600);
/// The most strikes (wrong codes, and requests that ended unpaired after the code showed) per hour from one address,
/// and from all addresses together; past either, no request is taken until the oldest is an hour old.
pub const PEER_STRIKES_MAX: usize = 10;
pub const ALL_STRIKES_MAX: usize = 20;
/// How long either end waits for the other: the person reads the code and types it in this time.
pub const WINDOW: Duration = Duration::from_secs(300);
/// How long the host waits for a viewer's hello; the request slot is taken only after a valid one (rule 1261).
pub const HELLO_WAIT: Duration = Duration::from_secs(5);
/// The most connections that wait for their hello at once; another is closed at once.
pub const HELLO_WAITING_MAX: usize = 4;
/// How long turning sharing off waits for a request that runs (one may be at the UAC prompt) before its clean up.
pub const DRAIN_WAIT: Duration = Duration::from_secs(120);

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
    /// How long one read waits before it fails (the hello's short wait, then the code's window).
    fn set_wait(&self, _wait: Duration) {}
}

impl Duplex for std::net::TcpStream {
    fn set_wait(&self, wait: Duration) {
        let _ = self.set_read_timeout(Some(wait));
    }
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
    /// A listener on the first free port of [`PAIRING_PORTS`] ([`bind_first`]). Err is the one line for the person
    /// (none of the ports was free), or empty when there is nothing to say (no pairing here).
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
        Err(String::new())
    }
    fn connect(&self, _to: &Nearby) -> Result<(Box<dyn Duplex>, String), String> {
        Err("no pairing here".into())
    }
}

/// Pure: binds the first port of [`PAIRING_PORTS`] that `bind` takes, in order, and says which (rule 1256). The ports
/// are fixed, so Flare's Windows firewall rule names them. When every one is taken, the one plain line that says so
/// (`bind`'s own words go to the log).
pub fn bind_first<T>(mut bind: impl FnMut(u16) -> Result<T, String>) -> Result<(T, u16), String> {
    for port in PAIRING_PORTS {
        match bind(port) {
            Ok(t) => return Ok((t, port)),
            Err(e) => log::info!("pairing: port {port} is taken: {e}"),
        }
    }
    Err(ports_taken_line())
}

/// The one line when every pairing port is taken by another program.
pub fn ports_taken_line() -> String {
    format!(
        "Other programs use ports {} to {}, so other computers cannot pair with this one. Close them, then turn \
         Computer sharing off and on.",
        PAIRING_PORTS.start(),
        PAIRING_PORTS.end()
    )
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
    /// The request ended: `linked` with the computers that reach this one now, or not.
    Done {
        id: u64,
        linked: Option<Vec<Incoming>>,
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
    /// Computer sharing turned off: a key that went in meanwhile comes out again (rule 1258).
    pub stop: Arc<AtomicBool>,
    /// The settings store, where a key in the admin file is noted the moment it goes in (rule 1258).
    pub prefs: Arc<Mutex<Option<PrefStore>>>,
    /// The sharing clean ups that went past their wait for the requests, and how many had when this host started: one
    /// more means this host's clean up may have read `adminKeys` already (rule 1258).
    pub clean_ups: CleanUps,
    pub clean_ups_at_start: u64,
}

impl HostCtx {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// This host's sharing clean up went past its wait for the requests, so it may have read `adminKeys` and cleaned
    /// the admin file already. Before that, it provably has not: it waits for this request first.
    fn clean_up_passed(&self) -> bool {
        self.clean_ups.passed() != self.clean_ups_at_start
    }

    /// A key went into `administrators_authorized_keys`: turning sharing off cleans it up elevated (rule 1255).
    fn note_admin(&self) {
        if let Some(p) = lock(&self.prefs).as_ref() {
            flare_settings::machine_prefs::set_admin_keys(p, true);
        }
    }
}

/// BLOCKING: the viewer's hello, which must come within [`HELLO_WAIT`] (the caller sets the wait). None when it is no
/// hello, or speaks another protocol (that viewer hears `proto`).
pub fn read_hello(s: &mut dyn Duplex) -> Option<Value> {
    let hello = wire::recv(s).ok()?;
    if wire::kind(&hello) != "hello" {
        return None;
    }
    if hello.get("proto").and_then(Value::as_u64) != Some(PROTOCOL) {
        let _ = wire::send(s, &json!({"t": "proto", "proto": PROTOCOL}));
        return None;
    }
    Some(hello)
}

/// BLOCKING: one request on the host (rules 1257 and 1258), from the viewer's valid `hello` to the end. `events`
/// reaches the UI thread.
///
/// Everything that can fail is read before the viewer's key goes in (the SSH host key, the user name). The key's entry
/// is saved, and a key in the admin file noted, the moment it is in; when a later step fails (the answer cannot be
/// sent) or sharing turned off meanwhile, the key and the entry come out again.
pub fn host_session(
    s: &mut dyn Duplex,
    peer: &str,
    hello: &Value,
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
    let key_field = parse_public_key(key, &[crate::pair_keys::ED25519])
        .map(|k| k.key)
        .unwrap_or_default();
    // Everything the answer needs is read before the key goes in, so a failure here leaves nothing behind.
    let Some(host_key) = ctx
        .os
        .ssh_host_key()
        .ok()
        .and_then(|k| parse_public_key(k.trim(), &HOST_KEY_TYPES))
    else {
        refuse(&mut ch, s, "The SSH server's key could not be read there.");
        return done(None, false);
    };
    let user = ctx.os.user_name();
    if !user_word(&user) {
        log::warn!("pairing: this account's user name cannot go to ssh");
        refuse(
            &mut ch,
            s,
            "This account's user name cannot be used over SSH.",
        );
        return done(None, false);
    }
    if ctx.stopped() {
        return done(None, false);
    }
    // Audit #3: a record that cannot be read is never saved over, so no key goes in that it would not list.
    let before = match ctx.io.load_in() {
        Ok(list) => list,
        Err(e) => {
            log::warn!("pairing: {e}");
            refuse(
                &mut ch,
                s,
                "This computer's list of linked computers could not be read.",
            );
            return done(None, false);
        }
    };
    // The same key paired before: it is in already, so a failure later leaves it there.
    let was_in = before.iter().any(|m| m.key == key_field);
    let admin = match crate::sharing::install_key(&*ctx.os, &line) {
        Ok(admin) => admin,
        Err(why) => {
            refuse(&mut ch, s, &why);
            return done(None, false);
        }
    };
    // The key is in: Flare records it at once, so a crash or a turn off from here still cleans it up.
    if admin {
        ctx.note_admin();
    }
    let mut list = before.clone();
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
        key: key_field.clone(),
        // In the admin file now, or as the older entry said (None stays unknown: Remove then asks, rule 1260).
        admin: if admin {
            Some(true)
        } else {
            old.map_or(Some(false), |i| list[i].admin)
        },
    };
    match old {
        Some(i) => list[i] = entry,
        None => list.push(entry),
    }
    if let Err(e) = ctx.io.save_in(&list) {
        log::warn!("pairing: {e}");
    }
    // A failed step after the key went in takes it out again, and the entry with it.
    let roll_back = |why: &str| {
        log::info!("pairing: the request from {peer} is undone: {why}");
        if !was_in {
            // After a turn off, its clean up removes the admin file's keys elevated, so no second prompt here, but only
            // while that clean up provably has not run: it waits for this request before it reads `adminKeys`. Once it
            // went past that wait (it gave up after DRAIN_WAIT), it may have cleaned already, so the key goes here.
            let admin_too = admin && (!ctx.stopped() || ctx.clean_up_passed());
            if let Err(e) = crate::sharing::remove_key(&*ctx.os, &key_field, admin_too) {
                log::warn!("pairing: the key could not be taken out again: {e}");
            }
        }
        if let Err(e) = ctx.io.save_in(&before) {
            log::warn!("pairing: {e}");
        }
    };
    if ctx.stopped() {
        roll_back("Computer sharing turned off");
        return done(None, false);
    }
    let mut reply = json!({
        "ok": true,
        "host_key": host_key.line(),
        "user": user,
        "os": os_word(ctx.me.os),
        "name": ctx.me.name,
        "version": ctx.me.version,
        "port": crate::sharing::SSH_PORT,
    });
    // A Mac copy that runs from a disk image or a moved-aside copy names the installed one, or none (rule 1259).
    let mac = ctx.me.os == Some(Os::Mac);
    if let Some(flare) = reply_flare_path(&ctx.flare, mac, &|p| std::path::Path::new(p).is_file()) {
        reply["flare"] = json!(flare);
    }
    if let Err(e) = ch.send(s, &reply) {
        roll_back(&format!("the answer could not be sent: {e}"));
        return done(None, false);
    }
    if ctx.stopped() {
        roll_back("Computer sharing turned off");
        return done(None, false);
    }
    done(Some(list), false)
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

/// Where the Mac app keeps Flare once it is installed.
pub const MAC_APP_FLARE: &str = "/Applications/Flare.app/Contents/MacOS/flare";

/// Pure: the Flare path the host answers with (rule 1259). A Mac copy that runs from a mounted disk image
/// (`/Volumes/`) or from macOS' moved-aside copy (`/AppTranslocation/`) is gone once that closes, so the host names the
/// installed app when `exists` finds it, or no path at all (the viewer then looks Flare up itself). Any other path is
/// sent as it is; an empty one is none.
pub fn reply_flare_path(exe: &str, mac: bool, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    if mac && (exe.starts_with("/Volumes/") || exe.contains("/AppTranslocation/")) {
        return exists(MAC_APP_FLARE).then(|| MAC_APP_FLARE.to_string());
    }
    (!exe.trim().is_empty()).then(|| exe.to_string())
}

/// Pure: the Flare path a viewer saves when the host named none (rule 1259): where the Mac app keeps it on a Mac, else
/// `flare` on the host's PATH, as a machine added by hand starts.
pub fn fallback_flare(kind: Option<Os>) -> &'static str {
    match kind {
        Some(Os::Mac) => MAC_APP_FLARE,
        _ => "flare",
    }
}

/// Pure: a Flare path the host named, kept only when ssh can carry it as one word of the remote command.
pub fn flare_path_ok(p: &str) -> bool {
    !p.trim().is_empty()
        && p.len() <= 1024
        && !p.chars().any(|c| c.is_control() || c == '"' || c == '\'')
}

/// BLOCKING: one pairing on the viewer (rules 1257 and 1259), from the hello to the saved machine. The typed codes come
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
    // A host that is locked or busy answers `busy` at once and hangs up, maybe before the hello is written: its answer
    // is still read.
    if let Err(e) = wire::send(s, &hello) {
        log::info!("pairing: the hello to {name} did not go: {e}");
    }
    let answer = wire::recv(s);
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
    // Audit #3: the list is read before the other computer adds this one's key, and a file that cannot be read whole is
    // never saved over.
    let mut list = match ctx.io.load() {
        Ok(list) => list,
        Err(e) => return fail(e),
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
    // A host that names no Flare path (its copy runs from a disk image) leaves the viewer its own default.
    let flare = match reply.get("flare") {
        None => fallback_flare(Os::parse(field("os")).or(to.os)),
        Some(_) => field("flare"),
    };
    if !user_word(user) || !flare_path_ok(flare) || !argv_word(address) {
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
    // Pairing a machine again keeps its id (and its tabs). It is the same machine only when it proves the same SSH host
    // key: a name or an address the other computer says could be anyone's, so neither ever picks an entry to replace.
    let old = list
        .iter()
        .position(|m| m.host_key.as_deref() == Some(host_key.key.as_str()));
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
        host_key: Some(host_key.key.clone()),
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

/// The host's count of failed guessing (rule 1261), kept across connections for as long as sharing stays on. A strike is
/// a wrong code, or a request that ended unpaired once its code showed (hung up, Cancel, Decline, three wrong codes).
/// Each request that ended so locks the host for the next of [`LOCKOUTS`]; past [`PEER_STRIKES_MAX`] strikes from one
/// address or [`ALL_STRIKES_MAX`] from all in [`STRIKE_WINDOW`], no request is taken until the oldest ages out. Without
/// it a stranger could guess, hang up and ask again for a fresh code at once, with no limit on its guesses. A typo
/// followed by the right code pairs, and the pairing address's strikes go.
#[derive(Debug, Default)]
pub struct Guard {
    strikes: Vec<(Instant, String)>,
    failed: Vec<Instant>,
    locked_until: Option<Instant>,
}

impl Guard {
    fn forget_old(&mut self, now: Instant) {
        let fresh = |t: &Instant| now.saturating_duration_since(*t) < STRIKE_WINDOW;
        self.strikes.retain(|(t, _)| fresh(t));
        self.failed.retain(fresh);
    }

    /// Whether a request from `peer` is taken now.
    pub fn admits(&mut self, peer: &str, now: Instant) -> bool {
        self.forget_old(now);
        if self.locked_until.is_some_and(|t| now < t) {
            return false;
        }
        let mine = self.strikes.iter().filter(|(_, p)| p == peer).count();
        mine < PEER_STRIKES_MAX && self.strikes.len() < ALL_STRIKES_MAX
    }

    /// A wrong code from `peer`.
    pub fn strike(&mut self, peer: &str, now: Instant) {
        self.strikes.push((now, peer.to_string()));
    }

    /// A request from `peer` ended: `shown` once its code showed, `linked` when it paired.
    pub fn ended(&mut self, peer: &str, now: Instant, shown: bool, linked: bool) {
        self.forget_old(now);
        if linked {
            self.strikes.retain(|(_, p)| p != peer);
            return;
        }
        if !shown {
            return;
        }
        self.strikes.push((now, peer.to_string()));
        self.failed.push(now);
        let lockout = LOCKOUTS[(self.failed.len() - 1).min(LOCKOUTS.len() - 1)];
        let until = now + lockout;
        self.locked_until = Some(self.locked_until.map_or(until, |t| t.max(until)));
    }
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
    /// The connections that wait for their hello: their number, when their wait ends, and how to end them.
    waiting: Vec<(u64, Instant, Closer)>,
    guard: Guard,
    next: u64,
}

/// The host's workers that run now (the listener and each request), so turning sharing off can wait for them before
/// its clean up (rule 1258).
#[derive(Clone, Default)]
pub struct Running(Arc<(Mutex<usize>, Condvar)>);

/// One running worker; it counts until dropped.
pub struct RunningOne(Running);

/// How many sharing clean ups went past their wait for the host's requests (rule 1258). A request that rolls its key
/// back compares it with the count when its host started.
#[derive(Clone, Default)]
pub struct CleanUps(Arc<AtomicU64>);

impl CleanUps {
    /// A clean up's wait ended (the requests ended, or it gave up): it reads `adminKeys` next.
    pub fn pass(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    pub fn passed(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl Running {
    fn enter(&self) -> RunningOne {
        *lock(&self.0.0) += 1;
        RunningOne(self.clone())
    }

    /// BLOCKING: waits until none runs, at most `max`. Whether none runs.
    pub fn wait_idle(&self, max: Duration) -> bool {
        let (m, cv) = &*self.0;
        let n = lock(m);
        let (n, _) = cv
            .wait_timeout_while(n, max, |n| *n > 0) // sync-ok: the sharing clean up's worker waits
            .unwrap_or_else(PoisonError::into_inner);
        *n == 0
    }
}

impl Drop for RunningOne {
    fn drop(&mut self) {
        let (m, cv) = &*self.0.0;
        let mut n = lock(m);
        *n = n.saturating_sub(1);
        cv.notify_all();
    }
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
    /// The settings store the host's requests note an admin key in.
    prefs: Arc<Mutex<Option<PrefStore>>>,
    running: Running,
    clean_ups: CleanUps,
    /// No pairing port was free while sharing is on: said once, tried again after sharing went off.
    listen_failed: bool,
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
            prefs: Arc::default(),
            running: Running::default(),
            clean_ups: CleanUps::default(),
            listen_failed: false,
        }
    }

    /// The same pairing with the switch's OS calls.
    pub fn set_os(&mut self, os: Arc<dyn SharingOs>) {
        self.os = os;
    }

    /// The settings store a host request notes a key in `administrators_authorized_keys` in, the moment it goes in.
    pub fn set_prefs(&self, prefs: PrefStore) {
        *lock(&self.prefs) = Some(prefs);
    }

    /// BLOCKING when called: waits until the host's listener and requests have ended (at most [`DRAIN_WAIT`]), for
    /// turning sharing off to run its clean up after them (rule 1258).
    pub fn drain(&self) -> Arc<dyn Fn() + Send + Sync> {
        let (running, clean_ups) = (self.running.clone(), self.clean_ups.clone());
        Arc::new(move || {
            if !running.wait_idle(DRAIN_WAIT) {
                log::warn!("pairing: a request still runs; the clean up goes ahead");
            }
            // From here the clean up may read `adminKeys`: a request that rolls back later removes its own admin key.
            clean_ups.pass();
        })
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

    /// Computer sharing is on (listen and announce) or off (stop both, end any request). The one line for the sharing
    /// dialog when no pairing port was free (rule 1256); it is said once, and tried again only after sharing went off.
    pub fn host(&mut self, on: bool, io: &Arc<dyn MachinesIo>) -> Option<String> {
        if !on {
            self.listen_failed = false;
        }
        if on == self.host.is_some() || self.files.is_none() || (on && self.listen_failed) {
            return None;
        }
        if !on {
            if let Some(h) = self.host.take() {
                h.stop.store(true, Ordering::SeqCst);
                let mut sh = lock(&h.shared);
                if let Some((_, close)) = sh.active.take() {
                    close();
                }
                for (_, _, close) in sh.waiting.drain(..) {
                    close();
                }
            }
            self.prompt = None;
            self.net.announce(None);
            return None;
        }
        let listener = match self.net.listen() {
            Ok(l) => l,
            Err(e) => {
                log::warn!("pairing: no listener: {e}");
                self.listen_failed = true;
                return (!e.is_empty()).then_some(e);
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
            stop: Arc::clone(&stop),
            prefs: Arc::clone(&self.prefs),
            clean_ups: self.clean_ups.clone(),
            clean_ups_at_start: self.clean_ups.passed(),
        });
        let (post, running) = (Arc::clone(&self.post), self.running.clone());
        // The listener counts as running from here, so a turn off right after waits for every request it starts.
        let listening = running.enter();
        worker("flare-pair-listen", move || {
            let _listening = listening;
            while !stop.load(Ordering::SeqCst) {
                // A connection that is still quiet when its hello wait ends is closed (no slow drip holds a place).
                {
                    let now = Instant::now();
                    let mut sh = lock(&shared);
                    sh.waiting.retain(|(_, until, close)| {
                        let keep = now < *until;
                        if !keep {
                            close();
                        }
                        keep
                    });
                }
                let Some((mut s, peer)) = listener.accept(Duration::from_millis(200)) else {
                    continue;
                };
                let n = {
                    let mut sh = lock(&shared);
                    if !sh.guard.admits(&peer, Instant::now()) {
                        drop(sh);
                        let _ = wire::send(&mut *s, &json!({"t": "busy"}));
                        continue;
                    }
                    if sh.waiting.len() >= HELLO_WAITING_MAX {
                        continue;
                    }
                    sh.next += 1;
                    let n = sh.next;
                    sh.waiting
                        .push((n, Instant::now() + HELLO_WAIT, s.closer()));
                    n
                };
                let (ctx, post, shared, one) = (
                    Arc::clone(&ctx),
                    Arc::clone(&post),
                    Arc::clone(&shared),
                    running.enter(),
                );
                worker("flare-pair-host", move || {
                    let _one = one;
                    s.set_wait(HELLO_WAIT);
                    let hello = read_hello(&mut *s);
                    lock(&shared).waiting.retain(|(w, _, _)| *w != n);
                    let Some(hello) = hello else {
                        return;
                    };
                    // The one request slot is taken only now, after a valid hello (rule 1261).
                    let claimed = {
                        let mut sh = lock(&shared);
                        let free = sh.active.is_none()
                            && !ctx.stopped()
                            && sh.guard.admits(&peer, Instant::now());
                        if free {
                            sh.active = Some((n, s.closer()));
                        }
                        free
                    };
                    if !claimed {
                        let _ = wire::send(&mut *s, &json!({"t": "busy"}));
                        return;
                    }
                    s.set_wait(WINDOW);
                    // The code showed: leaving unpaired from here counts against this address and all (rule 1261).
                    let shown = std::cell::Cell::new(false);
                    let events = |e: HostEvent| {
                        match &e {
                            HostEvent::Ask { .. } => shown.set(true),
                            HostEvent::Strike { .. } => {
                                lock(&shared).guard.strike(&peer, Instant::now());
                            }
                            HostEvent::Done { linked, .. } => {
                                let mut sh = lock(&shared);
                                sh.active = None;
                                sh.guard.ended(
                                    &peer,
                                    Instant::now(),
                                    shown.get(),
                                    linked.is_some(),
                                );
                            }
                        }
                        Self::say(&post, PairMsg::Host(e));
                    };
                    host_session(&mut *s, &peer, &hello, &ctx, n, &events);
                });
            }
        });
        None
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

/// Pure: the mDNS host name the announcement's address records go under (rule 1256): the computer name in letters,
/// digits and dashes, then `-flare.local.`. Never the computer's own `.local` name: macOS' mDNSResponder (and Windows'
/// own responder) answer that one, and a second answer from `mdns-sd` is a name conflict, after which macOS renames
/// the Mac ("Maxs-Mac-mini (2)"). The readable name stays in the instance name and the TXT `name`.
pub fn mdns_host(name: &str) -> String {
    let mut h: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(40)
        .collect();
    h = h.trim_matches('-').to_string();
    if h.is_empty() {
        h = "computer".into();
    }
    format!("{h}-flare.local.")
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
        let (l, port) = bind_first(|port| {
            let l = std::net::TcpListener::bind(("0.0.0.0", port)).map_err(|e| e.to_string())?;
            l.set_nonblocking(true).map_err(|e| e.to_string())?;
            Ok(l)
        })?;
        log::info!("pairing: listening on port {port}");
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
