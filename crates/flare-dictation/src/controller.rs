//! The pure dictation state machine (Task 18). Ported from Prism Terminal's
//! `core/renderer/lib/dictation.ts` at e5848cb (key -> microphone -> local engine -> paste), with the outside
//! recording (N8), page keys (N9), auto-warm (N10) and the paste into the tab in front (N11).
//!
//! `Controller::on(Event, now_ms) -> Vec<Effect>` holds every rule and touches nothing: no window, no microphone,
//! no engine, no clock. The app (`dictation_ctl.rs`, Task 22) executes the effects on the UI thread and feeds what
//! happened back as events. The clock is passed in, so every timing rule is tested without waiting.
//!
//! THE RULES IT KEEPS
//! - Off means off: with dictation off the key does nothing, and switching off cancels, stops the engine and
//!   resumes the media.
//! - Live text is provisional and only ever shown; the one thing pasted is the final pass over the whole clip. A
//!   final is never dropped (only a cancel drops it).
//! - The paste goes to the session that was in front when the recording STARTED, as the cleaned text plus one
//!   space: never a newline, never Enter.
//! - A recording that belongs to no tab (`OUTSIDE`) never pastes: what was heard, or why nothing was, goes to the
//!   route sink as a `RouteResult`.
//! - Phase crumbs are phase names only, never text.
//!
//! **[seam]** The plan's `Event::Frame` carried a sample count; the controller needs the audio itself (the final
//! request is the whole clip and a partial is its newest 28 s), so a frame carries its samples. The plan's
//! `Phase` is `controller::Phase` here (the reducer's `hotkey::Phase` is a different thing). `Effect::Level`
//! carries the meter, which Prism Terminal's pill writes straight to the DOM.

use crate::catalog::{language_for, model, Runner};
use crate::clean::clean_transcript;
use crate::cues::Cue;
use crate::engine::{Reason, TranscribeRequest, TranscribeResult};
use crate::hotkey::KeyAction;
use crate::mic::{CaptureFailure, TARGET_RATE};
use crate::prefs::DictPrefs;
use crate::wav::{encode_wav, tail_samples};

/// The session id of a recording that belongs to no tab (a recording from another app).
pub const OUTSIDE: &str = "\u{0}outside";
/// A recording is capped: nobody dictates for five minutes on purpose, and a key held down by a book must not
/// fill memory.
pub const MAX_SECONDS: f64 = 300.0;
/// Whisper's window. A partial only ever needs the newest of it.
pub const PARTIAL_WINDOW_S: f64 = 28.0;
/// Below this there is nothing to transcribe: a brush of the key.
pub const MIN_SECONDS: f64 = 0.35;
/// How often the app sends `Event::PartialTick` while a recording listens.
pub const PARTIAL_EVERY_MS: u64 = 350;
/// The least new audio between two partials: the resident Whisper server takes one every 0.6 s.
pub const PARTIAL_GAP_WHISPER_S: f64 = 0.6;
/// A Parakeet partial is a fresh pass (0.7 to 0.9 s, measured), so about one a second.
pub const PARTIAL_GAP_PARAKEET_S: f64 = 1.0;
/// How long a transient message stays on the pill.
pub const MESSAGE_MS: f64 = 2600.0;
/// The engine is warmed this long after dictation is first armed (the app has just opened).
pub const WARM_FIRST_MS: u64 = 3000;
/// And this long after a re-sync (a model or language change, any other setting).
pub const WARM_RESYNC_MS: u64 = 500;

/// A frame is taken in only up to this much audio (the cap plus a little for the tick that notices it).
const CLIP_CAP: usize = ((MAX_SECONDS + 2.0) * TARGET_RATE as f64) as usize;

const NO_MODEL: &str = "Choose a model in Settings";
const NO_MODEL_OUTSIDE: &str = "Choose a speech model in Settings";
const NO_ENGINE: &str = "The speech engine is missing. Reinstall the app.";
const ENGINE_STOPPED: &str = "The speech engine stopped. Try again.";
const TAB_CLOSED: &str = "That tab has closed";
const HEARD_NOTHING: &str = "Heard nothing";
const CANCELLED: &str = "Cancelled";

/// What the pill shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Listening,
    Transcribing,
}

impl Phase {
    /// The crumb for a phase change: the name, never text.
    pub fn name(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Listening => "listening",
            Phase::Transcribing => "transcribing",
        }
    }
}

/// An outside recording (from another app) asked for by the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutsideAction {
    Start,
    Stop,
    Cancel,
}

/// Who reads the key (N9, N10, N17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Nobody else reads the key: this window does, and warms its own engine.
    None,
    /// This copy runs the hook and the dictation for every copy.
    Service,
    /// Another copy runs the dictation: this window reads no key and warms no engine.
    Client,
}

/// What an outside recording hands to the route sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteResult {
    Text(String),
    Message(String),
}

/// Something that happened.
#[derive(Debug, Clone)]
pub enum Event {
    /// The window's own key reducer decided (only read while this window reads the key, N9).
    Key(KeyAction),
    /// The microphone opened.
    MicOpened,
    /// The microphone could not be opened.
    MicFailed(CaptureFailure),
    /// A frame of 16 kHz mono samples and its meter level (0 to 1).
    Frame {
        level: f32,
        samples: Vec<f32>,
    },
    /// The app's 350 ms tick (partials, and the message's expiry). Needed while `needs_tick()`.
    PartialTick,
    /// The engine answered the pass `id`.
    Heard {
        id: u64,
        result: TranscribeResult,
    },
    /// The hook asked for a recording from another app (N8).
    Outside(OutsideAction),
    Role(Role),
    /// The preferences as they are now (also the first call that arms dictation).
    Prefs(DictPrefs),
    /// The window lost focus: a recording this window's key started, still listening, is cancelled (the key
    /// reducer cancels on a blur too; the second cancel is a no-op).
    Blur,
    /// The shell tab in front now (`None`: no shell with a live session).
    Front(Option<String>),
    /// Words another copy routed back because this window was in front (N11): pasted as text plus one space\n    /// into the shell in front, never Enter. Nothing when no shell is in front.
    PasteFront(String),
    /// This tab is gone.
    TabGone(String),
    /// The app is quitting.
    Quit,
}

/// Something for the app to do.
#[derive(Debug, Clone)]
pub enum Effect {
    /// Open the microphone (`None`: the system default). Answered with `MicOpened` or `MicFailed`.
    OpenMic(Option<String>),
    CloseMic,
    /// Run a pass; answered with `Heard { id, .. }`.
    Transcribe {
        id: u64,
        req: TranscribeRequest,
    },
    /// Paste at the cursor of this session's shell, as typed text, never Enter.
    Paste {
        session: String,
        text: String,
    },
    /// Hand an outside recording's result to the route sink.
    Route(RouteResult),
    Cue(Cue),
    PauseMedia,
    ResumeMedia,
    /// The pill's state changed.
    View(View),
    /// The meter, 0 to 1, many times a second.
    Level(f32),
    /// Have the engine ready for the current model and language, after this long. A later `Warm` replaces an
    /// earlier one; `StopEngine` cancels it.
    Warm {
        after_ms: u64,
    },
    StopEngine,
    /// A phase crumb for the diag timeline: the phase's name only.
    Crumb(&'static str),
}

/// What the pill, the tab mark and the widget show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub phase: Phase,
    /// The session being dictated into (`OUTSIDE` for a recording from another app), for the mark on its tab.
    pub session: Option<String>,
    /// The provisional text while listening. Shown, never pasted.
    pub live_text: String,
    /// The speech engine is still starting, so there is no live text yet.
    pub starting: bool,
    /// A short line for the pill when something needs saying.
    pub message: Option<String>,
}

impl View {
    pub fn idle() -> Self {
        View {
            phase: Phase::Idle,
            session: None,
            live_text: String::new(),
            starting: false,
            message: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Session(String),
    Outside,
}

impl Target {
    fn is_outside(&self) -> bool {
        matches!(self, Target::Outside)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    Stop,
    Cancel,
}

struct Rec {
    target: Target,
    mic_open: bool,
    /// Set when stop or cancel arrives, before the mic has opened or after. Once set, a second stop is not
    /// heard: a hold released while the last clip transcribes used to finish THAT clip again and paste twice.
    ended: Option<Ended>,
    /// Media was paused for this recording, so it is resumed on every path out.
    paused: bool,
    samples: Vec<f32>,
    partial: Option<u64>,
    /// The clip length, in samples, when the last partial was asked.
    last_partial_at: usize,
    final_id: Option<u64>,
    /// The tab it will paste into has closed.
    gone: bool,
}

/// The state machine. See the module comment.
pub struct Controller {
    prefs: DictPrefs,
    role: Role,
    armed: bool,
    has_sink: bool,
    front: Option<String>,
    view: View,
    message_until: Option<f64>,
    rec: Option<Rec>,
    next_id: u64,
}

impl Controller {
    /// A controller that is not armed yet: send `Event::Prefs` to arm it.
    pub fn new(prefs: DictPrefs) -> Self {
        Controller {
            prefs,
            role: Role::None,
            armed: false,
            has_sink: false,
            front: None,
            view: View::idle(),
            message_until: None,
            rec: None,
            next_id: 1,
        }
    }

    /// The pill's state now.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Whether this window reads the dictation key itself (N9): only with nobody else reading it.
    pub fn page_keys(&self) -> bool {
        self.role == Role::None
    }

    /// Whether this window warms the engine on its own (N10).
    pub fn auto_warm(&self) -> bool {
        self.role != Role::Client
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Whether dictation is armed (switched on).
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Whether a recording exists (opening, listening or transcribing).
    pub fn recording(&self) -> bool {
        self.rec.is_some()
    }

    /// Whether the app must keep sending `PartialTick` (every `PARTIAL_EVERY_MS`): a recording listens, or a
    /// message is waiting to expire.
    pub fn needs_tick(&self) -> bool {
        self.message_until.is_some()
            || self
                .rec
                .as_ref()
                .is_some_and(|r| r.mic_open && r.ended.is_none())
    }

    /// Whether the host has a route sink (an outside recording starts only with one).
    pub fn set_sink(&mut self, on: bool) {
        self.has_sink = on;
    }

    pub fn on(&mut self, e: Event, now_ms: f64) -> Vec<Effect> {
        let mut fx = Vec::new();
        match e {
            Event::Key(a) => self.on_key(&mut fx, now_ms, a),
            Event::MicOpened => self.on_mic_opened(&mut fx, now_ms),
            Event::MicFailed(f) => self.on_mic_failed(&mut fx, now_ms, f),
            Event::Frame { level, samples } => self.on_frame(&mut fx, level, &samples),
            Event::PartialTick => self.on_tick(&mut fx, now_ms),
            Event::Heard { id, result } => self.on_heard(&mut fx, now_ms, id, result),
            Event::Outside(a) => self.on_outside(&mut fx, now_ms, a),
            Event::Role(r) => self.on_role(&mut fx, r),
            Event::Prefs(p) => self.on_prefs(&mut fx, now_ms, p),
            Event::Blur => {
                let from_window = self.rec.as_ref().is_some_and(|r| {
                    !r.target.is_outside() && r.ended.is_none() && self.role == Role::None
                });
                if from_window {
                    self.cancel(&mut fx, now_ms);
                }
            }
            Event::Front(f) => self.front = f,
            Event::PasteFront(text) => {
                if let Some(session) = self.front.clone() {
                    fx.push(Effect::Paste {
                        session,
                        text: format!("{text} "),
                    });
                }
            }
            Event::TabGone(id) => {
                if let Some(r) = self.rec.as_mut() {
                    if r.target == Target::Session(id) {
                        r.gone = true;
                    }
                }
            }
            Event::Quit => self.on_quit(&mut fx),
        }
        fx
    }

    // ---- events ----

    fn on_key(&mut self, fx: &mut Vec<Effect>, now: f64, a: KeyAction) {
        // Another reader has the key, or dictation is off: nothing is read here.
        if !self.page_keys() || !self.prefs.enabled {
            return;
        }
        match a {
            KeyAction::Start => {
                if let Some(id) = self.front.clone() {
                    self.begin(fx, now, Target::Session(id));
                }
            }
            KeyAction::Stop => self.stop(fx, now),
            KeyAction::Cancel => self.cancel(fx, now),
        }
    }

    fn on_outside(&mut self, fx: &mut Vec<Effect>, now: f64, a: OutsideAction) {
        match a {
            OutsideAction::Start => {
                if !self.has_sink || !self.prefs.enabled {
                    return;
                }
                // No model: said in the host's widget, where the owner is looking.
                if self.prefs.model.is_empty() {
                    fx.push(Effect::Route(RouteResult::Message(NO_MODEL_OUTSIDE.into())));
                    return;
                }
                self.begin(fx, now, Target::Outside);
            }
            OutsideAction::Stop => self.stop(fx, now),
            OutsideAction::Cancel => self.cancel(fx, now),
        }
    }

    fn on_mic_opened(&mut self, fx: &mut Vec<Effect>, now: f64) {
        let Some(r) = self.rec.as_mut() else {
            return;
        };
        if r.mic_open {
            return;
        }
        r.mic_open = true;
        // The key was released (or Escape pressed) while the mic was still opening.
        match r.ended {
            Some(Ended::Cancel) => self.finish_cancel(fx, now),
            Some(Ended::Stop) => self.finish_stop(fx),
            None => {}
        }
    }

    fn on_mic_failed(&mut self, fx: &mut Vec<Effect>, now: f64, f: CaptureFailure) {
        let Some(r) = self.rec.take() else {
            return;
        };
        if r.paused {
            fx.push(Effect::ResumeMedia);
        }
        self.tell(fx, now, r.target.is_outside(), f.text());
    }

    fn on_frame(&mut self, fx: &mut Vec<Effect>, level: f32, samples: &[f32]) {
        let Some(r) = self.rec.as_mut() else {
            return;
        };
        if !r.mic_open || r.ended.is_some() {
            return;
        }
        let room = CLIP_CAP.saturating_sub(r.samples.len());
        r.samples
            .extend_from_slice(&samples[..samples.len().min(room)]);
        fx.push(Effect::Level(level));
    }

    fn on_tick(&mut self, fx: &mut Vec<Effect>, now: f64) {
        if let Some(until) = self.message_until {
            if now >= until {
                self.message_until = None;
                if self.view.phase == Phase::Idle && self.view.message.is_some() {
                    let mut v = self.view.clone();
                    v.message = None;
                    self.show(fx, v);
                }
            }
        }
        let Some(r) = self.rec.as_ref() else {
            return;
        };
        if !r.mic_open || r.ended.is_some() {
            return;
        }
        // Counted in samples, so 0.6 s of new audio is exactly 9600 of them, never 0.5999999.
        let rate = f64::from(TARGET_RATE);
        let have = r.samples.len();
        if have as f64 >= MAX_SECONDS * rate {
            self.stop(fx, now);
            return;
        }
        let gap = match model(&self.prefs.model).map(|m| m.runner) {
            Some(Runner::Parakeet) => PARTIAL_GAP_PARAKEET_S,
            _ => PARTIAL_GAP_WHISPER_S,
        };
        let gap = (gap * rate).round() as usize;
        if r.partial.is_some() || (have as f64) < rate || have - r.last_partial_at < gap {
            return;
        }
        let wav = encode_wav(
            tail_samples(&r.samples, TARGET_RATE, PARTIAL_WINDOW_S),
            TARGET_RATE,
        );
        let id = self.take_id();
        let req = TranscribeRequest {
            wav,
            model_id: self.prefs.model.clone(),
            language: language_for(&self.prefs.model, &self.prefs.language).to_string(),
            final_: false,
        };
        if let Some(r) = self.rec.as_mut() {
            r.partial = Some(id);
            r.last_partial_at = have;
        }
        fx.push(Effect::Transcribe { id, req });
    }

    fn on_heard(&mut self, fx: &mut Vec<Effect>, now: f64, id: u64, result: TranscribeResult) {
        let Some(r) = self.rec.as_mut() else {
            return;
        };
        if r.partial == Some(id) {
            r.partial = None;
            // Only while THIS recording is still listening: a partial that lands after the final would
            // overwrite the pill with older words.
            if self.view.phase != Phase::Listening {
                return;
            }
            let mut v = self.view.clone();
            match result {
                TranscribeResult::Ok(text) => {
                    v.live_text = clean_transcript(&text);
                    v.starting = false;
                }
                TranscribeResult::No {
                    reason: Reason::Warming,
                    ..
                } => v.starting = true,
                TranscribeResult::No { .. } => {}
            }
            self.show(fx, v);
        } else if r.final_id == Some(id) {
            self.on_final(fx, now, result);
        }
    }

    fn on_final(&mut self, fx: &mut Vec<Effect>, now: f64, result: TranscribeResult) {
        let Some(r) = self.rec.take() else {
            return;
        };
        if r.paused {
            fx.push(Effect::ResumeMedia);
        }
        let outside = r.target.is_outside();
        let raw = match result {
            TranscribeResult::Ok(t) => t,
            TranscribeResult::No { reason, .. } => {
                let said = match reason {
                    Reason::NoModel => NO_MODEL,
                    Reason::NoEngine => NO_ENGINE,
                    _ => ENGINE_STOPPED,
                };
                self.tell(fx, now, outside, said);
                return;
            }
        };
        let text = clean_transcript(&raw);
        if text.is_empty() {
            self.tell(fx, now, outside, HEARD_NOTHING);
            return;
        }
        match r.target {
            // From outside the app the host routes it (and presses Enter there, by the checked send), never a
            // paste into whatever tab is in front.
            Target::Outside => {
                fx.push(Effect::Route(RouteResult::Text(text)));
                self.show(fx, View::idle());
            }
            Target::Session(session) => {
                if r.gone {
                    self.say(fx, now, TAB_CLOSED);
                } else {
                    // A trailing space, so the next dictation (or the next typed word) does not run into this
                    // one. Never a newline: that is Enter.
                    fx.push(Effect::Paste {
                        session,
                        text: format!("{text} "),
                    });
                    self.show(fx, View::idle());
                }
            }
        }
    }

    fn on_role(&mut self, fx: &mut Vec<Effect>, r: Role) {
        self.role = r;
        // One dictation: with a service in reach the hook reads the key, so this window does not (page_keys);
        // only the service's copy keeps an engine warm.
        if r == Role::Client {
            fx.push(Effect::StopEngine);
        } else if r == Role::Service && self.prefs.enabled && !self.prefs.model.is_empty() {
            fx.push(Effect::Warm { after_ms: 0 });
        }
    }

    fn on_prefs(&mut self, fx: &mut Vec<Effect>, now: f64, p: DictPrefs) {
        self.prefs = p;
        let want = self.prefs.enabled;
        // Warm the engine a moment after dictation is armed (the app has just opened), and again when its model
        // or language changes; off means off, so nothing is warmed while it is off.
        if want && self.auto_warm() && !self.prefs.model.is_empty() {
            fx.push(Effect::Warm {
                after_ms: if self.armed {
                    WARM_RESYNC_MS
                } else {
                    WARM_FIRST_MS
                },
            });
        }
        if want == self.armed {
            return;
        }
        self.armed = want;
        if !want {
            // Off means off: nothing is pasted, no server process may remain.
            self.cancel(fx, now);
            fx.push(Effect::StopEngine);
        }
    }

    fn on_quit(&mut self, fx: &mut Vec<Effect>) {
        self.armed = false;
        if let Some(r) = self.rec.take() {
            fx.push(Effect::CloseMic);
            fx.push(Effect::Level(0.0));
            if r.paused {
                fx.push(Effect::ResumeMedia);
            }
        }
        self.message_until = None;
        self.show(fx, View::idle());
        fx.push(Effect::StopEngine);
    }

    // ---- the recording ----

    fn begin(&mut self, fx: &mut Vec<Effect>, now: f64, target: Target) {
        if self.rec.is_some() {
            return;
        }
        if self.prefs.model.is_empty() {
            self.say(fx, now, NO_MODEL);
            return;
        }
        let paused = self.prefs.pause_media;
        if paused {
            fx.push(Effect::PauseMedia);
        }
        let session = match &target {
            Target::Session(id) => id.clone(),
            Target::Outside => OUTSIDE.to_string(),
        };
        self.rec = Some(Rec {
            target,
            mic_open: false,
            ended: None,
            paused,
            samples: Vec::new(),
            partial: None,
            last_partial_at: 0,
            final_id: None,
            gone: false,
        });
        self.message_until = None;
        self.show(
            fx,
            View {
                phase: Phase::Listening,
                session: Some(session),
                live_text: String::new(),
                starting: false,
                message: None,
            },
        );
        // The engine may have stood down: it warms while you speak, so the words are not waiting on it when
        // you let go.
        fx.push(Effect::Warm { after_ms: 0 });
        if self.prefs.sounds {
            fx.push(Effect::Cue(Cue::Start));
        }
        let mic = (!self.prefs.mic.is_empty()).then(|| self.prefs.mic.clone());
        fx.push(Effect::OpenMic(mic));
    }

    fn stop(&mut self, fx: &mut Vec<Effect>, _now: f64) {
        let Some(r) = self.rec.as_mut() else {
            return;
        };
        if r.ended.is_some() {
            return;
        }
        r.ended = Some(Ended::Stop);
        // Without an open mic `on_mic_opened` finishes it.
        if r.mic_open {
            self.finish_stop(fx);
        }
    }

    fn cancel(&mut self, fx: &mut Vec<Effect>, now: f64) {
        let Some(r) = self.rec.as_mut() else {
            return;
        };
        if r.ended == Some(Ended::Cancel) {
            return;
        }
        r.ended = Some(Ended::Cancel);
        if r.mic_open {
            self.finish_cancel(fx, now);
        }
    }

    fn end_capture(&mut self, fx: &mut Vec<Effect>) {
        fx.push(Effect::CloseMic);
        fx.push(Effect::Level(0.0));
    }

    fn finish_cancel(&mut self, fx: &mut Vec<Effect>, _now: f64) {
        self.end_capture(fx);
        let Some(r) = self.rec.take() else {
            return;
        };
        if r.paused {
            fx.push(Effect::ResumeMedia);
        }
        if self.prefs.sounds {
            fx.push(Effect::Cue(Cue::Cancel));
        }
        if r.target.is_outside() {
            fx.push(Effect::Route(RouteResult::Message(CANCELLED.into())));
        }
        self.message_until = None;
        self.show(fx, View::idle());
    }

    fn finish_stop(&mut self, fx: &mut Vec<Effect>) {
        self.end_capture(fx);
        if self.prefs.sounds {
            fx.push(Effect::Cue(Cue::Stop));
        }
        let long_enough = self
            .rec
            .as_ref()
            .is_some_and(|r| r.samples.len() as f64 >= MIN_SECONDS * f64::from(TARGET_RATE));
        if !long_enough {
            // A brush of the key: dropped silently (the sink is told, the tab is not).
            if let Some(r) = self.rec.take() {
                if r.paused {
                    fx.push(Effect::ResumeMedia);
                }
                if r.target.is_outside() {
                    fx.push(Effect::Route(RouteResult::Message(HEARD_NOTHING.into())));
                }
            }
            self.message_until = None;
            self.show(fx, View::idle());
            return;
        }
        let id = self.take_id();
        let req = {
            let r = self.rec.as_mut().expect("checked above");
            r.final_id = Some(id);
            TranscribeRequest {
                wav: encode_wav(&r.samples, TARGET_RATE),
                model_id: self.prefs.model.clone(),
                language: language_for(&self.prefs.model, &self.prefs.language).to_string(),
                final_: true,
            }
        };
        let mut v = self.view.clone();
        v.phase = Phase::Transcribing;
        self.show(fx, v);
        fx.push(Effect::Transcribe { id, req });
    }

    // ---- the pill ----

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Sets the view; a phase change leaves a crumb (the name only: never the text).
    fn show(&mut self, fx: &mut Vec<Effect>, v: View) {
        if v == self.view {
            return;
        }
        if v.phase != self.view.phase {
            fx.push(Effect::Crumb(v.phase.name()));
        }
        fx.push(Effect::View(v.clone()));
        self.view = v;
    }

    /// Said to the sink when the recording is the outside one, else on the pill.
    fn tell(&mut self, fx: &mut Vec<Effect>, now: f64, outside: bool, message: &str) {
        if outside {
            fx.push(Effect::Route(RouteResult::Message(message.to_string())));
            self.message_until = None;
            self.show(fx, View::idle());
        } else {
            self.say(fx, now, message);
        }
    }

    fn say(&mut self, fx: &mut Vec<Effect>, now: f64, message: &str) {
        let mut v = View::idle();
        v.message = Some(message.to_string());
        self.message_until = Some(now + MESSAGE_MS);
        self.show(fx, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::{default_hotkey, DictMode};

    const TAB: &str = "tab-1";

    fn prefs() -> DictPrefs {
        DictPrefs {
            enabled: true,
            mode: DictMode::Hold,
            hotkey: default_hotkey(),
            mic: String::new(),
            language: "en".into(),
            pause_media: false,
            sounds: false,
            model: "tiny".into(),
            anywhere: true,
        }
    }

    /// A controller that has seen its preferences and has a shell in front.
    struct H {
        c: Controller,
        now: f64,
        all: Vec<Effect>,
    }

    impl H {
        fn with(p: DictPrefs) -> H {
            let mut h = H {
                c: Controller::new(p.clone()),
                now: 0.0,
                all: Vec::new(),
            };
            h.c.set_sink(true);
            h.ev(Event::Prefs(p));
            h.ev(Event::Front(Some(TAB.into())));
            h.all.clear();
            h
        }
        fn new() -> H {
            H::with(prefs())
        }
        fn ev(&mut self, e: Event) -> Vec<Effect> {
            let fx = self.c.on(e, self.now);
            self.all.extend(fx.iter().cloned());
            fx
        }
        fn at(&mut self, ms: f64) -> &mut H {
            self.now = ms;
            self
        }
        fn set(&mut self, f: impl FnOnce(&mut DictPrefs)) -> Vec<Effect> {
            let mut p = self.c.prefs.clone();
            f(&mut p);
            self.ev(Event::Prefs(p))
        }
        /// Press the key and open the microphone.
        fn listen(&mut self) {
            self.ev(Event::Key(KeyAction::Start));
            self.ev(Event::MicOpened);
        }
        fn listen_outside(&mut self) {
            self.ev(Event::Role(Role::Service));
            self.ev(Event::Outside(OutsideAction::Start));
            self.ev(Event::MicOpened);
        }
        fn speak(&mut self, secs: f64) {
            let n = (secs * 16000.0) as usize;
            self.ev(Event::Frame {
                level: 0.5,
                samples: vec![0.1; n],
            });
        }
        fn tick(&mut self) -> Vec<Effect> {
            self.ev(Event::PartialTick)
        }
        fn heard(&mut self, id: u64, text: &str) -> Vec<Effect> {
            self.ev(Event::Heard {
                id,
                result: TranscribeResult::Ok(text.into()),
            })
        }
        fn fail(&mut self, id: u64, reason: Reason) -> Vec<Effect> {
            self.ev(Event::Heard {
                id,
                result: TranscribeResult::No {
                    reason,
                    detail: None,
                },
            })
        }
        /// Press, speak, release: the final pass is asked.
        fn dictate(&mut self, secs: f64) -> u64 {
            self.listen();
            self.speak(secs);
            let fx = self.ev(Event::Key(KeyAction::Stop));
            final_id(&fx).expect("a final was asked")
        }
    }

    fn final_id(fx: &[Effect]) -> Option<u64> {
        fx.iter().find_map(|e| match e {
            Effect::Transcribe { id, req } if req.final_ => Some(*id),
            _ => None,
        })
    }

    fn partial_id(fx: &[Effect]) -> Option<u64> {
        fx.iter().find_map(|e| match e {
            Effect::Transcribe { id, req } if !req.final_ => Some(*id),
            _ => None,
        })
    }

    fn request(fx: &[Effect]) -> Option<&TranscribeRequest> {
        fx.iter().find_map(|e| match e {
            Effect::Transcribe { req, .. } => Some(req),
            _ => None,
        })
    }

    fn pastes(fx: &[Effect]) -> Vec<(String, String)> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Paste { session, text } => Some((session.clone(), text.clone())),
                _ => None,
            })
            .collect()
    }

    fn routes(fx: &[Effect]) -> Vec<RouteResult> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Route(r) => Some(r.clone()),
                _ => None,
            })
            .collect()
    }

    fn count(fx: &[Effect], f: impl Fn(&Effect) -> bool) -> usize {
        fx.iter().filter(|e| f(e)).count()
    }

    fn resumes(fx: &[Effect]) -> usize {
        count(fx, |e| matches!(e, Effect::ResumeMedia))
    }

    fn warms(fx: &[Effect]) -> Vec<u64> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Warm { after_ms } => Some(*after_ms),
                _ => None,
            })
            .collect()
    }

    fn message(h: &H) -> Option<String> {
        h.c.view().message.clone()
    }

    fn wav_samples(req: &TranscribeRequest) -> usize {
        (req.wav.len() - 44) / 2
    }

    // ---- Prism Terminal's two tests, by name ----

    #[test]
    fn a_second_hold_while_the_last_clip_is_transcribing_pastes_the_clip_once() {
        let mut h = H::new();
        h.listen();
        h.speak(2.0);
        let first = final_id(&h.ev(Event::Key(KeyAction::Stop))).unwrap();
        assert_eq!(h.c.view().phase, Phase::Transcribing);
        // The next sentence, while the first is still being heard.
        let again = h.ev(Event::Key(KeyAction::Start));
        let again2 = h.ev(Event::Key(KeyAction::Stop));
        assert!(again.is_empty() && again2.is_empty());
        let landed = h.heard(first, "hello there");
        // A second final, if one had been asked for.
        let later = h.heard(first, "hello there");
        assert_eq!(pastes(&landed), vec![(TAB.into(), "hello there ".into())]);
        assert!(pastes(&later).is_empty());
        assert_eq!(pastes(&h.all).len(), 1);
    }

    #[test]
    fn escape_while_transcribing_drops_the_clip() {
        let mut h = H::new();
        let id = h.dictate(2.0);
        assert_eq!(h.c.view().phase, Phase::Transcribing);
        h.ev(Event::Key(KeyAction::Cancel));
        let late = h.heard(id, "never pasted");
        assert!(pastes(&late).is_empty());
        assert!(pastes(&h.all).is_empty());
        assert_eq!(h.c.view().phase, Phase::Idle);
    }

    // ---- off means off ----

    #[test]
    fn dictation_is_off_by_default_nothing_starts() {
        let mut h = H::with(DictPrefs {
            enabled: false,
            ..prefs()
        });
        assert!(h.ev(Event::Key(KeyAction::Start)).is_empty());
        assert!(h.ev(Event::Outside(OutsideAction::Start)).is_empty());
        assert!(warms(&h.all).is_empty());
        assert!(!h.c.recording());
    }

    #[test]
    fn with_dictation_off_the_key_does_nothing() {
        let mut h = H::new();
        h.set(|p| p.enabled = false);
        assert!(h.ev(Event::Key(KeyAction::Start)).is_empty());
        assert!(h.ev(Event::Key(KeyAction::Stop)).is_empty());
        assert!(!h.c.recording());
    }

    #[test]
    fn switching_off_mid_recording_pastes_nothing_and_stops() {
        let mut h = H::new();
        h.listen();
        h.speak(2.0);
        let fx = h.set(|p| p.enabled = false);
        assert!(count(&fx, |e| matches!(e, Effect::CloseMic)) == 1);
        assert!(count(&fx, |e| matches!(e, Effect::StopEngine)) == 1);
        assert!(pastes(&fx).is_empty());
        assert!(request(&fx).is_none());
        assert_eq!(h.c.view().phase, Phase::Idle);
        assert!(!h.c.recording());
    }

    #[test]
    fn switching_off_while_transcribing_drops_the_answer_and_resumes_media() {
        let mut h = H::with(DictPrefs {
            pause_media: true,
            ..prefs()
        });
        let id = h.dictate(2.0);
        let fx = h.set(|p| p.enabled = false);
        assert_eq!(resumes(&fx), 1);
        assert!(count(&fx, |e| matches!(e, Effect::StopEngine)) == 1);
        assert!(pastes(&h.heard(id, "late")).is_empty());
    }

    #[test]
    fn switching_off_before_the_mic_opened_is_honoured_on_arrival() {
        let mut h = H::new();
        h.ev(Event::Key(KeyAction::Start));
        h.set(|p| p.enabled = false);
        let fx = h.ev(Event::MicOpened);
        assert_eq!(count(&fx, |e| matches!(e, Effect::CloseMic)), 1);
        assert!(!h.c.recording());
    }

    // ---- starting ----

    #[test]
    fn recording_starts_only_when_a_model_is_stored() {
        let mut h = H::with(DictPrefs {
            model: String::new(),
            ..prefs()
        });
        let fx = h.ev(Event::Key(KeyAction::Start));
        assert!(count(&fx, |e| matches!(e, Effect::OpenMic(_))) == 0);
        assert_eq!(message(&h).as_deref(), Some("Choose a model in Settings"));
        assert!(!h.c.recording());
    }

    #[test]
    fn a_press_with_no_shell_in_front_does_nothing() {
        let mut h = H::new();
        h.ev(Event::Front(None));
        assert!(h.ev(Event::Key(KeyAction::Start)).is_empty());
    }

    #[test]
    fn a_press_opens_the_mic_named_in_the_preferences() {
        let mut h = H::with(DictPrefs {
            mic: "{0.0.1}.{abc}".into(),
            ..prefs()
        });
        let fx = h.ev(Event::Key(KeyAction::Start));
        assert!(fx
            .iter()
            .any(|e| matches!(e, Effect::OpenMic(Some(m)) if m == "{0.0.1}.{abc}")));
        let mut h = H::new();
        let fx = h.ev(Event::Key(KeyAction::Start));
        assert!(fx.iter().any(|e| matches!(e, Effect::OpenMic(None))));
    }

    #[test]
    fn a_recording_while_another_is_open_is_ignored() {
        let mut h = H::new();
        h.listen();
        assert!(h.ev(Event::Key(KeyAction::Start)).is_empty());
        assert!(h.ev(Event::Outside(OutsideAction::Start)).is_empty());
    }

    #[test]
    fn the_paste_target_is_the_session_in_front_at_the_start() {
        let mut h = H::new();
        h.listen();
        h.speak(2.0);
        // Switching tabs mid-sentence must not drop the words into another agent's prompt.
        h.ev(Event::Front(Some("tab-2".into())));
        let id = final_id(&h.ev(Event::Key(KeyAction::Stop))).unwrap();
        assert_eq!(
            pastes(&h.heard(id, "words")),
            vec![(TAB.to_string(), "words ".to_string())]
        );
    }

    #[test]
    fn the_view_names_the_session_for_the_tab_mark() {
        let mut h = H::new();
        h.ev(Event::Key(KeyAction::Start));
        assert_eq!(h.c.view().session.as_deref(), Some(TAB));
        assert_eq!(h.c.view().phase, Phase::Listening);
        let mut h = H::new();
        h.listen_outside();
        assert_eq!(h.c.view().session.as_deref(), Some(OUTSIDE));
    }

    #[test]
    fn a_tab_that_closed_says_that_tab_has_closed() {
        let mut h = H::new();
        h.listen();
        h.speak(2.0);
        let id = final_id(&h.ev(Event::Key(KeyAction::Stop))).unwrap();
        h.ev(Event::TabGone(TAB.into()));
        let fx = h.heard(id, "words");
        assert!(pastes(&fx).is_empty());
        assert_eq!(message(&h).as_deref(), Some("That tab has closed"));
    }

    #[test]
    fn another_tab_closing_changes_nothing() {
        let mut h = H::new();
        h.listen();
        h.speak(2.0);
        let id = final_id(&h.ev(Event::Key(KeyAction::Stop))).unwrap();
        h.ev(Event::TabGone("tab-9".into()));
        assert_eq!(pastes(&h.heard(id, "words")).len(), 1);
    }

    // ---- the microphone ----

    #[test]
    fn mic_failure_texts() {
        let table = [
            (CaptureFailure::NoDevice, "No microphone found"),
            (CaptureFailure::Denied, "Microphone access was refused"),
            (CaptureFailure::Failed, "The microphone could not be opened"),
            (
                CaptureFailure::Unsupported,
                "The microphone could not be opened",
            ),
        ];
        for (f, text) in table {
            let mut h = H::new();
            h.ev(Event::Key(KeyAction::Start));
            h.ev(Event::MicFailed(f));
            assert_eq!(message(&h).as_deref(), Some(text), "{f:?}");
            assert!(!h.c.recording());
            assert_eq!(h.c.view().phase, Phase::Idle);
        }
    }

    #[test]
    fn a_stale_mic_answer_with_no_recording_does_nothing() {
        let mut h = H::new();
        assert!(h.ev(Event::MicOpened).is_empty());
        assert!(h.ev(Event::MicFailed(CaptureFailure::Failed)).is_empty());
    }

    #[test]
    fn a_release_before_the_mic_opened_is_honoured_on_arrival() {
        let mut h = H::new();
        h.ev(Event::Key(KeyAction::Start));
        assert!(h.ev(Event::Key(KeyAction::Stop)).is_empty());
        // Nothing was heard: a brush of the key, dropped silently.
        let fx = h.ev(Event::MicOpened);
        assert_eq!(count(&fx, |e| matches!(e, Effect::CloseMic)), 1);
        assert!(request(&fx).is_none());
        assert!(!h.c.recording());
        assert_eq!(h.c.view().phase, Phase::Idle);
    }

    #[test]
    fn escape_before_the_mic_opened_is_honoured_on_arrival() {
        let mut h = H::new();
        h.ev(Event::Key(KeyAction::Start));
        assert!(h.ev(Event::Key(KeyAction::Cancel)).is_empty());
        assert_eq!(h.c.view().phase, Phase::Listening);
        let fx = h.ev(Event::MicOpened);
        assert_eq!(count(&fx, |e| matches!(e, Effect::CloseMic)), 1);
        assert_eq!(h.c.view().phase, Phase::Idle);
        assert!(!h.c.recording());
    }

    #[test]
    fn frames_after_the_stop_are_ignored() {
        let mut h = H::new();
        let id = h.dictate(2.0);
        let fx = h.ev(Event::Frame {
            level: 0.9,
            samples: vec![0.2; 16000],
        });
        assert!(fx.is_empty());
        assert_eq!(pastes(&h.heard(id, "ok")).len(), 1);
    }

    #[test]
    fn the_level_is_forwarded_and_zero_when_the_capture_ends() {
        let mut h = H::new();
        h.listen();
        let fx = h.ev(Event::Frame {
            level: 0.5,
            samples: vec![0.1; 160],
        });
        assert!(matches!(fx.as_slice(), [Effect::Level(l)] if (*l - 0.5).abs() < 1e-6));
        let fx = h.ev(Event::Key(KeyAction::Cancel));
        assert!(fx
            .iter()
            .any(|e| matches!(e, Effect::Level(l) if *l == 0.0)));
    }

    // ---- partials ----

    #[test]
    fn a_recording_is_capped_at_300_s() {
        let mut h = H::new();
        h.listen();
        h.speak(299.0);
        let fx = h.tick();
        assert!(final_id(&fx).is_none(), "299 s is under the cap");
        h.speak(1.0);
        h.at(400.0);
        let fx = h.tick();
        assert!(final_id(&fx).is_some(), "300 s stops the recording");
        assert_eq!(h.c.view().phase, Phase::Transcribing);
    }

    #[test]
    fn a_partial_needs_a_second_recorded_and_0_6_s_new_audio() {
        let mut h = H::new();
        h.listen();
        h.speak(0.9);
        assert!(h.tick().is_empty(), "under a second");
        h.speak(0.2);
        let fx = h.tick();
        assert!(partial_id(&fx).is_some(), "1.1 s recorded");
        let id = partial_id(&fx).unwrap();
        h.heard(id, "one");
        h.speak(0.5);
        assert!(partial_id(&h.tick()).is_none(), "only 0.5 s new");
        h.speak(0.1);
        assert!(partial_id(&h.tick()).is_some(), "0.6 s new");
    }

    #[test]
    fn a_parakeet_partial_needs_1_0_s_new_audio() {
        let mut h = H::with(DictPrefs {
            model: "parakeet-v3".into(),
            ..prefs()
        });
        h.listen();
        h.speak(1.2);
        let id = partial_id(&h.tick()).expect("first partial");
        h.heard(id, "one");
        h.speak(0.9);
        assert!(partial_id(&h.tick()).is_none(), "0.9 s is not enough");
        h.speak(0.1);
        assert!(partial_id(&h.tick()).is_some(), "1.0 s is");
    }

    #[test]
    fn only_one_partial_is_in_flight() {
        let mut h = H::new();
        h.listen();
        h.speak(1.5);
        let id = partial_id(&h.tick()).unwrap();
        h.speak(2.0);
        assert!(h.tick().is_empty(), "the first has not answered");
        h.heard(id, "words");
        assert!(partial_id(&h.tick()).is_some(), "now the next may go");
    }

    #[test]
    fn a_partial_sends_the_newest_28_s_and_final_false() {
        let mut h = H::new();
        h.listen();
        h.speak(40.0);
        let fx = h.tick();
        let req = request(&fx).unwrap();
        assert!(!req.final_);
        assert_eq!(wav_samples(req), 28 * 16000);
        assert_eq!(req.model_id, "tiny");
        assert_eq!(req.language, "en");
    }

    #[test]
    fn a_cold_partial_shows_starting_speech_engine() {
        let mut h = H::new();
        h.listen();
        h.speak(1.5);
        let id = partial_id(&h.tick()).unwrap();
        h.fail(id, Reason::Warming);
        assert!(h.c.view().starting);
        h.speak(1.0);
        let id = partial_id(&h.tick()).unwrap();
        h.heard(id, "now it speaks");
        assert!(!h.c.view().starting);
        assert_eq!(h.c.view().live_text, "now it speaks");
    }

    #[test]
    fn a_partial_is_cleaned_shown_and_never_pasted() {
        let mut h = H::new();
        h.listen();
        h.speak(1.5);
        let id = partial_id(&h.tick()).unwrap();
        let fx = h.heard(id, "  hello   world ");
        assert_eq!(h.c.view().live_text, "hello world");
        assert!(pastes(&fx).is_empty());
        assert_eq!(h.c.view().phase, Phase::Listening);
    }

    #[test]
    fn a_partial_after_the_stop_is_not_shown() {
        let mut h = H::new();
        h.listen();
        h.speak(1.5);
        let id = partial_id(&h.tick()).unwrap();
        h.ev(Event::Key(KeyAction::Stop));
        let fx = h.heard(id, "older words");
        assert!(fx.is_empty());
        assert_eq!(h.c.view().live_text, "");
    }

    #[test]
    fn a_partial_error_changes_nothing() {
        let mut h = H::new();
        h.listen();
        h.speak(1.5);
        let id = partial_id(&h.tick()).unwrap();
        let fx = h.fail(id, Reason::EngineFailed);
        assert!(fx.is_empty());
        assert_eq!(h.c.view().phase, Phase::Listening);
    }

    // ---- the final ----

    #[test]
    fn a_clip_under_0_35_s_is_dropped_silently() {
        let mut h = H::new();
        h.listen();
        h.speak(0.3);
        let fx = h.ev(Event::Key(KeyAction::Stop));
        assert!(request(&fx).is_none());
        assert_eq!(h.c.view().phase, Phase::Idle);
        assert_eq!(message(&h), None);
        assert!(routes(&fx).is_empty());
        assert!(!h.c.recording());
    }

    #[test]
    fn a_clip_of_0_35_s_is_kept() {
        let mut h = H::new();
        h.listen();
        h.speak(0.35);
        let fx = h.ev(Event::Key(KeyAction::Stop));
        assert!(final_id(&fx).is_some());
    }

    #[test]
    fn a_final_is_the_whole_clip_with_the_language_for_the_model() {
        let mut h = H::new();
        h.listen();
        h.speak(40.0);
        let fx = h.ev(Event::Key(KeyAction::Stop));
        let req = request(&fx).unwrap();
        assert!(req.final_);
        assert_eq!(wav_samples(req), 40 * 16000, "the whole clip, not 28 s");
        assert_eq!(req.language, "en");
        let mut h = H::with(DictPrefs {
            model: "parakeet-v3".into(),
            ..prefs()
        });
        h.listen();
        h.speak(2.0);
        let fx = h.ev(Event::Key(KeyAction::Stop));
        assert_eq!(request(&fx).unwrap().language, "auto");
    }

    #[test]
    fn a_final_is_never_dropped() {
        let mut h = H::new();
        let id = h.dictate(2.0);
        // Everything that can happen meanwhile, short of a cancel.
        h.ev(Event::Key(KeyAction::Start));
        h.ev(Event::Key(KeyAction::Stop));
        h.ev(Event::Front(Some("tab-2".into())));
        h.ev(Event::Role(Role::None));
        h.ev(Event::PartialTick);
        h.set(|p| p.language = "no".into());
        h.at(10_000.0);
        let fx = h.heard(id, "still here");
        assert_eq!(pastes(&fx), vec![(TAB.into(), "still here ".into())]);
    }

    #[test]
    fn the_paste_ends_with_one_space_and_no_newline() {
        let mut h = H::new();
        let id = h.dictate(2.0);
        let fx = h.heard(id, "first line\nsecond line\r\n");
        let text = &pastes(&fx)[0].1;
        assert!(text.ends_with(' ') && !text.ends_with("  "));
        assert!(!text.contains('\n') && !text.contains('\r') && !text.contains('\u{2028}'));
    }

    #[test]
    fn empty_after_cleaning_says_heard_nothing() {
        let mut h = H::new();
        let id = h.dictate(2.0);
        let fx = h.heard(id, "[BLANK_AUDIO]");
        assert!(pastes(&fx).is_empty());
        assert_eq!(message(&h).as_deref(), Some("Heard nothing"));
    }

    #[test]
    fn engine_error_texts() {
        let table = [
            (Reason::NoModel, "Choose a model in Settings"),
            (
                Reason::NoEngine,
                "The speech engine is missing. Reinstall the app.",
            ),
            (
                Reason::EngineFailed,
                "The speech engine stopped. Try again.",
            ),
            (Reason::Superseded, "The speech engine stopped. Try again."),
        ];
        for (reason, text) in table {
            let mut h = H::new();
            let id = h.dictate(2.0);
            let fx = h.fail(id, reason);
            assert!(pastes(&fx).is_empty());
            assert_eq!(message(&h).as_deref(), Some(text), "{reason:?}");
            assert!(!h.c.recording());
        }
    }

    #[test]
    fn stop_is_heard_once() {
        let mut h = H::new();
        h.listen();
        h.speak(2.0);
        let first = h.ev(Event::Key(KeyAction::Stop));
        assert_eq!(count(&first, |e| matches!(e, Effect::Transcribe { .. })), 1);
        assert!(h.ev(Event::Key(KeyAction::Stop)).is_empty());
        assert!(h.ev(Event::Outside(OutsideAction::Stop)).is_empty());
    }

    #[test]
    fn cancel_is_heard_once() {
        let mut h = H::with(DictPrefs {
            sounds: true,
            ..prefs()
        });
        h.listen();
        h.speak(2.0);
        let first = h.ev(Event::Key(KeyAction::Cancel));
        assert_eq!(count(&first, |e| matches!(e, Effect::Cue(Cue::Cancel))), 1);
        assert!(h.ev(Event::Key(KeyAction::Cancel)).is_empty());
    }

    // ---- cues, media, crumbs ----

    #[test]
    fn cues_follow_the_sounds_setting() {
        let mut h = H::with(DictPrefs {
            sounds: true,
            ..prefs()
        });
        let fx = h.ev(Event::Key(KeyAction::Start));
        assert_eq!(count(&fx, |e| matches!(e, Effect::Cue(Cue::Start))), 1);
        h.ev(Event::MicOpened);
        h.speak(2.0);
        let fx = h.ev(Event::Key(KeyAction::Stop));
        assert_eq!(count(&fx, |e| matches!(e, Effect::Cue(Cue::Stop))), 1);
        let mut h = H::new();
        h.dictate(2.0);
        assert_eq!(count(&h.all, |e| matches!(e, Effect::Cue(_))), 0);
    }

    #[test]
    fn media_is_not_touched_when_the_setting_is_off() {
        let mut h = H::new();
        let id = h.dictate(2.0);
        h.heard(id, "hello");
        assert_eq!(count(&h.all, |e| matches!(e, Effect::PauseMedia)), 0);
        assert_eq!(resumes(&h.all), 0);
    }

    #[test]
    fn media_is_resumed_on_every_path() {
        type Path = (&'static str, fn(&mut H));
        let paths: [Path; 9] = [
            ("mic failed", |h| {
                h.ev(Event::Key(KeyAction::Start));
                h.ev(Event::MicFailed(CaptureFailure::NoDevice));
            }),
            ("cancelled while listening", |h| {
                h.listen();
                h.speak(2.0);
                h.ev(Event::Key(KeyAction::Cancel));
            }),
            ("cancelled before the mic opened", |h| {
                h.ev(Event::Key(KeyAction::Start));
                h.ev(Event::Key(KeyAction::Cancel));
                h.ev(Event::MicOpened);
            }),
            ("a brush of the key", |h| {
                h.listen();
                h.speak(0.2);
                h.ev(Event::Key(KeyAction::Stop));
            }),
            ("pasted", |h| {
                let id = h.dictate(2.0);
                h.heard(id, "hello");
            }),
            ("engine error", |h| {
                let id = h.dictate(2.0);
                h.fail(id, Reason::EngineFailed);
            }),
            ("heard nothing", |h| {
                let id = h.dictate(2.0);
                h.heard(id, "");
            }),
            ("cancelled while transcribing", |h| {
                h.dictate(2.0);
                h.ev(Event::Key(KeyAction::Cancel));
            }),
            ("switched off", |h| {
                h.listen();
                h.set(|p| p.enabled = false);
            }),
        ];
        for (name, run) in paths {
            let mut h = H::with(DictPrefs {
                pause_media: true,
                ..prefs()
            });
            run(&mut h);
            assert_eq!(
                count(&h.all, |e| matches!(e, Effect::PauseMedia)),
                1,
                "{name}"
            );
            assert_eq!(resumes(&h.all), 1, "{name}: resumed exactly once");
        }
    }

    #[test]
    fn media_is_resumed_when_the_app_quits_and_for_an_outside_recording() {
        let mut h = H::with(DictPrefs {
            pause_media: true,
            ..prefs()
        });
        h.listen();
        h.ev(Event::Quit);
        assert_eq!(resumes(&h.all), 1);
        let mut h = H::with(DictPrefs {
            pause_media: true,
            ..prefs()
        });
        h.listen_outside();
        h.ev(Event::Outside(OutsideAction::Cancel));
        assert_eq!(resumes(&h.all), 1);
    }

    #[test]
    fn crumbs_are_phase_names_only() {
        let mut h = H::new();
        let id = h.dictate(2.0);
        h.heard(id, "a secret sentence");
        let crumbs: Vec<&str> = h
            .all
            .iter()
            .filter_map(|e| match e {
                Effect::Crumb(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert_eq!(crumbs, vec!["listening", "transcribing", "idle"]);
    }

    // ---- warming (N10) ----

    #[test]
    fn warm_after_3000_ms_the_first_time_and_500_ms_on_re_syncs() {
        let mut c = Controller::new(prefs());
        let first = c.on(Event::Prefs(prefs()), 0.0);
        assert_eq!(warms(&first), vec![3000]);
        let again = c.on(Event::Prefs(prefs()), 10.0);
        assert_eq!(warms(&again), vec![500]);
    }

    #[test]
    fn warm_is_asked_again_at_each_press() {
        let mut h = H::new();
        let fx = h.ev(Event::Key(KeyAction::Start));
        assert_eq!(warms(&fx), vec![0]);
    }

    #[test]
    fn warm_is_asked_again_when_the_model_or_language_changes() {
        let mut h = H::new();
        assert_eq!(warms(&h.set(|p| p.model = "base".into())), vec![500]);
        assert_eq!(warms(&h.set(|p| p.language = "no".into())), vec![500]);
    }

    #[test]
    fn warm_never_with_no_model() {
        let mut c = Controller::new(prefs());
        let fx = c.on(
            Event::Prefs(DictPrefs {
                model: String::new(),
                ..prefs()
            }),
            0.0,
        );
        assert!(warms(&fx).is_empty());
        let fx = c.on(Event::Role(Role::Service), 0.0);
        assert!(warms(&fx).is_empty());
    }

    #[test]
    fn warm_never_while_dictation_is_off() {
        let mut c = Controller::new(prefs());
        let fx = c.on(
            Event::Prefs(DictPrefs {
                enabled: false,
                ..prefs()
            }),
            0.0,
        );
        assert!(warms(&fx).is_empty());
    }

    #[test]
    fn a_client_warms_nothing_and_stops_its_own() {
        let mut h = H::new();
        let fx = h.ev(Event::Role(Role::Client));
        assert_eq!(count(&fx, |e| matches!(e, Effect::StopEngine)), 1);
        assert!(!h.c.auto_warm());
        assert!(warms(&h.set(|p| p.model = "base".into())).is_empty());
    }

    #[test]
    fn a_service_with_dictation_on_keeps_the_engine_warm_at_once() {
        let mut h = H::new();
        let fx = h.ev(Event::Role(Role::Service));
        assert_eq!(warms(&fx), vec![0]);
        let mut h = H::new();
        h.set(|p| p.enabled = false);
        assert!(warms(&h.ev(Event::Role(Role::Service))).is_empty());
    }

    // ---- page keys (N9) ----

    #[test]
    fn with_page_keys_off_the_key_does_nothing() {
        for role in [Role::Service, Role::Client] {
            let mut h = H::new();
            h.ev(Event::Role(role));
            assert!(!h.c.page_keys());
            assert!(h.ev(Event::Key(KeyAction::Start)).is_empty(), "{role:?}");
            assert!(!h.c.recording());
        }
    }

    #[test]
    fn with_role_none_the_window_reads_the_key() {
        let mut h = H::new();
        h.ev(Event::Role(Role::Service));
        h.ev(Event::Role(Role::None));
        assert!(h.c.page_keys());
        assert!(!h.ev(Event::Key(KeyAction::Start)).is_empty());
    }

    // ---- outside recordings (N8) ----

    #[test]
    fn an_outside_recording_never_pastes_and_routes_the_text() {
        let mut h = H::new();
        h.listen_outside();
        h.speak(2.0);
        let id = final_id(&h.ev(Event::Outside(OutsideAction::Stop))).unwrap();
        let fx = h.heard(id, "send this");
        assert!(pastes(&fx).is_empty());
        assert_eq!(routes(&fx), vec![RouteResult::Text("send this".into())]);
        assert_eq!(h.c.view().phase, Phase::Idle);
    }

    #[test]
    fn an_outside_recording_needs_dictation_on_and_a_sink() {
        let mut h = H::new();
        h.ev(Event::Role(Role::Service));
        h.set(|p| p.enabled = false);
        assert!(h.ev(Event::Outside(OutsideAction::Start)).is_empty());
        let mut h = H::new();
        h.c.set_sink(false);
        assert!(h.ev(Event::Outside(OutsideAction::Start)).is_empty());
        assert!(!h.c.recording());
    }

    #[test]
    fn a_host_that_sets_no_sink_never_starts_an_outside_recording() {
        let p = prefs();
        let mut c = Controller::new(p.clone());
        c.on(Event::Prefs(p), 0.0);
        assert!(c.on(Event::Outside(OutsideAction::Start), 0.0).is_empty());
        assert!(!c.recording());
    }

    #[test]
    fn routed_words_paste_into_the_front_session_with_one_space_and_no_newline() {
        let mut h = H::new();
        let fx = h.ev(Event::PasteFront("routed words".into()));
        assert_eq!(pastes(&fx), vec![(TAB.into(), "routed words ".into())]);
        assert!(!pastes(&fx)[0].1.contains('\n'));
        h.ev(Event::Front(Some("tab-2".into())));
        assert_eq!(pastes(&h.ev(Event::PasteFront("x".into())))[0].0, "tab-2");
    }

    #[test]
    fn routed_words_with_no_shell_in_front_paste_nothing() {
        let mut h = H::new();
        h.ev(Event::Front(None));
        assert!(h.ev(Event::PasteFront("lost".into())).is_empty());
    }

    #[test]
    fn an_outside_start_with_no_model_tells_the_sink() {
        let mut h = H::with(DictPrefs {
            model: String::new(),
            ..prefs()
        });
        let fx = h.ev(Event::Outside(OutsideAction::Start));
        assert_eq!(
            routes(&fx),
            vec![RouteResult::Message(
                "Choose a speech model in Settings".into()
            )]
        );
        assert!(!h.c.recording());
    }

    #[test]
    fn outside_failures_and_notes_go_to_the_sink_as_messages() {
        // A microphone that fails.
        let mut h = H::new();
        h.ev(Event::Outside(OutsideAction::Start));
        let fx = h.ev(Event::MicFailed(CaptureFailure::Denied));
        assert_eq!(
            routes(&fx),
            vec![RouteResult::Message("Microphone access was refused".into())]
        );
        assert_eq!(h.c.view(), &View::idle());
        // An engine that fails.
        let mut h = H::new();
        h.listen_outside();
        h.speak(2.0);
        let id = final_id(&h.ev(Event::Outside(OutsideAction::Stop))).unwrap();
        let fx = h.fail(id, Reason::NoEngine);
        assert_eq!(
            routes(&fx),
            vec![RouteResult::Message(
                "The speech engine is missing. Reinstall the app.".into()
            )]
        );
        // Cancelled.
        let mut h = H::new();
        h.listen_outside();
        let fx = h.ev(Event::Outside(OutsideAction::Cancel));
        assert_eq!(routes(&fx), vec![RouteResult::Message("Cancelled".into())]);
        // Heard nothing, from an empty transcript and from a brush of the key.
        let mut h = H::new();
        h.listen_outside();
        h.speak(2.0);
        let id = final_id(&h.ev(Event::Outside(OutsideAction::Stop))).unwrap();
        let fx = h.heard(id, "[BLANK_AUDIO]");
        assert_eq!(
            routes(&fx),
            vec![RouteResult::Message("Heard nothing".into())]
        );
        let mut h = H::new();
        h.listen_outside();
        h.speak(0.1);
        let fx = h.ev(Event::Outside(OutsideAction::Stop));
        assert_eq!(
            routes(&fx),
            vec![RouteResult::Message("Heard nothing".into())]
        );
    }

    #[test]
    fn an_outside_recording_belongs_to_no_tab() {
        let mut h = H::new();
        h.ev(Event::Front(None));
        h.listen_outside();
        h.speak(2.0);
        h.ev(Event::TabGone(TAB.into()));
        let id = final_id(&h.ev(Event::Outside(OutsideAction::Stop))).unwrap();
        let fx = h.heard(id, "words");
        assert_eq!(routes(&fx).len(), 1);
        assert!(pastes(&fx).is_empty());
    }

    // ---- the pill ----

    #[test]
    fn the_message_face_lasts_2600_ms() {
        let mut h = H::with(DictPrefs {
            model: String::new(),
            ..prefs()
        });
        h.at(1000.0);
        h.ev(Event::Key(KeyAction::Start));
        assert!(h.c.needs_tick());
        h.at(3599.0);
        h.tick();
        assert!(message(&h).is_some(), "still there at 2599 ms");
        h.at(3600.0);
        h.tick();
        assert_eq!(message(&h), None);
        assert!(!h.c.needs_tick());
    }

    #[test]
    fn a_new_recording_clears_the_message() {
        let mut h = H::new();
        h.ev(Event::Key(KeyAction::Start));
        h.ev(Event::MicFailed(CaptureFailure::NoDevice));
        assert!(message(&h).is_some());
        h.ev(Event::Key(KeyAction::Start));
        assert_eq!(message(&h), None);
        assert_eq!(h.c.view().phase, Phase::Listening);
    }

    #[test]
    fn a_message_is_not_cleared_while_something_is_shown_over_it() {
        let mut h = H::new();
        h.ev(Event::Key(KeyAction::Start));
        h.ev(Event::MicFailed(CaptureFailure::NoDevice));
        h.ev(Event::Key(KeyAction::Start));
        h.at(5000.0);
        h.tick();
        assert_eq!(h.c.view().phase, Phase::Listening);
    }

    // ---- blur, quit ----

    #[test]
    fn blur_cancels_a_window_recording_that_is_still_listening() {
        let mut h = H::new();
        h.listen();
        h.speak(2.0);
        let fx = h.ev(Event::Blur);
        assert_eq!(count(&fx, |e| matches!(e, Effect::CloseMic)), 1);
        assert!(!h.c.recording());
    }

    #[test]
    fn blur_never_cancels_an_outside_recording_or_a_transcription() {
        let mut h = H::new();
        h.listen_outside();
        assert!(h.ev(Event::Blur).is_empty());
        assert!(h.c.recording());
        let mut h = H::new();
        h.dictate(2.0);
        assert!(h.ev(Event::Blur).is_empty());
        assert_eq!(h.c.view().phase, Phase::Transcribing);
    }

    #[test]
    fn quit_closes_everything_and_stops_the_engine() {
        let mut h = H::new();
        h.listen();
        let fx = h.ev(Event::Quit);
        assert_eq!(count(&fx, |e| matches!(e, Effect::CloseMic)), 1);
        assert_eq!(count(&fx, |e| matches!(e, Effect::StopEngine)), 1);
        assert!(pastes(&fx).is_empty());
        assert!(!h.c.recording());
        assert!(!h.c.armed());
        assert_eq!(h.c.view(), &View::idle());
    }

    #[test]
    fn an_answer_for_an_unknown_pass_is_ignored() {
        let mut h = H::new();
        h.dictate(2.0);
        assert!(h.heard(9999, "who asked").is_empty());
        assert_eq!(h.c.view().phase, Phase::Transcribing);
    }

    #[test]
    fn the_effects_of_a_full_dictation_come_in_order() {
        let mut h = H::with(DictPrefs {
            sounds: true,
            pause_media: true,
            ..prefs()
        });
        let id = h.dictate(2.0);
        h.heard(id, "ok");
        let names: Vec<&str> = h
            .all
            .iter()
            .filter_map(|e| match e {
                Effect::PauseMedia => Some("pause"),
                Effect::Cue(Cue::Start) => Some("cue-start"),
                Effect::OpenMic(_) => Some("open"),
                Effect::CloseMic => Some("close"),
                Effect::Cue(Cue::Stop) => Some("cue-stop"),
                Effect::Transcribe { .. } => Some("transcribe"),
                Effect::ResumeMedia => Some("resume"),
                Effect::Paste { .. } => Some("paste"),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "pause",
                "cue-start",
                "open",
                "close",
                "cue-stop",
                "transcribe",
                "resume",
                "paste"
            ]
        );
    }
}
