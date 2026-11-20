//! One device's input in the sim host (rules 1313 and 1314): a worker thread that holds the device's input
//! connection for the session and runs touches, keys, buttons, text and rotations strictly in order (so a pasted text
//! is in the field before the Return that follows it).
//!
//! - The path is picked once per boot ([`indigo::PathLock`]): DTUHID from CoreSimulator 1155.4, legacy Indigo below.
//! - Until the connection answers ("Connecting touch..."), input waits up to 20 s ([`InputQueue`]), then is dropped
//!   with a log line.
//! - On an interruption the contacts are forgotten (the gesture is lost), Flare hears `lost` (the agent's call fails
//!   with `retry`) and the connection is made once again; if that fails too, input is broken for the device.
//! - A sim host started again after a crash ends any finger the old one left down, at its first connection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::dtuhid::{self, Contacts, Dtuhid};
use super::guarded::Broken;
use super::indigo::Indigo;
use super::orient;
use super::sim::Device;
use super::simctl::Simctl;
use crate::client::{InputQueue, TextStep, text_steps};
use crate::geom::{Orientation, Size};
use crate::job::Fail;
use crate::source::{Button, InputState, Key, Touch};
use crate::target::Target;
use crate::versions::InputPath;

/// What the worker is asked to do, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Touch(Touch),
    Key(Key),
    /// A whole press (down, then up); shake and the rotations have their own ways.
    Button {
        id: u64,
        button: Button,
    },
    /// Text typed as keys or pasted through the simulator's pasteboard; answered once it is in.
    Text {
        id: u64,
        text: String,
    },
    /// Read the interface's orientation again (the screen's properties changed).
    ReadOrientation,
}

/// Where the worker's answers and news go (the sim host's stdout and log).
pub trait Out: Send + Sync {
    fn answer(&self, id: u64, r: Result<(), (Fail, String)>);
    fn input(&self, target: &Target, state: InputState);
    fn orientation(&self, target: &Target, o: Orientation);
    fn log(&self, line: &str);
}

/// What the worker needs to start.
pub struct Setup {
    pub target: Target,
    pub device: Arc<Device>,
    pub path: InputPath,
    /// The screen's portrait size in points.
    pub portrait: Size,
    /// The interface's orientation, shared with the frame stream.
    pub orientation: Arc<Mutex<Orientation>>,
    pub simctl: Simctl,
    /// This sim host replaced one that ended: end any finger it left down.
    pub restarted: bool,
    pub out: Arc<dyn Out>,
}

/// A device's input worker.
pub struct Input {
    tx: Mutex<Sender<(Instant, Cmd)>>,
    stop: Arc<AtomicBool>,
}

impl Input {
    /// Starts the worker; it connects on its own thread (up to about 36 s after a cold boot).
    pub fn start(s: Setup) -> Input {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let st = Arc::clone(&stop);
        let name = format!("input-{}", s.target.udid());
        let _ = std::thread::Builder::new()
            .name(name)
            .spawn(move || Worker::new(s, st).run(rx));
        Input {
            tx: Mutex::new(tx),
            stop,
        }
    }

    pub fn send(&self, c: Cmd) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send((Instant::now(), c));
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

enum Transport {
    Dtuhid(Dtuhid, Contacts),
    Indigo(Indigo),
}

struct Worker {
    s: Setup,
    stop: Arc<AtomicBool>,
    transport: Option<Transport>,
    /// The interruptions already handled.
    seen: u32,
    /// Where the device itself was turned to (the interface may stay portrait for a portrait-only app).
    device_o: Orientation,
    broken: Option<String>,
}

impl Worker {
    fn new(s: Setup, stop: Arc<AtomicBool>) -> Worker {
        let device_o = *s.orientation.lock().unwrap_or_else(|p| p.into_inner());
        Worker {
            s,
            stop,
            transport: None,
            seen: 0,
            device_o,
            broken: None,
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Sleeps `d`, or less if the worker is stopped.
    fn nap(stop: &AtomicBool, d: Duration) {
        let end = Instant::now() + d;
        while !stop.load(Ordering::Relaxed) {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(50))); // sync-ok: the input worker's own pacing
        }
    }

    /// Makes the connection (DTUHID with its liveness tries, or the legacy client).
    fn connect(&mut self) -> Result<Transport, Broken> {
        let dev = self.s.device.ptr();
        match self.s.path {
            InputPath::Dtuhid => {
                let out = Arc::clone(&self.s.out);
                let stop = Arc::clone(&self.stop);
                let started = Instant::now();
                let r = super::xpc::until_live(
                    |attempt| {
                        if stop.load(Ordering::Relaxed) {
                            return Err("stopped".into());
                        }
                        let t = Instant::now();
                        let conn = super::xpc::Conn::open(dev, dtuhid::DIGITIZER)
                            .map_err(|b| b.to_string())?;
                        let r = conn.request(&dtuhid::liveness(), super::xpc::LIVENESS_LIMIT);
                        out.log(&format!(
                            "input: liveness try {attempt}: {} in {} ms",
                            if r.is_ok() { "answered" } else { "no answer" },
                            t.elapsed().as_millis()
                        ));
                        r.map(|_| Dtuhid { conn }).map_err(|b| b.to_string())
                    },
                    |d| Worker::nap(&stop, d),
                );
                if r.is_ok() {
                    self.s.out.log(&format!(
                        "{}: input live in {} ms",
                        self.s.target,
                        started.elapsed().as_millis()
                    ));
                }
                r.map(|d| {
                    self.seen = d.interruptions();
                    Transport::Dtuhid(d, Contacts::default())
                })
            }
            InputPath::Indigo => {
                Indigo::open(&self.s.device, self.s.portrait).map(Transport::Indigo)
            }
        }
    }

    fn set_state(&self, state: InputState) {
        self.s.out.input(&self.s.target, state);
    }

    fn run(mut self, rx: Receiver<(Instant, Cmd)>) {
        self.set_state(InputState::Connecting);
        let mut queue: InputQueue<Cmd> = InputQueue::default();
        match self.connect() {
            Ok(t) => {
                self.transport = Some(t);
                if self.s.restarted {
                    self.end_all_fingers();
                }
                self.set_state(InputState::Live);
            }
            Err(b) => self.go_broken(b.to_string()),
        }
        // What came while connecting: in order, unless it waited too long.
        while let Ok((t, c)) = rx.try_recv() {
            queue.push(c, t);
        }
        let (go, dropped) = queue.release(Instant::now());
        self.dropped(dropped);
        for c in go {
            self.run_one(c);
        }
        loop {
            if self.stopped() {
                break;
            }
            // sync-ok: the input worker's own queue
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok((t, c)) => {
                    if t.elapsed() > crate::client::INPUT_WAIT {
                        self.dropped(vec![c]);
                    } else {
                        self.run_one(c);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.check_interrupted();
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // Detached: no finger is left down.
        self.release();
    }

    fn dropped(&self, cmds: Vec<Cmd>) {
        if cmds.is_empty() {
            return;
        }
        self.s.out.log(&format!(
            "{}: {} input events waited over 20 s for the connection and were dropped",
            self.s.target,
            cmds.len()
        ));
        for c in cmds {
            if let Cmd::Text { id, .. } | Cmd::Button { id, .. } = c {
                self.s.out.answer(
                    id,
                    Err((
                        Fail::Retry,
                        "the touch connection was not ready in 20 s".into(),
                    )),
                );
            }
        }
    }

    fn go_broken(&mut self, why: String) {
        self.s
            .out
            .log(&format!("{}: no input: {why}", self.s.target));
        self.transport = None;
        self.broken = Some(why.clone());
        self.set_state(InputState::Broken { why });
    }

    /// An interruption since the last look: the gesture is lost, Flare hears it, the connection is made again once.
    fn check_interrupted(&mut self) -> bool {
        let (now, invalid) = match &self.transport {
            Some(Transport::Dtuhid(d, _)) => (d.interruptions(), d.conn.is_invalid()),
            _ => return false,
        };
        // An invalid connection (the service went away for good) still runs its send barriers, so its sends look
        // fine: it is lost too, not left live while every touch goes nowhere.
        if invalid {
            self.lost("the connection became invalid");
            return true;
        }
        if now == self.seen {
            return false;
        }
        self.seen = now;
        self.lost("the connection was interrupted");
        true
    }

    fn lost(&mut self, why: &str) {
        self.s.out.log(&format!(
            "{}: input lost ({why}), connecting again",
            self.s.target
        ));
        self.transport = None;
        self.set_state(InputState::Lost);
        match self.connect() {
            Ok(t) => {
                self.transport = Some(t);
                self.set_state(InputState::Live);
            }
            Err(b) => self.go_broken(b.to_string()),
        }
    }

    fn unavailable(&self) -> (Fail, String) {
        match &self.broken {
            Some(w) => (Fail::Unavailable, format!("no touch input: {w}")),
            None => (Fail::Retry, "the touch connection dropped".into()),
        }
    }

    /// Sends one DTUHID message; a send that does not leave is a lost connection.
    fn send_dtuhid(&mut self, m: &super::xpc::XVal) -> bool {
        let ok = match &self.transport {
            Some(Transport::Dtuhid(d, _)) => d.send(m),
            _ => false,
        };
        if !ok && self.transport.is_some() {
            self.lost("a send did not leave");
        }
        ok
    }

    fn touch(&mut self, t: &Touch) -> bool {
        if self.check_interrupted() {
            return false;
        }
        let o = *self.s.orientation.lock().unwrap_or_else(|p| p.into_inner());
        match &mut self.transport {
            Some(Transport::Dtuhid(d, contacts)) => {
                let n = d.interruptions();
                match dtuhid::touch_message(contacts, t, self.s.portrait, o, n) {
                    Some(m) => self.send_dtuhid(&m),
                    None => true,
                }
            }
            Some(Transport::Indigo(i)) => i.touch(t, o),
            None => false,
        }
    }

    fn key(&mut self, k: Key) -> bool {
        match &self.transport {
            Some(Transport::Dtuhid(..)) => self.send_dtuhid(&dtuhid::key(k.usage, k.down)),
            Some(Transport::Indigo(i)) => i.key(k.usage, k.down),
            None => false,
        }
    }

    fn press(&mut self, b: Button) -> Result<(), (Fail, String)> {
        match b {
            Button::Shake => self
                .s
                .simctl
                .shake(&self.s.target)
                .map_err(|e| (Fail::Unavailable, e.message)),
            Button::RotateLeft | Button::RotateRight => self.rotate(b == Button::RotateLeft),
            _ => {
                let ok = match &self.transport {
                    Some(Transport::Dtuhid(..)) => {
                        let (page, code) = dtuhid::consumer_usage(b).unwrap_or((0, 0));
                        self.send_dtuhid(&dtuhid::button(page, code, true))
                            && self.send_dtuhid(&dtuhid::button(page, code, false))
                    }
                    Some(Transport::Indigo(i)) => i.button(b, true) && i.button(b, false),
                    None => false,
                };
                if ok { Ok(()) } else { Err(self.unavailable()) }
            }
        }
    }

    /// Turns the device a quarter and waits (at most 1.5 s) for the interface to follow.
    fn rotate(&mut self, left: bool) -> Result<(), (Fail, String)> {
        let to = orient::turned(self.device_o, left);
        let dev = self.s.device.ptr();
        orient::rotate(dev, to).map_err(|b| (Fail::Unavailable, b.to_string()))?;
        self.device_o = to;
        let before = *self.s.orientation.lock().unwrap_or_else(|p| p.into_inner());
        let udid = self.s.target.udid().to_string();
        match orient::poll_change(before, || orient::read(dev, &udid)) {
            Ok(now) => self.set_orientation(now),
            Err(b) => self
                .s
                .out
                .log(&format!("{}: orientation unread: {b}", self.s.target)),
        }
        Ok(())
    }

    fn set_orientation(&self, o: Orientation) {
        let changed = {
            let mut cur = self.s.orientation.lock().unwrap_or_else(|p| p.into_inner());
            let changed = *cur != o;
            *cur = o;
            changed
        };
        if changed {
            self.s
                .out
                .log(&format!("{}: interface {o:?}", self.s.target));
            self.s.out.orientation(&self.s.target, o);
        }
    }

    fn text(&mut self, text: &str) -> Result<(), (Fail, String)> {
        for step in text_steps(text) {
            match step {
                TextStep::Keys(keys) => {
                    for k in keys {
                        if !self.key(k) {
                            return Err(self.unavailable());
                        }
                    }
                }
                TextStep::Pasteboard(t) => {
                    self.s.simctl.pbcopy(&self.s.target, &t).map_err(|e| {
                        (
                            Fail::Unavailable,
                            format!("the simulator's pasteboard: {}", e.message),
                        )
                    })?
                }
            }
        }
        Ok(())
    }

    fn run_one(&mut self, c: Cmd) {
        if let Cmd::ReadOrientation = c {
            let udid = self.s.target.udid().to_string();
            match orient::read(self.s.device.ptr(), &udid) {
                Ok(o) => self.set_orientation(o),
                Err(b) => self
                    .s
                    .out
                    .log(&format!("{}: orientation unread: {b}", self.s.target)),
            }
            return;
        }
        // Shake and rotation need no input connection.
        if let Cmd::Button { id, button } = c
            && matches!(
                button,
                Button::Shake | Button::RotateLeft | Button::RotateRight
            )
        {
            let r = self.press(button);
            self.s.out.answer(id, r);
            return;
        }
        if self.transport.is_none() {
            if let Cmd::Text { id, .. } | Cmd::Button { id, .. } = c {
                self.s.out.answer(id, Err(self.unavailable()));
            }
            return;
        }
        match c {
            Cmd::Touch(t) => {
                self.touch(&t);
            }
            Cmd::Key(k) => {
                self.key(k);
            }
            Cmd::Button { id, button } => {
                let r = self.press(button);
                self.s.out.answer(id, r);
            }
            Cmd::Text { id, text } => {
                let r = self.text(&text);
                self.s.out.answer(id, r);
            }
            Cmd::ReadOrientation => {}
        }
    }

    /// The end of every contact this worker has down.
    fn release(&mut self) {
        match &mut self.transport {
            Some(Transport::Dtuhid(d, contacts)) => {
                for m in dtuhid::release_all(contacts) {
                    d.send(&m);
                }
            }
            Some(Transport::Indigo(i)) => i.release(),
            None => {}
        }
    }

    /// A sim host started after one that ended: whatever it left down is ended (one finger and a pinch).
    fn end_all_fingers(&mut self) {
        match &mut self.transport {
            Some(Transport::Dtuhid(d, _)) => {
                d.send(&dtuhid::digitizer(dtuhid::EventType::End, (0.5, 0.5), None));
                d.send(&dtuhid::digitizer(
                    dtuhid::EventType::End,
                    (0.5, 0.5),
                    Some((0.5, 0.5)),
                ));
            }
            Some(Transport::Indigo(_)) | None => {}
        }
        self.s.out.log(&format!(
            "{}: ended any finger the last sim host left down",
            self.s.target
        ));
    }
}
