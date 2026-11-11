//! The dictate-from-any-app service (Task 21). Ported from Prism Terminal's `src/main/dictateAnywhere.ts` and
//! `core/main/dictationClient.ts` at e5848cb.
//!
//! A pure state machine like the controller (D1): `Service::on(SvcEvent, now_ms) -> Vec<SvcEffect>` holds every rule
//! and touches nothing: no hook, no pipe, no window, no clock. The app (`dictation_ctl.rs`, Task 22) executes the
//! effects and feeds what happened back as events.
//!
//! WHAT IT HOLDS
//! - Roles (N17): the copy whose hook holds the lock is the SERVICE (it records and runs the link service); a copy that
//!   is waiting or failed is a CLIENT (it answers on the link); with no other copy the role is none and the window
//!   reads the key itself. Every role change and every hook state is a crumb.
//! - The key (N13, N16, N21): the hook's lines feed the same reducer as the window's keys; the press that starts a
//!   recording remembers which window was in front.
//! - Routing what was heard (N22): this copy's window in front pastes at its front tab, no Enter; another linked
//!   copy's window in front pastes there; otherwise the list of every copy's Claude tabs, at most nine.
//! - The picker (N23): a digit or a click picks a row; Esc, or 30 s with no pick, says "Not sent"; a disabled row
//!   cannot be picked. While it is open the hook swallows digits and Esc.
//! - The checked send (N24) is `route::type_checked`; the client's half is `dictation_answers` below.
//! - Off means off (N13): no hook while dictation or "Dictate from any app" is off or the binding has no Windows
//!   virtual key; under `--e2e` no hook is ever started (N16): lines arrive only as `SvcEvent::Hook`.
//!
//! **[seam]** The plan lists the events and effects it expects; the ones added here are `Pasted` (the answer to
//! `PasteCopy`), `Stop` (quit), the effects `ClearTimer`, `StopLinks` and `WidgetClose`, and `ServiceOpts` (the
//! facts the machine needs: the e2e flag, whether a link exists, this copy's process id). `WidgetHide` is armed by the
//! app and every `Widget` effect cancels a pending hide, so the widget is never hidden while it shows a picker.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::controller::{OutsideAction, Role, RouteResult};
use crate::hotkey::{DictMode, Hotkey, KeyAction, KeyConfig};
use crate::keyfeed::{keys_for, masks_start, HookEvent, KeyFeed};
use crate::link::ClientHandlers;
use crate::route::{
    decide_into, dictation_route, pick_rows, publish_tabs, type_checked, OverlayRow, Route,
    SendDecision, SendHost, SendTab,
};

/// Thirty seconds with no pick says "Not sent".
pub const PICK_MS: u64 = 30_000;
/// "Sent to <tab>" stays this long.
pub const SHOW_MS: u64 = 2200;
/// A note stays this long (PT: SHOW_MS + 800).
pub const NOTE_HIDE_MS: u64 = 3000;
/// A warning stays this long: it says words were NOT sent or typed, and three seconds was too short to
/// read it (review #93). The next press or the next face replaces it sooner.
pub const WARN_HIDE_MS: u64 = 12_000;
/// About how many characters of a note's message fit one line of the card.
const NOTE_LINE_CHARS: usize = 34;
/// The key feed's tick while a bare-modifier press is deciding (the hold time is up a little before it).
pub const KEY_TICK_MS: u64 = 210;
/// A hook that died is armed again after this long (N15; appHelper.ps1 HookLoop).
pub const HOOK_REARM_MS: u64 = 1000;
/// The widget is 480 css px wide.
pub const WIDGET_WIDTH: u32 = 480;
/// What separates two texts kept together: a blank line.
const WAITING_JOIN: &str = "\n\n";
/// What a heard text may be, and a message.
pub const TEXT_MAX: usize = 8000;
pub const MESSAGE_MAX: usize = 200;
/// A recording whose words have not come back counts as busy this long (the 300 s cap and a final pass).
const BUSY_MAX_MS: f64 = 330_000.0;
/// What a note adds when the words were copied.
const COPIED: &str = ", copied";

/// The line a disabled row carries (N19).
pub const NO_PICK_REASON: &str = "Waiting on a question: answer it there first";

/// The binding as this service reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnywhereConfig {
    pub on: bool,
    pub hotkey: Hotkey,
    pub mode: DictMode,
}

/// The facts about this copy the machine needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceOpts {
    /// Under `--e2e`: no real hook is ever started (N16).
    pub e2e: bool,
    /// A link exists for this copy (not under `--e2e` without the scenario's own pipe and key, N26).
    pub link: bool,
    /// This copy's process id: what the hook names when this copy's window is in front.
    pub self_pid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePhase {
    Idle,
    Listening,
    Transcribing,
}

/// The outside recording's face, from the controller.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveView {
    pub phase: LivePhase,
    pub text: String,
    pub level: f32,
    pub starting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Quiet,
    Warn,
}

/// The widget's faces (N19).
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayState {
    Listening {
        text: String,
        level: f32,
        starting: bool,
    },
    Transcribing,
    Pick {
        text: String,
        rows: Vec<OverlayRow>,
    },
    Sending {
        to: String,
    },
    Sent {
        to: String,
    },
    Note {
        message: String,
        tone: Tone,
    },
}

impl OverlayState {
    /// The widget's height in css px for a face (N18): 196 listening, 196 + 48 per row + 30 per group header when
    /// picking, 104 otherwise.
    pub fn height(&self) -> u32 {
        match self {
            OverlayState::Listening { .. } => 196,
            OverlayState::Pick { rows, .. } => {
                let mut groups: Vec<&str> = Vec::new();
                for g in rows.iter().filter_map(|r| r.group.as_deref()) {
                    if !groups.contains(&g) {
                        groups.push(g);
                    }
                }
                196 + 48 * rows.len() as u32 + 30 * groups.len() as u32
            }
            // A note wraps (up to four lines) and the card grows with it.
            OverlayState::Note { message, .. } => {
                let lines = message
                    .chars()
                    .count()
                    .div_ceil(NOTE_LINE_CHARS)
                    .clamp(1, 4) as u32;
                104 + 22 * (lines - 1)
            }
            _ => 104,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerId {
    /// The picker's 30 s.
    Pick,
    /// The key feed's tick (the same as `SvcEvent::Tick`).
    KeyTick,
    /// A hook that died is armed again (N15).
    HookRearm,
}

/// Something that happened.
#[derive(Debug, Clone)]
pub enum SvcEvent {
    /// The binding, from the window's settings (also sent on every apply).
    Config(AnywhereConfig),
    /// A line of the hook, or one arriving through the e2e seam.
    Hook(HookEvent),
    /// The key feed's tick (the same as `Timer(TimerId::KeyTick)`).
    Tick,
    /// The outside recording's face, from the controller.
    Live(LiveView),
    /// What an outside recording heard, or why nothing was (capped at 8000 and 200 characters).
    Routed(RouteResult),
    /// The answer to `PasteCopy`: false when that copy is not connected.
    Pasted(bool),
    /// A row clicked in the widget.
    Pick(String),
    /// The widget dismissed.
    Dismiss,
    /// A click in another app while the picker is open (macOS): the same as Esc.
    Away,
    /// The paste into the tab in front at the start found it gone: say so.
    PasteFailed,
    /// The answer to `AskTabs`: this copy's tabs and every linked copy's, merged (`route::merge_tabs`).
    Tabs(Vec<SendTab>),
    /// The answer to `SendLocal` or `SendRemote`, with the words that were sent (they travel with the answer, so
    /// two sends in flight never swap their words).
    Sent(SendDecision, String),
    /// A client connected to a service (proved both ways), or lost it.
    LinkState(bool),
    Timer(TimerId),
    /// The app is quitting.
    Stop,
}

/// Something for the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum SvcEffect {
    /// The binding's virtual keys.
    HookKeys(Vec<u16>),
    /// Mask the Start menu on a Win up after Ctrl+Win (N14).
    HookMask(bool),
    /// The picker is open: report and swallow digits and Esc.
    HookPick(bool),
    /// A recording is live (Start to Stop or Cancel): report and swallow Esc, so it can cancel.
    HookRec(bool),
    /// A recording begins: note the tab in front now, the one its words go to when they come back.
    NoteFront,
    /// Start the hook thread (never emitted under `--e2e`).
    HookStart,
    /// Pick off, then unhook and let the lock go (N15).
    HookStop,
    ToController(OutsideAction),
    /// Show this face (and cancel a pending hide).
    Widget(OverlayState),
    /// Hide the widget after this long (0: now), unless a `Widget` effect comes first.
    WidgetHide {
        after_ms: u64,
    },
    /// The widget is no longer needed.
    WidgetClose,
    /// Paste at the cursor of this copy's front tab, no Enter (answered with `Pasted`: false when no shell is in front).
    PastePage(String),
    /// Paste at the cursor of another copy's front tab (answered with `Pasted`).
    PasteCopy {
        pid: u32,
        text: String,
    },
    /// Gather this copy's tabs and every linked copy's within 800 ms (answered with `Tabs`).
    AskTabs,
    /// Type into one of this copy's tabs through the checked send (answered with `Sent`).
    SendLocal {
        id: String,
        text: String,
    },
    /// Type into a tab of another copy (answered with `Sent`).
    SendRemote {
        pid: u32,
        id: String,
        text: String,
    },
    /// Words that could not be sent are kept: copy them to the clipboard (on a worker). `shown` is the exact
    /// "copied" note put on screen for it (`None`: no note was shown). When the copy fails the app shows
    /// `fallback` (the note without "copied") but only while `shown` is still the face on screen; with no
    /// `fallback` it says the words could not be copied, but only when nothing else is on screen.
    CopyText {
        text: String,
        shown: Option<OverlayState>,
        fallback: Option<OverlayState>,
    },
    /// The role changed (or was told again on an apply).
    Role(Role),
    /// Start the link service on the pipe; closes any client first.
    StartLinkService,
    /// Start the link client; closes any service first.
    StartLinkClient,
    /// Close the link service and the client (no link, or none wanted).
    StopLinks,
    /// A diag crumb `dictate-anywhere`: the binding's key actions, hook states and roles, never typed text.
    Crumb(Value),
    /// Arm a timer; a later `SetTimer` with the same id replaces it.
    SetTimer(TimerId, u64),
    ClearTimer(TimerId),
}

struct Picking {
    text: String,
    rows: Vec<OverlayRow>,
}

/// The state machine. See the module comment.
pub struct Service {
    opts: ServiceOpts,
    cfg: Option<AnywhereConfig>,
    feed: KeyFeed,
    hook_on: bool,
    /// The keys and mask last sent to the hook: a fresh hook gets them again.
    last_keys: Vec<u16>,
    last_mask: bool,
    rearm_set: bool,
    /// An outside recording is under way (the widget follows it).
    outside: bool,
    /// The recording is live, Start to Stop or Cancel: the hook reports Esc meanwhile (`HookRec`).
    rec: bool,
    /// What the hook was last told about `rec`.
    rec_sent: bool,
    /// When the last recording began and its words have not come back yet (the controller is busy with it).
    busy_since: Option<f64>,
    /// The host's clock at the event being handled.
    now: f64,
    tick_set: bool,
    picking: Option<Picking>,
    /// Heard text waiting for the list of tabs.
    asking: Option<String>,
    /// Heard text waiting for another copy to say it pasted it.
    pending_paste: Option<String>,
    role: Role,
    svc: bool,
    client: bool,
    /// The process in front at the last binding key down.
    last_front: Option<u32>,
    /// ... and at the press that started the recording.
    rec_front: Option<u32>,
}

/// Where heard text goes, as the crumb names it (never the words): `front tab` (this copy's window was in front),
/// `other copy` (a linked copy's window), else `picker` (another app was in front: the list of sessions).
pub fn route_name(front: Option<u32>, self_pid: u32, serving: bool) -> &'static str {
    match front {
        Some(p) if p == self_pid => "front tab",
        Some(_) if serving => "other copy",
        _ => "picker",
    }
}

fn action_name(a: KeyAction) -> &'static str {
    match a {
        KeyAction::Start => "start",
        KeyAction::Stop => "stop",
        KeyAction::Cancel => "cancel",
    }
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::None => "none",
        Role::Service => "service",
        Role::Client => "client",
    }
}

/// `<pid>:<id>`: a tab of another copy.
fn remote_of(id: &str) -> Option<(u32, String)> {
    let (a, b) = id.split_once(':')?;
    if a.is_empty() || b.is_empty() || !a.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((a.parse().ok()?, b.to_string()))
}

fn cap(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        s
    } else {
        s.chars().take(max).collect()
    }
}

impl Service {
    pub fn new(opts: ServiceOpts) -> Self {
        Service {
            opts,
            cfg: None,
            feed: KeyFeed::new(),
            hook_on: false,
            last_keys: Vec::new(),
            last_mask: false,
            rearm_set: false,
            outside: false,
            rec: false,
            rec_sent: false,
            busy_since: None,
            now: 0.0,
            tick_set: false,
            picking: None,
            asking: None,
            pending_paste: None,
            role: Role::None,
            svc: false,
            client: false,
            last_front: None,
            rec_front: None,
        }
    }

    /// Every copy looks for a service from the start; holding the key makes it one.
    pub fn start(&mut self) -> Vec<SvcEffect> {
        let mut fx = Vec::new();
        self.become_client(&mut fx);
        fx
    }

    /// The e2e seam (`DictRoute`): as if a recording begun with the window of process `front` in front heard
    /// `text`. Never called outside --e2e.
    #[doc(hidden)]
    pub fn route(&mut self, text: String, front: Option<u32>, now_ms: f64) -> Vec<SvcEffect> {
        self.rec_front = front;
        self.on(SvcEvent::Routed(RouteResult::Text(text)), now_ms)
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Whether the picker is open.
    pub fn picking(&self) -> bool {
        self.picking.is_some()
    }

    /// Whether the hook was asked to run.
    pub fn hook_running(&self) -> bool {
        self.hook_on
    }

    pub fn on(&mut self, e: SvcEvent, now_ms: f64) -> Vec<SvcEffect> {
        let mut fx = Vec::new();
        self.now = now_ms;
        self.handle(&mut fx, e, now_ms);
        // The hook reports Esc exactly while a recording is live.
        if !self.outside {
            self.rec = false;
        }
        if self.rec != self.rec_sent {
            self.rec_sent = self.rec;
            fx.push(SvcEffect::HookRec(self.rec));
        }
        fx
    }

    fn handle(&mut self, out: &mut Vec<SvcEffect>, e: SvcEvent, now_ms: f64) {
        let mut fx = std::mem::take(out);
        match e {
            SvcEvent::Config(c) => self.apply(&mut fx, c),
            SvcEvent::Hook(h) => self.on_hook(&mut fx, h, now_ms),
            SvcEvent::Tick | SvcEvent::Timer(TimerId::KeyTick) => {
                self.tick_set = false;
                let a = self.feed.tick(now_ms, &self.key_config());
                self.act(&mut fx, a);
            }
            SvcEvent::Timer(TimerId::HookRearm) => {
                self.rearm_set = false;
                if self.hook_on && !self.opts.e2e {
                    fx.push(SvcEffect::HookStart);
                    fx.push(SvcEffect::HookKeys(self.last_keys.clone()));
                    fx.push(SvcEffect::HookMask(self.last_mask));
                    if self.picking.is_some() {
                        fx.push(SvcEffect::HookPick(true));
                    }
                    if self.rec {
                        fx.push(SvcEffect::HookRec(true));
                    }
                }
            }
            SvcEvent::Timer(TimerId::Pick) => {
                if self.picking.is_some() {
                    fx.push(SvcEffect::Crumb(json!({ "pick": "timed out" })));
                    let text = self.picking.as_ref().map(|p| p.text.clone());
                    self.end_pick(&mut fx);
                    self.unsent(&mut fx, text, "Not sent", Tone::Quiet);
                }
            }
            SvcEvent::Live(v) => self.on_live(&mut fx, v),
            SvcEvent::Routed(r) => self.routed(&mut fx, r),
            SvcEvent::Pasted(ok) => {
                if let Some(text) = self.pending_paste.take() {
                    fx.push(SvcEffect::Crumb(
                        json!({ "pasted": ok, "chars": text.chars().count() }),
                    ));
                    if ok {
                        self.note(&mut fx, "Typed into the tab in front", Tone::Quiet);
                    } else {
                        self.ask(&mut fx, text);
                    }
                }
            }
            SvcEvent::Pick(id) => self.choose(&mut fx, &id, "click"),
            SvcEvent::Away => {
                if self.picking.is_some() {
                    fx.push(SvcEffect::Crumb(
                        json!({ "pick": "cancelled", "by": "click away" }),
                    ));
                    let text = self.picking.as_ref().map(|p| p.text.clone());
                    self.end_pick(&mut fx);
                    self.unsent(&mut fx, text, "Not sent", Tone::Quiet);
                }
            }
            SvcEvent::Dismiss => {
                if self.picking.is_some() {
                    fx.push(SvcEffect::Crumb(json!({ "pick": "dismissed" })));
                }
                let text = self.picking.as_ref().map(|p| p.text.clone());
                self.end_pick(&mut fx);
                fx.push(SvcEffect::WidgetHide { after_ms: 0 });
                self.keep(&mut fx, text, None);
            }
            SvcEvent::Tabs(all) => self.on_tabs(&mut fx, all),
            SvcEvent::Sent(d, text) => {
                self.outside = false;
                // `sent`: the words and the Enter landed. `typed`: the words are in the prompt (sent or not).
                fx.push(SvcEffect::Crumb(json!({
                    "sent": matches!(d, SendDecision::Ok(_)),
                    "chars": text.chars().count(),
                    "typed": !matches!(d, SendDecision::No(_)),
                })));
                match d {
                    SendDecision::Ok(t) => {
                        fx.push(SvcEffect::Widget(OverlayState::Sent { to: t.label }));
                        fx.push(SvcEffect::WidgetHide { after_ms: SHOW_MS });
                    }
                    // The words are in the prompt, but the Enter did not send them: say so, plainly.
                    SendDecision::Typed(t) => self.note(
                        &mut fx,
                        format!("Typed into {}, press Enter there", t.label),
                        Tone::Warn,
                    ),
                    SendDecision::No(why) => {
                        self.unsent(&mut fx, Some(text), &format!("Not sent: {why}"), Tone::Warn)
                    }
                }
            }
            SvcEvent::LinkState(on) => {
                // A late answer from a client that was closed (this copy became the service) says nothing.
                if self.client && !self.svc {
                    self.set_role(&mut fx, if on { Role::Client } else { Role::None });
                }
            }
            SvcEvent::Stop => self.stop(&mut fx),
            SvcEvent::PasteFailed => {
                // The host kept the words (the paste found the tab gone), so nothing waits on an answer.
                self.pending_paste = None;
                fx.push(SvcEffect::Crumb(json!({ "pasted": "tab gone" })));
                self.note(&mut fx, "Not typed: that tab has closed", Tone::Warn)
            }
        }
        *out = fx;
    }

    fn key_config(&self) -> KeyConfig {
        match &self.cfg {
            Some(c) => KeyConfig {
                hotkey: c.hotkey.clone(),
                mode: c.mode,
                hold_ms: None,
            },
            None => KeyConfig {
                hotkey: crate::hotkey::default_hotkey(),
                mode: DictMode::Hold,
                hold_ms: None,
            },
        }
    }

    /* ----- the copies of the app ----- */

    fn set_role(&mut self, fx: &mut Vec<SvcEffect>, r: Role) {
        if r == self.role {
            return;
        }
        self.role = r;
        fx.push(SvcEffect::Crumb(json!({ "role": role_name(r) })));
        fx.push(SvcEffect::Role(r));
    }

    fn become_client(&mut self, fx: &mut Vec<SvcEffect>) {
        let was_service = self.svc;
        self.svc = false;
        if !self.opts.link {
            if was_service || self.client {
                fx.push(SvcEffect::StopLinks);
            }
            self.client = false;
            self.set_role(fx, Role::None);
            return;
        }
        // A former service has no one to serve until a client connects to another one.
        if was_service {
            self.set_role(fx, Role::None);
        }
        if self.client {
            return;
        }
        self.client = true;
        fx.push(SvcEffect::StartLinkClient);
    }

    fn become_service(&mut self, fx: &mut Vec<SvcEffect>) {
        self.client = false;
        if self.opts.link && !self.svc {
            self.svc = true;
            fx.push(SvcEffect::StartLinkService);
        }
        self.set_role(fx, Role::Service);
    }

    /* ----- the binding ----- */

    fn apply(&mut self, fx: &mut Vec<SvcEffect>, next: AnywhereConfig) {
        let keys = if next.on {
            keys_for(&next.hotkey)
        } else {
            None
        };
        let now = AnywhereConfig {
            on: keys.is_some(),
            ..next.clone()
        };
        // Any dictation pref arrives here; only the binding or its mode may drop the keys held (L35).
        let changed = self.cfg.as_ref() != Some(&now);
        self.cfg = Some(now);
        if changed {
            self.feed.reset();
            self.clear_tick(fx);
        }
        // The window says this as it loads: the part it may have missed, again.
        fx.push(SvcEffect::Role(self.role));
        let Some(keys) = keys else {
            // Off means off: an answer still out (the tab list, a paste) acts on nothing.
            // The words heard are kept on the clipboard, with no note: the user turned it off.
            let picked = self.picking.take().map(|p| p.text);
            let older = self.take_waiting(picked.clone());
            self.keep(fx, older, None);
            self.rec_front = None;
            self.busy_since = None;
            // Off sends pick off, then unhooks and lets the lock go (N15).
            if picked.is_some() {
                fx.push(SvcEffect::ClearTimer(TimerId::Pick));
                fx.push(SvcEffect::WidgetHide { after_ms: 0 });
            }
            self.clear_rearm(fx);
            if self.hook_on {
                self.hook_on = false;
                fx.push(SvcEffect::HookPick(false));
                fx.push(SvcEffect::HookStop);
            }
            self.become_client(fx);
            return;
        };
        if self.opts.e2e {
            return;
        }
        if !self.hook_on {
            self.hook_on = true;
            fx.push(SvcEffect::HookStart);
        }
        self.last_keys = keys.clone();
        self.last_mask = masks_start(&next.hotkey);
        fx.push(SvcEffect::HookKeys(keys));
        fx.push(SvcEffect::HookMask(self.last_mask));
    }

    fn clear_rearm(&mut self, fx: &mut Vec<SvcEffect>) {
        if self.rearm_set {
            self.rearm_set = false;
            fx.push(SvcEffect::ClearTimer(TimerId::HookRearm));
        }
    }

    /// The hook died while still wanted: drop it now, start a fresh one after HOOK_REARM_MS (N15).
    fn rearm(&mut self, fx: &mut Vec<SvcEffect>) {
        if !self.hook_on || self.opts.e2e {
            return;
        }
        fx.push(SvcEffect::HookStop);
        self.rearm_set = true;
        fx.push(SvcEffect::SetTimer(TimerId::HookRearm, HOOK_REARM_MS));
    }

    fn clear_tick(&mut self, fx: &mut Vec<SvcEffect>) {
        if self.tick_set {
            self.tick_set = false;
            fx.push(SvcEffect::ClearTimer(TimerId::KeyTick));
        }
    }

    /* ----- the key ----- */

    fn act(&mut self, fx: &mut Vec<SvcEffect>, a: Option<KeyAction>) {
        let Some(a) = a else { return };
        fx.push(SvcEffect::Crumb(json!({ "key": action_name(a) })));
        let out = match a {
            KeyAction::Start => {
                // The last recording's words have not come back: a second one would be lost, and the first
                // would route by the second's front. Say so; the first keeps its own.
                if self.busy_since.is_some_and(|t| self.now - t < BUSY_MAX_MS) {
                    // Nothing started, so the key must not think it did: its next press is a start again.
                    self.feed.to_idle();
                    fx.push(SvcEffect::Crumb(
                        json!({ "key": "start", "refused": "busy" }),
                    ));
                    self.flash(fx, "Still transcribing", Tone::Quiet);
                    return;
                }
                if self.picking.is_some() {
                    fx.push(SvcEffect::Crumb(json!({ "pick": "not sent" })));
                }
                // A new recording replaces whatever an earlier text was waiting on; those words are kept.
                let picked = self.picking.as_ref().map(|p| p.text.clone());
                self.end_pick(fx);
                let older = self.take_waiting(picked);
                self.keep(fx, older, None);
                self.outside = true;
                self.rec = true;
                self.busy_since = Some(self.now);
                self.rec_front = self.last_front;
                fx.push(SvcEffect::Crumb(self.front_crumb(self.rec_front)));
                fx.push(SvcEffect::NoteFront);
                fx.push(SvcEffect::Widget(OverlayState::Listening {
                    text: String::new(),
                    level: 0.0,
                    starting: false,
                }));
                OutsideAction::Start
            }
            KeyAction::Stop => {
                self.rec = false;
                OutsideAction::Stop
            }
            KeyAction::Cancel => {
                self.rec = false;
                OutsideAction::Cancel
            }
        };
        fx.push(SvcEffect::ToController(out));
    }

    fn on_hook(&mut self, fx: &mut Vec<SvcEffect>, h: HookEvent, now: f64) {
        // A list on screen answers its digits and Esc whatever else changed.
        if self.picking.is_some() {
            match &h {
                HookEvent::Esc => {
                    fx.push(SvcEffect::Crumb(
                        json!({ "pick": "cancelled", "by": "esc" }),
                    ));
                    let text = self.picking.as_ref().map(|p| p.text.clone());
                    self.end_pick(fx);
                    self.unsent(fx, text, "Not sent", Tone::Quiet);
                    return;
                }
                HookEvent::Digit(n) => {
                    let id = self
                        .picking
                        .as_ref()
                        .and_then(|p| p.rows.iter().find(|r| r.n == *n))
                        .map(|r| r.id.clone());
                    if let Some(id) = id {
                        self.choose(fx, &id, "digit");
                    }
                    return;
                }
                _ => {}
            }
        }
        match h {
            HookEvent::Ready | HookEvent::Failed | HookEvent::Waiting => {
                let name = match h {
                    HookEvent::Ready => "ready",
                    HookEvent::Failed => "failed",
                    _ => "waiting",
                };
                fx.push(SvcEffect::Crumb(json!({ "hook": name })));
                if h == HookEvent::Ready {
                    self.become_service(fx);
                } else {
                    // The hook went (or never was ours): a key-up it will not deliver would leave the mic
                    // open until the 300 s cap. What was recording is cancelled, the keys it held are
                    // forgotten and a picker is closed (review #87).
                    self.drop_recording(fx);
                    self.become_client(fx);
                    if h == HookEvent::Failed {
                        self.rearm(fx);
                    }
                }
                return;
            }
            _ => {}
        }
        if !self.cfg.as_ref().is_some_and(|c| c.on) {
            return;
        }
        // Which app is in front: this copy, one on the link, or another app. The chord's FIRST key-down says
        // it: by a later key's down (Win after Ctrl) the shell's own windows may be in front, or none at all.
        // A later down only fills in a front the first one could not name.
        if let HookEvent::Key {
            down: true, front, ..
        } = &h
        {
            if !self.feed.holding() || self.last_front.is_none() {
                self.last_front = *front;
            }
        }
        let a = self.feed.feed(&h, now, &self.key_config());
        self.act(fx, a);
        self.clear_tick(fx);
        if self.feed.arming() {
            self.tick_set = true;
            fx.push(SvcEffect::SetTimer(TimerId::KeyTick, KEY_TICK_MS));
        }
    }

    /* ----- the widget ----- */

    fn note(&mut self, fx: &mut Vec<SvcEffect>, message: impl Into<String>, tone: Tone) {
        self.outside = false;
        self.flash(fx, message, tone);
    }

    /// A note on the widget that leaves the recording's state alone.
    fn flash(&mut self, fx: &mut Vec<SvcEffect>, message: impl Into<String>, tone: Tone) {
        fx.push(SvcEffect::Widget(OverlayState::Note {
            message: message.into(),
            tone,
        }));
        fx.push(SvcEffect::WidgetHide {
            after_ms: if tone == Tone::Warn {
                WARN_HIDE_MS
            } else {
                NOTE_HIDE_MS
            },
        });
    }

    /// Words that ended unsent: say why and that they were copied, and copy them. A copy that fails puts the
    /// plain note back (the app does that).
    fn unsent(&mut self, fx: &mut Vec<SvcEffect>, text: Option<String>, why: &str, tone: Tone) {
        self.unsent_as(fx, text, why, why, tone)
    }

    /// `unsent`, where the note without the copy (`plain`, also the one a failed copy falls back to) says more
    /// than the note with it (`short` + ", copied").
    fn unsent_as(
        &mut self,
        fx: &mut Vec<SvcEffect>,
        text: Option<String>,
        short: &str,
        plain: &str,
        tone: Tone,
    ) {
        let Some(text) = text else {
            return self.note(fx, plain.to_string(), tone);
        };
        let plain = cap(plain.to_string(), MESSAGE_MAX);
        let copied = format!(
            "{}{COPIED}",
            cap(short.to_string(), MESSAGE_MAX - COPIED.len())
        );
        self.note(fx, copied.clone(), tone);
        fx.push(SvcEffect::CopyText {
            text,
            shown: Some(OverlayState::Note {
                message: copied,
                tone,
            }),
            fallback: Some(OverlayState::Note {
                message: plain,
                tone,
            }),
        });
    }

    /// Every text still waiting on an answer (`picked` from the list, the one asking for tabs, the one waiting on
    /// another copy's paste), joined oldest first; the waiting slots are emptied.
    fn take_waiting(&mut self, picked: Option<String>) -> Option<String> {
        let all: Vec<String> = [picked, self.asking.take(), self.pending_paste.take()]
            .into_iter()
            .flatten()
            .collect();
        (!all.is_empty()).then(|| all.join(WAITING_JOIN))
    }

    /// Copies words that would otherwise be lost, with the note the caller already showed (or none).
    fn keep(
        &mut self,
        fx: &mut Vec<SvcEffect>,
        text: Option<String>,
        fallback: Option<OverlayState>,
    ) {
        if let Some(text) = text {
            fx.push(SvcEffect::CopyText {
                text,
                shown: None,
                fallback,
            });
        }
    }

    /// A crumb naming the window in front at a press: this copy, another process (by id), or none.
    fn front_crumb(&self, front: Option<u32>) -> Value {
        match front {
            Some(p) if p == self.opts.self_pid => json!({ "front": "self" }),
            Some(p) => json!({ "front": "other", "pid": p }),
            None => json!({ "front": "none" }),
        }
    }

    fn on_live(&mut self, fx: &mut Vec<SvcEffect>, v: LiveView) {
        // The recording ended without the key (the 300 s cap): the key must not think it is still on, or its
        // next press is a dead stop and the one after starts a recording nobody wanted.
        if v.phase != LivePhase::Listening && self.feed.listening() {
            self.feed.to_idle();
            fx.push(SvcEffect::Crumb(json!({
                "stop": "not by the key",
                "phase": if v.phase == LivePhase::Idle { "idle" } else { "transcribing" },
            })));
        }
        if !self.outside {
            return;
        }
        match v.phase {
            LivePhase::Listening => fx.push(SvcEffect::Widget(OverlayState::Listening {
                text: v.text,
                level: v.level,
                starting: v.starting,
            })),
            LivePhase::Transcribing => fx.push(SvcEffect::Widget(OverlayState::Transcribing)),
            LivePhase::Idle => {}
        }
    }

    /* ----- routing what was heard ----- */

    fn routed(&mut self, fx: &mut Vec<SvcEffect>, r: RouteResult) {
        self.busy_since = None;
        let front = self.rec_front.take();
        let text = match r {
            RouteResult::Message(m) => {
                let m = cap(m, MESSAGE_MAX);
                let tone = if m == "Cancelled" || m == "Heard nothing" {
                    Tone::Quiet
                } else {
                    Tone::Warn
                };
                return self.note(fx, m, tone);
            }
            RouteResult::Text(t) => cap(t, TEXT_MAX),
        };
        // Which way the words go, and why: never the words, only their length.
        let way = route_name(front, self.opts.self_pid, self.svc);
        let mut crumb = self.front_crumb(front);
        crumb["route"] = json!(way);
        crumb["chars"] = json!(text.chars().count());
        fx.push(SvcEffect::Crumb(crumb));
        match (way, front) {
            // This copy's window was in front: the words go to its front tab, at the cursor, never Enter.
            ("front tab", _) => {
                // The note waits for the answer: with no shell in front the words go on to the list.
                let older = self.pending_paste.replace(text.clone());
                self.keep(fx, older, None);
                fx.push(SvcEffect::PastePage(text));
            }
            // Another linked copy's window was in front: that copy pastes.
            ("other copy", Some(pid)) => {
                let older = self.pending_paste.replace(text.clone());
                self.keep(fx, older, None);
                fx.push(SvcEffect::PasteCopy { pid, text });
            }
            _ => self.ask(fx, text),
        }
    }

    fn ask(&mut self, fx: &mut Vec<SvcEffect>, text: String) {
        // A newer dictation never loses the words of one still waiting.
        let older = self.asking.replace(text);
        self.keep(fx, older, None);
        fx.push(SvcEffect::AskTabs);
    }

    fn on_tabs(&mut self, fx: &mut Vec<SvcEffect>, all: Vec<SendTab>) {
        let Some(text) = self.asking.take() else {
            return;
        };
        match dictation_route(&all, false) {
            Route::Lead(t) => self.send_to(fx, t.id, t.label, text),
            Route::Pick(list) => {
                if list.is_empty() {
                    fx.push(SvcEffect::Crumb(json!({ "pick": "no session" })));
                    return self.unsent_as(
                        fx,
                        Some(text),
                        "No Claude session",
                        "No Claude session to send to",
                        Tone::Warn,
                    );
                }
                let rows = pick_rows(&list);
                let older = self.picking.as_ref().map(|p| p.text.clone());
                self.end_pick(fx);
                self.keep(fx, older, None);
                self.picking = Some(Picking {
                    text: text.clone(),
                    rows: rows.clone(),
                });
                fx.push(SvcEffect::Crumb(
                    json!({ "pick": "shown", "rows": rows.len(), "tabs": all.len() }),
                ));
                fx.push(SvcEffect::SetTimer(TimerId::Pick, PICK_MS));
                fx.push(SvcEffect::HookPick(true));
                fx.push(SvcEffect::Widget(OverlayState::Pick { text, rows }));
            }
        }
    }

    /// The hook is no longer ours: cancel what is recording, forget the held keys, end a pick.
    fn drop_recording(&mut self, fx: &mut Vec<SvcEffect>) {
        let live = self.outside || self.rec;
        let picking = self.picking.is_some();
        self.feed.reset();
        self.clear_tick(fx);
        self.end_pick(fx);
        if live {
            fx.push(SvcEffect::ToController(OutsideAction::Cancel));
            self.rec = false;
            self.busy_since = None;
            self.note(fx, "Cancelled", Tone::Quiet);
        } else if picking {
            self.note(fx, "Not sent", Tone::Quiet);
        }
    }

    fn end_pick(&mut self, fx: &mut Vec<SvcEffect>) {
        if self.picking.take().is_some() {
            fx.push(SvcEffect::ClearTimer(TimerId::Pick));
            fx.push(SvcEffect::HookPick(false));
        }
    }

    /// A row picked `how` (a digit or a click).
    fn choose(&mut self, fx: &mut Vec<SvcEffect>, id: &str, how: &str) {
        let Some(p) = &self.picking else { return };
        let Some(row) = p.rows.iter().find(|r| r.id == id) else {
            return;
        };
        if row.disabled.is_some() {
            return;
        }
        let (rid, label, text) = (row.id.clone(), row.label.clone(), p.text.clone());
        fx.push(SvcEffect::Crumb(
            json!({ "pick": "picked", "row": row.n, "id": rid, "by": how }),
        ));
        self.end_pick(fx);
        self.send_to(fx, rid, label, text);
    }

    fn send_to(&mut self, fx: &mut Vec<SvcEffect>, id: String, label: String, text: String) {
        fx.push(SvcEffect::Widget(OverlayState::Sending { to: label }));
        match remote_of(&id) {
            // Another copy's tab: that copy types it, through its own checks.
            Some((pid, rest)) if self.svc => fx.push(SvcEffect::SendRemote {
                pid,
                id: rest,
                text,
            }),
            Some(_) => self.unsent(
                fx,
                Some(text),
                "Not sent: that copy of the app has closed",
                Tone::Warn,
            ),
            None => fx.push(SvcEffect::SendLocal { id, text }),
        }
    }

    fn stop(&mut self, fx: &mut Vec<SvcEffect>) {
        self.clear_rearm(fx);
        if self.hook_on {
            self.hook_on = false;
            fx.push(SvcEffect::HookStop);
        }
        if self.svc || self.client {
            fx.push(SvcEffect::StopLinks);
        }
        self.svc = false;
        self.client = false;
        self.picking = None;
        self.asking = None;
        self.pending_paste = None;
        self.outside = false;
        self.busy_since = None;
        self.clear_tick(fx);
        fx.push(SvcEffect::ClearTimer(TimerId::Pick));
        fx.push(SvcEffect::WidgetClose);
    }
}

/* ----- the client's half: this copy answers the service on the link ----- */

/// What this copy gives the one dictation (N24): its tabs, what Windows says runs under one, a way to write to a
/// tab's shell, and a paste into its front tab. Implemented by the app (Task 22).
pub trait ClientHost: Send + Sync + 'static {
    /// The tabs now: label, folder, the agent's mark and state, the session.
    fn tabs(&self) -> Vec<SendTab>;
    /// Whether this app's orchestrator switch is on. Flare has none (D9): always false.
    fn orchestrator_on(&self) -> bool {
        false
    }
    /// What Windows says runs under a tab's shell (`claude`, or not).
    fn agent_under(&self, id: &str) -> Option<String>;
    /// Write to a tab's shell. False when nothing was written (the window could not take it).
    fn write(&self, id: &str, data: &str) -> bool;
    /// Whether Claude's prompt box in the tab holds unsent text (read off its screen). `None`: no box to read.
    fn prompt_draft(&self, _id: &str) -> Option<bool> {
        None
    }
    /// Wait this long (the send watches its words land and its Enter take). The link's handler thread sleeps; the
    /// UI thread never calls this.
    fn pause(&self, ms: u64) {
        // sync-ok: runs on the link's own handler thread, never the UI thread
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    /// A step of a checked send, for the diagnostics log (never the words).
    fn stage(&self, _what: &str, _n: u64) {}
    /// Paste dictated words at the cursor of the tab in front, never Enter. Must not block.
    fn paste(&self, text: &str);
}

struct Checked<'a, H: ClientHost>(&'a H);

impl<H: ClientHost> SendHost for Checked<'_, H> {
    fn agent_under(&self, id: &str) -> Option<String> {
        self.0.agent_under(id)
    }
    fn write(&self, id: &str, bytes: &str) -> bool {
        self.0.write(id, bytes)
    }
    fn prompt_draft(&self, id: &str) -> Option<bool> {
        self.0.prompt_draft(id)
    }
    fn later(&self, ms: u64, f: Box<dyn FnOnce() + '_>) {
        self.0.pause(ms);
        f();
    }
    fn pause(&self, ms: u64) {
        self.0.pause(ms);
    }
    fn stage(&self, what: &str, n: u64) {
        self.0.stage(what, n);
    }
}

struct Answers<H: ClientHost>(Arc<H>);

impl<H: ClientHost> ClientHandlers for Answers<H> {
    fn tabs(&self) -> Vec<SendTab> {
        let h = &*self.0;
        publish_tabs(h.tabs(), h.orchestrator_on(), &|id| h.agent_under(id))
    }

    fn send(&self, id: &str, text: &str) -> SendDecision {
        let h = &*self.0;
        type_checked(&|| decide_into(id, &h.tabs()), text, &Checked(h))
    }

    fn paste(&self, text: &str) {
        self.0.paste(text);
    }
}

/// What a client answers the service with, for this app: its tabs, a checked send, and a paste into its front tab.
pub fn dictation_answers<H: ClientHost>(host: Arc<H>) -> Arc<dyn ClientHandlers> {
    Arc::new(Answers(host))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::default_hotkey;
    use std::sync::Mutex;

    fn tab(over: impl FnOnce(&mut SendTab)) -> SendTab {
        let mut t = SendTab {
            id: "p1".into(),
            tab: "t1".into(),
            label: "Wind".into(),
            cwd: "C:\\src\\wind".into(),
            agent: Some("claude".into()),
            state: None,
            orchestrator: false,
            session: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into()),
            copy: None,
        };
        over(&mut t);
        t
    }

    fn f8() -> Hotkey {
        Hotkey {
            code: "F8".into(),
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
        }
    }

    fn cfg(h: Hotkey, mode: DictMode) -> AnywhereConfig {
        AnywhereConfig {
            on: true,
            hotkey: h,
            mode,
        }
    }

    fn opts(e2e: bool, link: bool) -> ServiceOpts {
        ServiceOpts {
            e2e,
            link,
            self_pid: 1,
        }
    }

    /// An F8 toggle service under `--e2e` (the PT tests' `started`).
    fn started() -> Service {
        let mut s = Service::new(opts(true, false));
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Toggle)), 0.0);
        s
    }

    fn key(down: bool, front: Option<u32>) -> SvcEvent {
        SvcEvent::Hook(HookEvent::Key {
            down,
            code: "F8".into(),
            front,
        })
    }

    fn has(fx: &[SvcEffect], e: &SvcEffect) -> bool {
        fx.contains(e)
    }

    fn started_ctl(fx: &[SvcEffect]) -> bool {
        has(fx, &SvcEffect::ToController(OutsideAction::Start))
    }

    /// A service that is in `Role::Service` with a link, F8 held, not under e2e.
    fn serving() -> Service {
        let mut s = Service::new(opts(false, true));
        s.start();
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        s.on(SvcEvent::Hook(HookEvent::Ready), 1.0);
        s
    }

    /// Hears `text` as if the window in front at the press was `front`, and returns the effects.
    fn hear(s: &mut Service, front: Option<u32>, text: &str) -> Vec<SvcEffect> {
        s.on(key(true, front), 10.0);
        s.on(key(false, None), 11.0);
        s.on(SvcEvent::Routed(RouteResult::Text(text.into())), 20.0)
    }

    /// The effect that keeps `text`, with the plain note a failed copy falls back to.
    fn copy(text: &str, fallback: OverlayState) -> SvcEffect {
        let shown = match &fallback {
            OverlayState::Note { message, tone } => Some(OverlayState::Note {
                message: format!("{message}, copied"),
                tone: *tone,
            }),
            _ => None,
        };
        SvcEffect::CopyText {
            text: text.into(),
            shown,
            fallback: Some(fallback),
        }
    }

    fn silent(text: &str) -> SvcEffect {
        SvcEffect::CopyText {
            text: text.into(),
            shown: None,
            fallback: None,
        }
    }

    fn plain(message: &str, tone: Tone) -> OverlayState {
        OverlayState::Note {
            message: message.into(),
            tone,
        }
    }

    fn copies(fx: &[SvcEffect]) -> Vec<&SvcEffect> {
        fx.iter()
            .filter(|e| matches!(e, SvcEffect::CopyText { .. }))
            .collect()
    }

    fn rows_of(fx: &[SvcEffect]) -> Vec<OverlayRow> {
        fx.iter()
            .find_map(|e| match e {
                SvcEffect::Widget(OverlayState::Pick { rows, .. }) => Some(rows.clone()),
                _ => None,
            })
            .expect("a picker")
    }

    // ----- the key with a Flare in front (PT: describe 'the key with a Prism Terminal in front') -----

    #[test]
    fn starts_with_another_app_in_front_whichever_it_is_named_by_its_process() {
        let mut s = started();
        let a = s.on(key(true, Some(4242)), 1.0);
        let b = s.on(key(false, None), 2.0);
        assert!(started_ctl(&a) || started_ctl(&b));
    }

    #[test]
    fn starts_with_this_copy_in_front() {
        let mut s = started();
        let a = s.on(key(true, Some(1)), 1.0);
        let b = s.on(key(false, None), 2.0);
        assert!(started_ctl(&a) || started_ctl(&b));
    }

    #[test]
    fn starts_with_any_other_app_in_front() {
        let mut s = started();
        let a = s.on(key(true, None), 1.0);
        let b = s.on(key(false, None), 2.0);
        assert!(started_ctl(&a) || started_ctl(&b));
    }

    #[test]
    fn a_start_shows_listening_and_remembers_the_window_in_front() {
        let mut s = started();
        let fx = s.on(key(true, Some(4242)), 1.0);
        assert!(has(
            &fx,
            &SvcEffect::Widget(OverlayState::Listening {
                text: String::new(),
                level: 0.0,
                starting: false
            })
        ));
        let fx = s.on(SvcEvent::Routed(RouteResult::Text("hi".into())), 5.0);
        // 4242 is not this copy and there is no link service: the list.
        assert!(has(&fx, &SvcEffect::AskTabs));
    }

    /* ----- review: Esc cancels an outside recording, a second press never steals the first ----- */

    #[test]
    fn a_live_recording_asks_the_hook_for_esc_and_esc_cancels_it() {
        let mut s = started();
        let fx = s.on(key(true, Some(7)), 1.0);
        assert!(has(&fx, &SvcEffect::HookRec(true)), "{fx:?}");
        s.on(key(false, None), 2.0);
        let fx = s.on(SvcEvent::Hook(HookEvent::Esc), 400.0);
        assert!(
            has(&fx, &SvcEffect::ToController(OutsideAction::Cancel)),
            "{fx:?}"
        );
        assert!(has(&fx, &SvcEffect::HookRec(false)), "{fx:?}");
    }

    #[test]
    fn the_hook_is_not_asked_for_esc_before_a_recording_or_after_its_result() {
        let mut s = started();
        let fx = s.on(SvcEvent::Tick, 0.5);
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::HookRec(_))));
        s.on(key(true, Some(7)), 1.0);
        s.on(key(false, None), 2.0);
        let fx = s.on(
            SvcEvent::Routed(RouteResult::Message("Cancelled".into())),
            3.0,
        );
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::HookRec(true))));
    }

    #[test]
    fn a_second_press_while_the_first_is_transcribing_keeps_the_first_recordings_front() {
        let mut s = serving();
        s.on(key(true, Some(7)), 10.0);
        s.on(key(false, None), 11.0);
        // Pressed again before the first result: refused, said so, nothing started.
        let fx = s.on(key(true, Some(9)), 12.0);
        assert!(!started_ctl(&fx), "{fx:?}");
        assert!(
            has(
                &fx,
                &SvcEffect::Widget(OverlayState::Note {
                    message: "Still transcribing".into(),
                    tone: Tone::Quiet
                })
            ),
            "{fx:?}"
        );
        s.on(key(false, None), 13.0);
        // The first clip routes by ITS front.
        let fx = s.on(SvcEvent::Routed(RouteResult::Text("first".into())), 20.0);
        assert!(
            has(
                &fx,
                &SvcEffect::PasteCopy {
                    pid: 7,
                    text: "first".into()
                }
            ),
            "{fx:?}"
        );
        // Once it is back, a new press starts again.
        let fx = s.on(key(true, Some(9)), 30.0);
        assert!(started_ctl(&fx), "{fx:?}");
    }

    /// A toggle F8 service that serves, under `--e2e`-free rules (the PT tests' `started`, toggled).
    fn toggling() -> Service {
        let mut s = Service::new(opts(false, true));
        s.start();
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Toggle)), 0.0);
        s.on(SvcEvent::Hook(HookEvent::Ready), 1.0);
        s
    }

    fn tap(s: &mut Service, at: f64) -> Vec<SvcEffect> {
        let mut fx = s.on(key(true, Some(1)), at);
        fx.extend(s.on(key(false, None), at + 1.0));
        fx
    }

    fn live(phase: LivePhase) -> SvcEvent {
        SvcEvent::Live(LiveView {
            phase,
            text: String::new(),
            level: 0.0,
            starting: false,
        })
    }

    #[test]
    fn a_start_refused_while_transcribing_leaves_the_key_ready_to_start_again() {
        let mut s = toggling();
        assert!(started_ctl(&tap(&mut s, 10.0)));
        let fx = tap(&mut s, 20.0);
        assert!(has(&fx, &SvcEffect::ToController(OutsideAction::Stop)));
        // Pressed while the words are still coming back: refused, and the log says so.
        let fx = tap(&mut s, 30.0);
        assert!(!started_ctl(&fx), "{fx:?}");
        assert!(has(
            &fx,
            &SvcEffect::Crumb(json!({ "key": "start", "refused": "busy" }))
        ));
        s.on(SvcEvent::Routed(RouteResult::Text("words".into())), 40.0);
        // The next press starts: it is not a dead stop.
        let fx = tap(&mut s, 50.0);
        assert!(started_ctl(&fx), "{fx:?}");
    }

    #[test]
    fn a_recording_the_cap_stopped_leaves_the_key_ready_to_start_again() {
        let mut s = toggling();
        assert!(started_ctl(&tap(&mut s, 10.0)));
        s.on(live(LivePhase::Listening), 20.0);
        // Five minutes: the controller stops by itself, no key involved.
        let fx = s.on(live(LivePhase::Transcribing), 300_010.0);
        assert!(
            has(
                &fx,
                &SvcEffect::Crumb(json!({ "stop": "not by the key", "phase": "transcribing" }))
            ),
            "{fx:?}"
        );
        s.on(
            SvcEvent::Routed(RouteResult::Text("words".into())),
            300_020.0,
        );
        let fx = tap(&mut s, 300_030.0);
        assert!(
            started_ctl(&fx),
            "the first press after the cap starts: {fx:?}"
        );
    }

    #[test]
    fn the_chords_first_key_down_names_the_window_in_front_not_the_win_key_down() {
        let mut s = Service::new(opts(false, true));
        s.start();
        s.on(
            SvcEvent::Config(cfg(default_hotkey(), DictMode::Toggle)),
            0.0,
        );
        s.on(SvcEvent::Hook(HookEvent::Ready), 1.0);
        let k = |down: bool, code: &str, front: Option<u32>| {
            SvcEvent::Hook(HookEvent::Key {
                down,
                code: code.into(),
                front,
            })
        };
        // One tap of Ctrl+Win, with the window each down sees in front.
        let chord = |s: &mut Service, at: f64, ctrl: Option<u32>, win: Option<u32>| {
            let mut fx = s.on(k(true, "ControlLeft", ctrl), at);
            fx.extend(s.on(k(true, "MetaLeft", win), at + 10.0));
            fx.extend(s.on(k(false, "MetaLeft", None), at + 90.0));
            fx.extend(s.on(k(false, "ControlLeft", None), at + 100.0));
            fx
        };
        // Ctrl down with this copy in front; by the Win down the shell's window (another process) is in front.
        let fx = chord(&mut s, 10.0, Some(1), Some(77));
        assert!(started_ctl(&fx), "{fx:?}");
        assert!(has(&fx, &SvcEffect::Crumb(json!({ "front": "self" }))));
        // The stopping tap's fronts do not matter.
        chord(&mut s, 1000.0, Some(77), Some(77));
        let fx = s.on(SvcEvent::Routed(RouteResult::Text("hi".into())), 1500.0);
        assert!(has(&fx, &SvcEffect::PastePage("hi".into())), "{fx:?}");
        assert!(!has(&fx, &SvcEffect::AskTabs));
        s.on(SvcEvent::Pasted(true), 1600.0);
        // A first down that names nothing is filled in by the next down of the same chord.
        let fx = chord(&mut s, 2000.0, None, Some(1));
        assert!(started_ctl(&fx), "{fx:?}");
        chord(&mut s, 3000.0, None, None);
        let fx = s.on(SvcEvent::Routed(RouteResult::Text("hi".into())), 3500.0);
        assert!(has(&fx, &SvcEffect::PastePage("hi".into())), "{fx:?}");
    }

    #[test]
    fn a_send_whose_enter_did_not_take_says_typed_not_sent_and_keeps_no_copy() {
        let mut s = serving();
        let fx = s.on(
            SvcEvent::Sent(
                SendDecision::Typed(tab(|t| t.label = "Web".into())),
                "the words".into(),
            ),
            10.0,
        );
        assert!(
            has(
                &fx,
                &SvcEffect::Widget(plain("Typed into Web, press Enter there", Tone::Warn))
            ),
            "{fx:?}"
        );
        assert!(copies(&fx).is_empty(), "the words are in the prompt");
        assert!(!fx
            .iter()
            .any(|e| matches!(e, SvcEffect::Widget(OverlayState::Sent { .. }))));
        assert!(has(
            &fx,
            &SvcEffect::Crumb(json!({ "sent": false, "chars": 9, "typed": true }))
        ));
    }

    #[test]
    fn the_route_and_pick_crumbs_name_the_way_and_lengths_never_the_words() {
        let mut s = serving();
        let mut all = hear(&mut s, Some(77), "secret words");
        all.extend(s.on(SvcEvent::Pasted(false), 21.0));
        all.extend(s.on(SvcEvent::Tabs(vec![tab(|_| {})]), 22.0));
        all.extend(s.on(SvcEvent::Pick("p1".into()), 23.0));
        all.extend(s.on(
            SvcEvent::Sent(SendDecision::Ok(tab(|_| {})), "secret words".into()),
            24.0,
        ));
        let crumbs: Vec<&Value> = all
            .iter()
            .filter_map(|e| match e {
                SvcEffect::Crumb(v) => Some(v),
                _ => None,
            })
            .collect();
        for want in [
            json!({ "front": "other", "pid": 77, "route": "other copy", "chars": 12 }),
            json!({ "pasted": false, "chars": 12 }),
            json!({ "pick": "shown", "rows": 1, "tabs": 1 }),
            json!({ "pick": "picked", "row": 1, "id": "p1", "by": "click" }),
            json!({ "sent": true, "chars": 12, "typed": true }),
        ] {
            assert!(crumbs.contains(&&want), "{want} in {crumbs:?}");
        }
        assert!(
            crumbs.iter().all(|c| !c.to_string().contains("secret")),
            "never the words"
        );
    }

    #[test]
    fn a_start_notes_the_tab_in_front_and_a_failed_paste_says_so() {
        let mut s = serving();
        let fx = s.on(key(true, Some(1)), 10.0);
        assert!(has(&fx, &SvcEffect::NoteFront), "{fx:?}");
        let fx = s.on(SvcEvent::PasteFailed, 20.0);
        assert!(has(
            &fx,
            &SvcEffect::Widget(OverlayState::Note {
                message: "Not typed: that tab has closed".into(),
                tone: Tone::Warn
            })
        ));
    }

    /* ----- audit L35: a pref change that is not the binding keeps the key feed ----- */

    #[test]
    fn an_unchanged_binding_config_mid_recording_keeps_the_release_stopping_it() {
        let mut s = Service::new(opts(true, false));
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        let fx = s.on(key(true, Some(7)), 1.0);
        assert!(started_ctl(&fx));
        // Any other dictation pref arrives as the same binding again.
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 2.0);
        let fx = s.on(key(false, None), 3.0);
        assert!(
            has(&fx, &SvcEffect::ToController(OutsideAction::Stop)),
            "{fx:?}"
        );
    }

    #[test]
    fn a_changed_binding_mid_recording_still_resets_the_key_feed() {
        let mut s = Service::new(opts(true, false));
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        s.on(key(true, Some(7)), 1.0);
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Toggle)), 2.0);
        let fx = s.on(key(false, None), 3.0);
        assert!(
            !has(&fx, &SvcEffect::ToController(OutsideAction::Stop)),
            "{fx:?}"
        );
    }
    /* ----- off means off, e2e never hooks ----- */

    #[test]
    fn the_hook_is_never_started_under_e2e() {
        let mut s = Service::new(opts(true, true));
        let fx = s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        assert!(!fx.iter().any(|e| matches!(
            e,
            SvcEffect::HookStart | SvcEffect::HookKeys(_) | SvcEffect::HookMask(_)
        )));
        assert!(!s.hook_running());
        // The lines through the seam still drive the reducer.
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Toggle)), 0.0);
        let fx = s.on(key(true, Some(7)), 1.0);
        assert!(started_ctl(&fx));
    }

    #[test]
    fn the_hook_starts_with_the_bindings_keys_and_mask_and_only_once() {
        let mut s = Service::new(opts(false, false));
        let fx = s.on(SvcEvent::Config(cfg(default_hotkey(), DictMode::Hold)), 0.0);
        assert!(has(&fx, &SvcEffect::HookStart));
        assert!(has(&fx, &SvcEffect::HookKeys(vec![0x5b, 0x5c, 0xa2, 0xa3])));
        assert!(has(&fx, &SvcEffect::HookMask(true)));
        let fx = s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 1.0);
        assert!(!has(&fx, &SvcEffect::HookStart));
        assert!(has(&fx, &SvcEffect::HookKeys(vec![0x77])));
        assert!(has(&fx, &SvcEffect::HookMask(false)));
    }

    #[test]
    fn dictation_off_or_a_key_without_a_virtual_key_means_no_hook() {
        let mut s = Service::new(opts(false, false));
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        assert!(s.hook_running());
        let mut off = cfg(f8(), DictMode::Hold);
        off.on = false;
        let fx = s.on(SvcEvent::Config(off), 1.0);
        let pick_off = fx.iter().position(|e| *e == SvcEffect::HookPick(false));
        let stop = fx.iter().position(|e| *e == SvcEffect::HookStop);
        assert!(
            pick_off.is_some() && stop.is_some() && pick_off < stop,
            "{fx:?}"
        );
        assert!(!s.hook_running());
        // A key the hook cannot name.
        let odd = Hotkey {
            code: "Numpad5".into(),
            ..f8()
        };
        let mut s = Service::new(opts(false, false));
        let fx = s.on(SvcEvent::Config(cfg(odd, DictMode::Hold)), 0.0);
        assert!(!has(&fx, &SvcEffect::HookStart));
        // And while off, the key does nothing.
        let fx = s.on(key(true, Some(1)), 1.0);
        assert!(!started_ctl(&fx));
    }

    #[test]
    fn a_bare_modifier_press_arms_the_tick_and_the_tick_starts_it() {
        let mut s = Service::new(opts(true, false));
        s.on(
            SvcEvent::Config(cfg(
                Hotkey {
                    code: "AltRight".into(),
                    ctrl: false,
                    alt: false,
                    shift: false,
                    meta: false,
                },
                DictMode::Hold,
            )),
            0.0,
        );
        let fx = s.on(
            SvcEvent::Hook(HookEvent::Key {
                down: true,
                code: "AltRight".into(),
                front: Some(9),
            }),
            100.0,
        );
        assert!(has(
            &fx,
            &SvcEffect::SetTimer(TimerId::KeyTick, KEY_TICK_MS)
        ));
        assert!(!started_ctl(&fx));
        let fx = s.on(SvcEvent::Tick, 310.0);
        assert!(started_ctl(&fx));
    }

    #[test]
    fn a_hook_that_died_is_armed_again_after_1000_ms_with_its_last_keys_and_mask() {
        let mut s = Service::new(opts(false, false));
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        s.on(SvcEvent::Hook(HookEvent::Ready), 1.0);
        let fx = s.on(SvcEvent::Hook(HookEvent::Failed), 2.0);
        // The dead hook is dropped now, a fresh one is asked for later.
        assert!(has(&fx, &SvcEffect::HookStop), "{fx:?}");
        assert!(
            has(&fx, &SvcEffect::SetTimer(TimerId::HookRearm, HOOK_REARM_MS)),
            "{fx:?}"
        );
        assert_eq!(HOOK_REARM_MS, 1000);
        assert!(s.hook_running(), "still wanted");
        let fx = s.on(SvcEvent::Timer(TimerId::HookRearm), 1002.0);
        let start = fx.iter().position(|e| *e == SvcEffect::HookStart);
        let keys = fx
            .iter()
            .position(|e| *e == SvcEffect::HookKeys(vec![0x77]));
        let mask = fx.iter().position(|e| *e == SvcEffect::HookMask(false));
        assert!(
            start.is_some() && keys > start && mask > start,
            "start, then the last keys and mask: {fx:?}"
        );
    }

    #[test]
    fn no_re_arm_when_it_was_turned_off_or_stopped_meanwhile() {
        let mut s = Service::new(opts(false, false));
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        s.on(SvcEvent::Hook(HookEvent::Failed), 1.0);
        let mut off = cfg(f8(), DictMode::Hold);
        off.on = false;
        let fx = s.on(SvcEvent::Config(off), 2.0);
        assert!(
            has(&fx, &SvcEffect::ClearTimer(TimerId::HookRearm)),
            "{fx:?}"
        );
        let fx = s.on(SvcEvent::Timer(TimerId::HookRearm), 1001.0);
        assert!(!has(&fx, &SvcEffect::HookStart), "{fx:?}");
        // Never with the hook off to begin with, and not after the app stops.
        let mut s = Service::new(opts(false, false));
        let fx = s.on(SvcEvent::Hook(HookEvent::Failed), 1.0);
        assert!(!has(
            &fx,
            &SvcEffect::SetTimer(TimerId::HookRearm, HOOK_REARM_MS)
        ));
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 2.0);
        s.on(SvcEvent::Hook(HookEvent::Failed), 3.0);
        let fx = s.on(SvcEvent::Stop, 4.0);
        assert!(
            has(&fx, &SvcEffect::ClearTimer(TimerId::HookRearm)),
            "{fx:?}"
        );
        let fx = s.on(SvcEvent::Timer(TimerId::HookRearm), 1004.0);
        assert!(!has(&fx, &SvcEffect::HookStart));
    }

    #[test]
    fn waiting_is_not_a_death_and_arms_nothing() {
        let mut s = Service::new(opts(false, false));
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        let fx = s.on(SvcEvent::Hook(HookEvent::Waiting), 1.0);
        assert!(!has(&fx, &SvcEffect::HookStop));
        assert!(!fx
            .iter()
            .any(|e| matches!(e, SvcEffect::SetTimer(TimerId::HookRearm, _))));
    }
    #[test]
    fn a_digit_outside_the_picker_does_nothing() {
        let mut s = started();
        for n in 1..=9u8 {
            assert!(s
                .on(SvcEvent::Hook(HookEvent::Digit(n)), f64::from(n))
                .is_empty());
        }
        assert!(!s.picking());
    }

    /* ----- roles ----- */

    #[test]
    fn ready_makes_the_service_and_failed_or_waiting_a_client() {
        let mut s = Service::new(opts(false, true));
        let fx = s.start();
        assert_eq!(fx, vec![SvcEffect::StartLinkClient]);
        let fx = s.on(SvcEvent::Hook(HookEvent::Ready), 1.0);
        assert!(has(&fx, &SvcEffect::StartLinkService));
        assert!(has(&fx, &SvcEffect::Role(Role::Service)));
        assert_eq!(s.role(), Role::Service);
        // The service loses the hook: it stands down and looks for another service.
        let fx = s.on(SvcEvent::Hook(HookEvent::Failed), 2.0);
        assert!(has(&fx, &SvcEffect::StartLinkClient));
        assert_eq!(s.role(), Role::None);
        // A client that connects is a client, and when the link drops, none.
        s.on(SvcEvent::LinkState(true), 3.0);
        assert_eq!(s.role(), Role::Client);
        s.on(SvcEvent::LinkState(false), 4.0);
        assert_eq!(s.role(), Role::None);
    }

    #[test]
    fn with_no_link_a_copy_is_alone_and_waiting_is_still_none() {
        let mut s = Service::new(opts(false, false));
        assert!(s.start().is_empty());
        let fx = s.on(SvcEvent::Hook(HookEvent::Waiting), 1.0);
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::StartLinkClient)));
        assert_eq!(s.role(), Role::None);
        let fx = s.on(SvcEvent::Hook(HookEvent::Ready), 2.0);
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::StartLinkService)));
        assert_eq!(s.role(), Role::Service);
    }

    #[test]
    fn the_service_closing_hands_over_to_a_waiting_copy() {
        // Copy B waits on the lock copy A holds: a client.
        let mut b = Service::new(opts(false, true));
        b.start();
        b.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 0.0);
        b.on(SvcEvent::Hook(HookEvent::Waiting), 1.0);
        b.on(SvcEvent::LinkState(true), 2.0);
        assert_eq!(b.role(), Role::Client);
        // A closes: its link drops, B's hook takes the lock within its poll and B becomes the service.
        b.on(SvcEvent::LinkState(false), 3.0);
        let fx = b.on(SvcEvent::Hook(HookEvent::Ready), 4.0);
        assert!(has(&fx, &SvcEffect::StartLinkService));
        assert!(has(&fx, &SvcEffect::Role(Role::Service)));
        // And its own tabs are what the list asks for.
        let fx = hear(&mut b, None, "hello");
        assert!(has(&fx, &SvcEffect::AskTabs));
    }

    #[test]
    fn a_late_link_answer_never_unmakes_the_service() {
        let mut s = serving();
        s.on(SvcEvent::LinkState(false), 5.0);
        assert_eq!(s.role(), Role::Service);
    }

    #[test]
    fn an_apply_tells_the_role_again() {
        let mut s = serving();
        let fx = s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 6.0);
        assert!(has(&fx, &SvcEffect::Role(Role::Service)));
    }

    /* ----- the e2e route seam (M8 Task 27) ----- */

    #[test]
    fn a_route_with_this_copy_in_front_pastes_into_the_front_tab() {
        let mut s = started();
        let me = s.opts.self_pid;
        let fx = s.route("hello there".into(), Some(me), 20.0);
        assert!(has(&fx, &SvcEffect::PastePage("hello there".into())));
        assert!(!has(&fx, &SvcEffect::AskTabs));
    }

    #[test]
    fn a_route_with_no_front_window_asks_for_the_tabs() {
        let mut s = started();
        let fx = s.route("hello there".into(), None, 20.0);
        assert!(has(&fx, &SvcEffect::AskTabs));
        assert!(!has(&fx, &SvcEffect::PastePage("hello there".into())));
    }

    /* ----- routing ----- */

    #[test]
    fn heard_text_with_this_copy_in_front_is_pasted_and_never_listed() {
        let mut s = serving();
        let fx = hear(&mut s, Some(1), "hello there");
        assert!(has(&fx, &SvcEffect::PastePage("hello there".into())));
        assert!(!has(&fx, &SvcEffect::AskTabs));
        // The note waits for the answer: a shell was in front.
        assert!(!fx
            .iter()
            .any(|e| matches!(e, SvcEffect::Widget(OverlayState::Note { .. }))));
        let fx = s.on(SvcEvent::Pasted(true), 21.0);
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, tone: Tone::Quiet }) if message == "Typed into the tab in front")));
        assert!(has(&fx, &SvcEffect::WidgetHide { after_ms: 3000 }));
        assert!(copies(&fx).is_empty());
    }

    #[test]
    fn a_paste_with_no_shell_in_front_goes_on_to_the_list_and_never_claims_it_typed() {
        let mut s = serving();
        hear(&mut s, Some(1), "lost no more");
        let fx = s.on(SvcEvent::Pasted(false), 21.0);
        assert!(has(&fx, &SvcEffect::AskTabs));
        assert!(!fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, .. }) if message == "Typed into the tab in front")));
        let fx = s.on(SvcEvent::Tabs(vec![]), 22.0);
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::CopyText { text, .. } if text == "lost no more")));
    }

    #[test]
    fn two_sends_in_flight_each_copy_their_own_words() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        s.on(SvcEvent::Pick("p1".into()), 30.0);
        // A second dictation routes and sends before the first answers.
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Pick("p1".into()), 31.0);
        assert!(fx.iter().any(|e| matches!(e, SvcEffect::SendLocal { .. })));
        let fx = s.on(
            SvcEvent::Sent(SendDecision::No("a".into()), "first".into()),
            32.0,
        );
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::CopyText { text, .. } if text == "first")));
        let fx = s.on(
            SvcEvent::Sent(SendDecision::No("b".into()), "second".into()),
            33.0,
        );
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::CopyText { text, .. } if text == "second")));
        // A first send that landed clears nothing of the second.
        let fx = s.on(
            SvcEvent::Sent(SendDecision::Ok(tab(|_| {})), "third".into()),
            34.0,
        );
        assert!(copies(&fx).is_empty());
    }

    #[test]
    fn heard_text_with_another_copy_in_front_is_pasted_there_or_listed() {
        let mut s = serving();
        let fx = hear(&mut s, Some(77), "hi");
        assert!(has(
            &fx,
            &SvcEffect::PasteCopy {
                pid: 77,
                text: "hi".into()
            }
        ));
        let fx = s.on(SvcEvent::Pasted(true), 21.0);
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, .. }) if message == "Typed into the tab in front")));
        // The copy is not connected after all: the list.
        let fx = hear(&mut s, Some(77), "hi");
        assert!(fx.iter().any(|e| matches!(e, SvcEffect::PasteCopy { .. })));
        let fx = s.on(SvcEvent::Pasted(false), 22.0);
        assert!(has(&fx, &SvcEffect::AskTabs));
    }

    #[test]
    fn a_client_never_pastes_into_another_copy() {
        let mut s = Service::new(opts(false, true));
        s.start();
        let fx = hear(&mut s, Some(77), "hi");
        assert!(has(&fx, &SvcEffect::AskTabs));
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::PasteCopy { .. })));
    }

    #[test]
    fn nothing_with_no_claude_session_says_so() {
        let mut s = serving();
        hear(&mut s, None, "hi");
        let shell = tab(|t| {
            t.agent = None;
            t.session = None;
        });
        let fx = s.on(SvcEvent::Tabs(vec![shell]), 21.0);
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, tone: Tone::Warn }) if message == "No Claude session, copied")));
        assert!(has(
            &fx,
            &SvcEffect::CopyText {
                text: "hi".into(),
                shown: Some(plain("No Claude session, copied", Tone::Warn)),
                fallback: Some(plain("No Claude session to send to", Tone::Warn)),
            }
        ));
        assert!(!s.picking());
    }

    #[test]
    fn messages_are_quiet_or_warnings() {
        for (m, tone) in [
            ("Cancelled", Tone::Quiet),
            ("Heard nothing", Tone::Quiet),
            ("Choose a speech model in Settings", Tone::Warn),
        ] {
            let mut s = serving();
            let fx = s.on(SvcEvent::Routed(RouteResult::Message(m.into())), 1.0);
            assert!(
                fx.iter().any(|e| matches!(e,
                    SvcEffect::Widget(OverlayState::Note { message, tone: t }) if message == m && *t == tone)),
                "{m}"
            );
        }
    }

    #[test]
    fn heard_text_and_messages_are_capped() {
        let mut s = serving();
        let fx = hear(&mut s, Some(1), &"a".repeat(9000));
        let pasted = fx
            .iter()
            .find_map(|e| match e {
                SvcEffect::PastePage(t) => Some(t.len()),
                _ => None,
            })
            .unwrap();
        assert_eq!(pasted, TEXT_MAX);
        let fx = s.on(SvcEvent::Routed(RouteResult::Message("m".repeat(300))), 2.0);
        assert!(
            fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, .. }) if message.len() == MESSAGE_MAX))
        );
    }

    /* ----- the picker ----- */

    fn open_picker(s: &mut Service, tabs: Vec<SendTab>) -> Vec<SvcEffect> {
        hear(s, None, "send this");
        s.on(SvcEvent::Tabs(tabs), 21.0)
    }

    #[test]
    fn the_list_opens_the_picker_with_the_hook_told_and_a_thirty_second_timer() {
        let mut s = serving();
        let fx = open_picker(&mut s, vec![tab(|_| {}), tab(|t| t.id = "p2".into())]);
        assert!(has(&fx, &SvcEffect::HookPick(true)));
        assert!(has(&fx, &SvcEffect::SetTimer(TimerId::Pick, 30_000)));
        assert_eq!(rows_of(&fx).len(), 2);
        assert!(s.picking());
    }

    #[test]
    fn the_picker_lists_nine_at_most_and_only_claude_tabs() {
        let mut s = serving();
        let mut tabs: Vec<SendTab> = (0..12).map(|i| tab(|t| t.id = format!("p{i}"))).collect();
        tabs.insert(
            0,
            tab(|t| {
                t.id = "sh".into();
                t.agent = None;
                t.session = None;
            }),
        );
        let fx = open_picker(&mut s, tabs);
        let rows = rows_of(&fx);
        assert_eq!(rows.len(), 9);
        assert!(rows.iter().all(|r| r.id != "sh"));
    }

    #[test]
    fn a_digit_picks_the_row_and_sends_it_locally() {
        let mut s = serving();
        open_picker(
            &mut s,
            vec![
                tab(|_| {}),
                tab(|t| {
                    t.id = "p2".into();
                    t.label = "Web".into();
                }),
            ],
        );
        let fx = s.on(SvcEvent::Hook(HookEvent::Digit(2)), 30.0);
        assert!(has(&fx, &SvcEffect::HookPick(false)));
        assert!(has(&fx, &SvcEffect::ClearTimer(TimerId::Pick)));
        assert!(has(
            &fx,
            &SvcEffect::Widget(OverlayState::Sending { to: "Web".into() })
        ));
        assert!(has(
            &fx,
            &SvcEffect::SendLocal {
                id: "p2".into(),
                text: "send this".into()
            }
        ));
        assert!(!s.picking());
        // The answer: sent, and the widget goes after 2200 ms.
        let fx = s.on(
            SvcEvent::Sent(
                SendDecision::Ok(tab(|t| t.label = "Web".into())),
                "send this".into(),
            ),
            31.0,
        );
        assert!(has(
            &fx,
            &SvcEffect::Widget(OverlayState::Sent { to: "Web".into() })
        ));
        assert!(has(&fx, &SvcEffect::WidgetHide { after_ms: 2200 }));
    }

    #[test]
    fn a_click_picks_a_row_and_a_refusal_says_why() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Pick("p1".into()), 30.0);
        assert!(fx.iter().any(|e| matches!(e, SvcEffect::SendLocal { .. })));
        let fx = s.on(
            SvcEvent::Sent(
                SendDecision::No("Wind changed while it was checked: send again".into()),
                "send this".into(),
            ),
            31.0,
        );
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, tone: Tone::Warn })
                if message == "Not sent: Wind changed while it was checked: send again, copied")));
        assert!(has(
            &fx,
            &copy(
                "send this",
                plain(
                    "Not sent: Wind changed while it was checked: send again",
                    Tone::Warn
                )
            )
        ));
    }

    #[test]
    fn a_tab_of_another_copy_is_sent_through_that_copy() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|t| t.id = "9:q1".into())]);
        let fx = s.on(SvcEvent::Hook(HookEvent::Digit(1)), 30.0);
        assert!(has(
            &fx,
            &SvcEffect::SendRemote {
                pid: 9,
                id: "q1".into(),
                text: "send this".into()
            }
        ));
    }

    #[test]
    fn a_tab_of_another_copy_with_no_service_is_not_sent() {
        let mut s = Service::new(opts(false, true));
        s.start();
        s.on(SvcEvent::Config(cfg(f8(), DictMode::Toggle)), 0.0);
        open_picker(&mut s, vec![tab(|t| t.id = "9:q1".into())]);
        let fx = s.on(SvcEvent::Pick("9:q1".into()), 30.0);
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::SendRemote { .. })));
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, tone: Tone::Warn })
                if message == "Not sent: that copy of the app has closed, copied")));
        assert!(has(
            &fx,
            &copy(
                "send this",
                plain("Not sent: that copy of the app has closed", Tone::Warn)
            )
        ));
    }

    #[test]
    fn a_disabled_row_cannot_be_picked() {
        let mut s = serving();
        let fx = open_picker(
            &mut s,
            vec![tab(|t| {
                t.state = Some("question".into());
            })],
        );
        let rows = rows_of(&fx);
        assert_eq!(rows[0].disabled.as_deref(), Some(NO_PICK_REASON));
        assert!(s.on(SvcEvent::Hook(HookEvent::Digit(1)), 30.0).is_empty());
        assert!(s.on(SvcEvent::Pick("p1".into()), 31.0).is_empty());
        assert!(s.picking());
    }

    #[test]
    fn a_digit_with_no_such_row_is_swallowed_and_does_nothing() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        assert!(s.on(SvcEvent::Hook(HookEvent::Digit(5)), 30.0).is_empty());
        assert!(s.picking());
    }

    #[test]
    fn esc_keeps_it_unsent() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Hook(HookEvent::Esc), 30.0);
        assert!(has(&fx, &SvcEffect::HookPick(false)));
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, tone: Tone::Quiet }) if message == "Not sent, copied")));
        assert!(has(&fx, &copy("send this", plain("Not sent", Tone::Quiet))));
        assert!(!s.picking());
    }

    /// The crumbs of `fx`, as their JSON text.
    fn crumbs_of(fx: &[SvcEffect]) -> Vec<String> {
        fx.iter()
            .filter_map(|e| match e {
                SvcEffect::Crumb(v) => Some(v.to_string()),
                _ => None,
            })
            .collect()
    }

    // ----- rule 1139: the route decision on the Mac (Flare in front or another app) -----

    #[test]
    fn the_route_is_named_by_the_app_in_front_at_the_press() {
        // The Mac hook names the front app's process (NSWorkspace.frontmostApplication), as Windows names the
        // foreground window's: this copy (pid 1) pastes into its front tab, any other app gets the picker.
        assert_eq!(route_name(Some(1), 1, true), "front tab");
        assert_eq!(route_name(Some(1), 1, false), "front tab");
        assert_eq!(route_name(Some(4242), 1, false), "picker");
        assert_eq!(route_name(Some(4242), 1, true), "other copy");
        assert_eq!(route_name(None, 1, true), "picker");
    }

    #[test]
    fn flare_in_front_pastes_and_another_app_in_front_opens_the_picker() {
        let mut s = started();
        let fx = hear(&mut s, Some(1), "hello");
        assert!(has(&fx, &SvcEffect::PastePage("hello".into())));
        assert!(crumbs_of(&fx)
            .contains(&r#"{"front":"self","route":"front tab","chars":5}"#.to_string()));
        // Another app in front (Safari, say), alone with no link: the list of sessions, then the picker.
        let mut s = started();
        let fx = hear(&mut s, Some(4242), "hello");
        assert!(has(&fx, &SvcEffect::AskTabs));
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::PastePage(_))));
        assert!(crumbs_of(&fx)
            .contains(&r#"{"front":"other","pid":4242,"route":"picker","chars":5}"#.to_string()));
        let fx = s.on(SvcEvent::Tabs(vec![tab(|_| {})]), 21.0);
        assert!(s.picking());
        assert_eq!(rows_of(&fx).len(), 1);
        assert!(crumbs_of(&fx).contains(&r#"{"pick":"shown","rows":1,"tabs":1}"#.to_string()));
    }

    #[test]
    fn the_picker_logs_shown_picked_and_sent_with_counts_never_the_words() {
        let mut s = serving();
        let fx = open_picker(&mut s, vec![tab(|_| {}), tab(|t| t.id = "p2".into())]);
        assert!(crumbs_of(&fx).contains(&r#"{"pick":"shown","rows":2,"tabs":2}"#.to_string()));
        let fx = s.on(SvcEvent::Hook(HookEvent::Digit(2)), 30.0);
        assert!(has(
            &fx,
            &SvcEffect::SendLocal {
                id: "p2".into(),
                text: "send this".into()
            }
        ));
        assert!(crumbs_of(&fx)
            .contains(&r#"{"pick":"picked","row":2,"id":"p2","by":"digit"}"#.to_string()));
        let fx = s.on(
            SvcEvent::Sent(
                SendDecision::Ok(tab(|t| t.id = "p2".into())),
                "send this".into(),
            ),
            31.0,
        );
        assert!(crumbs_of(&fx).contains(&r#"{"sent":true,"chars":9,"typed":true}"#.to_string()));
        for c in crumbs_of(&fx) {
            assert!(!c.contains("send this"), "{c}");
        }
    }

    #[test]
    fn a_click_in_another_app_cancels_the_picker_like_esc_and_keeps_the_words() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Away, 30.0);
        assert!(has(&fx, &SvcEffect::HookPick(false)));
        assert!(has(&fx, &copy("send this", plain("Not sent", Tone::Quiet))));
        assert!(crumbs_of(&fx).contains(&r#"{"pick":"cancelled","by":"click away"}"#.to_string()));
        assert!(!s.picking());
        // With no picker open a click away is nothing.
        assert!(s.on(SvcEvent::Away, 31.0).is_empty());
    }

    #[test]
    fn esc_and_the_timeout_are_logged_as_cancels() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Hook(HookEvent::Esc), 30.0);
        assert!(crumbs_of(&fx).contains(&r#"{"pick":"cancelled","by":"esc"}"#.to_string()));
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Timer(TimerId::Pick), 40.0);
        assert!(crumbs_of(&fx).contains(&r#"{"pick":"timed out"}"#.to_string()));
    }

    #[test]
    fn thirty_seconds_without_a_pick_says_not_sent() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Timer(TimerId::Pick), 30_021.0);
        assert!(fx.iter().any(|e| matches!(e,
            SvcEffect::Widget(OverlayState::Note { message, tone: Tone::Quiet }) if message == "Not sent, copied")));
        assert!(has(&fx, &copy("send this", plain("Not sent", Tone::Quiet))));
        assert!(has(&fx, &SvcEffect::HookPick(false)));
        assert!(!s.picking());
        // A timer that fires after a pick says nothing.
        assert!(s.on(SvcEvent::Timer(TimerId::Pick), 30_022.0).is_empty());
    }

    #[test]
    fn a_new_press_ends_an_open_picker_and_drops_a_list_still_coming() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(key(true, None), 40.0);
        assert!(has(&fx, &SvcEffect::HookPick(false)));
        assert!(started_ctl(&fx));
        assert!(!s.picking());
        s.on(key(false, None), 41.0);
        // A list that arrives for the earlier text opens nothing.
        hear(&mut s, None, "one");
        s.on(key(true, None), 50.0);
        let fx = s.on(SvcEvent::Tabs(vec![tab(|_| {})]), 51.0);
        assert!(fx.is_empty());
    }

    #[test]
    fn dismiss_ends_the_pick_and_hides_the_widget() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Dismiss, 30.0);
        assert!(has(&fx, &SvcEffect::HookPick(false)));
        assert!(has(&fx, &SvcEffect::WidgetHide { after_ms: 0 }));
        assert!(!s.picking());
    }

    #[test]
    fn switching_off_mid_pick_unhooks_and_sends_nothing() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let mut off = cfg(f8(), DictMode::Hold);
        off.on = false;
        let fx = s.on(SvcEvent::Config(off), 30.0);
        assert!(has(&fx, &SvcEffect::HookStop));
        assert!(!fx.iter().any(|e| matches!(
            e,
            SvcEffect::SendLocal { .. } | SvcEffect::SendRemote { .. }
        )));
        assert!(!s.picking());
    }

    #[test]
    fn a_remote_send_that_is_refused_keeps_the_words() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|t| t.id = "9:q1".into())]);
        s.on(SvcEvent::Pick("9:q1".into()), 30.0);
        let fx = s.on(
            SvcEvent::Sent(
                SendDecision::No("that copy of the app has closed".into()),
                "send this".into(),
            ),
            31.0,
        );
        assert!(has(
            &fx,
            &copy(
                "send this",
                plain("Not sent: that copy of the app has closed", Tone::Warn)
            )
        ));
    }

    #[test]
    fn a_dropped_paste_round_trip_asks_for_the_tabs_and_keeps_the_words_when_none_can_take_them() {
        let mut s = serving();
        let fx = hear(&mut s, Some(77), "hi");
        assert!(copies(&fx).is_empty());
        let fx = s.on(SvcEvent::Pasted(false), 21.0);
        assert!(has(&fx, &SvcEffect::AskTabs));
        assert!(copies(&fx).is_empty());
        let fx = s.on(SvcEvent::Tabs(vec![]), 22.0);
        assert!(has(
            &fx,
            &SvcEffect::CopyText {
                text: "hi".into(),
                shown: Some(plain("No Claude session, copied", Tone::Warn)),
                fallback: Some(plain("No Claude session to send to", Tone::Warn)),
            }
        ));
    }

    #[test]
    fn dismissing_the_picker_keeps_the_words_without_a_note() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Dismiss, 30.0);
        assert!(has(&fx, &silent("send this")));
        // Nothing open: nothing to keep.
        assert!(copies(&s.on(SvcEvent::Dismiss, 31.0)).is_empty());
    }

    #[test]
    fn a_new_recording_keeps_the_words_an_earlier_one_was_still_waiting_on() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(key(true, None), 40.0);
        assert!(has(&fx, &silent("send this")));
        s.on(key(false, None), 41.0);
        hear(&mut s, None, "one");
        let fx = s.on(key(true, None), 50.0);
        assert!(has(&fx, &silent("one")));
    }

    #[test]
    fn a_newer_text_never_loses_the_words_of_one_still_waiting() {
        let mut s = serving();
        hear(&mut s, None, "first");
        let fx = s.on(SvcEvent::Routed(RouteResult::Text("second".into())), 22.0);
        assert!(has(&fx, &silent("first")));
        // A list that opens over an open list keeps the older one too.
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        s.on(SvcEvent::Routed(RouteResult::Text("second".into())), 22.0);
        let fx = s.on(SvcEvent::Tabs(vec![tab(|_| {})]), 23.0);
        assert!(has(&fx, &silent("send this")));
    }

    #[test]
    fn switching_off_with_words_waiting_keeps_them() {
        let mut s = serving();
        hear(&mut s, None, "hi");
        let fx = s.on(SvcEvent::Config(off_cfg()), 21.0);
        assert!(has(&fx, &silent("hi")));
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Config(off_cfg()), 30.0);
        assert!(has(&fx, &silent("send this")));
        // Off with nothing waiting copies nothing.
        let mut s = serving();
        assert!(copies(&s.on(SvcEvent::Config(off_cfg()), 5.0)).is_empty());
    }

    #[test]
    fn a_note_with_the_copy_stays_under_the_cap() {
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        s.on(SvcEvent::Pick("p1".into()), 30.0);
        let why = "x".repeat(300);
        let fx = s.on(
            SvcEvent::Sent(SendDecision::No(why), "send this".into()),
            31.0,
        );
        let msg = fx
            .iter()
            .find_map(|e| match e {
                SvcEffect::Widget(OverlayState::Note { message, .. }) => Some(message.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(msg.chars().count(), MESSAGE_MAX);
        assert!(msg.ends_with(", copied"));
    }

    #[test]
    fn cancelled_heard_nothing_and_a_successful_send_or_paste_copy_nothing() {
        for m in [
            "Cancelled",
            "Heard nothing",
            "Choose a speech model in Settings",
        ] {
            let mut s = serving();
            let fx = s.on(SvcEvent::Routed(RouteResult::Message(m.into())), 1.0);
            assert!(copies(&fx).is_empty(), "{m}");
        }
        // This copy in front: pasted.
        let mut s = serving();
        assert!(copies(&hear(&mut s, Some(1), "hi")).is_empty());
        // Another copy pasted it.
        let mut s = serving();
        hear(&mut s, Some(77), "hi");
        assert!(copies(&s.on(SvcEvent::Pasted(true), 21.0)).is_empty());
        // A picked tab took it.
        let mut s = serving();
        open_picker(&mut s, vec![tab(|_| {})]);
        let fx = s.on(SvcEvent::Pick("p1".into()), 30.0);
        assert!(copies(&fx).is_empty());
        let fx = s.on(
            SvcEvent::Sent(SendDecision::Ok(tab(|_| {})), "send this".into()),
            31.0,
        );
        assert!(copies(&fx).is_empty());
        // A stop while still listening is not a finished transcript.
        let mut s = serving();
        s.on(key(true, None), 10.0);
        assert!(copies(&s.on(key(false, None), 11.0)).is_empty());
        assert!(copies(&s.on(SvcEvent::Stop, 40.0)).is_empty());
    }

    fn off_cfg() -> AnywhereConfig {
        let mut off = cfg(f8(), DictMode::Hold);
        off.on = false;
        off
    }

    #[test]
    fn switching_off_while_the_tab_list_is_gathered_opens_no_picker_after() {
        let mut s = serving();
        // Heard with another app in front and no copy to paste into: the list is being gathered.
        let fx = hear(&mut s, None, "hi");
        assert!(has(&fx, &SvcEffect::AskTabs));
        s.on(SvcEvent::Config(off_cfg()), 21.0);
        let fx = s.on(SvcEvent::Tabs(vec![tab(|_| {})]), 22.0);
        assert!(!s.picking());
        assert!(!fx.iter().any(|e| matches!(
            e,
            SvcEffect::Widget(OverlayState::Pick { .. })
                | SvcEffect::HookPick(true)
                | SvcEffect::SetTimer(..)
        )));
    }

    #[test]
    fn switching_off_during_a_paste_round_trip_opens_no_picker_after() {
        let mut s = serving();
        let fx = hear(&mut s, Some(77), "hi");
        assert!(fx.iter().any(|e| matches!(e, SvcEffect::PasteCopy { .. })));
        s.on(SvcEvent::Config(off_cfg()), 21.0);
        let fx = s.on(SvcEvent::Pasted(false), 22.0);
        assert!(!s.picking());
        assert!(!has(&fx, &SvcEffect::AskTabs));
        assert!(!fx.iter().any(|e| matches!(
            e,
            SvcEffect::Widget(OverlayState::Pick { .. }) | SvcEffect::HookPick(true)
        )));
        let fx = s.on(SvcEvent::Tabs(vec![tab(|_| {})]), 23.0);
        assert!(!s.picking());
        assert!(fx.is_empty());
    }

    #[test]
    fn the_live_view_reaches_the_widget_only_for_an_outside_recording() {
        let mut s = started();
        let v = LiveView {
            phase: LivePhase::Listening,
            text: "hel".into(),
            level: 0.5,
            starting: false,
        };
        assert!(s.on(SvcEvent::Live(v.clone()), 0.5).is_empty());
        s.on(key(true, None), 1.0);
        let fx = s.on(SvcEvent::Live(v), 2.0);
        assert_eq!(
            fx,
            vec![SvcEffect::Widget(OverlayState::Listening {
                text: "hel".into(),
                level: 0.5,
                starting: false
            })]
        );
        let fx = s.on(
            SvcEvent::Live(LiveView {
                phase: LivePhase::Transcribing,
                text: String::new(),
                level: 0.0,
                starting: false,
            }),
            3.0,
        );
        // No key stopped it (the cap did): the key is told, and the log says so, before the face.
        assert_eq!(
            fx.last(),
            Some(&SvcEffect::Widget(OverlayState::Transcribing)),
            "{fx:?}"
        );
        // After the words are routed the recording is over: late live views draw nothing.
        s.on(
            SvcEvent::Routed(RouteResult::Message("Cancelled".into())),
            4.0,
        );
        assert!(s
            .on(
                SvcEvent::Live(LiveView {
                    phase: LivePhase::Transcribing,
                    text: String::new(),
                    level: 0.0,
                    starting: false
                }),
                5.0
            )
            .is_empty());
    }

    #[test]
    fn the_widget_heights_follow_the_face() {
        let row = |g: Option<&str>| OverlayRow {
            id: "x".into(),
            n: 1,
            label: "l".into(),
            folder: "f".into(),
            state: None,
            disabled: None,
            lead: false,
            group: g.map(String::from),
        };
        let pick = |rows| OverlayState::Pick {
            text: "t".into(),
            rows,
        };
        assert_eq!(
            OverlayState::Listening {
                text: String::new(),
                level: 0.0,
                starting: false
            }
            .height(),
            196
        );
        assert_eq!(OverlayState::Transcribing.height(), 104);
        assert_eq!(OverlayState::Sent { to: "x".into() }.height(), 104);
        assert_eq!(pick(vec![row(None), row(None)]).height(), 196 + 96);
        assert_eq!(
            pick(vec![row(Some("A")), row(Some("A")), row(Some("B"))]).height(),
            196 + 144 + 60
        );
    }

    /// Review #93: a long note wraps and the card grows with it, and a warning stays longer than a note.
    #[test]
    fn a_long_note_grows_the_card_and_a_warning_is_held_longer() {
        let note = |m: &str| OverlayState::Note {
            message: m.into(),
            tone: Tone::Warn,
        };
        assert_eq!(note("Not sent").height(), 104);
        let long = "Not sent: Wind has unsent text in its prompt: send or clear it first";
        assert!(
            note(long).height() > 104,
            "the card must grow for two lines"
        );
        assert!(
            note(&"x".repeat(200)).height() <= 104 + 22 * 3,
            "four lines at most"
        );
        let mut s = serving();
        let fx = s.on(SvcEvent::PasteFailed, 1.0);
        assert!(
            has(
                &fx,
                &SvcEffect::WidgetHide {
                    after_ms: WARN_HIDE_MS
                }
            ),
            "{fx:?}"
        );
        let fx = s.on(
            SvcEvent::Routed(RouteResult::Message("Cancelled".into())),
            2.0,
        );
        assert!(
            has(
                &fx,
                &SvcEffect::WidgetHide {
                    after_ms: NOTE_HIDE_MS
                }
            ),
            "{fx:?}"
        );
    }

    /// Review #87: a hook that fails mid-recording cancels the recording instead of leaving the mic open.
    #[test]
    fn a_failed_hook_cancels_a_recording_in_flight() {
        let mut s = serving();
        let fx = s.on(key(true, Some(7)), 10.0);
        assert!(started_ctl(&fx));
        let fx = s.on(SvcEvent::Hook(HookEvent::Failed), 11.0);
        assert!(
            has(&fx, &SvcEffect::ToController(OutsideAction::Cancel)),
            "{fx:?}"
        );
        assert!(has(&fx, &SvcEffect::HookRec(false)), "{fx:?}");
        // The key it held is forgotten: the next press starts afresh.
        let fx = s.on(SvcEvent::Hook(HookEvent::Ready), 20.0);
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::ToController(_))));
        let fx = s.on(key(true, Some(7)), 30.0);
        assert!(started_ctl(&fx), "{fx:?}");
    }

    #[test]
    fn a_failed_hook_with_nothing_recording_cancels_nothing() {
        let mut s = serving();
        let fx = s.on(SvcEvent::Hook(HookEvent::Failed), 11.0);
        assert!(!fx.iter().any(|e| matches!(e, SvcEffect::ToController(_))));
    }

    /* ----- crumbs ----- */

    #[test]
    fn every_key_action_is_a_crumb_without_typed_text() {
        let mut s = serving();
        let mut all = Vec::new();
        all.extend(s.on(key(true, Some(5)), 1.0));
        all.extend(s.on(key(true, Some(5)), 2.0));
        all.extend(s.on(key(true, Some(5)), 3.0));
        all.extend(s.on(key(false, None), 4.0));
        all.extend(s.on(
            SvcEvent::Routed(RouteResult::Text("my secret words".into())),
            5.0,
        ));
        all.extend(s.on(SvcEvent::Hook(HookEvent::Failed), 6.0));
        let crumbs: Vec<String> = all
            .iter()
            .filter_map(|e| match e {
                SvcEffect::Crumb(v) => Some(v.to_string()),
                _ => None,
            })
            .collect();
        assert!(
            crumbs.iter().any(|c| c == r#"{"key":"start"}"#),
            "{crumbs:?}"
        );
        assert!(
            crumbs.iter().any(|c| c == r#"{"hook":"failed"}"#),
            "{crumbs:?}"
        );
        assert!(
            crumbs.iter().any(|c| c == r#"{"role":"none"}"#),
            "{crumbs:?}"
        );
        assert!(crumbs.iter().all(|c| !c.contains("secret")), "{crumbs:?}");
        // The route's crumb carries the window in front, the way and a count, never the words.
        assert!(
            crumbs
                .iter()
                .any(|c| c == r#"{"front":"other","pid":5,"route":"other copy","chars":15}"#),
            "{crumbs:?}"
        );
        // A crumb names only what happened (a key action, a hook state, a role, the way the words went and
        // their length), never anything typed.
        let fields = [
            "key", "refused", "hook", "role", "front", "pid", "route", "chars", "pick", "rows",
            "tabs", "by", "row", "id", "pasted", "sent", "typed", "stop", "phase",
        ];
        for c in &crumbs {
            let v: Value = serde_json::from_str(c).unwrap();
            let o = v.as_object().unwrap();
            assert!(o.keys().all(|k| fields.contains(&k.as_str())), "{c}");
            assert!(
                o.values()
                    .all(|v| v.is_number() || v.is_boolean() || v.is_string()),
                "{c}"
            );
        }
    }

    #[test]
    fn stop_unhooks_closes_the_links_and_the_widget() {
        let mut s = serving();
        assert!(s.on(SvcEvent::Config(cfg(f8(), DictMode::Hold)), 2.0).len() > 1);
        assert!(s.hook_running());
        let fx = s.on(SvcEvent::Stop, 3.0);
        assert!(has(&fx, &SvcEffect::HookStop));
        assert!(has(&fx, &SvcEffect::StopLinks));
        assert!(has(&fx, &SvcEffect::WidgetClose));
        assert!(!s.hook_running());
    }

    /* ----- dictationAnswers (PT: dictationClient.test) ----- */

    #[derive(Default)]
    struct Host {
        tabs: Mutex<Vec<SendTab>>,
        under: Mutex<Option<String>>,
        written: Mutex<Vec<String>>,
        pasted: Mutex<Vec<String>>,
        pauses: Mutex<Vec<u64>>,
    }

    impl ClientHost for Host {
        fn tabs(&self) -> Vec<SendTab> {
            self.tabs.lock().unwrap().clone()
        }
        fn agent_under(&self, _id: &str) -> Option<String> {
            self.under.lock().unwrap().clone()
        }
        fn write(&self, _id: &str, data: &str) -> bool {
            self.written.lock().unwrap().push(data.to_string());
            true
        }
        fn pause(&self, ms: u64) {
            self.pauses.lock().unwrap().push(ms);
        }
        fn paste(&self, text: &str) {
            self.pasted.lock().unwrap().push(text.to_string());
        }
    }

    fn host(under: Option<&str>) -> Arc<Host> {
        let h = Host::default();
        *h.tabs.lock().unwrap() = vec![tab(|_| {})];
        *h.under.lock().unwrap() = under.map(String::from);
        Arc::new(h)
    }

    #[test]
    fn dictation_answers_answers_with_the_tabs_sends_checked_and_pastes_through_the_host() {
        let h = host(Some("claude"));
        let a = dictation_answers(h.clone());
        assert_eq!(a.tabs(), vec![tab(|_| {})]);
        assert!(matches!(a.send("p1", "hi"), SendDecision::Ok(_)));
        assert_eq!(h.written.lock().unwrap()[0], "\u{1b}[200~hi\u{1b}[201~");
        // The Enter follows after 300 ms, once the same tab is still sendable.
        assert_eq!(*h.written.lock().unwrap().last().unwrap(), "\r");
        assert_eq!(*h.pauses.lock().unwrap(), vec![300]);
        assert!(matches!(a.send("nope", "hi"), SendDecision::No(_)));
        a.paste("typed");
        assert_eq!(*h.pasted.lock().unwrap(), vec!["typed".to_string()]);
    }

    #[test]
    fn dictation_answers_types_nothing_where_windows_says_no_claude_runs() {
        let h = host(Some("pwsh"));
        let a = dictation_answers(h.clone());
        assert_eq!(
            a.send("p1", "hi"),
            SendDecision::No(format!(
                "no Claude is running in Wind (checked with {})",
                crate::route::SYSTEM
            ))
        );
        assert!(h.written.lock().unwrap().is_empty());
    }

    #[test]
    fn dictation_answers_never_types_into_a_shell_or_a_question() {
        let h = host(Some("claude"));
        *h.tabs.lock().unwrap() = vec![tab(|t| {
            t.agent = None;
            t.session = None;
        })];
        let a = dictation_answers(h.clone());
        assert!(matches!(a.send("p1", "hi"), SendDecision::No(_)));
        *h.tabs.lock().unwrap() = vec![tab(|t| t.state = Some("question".into()))];
        assert!(matches!(a.send("p1", "hi"), SendDecision::No(_)));
        assert!(h.written.lock().unwrap().is_empty());
    }

    #[test]
    fn dictation_answers_publishes_no_orchestrator_in_flare() {
        let h = host(Some("claude"));
        *h.tabs.lock().unwrap() = vec![tab(|t| t.orchestrator = true)];
        let a = dictation_answers(h);
        assert!(a.tabs().iter().all(|t| !t.orchestrator));
    }
}
