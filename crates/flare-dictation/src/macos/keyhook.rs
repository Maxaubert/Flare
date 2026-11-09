//! The dictation key from any app on macOS (port phase 7). The same `KeyHook` the owner drives on Windows
//! (`start`, `keys`, `pick`, `rec`, `alive`, `stop`, the lock, `Ready` / `Waiting` / `Failed`), on two public
//! APIs chosen by the binding:
//!
//! - **A chord that ends in a key** (Ctrl+Shift+D, F9): Carbon's `RegisterEventHotKey`. It needs no permission
//!   at all, swallows the key system-wide by itself and reports the press and the release. Flare reports the
//!   chord's modifiers down, then its key, and on the release the key up, then the modifiers, so the shared
//!   `KeyFeed` and reducer read it as they read the Windows hook.
//! - **A bare modifier or a chord of modifiers** (Right Option, the default Ctrl+Cmd): there is no hot key for
//!   these, and the solo rules need the key-up and "another key". A LISTEN-ONLY `CGEventTap` on the hook thread
//!   reads the modifiers and says only "another key" (never which) while a binding key is down. It needs Input
//!   Monitoring, which is read with `CGPreflightListenEventAccess` (it never asks); while it is not granted no tap
//!   is created (creating one would make the system ask), nothing is reported, and the window reads the key
//!   itself, as with no hook. Settings names the pane (`anywhere_problem`). It is polled, so a grant is followed.
//!
//! The session picker's digits 1 to 9 and Esc (and Esc while a recording is live) are Carbon hot keys in both
//! cases, registered only while the owner switches them on: a listen-only tap cannot swallow. They are registered
//! with no modifier and with every set of the binding's own modifiers, which is the Windows hook's "no foreign
//! modifier" rule (Cmd+2 in a browser stays the browser's).
//!
//! Nothing is posted: no `CGEventPost`, no Accessibility. The Start-menu mask has no Mac meaning (`mask` does
//! nothing). Carbon's calls run on the main thread (`dispatch_async_f` on the main queue, which the app's
//! `NSApplication` drains); the tap runs on the hook thread's own run loop. Every system call sits behind the
//! `System` seam, and the tests drive a fake one: no test registers a real hot key or creates a real tap.

use super::*;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::os::fd::AsRawFd;
use std::sync::atomic::AtomicU64;

// ----- the keys -----

/// Windows virtual keys to Mac virtual key codes (`kVK_*`, by position), for every key a binding or the picker can
/// name. Pause, Scroll Lock and F21 to F24 have no Mac key and are left out.
const KEYCODES: &[(u16, u16)] = &[
    // Letters (kVK_ANSI_A ..).
    (0x41, 0x00),
    (0x42, 0x0b),
    (0x43, 0x08),
    (0x44, 0x02),
    (0x45, 0x0e),
    (0x46, 0x03),
    (0x47, 0x05),
    (0x48, 0x04),
    (0x49, 0x22),
    (0x4a, 0x26),
    (0x4b, 0x28),
    (0x4c, 0x25),
    (0x4d, 0x2e),
    (0x4e, 0x2d),
    (0x4f, 0x1f),
    (0x50, 0x23),
    (0x51, 0x0c),
    (0x52, 0x0f),
    (0x53, 0x01),
    (0x54, 0x11),
    (0x55, 0x20),
    (0x56, 0x09),
    (0x57, 0x0d),
    (0x58, 0x07),
    (0x59, 0x10),
    (0x5a, 0x06),
    // The digit row.
    (0x30, 0x1d),
    (0x31, 0x12),
    (0x32, 0x13),
    (0x33, 0x14),
    (0x34, 0x15),
    (0x35, 0x17),
    (0x36, 0x16),
    (0x37, 0x1a),
    (0x38, 0x1c),
    (0x39, 0x19),
    // The keypad's 1 to 9 (the picker's).
    (0x61, 0x53),
    (0x62, 0x54),
    (0x63, 0x55),
    (0x64, 0x56),
    (0x65, 0x57),
    (0x66, 0x58),
    (0x67, 0x59),
    (0x68, 0x5b),
    (0x69, 0x5c),
    // F1 to F20.
    (0x70, 0x7a),
    (0x71, 0x78),
    (0x72, 0x63),
    (0x73, 0x76),
    (0x74, 0x60),
    (0x75, 0x61),
    (0x76, 0x62),
    (0x77, 0x64),
    (0x78, 0x65),
    (0x79, 0x6d),
    (0x7a, 0x67),
    (0x7b, 0x6f),
    (0x7c, 0x69),
    (0x7d, 0x6b),
    (0x7e, 0x71),
    (0x7f, 0x6a),
    (0x80, 0x40),
    (0x81, 0x4f),
    (0x82, 0x50),
    (0x83, 0x5a),
    // The modifiers, both sides: Control, Shift, Option (Alt, AltGr on a Norwegian PC), Command (Win).
    (VK_LCTRL, 0x3b),
    (VK_RCTRL, 0x3e),
    (VK_LSHIFT, 0x38),
    (VK_RSHIFT, 0x3c),
    (VK_LMENU, 0x3a),
    (VK_RMENU, 0x3d),
    (VK_LWIN, 0x37),
    (VK_RWIN, 0x36),
    (VK_ESC, 0x35),
    (0x20, 0x31),
    // Backquote: the key left of 1 (kVK_ANSI_Grave).
    (0xc0, 0x32),
];

/// The Mac key code of a Windows virtual key, or None when a Mac keyboard has no such key.
pub fn mac_keycode(vk: u16) -> Option<u16> {
    KEYCODES.iter().find(|(v, _)| *v == vk).map(|(_, k)| *k)
}

/// The Windows virtual key of a Mac key code, or None for a key no binding names.
pub fn vk_of_mac_keycode(kc: u16) -> Option<u16> {
    KEYCODES.iter().find(|(_, k)| *k == kc).map(|(v, _)| *v)
}

/// Carbon's modifier bits by family, in `FAMILIES` order (Ctrl, Shift, Alt, Win).
const CARBON: [u32; 4] = [0x1000, 0x200, 0x800, 0x100];
/// `CGEventFlags`' family bits, in the same order.
const CG_FAMILY: [u64; 4] = [0x4_0000, 0x2_0000, 0x8_0000, 0x10_0000];
/// The device-dependent bits (IOKit's `NX_DEVICE*KEYMASK`) that tell the left key from the right.
const CG_SIDES: &[(u16, u64)] = &[
    (VK_LCTRL, 0x0001),
    (VK_RCTRL, 0x2000),
    (VK_LSHIFT, 0x0002),
    (VK_RSHIFT, 0x0004),
    (VK_LMENU, 0x0020),
    (VK_RMENU, 0x0040),
    (VK_LWIN, 0x0008),
    (VK_RWIN, 0x0010),
];
const CG_ALL_SIDES: u64 = 0x207f;
const KC_ESC: u16 = 0x35;
const KC_CAPS: u16 = 0x39;
const KC_FN: u16 = 0x3f;

fn family_of(vk: u16) -> Option<usize> {
    FAMILIES.iter().position(|f| f.contains(&vk))
}

/// Is this side's modifier down in `flags`? The side's own bit when the event carries side bits, else its family's.
fn side_down(vk: u16, flags: u64) -> bool {
    let Some(f) = family_of(vk) else { return false };
    match CG_SIDES.iter().find(|(v, _)| *v == vk) {
        Some((_, bit)) if flags & CG_ALL_SIDES != 0 => flags & bit != 0,
        _ => flags & CG_FAMILY[f] != 0,
    }
}

/// The Carbon modifiers an event's flags carry.
fn carbon_mods(flags: u64) -> u32 {
    (0..4)
        .filter(|i| flags & CG_FAMILY[*i] != 0)
        .map(|i| CARBON[i])
        .fold(0, |a, b| a | b)
}

// ----- the plan: what to register and whether a tap is needed -----

/// What a Carbon hot key stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The binding's own chord.
    Binding,
    Esc,
    Digit(u8),
}

/// One Carbon hot key: a Mac key code with Carbon modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reg {
    pub keycode: u16,
    pub mods: u32,
    pub hit: Hit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// The binding ends in a key: a Carbon hot key, no permission.
    Chord,
    /// Modifiers only: the listen-only tap, Input Monitoring.
    Tap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub way: Way,
    pub regs: Vec<Reg>,
}

/// The plan for the binding's virtual keys (`keyfeed::keys_for`) and the owner's switches, or None when the Mac
/// cannot serve it (no keys yet, or a key a Mac keyboard does not have).
pub fn plan(keys: &HashSet<u16>, pick: bool, rec: bool) -> Option<Plan> {
    if keys.is_empty() || keys.iter().any(|vk| mac_keycode(*vk).is_none()) {
        return None;
    }
    let fam: Vec<usize> = (0..4)
        .filter(|i| FAMILIES[*i].iter().any(|k| keys.contains(k)))
        .collect();
    let plain: Vec<u16> = keys
        .iter()
        .copied()
        .filter(|vk| family_of(*vk).is_none())
        .collect();
    if plain.len() > 1 {
        return None;
    }
    let all_mods = fam.iter().map(|i| CARBON[*i]).fold(0, |a, b| a | b);
    let mut regs: Vec<Reg> = Vec::new();
    let way = match plain.first() {
        Some(vk) => {
            regs.push(Reg {
                keycode: mac_keycode(*vk)?,
                mods: all_mods,
                hit: Hit::Binding,
            });
            Way::Chord
        }
        None => Way::Tap,
    };
    // No modifier, and every set of the binding's own: the hook's "no foreign modifier" rule.
    let subsets: Vec<u32> = (0..(1u32 << fam.len()))
        .map(|m| {
            fam.iter()
                .enumerate()
                .filter(|(j, _)| m & (1 << j) != 0)
                .map(|(_, i)| CARBON[*i])
                .fold(0, |a, b| a | b)
        })
        .collect();
    let mut push = |r: Reg| {
        // The binding's own chord comes first and wins (as on Windows, a binding key is a binding key first).
        if !regs
            .iter()
            .any(|x| x.keycode == r.keycode && x.mods == r.mods)
        {
            regs.push(r);
        }
    };
    if pick {
        for n in 1..=9u16 {
            for vk in [0x30 + n, 0x60 + n] {
                for m in &subsets {
                    push(Reg {
                        keycode: mac_keycode(vk)?,
                        mods: *m,
                        hit: Hit::Digit(n as u8),
                    });
                }
            }
        }
    }
    if pick || rec {
        for m in &subsets {
            push(Reg {
                keycode: KC_ESC,
                mods: *m,
                hit: Hit::Esc,
            });
        }
    }
    Some(Plan { way, regs })
}

// ----- Carbon's press and release as the hook's lines -----

/// Whether the binding's chord is down as far as Carbon said.
#[derive(Debug, Default)]
pub struct ChordState {
    held: bool,
}

/// The binding's modifiers, one key per family (the left one the binding watches), in `FAMILIES` order.
fn chord_modifiers(keys: &HashSet<u16>) -> Vec<u16> {
    FAMILIES
        .iter()
        .filter_map(|f| f.iter().copied().find(|k| keys.contains(k)))
        .collect()
}

fn key_line(down: bool, vk: u16) -> Option<HookEvent> {
    code_of_vk(vk).map(|code| HookEvent::Key {
        down,
        code,
        front: None,
    })
}

/// One Carbon hot key event as the hook's lines. The binding's press is its modifiers down then its key down (a
/// press while held is the key's repeat); its release is the key up then the modifiers up. A digit or Esc is said
/// on its press only.
pub fn on_hit(st: &mut ChordState, hit: Hit, pressed: bool, keys: &HashSet<u16>) -> Vec<HookEvent> {
    match hit {
        Hit::Esc => pressed.then_some(HookEvent::Esc).into_iter().collect(),
        Hit::Digit(n) => pressed.then_some(HookEvent::Digit(n)).into_iter().collect(),
        Hit::Binding => {
            let mods = chord_modifiers(keys);
            let Some(main) = keys.iter().copied().find(|vk| family_of(*vk).is_none()) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            if pressed {
                if !st.held {
                    out.extend(mods.iter().filter_map(|vk| key_line(true, *vk)));
                }
                st.held = true;
                out.extend(key_line(true, main));
            } else if std::mem::take(&mut st.held) {
                out.extend(key_line(false, main));
                out.extend(mods.iter().rev().filter_map(|vk| key_line(false, *vk)));
            }
            out
        }
    }
}

// ----- the tap's events as the hook's lines -----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapKind {
    KeyDown,
    KeyUp,
    Flags,
}

/// The binding keys the tap holds down.
#[derive(Debug, Default)]
pub struct TapState {
    down: HashSet<u16>,
}

/// One tap event as at most one hook line. A binding modifier's change is its down or up; any other key going down
/// while a binding key is held is `Other`, never which key; a key Carbon has taken (the picker's, Esc) is Carbon's.
pub fn on_tap(
    st: &mut TapState,
    kind: TapKind,
    keycode: u16,
    flags: u64,
    keys: &HashSet<u16>,
    carbon: &[Reg],
) -> Option<HookEvent> {
    match kind {
        TapKind::KeyUp => None,
        TapKind::KeyDown => {
            let mods = carbon_mods(flags);
            if carbon
                .iter()
                .any(|r| r.keycode == keycode && r.mods == mods)
            {
                return None;
            }
            (!st.down.is_empty()).then_some(HookEvent::Other)
        }
        TapKind::Flags => {
            let vk = vk_of_mac_keycode(keycode).filter(|vk| family_of(*vk).is_some());
            match vk {
                Some(vk) if keys.contains(&vk) => {
                    if side_down(vk, flags) {
                        st.down.insert(vk).then(|| key_line(true, vk)).flatten()
                    } else {
                        st.down.remove(&vk).then(|| key_line(false, vk)).flatten()
                    }
                }
                Some(vk) => {
                    (side_down(vk, flags) && !st.down.is_empty()).then_some(HookEvent::Other)
                }
                // Caps Lock and Fn change the flags too: another key.
                None if keycode == KC_CAPS || keycode == KC_FN => {
                    (!st.down.is_empty()).then_some(HookEvent::Other)
                }
                None => None,
            }
        }
    }
}

/// The binding keys the tap holds that the system's flags now say are up (an up the tap never saw: it was
/// disabled for a moment, or secure input was on). Each is forgotten and answered with its up.
pub fn sweep(st: &mut TapState, flags_now: u64) -> Vec<HookEvent> {
    let gone: Vec<u16> = st
        .down
        .iter()
        .copied()
        .filter(|vk| !side_down(*vk, flags_now))
        .collect();
    let mut out = Vec::new();
    for vk in gone {
        st.down.remove(&vk);
        out.extend(key_line(false, vk));
    }
    out
}

// ----- what the dictation page says -----

/// Settings' words for a binding that needs Input Monitoring the app does not have.
pub const INPUT_MONITORING: &str = "Dictating from any app with a modifier key needs Input Monitoring. Turn Flare on in System Settings > Privacy & Security > Input Monitoring, or choose a key such as F9 or Ctrl+Shift+D, which needs nothing.";
/// Settings' words for a binding a Mac keyboard cannot press.
pub const NO_MAC_KEY: &str =
    "A Mac keyboard has no such key, so it works only in Flare's window. Choose another key.";

/// What stops "Dictate from any app" for these binding keys, given whether Input Monitoring is granted.
pub fn problem_for(keys: Option<&[u16]>, listen: bool) -> Option<&'static str> {
    let set: HashSet<u16> = keys?.iter().copied().collect();
    match plan(&set, false, false) {
        None => Some(NO_MAC_KEY),
        Some(p) if p.way == Way::Tap && !listen => Some(INPUT_MONITORING),
        Some(_) => None,
    }
}

// ----- the system seam -----

/// What a Carbon hot key event is handed to (it runs on the main thread).
pub type Sink = Arc<dyn Fn(Hit, bool) + Send + Sync>;

/// The tap's side of the hook thread.
pub trait TapSink {
    fn event(&self, kind: TapKind, keycode: u16, flags: u64);
    /// The tap exists and is enabled.
    fn ready(&self);
    /// Between events (every 50 ms at most): the system's flags now, when known. False ends the tap.
    fn tick(&self, flags_now: Option<u64>) -> bool;
}

/// Every system call the hook makes. `Real` in the app, a fake in the tests.
pub trait System: Send + Sync + 'static {
    /// Input Monitoring is granted. Never asks.
    fn listen_allowed(&self) -> bool;
    /// Replaces the hot keys with `regs`, owned by `owner`, on the main thread. False when the binding's own could
    /// not be registered (another app holds it) or the main thread did not answer in time. Gives up early (false)
    /// once `wanted` reads false: a quit on the main thread may be joining this hook, so no answer can come.
    fn register(&self, owner: u64, regs: &[Reg], sink: Sink, wanted: &AtomicBool) -> bool;
    /// Lets go of `owner`'s hot keys (a later owner's are left alone).
    fn unregister(&self, owner: u64);
    /// Runs a listen-only tap on this thread until `sink.tick` says stop. False when no tap could be made.
    fn run_tap(&self, sink: &dyn TapSink) -> bool;
    /// The process of the app in front.
    fn front_pid(&self) -> Option<u32>;
}

// ----- the lock -----

/// Take the lock file with an exclusive `flock`, without waiting. Held until the file drops (or the process ends).
pub fn take_lock(dir: &Path) -> io::Result<File> {
    let _ = std::fs::create_dir_all(dir);
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_NAME))?;
    loop {
        // SAFETY: flock on a descriptor this function owns; LOCK_NB never waits.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(f);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// True when the error means another holder has the lock.
pub fn is_busy(e: &io::Error) -> bool {
    e.raw_os_error() == Some(libc::EWOULDBLOCK)
}

// ----- the hook thread -----

/// How long the waits sleep between looks at `wanted`.
const SLICE: Duration = Duration::from_millis(20);
/// The lock and permission poll (the Windows hook's 500 ms), in slices.
const POLL_SLICES: u32 = 25;

#[derive(Default)]
struct Shared {
    keys: Mutex<HashSet<u16>>,
    pick: AtomicBool,
    rec: AtomicBool,
    /// Bumped by every change above: the thread re-plans when it moves.
    gen: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Shared {
    fn plan(&self) -> Option<Plan> {
        let keys = lock(&self.keys).clone();
        plan(
            &keys,
            self.pick.load(Ordering::SeqCst),
            self.rec.load(Ordering::SeqCst),
        )
    }
}

static OWNERS: AtomicU64 = AtomicU64::new(1);

/// The running hook: a thread that waits for a binding it can serve, takes the lock and serves it.
pub struct KeyHook {
    thread: Option<JoinHandle<()>>,
    wanted: Arc<AtomicBool>,
    shared: Arc<Shared>,
}

impl KeyHook {
    /// Start the thread: once the binding's keys are set and (for a modifier binding) Input Monitoring is granted,
    /// take the lock (`Waiting` while another copy holds it), register or tap, say `Ready` or `Failed`.
    pub fn start(lock_dir: PathBuf, tx: Sender<HookEvent>) -> Self {
        Self::start_with(lock_dir, tx, Arc::new(Real))
    }

    /// The same with any `System` (the tests' fake).
    pub fn start_with(lock_dir: PathBuf, tx: Sender<HookEvent>, sys: Arc<dyn System>) -> Self {
        let wanted = Arc::new(AtomicBool::new(true));
        let shared = Arc::new(Shared::default());
        let owner = OWNERS.fetch_add(1, Ordering::SeqCst);
        let t = {
            let (wanted, shared) = (wanted.clone(), shared.clone());
            std::thread::Builder::new()
                .name("flare-keyhook".into())
                .spawn(move || {
                    run(Run {
                        lock_dir,
                        tx,
                        sys,
                        shared,
                        wanted,
                        owner,
                    })
                })
                .ok()
        };
        KeyHook {
            thread: t,
            wanted,
            shared,
        }
    }

    /// The binding's virtual keys (either side of a modifier), as on Windows.
    pub fn keys(&self, vks: Vec<u16>) {
        *lock(&self.shared.keys) = vks.into_iter().collect();
        self.shared.gen.fetch_add(1, Ordering::SeqCst);
    }

    /// The Start-menu mask: no Mac meaning.
    pub fn mask(&self, _on: bool) {}

    /// Take digits 1 to 9 and Esc (the session picker is open).
    pub fn pick(&self, on: bool) {
        self.shared.pick.store(on, Ordering::SeqCst);
        self.shared.gen.fetch_add(1, Ordering::SeqCst);
    }

    /// Take Esc while a recording is live, so Esc cancels it from any app.
    pub fn rec(&self, on: bool) {
        self.shared.rec.store(on, Ordering::SeqCst);
        self.shared.gen.fetch_add(1, Ordering::SeqCst);
    }

    /// True while the thread runs.
    pub fn alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Let go of the hot keys, end the tap, let the lock go and end the thread (it looks every 50 ms at most).
    pub fn stop(mut self) {
        self.wanted.store(false, Ordering::SeqCst);
        let Some(t) = self.thread.take() else { return };
        for _ in 0..100 {
            if t.is_finished() {
                break;
            }
            std::thread::sleep(SLICE);
        }
        if t.is_finished() {
            let _ = t.join();
        }
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        // Dropped without `stop`: the thread sees this within a slice and ends by itself; the dropper never waits.
        self.wanted.store(false, Ordering::SeqCst);
    }
}

struct Run {
    lock_dir: PathBuf,
    tx: Sender<HookEvent>,
    sys: Arc<dyn System>,
    shared: Arc<Shared>,
    wanted: Arc<AtomicBool>,
    owner: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Stopped,
    Failed,
}

impl Run {
    fn want(&self) -> bool {
        self.wanted.load(Ordering::SeqCst)
    }

    /// Sleep up to `slices`, waking early when stopped or when the owner changed something.
    fn nap(&self, slices: u32) {
        let gen = self.shared.gen.load(Ordering::SeqCst);
        for _ in 0..slices {
            if !self.want() || self.shared.gen.load(Ordering::SeqCst) != gen {
                return;
            }
            std::thread::sleep(SLICE);
        }
    }

    /// A plan this Mac can serve now: a chord always, modifiers only with Input Monitoring.
    fn servable(&self) -> Option<Plan> {
        self.shared
            .plan()
            .filter(|p| p.way == Way::Chord || self.sys.listen_allowed())
    }

    fn sink(&self) -> Sink {
        let shared = self.shared.clone();
        let sys = self.sys.clone();
        let tx = Mutex::new(self.tx.clone());
        let st = Mutex::new(ChordState::default());
        Arc::new(move |hit, pressed| {
            let keys = lock(&shared.keys).clone();
            let evs = on_hit(&mut lock(&st), hit, pressed, &keys);
            let front = evs
                .iter()
                .any(|e| matches!(e, HookEvent::Key { down: true, .. }))
                .then(|| sys.front_pid())
                .flatten();
            let tx = lock(&tx);
            for mut ev in evs {
                if let HookEvent::Key {
                    down: true,
                    front: f,
                    ..
                } = &mut ev
                {
                    *f = front;
                }
                let _ = tx.send(ev);
            }
        })
    }

    fn serve_chord(&self, mut plan: Plan, gen: u64) -> Outcome {
        let sink = self.sink();
        if !self
            .sys
            .register(self.owner, &plan.regs, sink.clone(), &self.wanted)
        {
            return Outcome::Failed;
        }
        let _ = self.tx.send(HookEvent::Ready);
        let mut gen = gen;
        loop {
            std::thread::sleep(SLICE);
            if !self.want() {
                return Outcome::Stopped;
            }
            let g = self.shared.gen.load(Ordering::SeqCst);
            if g == gen {
                continue;
            }
            gen = g;
            match self.shared.plan() {
                Some(p) if p.way == Way::Chord => {
                    if p.regs != plan.regs {
                        if !self
                            .sys
                            .register(self.owner, &p.regs, sink.clone(), &self.wanted)
                        {
                            return Outcome::Failed;
                        }
                        plan = p;
                    }
                }
                // The binding became one this way cannot serve: say so; the owner re-arms a fresh hook.
                _ => return Outcome::Failed,
            }
        }
    }

    fn serve_tap(&self, plan: Plan, gen: u64) -> Outcome {
        let sink = self.sink();
        if !plan.regs.is_empty()
            && !self
                .sys
                .register(self.owner, &plan.regs, sink.clone(), &self.wanted)
        {
            return Outcome::Failed;
        }
        let ctx = TapCtx {
            run: self,
            sink,
            state: RefCell::new(TapState::default()),
            keys: RefCell::new(lock(&self.shared.keys).clone()),
            regs: RefCell::new(plan.regs),
            gen: Cell::new(gen),
            outcome: Cell::new(Outcome::Stopped),
            ticks: Cell::new(0),
        };
        if !self.sys.run_tap(&ctx) {
            return Outcome::Failed;
        }
        ctx.outcome.get()
    }
}

struct TapCtx<'a> {
    run: &'a Run,
    sink: Sink,
    state: RefCell<TapState>,
    keys: RefCell<HashSet<u16>>,
    regs: RefCell<Vec<Reg>>,
    gen: Cell<u64>,
    outcome: Cell<Outcome>,
    ticks: Cell<u32>,
}

/// Ticks (50 ms each) between reads of Input Monitoring while the tap runs: revoked, the tap ends within 500 ms.
const LISTEN_TICKS: u32 = 10;

impl TapCtx<'_> {
    fn send(&self, ev: HookEvent) {
        let ev = match ev {
            HookEvent::Key {
                down: true, code, ..
            } => HookEvent::Key {
                down: true,
                code,
                front: self.run.sys.front_pid(),
            },
            other => other,
        };
        let _ = self.run.tx.send(ev);
    }
}

impl TapSink for TapCtx<'_> {
    fn event(&self, kind: TapKind, keycode: u16, flags: u64) {
        let ev = on_tap(
            &mut self.state.borrow_mut(),
            kind,
            keycode,
            flags,
            &self.keys.borrow(),
            &self.regs.borrow(),
        );
        if let Some(ev) = ev {
            self.send(ev);
        }
    }

    fn ready(&self) {
        let _ = self.run.tx.send(HookEvent::Ready);
    }

    fn tick(&self, flags_now: Option<u64>) -> bool {
        if !self.run.want() {
            self.outcome.set(Outcome::Stopped);
            return false;
        }
        // Input Monitoring revoked while the tap runs: a tap that hears nothing must not keep the lock (and so the
        // window from reading the key). Failed: the owner re-arms a hook that waits without the lock (rule 313).
        let t = self.ticks.get().wrapping_add(1);
        self.ticks.set(t);
        if t.is_multiple_of(LISTEN_TICKS) && !self.run.sys.listen_allowed() {
            self.outcome.set(Outcome::Failed);
            return false;
        }
        let g = self.run.shared.gen.load(Ordering::SeqCst);
        if g != self.gen.get() {
            self.gen.set(g);
            match self.run.shared.plan() {
                Some(p) if p.way == Way::Tap => {
                    let keys = lock(&self.run.shared.keys).clone();
                    self.state.borrow_mut().down.retain(|k| keys.contains(k));
                    *self.keys.borrow_mut() = keys;
                    if p.regs != *self.regs.borrow() {
                        let ok = if p.regs.is_empty() {
                            self.run.sys.unregister(self.run.owner);
                            true
                        } else {
                            self.run.sys.register(
                                self.run.owner,
                                &p.regs,
                                self.sink.clone(),
                                &self.run.wanted,
                            )
                        };
                        if !ok {
                            self.outcome.set(Outcome::Failed);
                            return false;
                        }
                        *self.regs.borrow_mut() = p.regs;
                    }
                }
                _ => {
                    self.outcome.set(Outcome::Failed);
                    return false;
                }
            }
        }
        if let Some(flags) = flags_now {
            let gone = sweep(&mut self.state.borrow_mut(), flags);
            for ev in gone {
                self.send(ev);
            }
        }
        true
    }
}

fn run(r: Run) {
    let mut told = false;
    // A binding this Mac can serve, then the lock: a copy waiting on Input Monitoring holds nothing.
    let (lock_file, plan, gen) = loop {
        if !r.want() {
            return;
        }
        let gen = r.shared.gen.load(Ordering::SeqCst);
        if r.servable().is_none() {
            r.nap(POLL_SLICES);
            continue;
        }
        match take_lock(&r.lock_dir) {
            Ok(f) => {
                // Read again with the lock held: the owner may have changed the binding meanwhile.
                let gen = r.shared.gen.load(Ordering::SeqCst).max(gen);
                match r.servable() {
                    Some(p) => break (f, p, gen),
                    None => {
                        drop(f);
                        continue;
                    }
                }
            }
            Err(e) if is_busy(&e) => {
                if !told {
                    told = true;
                    let _ = r.tx.send(HookEvent::Waiting);
                }
                r.nap(POLL_SLICES);
            }
            Err(_) => {
                let _ = r.tx.send(HookEvent::Failed);
                return;
            }
        }
    };
    let outcome = match plan.way {
        Way::Chord => r.serve_chord(plan, gen),
        Way::Tap => r.serve_tap(plan, gen),
    };
    r.sys.unregister(r.owner);
    drop(lock_file);
    if outcome == Outcome::Failed || r.want() {
        // Failed is said once: the owner re-arms on it, so nothing trails it.
        let _ = r.tx.send(HookEvent::Failed);
    }
}

// ----- the real system -----

/// Input Monitoring is granted to this process (`CGPreflightListenEventAccess`, never asks).
pub fn listen_allowed() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { CGPreflightListenEventAccess() }
}

struct Real;

type OSStatus = i32;
type Ptr = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

type EventHandlerUPP = extern "C" fn(Ptr, Ptr, Ptr) -> OSStatus;
type CGEventTapCallBack = extern "C" fn(Ptr, u32, Ptr, Ptr) -> Ptr;

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const SIGNATURE: u32 = fourcc(b"Flrd");
const CLASS_KEYBOARD: u32 = fourcc(b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
const HOT_KEY_RELEASED: u32 = 6;
const PARAM_DIRECT_OBJECT: u32 = fourcc(b"----");
const TYPE_HOT_KEY_ID: u32 = fourcc(b"hkid");
const EVENT_NOT_HANDLED: OSStatus = -9874;

const SESSION_TAP: u32 = 1;
const HEAD_INSERT: u32 = 0;
const LISTEN_ONLY: u32 = 1;
const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xffff_fffe;
const TAP_DISABLED_BY_USER: u32 = 0xffff_ffff;
const FIELD_KEYCODE: u32 = 9;
const HID_STATE: i32 = 1;

/// How long the hook thread waits on the main thread to register hot keys.
const MAIN_WAIT: Duration = Duration::from_secs(2);

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> Ptr;
    fn InstallEventHandler(
        target: Ptr,
        handler: EventHandlerUPP,
        num_types: usize,
        list: *const EventTypeSpec,
        user: Ptr,
        out: *mut Ptr,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        code: u32,
        mods: u32,
        id: EventHotKeyID,
        target: Ptr,
        options: u32,
        out: *mut Ptr,
    ) -> OSStatus;
    fn UnregisterEventHotKey(r: Ptr) -> OSStatus;
    fn GetEventParameter(
        ev: Ptr,
        name: u32,
        desired: u32,
        actual: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: Ptr,
    ) -> OSStatus;
    fn GetEventKind(ev: Ptr) -> u32;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        mask: u64,
        callback: CGEventTapCallBack,
        user: Ptr,
    ) -> Ptr;
    fn CGEventTapEnable(tap: Ptr, enable: bool);
    fn CGEventGetIntegerValueField(ev: Ptr, field: u32) -> i64;
    fn CGEventGetFlags(ev: Ptr) -> u64;
    fn CGEventSourceFlagsState(state: i32) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: Ptr;
    fn CFMachPortCreateRunLoopSource(alloc: Ptr, port: Ptr, order: isize) -> Ptr;
    fn CFMachPortInvalidate(port: Ptr);
    fn CFRunLoopGetCurrent() -> Ptr;
    fn CFRunLoopAddSource(rl: Ptr, src: Ptr, mode: Ptr);
    fn CFRunLoopRemoveSource(rl: Ptr, src: Ptr, mode: Ptr);
    fn CFRunLoopRunInMode(mode: Ptr, seconds: f64, after_source: u8) -> i32;
    fn CFRelease(obj: *const c_void);
}

// NSWorkspace, for the app in front.
#[link(name = "AppKit", kind = "framework")]
extern "C" {}

extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, ctx: Ptr, work: extern "C" fn(Ptr));
}

/// Run `f` on the main thread, later (the main queue, drained by the app's `NSApplication`).
fn on_main(f: Box<dyn FnOnce() + Send>) {
    extern "C" fn trampoline(ctx: Ptr) {
        // SAFETY: `ctx` is the box `on_main` leaked below, handed over exactly once.
        let f = unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>) };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    }
    let ctx = Box::into_raw(Box::new(f)) as Ptr;
    // SAFETY: the main queue is a process-wide static; the trampoline takes the box back exactly once.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, ctx, trampoline) };
}

/// The main thread's answer to a register, read in slices: `None` after `total`, or as soon as the hook is no
/// longer wanted. A quit or switch-off runs on the main thread and joins the hook, so the main queue cannot answer
/// until the hook ends: waiting the whole `total` there would stall the quit for it (rule 316).
fn await_main(
    rx: &std::sync::mpsc::Receiver<bool>,
    total: Duration,
    wanted: &AtomicBool,
) -> Option<bool> {
    let start = std::time::Instant::now();
    loop {
        match rx.recv_timeout(SLICE) {
            Ok(ok) => return Some(ok),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if !wanted.load(Ordering::SeqCst) || start.elapsed() >= total {
            return None;
        }
    }
}

/// A registered hot key's handle; only the main thread touches it.
struct HotKeyRef(Ptr);
// SAFETY: created, read and unregistered on the main thread only; the mutex merely holds it.
unsafe impl Send for HotKeyRef {}

struct MainState {
    handler: bool,
    owner: u64,
    refs: Vec<HotKeyRef>,
    hits: Vec<Hit>,
    sink: Option<Sink>,
}

static MAIN: Mutex<MainState> = Mutex::new(MainState {
    handler: false,
    owner: 0,
    refs: Vec::new(),
    hits: Vec::new(),
    sink: None,
});

extern "C" fn hot_key_handler(_call: Ptr, event: Ptr, _user: Ptr) -> OSStatus {
    let r = std::panic::catch_unwind(|| {
        let mut id = EventHotKeyID {
            signature: 0,
            id: 0,
        };
        // SAFETY: `event` is the event Carbon hands this handler; `id` is a local of the size given.
        let status = unsafe {
            GetEventParameter(
                event,
                PARAM_DIRECT_OBJECT,
                TYPE_HOT_KEY_ID,
                std::ptr::null_mut(),
                std::mem::size_of::<EventHotKeyID>(),
                std::ptr::null_mut(),
                (&raw mut id).cast(),
            )
        };
        if status != 0 || id.signature != SIGNATURE {
            return false;
        }
        // SAFETY: as above.
        let pressed = match unsafe { GetEventKind(event) } {
            HOT_KEY_PRESSED => true,
            HOT_KEY_RELEASED => false,
            _ => return false,
        };
        let (hit, sink) = {
            let m = lock(&MAIN);
            (m.hits.get(id.id as usize).copied(), m.sink.clone())
        };
        match (hit, sink) {
            (Some(hit), Some(sink)) => {
                sink(hit, pressed);
                true
            }
            _ => false,
        }
    });
    if matches!(r, Ok(true)) {
        0
    } else {
        EVENT_NOT_HANDLED
    }
}

fn main_unregister(m: &mut MainState) {
    for r in m.refs.drain(..) {
        // SAFETY: a handle RegisterEventHotKey gave and nothing has unregistered yet.
        unsafe { UnregisterEventHotKey(r.0) };
    }
    m.hits.clear();
    m.sink = None;
}

/// On the main thread: install the one handler (once), then replace every hot key with `regs`.
fn main_register(owner: u64, regs: &[Reg], sink: Sink) -> bool {
    let mut m = lock(&MAIN);
    if !m.handler {
        let types = [
            EventTypeSpec {
                event_class: CLASS_KEYBOARD,
                event_kind: HOT_KEY_PRESSED,
            },
            EventTypeSpec {
                event_class: CLASS_KEYBOARD,
                event_kind: HOT_KEY_RELEASED,
            },
        ];
        let mut out: Ptr = std::ptr::null_mut();
        // SAFETY: the handler has Carbon's signature; the type list outlives the call (Carbon copies it).
        let s = unsafe {
            InstallEventHandler(
                GetApplicationEventTarget(),
                hot_key_handler,
                types.len(),
                types.as_ptr(),
                std::ptr::null_mut(),
                &mut out,
            )
        };
        if s != 0 {
            log::warn!("dictation key: no Carbon handler (status {s})");
            return false;
        }
        m.handler = true;
    }
    main_unregister(&mut m);
    let mut ok = true;
    for (i, r) in regs.iter().enumerate() {
        let mut out: Ptr = std::ptr::null_mut();
        // SAFETY: plain values and an out pointer to a local.
        let s = unsafe {
            RegisterEventHotKey(
                u32::from(r.keycode),
                r.mods,
                EventHotKeyID {
                    signature: SIGNATURE,
                    id: i as u32,
                },
                GetApplicationEventTarget(),
                0,
                &mut out,
            )
        };
        if s == 0 && !out.is_null() {
            m.refs.push(HotKeyRef(out));
        } else if r.hit == Hit::Binding {
            log::warn!("dictation key: the chord is taken by another app (status {s})");
            ok = false;
        } else {
            log::debug!("dictation key: a picker key is taken by another app (status {s})");
        }
    }
    if !ok {
        main_unregister(&mut m);
        return false;
    }
    m.owner = owner;
    m.hits = regs.iter().map(|r| r.hit).collect();
    m.sink = Some(sink);
    true
}

/// The tap's user data: the sink and the port (to enable it again).
struct TapUser<'a> {
    sink: &'a dyn TapSink,
    port: Cell<Ptr>,
}

extern "C" fn tap_callback(_proxy: Ptr, ty: u32, event: Ptr, user: Ptr) -> Ptr {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `user` is the `TapUser` `run_tap` keeps alive while the tap's source is on its run loop, and
        // the callback runs only inside that loop, on that thread.
        let u = unsafe { &*(user as *const TapUser<'_>) };
        let kind = match ty {
            TAP_DISABLED_BY_TIMEOUT | TAP_DISABLED_BY_USER => {
                // SAFETY: the live port of this tap.
                unsafe { CGEventTapEnable(u.port.get(), true) };
                return;
            }
            KEY_DOWN => TapKind::KeyDown,
            KEY_UP => TapKind::KeyUp,
            FLAGS_CHANGED => TapKind::Flags,
            _ => return,
        };
        // SAFETY: `event` is the event this call is about.
        let (kc, flags) = unsafe {
            (
                CGEventGetIntegerValueField(event, FIELD_KEYCODE) as u16,
                CGEventGetFlags(event),
            )
        };
        u.sink.event(kind, kc, flags);
    }));
    event
}

impl System for Real {
    fn listen_allowed(&self) -> bool {
        listen_allowed()
    }

    fn register(&self, owner: u64, regs: &[Reg], sink: Sink, wanted: &AtomicBool) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let regs = regs.to_vec();
        on_main(Box::new(move || {
            let _ = tx.send(main_register(owner, &regs, sink));
        }));
        match await_main(&rx, MAIN_WAIT, wanted) {
            Some(ok) => ok,
            None => {
                // The main thread may still run it: what it registers late is let go right after.
                self.unregister(owner);
                false
            }
        }
    }

    fn unregister(&self, owner: u64) {
        on_main(Box::new(move || {
            let mut m = lock(&MAIN);
            if m.owner == owner {
                main_unregister(&mut m);
            }
        }));
    }

    fn run_tap(&self, sink: &dyn TapSink) -> bool {
        // `user` (and so `sink`) outlives the run loop below, the only place the callback can run: the source is
        // removed and the port invalidated before this function returns.
        let user = Box::new(TapUser {
            sink,
            port: Cell::new(std::ptr::null_mut()),
        });
        let mask = (1u64 << KEY_DOWN) | (1u64 << KEY_UP) | (1u64 << FLAGS_CHANGED);
        // SAFETY: a listen-only session tap with a callback of the right signature and a user pointer kept alive.
        let port = unsafe {
            CGEventTapCreate(
                SESSION_TAP,
                HEAD_INSERT,
                LISTEN_ONLY,
                mask,
                tap_callback,
                (&*user as *const TapUser<'_>) as Ptr,
            )
        };
        if port.is_null() {
            return false;
        }
        user.port.set(port);
        // SAFETY: CoreFoundation calls on objects this function created and releases below.
        unsafe {
            let src = CFMachPortCreateRunLoopSource(std::ptr::null_mut(), port, 0);
            if src.is_null() {
                CFMachPortInvalidate(port);
                CFRelease(port);
                return false;
            }
            let rl = CFRunLoopGetCurrent();
            CFRunLoopAddSource(rl, src, kCFRunLoopDefaultMode);
            CGEventTapEnable(port, true);
            sink.ready();
            loop {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 1);
                if !sink.tick(Some(CGEventSourceFlagsState(HID_STATE))) {
                    break;
                }
            }
            CGEventTapEnable(port, false);
            CFRunLoopRemoveSource(rl, src, kCFRunLoopDefaultMode);
            CFMachPortInvalidate(port);
            CFRelease(src);
            CFRelease(port);
        }
        drop(user);
        true
    }

    fn front_pid(&self) -> Option<u32> {
        front_pid()
    }
}

/// The process of the app in front (`NSWorkspace.frontmostApplication`).
fn front_pid() -> Option<u32> {
    use objc2::runtime::{AnyClass, AnyObject};
    objc2::rc::autoreleasepool(|_| {
        let cls = AnyClass::get(c"NSWorkspace")?;
        // SAFETY: class and instance messages NSWorkspace and NSRunningApplication answer, with their types.
        unsafe {
            let ws: *mut AnyObject = objc2::msg_send![cls, sharedWorkspace];
            let ws = ws.as_ref()?;
            let app: *mut AnyObject = objc2::msg_send![ws, frontmostApplication];
            let app = app.as_ref()?;
            let pid: i32 = objc2::msg_send![app, processIdentifier];
            u32::try_from(pid).ok().filter(|p| *p != 0)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::{default_hotkey, DictMode, Hotkey, KeyAction, KeyConfig};
    use crate::keyfeed::{keys_for, KeyFeed};
    use std::sync::mpsc::{channel, Receiver};

    fn hk(code: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Hotkey {
        Hotkey {
            code: code.into(),
            ctrl,
            alt,
            shift,
            meta,
        }
    }

    fn keys_of(h: &Hotkey) -> HashSet<u16> {
        keys_for(h).expect("a Windows key").into_iter().collect()
    }

    fn cfg(h: &Hotkey, mode: DictMode) -> KeyConfig {
        KeyConfig {
            hotkey: h.clone(),
            mode,
            hold_ms: None,
        }
    }

    // Right Option down and up, as the tap's flags say it (the family bit and the right side's bit).
    const RIGHT_OPTION_DOWN: u64 = 0x8_0000 | 0x40;
    const LEFT_OPTION_DOWN: u64 = 0x8_0000 | 0x20;
    const CTRL_CMD_DOWN: u64 = 0x4_0000 | 0x0001 | 0x10_0000 | 0x0008;
    const NONE: u64 = 0x100; // NX_NONCOALESCEDMASK only: no modifier held.

    #[test]
    fn every_binding_key_maps_to_the_mac_key_in_its_place_and_back() {
        // The table is a bijection, so a tap's key code reads back as the binding's virtual key.
        for (vk, kc) in KEYCODES {
            assert_eq!(mac_keycode(*vk), Some(*kc));
            assert_eq!(vk_of_mac_keycode(*kc), Some(*vk), "key code {kc:#x}");
        }
        assert_eq!(mac_keycode(0x44), Some(0x02), "D is kVK_ANSI_D");
        assert_eq!(mac_keycode(0x78), Some(0x65), "F9 is kVK_F9");
        assert_eq!(
            mac_keycode(VK_RMENU),
            Some(0x3d),
            "Right Alt is Right Option"
        );
        assert_eq!(mac_keycode(VK_LWIN), Some(0x37), "Win is Command");
        assert_eq!(mac_keycode(0x13), None, "a Mac has no Pause key");
        assert_eq!(mac_keycode(0x91), None, "nor Scroll Lock");
        assert_eq!(mac_keycode(0x84), None, "nor F21");
    }

    #[test]
    fn a_chord_with_a_key_is_one_carbon_hot_key_with_its_modifiers() {
        let p = plan(
            &keys_of(&hk("KeyD", true, false, true, false)),
            false,
            false,
        )
        .unwrap();
        assert_eq!(p.way, Way::Chord);
        assert_eq!(
            p.regs,
            vec![Reg {
                keycode: 0x02,
                mods: 0x1000 | 0x200,
                hit: Hit::Binding
            }]
        );
        let f9 = plan(
            &keys_of(&hk("F9", false, false, false, false)),
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            (f9.way, f9.regs[0].keycode, f9.regs[0].mods),
            (Way::Chord, 0x65, 0)
        );
        // Option and Command are Carbon's optionKey and cmdKey.
        let oc = plan(
            &keys_of(&hk("KeyK", false, true, false, true)),
            false,
            false,
        )
        .unwrap();
        assert_eq!(oc.regs[0].mods, 0x800 | 0x100);
    }

    #[test]
    fn a_bare_modifier_or_a_chord_of_modifiers_needs_the_tap_and_registers_nothing() {
        let d = plan(&keys_of(&default_hotkey()), false, false).unwrap();
        assert_eq!(
            (d.way, d.regs.len()),
            (Way::Tap, 0),
            "Ctrl+Cmd, the default"
        );
        let ro = plan(
            &keys_of(&hk("AltRight", false, false, false, false)),
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            (ro.way, ro.regs.len()),
            (Way::Tap, 0),
            "Right Option, AltGr's key"
        );
    }

    #[test]
    fn a_key_a_mac_keyboard_lacks_or_no_key_at_all_cannot_be_served() {
        assert_eq!(
            plan(
                &keys_of(&hk("Pause", false, false, false, false)),
                false,
                false
            ),
            None
        );
        assert_eq!(
            plan(
                &keys_of(&hk("ScrollLock", true, false, false, false)),
                false,
                false
            ),
            None
        );
        assert_eq!(plan(&HashSet::new(), true, true), None);
    }

    #[test]
    fn the_picker_takes_digits_and_esc_alone_or_with_the_bindings_own_modifiers_only() {
        let p = plan(&keys_of(&default_hotkey()), true, false).unwrap();
        // Ctrl+Cmd: no modifier, Control, Command, Control+Command; the digit row and the keypad.
        let digit_two: Vec<u32> = p
            .regs
            .iter()
            .filter(|r| r.keycode == 0x13)
            .map(|r| r.mods)
            .collect();
        assert_eq!(digit_two, vec![0, 0x1000, 0x100, 0x1100]);
        assert!(
            p.regs
                .iter()
                .any(|r| r.keycode == 0x54 && r.hit == Hit::Digit(2)),
            "keypad 2"
        );
        assert!(
            !p.regs.iter().any(|r| r.mods & (0x800 | 0x200) != 0),
            "never with Option or Shift, which this binding does not name: Option+2 types @"
        );
        assert!(
            !p.regs.iter().any(|r| r.keycode == 0x1d),
            "0 is never the picker's"
        );
        assert_eq!(p.regs.iter().filter(|r| r.hit == Hit::Esc).count(), 4);
        // A live recording takes Esc only.
        let rec = plan(&keys_of(&default_hotkey()), false, true).unwrap();
        assert!(rec.regs.iter().all(|r| r.hit == Hit::Esc));
        assert_eq!(rec.regs.len(), 4);
        // Right Option: Esc alone or with Option (holding the key and pressing Esc cancels).
        let ro = plan(
            &keys_of(&hk("AltRight", false, false, false, false)),
            false,
            true,
        )
        .unwrap();
        let mods: Vec<u32> = ro.regs.iter().map(|r| r.mods).collect();
        assert_eq!(mods, vec![0, 0x800]);
    }

    #[test]
    fn a_binding_that_is_also_a_picker_key_is_the_binding_first() {
        let keys = keys_of(&hk("Digit2", true, true, false, false));
        let p = plan(&keys, true, false).unwrap();
        let two: Vec<&Reg> = p
            .regs
            .iter()
            .filter(|r| r.keycode == 0x13 && r.mods == 0x1000 | 0x800)
            .collect();
        assert_eq!(two.len(), 1);
        assert_eq!(two[0].hit, Hit::Binding);
    }

    #[test]
    fn carbons_press_and_release_read_as_the_windows_hooks_lines_and_dictate() {
        let h = hk("KeyD", true, false, true, false);
        let keys = keys_of(&h);
        let mut st = ChordState::default();
        let down = on_hit(&mut st, Hit::Binding, true, &keys);
        let codes: Vec<(bool, String)> = down
            .iter()
            .map(|e| match e {
                HookEvent::Key { down, code, .. } => (*down, code.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            codes,
            vec![
                (true, "ControlLeft".to_string()),
                (true, "ShiftLeft".to_string()),
                (true, "KeyD".to_string())
            ]
        );
        // A second press while held is the key's repeat only.
        let again = on_hit(&mut st, Hit::Binding, true, &keys);
        assert_eq!(again.len(), 1);
        let up = on_hit(&mut st, Hit::Binding, false, &keys);
        assert_eq!(up.len(), 3);
        assert!(matches!(&up[0], HookEvent::Key { down: false, code, .. } if code == "KeyD"));
        assert!(
            on_hit(&mut st, Hit::Binding, false, &keys).is_empty(),
            "one release only"
        );
        // Through the shared feed: hold starts on the press and stops on the release.
        for (mode, want) in [
            (DictMode::Hold, vec![KeyAction::Start, KeyAction::Stop]),
            (DictMode::Toggle, vec![KeyAction::Start]),
        ] {
            let c = cfg(&h, mode);
            let mut feed = KeyFeed::new();
            let mut st = ChordState::default();
            let mut acts = Vec::new();
            let mut at = 0.0;
            for pressed in [true, false] {
                for ev in on_hit(&mut st, Hit::Binding, pressed, &keys) {
                    at += 30.0;
                    acts.extend(feed.feed(&ev, at, &c));
                }
            }
            assert_eq!(acts, want, "{mode:?}");
        }
    }

    #[test]
    fn a_picker_digit_or_esc_is_said_on_its_press_only() {
        let keys = keys_of(&default_hotkey());
        let mut st = ChordState::default();
        assert_eq!(
            on_hit(&mut st, Hit::Digit(3), true, &keys),
            vec![HookEvent::Digit(3)]
        );
        assert!(on_hit(&mut st, Hit::Digit(3), false, &keys).is_empty());
        assert_eq!(on_hit(&mut st, Hit::Esc, true, &keys), vec![HookEvent::Esc]);
        assert!(on_hit(&mut st, Hit::Esc, false, &keys).is_empty());
    }

    fn tap_seq(keys: &HashSet<u16>, evs: &[(TapKind, u16, u64)], carbon: &[Reg]) -> Vec<HookEvent> {
        let mut st = TapState::default();
        evs.iter()
            .filter_map(|(k, kc, f)| on_tap(&mut st, *k, *kc, *f, keys, carbon))
            .collect()
    }

    #[test]
    fn right_option_held_alone_dictates_and_its_up_stops() {
        let h = hk("AltRight", false, false, false, false);
        let keys = keys_of(&h);
        let lines = tap_seq(
            &keys,
            &[
                (TapKind::Flags, 0x3d, RIGHT_OPTION_DOWN),
                (TapKind::Flags, 0x3d, NONE),
            ],
            &[],
        );
        assert_eq!(
            lines,
            vec![
                HookEvent::Key {
                    down: true,
                    code: "AltRight".into(),
                    front: None
                },
                HookEvent::Key {
                    down: false,
                    code: "AltRight".into(),
                    front: None
                },
            ]
        );
        let c = cfg(&h, DictMode::Hold);
        let mut feed = KeyFeed::new();
        assert_eq!(feed.feed(&lines[0], 0.0, &c), None, "arming");
        assert_eq!(feed.tick(300.0, &c), Some(KeyAction::Start));
        assert_eq!(feed.feed(&lines[1], 900.0, &c), Some(KeyAction::Stop));
    }

    #[test]
    fn option_typing_at_on_a_norwegian_mac_never_dictates_and_never_says_which_key() {
        // Norwegian: Option+2 is @. Right Option is the binding (AltGr's key), 2 goes down while it is held.
        let h = hk("AltRight", false, false, false, false);
        let keys = keys_of(&h);
        let lines = tap_seq(
            &keys,
            &[
                (TapKind::Flags, 0x3d, RIGHT_OPTION_DOWN),
                (TapKind::KeyDown, 0x13, RIGHT_OPTION_DOWN),
                (TapKind::KeyUp, 0x13, RIGHT_OPTION_DOWN),
                (TapKind::Flags, 0x3d, NONE),
            ],
            &[],
        );
        assert_eq!(lines[1], HookEvent::Other, "another key, never which");
        assert_eq!(lines.len(), 3, "a key's up says nothing");
        let c = cfg(&h, DictMode::Hold);
        let mut feed = KeyFeed::new();
        let mut acts = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            acts.extend(feed.feed(l, 40.0 * i as f64, &c));
            acts.extend(feed.tick(40.0 * i as f64 + 5.0, &c));
        }
        acts.extend(feed.tick(1000.0, &c));
        assert!(acts.is_empty(), "{acts:?}");
    }

    #[test]
    fn a_key_with_no_binding_key_held_is_not_read_at_all() {
        let keys = keys_of(&hk("AltRight", false, false, false, false));
        let lines = tap_seq(
            &keys,
            &[
                (TapKind::KeyDown, 0x00, NONE),
                (TapKind::Flags, 0x3a, LEFT_OPTION_DOWN),
                (TapKind::KeyDown, 0x13, LEFT_OPTION_DOWN),
                (TapKind::Flags, 0x3a, NONE),
            ],
            &[],
        );
        assert!(
            lines.is_empty(),
            "Left Option is not Right Option: {lines:?}"
        );
    }

    #[test]
    fn ctrl_cmd_is_the_default_chord_of_modifiers_read_from_both_sides() {
        let h = default_hotkey();
        let keys = keys_of(&h);
        let lines = tap_seq(
            &keys,
            &[
                (TapKind::Flags, 0x3b, 0x4_0000 | 0x0001),
                (TapKind::Flags, 0x37, CTRL_CMD_DOWN),
                (TapKind::Flags, 0x37, 0x4_0000 | 0x0001),
                (TapKind::Flags, 0x3b, NONE),
            ],
            &[],
        );
        let names: Vec<(bool, String)> = lines
            .iter()
            .map(|e| match e {
                HookEvent::Key { down, code, .. } => (*down, code.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                (true, "ControlLeft".into()),
                (true, "MetaLeft".into()),
                (false, "MetaLeft".into()),
                (false, "ControlLeft".into())
            ]
        );
        let c = cfg(&h, DictMode::Hold);
        let mut feed = KeyFeed::new();
        assert_eq!(feed.feed(&lines[0], 0.0, &c), None);
        assert_eq!(feed.feed(&lines[1], 20.0, &c), None);
        assert_eq!(feed.tick(300.0, &c), Some(KeyAction::Start));
        assert_eq!(feed.feed(&lines[2], 800.0, &c), Some(KeyAction::Stop));
        // Ctrl+Cmd+F (full screen) is another key: it never dictates.
        let chord = tap_seq(
            &keys,
            &[
                (TapKind::Flags, 0x3b, 0x4_0000 | 0x0001),
                (TapKind::Flags, 0x37, CTRL_CMD_DOWN),
                (TapKind::KeyDown, 0x03, CTRL_CMD_DOWN),
            ],
            &[],
        );
        assert_eq!(chord[2], HookEvent::Other);
    }

    #[test]
    fn a_key_carbon_has_taken_is_carbons_and_not_another_key() {
        let keys = keys_of(&default_hotkey());
        let p = plan(&keys, false, true).unwrap();
        let lines = tap_seq(
            &keys,
            &[
                (TapKind::Flags, 0x3b, 0x4_0000 | 0x0001),
                (TapKind::KeyDown, KC_ESC, 0x4_0000 | 0x0001),
            ],
            &p.regs,
        );
        assert_eq!(
            lines.len(),
            1,
            "Esc with Control held is Carbon's: {lines:?}"
        );
        // Esc with Option (not the binding's) is another key.
        let other = tap_seq(
            &keys,
            &[
                (TapKind::Flags, 0x3b, 0x4_0000 | 0x0001),
                (
                    TapKind::KeyDown,
                    KC_ESC,
                    0x4_0000 | 0x0001 | LEFT_OPTION_DOWN,
                ),
            ],
            &p.regs,
        );
        assert_eq!(other[1], HookEvent::Other);
    }

    #[test]
    fn caps_lock_and_fn_are_another_key_while_the_binding_is_held() {
        let keys = keys_of(&hk("AltRight", false, false, false, false));
        let lines = tap_seq(
            &keys,
            &[
                (TapKind::Flags, 0x3d, RIGHT_OPTION_DOWN),
                (TapKind::Flags, KC_CAPS, RIGHT_OPTION_DOWN | 0x1_0000),
                (TapKind::Flags, KC_FN, RIGHT_OPTION_DOWN | 0x80_0000),
            ],
            &[],
        );
        assert_eq!(&lines[1..], &[HookEvent::Other, HookEvent::Other]);
    }

    #[test]
    fn an_up_the_tap_missed_is_found_from_the_systems_flags() {
        let keys = keys_of(&hk("AltRight", false, false, false, false));
        let mut st = TapState::default();
        on_tap(&mut st, TapKind::Flags, 0x3d, RIGHT_OPTION_DOWN, &keys, &[]);
        assert!(sweep(&mut st, RIGHT_OPTION_DOWN).is_empty(), "still held");
        assert!(
            sweep(&mut st, LEFT_OPTION_DOWN).len() == 1,
            "the right side is up"
        );
        assert!(sweep(&mut st, NONE).is_empty(), "said once");
        // An event with no side bits reads the family bit.
        let mut st = TapState::default();
        assert!(on_tap(&mut st, TapKind::Flags, 0x3d, 0x8_0000, &keys, &[]).is_some());
    }

    #[test]
    fn the_settings_problem_names_input_monitoring_only_for_a_modifier_key_without_it() {
        let ks = |h: &Hotkey| keys_for(h);
        let d = ks(&default_hotkey());
        assert_eq!(problem_for(d.as_deref(), false), Some(INPUT_MONITORING));
        assert_eq!(problem_for(d.as_deref(), true), None);
        let chord = ks(&hk("KeyD", true, false, true, false));
        assert_eq!(
            problem_for(chord.as_deref(), false),
            None,
            "a chord needs nothing"
        );
        let pause = ks(&hk("Pause", false, false, false, false));
        assert_eq!(problem_for(pause.as_deref(), true), Some(NO_MAC_KEY));
        assert_eq!(
            problem_for(None, false),
            None,
            "no binding, no problem to name"
        );
        assert!(
            INPUT_MONITORING.contains("System Settings > Privacy & Security > Input Monitoring")
        );
    }

    // ----- the thread, on a fake system -----

    #[derive(Default)]
    struct Fake {
        listen: AtomicBool,
        refuse: AtomicBool,
        tap_fails: AtomicBool,
        regs: Mutex<Option<(u64, Vec<Reg>)>>,
        sink: Mutex<Option<Sink>>,
        unregistered: Mutex<Vec<u64>>,
        taps: AtomicU64,
        tap_in: Mutex<Option<Receiver<(TapKind, u16, u64)>>>,
        flags_now: AtomicU64,
    }

    impl System for Fake {
        fn listen_allowed(&self) -> bool {
            self.listen.load(Ordering::SeqCst)
        }
        fn register(&self, owner: u64, regs: &[Reg], sink: Sink, _wanted: &AtomicBool) -> bool {
            if self.refuse.load(Ordering::SeqCst) {
                return false;
            }
            *lock(&self.regs) = Some((owner, regs.to_vec()));
            *lock(&self.sink) = Some(sink);
            true
        }
        fn unregister(&self, owner: u64) {
            let mut r = lock(&self.regs);
            if r.as_ref().is_some_and(|(o, _)| *o == owner) {
                *r = None;
            }
            lock(&self.unregistered).push(owner);
        }
        fn run_tap(&self, sink: &dyn TapSink) -> bool {
            if self.tap_fails.load(Ordering::SeqCst) {
                return false;
            }
            self.taps.fetch_add(1, Ordering::SeqCst);
            let rx = lock(&self.tap_in).take();
            sink.ready();
            loop {
                if let Some(rx) = &rx {
                    while let Ok((k, kc, f)) = rx.try_recv() {
                        sink.event(k, kc, f);
                    }
                }
                if !sink.tick(Some(self.flags_now.load(Ordering::SeqCst))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            true
        }
        fn front_pid(&self) -> Option<u32> {
            Some(4242)
        }
    }

    impl Fake {
        fn press(&self, hit: Hit, pressed: bool) {
            let s = lock(&self.sink).clone().expect("registered");
            s(hit, pressed);
        }
        fn regs(&self) -> Option<Vec<Reg>> {
            lock(&self.regs).as_ref().map(|(_, r)| r.clone())
        }
    }

    fn vks(h: &Hotkey) -> Vec<u16> {
        keys_for(h).unwrap()
    }

    fn next(rx: &Receiver<HookEvent>) -> HookEvent {
        rx.recv_timeout(Duration::from_secs(10))
            .expect("a line from the hook")
    }

    fn quiet(rx: &Receiver<HookEvent>, ms: u64) -> Option<HookEvent> {
        rx.recv_timeout(Duration::from_millis(ms)).ok()
    }

    fn until(what: &str, f: impl Fn() -> bool) {
        for _ in 0..1000 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("never: {what}");
    }

    #[test]
    fn a_chord_registers_says_ready_and_reports_its_keys_with_the_app_in_front() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(Fake::default());
        let (tx, rx) = channel();
        let hook = KeyHook::start_with(dir.path().to_path_buf(), tx, sys.clone());
        assert_eq!(quiet(&rx, 150), None, "nothing before the keys are set");
        hook.keys(vks(&hk("KeyD", true, false, true, false)));
        assert_eq!(next(&rx), HookEvent::Ready);
        assert_eq!(sys.regs().unwrap().len(), 1);
        assert!(
            !sys.listen.load(Ordering::SeqCst),
            "a chord never needed Input Monitoring"
        );
        assert_eq!(sys.taps.load(Ordering::SeqCst), 0, "and no tap");
        sys.press(Hit::Binding, true);
        assert_eq!(
            next(&rx),
            HookEvent::Key {
                down: true,
                code: "ControlLeft".into(),
                front: Some(4242)
            }
        );
        next(&rx);
        assert_eq!(
            next(&rx),
            HookEvent::Key {
                down: true,
                code: "KeyD".into(),
                front: Some(4242)
            }
        );
        sys.press(Hit::Binding, false);
        assert_eq!(
            next(&rx),
            HookEvent::Key {
                down: false,
                code: "KeyD".into(),
                front: None
            }
        );
        // The picker opens: its digits and Esc are registered too, and a digit comes through.
        hook.pick(true);
        until("the picker keys are registered", || {
            sys.regs()
                .is_some_and(|r| r.iter().any(|x| x.hit == Hit::Digit(9)))
        });
        sys.press(Hit::Digit(4), true);
        // The two modifier ups come first.
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(next(&rx));
        }
        assert_eq!(seen[2], HookEvent::Digit(4));
        hook.pick(false);
        until("the picker keys are let go", || {
            sys.regs().is_some_and(|r| r.len() == 1)
        });
        assert!(hook.alive());
        hook.stop();
        assert_eq!(sys.regs(), None, "stop lets the hot keys go");
        assert!(take_lock(dir.path()).is_ok(), "and the lock");
        assert_eq!(quiet(&rx, 50), None, "a stop says nothing");
    }

    #[test]
    fn a_modifier_key_waits_silently_for_input_monitoring_and_asks_for_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(Fake::default());
        let (tx, rx) = channel();
        let hook = KeyHook::start_with(dir.path().to_path_buf(), tx, sys.clone());
        hook.keys(vks(&default_hotkey()));
        assert_eq!(
            quiet(&rx, 700),
            None,
            "no Ready, no Failed, no Waiting: the window reads the key"
        );
        assert_eq!(
            sys.taps.load(Ordering::SeqCst),
            0,
            "no tap is made without the permission"
        );
        assert!(
            take_lock(dir.path()).is_ok(),
            "and the lock is not held while it waits"
        );
        // Granted in System Settings: the next poll taps.
        sys.listen.store(true, Ordering::SeqCst);
        assert_eq!(next(&rx), HookEvent::Ready);
        assert_eq!(sys.taps.load(Ordering::SeqCst), 1);
        hook.stop();
    }

    #[test]
    fn the_tap_reports_the_binding_says_other_and_sweeps_a_lost_up() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(Fake::default());
        sys.listen.store(true, Ordering::SeqCst);
        let (etx, erx) = channel();
        *lock(&sys.tap_in) = Some(erx);
        sys.flags_now.store(RIGHT_OPTION_DOWN, Ordering::SeqCst);
        let (tx, rx) = channel();
        let hook = KeyHook::start_with(dir.path().to_path_buf(), tx, sys.clone());
        hook.keys(vks(&hk("AltRight", false, false, false, false)));
        assert_eq!(next(&rx), HookEvent::Ready);
        assert_eq!(sys.regs(), None, "nothing registered with the picker shut");
        etx.send((TapKind::Flags, 0x3d, RIGHT_OPTION_DOWN)).unwrap();
        assert_eq!(
            next(&rx),
            HookEvent::Key {
                down: true,
                code: "AltRight".into(),
                front: Some(4242)
            }
        );
        etx.send((TapKind::KeyDown, 0x13, RIGHT_OPTION_DOWN))
            .unwrap();
        assert_eq!(next(&rx), HookEvent::Other);
        // A recording is live: Esc is Carbon's.
        hook.rec(true);
        until("Esc is registered", || {
            sys.regs()
                .is_some_and(|r| r.iter().all(|x| x.hit == Hit::Esc))
        });
        sys.press(Hit::Esc, true);
        assert_eq!(next(&rx), HookEvent::Esc);
        hook.rec(false);
        until("Esc is let go", || sys.regs().is_none());
        // The up was lost (secure input, a disabled tap): the system's flags find it.
        sys.flags_now.store(NONE, Ordering::SeqCst);
        assert_eq!(
            next(&rx),
            HookEvent::Key {
                down: false,
                code: "AltRight".into(),
                front: None
            }
        );
        hook.stop();
        assert!(take_lock(dir.path()).is_ok());
    }

    #[test]
    fn input_monitoring_revoked_while_the_tap_runs_ends_it_lets_the_lock_go_and_waits_again() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(Fake::default());
        sys.listen.store(true, Ordering::SeqCst);
        let (tx, rx) = channel();
        let hook = KeyHook::start_with(dir.path().to_path_buf(), tx, sys.clone());
        hook.keys(vks(&default_hotkey()));
        assert_eq!(next(&rx), HookEvent::Ready);
        assert!(take_lock(dir.path()).is_err(), "the tap holds the lock");
        // Revoked in System Settings: the tap ends, says Failed once and lets the lock go.
        sys.listen.store(false, Ordering::SeqCst);
        assert_eq!(next(&rx), HookEvent::Failed);
        until("the thread ends", || !hook.alive());
        assert!(take_lock(dir.path()).is_ok(), "the lock is let go");
        // The owner re-arms: the fresh hook waits silently, holding nothing.
        let (tx, rx) = channel();
        let again = KeyHook::start_with(dir.path().to_path_buf(), tx, sys.clone());
        again.keys(vks(&default_hotkey()));
        assert_eq!(quiet(&rx, 300), None, "no Ready without the permission");
        assert!(take_lock(dir.path()).is_ok(), "and no lock held");
        again.stop();
    }

    #[test]
    fn a_second_copy_waits_and_takes_over_when_the_first_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(Fake::default());
        let (tx1, rx1) = channel();
        let first = KeyHook::start_with(dir.path().to_path_buf(), tx1, sys.clone());
        first.keys(vks(&hk("F9", false, false, false, false)));
        assert_eq!(next(&rx1), HookEvent::Ready);
        let (tx2, rx2) = channel();
        let second = KeyHook::start_with(dir.path().to_path_buf(), tx2, sys.clone());
        second.keys(vks(&hk("F9", false, false, false, false)));
        assert_eq!(next(&rx2), HookEvent::Waiting);
        assert_eq!(quiet(&rx2, 100), None, "Waiting is said once");
        first.stop();
        assert_eq!(next(&rx2), HookEvent::Ready);
        assert!(
            sys.regs().is_some(),
            "the first's let-go left the second's hot key alone"
        );
        second.stop();
    }

    #[test]
    fn a_binding_this_way_cannot_serve_says_failed_once_and_lets_everything_go() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(Fake::default());
        let (tx, rx) = channel();
        let hook = KeyHook::start_with(dir.path().to_path_buf(), tx, sys.clone());
        hook.keys(vks(&hk("F9", false, false, false, false)));
        assert_eq!(next(&rx), HookEvent::Ready);
        // Now a modifier key: the chord's hook cannot serve it; the owner re-arms a fresh one.
        hook.keys(vks(&default_hotkey()));
        assert_eq!(next(&rx), HookEvent::Failed);
        assert_eq!(quiet(&rx, 100), None, "once");
        until("the thread ends", || !hook.alive());
        assert_eq!(sys.regs(), None);
        assert!(take_lock(dir.path()).is_ok());
    }

    #[test]
    fn a_chord_another_app_holds_or_a_tap_that_cannot_be_made_is_failed() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(Fake::default());
        sys.refuse.store(true, Ordering::SeqCst);
        let (tx, rx) = channel();
        let hook = KeyHook::start_with(dir.path().to_path_buf(), tx, sys.clone());
        hook.keys(vks(&hk("F9", false, false, false, false)));
        assert_eq!(next(&rx), HookEvent::Failed);
        until("the thread ends", || !hook.alive());
        assert!(take_lock(dir.path()).is_ok(), "the lock is let go");
        let sys = Arc::new(Fake::default());
        sys.listen.store(true, Ordering::SeqCst);
        sys.tap_fails.store(true, Ordering::SeqCst);
        let (tx, rx) = channel();
        let hook = KeyHook::start_with(dir.path().to_path_buf(), tx, sys);
        hook.keys(vks(&default_hotkey()));
        assert_eq!(next(&rx), HookEvent::Failed);
        until("the thread ends", || !hook.alive());
    }

    #[test]
    fn a_hook_dropped_without_stop_ends_and_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(Fake::default());
        let (tx, rx) = channel();
        let hook = KeyHook::start_with(dir.path().to_path_buf(), tx, sys.clone());
        hook.keys(vks(&hk("F9", false, false, false, false)));
        assert_eq!(next(&rx), HookEvent::Ready);
        drop(hook);
        until("the hot key is let go", || sys.regs().is_none());
        until("the lock is let go", || take_lock(dir.path()).is_ok());
    }

    #[test]
    fn a_register_the_main_thread_cannot_answer_gives_up_as_soon_as_the_hook_is_stopped() {
        // The quit runs on the main thread and joins the hook: waiting MAIN_WAIT for the main queue would stall it.
        let (_tx, rx) = channel::<bool>();
        let wanted = Arc::new(AtomicBool::new(true));
        let w = wanted.clone();
        let flip = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            w.store(false, Ordering::SeqCst);
        });
        let start = std::time::Instant::now();
        assert_eq!(await_main(&rx, MAIN_WAIT, &wanted), None);
        let took = start.elapsed();
        flip.join().unwrap();
        assert!(
            took < MAIN_WAIT / 2,
            "gave up after {took:?}, not soon after the stop"
        );
        // An answer is taken at once, and a hook still wanted waits for it up to the bound.
        let (tx, rx) = channel();
        tx.send(true).unwrap();
        assert_eq!(
            await_main(&rx, MAIN_WAIT, &AtomicBool::new(true)),
            Some(true)
        );
        let (_tx, rx) = channel::<bool>();
        let start = std::time::Instant::now();
        assert_eq!(
            await_main(&rx, Duration::from_millis(100), &AtomicBool::new(true)),
            None
        );
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn the_lock_is_held_by_one_hook_at_a_time_on_a_mac() {
        let dir = tempfile::tempdir().unwrap();
        let first = take_lock(dir.path()).expect("the first takes the lock");
        let second = take_lock(dir.path());
        assert!(is_busy(&second.unwrap_err()));
        drop(first);
        assert!(take_lock(dir.path()).is_ok());
    }

    #[test]
    fn the_c_layouts_are_carbons_own() {
        assert_eq!(std::mem::size_of::<EventHotKeyID>(), 8);
        assert_eq!(std::mem::size_of::<EventTypeSpec>(), 8);
        assert_eq!(CLASS_KEYBOARD, 0x6b65_7962);
        assert_eq!(PARAM_DIRECT_OBJECT, 0x2d2d_2d2d);
    }

    #[test]
    fn reading_input_monitoring_asks_nothing_and_answers() {
        // CGPreflightListenEventAccess only reads: it never shows the system's question.
        let a = listen_allowed();
        assert_eq!(a, listen_allowed());
    }
}
