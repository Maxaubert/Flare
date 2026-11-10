//! Arrows over the page (2026-11-10, rules 254 to 256): the agent points at what the person must do themselves, and
//! the person points at something to ask the agent about.
//!
//! An arrow is a window of Flare's own above the page (`flare_win::agent_cursor`), never in it: the page cannot read,
//! move or block it. It is drawn only over the page in front and live, exactly where the agent's cursor may be
//! ([`WebCtl::cursor_on`]), and never while a pop-up is open over the page.
//!
//! - **The agent's arrow** (`browser.point`): one orange arrow per tab with an optional short label. It is not a job:
//!   it scrolls its element into view, answers `{pointed, offscreen}` once it has measured it, and then measures again
//!   about every [`POINT_MEASURE_MS`] (`Page.getLayoutMetrics` and `DOM.getContentQuads`, its own DevTools route), so
//!   it follows the page as it scrolls and resizes. It goes on `clear`, a new arrow, the person clicking or typing in
//!   the page, a navigation, the panel or tab closing, or after [`POINT_MS`].
//! - **The person's arrow**: the bar's "Draw arrow" puts a pad over the page (the probe of 2026-11-10 showed a layered
//!   window there catches the mouse and leaves the keyboard with the page). The agent timer polls the pointer: a press
//!   on the page starts the arrow, a blue arrow follows the drag, the release asks the browser (no page script) what
//!   is under the head and hands the window a framed note to type, unsent. The button again, a right click, Escape
//!   where it arrives, a menu, a tab switch, a pop-up or a navigation cancel it. The arrow is kept for
//!   [`USER_ARROW_MS`] or until a navigation, and the agent's next look gets it, drawn on its screenshot. Its ends are
//!   kept on the document (the viewport point plus the scroll at the release), so it stays on what it points at: the
//!   scroll is measured again while its page is in front, it is drawn only while all of it shows, and a look gets its
//!   ends in the look's own viewport, `offscreen` and undrawn when its head is outside it.

use flare_surfaces::agent_job::Target;
use flare_surfaces::agent_web::{Pointed, click_point, element_rect, pointed_at, viewport_of};
use flare_win::agent_cursor::{
    ArrowInk, POINT_TAIL_PX, PadShell, RecordArrow, RecordMark, RecordPad, blend_onto_rgba,
    compose_arrow, point_tail,
};

use super::*;

type Reply = Sender<Result<Value, CtlError>>;

/// How long the agent's arrow stays when nothing ends it sooner.
pub const POINT_MS: u64 = 60_000;
/// How often the agent's arrow measures its element again (it follows a scroll a moment late).
pub const POINT_MEASURE_MS: u64 = 100;
/// How long the first measure may take before `browser.point` is answered `timeout`.
pub const POINT_ANSWER_MS: u64 = 10_000;
/// How long the person's arrow is kept for the agent's next look.
pub const USER_ARROW_MS: u64 = 5 * 60_000;
/// How long the browser has to say what is under the person's arrow.
const LOOKUP_MS: u64 = 5_000;

/// `browser.point`'s params: an element (`ref`) or a point, a label, or `clear`.
#[derive(Clone, Debug, PartialEq)]
pub struct PointReq {
    pub target: Option<Target>,
    pub label: String,
    pub clear: bool,
}

/// The mouse as the draw mode reads it, in client pixels of Flare's window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerNow {
    pub x: i32,
    pub y: i32,
    /// The left button is down now.
    pub left_down: bool,
    /// The left button went down since the last look (a click quicker than a tick).
    pub left_pressed: bool,
    /// The right button is down, or went down since the last look.
    pub right: bool,
    /// The pointer is over Flare's window or its pad, not another window in front.
    pub over_flare: bool,
}

/// Where the draw mode reads the mouse (tests hand a stand-in).
pub type PointerFn = Rc<dyn Fn(Option<isize>) -> Option<PointerNow>>;

/// What the agent's arrow points at.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PointTo {
    Node(i64),
    Css(f64, f64),
}

/// A DevTools answer slot: the call's id and, once in, its answer.
type Slot = (u64, Option<Result<Value, String>>);

/// The agent's arrow on a tab's page.
pub(super) struct AgentPoint {
    to: PointTo,
    label: String,
    doc: u64,
    until: u64,
    /// Where the head is now: x, y and the viewport's width and height, CSS pixels. None while it is not measured or
    /// its element does not show.
    head: Option<[f64; 4]>,
    /// The scroll into view still out (DOM.enable and the scroll): measuring starts when both are in.
    scrolling: Vec<u64>,
    /// The measure out: the layout and, for an element, its boxes.
    measure: Option<(Slot, Option<Slot>)>,
    next_ms: u64,
    /// The call that waits for the first measure, and when it gives up.
    reply: Option<(Reply, u64)>,
}

/// The person's arrow, kept for the agent's next look. Its ends are on the document, not the screen, so it stays with
/// what it points at as the page scrolls.
pub(super) struct UserArrow {
    /// Tail and head on the document: the viewport point plus the scroll at the release, CSS pixels.
    from: (f64, f64),
    to: (f64, f64),
    /// The scroll at the release, for a look that does not say its own.
    scroll: (f64, f64),
    /// The page as last measured while it is in front: scroll x, y and the viewport's width and height. None until
    /// it is measured again (it is not drawn on a stale scroll).
    seen: Option<[f64; 4]>,
    /// The measure out (`Page.getLayoutMetrics`) and when it was sent.
    measure: Option<(u64, u64)>,
    next_ms: u64,
    pointed: Option<Pointed>,
    doc: u64,
    until: u64,
}

impl UserArrow {
    /// The arrow in a viewport scrolled to `scroll`: tail and head in its CSS pixels.
    fn in_view(&self, scroll: (f64, f64)) -> ((f64, f64), (f64, f64)) {
        (
            (self.from.0 - scroll.0, self.from.1 - scroll.1),
            (self.to.0 - scroll.0, self.to.1 - scroll.1),
        )
    }
}

/// Whether a CSS point is inside a viewport of `vp`.
fn shows(p: (f64, f64), vp: (f64, f64)) -> bool {
    p.0 >= 0.0 && p.1 >= 0.0 && p.0 < vp.0 && p.1 < vp.1
}

/// The draw mode: on one tab's page, before and during the drag.
struct Draw {
    tab: String,
    doc: u64,
    /// The left button has been seen up since the mode began (the click on the button itself is not a press on the
    /// page).
    armed: bool,
    from: Option<(i32, i32)>,
    to: (i32, i32),
}

enum LookupStage {
    Metrics { waiting: Vec<u64>, layout: Slot },
    Node { id: u64, at: Released },
    Ax { id: u64, at: Released, backend: i64 },
}

/// Where the person let go, in the viewport of the release: its size, the tail, the head and the scroll, CSS pixels.
#[derive(Clone, Copy)]
struct Released {
    vp: (f64, f64),
    from: (f64, f64),
    to: (f64, f64),
    scroll: (f64, f64),
}

/// The person's arrow, let go: the browser is asked what is under its head, step by step.
struct Lookup {
    tab: String,
    doc: u64,
    from_px: (i32, i32),
    to_px: (i32, i32),
    rect: PhysRect,
    started: u64,
    stage: LookupStage,
}

/// What the agent's arrow's picture is drawn for: which way its tail goes (signs), the scale in hundredths, the label.
type ArtKey = (i32, i32, u32, String);

/// The arrows' windows and the draw mode (one per window, like the cursor).
pub(super) struct Arrows {
    agent_shell: Box<dyn CursorShell>,
    mark_shell: Box<dyn CursorShell>,
    pad_shell: Box<dyn PadShell>,
    pointer: PointerFn,
    /// What each window last showed: the head on the screen and the picture's key, so a still frame is not drawn again.
    agent_last: Option<(i32, i32, ArtKey)>,
    agent_art: Option<(ArtKey, CursorBitmap)>,
    mark_last: Option<(i32, i32, i32, i32, u32)>,
    pad_last: Option<(i32, i32, u32, u32)>,
    draw: Option<Draw>,
    lookup: Option<Lookup>,
}

impl Default for Arrows {
    fn default() -> Self {
        Self {
            agent_shell: Box::new(RecordArrow),
            mark_shell: Box::new(RecordMark),
            pad_shell: Box::new(RecordPad),
            pointer: Rc::new(real_pointer),
            agent_last: None,
            agent_art: None,
            mark_last: None,
            pad_last: None,
            draw: None,
            lookup: None,
        }
    }
}

impl Arrows {
    /// The person is drawing an arrow on some page.
    pub(super) fn drawing_any(&self) -> bool {
        self.draw.is_some()
    }
}

/// macOS: phase 5 of the port plan (the person's input through NSEvent). No pointer is read until then.
#[cfg(not(windows))]
fn real_pointer(_hwnd: Option<isize>) -> Option<PointerNow> {
    None
}

/// The real mouse: where it is in Flare's client pixels, and its buttons. The "pressed since" bits are read here too.
#[cfg(windows)]
fn real_pointer(hwnd: Option<isize>) -> Option<PointerNow> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GW_OWNER, GetAncestor, GetCursorPos, GetWindow, WindowFromPoint,
    };
    let hwnd = hwnd?;
    // SAFETY: plain reads of the pointer, the buttons and the window under the pointer; `p` is a valid out value and
    // `hwnd` is Flare's own window.
    unsafe {
        let left = GetAsyncKeyState(i32::from(VK_LBUTTON)) as u16;
        let right = GetAsyncKeyState(i32::from(VK_RBUTTON)) as u16;
        let mut p = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut p) == 0 {
            return None;
        }
        let root = GetAncestor(WindowFromPoint(p), GA_ROOT);
        // Flare's window itself, or a window it owns (the pad).
        let over_flare = root == hwnd as *mut _ || GetWindow(root, GW_OWNER) == hwnd as *mut _;
        if ScreenToClient(hwnd as *mut _, &mut p) == 0 {
            return None;
        }
        Some(PointerNow {
            x: p.x,
            y: p.y,
            left_down: left & 0x8000 != 0,
            left_pressed: left & 0x0001 != 0,
            right: right & 0x8001 != 0,
            over_flare,
        })
    }
}

/// Rounds to a tenth, as the look's numbers are.
fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// A point of the page's rectangle (client pixels) as CSS pixels of a viewport of `vp`, kept inside it.
fn to_css(p: (i32, i32), rect: &PhysRect, vp: (f64, f64)) -> (f64, f64) {
    let sx = vp.0 / f64::from(rect.w.max(1));
    let sy = vp.1 / f64::from(rect.h.max(1));
    (
        (f64::from(p.0 - rect.x) * sx).clamp(0.0, (vp.0 - 1.0).max(0.0)),
        (f64::from(p.1 - rect.y) * sy).clamp(0.0, (vp.1 - 1.0).max(0.0)),
    )
}

/// A CSS point of a viewport of `vp` in client pixels of the page's rectangle (the cursor's mapping).
fn to_client(p: (f64, f64), rect: &PhysRect, vp: (f64, f64)) -> (i32, i32) {
    (
        rect.x + (p.0 * f64::from(rect.w) / vp.0.max(1.0)).round() as i32,
        rect.y + (p.1 * f64::from(rect.h) / vp.1.max(1.0)).round() as i32,
    )
}

/// Keeps a point inside a rectangle.
fn clamp_in(p: (i32, i32), r: &PhysRect) -> (i32, i32) {
    (
        p.0.clamp(r.x, (r.right() - 1).max(r.x)),
        p.1.clamp(r.y, (r.bottom() - 1).max(r.y)),
    )
}

/// The look's answer gets the person's arrow: `user_arrow` `{from, to, ref, role, name, offscreen?}`, a ref made for
/// the element when the look did not list it, and the arrow drawn on the screenshot. Its ends are where they are in
/// the look's own viewport (the page may have scrolled since the release); when its head is outside that viewport it
/// is `offscreen` and not drawn.
fn add_user_arrow(v: &mut Value, u: &UserArrow, refs: &mut RefMap) {
    let num = |k: &str| {
        v.get("viewport")
            .and_then(|p| p.get(k))
            .and_then(Value::as_f64)
    };
    let look_scroll = v
        .get("viewport")
        .and_then(|p| p.get("scroll"))
        .and_then(Value::as_array)
        .and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)));
    let scroll = look_scroll
        .or_else(|| u.seen.map(|s| (s[0], s[1])))
        .unwrap_or(u.scroll);
    let (from, to) = u.in_view(scroll);
    let offscreen = match (num("width"), num("height")) {
        (Some(w), Some(h)) => !shows(to, (w, h)),
        _ => false,
    };
    let r = u.pointed.as_ref().map(|p| refs.ref_for(p.backend_id));
    v["user_arrow"] = json!({
        "from": [r1(from.0), r1(from.1)],
        "to": [r1(to.0), r1(to.1)],
        "ref": r,
        "role": u.pointed.as_ref().map_or("", |p| p.role.as_str()),
        "name": u.pointed.as_ref().map_or("", |p| p.name.as_str()),
    });
    if offscreen {
        v["user_arrow"]["offscreen"] = json!(true);
    } else if let Some(img) = v.get_mut("image")
        && let Some(drawn) = arrow_on_shot(img, from, to)
    {
        img["data"] = json!(drawn);
    }
}

/// The screenshot of a look (base64 PNG) with the person's arrow drawn on it at the look's scale (tail and head in
/// the look's viewport, CSS pixels), or None when the picture could not be read.
fn arrow_on_shot(img: &Value, from: (f64, f64), to: (f64, f64)) -> Option<String> {
    use base64::Engine;
    let data = img.get("data")?.as_str()?;
    let scale = img
        .get("scale")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s > 0.0)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let mut rgba = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let (w, h) = rgba.dimensions();
    let tail = ((from.0 - to.0) * scale, (from.1 - to.1) * scale);
    let bmp = compose_arrow(tail, scale, ArrowInk::Person, "");
    let head = ((to.0 * scale).round() as i32, (to.1 * scale).round() as i32);
    blend_onto_rgba(&mut rgba, w, h, head, &bmp);
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(out))
}

impl WebCtl {
    /// What draws the arrows: the agent's, the person's and the pad (the real windows once Flare's has a handle).
    pub fn set_arrow_shells(
        &mut self,
        agent: Box<dyn CursorShell>,
        mark: Box<dyn CursorShell>,
        pad: Box<dyn PadShell>,
    ) {
        let a = &mut self.arrows;
        a.agent_shell.hide();
        a.mark_shell.hide();
        a.pad_shell.hide();
        a.agent_shell = agent;
        a.mark_shell = mark;
        a.pad_shell = pad;
        a.agent_last = None;
        a.mark_last = None;
        a.pad_last = None;
        self.update_arrows();
    }

    /// Where the draw mode reads the mouse (tests).
    #[cfg(test)]
    pub fn set_pointer(&mut self, f: PointerFn) {
        self.arrows.pointer = f;
    }

    fn next_id(&mut self) -> u64 {
        self.call_ids += 1;
        self.call_ids
    }

    fn send_cdp(&mut self, tab: &str, id: u64, method: &str, params: Value) {
        if let Some(b) = self.backend.as_mut() {
            b.cdp(tab, id, method, &params.to_string());
        }
    }

    // ---- the agent's arrow ------------------------------------------------------------------------------------

    /// `browser.point`: an arrow at an element or a point of the tab's page, answered once it is measured; `clear`
    /// takes it away. It never clicks and cannot type.
    pub fn start_point(&mut self, tab: &str, req: PointReq, reply: Reply) {
        let say = |c: &'static str, m: &str| {
            let _ = reply.send(Err(CtlError::new(c, m.to_string())));
        };
        let now = self.now_ms();
        let Some(t) = self.tabs.get_mut(tab) else {
            say(
                code::UNAVAILABLE,
                "the page is not ready yet; try again in a moment",
            );
            return;
        };
        if let Some((why, missing)) = &t.failed {
            say(
                code::UNAVAILABLE,
                &if *missing {
                    format!("the WebView2 Runtime is not installed ({RUNTIME_URL})")
                } else {
                    format!("the web view could not be made: {why}")
                },
            );
            return;
        }
        if !t.ready {
            say(
                code::UNAVAILABLE,
                "the page is not ready yet; try again in a moment",
            );
            return;
        }
        // The person finishes a pop-up first (rule 250): the page behind it is not pointed at.
        if !t.popups.is_empty() {
            say(code::UNAVAILABLE, POPUP_OPEN);
            return;
        }
        if req.clear {
            let had = t.point.take().is_some_and(|p| {
                if let Some((r, _)) = p.reply {
                    let _ = r.send(Ok(json!({"pointed": false, "cleared": true})));
                }
                true
            });
            let _ = reply.send(Ok(json!({"cleared": had})));
            self.update_arrows();
            return;
        }
        let to = match req.target {
            Some(Target::Ref(r)) => match t.refs.get(&r) {
                Some(b) => PointTo::Node(b),
                None => {
                    say(
                        code::NOT_FOUND,
                        &format!(
                            "there is no element {r}; refs last until the next look or page load, so look again"
                        ),
                    );
                    return;
                }
            },
            Some(Target::Point(x, y)) if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 => {
                PointTo::Css(x, y)
            }
            Some(Target::Point(..)) => {
                say(
                    code::BAD_PARAMS,
                    "x and y are page pixels from the top left, zero or more",
                );
                return;
            }
            None => {
                say(code::BAD_PARAMS, "give ref, or both x and y, or clear");
                return;
            }
        };
        // A new arrow replaces the last; a call still waiting for the old one is told.
        if let Some(old) = t.point.take()
            && let Some((r, _)) = old.reply
        {
            let _ = r.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "a newer arrow replaced this one",
            )));
        }
        let doc = t.doc;
        let label = crate::ask_agent::point_label(&req.label);
        let scroll = match to {
            PointTo::Node(b) => {
                let (enable, into) = (self.next_id(), self.next_id());
                self.send_cdp(tab, enable, "DOM.enable", json!({}));
                self.send_cdp(
                    tab,
                    into,
                    "DOM.scrollIntoViewIfNeeded",
                    json!({"backendNodeId": b}),
                );
                vec![enable, into]
            }
            PointTo::Css(..) => Vec::new(),
        };
        let measure_now = scroll.is_empty();
        if let Some(t) = self.tabs.get_mut(tab) {
            t.point = Some(AgentPoint {
                to,
                label,
                doc,
                until: now + POINT_MS,
                head: None,
                scrolling: scroll,
                measure: None,
                next_ms: now,
                reply: Some((reply, now + POINT_ANSWER_MS)),
            });
        }
        // A press from before the arrow is not the person's answer to it.
        flush_button_edges();
        self.ensure_agent_timer();
        if measure_now {
            self.point_measure(tab);
        }
    }

    /// Asks where the arrow's target is now.
    fn point_measure(&mut self, tab: &str) {
        let Some(to) = self
            .tabs
            .get(tab)
            .and_then(|t| t.point.as_ref())
            .filter(|p| p.measure.is_none() && p.scrolling.is_empty())
            .map(|p| p.to)
        else {
            return;
        };
        let layout = self.next_id();
        self.send_cdp(tab, layout, "Page.getLayoutMetrics", json!({}));
        let quads = match to {
            PointTo::Node(b) => {
                let id = self.next_id();
                self.send_cdp(tab, id, "DOM.getContentQuads", json!({"backendNodeId": b}));
                Some((id, None))
            }
            PointTo::Css(..) => None,
        };
        if let Some(p) = self.tabs.get_mut(tab).and_then(|t| t.point.as_mut()) {
            p.measure = Some(((layout, None), quads));
        }
    }

    /// A DevTools answer for the tab's arrow. False when it is not one of the arrow's.
    fn point_answer(&mut self, tab: &str, id: u64, result: &Result<String, String>) -> bool {
        let now = self.now_ms();
        let Some(p) = self.tabs.get_mut(tab).and_then(|t| t.point.as_mut()) else {
            return false;
        };
        if let Some(i) = p.scrolling.iter().position(|x| *x == id) {
            // Whether the scroll worked does not matter (a text node cannot be scrolled to): the measure says.
            p.scrolling.remove(i);
            if p.scrolling.is_empty() {
                p.next_ms = now;
                self.point_measure(tab);
            }
            return true;
        }
        let parsed = || match result {
            Ok(s) => serde_json::from_str::<Value>(s).map_err(|e| e.to_string()),
            Err(e) => Err(e.clone()),
        };
        let Some((layout, quads)) = p.measure.as_mut() else {
            return false;
        };
        if layout.0 == id {
            layout.1 = Some(parsed());
        } else if let Some(q) = quads.as_mut().filter(|q| q.0 == id) {
            q.1 = Some(parsed());
        } else {
            return false;
        }
        if layout.1.is_none() || quads.as_ref().is_some_and(|q| q.1.is_none()) {
            return true;
        }
        let Some((layout, quads)) = p.measure.take() else {
            return true;
        };
        p.next_ms = now + POINT_MEASURE_MS;
        let vp = layout.1.and_then(Result::ok).and_then(|v| viewport_of(&v));
        let mut gone = false;
        p.head = vp.and_then(|vp| match p.to {
            PointTo::Css(x, y) => (x < vp.w && y < vp.h).then_some([x, y, vp.w, vp.h]),
            PointTo::Node(_) => {
                let q = quads.and_then(|q| q.1).and_then(Result::ok);
                gone = q.as_ref().and_then(element_rect).is_none();
                q.and_then(|q| click_point(&q, &vp))
                    .map(|(x, y)| [x, y, vp.w, vp.h])
            }
        });
        let first = p.reply.take();
        if let Some((r, _)) = first {
            if gone {
                let _ = r.send(Err(CtlError::new(
                    code::NOT_FOUND,
                    "that element is gone or has no place on the page; look again",
                )));
                if let Some(t) = self.tabs.get_mut(tab) {
                    t.point = None;
                }
            } else if vp.is_none() {
                let _ = r.send(Err(CtlError::new(
                    code::UNAVAILABLE,
                    "the page's size could not be read",
                )));
                if let Some(t) = self.tabs.get_mut(tab) {
                    t.point = None;
                }
            } else {
                let offscreen = p.head.is_none();
                let _ = r.send(Ok(json!({"pointed": true, "offscreen": offscreen})));
            }
        }
        self.update_arrows();
        true
    }

    /// The tab's arrow goes (the person acted, the page moved on, the panel closed): a call still waiting is told.
    fn drop_point(&mut self, tab: &str, why: &str) {
        if let Some(p) = self.tabs.get_mut(tab).and_then(|t| t.point.take())
            && let Some((r, _)) = p.reply
        {
            let _ = r.send(Err(CtlError::new(code::UNAVAILABLE, why.to_string())));
        }
    }

    /// The tab's arrow, while one is up (the tests): its head and its label.
    #[cfg(test)]
    pub fn point_of(&self, tab: &str) -> Option<(Option<[f64; 4]>, String)> {
        self.tabs
            .get(tab)?
            .point
            .as_ref()
            .map(|p| (p.head, p.label.clone()))
    }

    // ---- the person's arrow -----------------------------------------------------------------------------------

    /// Whether the person may draw on the tab's page now: in front and live, no pop-up over it, loaded.
    fn draw_possible(&self, tab: &str) -> bool {
        self.cursor_on(tab)
            && self
                .tabs
                .get(tab)
                .is_some_and(|t| t.ready && t.failed.is_none() && t.popups.is_empty())
    }

    /// "Draw arrow": starts the draw mode on the tab's page, or ends it when it is on. False when the page cannot be
    /// drawn on now.
    pub fn toggle_draw(&mut self, tab: &str) -> bool {
        if self.arrows.draw.as_ref().is_some_and(|d| d.tab == tab) {
            self.cancel_draw();
            return true;
        }
        if !self.draw_possible(tab) {
            return false;
        }
        let Some(t) = self.tabs.get_mut(tab) else {
            return false;
        };
        // A new arrow replaces the person's last.
        t.user_arrow = None;
        let doc = t.doc;
        if self.arrows.lookup.as_ref().is_some_and(|l| l.tab == tab) {
            self.arrows.lookup = None;
        }
        self.arrows.draw = Some(Draw {
            tab: tab.to_string(),
            doc,
            armed: false,
            from: None,
            to: (0, 0),
        });
        self.keys.drawing.set(true);
        flush_button_edges();
        self.ensure_agent_timer();
        self.update_arrows();
        true
    }

    /// The draw mode ends with nothing drawn.
    pub fn cancel_draw(&mut self) {
        if self.arrows.draw.take().is_some() {
            self.keys.drawing.set(false);
            self.update_arrows();
        }
    }

    /// Whether the draw mode is on for the tab's page (the button shows pressed).
    pub fn drawing(&self, tab: &str) -> bool {
        self.arrows.draw.as_ref().is_some_and(|d| d.tab == tab)
    }

    /// One turn of the draw mode: the pointer's press, drag and release over the page in front.
    fn draw_tick(&mut self) {
        let Some((tab, doc)) = self.arrows.draw.as_ref().map(|d| (d.tab.clone(), d.doc)) else {
            return;
        };
        let ok = self.draw_possible(&tab) && self.tabs.get(&tab).is_some_and(|t| t.doc == doc);
        let rect = match &self.bounds {
            Some((t, r)) if *t == tab => Some(*r),
            _ => None,
        };
        let Some(rect) = rect.filter(|_| ok) else {
            self.cancel_draw();
            return;
        };
        let Some(p) = (self.arrows.pointer)(self.hwnd) else {
            return;
        };
        if p.right {
            self.cancel_draw();
            return;
        }
        let Some(d) = self.arrows.draw.as_mut() else {
            return;
        };
        if !d.armed {
            if !p.left_down {
                d.armed = true;
            }
            return;
        }
        let at = (p.x, p.y);
        match d.from {
            None => {
                if (p.left_down || p.left_pressed) && p.over_flare && point_in(&rect, p.x, p.y) {
                    d.from = Some(at);
                    d.to = at;
                    if !p.left_down {
                        // Pressed and let go between two looks: a click on the target.
                        self.draw_release(rect);
                        return;
                    }
                }
            }
            Some(_) if p.left_down => d.to = clamp_in(at, &rect),
            Some(_) => {
                d.to = clamp_in(at, &rect);
                self.draw_release(rect);
                return;
            }
        }
        self.update_arrows();
    }

    /// The person let go: the browser is asked what is under the head. A click with no drag points from up and left.
    fn draw_release(&mut self, rect: PhysRect) {
        let Some(d) = self.arrows.draw.take() else {
            return;
        };
        self.keys.drawing.set(false);
        let Some(from) = d.from else {
            self.update_arrows();
            return;
        };
        let to = d.to;
        let from = if (from.0 - to.0).abs() < 4 && (from.1 - to.1).abs() < 4 {
            let off = (POINT_TAIL_PX * f64::from(self.scale.clamp(0.5, 4.0))
                / std::f64::consts::SQRT_2)
                .round() as i32;
            (to.0 - off, to.1 - off)
        } else {
            from
        };
        let (enable, layout) = (self.next_id(), self.next_id());
        self.send_cdp(&d.tab, enable, "DOM.enable", json!({}));
        self.send_cdp(&d.tab, layout, "Page.getLayoutMetrics", json!({}));
        let now = self.now_ms();
        self.arrows.lookup = Some(Lookup {
            tab: d.tab,
            doc: d.doc,
            from_px: from,
            to_px: to,
            rect,
            started: now,
            stage: LookupStage::Metrics {
                waiting: vec![enable, layout],
                layout: (layout, None),
            },
        });
        self.update_arrows();
    }

    /// A DevTools answer for the person's arrow. None when it is not one of its.
    fn lookup_answer(
        &mut self,
        tab: &str,
        id: u64,
        result: &Result<String, String>,
    ) -> Option<Vec<WebOut>> {
        let l = self.arrows.lookup.as_mut().filter(|l| l.tab == tab)?;
        let parsed = match result {
            Ok(s) => serde_json::from_str::<Value>(s).map_err(|e| e.to_string()),
            Err(e) => Err(e.clone()),
        };
        match &mut l.stage {
            LookupStage::Metrics { waiting, layout } => {
                let i = waiting.iter().position(|x| *x == id)?;
                waiting.remove(i);
                if layout.0 == id {
                    layout.1 = Some(parsed);
                }
                if !waiting.is_empty() {
                    return Some(Vec::new());
                }
                let metrics = layout
                    .1
                    .take()
                    .and_then(Result::ok)
                    .and_then(|v| viewport_of(&v));
                let Some(metrics) = metrics else {
                    // No size, no place on the page: the arrow is dropped with nothing typed.
                    self.arrows.lookup = None;
                    self.update_arrows();
                    return Some(Vec::new());
                };
                let vp = (metrics.w, metrics.h);
                let from = to_css(l.from_px, &l.rect, vp);
                let to = to_css(l.to_px, &l.rect, vp);
                let at = Released {
                    vp,
                    from,
                    to,
                    scroll: (metrics.page_x, metrics.page_y),
                };
                let node = self.next_id();
                self.send_cdp(
                    tab,
                    node,
                    "DOM.getNodeForLocation",
                    json!({
                        "x": to.0.floor() as i64,
                        "y": to.1.floor() as i64,
                        "includeUserAgentShadowDOM": false,
                        "ignorePointerEventsNone": true,
                    }),
                );
                if let Some(l) = self.arrows.lookup.as_mut() {
                    l.stage = LookupStage::Node { id: node, at };
                }
                Some(Vec::new())
            }
            LookupStage::Node { id: want, at } => {
                if *want != id {
                    return None;
                }
                let at = *at;
                let backend = parsed
                    .ok()
                    .and_then(|v| v.get("backendNodeId").and_then(Value::as_i64));
                let Some(backend) = backend else {
                    return Some(self.lookup_done(at, None));
                };
                let ax = self.next_id();
                self.send_cdp(
                    tab,
                    ax,
                    "Accessibility.getPartialAXTree",
                    json!({"backendNodeId": backend, "fetchRelatives": true}),
                );
                if let Some(l) = self.arrows.lookup.as_mut() {
                    l.stage = LookupStage::Ax {
                        id: ax,
                        at,
                        backend,
                    };
                }
                Some(Vec::new())
            }
            LookupStage::Ax {
                id: want,
                at,
                backend,
            } => {
                if *want != id {
                    return None;
                }
                let (at, backend) = (*at, *backend);
                let pointed = parsed
                    .ok()
                    .and_then(|v| pointed_at(&v, backend))
                    .or(Some(Pointed {
                        backend_id: backend,
                        role: "element".to_string(),
                        name: String::new(),
                    }));
                Some(self.lookup_done(at, pointed))
            }
        }
    }

    /// The person's arrow is known: it is kept for the agent's next look, and the window types its frame.
    fn lookup_done(&mut self, at: Released, pointed: Option<Pointed>) -> Vec<WebOut> {
        let Released {
            vp,
            from,
            to,
            scroll,
        } = at;
        let now = self.now_ms();
        let Some(l) = self.arrows.lookup.take() else {
            return Vec::new();
        };
        let Some(t) = self.tabs.get_mut(&l.tab).filter(|t| t.doc == l.doc) else {
            self.update_arrows();
            return Vec::new();
        };
        let site = if let Some(file) = &t.source {
            file.rsplit(['\\', '/']).next().unwrap_or(file).to_string()
        } else if t.app_host.is_none() {
            host_label(&t.url)
        } else {
            "this page".to_string()
        };
        let text = crate::ask_agent::frame_arrow(
            &site,
            pointed.as_ref().map(|p| (p.role.as_str(), p.name.as_str())),
            to,
            vp,
        );
        t.user_arrow = Some(UserArrow {
            from: (from.0 + scroll.0, from.1 + scroll.1),
            to: (to.0 + scroll.0, to.1 + scroll.1),
            scroll,
            seen: Some([scroll.0, scroll.1, vp.0, vp.1]),
            measure: None,
            next_ms: now + POINT_MEASURE_MS,
            pointed,
            doc: l.doc,
            until: now + USER_ARROW_MS,
        });
        self.update_arrows();
        vec![WebOut::ArrowDrawn { tab: l.tab, text }]
    }

    /// Asks where the page is scrolled now, for the person's arrow kept on it.
    fn user_arrow_measure(&mut self, tab: &str, now: u64) {
        let id = self.next_id();
        self.send_cdp(tab, id, "Page.getLayoutMetrics", json!({}));
        if let Some(u) = self.tabs.get_mut(tab).and_then(|t| t.user_arrow.as_mut()) {
            u.measure = Some((id, now));
        }
    }

    /// A DevTools answer for the person's kept arrow (where the page is scrolled). False when it is not one of its.
    fn user_arrow_answer(&mut self, tab: &str, id: u64, result: &Result<String, String>) -> bool {
        let now = self.now_ms();
        let Some(u) = self
            .tabs
            .get_mut(tab)
            .and_then(|t| t.user_arrow.as_mut())
            .filter(|u| u.measure.is_some_and(|(m, _)| m == id))
        else {
            return false;
        };
        u.measure = None;
        u.next_ms = now + POINT_MEASURE_MS;
        u.seen = result
            .as_ref()
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| viewport_of(&v))
            .map(|v| [v.page_x, v.page_y, v.w, v.h]);
        self.update_arrows();
        true
    }

    /// The person's arrow on the tab's page, while it is kept (the tests): from and to on the document, CSS pixels.
    #[cfg(test)]
    pub fn user_arrow_of(&self, tab: &str) -> Option<((f64, f64), (f64, f64))> {
        let now = self.now_ms();
        self.tabs
            .get(tab)?
            .user_arrow
            .as_ref()
            .filter(|u| now < u.until)
            .map(|u| (u.from, u.to))
    }

    /// A DevTools answer that is an arrow's: the agent's measure or the person's lookup. None when it is neither.
    pub(super) fn arrow_cdp(
        &mut self,
        tab: &str,
        id: u64,
        result: &Result<String, String>,
    ) -> Option<Vec<WebOut>> {
        if let Some(out) = self.lookup_answer(tab, id, result) {
            return Some(out);
        }
        if self.user_arrow_answer(tab, id, result) {
            return Some(Vec::new());
        }
        self.point_answer(tab, id, result).then(Vec::new)
    }

    /// A finished look gets the person's arrow, once, while it is kept.
    pub(super) fn look_gets_arrow(&mut self, tab: &str, v: &mut Value) {
        let now = self.now_ms();
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        let doc = t.doc;
        if let Some(u) = t.user_arrow.take_if(|u| u.doc == doc && now < u.until) {
            add_user_arrow(v, &u, &mut t.refs);
        }
    }

    // ---- both ---------------------------------------------------------------------------------------------------

    /// The page of `tab` started a new document: its arrows and a draw on it go.
    pub(super) fn arrows_new_doc(&mut self, tab: &str) {
        self.drop_point(tab, "the page moved on before the arrow was measured");
        if let Some(t) = self.tabs.get_mut(tab) {
            t.user_arrow = None;
        }
        if self.arrows.lookup.as_ref().is_some_and(|l| l.tab == tab) {
            self.arrows.lookup = None;
        }
        if self.drawing(tab) {
            self.cancel_draw();
        }
    }

    /// The tab's page is gone (its panel or the tab closed).
    pub(super) fn arrows_tab_gone(&mut self, tab: &str) {
        self.drop_point(tab, "the panel was closed");
        if self.arrows.lookup.as_ref().is_some_and(|l| l.tab == tab) {
            self.arrows.lookup = None;
        }
        if self.drawing(tab) {
            self.arrows.draw = None;
            self.keys.drawing.set(false);
        }
    }

    /// The person clicked or typed in the page in front: the agent's arrow there goes at once.
    pub(super) fn arrows_user_input(&mut self, tab: &str) {
        if self.tabs.get(tab).is_some_and(|t| t.point.is_some()) {
            self.drop_point(tab, &Job::interrupted().message);
        }
    }

    /// Whether the agent timer still has an arrow to watch: a measure to make, a draw, a lookup, the person's arrow
    /// kept (it follows the page's scroll).
    pub(super) fn arrows_busy(&self) -> bool {
        self.arrows.draw.is_some()
            || self.arrows.lookup.is_some()
            || self
                .tabs
                .values()
                .any(|t| t.point.is_some() || t.user_arrow.is_some())
    }

    /// One turn of the agent timer for the arrows: the draw mode, the measures, the ends by time, the person's own
    /// click or key over the agent's arrow.
    pub(super) fn arrows_tick(&mut self, now: u64) {
        self.draw_tick();
        if self
            .arrows
            .lookup
            .as_ref()
            .is_some_and(|l| now >= l.started + LOOKUP_MS)
        {
            // The page never said: nothing is typed and the arrow goes.
            self.arrows.lookup = None;
        }
        let tabs: Vec<String> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.point.is_some())
            .map(|(k, _)| k.clone())
            .collect();
        for tab in tabs {
            let Some(t) = self.tabs.get_mut(&tab) else {
                continue;
            };
            let doc = t.doc;
            let Some(p) = t.point.as_mut() else {
                continue;
            };
            if p.doc != doc {
                self.drop_point(&tab, "the page moved on before the arrow was measured");
                continue;
            }
            if p.reply.as_ref().is_some_and(|(_, by)| now >= *by) {
                if let Some((r, _)) = p.reply.take() {
                    let _ = r.send(Err(CtlError::new(
                        code::TIMEOUT,
                        "the page did not say where that is in time",
                    )));
                }
                t.point = None;
                continue;
            }
            if now >= p.until {
                t.point = None;
                continue;
            }
            if p.measure.is_none() && p.scrolling.is_empty() && now >= p.next_ms {
                self.point_measure(&tab);
            }
        }
        // The person's kept arrow: it goes with its time or its page, and while its page is in front the scroll is
        // measured again, so it is drawn where its ends are now. Away from the front its last scroll is not trusted.
        let kept: Vec<String> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.user_arrow.is_some())
            .map(|(k, _)| k.clone())
            .collect();
        for tab in kept {
            let front = self.cursor_on(&tab);
            let Some(t) = self.tabs.get_mut(&tab) else {
                continue;
            };
            let doc = t.doc;
            let Some(u) = t.user_arrow.as_mut() else {
                continue;
            };
            if u.doc != doc || now >= u.until {
                t.user_arrow = None;
                continue;
            }
            if u.measure.is_some_and(|(_, sent)| now >= sent + LOOKUP_MS) {
                u.measure = None;
            }
            if !front {
                u.seen = None;
            } else if u.measure.is_none() && now >= u.next_ms {
                self.user_arrow_measure(&tab, now);
            }
        }
        // The person's own click or key on the page in front takes the agent's arrow away (as it ends a call).
        if let (Some(hwnd), Some((tab, rect)), false) =
            (self.hwnd, self.bounds.clone(), self.splitting)
            && self.arrows.draw.is_none()
            && self.shown_tab.as_deref() == Some(tab.as_str())
            && self.tabs.get(&tab).is_some_and(|t| t.point.is_some())
            && self.phase == Phase::Live
            && (real_button_down_over(hwnd, &rect) || (self.has_focus && real_key_pressed()))
        {
            self.arrows_user_input(&tab);
        }
        self.update_arrows();
    }

    /// Draws the arrows as they are now, where the page is now: the agent's and the person's over the page in front
    /// (live, no pop-up), the pad while the person draws. A frame that is the same as the last is not drawn again.
    pub(super) fn update_arrows(&mut self) {
        let now = self.now_ms();
        // A draw whose page is no longer in front and live (a menu, a tab switch, a pop-up) ends.
        let stale = self.arrows.draw.as_ref().is_some_and(|d| {
            !self.draw_possible(&d.tab) || self.tabs.get(&d.tab).is_none_or(|t| t.doc != d.doc)
        });
        if stale {
            self.arrows.draw = None;
            self.keys.drawing.set(false);
        }
        let place = self
            .shown_tab
            .clone()
            .filter(|t| self.cursor_on(t) && !self.popup_open(t))
            .and_then(|t| match &self.bounds {
                Some((bt, r)) if *bt == t => Some((t, *r)),
                _ => None,
            });
        let (ox, oy) = self.hwnd.map_or((0, 0), client_origin);
        let scale = f64::from(self.scale.clamp(0.5, 4.0));
        let scale_key = (scale * 100.0).round() as u32;
        // The agent's arrow.
        let agent = place.as_ref().and_then(|(tab, rect)| {
            let t = self.tabs.get(tab)?;
            let p = t
                .point
                .as_ref()
                .filter(|p| p.doc == t.doc && now < p.until)?;
            let [x, y, w, h] = p.head?;
            let head = to_client((x, y), rect, (w, h));
            Some((head, *rect, p.label.clone()))
        });
        match agent {
            Some((head, rect, label)) => {
                let tail = point_tail(
                    head,
                    (rect.x, rect.y, rect.w, rect.h),
                    POINT_TAIL_PX * scale,
                );
                let key = (
                    tail.0.signum() as i32,
                    tail.1.signum() as i32,
                    scale_key,
                    label,
                );
                if self
                    .arrows
                    .agent_art
                    .as_ref()
                    .is_none_or(|(k, _)| *k != key)
                {
                    let bmp = compose_arrow(tail, scale, ArrowInk::Agent, &key.3);
                    self.arrows.agent_art = Some((key.clone(), bmp));
                    self.arrows.agent_last = None;
                }
                let (sx, sy) = (ox + head.0, oy + head.1);
                if self.arrows.agent_last.as_ref() != Some(&(sx, sy, key.clone()))
                    && let Some((_, bmp)) = self.arrows.agent_art.as_ref()
                {
                    let shown = self.arrows.agent_shell.show_at(sx, sy, bmp, 255);
                    self.arrows.agent_last = shown.then_some((sx, sy, key));
                }
            }
            None => {
                if self.arrows.agent_last.take().is_some() {
                    self.arrows.agent_shell.hide();
                }
            }
        }
        // The person's arrow: while drawn, while its page is asked about it, or kept for the agent.
        let mark = place.as_ref().and_then(|(tab, rect)| {
            if let Some(d) = self.arrows.draw.as_ref().filter(|d| d.tab == *tab) {
                return d.from.map(|f| (f, d.to));
            }
            if let Some(l) = self.arrows.lookup.as_ref().filter(|l| l.tab == *tab) {
                return Some((l.from_px, l.to_px));
            }
            let t = self.tabs.get(tab)?;
            let u = t
                .user_arrow
                .as_ref()
                .filter(|u| u.doc == t.doc && now < u.until)?;
            // Where its ends are now the page has scrolled; drawn only while all of it shows, so it never points
            // at something else or reaches over Flare's own bar.
            let [sx, sy, w, h] = u.seen?;
            let (from, to) = u.in_view((sx, sy));
            (shows(from, (w, h)) && shows(to, (w, h)))
                .then(|| (to_client(from, rect, (w, h)), to_client(to, rect, (w, h))))
        });
        match mark {
            Some((from, to)) => {
                let key = (
                    ox + to.0,
                    oy + to.1,
                    from.0 - to.0,
                    from.1 - to.1,
                    scale_key,
                );
                if self.arrows.mark_last != Some(key) {
                    let bmp = compose_arrow(
                        (f64::from(key.2), f64::from(key.3)),
                        scale,
                        ArrowInk::Person,
                        "",
                    );
                    let shown = self.arrows.mark_shell.show_at(key.0, key.1, &bmp, 255);
                    self.arrows.mark_last = shown.then_some(key);
                }
            }
            None => {
                if self.arrows.mark_last.take().is_some() {
                    self.arrows.mark_shell.hide();
                }
            }
        }
        // The pad, while the person draws.
        let pad = place.as_ref().and_then(|(tab, rect)| {
            self.arrows
                .draw
                .as_ref()
                .filter(|d| d.tab == *tab)
                .map(|_| {
                    (
                        ox + rect.x,
                        oy + rect.y,
                        rect.w.max(0) as u32,
                        rect.h.max(0) as u32,
                    )
                })
        });
        match pad {
            Some(at) => {
                if self.arrows.pad_last != Some(at) {
                    let shown = self.arrows.pad_shell.show(at.0, at.1, at.2, at.3);
                    self.arrows.pad_last = shown.then_some(at);
                }
            }
            None => {
                if self.arrows.pad_last.take().is_some() {
                    self.arrows.pad_shell.hide();
                }
            }
        }
    }

    /// Every arrow off the screen (the window is ending).
    pub(super) fn arrows_off(&mut self) {
        self.arrows.draw = None;
        self.arrows.lookup = None;
        self.keys.drawing.set(false);
        self.arrows.agent_shell.hide();
        self.arrows.mark_shell.hide();
        self.arrows.pad_shell.hide();
        self.arrows.agent_last = None;
        self.arrows.mark_last = None;
        self.arrows.pad_last = None;
    }

    /// The arrows as the e2e and the tests read them: what each window shows, in client pixels where it has a place.
    pub fn arrow_probe(&self) -> Value {
        let (ox, oy) = self.hwnd.map_or((0, 0), client_origin);
        json!({
            "agent": self.arrows.agent_last.as_ref().map(|(x, y, _)| json!([x - ox, y - oy])),
            "mark": self.arrows.mark_last.map(|(x, y, dx, dy, _)| json!({"to": [x - ox, y - oy], "from": [x - ox + dx, y - oy + dy]})),
            "pad": self.arrows.pad_last.map(|(x, y, w, h)| json!([x - ox, y - oy, w, h])),
            "drawing": self.arrows.draw.as_ref().map(|d| d.tab.clone()),
            "asking": self.arrows.lookup.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_pixels_map_to_css_and_back_and_stay_on_the_page() {
        let rect = PhysRect {
            x: 100,
            y: 50,
            w: 900,
            h: 600,
        };
        let vp = (600.0, 400.0);
        assert_eq!(to_css((100, 50), &rect, vp), (0.0, 0.0));
        assert_eq!(to_css((550, 350), &rect, vp), (300.0, 200.0));
        assert_eq!(to_client((300.0, 200.0), &rect, vp), (550, 350));
        // Past the edges is the edge.
        assert_eq!(to_css((5000, -10), &rect, vp), (599.0, 0.0));
        assert_eq!(clamp_in((5000, -10), &rect), (999, 50));
    }

    /// A kept arrow drawn on a page that was not scrolled.
    fn kept(from: (f64, f64), to: (f64, f64), pointed: Option<Pointed>) -> UserArrow {
        UserArrow {
            from,
            to,
            scroll: (0.0, 0.0),
            seen: None,
            measure: None,
            next_ms: 0,
            pointed,
            doc: 1,
            until: 0,
        }
    }

    #[test]
    fn a_look_gets_the_arrow_with_a_ref_made_for_an_element_it_did_not_list() {
        let u = kept(
            (10.04, 20.0),
            (100.0, 120.06),
            Some(Pointed {
                backend_id: 77,
                role: "button".into(),
                name: "Copy key".into(),
            }),
        );
        let mut refs = RefMap::default();
        refs.ref_for(11);
        let mut v = json!({"elements": [{"ref": "e1"}]});
        add_user_arrow(&mut v, &u, &mut refs);
        assert_eq!(
            v["user_arrow"],
            json!({"from": [10.0, 20.0], "to": [100.0, 120.1], "ref": "e2", "role": "button", "name": "Copy key"})
        );
        assert_eq!(refs.get("e2"), Some(77));
        // Nothing named: no ref, empty words.
        let mut v = json!({});
        let none = UserArrow { pointed: None, ..u };
        add_user_arrow(&mut v, &none, &mut refs);
        assert_eq!(v["user_arrow"]["ref"], Value::Null);
        assert_eq!(v["user_arrow"]["role"], "");
    }

    #[test]
    fn a_look_after_a_scroll_gets_the_arrow_where_its_ends_are_now_and_off_screen_undrawn() {
        use base64::Engine;
        let white = image::RgbaImage::from_pixel(600, 400, image::Rgba([255, 255, 255, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(white)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        // Drawn at 412,300 on a page scrolled 100 down: on the document, its head is at 412,400.
        let u = UserArrow {
            scroll: (0.0, 100.0),
            ..kept((300.0, 250.0), (412.0, 400.0), None)
        };
        let look_at = |dy: f64| {
            json!({
                "viewport": {"width": 600.0, "height": 400.0, "scroll": [0.0, dy]},
                "image": {"data": data, "scale": 1.0},
            })
        };
        // The page was scrolled 200 more since: the arrow is 200 higher in the look, and drawn there.
        let mut v = look_at(300.0);
        add_user_arrow(&mut v, &u, &mut RefMap::default());
        assert_eq!(v["user_arrow"]["to"], json!([412.0, 100.0]));
        assert_eq!(v["user_arrow"]["from"], json!([300.0, -50.0]));
        assert!(v["user_arrow"].get("offscreen").is_none());
        assert_ne!(v["image"]["data"], json!(data), "drawn on the screenshot");
        // Scrolled so far its head is above the look: said so, and the screenshot is left alone.
        let mut v = look_at(800.0);
        add_user_arrow(&mut v, &u, &mut RefMap::default());
        assert_eq!(v["user_arrow"]["to"], json!([412.0, -400.0]));
        assert_eq!(v["user_arrow"]["offscreen"], json!(true));
        assert_eq!(v["image"]["data"], json!(data), "nothing drawn");
        // A look that does not say its scroll uses the release's.
        let mut v = json!({"viewport": {"width": 600.0, "height": 400.0}});
        add_user_arrow(&mut v, &u, &mut RefMap::default());
        assert_eq!(v["user_arrow"]["to"], json!([412.0, 300.0]));
    }

    #[test]
    fn the_screenshot_gets_the_arrow_where_it_was_drawn_at_the_looks_scale() {
        use base64::Engine;
        let (w, h) = (300u32, 200u32);
        let white = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(white)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let img =
            json!({"data": base64::engine::general_purpose::STANDARD.encode(&png), "scale": 2.0});
        let (from, to) = ((20.0, 20.0), (100.0, 60.0));
        let drawn = arrow_on_shot(&img, from, to).expect("drawn");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(drawn)
            .unwrap();
        let out = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(out.dimensions(), (w, h));
        // Halfway along, at twice the CSS pixels: the person's blue.
        let p = out.get_pixel(120, 80);
        assert!(p[2] > p[0] && p[2] > 200 && p[0] < 120, "{p:?}");
        // Far from it, the page as it was.
        assert_eq!(out.get_pixel(290, 190), &image::Rgba([255, 255, 255, 255]));
        // A picture that is not one is left alone.
        assert_eq!(
            arrow_on_shot(&json!({"data": "xx", "scale": 1.0}), from, to),
            None
        );
    }
}
