//! One windowless browser: CEF paints it into a pixel buffer (`on_paint`, BGRA), it speaks the DevTools Protocol
//! through `SendDevToolsMessage` (answers come to an observer, keyed by id), and every handler that could open
//! something native on a Mac is answered here so nothing reaches the screen: popups (`window.open`,
//! `target=_blank`) are refused or loaded into the same view, the context menu is emptied and never run, JavaScript
//! dialogs are answered at once, file pickers are cancelled, permission prompts are denied. `<select>` popups are no
//! native menu in windowless mode: Chromium paints them as a `PET_POPUP` layer, which [`State::frame`] lays over the
//! page.
//!
//! A view made with [`View::create`] carries its walls ([`Walls`]: which addresses it may navigate to, what each
//! request gets, new windows, downloads) and reports through an [`EventSink`] ([`ViewEvent`]); the web panel's host
//! (`flare_web::host` on macOS) is built on that. [`View::open`] is the engine tests' plain view: no walls, no sink,
//! everything read from [`State`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use cef::*;
use serde_json::Value;

use crate::status;

/// The last picture of the page: `width * height` pixels, 4 bytes each, BGRA, premultiplied.
#[derive(Clone, Debug, Default)]
pub struct Paint {
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
}

impl Paint {
    /// The pixel at (x, y) as (r, g, b, a).
    pub fn rgba_at(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y * self.width + x) * 4) as usize;
        let p = self.bgra.get(i..i + 4)?;
        Some((p[2], p[1], p[0], p[3]))
    }

    /// The picture as straight RGBA (premultiplied alpha divided out), for a PNG or an image of the window's.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bgra.len());
        for p in self.bgra.as_chunks::<4>().0 {
            let a = p[3];
            let un = |c: u8| {
                if a == 0 || a == 255 {
                    c
                } else {
                    ((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8
                }
            };
            out.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
        }
        out
    }

    /// The picture as a PNG, or None when there is none or it cannot be encoded.
    pub fn png(&self) -> Option<Vec<u8>> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width, self.height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().ok()?;
            w.write_image_data(&self.to_rgba()).ok()?;
        }
        Some(out)
    }
}

/// What the handlers kept off the screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refused {
    pub popups: u32,
    pub context_menus: u32,
    pub js_dialogs: u32,
    pub before_unload: u32,
    pub file_dialogs: u32,
    pub media_requests: u32,
    pub permission_prompts: u32,
    /// Downloads refused before they started (`Downloads::Cancel`).
    pub downloads: u32,
    /// Requests the request wall answered with a refusal.
    pub requests: u32,
    /// Navigations the navigation wall cancelled (any frame).
    pub navigations: u32,
    /// Programs another scheme would have started (`mailto:` and the rest): never.
    pub os_launches: u32,
}

/// A view's own DevTools calls (enabling the Page domain, the script before the page's own, the bridge's binding)
/// count down from here, so they never meet a caller's id.
const SETUP_IDS: i32 = i32::MAX - 1000;
/// The id of the call that forgets the blank page a view is made on (`Page.resetNavigationHistory`), so Back on
/// the first page goes nowhere, as on Windows.
const RESET_ID: i32 = SETUP_IDS - 100;

/// What a view reports to its owner (the host), on the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewEvent {
    /// The view's own setup answered: it navigates now (Ok) or cannot be used (Err).
    Armed(Result<(), String>),
    /// A main-frame navigation the wall let through began; `nav` is the request's number.
    Loading {
        url: String,
        nav: u64,
    },
    /// A main-frame navigation ended (`status` is WebView2's error status, `0` for success).
    Loaded {
        url: String,
        ok: bool,
        status: i32,
        nav: u64,
    },
    /// The main frame has a new document at `url`.
    Url {
        url: String,
    },
    /// The address changed inside the same document (a fragment, `history.pushState`).
    SameDocument {
        url: String,
    },
    Title {
        title: String,
    },
    History {
        can_back: bool,
        can_forward: bool,
    },
    /// A main-frame navigation the wall cancelled.
    Refused {
        url: String,
    },
    /// A download is held for a yes ([`View::answer_download`]).
    Download {
        id: u64,
        file: String,
        url: String,
    },
    /// A held download that was let through ended: saved at `path` (ok) or not.
    DownloadEnded {
        id: u64,
        path: PathBuf,
        ok: bool,
    },
    Focus {
        has: bool,
    },
    FullScreen {
        on: bool,
    },
    /// The page's process ended.
    Gone {
        why: String,
    },
    /// A DevTools answer for a caller's id.
    Cdp {
        id: u64,
        result: Result<String, String>,
    },
    /// The page called the bridge's binding (a mod's `window.chrome.webview.postMessage`): its text, and the main
    /// frame's address when it did.
    Binding {
        payload: String,
        url: String,
    },
    /// A script dialog the page opened and how it was answered (never shown).
    Dialog {
        kind: &'static str,
    },
    /// A new picture is there ([`View::frame`]).
    Painted,
    /// The browser is gone for good (after [`View::close`]).
    Closed,
}

/// Where a view's events go.
pub type EventSink = Arc<dyn Fn(ViewEvent) + Send + Sync>;
/// May a frame navigate to this address? (`url`, whether it is the main frame.)
pub type NavRule = Arc<dyn Fn(&str, bool) -> bool + Send + Sync>;
/// What one request gets (called on CEF's IO thread).
pub type RequestRule = Arc<dyn Fn(&str) -> Answer + Send + Sync>;

/// The answer of the request wall for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The browser fetches it as usual.
    Pass,
    /// Answered here with this status and no body: the request never leaves the process.
    Refuse(u16),
    /// Answered here with this response.
    Serve {
        status: u16,
        mime: String,
        headers: Vec<(String, String)>,
        body: Arc<Vec<u8>>,
    },
}

/// What a page's `window.open` and `target=_blank` do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewWindows {
    /// Nothing opens.
    Refuse,
    /// The address loads in this view (if the navigation wall lets it).
    SameView,
}

/// What a download the page starts does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Downloads {
    /// Held until the owner answers ([`ViewEvent::Download`], [`View::answer_download`]).
    Ask,
    /// Refused before it starts.
    Cancel,
}

/// The walls of a view.
#[derive(Clone)]
pub struct Walls {
    pub nav: Option<NavRule>,
    pub requests: Option<RequestRule>,
    pub new_windows: NewWindows,
    pub downloads: Downloads,
}

impl Default for Walls {
    /// The engine tests' plain view: no navigation or request wall, no new window, no download.
    fn default() -> Self {
        Walls {
            nav: None,
            requests: None,
            new_windows: NewWindows::Refuse,
            downloads: Downloads::Cancel,
        }
    }
}

/// A download held for the owner's answer, or let through.
struct Held {
    callback: Option<BeforeDownloadCallback>,
    item: u32,
    /// The item's own callback, once CEF gave one (the way to cancel it).
    item_callback: Option<DownloadItemCallback>,
    /// The owner said no (or the view is closing): cancel at the first chance.
    cancel: bool,
    path: Option<PathBuf>,
}

/// The `<select>` list or another popup layer: where it sits (device pixels) and its picture.
#[derive(Clone, Debug, Default)]
struct Popup {
    shown: bool,
    x: i32,
    y: i32,
    paint: Option<Paint>,
}

/// Everything a view's handlers report, read by its owner between pumps.
#[derive(Default)]
pub struct State {
    /// The view's size in CSS pixels and its device scale.
    pub width: i32,
    pub height: i32,
    pub scale: f32,
    pub paint: Option<Paint>,
    pub paints: u64,
    pub popup_paints: u64,
    popup: Popup,
    pub loading: bool,
    /// Main-frame loads that ended: (url, HTTP status).
    pub loads: Vec<(String, i32)>,
    pub load_errors: Vec<String>,
    pub url: String,
    pub title: String,
    /// DevTools answers by message id: the result's JSON, or the error's.
    pub cdp: HashMap<i32, Result<String, String>>,
    /// DevTools events seen.
    pub cdp_events: u64,
    pub refused: Refused,
    /// The view may navigate (its setup answered).
    pub armed: bool,
    setup: HashSet<i32>,
    start_url: String,
    /// The main-frame navigation under way (its request's number).
    nav: u64,
    main_frame: String,
    history: Option<(bool, bool)>,
    downloads: HashMap<u64, Held>,
    download_seq: u64,
    pub focused: bool,
    pub fullscreen: bool,
    pub gone: Option<String>,
    pub closed: bool,
    /// The bridge's binding name, when the view has one.
    binding: Option<String>,
    /// The first page's load has not ended yet: the blank page is still in the history.
    reset_pending: bool,
    /// A main-frame navigation the wall cancelled: its own "aborted" end is not reported (it never began).
    refused_main: Option<String>,
    /// Script dialogs answered, by kind.
    pub dialogs: Vec<&'static str>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("paints", &self.paints)
            .field("armed", &self.armed)
            .field("refused", &self.refused)
            .finish_non_exhaustive()
    }
}

impl State {
    /// The page's picture with the popup layer (an open `<select>` list) laid over it where Chromium put it.
    pub fn frame(&self) -> Option<Paint> {
        let base = self.paint.clone()?;
        let Some(pop) = self.popup.paint.as_ref().filter(|_| self.popup.shown) else {
            return Some(base);
        };
        let mut px = (*base.bgra).clone();
        let (bw, bh) = (base.width as i32, base.height as i32);
        for row in 0..pop.height as i32 {
            let y = self.popup.y + row;
            if !(0..bh).contains(&y) {
                continue;
            }
            for col in 0..pop.width as i32 {
                let x = self.popup.x + col;
                if !(0..bw).contains(&x) {
                    continue;
                }
                let s = ((row * pop.width as i32 + col) * 4) as usize;
                let d = ((y * bw + x) * 4) as usize;
                if let (Some(src), true) = (pop.bgra.get(s..s + 4), d + 4 <= px.len()) {
                    px[d..d + 4].copy_from_slice(src);
                }
            }
        }
        Some(Paint {
            width: base.width,
            height: base.height,
            bgra: Arc::new(px),
        })
    }
}

pub type Shared = Arc<Mutex<State>>;

fn lock(s: &Shared) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn text(s: Option<&CefString>) -> String {
    s.map(|s| s.to_string()).unwrap_or_default()
}

/// What every handler of a view shares: its state, its walls and where its events go.
struct Ctx {
    state: Shared,
    walls: Walls,
    events: Option<EventSink>,
}

type C = Arc<Ctx>;

impl Ctx {
    fn emit(&self, e: ViewEvent) {
        if let Some(sink) = &self.events {
            sink(e);
        }
    }
}

wrap_render_handler! {
    struct Render {
        c: C,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let s = lock(&self.c.state);
                rect.x = 0;
                rect.y = 0;
                rect.width = s.width.max(1);
                rect.height = s.height.max(1);
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(info) = screen_info else { return 0 };
            let s = lock(&self.c.state);
            info.device_scale_factor = s.scale;
            let r = Rect { x: 0, y: 0, width: s.width.max(1), height: s.height.max(1) };
            info.rect = r.clone();
            info.available_rect = r;
            1
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
            let mut s = lock(&self.c.state);
            s.popup.shown = show != 0;
            if show == 0 {
                s.popup.paint = None;
            }
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            let Some(r) = rect else { return };
            let mut s = lock(&self.c.state);
            let k = s.scale.max(1.0);
            s.popup.x = (r.x as f32 * k).round() as i32;
            s.popup.y = (r.y as f32 * k).round() as i32;
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else { return };
            if buffer.is_null() || w == 0 || h == 0 {
                return;
            }
            let len = w as usize * h as usize * 4;
            // SAFETY: CEF hands a buffer of width * height BGRA pixels that lives for this call.
            let px = unsafe { std::slice::from_raw_parts(buffer, len) };
            let paint = Paint { width: w, height: h, bgra: Arc::new(px.to_vec()) };
            {
                let mut s = lock(&self.c.state);
                if type_ == PaintElementType::POPUP {
                    s.popup_paints += 1;
                    s.popup.paint = Some(paint);
                } else {
                    s.paint = Some(paint);
                    s.paints += 1;
                }
            }
            self.c.emit(ViewEvent::Painted);
        }
    }
}

wrap_load_handler! {
    struct Load {
        c: C,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            let now = (can_go_back != 0, can_go_forward != 0);
            let changed = {
                let mut s = lock(&self.c.state);
                s.loading = is_loading != 0;
                let changed = s.armed && s.history != Some(now);
                if changed {
                    s.history = Some(now);
                }
                changed
            };
            if changed {
                self.c.emit(ViewEvent::History { can_back: now.0, can_forward: now.1 });
            }
        }

        fn on_load_end(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            http_status_code: ::std::os::raw::c_int,
        ) {
            let Some(frame) = frame else { return };
            if frame.is_main() == 0 {
                return;
            }
            let url = CefString::from(&frame.url()).to_string();
            let (armed, nav) = {
                let mut s = lock(&self.c.state);
                s.loads.push((url.clone(), http_status_code));
                (s.armed, s.nav)
            };
            if armed && url != "about:blank" {
                forget_blank(&self.c, browser);
                self.c.emit(ViewEvent::Loaded { url, ok: true, status: 0, nav });
            }
        }

        fn on_load_error(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            let url = text(failed_url);
            let code: i32 = error_code.get_raw();
            let main = frame.is_some_and(|f| f.is_main() != 0);
            let (armed, nav, refused) = {
                let mut s = lock(&self.c.state);
                s.load_errors.push(format!("{url} {code} {}", text(error_text)));
                let refused = main && s.refused_main.as_deref() == Some(url.as_str());
                if refused {
                    s.refused_main = None;
                }
                (s.armed, s.nav, refused)
            };
            if main && armed && !refused {
                forget_blank(&self.c, browser);
                self.c.emit(ViewEvent::Loaded {
                    url,
                    ok: false,
                    status: status::web_error_status(code),
                    nav,
                });
            }
        }
    }
}

wrap_display_handler! {
    struct Display {
        c: C,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if frame.is_some_and(|f| f.is_main() != 0) {
                lock(&self.c.state).url = text(url);
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = text(title);
            let armed = {
                let mut s = lock(&self.c.state);
                s.title = title.clone();
                s.armed
            };
            if armed {
                self.c.emit(ViewEvent::Title { title });
            }
        }

        fn on_fullscreen_mode_change(
            &self,
            _browser: Option<&mut Browser>,
            fullscreen: ::std::os::raw::c_int,
        ) {
            let on = fullscreen != 0;
            lock(&self.c.state).fullscreen = on;
            self.c.emit(ViewEvent::FullScreen { on });
        }
    }
}

wrap_life_span_handler! {
    struct LifeSpan {
        c: C,
    }

    impl LifeSpanHandler {
        // A popup would be a new browser with a native window: never. With `NewWindows::SameView` its address
        // loads into this view instead (the navigation wall still decides), as WebView2's `NewWindowRequested`
        // does for the web panel.
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            lock(&self.c.state).refused.popups += 1;
            let url = text(target_url);
            if self.c.walls.new_windows == NewWindows::SameView
                && !url.is_empty()
                && let Some(f) = browser.and_then(|b| b.main_frame())
            {
                f.load_url(Some(&url.as_str().into()));
            }
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            lock(&self.c.state).closed = true;
            self.c.emit(ViewEvent::Closed);
        }
    }
}

wrap_context_menu_handler! {
    struct Menu {
        c: C,
    }

    impl ContextMenuHandler {
        fn on_before_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
        ) {
            if let Some(model) = model {
                model.clear();
            }
        }

        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            _model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> ::std::os::raw::c_int {
            lock(&self.c.state).refused.context_menus += 1;
            if let Some(cb) = callback {
                cb.cancel();
            }
            1
        }
    }
}

wrap_jsdialog_handler! {
    struct Dialogs {
        c: C,
    }

    impl JsdialogHandler {
        // Answered here, never shown: `alert` returns, `confirm` is false, `prompt` is null (the answers the
        // Windows seal gives a plugin's page; a Mac window has no native dialog to borrow).
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            _origin_url: Option<&CefString>,
            dialog_type: JsdialogType,
            _message_text: Option<&CefString>,
            _default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let kind = if dialog_type == JsdialogType::CONFIRM {
                "confirm"
            } else if dialog_type == JsdialogType::PROMPT {
                "prompt"
            } else {
                "alert"
            };
            {
                let mut s = lock(&self.c.state);
                s.refused.js_dialogs += 1;
                s.dialogs.push(kind);
            }
            if let Some(cb) = callback {
                cb.cont(i32::from(kind == "alert"), None);
            }
            self.c.emit(ViewEvent::Dialog { kind });
            1
        }

        fn on_before_unload_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _message_text: Option<&CefString>,
            _is_reload: ::std::os::raw::c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> ::std::os::raw::c_int {
            lock(&self.c.state).refused.before_unload += 1;
            // The leave-page question is answered "leave", as the Windows seal does.
            if let Some(cb) = callback {
                cb.cont(1, None);
            }
            1
        }
    }
}

wrap_dialog_handler! {
    struct Files {
        c: C,
    }

    impl DialogHandler {
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _mode: FileDialogMode,
            _title: Option<&CefString>,
            _default_file_path: Option<&CefString>,
            _accept_filters: Option<&mut CefStringList>,
            _accept_extensions: Option<&mut CefStringList>,
            _accept_descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> ::std::os::raw::c_int {
            lock(&self.c.state).refused.file_dialogs += 1;
            if let Some(cb) = callback {
                cb.cancel();
            }
            1
        }
    }
}

wrap_permission_handler! {
    struct Permissions {
        c: C,
    }

    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _requesting_origin: Option<&CefString>,
            _requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            lock(&self.c.state).refused.media_requests += 1;
            if let Some(cb) = callback {
                cb.cancel();
            }
            1
        }

        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            _prompt_id: u64,
            _requesting_origin: Option<&CefString>,
            _requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            lock(&self.c.state).refused.permission_prompts += 1;
            if let Some(cb) = callback {
                cb.cont(PermissionRequestResult::DENY);
            }
            1
        }
    }
}

wrap_focus_handler! {
    struct Focus {
        c: C,
    }

    impl FocusHandler {
        fn on_got_focus(&self, _browser: Option<&mut Browser>) {
            let was = std::mem::replace(&mut lock(&self.c.state).focused, true);
            if !was {
                self.c.emit(ViewEvent::Focus { has: true });
            }
        }

        // Tab past the page's last control: the page gives the keyboard back.
        fn on_take_focus(&self, _browser: Option<&mut Browser>, _next: ::std::os::raw::c_int) {
            let was = std::mem::replace(&mut lock(&self.c.state).focused, false);
            if was {
                self.c.emit(ViewEvent::Focus { has: false });
            }
        }
    }
}

wrap_download_handler! {
    struct Download {
        c: C,
    }

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            if self.c.walls.downloads == Downloads::Cancel {
                lock(&self.c.state).refused.downloads += 1;
                return 0;
            }
            1
        }

        // Held: CEF waits for the callback; nothing is written where the person keeps files until the owner says
        // yes (Chromium keeps the bytes in its own temporary file meanwhile, as WebView2 does under its deferral).
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(cb)) = (download_item, callback) else { return 1 };
            if self.c.walls.downloads == Downloads::Cancel {
                lock(&self.c.state).refused.downloads += 1;
                return 1;
            }
            let url = CefString::from(&item.url()).to_string();
            let file = text(suggested_name);
            let id = {
                let mut s = lock(&self.c.state);
                s.download_seq += 1;
                let id = s.download_seq;
                s.downloads.insert(id, Held {
                    callback: Some(cb.clone()),
                    item: item.id(),
                    item_callback: None,
                    cancel: false,
                    path: None,
                });
                id
            };
            self.c.emit(ViewEvent::Download { id, file, url });
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else { return };
            let item_id = item.id();
            let done = item.is_complete() != 0;
            let failed = item.is_canceled() != 0 || item.is_interrupted() != 0;
            let mut ended = None;
            // CEF calls are made with the state unlocked: a cancel can call this handler again at once.
            let mut cancel_now = None;
            let mut gone = None;
            {
                let mut s = lock(&self.c.state);
                let Some((&id, held)) = s.downloads.iter_mut().find(|(_, h)| h.item == item_id) else {
                    return;
                };
                if let Some(cb) = callback {
                    held.item_callback = Some(cb.clone());
                    if held.cancel && !done && !failed {
                        cancel_now = Some(cb.clone());
                    }
                }
                if done || failed {
                    ended = Some((id, held.path.clone(), done && !held.cancel));
                }
                if let Some((id, _, _)) = ended {
                    gone = s.downloads.remove(&id);
                }
            }
            drop(gone);
            if let Some(cb) = cancel_now {
                cb.cancel();
            }
            if let Some((id, path, ok)) = ended
                && let Some(path) = path
            {
                self.c.emit(ViewEvent::DownloadEnded { id, path, ok });
            }
        }
    }
}

wrap_resource_handler! {
    // One answer the request wall gives itself: a refusal or a file, never the network.
    struct Served {
        status: u16,
        mime: String,
        headers: Vec<(String, String)>,
        body: Arc<Vec<u8>>,
        at: Arc<Mutex<usize>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            if let Some(h) = handle_request {
                *h = 1;
            }
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            if let Some(r) = response {
                r.set_status(i32::from(self.status));
                let reason = if self.status == 200 { "OK" } else { refusal_reason(self.status) };
                r.set_status_text(Some(&reason.into()));
                // CEF takes the media type and its charset apart (`text/html; charset=utf-8` as a whole reads as
                // no type, and the page shows as text).
                let (mime, charset) = split_mime(&self.mime);
                if !mime.is_empty() {
                    r.set_mime_type(Some(&mime.as_str().into()));
                }
                if let Some(c) = charset {
                    r.set_charset(Some(&c.as_str().into()));
                }
                for (k, v) in &self.headers {
                    if !k.eq_ignore_ascii_case("content-type") {
                        r.set_header_by_name(Some(&k.as_str().into()), Some(&v.as_str().into()), 1);
                    }
                }
            }
            if let Some(len) = response_length {
                *len = i64::try_from(self.body.len()).unwrap_or(0);
            }
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: ::std::os::raw::c_int,
            bytes_read: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> ::std::os::raw::c_int {
            let mut at = self.at.lock().unwrap_or_else(|e| e.into_inner());
            let left = self.body.len().saturating_sub(*at);
            let n = left.min(usize::try_from(bytes_to_read).unwrap_or(0));
            if n > 0 && !data_out.is_null() {
                // SAFETY: CEF hands a buffer of at least `bytes_to_read` bytes for this call; `n` is no more.
                unsafe { std::ptr::copy_nonoverlapping(self.body[*at..].as_ptr(), data_out, n) };
                *at += n;
            }
            if let Some(b) = bytes_read {
                *b = i32::try_from(n).unwrap_or(0);
            }
            i32::from(n > 0)
        }

        fn cancel(&self) {}
    }
}

/// The reason phrase of a refusal (the Windows host's).
pub fn refusal_reason(status: u16) -> &'static str {
    match status {
        403 => "Blocked by Flare",
        404 => "Not found",
        413 => "Too large",
        503 => "Busy",
        _ => "Refused",
    }
}

/// A media type and its charset: `text/html; charset=utf-8` is (`text/html`, `utf-8`).
pub fn split_mime(m: &str) -> (String, Option<String>) {
    let mut parts = m.split(';');
    let base = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let charset = parts.find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| v.trim().trim_matches('"').to_string())
    });
    (base, charset)
}

/// The handler a request of a walled view gets (CEF's IO thread): the wall's answer, served here or passed on.
fn served(answer: Answer) -> Option<ResourceHandler> {
    let (status, mime, headers, body) = match answer {
        Answer::Pass => return None,
        // A refusal is a real answer with its status, as the Windows host gives one (an empty body reads as a broken
        // response to Chromium, so it carries its reason).
        Answer::Refuse(status) => (
            status,
            "text/plain; charset=utf-8".to_string(),
            vec![("X-Content-Type-Options".to_string(), "nosniff".to_string())],
            Arc::new(refusal_reason(status).as_bytes().to_vec()),
        ),
        Answer::Serve {
            status,
            mime,
            headers,
            body,
        } => (status, mime, headers, body),
    };
    Some(Served::new(
        status,
        mime,
        headers,
        body,
        Arc::new(Mutex::new(0)),
    ))
}

wrap_resource_request_handler! {
    // The request wall of one view (or of a request context, for requests no browser makes: service workers).
    pub struct Wall {
        rule: RequestRule,
        state: Option<Shared>,
    }

    impl ResourceRequestHandler {
        // The bindings' default answer is RV_CANCEL (0): every request would be dropped. The wall answers in
        // `resource_handler`, so the request goes on to it here.
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            ReturnValue::CONTINUE
        }

        fn resource_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            let answer = (self.rule)(&url);
            if matches!(answer, Answer::Refuse(_))
                && let Some(s) = &self.state
            {
                lock(s).refused.requests += 1;
            }
            served(answer)
        }

        // A scheme that would start a program (`mailto:`, `zoommtg:`): never.
        fn on_protocol_execution(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            allow_os_execution: Option<&mut ::std::os::raw::c_int>,
        ) {
            if let Some(a) = allow_os_execution {
                *a = 0;
            }
            if let Some(s) = &self.state {
                lock(s).refused.os_launches += 1;
            }
        }
    }
}

/// The request wall for a request context (requests of no browser, such as a service worker's).
pub fn context_wall(rule: RequestRule) -> ResourceRequestHandler {
    Wall::new(rule, None)
}

wrap_request_handler! {
    struct Requests {
        c: C,
    }

    impl RequestHandler {
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: ::std::os::raw::c_int,
            _is_redirect: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let Some(request) = request else { return 1 };
            let url = CefString::from(&request.url()).to_string();
            let main = frame.is_some_and(|f| f.is_main() != 0);
            let armed = lock(&self.c.state).armed;
            // The blank page a view is made on, before its own address: inert, and never reported.
            if !armed && url == "about:blank" {
                return 0;
            }
            let allowed = self.c.walls.nav.as_ref().is_none_or(|rule| rule(&url, main));
            if !allowed {
                {
                    let mut s = lock(&self.c.state);
                    s.refused.navigations += 1;
                    if main {
                        s.refused_main = Some(url.clone());
                    }
                }
                if main {
                    self.c.emit(ViewEvent::Refused { url });
                }
                return 1;
            }
            if main {
                let nav = request.identifier();
                lock(&self.c.state).nav = nav;
                if armed {
                    self.c.emit(ViewEvent::Loading { url, nav });
                }
            }
            0
        }

        // A middle click or Cmd+click asks for a tab of its own: this view, or nothing.
        fn on_open_urlfrom_tab(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let url = text(target_url);
            if self.c.walls.new_windows == NewWindows::SameView
                && let Some(f) = browser.and_then(|b| b.main_frame())
            {
                f.load_url(Some(&url.as_str().into()));
            }
            1
        }

        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            let rule = self.c.walls.requests.clone()?;
            Some(Wall::new(rule, Some(self.c.state.clone())))
        }

        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            status: TerminationStatus,
            error_code: ::std::os::raw::c_int,
            _error_string: Option<&CefString>,
        ) {
            let why = format!(
                "the page's process ended ({}, code {error_code})",
                termination_words(status)
            );
            lock(&self.c.state).gone = Some(why.clone());
            self.c.emit(ViewEvent::Gone { why });
        }
    }
}

/// After the first page's load ends, the blank page the view was made on leaves the history (once).
fn forget_blank(c: &C, browser: Option<&mut Browser>) {
    let due = std::mem::take(&mut lock(&c.state).reset_pending);
    if !due {
        return;
    }
    let Some(host) = browser.and_then(|b| b.host()) else {
        return;
    };
    if let Ok((_, msg)) = cdp_message(
        u64::try_from(RESET_ID).unwrap_or(0),
        "Page.resetNavigationHistory",
        &Value::Null,
    ) {
        host.send_dev_tools_message(Some(&msg));
    }
}

fn termination_words(s: TerminationStatus) -> &'static str {
    if s == TerminationStatus::PROCESS_CRASHED {
        "crashed"
    } else if s == TerminationStatus::PROCESS_WAS_KILLED {
        "killed"
    } else if s == TerminationStatus::PROCESS_OOM {
        "out of memory"
    } else if s == TerminationStatus::LAUNCH_FAILED {
        "could not start"
    } else {
        "ended abnormally"
    }
}

wrap_dev_tools_message_observer! {
    struct DevTools {
        c: C,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            let body = String::from_utf8_lossy(result.unwrap_or_default()).into_owned();
            if message_id == RESET_ID {
                // The history is the page alone now: nothing to go back or forward to.
                lock(&self.c.state).history = Some((false, false));
                self.c.emit(ViewEvent::History { can_back: false, can_forward: false });
                return;
            }
            let answer = if success != 0 { Ok(body) } else { Err(cdp_error(&body)) };
            let to_owner = {
                let mut s = lock(&self.c.state);
                if s.setup.contains(&message_id) || self.c.events.is_none() {
                    s.cdp.insert(message_id, answer.clone());
                    false
                } else {
                    true
                }
            };
            if to_owner && let Ok(id) = u64::try_from(message_id) {
                self.c.emit(ViewEvent::Cdp { id, result: answer });
            }
        }

        fn on_dev_tools_event(
            &self,
            _browser: Option<&mut Browser>,
            method: Option<&CefString>,
            params: Option<&[u8]>,
        ) {
            let method = text(method);
            lock(&self.c.state).cdp_events += 1;
            if !matches!(
                method.as_str(),
                "Page.frameNavigated" | "Page.navigatedWithinDocument" | "Runtime.bindingCalled"
            ) {
                return;
            }
            let Ok(p) = serde_json::from_slice::<Value>(params.unwrap_or_default()) else { return };
            let event = {
                let mut s = lock(&self.c.state);
                page_event(&mut s, &method, &p)
            };
            if let Some(e) = event {
                self.c.emit(e);
            }
        }
    }
}

/// The view event a DevTools event of the page is, if any: a new document in the main frame (`Url`), a change of
/// address inside it (`SameDocument`), a call of the bridge's binding.
fn page_event(s: &mut State, method: &str, p: &Value) -> Option<ViewEvent> {
    let str_of = |v: &Value| v.as_str().unwrap_or_default().to_string();
    match method {
        "Page.frameNavigated" => {
            let f = &p["frame"];
            if f.get("parentId").is_some() {
                return None;
            }
            s.main_frame = str_of(&f["id"]);
            let url = format!("{}{}", str_of(&f["url"]), str_of(&f["urlFragment"]));
            (s.armed && url != "about:blank").then_some(ViewEvent::Url { url })
        }
        "Page.navigatedWithinDocument" => {
            (s.armed && str_of(&p["frameId"]) == s.main_frame).then(|| ViewEvent::SameDocument {
                url: str_of(&p["url"]),
            })
        }
        "Runtime.bindingCalled" => {
            let name = str_of(&p["name"]);
            (s.binding.as_deref() == Some(name.as_str())).then(|| ViewEvent::Binding {
                payload: str_of(&p["payload"]),
                url: s.url.clone(),
            })
        }
        _ => None,
    }
}

/// Every handler of a view (one field, so the client stays a small struct).
#[derive(Clone)]
struct Handlers {
    render: RenderHandler,
    load: LoadHandler,
    display: DisplayHandler,
    life: LifeSpanHandler,
    menu: ContextMenuHandler,
    dialogs: JsdialogHandler,
    files: DialogHandler,
    permissions: PermissionHandler,
    focus: FocusHandler,
    download: DownloadHandler,
    requests: RequestHandler,
}

wrap_client! {
    struct ViewClient {
        h: Handlers,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.h.render.clone())
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.h.load.clone())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.h.display.clone())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.h.life.clone())
        }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.h.menu.clone())
        }
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(self.h.dialogs.clone())
        }
        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.h.files.clone())
        }
        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(self.h.permissions.clone())
        }
        fn focus_handler(&self) -> Option<FocusHandler> {
            Some(self.h.focus.clone())
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.h.download.clone())
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.h.requests.clone())
        }
    }
}

/// A DevTools error as one sentence: Chromium's `{"code":-32000,"message":"..."}`, its message first.
pub fn cdp_error(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => match v.get("message").and_then(Value::as_str) {
            Some(m) => format!("{m} {body}"),
            None => body.to_string(),
        },
        Err(_) if body.is_empty() => "the DevTools call failed".to_string(),
        Err(_) => body.to_string(),
    }
}

/// The DevTools message `SendDevToolsMessage` takes: `{"id":..,"method":..,"params":..}`. Ids are CEF's `int`.
pub fn cdp_message(id: u64, method: &str, params: &Value) -> Result<(i32, Vec<u8>), String> {
    let id = i32::try_from(id).map_err(|_| format!("DevTools id {id} is past CEF's int"))?;
    let params = if params.is_object() {
        params.clone()
    } else {
        Value::Object(Default::default())
    };
    let msg = serde_json::json!({"id": id, "method": method, "params": params});
    Ok((id, msg.to_string().into_bytes()))
}

/// The size of a new view.
#[derive(Clone, Copy, Debug)]
pub struct ViewSize {
    pub width: i32,
    pub height: i32,
    pub scale: f32,
}

/// How a view is made ([`View::create`]).
pub struct Options {
    pub url: String,
    pub size: ViewSize,
    pub walls: Walls,
    pub events: Option<EventSink>,
    /// The request context (the profile) the view runs in; None for an in-memory one of its own.
    pub context: Option<RequestContext>,
    /// A script that runs in every document of the view before the page's own
    /// (`Page.addScriptToEvaluateOnNewDocument`): the view navigates only once it is in.
    pub init_script: Option<String>,
    /// A global function the page may call with one string (`Runtime.addBinding`), reported as
    /// [`ViewEvent::Binding`].
    pub binding: Option<String>,
    pub frame_rate: i32,
}

/// A key as CEF takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub kind: KeyKind,
    /// The Windows virtual-key code (Chromium's `keyCode`).
    pub windows_code: i32,
    /// The macOS key code (`kVK_*`).
    pub native_code: i32,
    /// The UTF-16 character, for a `Char`.
    pub character: u16,
    pub unmodified: u16,
    /// CEF's event flags ([`crate::status::flags`]).
    pub modifiers: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyKind {
    #[default]
    Down,
    Up,
    Char,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// One windowless browser.
pub struct View {
    browser: Browser,
    state: Shared,
    _observer: Option<Registration>,
}

fn new_state(size: ViewSize) -> Shared {
    Arc::new(Mutex::new(State {
        width: size.width,
        height: size.height,
        scale: size.scale,
        ..Default::default()
    }))
}

impl View {
    /// The engine tests' plain view: windowless, Alloy style, its own in-memory request context, no walls, no
    /// event sink; it starts `url` at once.
    pub fn open(url: &str, size: ViewSize) -> Result<View, String> {
        let state = new_state(size);
        {
            let mut s = lock(&state);
            s.armed = true;
        }
        Self::make(url, state, Walls::default(), None, None, 30)
    }

    /// A view with walls and an event sink. It is made on `about:blank` and navigates to `opts.url` only when its
    /// setup (the Page domain, the script and the binding) has answered ([`View::poll`] does it;
    /// [`ViewEvent::Armed`] says so).
    pub fn create(opts: Options) -> Result<View, String> {
        let state = new_state(opts.size);
        let c = Ctx {
            state: state.clone(),
            walls: opts.walls.clone(),
            events: opts.events.clone(),
        };
        {
            let mut s = lock(&state);
            s.start_url = opts.url.clone();
            s.binding = opts.binding.clone();
        }
        let view = Self::make_with("about:blank", Arc::new(c), opts.context, opts.frame_rate)?;
        let mut calls: Vec<(&str, Value)> = vec![("Page.enable", serde_json::json!({}))];
        if let Some(src) = &opts.init_script {
            calls.push((
                "Page.addScriptToEvaluateOnNewDocument",
                serde_json::json!({"source": src}),
            ));
        }
        if let Some(name) = &opts.binding {
            calls.push(("Runtime.addBinding", serde_json::json!({"name": name})));
            calls.push(("Runtime.enable", serde_json::json!({})));
        }
        for (i, (method, params)) in calls.into_iter().enumerate() {
            let id = SETUP_IDS - i32::try_from(i).unwrap_or(0);
            lock(&view.state).setup.insert(id);
            view.cdp_send(u64::try_from(id).unwrap_or(0), method, &params)?;
        }
        Ok(view)
    }

    fn make(
        url: &str,
        state: Shared,
        walls: Walls,
        events: Option<EventSink>,
        context: Option<RequestContext>,
        frame_rate: i32,
    ) -> Result<View, String> {
        let c = Arc::new(Ctx {
            state,
            walls,
            events,
        });
        Self::make_with(url, c, context, frame_rate)
    }

    fn make_with(
        url: &str,
        c: C,
        context: Option<RequestContext>,
        frame_rate: i32,
    ) -> Result<View, String> {
        let state = c.state.clone();
        let mut client = ViewClient::new(Handlers {
            render: Render::new(c.clone()),
            load: Load::new(c.clone()),
            display: Display::new(c.clone()),
            life: LifeSpan::new(c.clone()),
            menu: Menu::new(c.clone()),
            dialogs: Dialogs::new(c.clone()),
            files: Files::new(c.clone()),
            permissions: Permissions::new(c.clone()),
            focus: Focus::new(c.clone()),
            download: Download::new(c.clone()),
            requests: Requests::new(c.clone()),
        });
        let window_info = WindowInfo {
            windowless_rendering_enabled: 1,
            // Windowless needs the Alloy style (no Chrome UI, no window).
            runtime_style: RuntimeStyle::ALLOY,
            ..Default::default()
        };
        let settings = BrowserSettings {
            windowless_frame_rate: frame_rate.clamp(1, 60),
            ..Default::default()
        };
        // An in-memory context of the view's own unless the owner gives one: nothing is shared with another
        // view or kept on disk.
        let mut context = context.or_else(|| {
            request_context_create_context(Some(&RequestContextSettings::default()), None)
        });
        let browser = browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&url.into()),
            Some(&settings),
            None,
            context.as_mut(),
        )
        .ok_or("CEF did not create the browser")?;
        let host = browser.host().ok_or("the browser has no host")?;
        let observer = host.add_dev_tools_message_observer(Some(&mut DevTools::new(c)));
        if observer.is_none() {
            return Err("the DevTools observer was refused".to_string());
        }
        Ok(View {
            browser,
            state,
            _observer: observer,
        })
    }

    /// Takes in the setup's answers: once all came, the view navigates to its address (Ok(true)); an answer that
    /// failed is the view's end (Err). Ok(false) while waiting, and for a view already armed.
    pub fn poll(&self) -> Result<bool, String> {
        let url = {
            let mut s = lock(&self.state);
            if s.armed || s.setup.is_empty() {
                return Ok(false);
            }
            let ids: Vec<i32> = s.setup.iter().copied().collect();
            if !ids.iter().all(|id| s.cdp.contains_key(id)) {
                return Ok(false);
            }
            let mut why = None;
            for id in ids {
                if let Some(Err(e)) = s.cdp.remove(&id) {
                    why.get_or_insert(e);
                }
            }
            s.setup.clear();
            if let Some(e) = why {
                return Err(format!("the view's setup failed: {e}"));
            }
            s.armed = true;
            s.reset_pending = true;
            s.start_url.clone()
        };
        self.navigate(&url);
        Ok(true)
    }

    /// What the handlers reported so far.
    pub fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// The page's latest picture with its popup layer, if it painted.
    pub fn frame(&self) -> Option<Paint> {
        lock(&self.state).frame()
    }

    fn host(&self) -> Result<BrowserHost, String> {
        self.browser
            .host()
            .ok_or_else(|| "the browser has no host".to_string())
    }

    /// Sends one DevTools call; its answer comes to [`View::cdp_take`] (no sink) or as [`ViewEvent::Cdp`].
    pub fn cdp_send(&self, id: u64, method: &str, params: &Value) -> Result<(), String> {
        let (_, msg) = cdp_message(id, method, params)?;
        if self.host()?.send_dev_tools_message(Some(&msg)) != 1 {
            return Err(format!("CEF did not take the DevTools call {method}"));
        }
        Ok(())
    }

    /// The answer of call `id`, once it came.
    pub fn cdp_take(&self, id: u64) -> Option<Result<String, String>> {
        let id = i32::try_from(id).ok()?;
        lock(&self.state).cdp.remove(&id)
    }

    /// Loads `url` in the main frame.
    pub fn navigate(&self, url: &str) {
        if let Some(f) = self.browser.main_frame() {
            f.load_url(Some(&url.into()));
        }
    }

    pub fn back(&self) {
        self.browser.go_back();
    }

    pub fn forward(&self) {
        self.browser.go_forward();
    }

    pub fn reload(&self) {
        self.browser.reload();
    }

    pub fn stop(&self) {
        self.browser.stop_load();
    }

    /// Runs a script in the main frame (no answer; `Runtime.evaluate` is the way to read one back).
    pub fn exec(&self, js: &str) {
        if let Some(f) = self.browser.main_frame() {
            f.execute_java_script(Some(&js.into()), None, 0);
        }
    }

    /// Changes the view's size (CSS pixels) and device scale, and tells CEF.
    pub fn resize_scaled(&self, width: i32, height: i32, scale: f32) -> Result<(), String> {
        let scale_changed = {
            let mut s = lock(&self.state);
            s.width = width;
            s.height = height;
            let changed = (s.scale - scale).abs() > f32::EPSILON;
            s.scale = scale;
            changed
        };
        let h = self.host()?;
        if scale_changed {
            h.notify_screen_info_changed();
        }
        h.was_resized();
        Ok(())
    }

    /// Changes the view's size (CSS pixels) and tells CEF.
    pub fn resize(&self, width: i32, height: i32) -> Result<(), String> {
        let scale = lock(&self.state).scale;
        self.resize_scaled(width, height, scale)
    }

    /// Hidden: no paints, timers slowed (a panel that is not shown); shown again with false.
    pub fn set_hidden(&self, hidden: bool) {
        if let Ok(h) = self.host() {
            h.was_hidden(i32::from(hidden));
        }
    }

    /// Gives the page the keyboard (true) or takes it away.
    pub fn set_focus(&self, focus: bool) {
        if let Ok(h) = self.host() {
            h.set_focus(i32::from(focus));
        }
        lock(&self.state).focused = focus;
    }

    /// The browser's own zoom, as a factor (1.0 is 100 %).
    pub fn set_zoom(&self, factor: f64) {
        if let Ok(h) = self.host() {
            h.set_zoom_level(status::zoom_level(factor));
        }
    }

    pub fn zoom_level(&self) -> f64 {
        self.host().map(|h| h.zoom_level()).unwrap_or(0.0)
    }

    /// The pointer moved to (x, y), CSS pixels of the view; `leave` when it left the view.
    pub fn mouse_move(&self, x: i32, y: i32, modifiers: u32, leave: bool) {
        if let Ok(h) = self.host() {
            let e = MouseEvent { x, y, modifiers };
            h.send_mouse_move_event(Some(&e), i32::from(leave));
        }
    }

    /// A button went down or up at (x, y); `clicks` is the click count (2 for a double click).
    pub fn mouse_button(&self, x: i32, y: i32, button: Button, up: bool, clicks: i32, mods: u32) {
        if let Ok(h) = self.host() {
            let e = MouseEvent {
                x,
                y,
                modifiers: mods,
            };
            let b = match button {
                Button::Left => MouseButtonType::LEFT,
                Button::Middle => MouseButtonType::MIDDLE,
                Button::Right => MouseButtonType::RIGHT,
            };
            h.send_mouse_click_event(Some(&e), b, i32::from(up), clicks.max(1));
        }
    }

    /// A wheel or trackpad scroll at (x, y) by (dx, dy) CSS pixels (positive dy scrolls the page up, as CEF has it).
    pub fn wheel(&self, x: i32, y: i32, dx: i32, dy: i32, mods: u32) {
        if let Ok(h) = self.host() {
            let e = MouseEvent {
                x,
                y,
                modifiers: mods,
            };
            h.send_mouse_wheel_event(Some(&e), dx, dy);
        }
    }

    pub fn key(&self, k: Key) {
        let Ok(h) = self.host() else { return };
        let e = KeyEvent {
            type_: match k.kind {
                KeyKind::Down => KeyEventType::RAWKEYDOWN,
                KeyKind::Up => KeyEventType::KEYUP,
                KeyKind::Char => KeyEventType::CHAR,
            },
            modifiers: k.modifiers,
            windows_key_code: k.windows_code,
            native_key_code: k.native_code,
            character: k.character,
            unmodified_character: k.unmodified,
            ..Default::default()
        };
        h.send_key_event(Some(&e));
    }

    /// The IME's text being composed (underlined, not yet the page's), the caret at `cursor` (UTF-16 units).
    pub fn ime_compose(&self, text: &str, cursor: u32) {
        if let Ok(h) = self.host() {
            let none = Range {
                from: u32::MAX,
                to: u32::MAX,
            };
            let sel = Range {
                from: cursor,
                to: cursor,
            };
            h.ime_set_composition(Some(&text.into()), None, Some(&none), Some(&sel));
        }
    }

    /// The IME's text is chosen: it goes into the page.
    pub fn ime_commit(&self, text: &str) {
        if let Ok(h) = self.host() {
            let none = Range {
                from: u32::MAX,
                to: u32::MAX,
            };
            h.ime_commit_text(Some(&text.into()), Some(&none), 0);
        }
    }

    pub fn ime_cancel(&self) {
        if let Ok(h) = self.host() {
            h.ime_cancel_composition();
        }
    }

    /// The owner's answer to a held download: a yes saves it as `to` (never asking where), a no (None) cancels it.
    /// False for an id that is not held.
    pub fn answer_download(&self, id: u64, to: Option<&Path>) -> bool {
        // Taken out under the lock, called after it: CEF may call the download handlers back at once.
        // Even letting go of a CEF object can run CEF's work (and its handlers) at once, so the before-download
        // callback leaves the lock first and is answered or dropped after it.
        let (callback, cancel) = {
            let mut s = lock(&self.state);
            let Some(held) = s.downloads.get_mut(&id) else {
                return false;
            };
            if let Some(path) = to {
                held.path = Some(path.to_path_buf());
                (held.callback.take(), None)
            } else {
                held.cancel = true;
                (held.callback.take(), held.item_callback.clone())
            }
        };
        match (callback, to) {
            (Some(cb), Some(path)) => {
                let p = path.to_string_lossy().into_owned();
                cb.cont(Some(&p.as_str().into()), 0);
            }
            (cb, _) => drop(cb),
        }
        if let Some(cb) = cancel {
            cb.cancel();
        }
        true
    }

    /// Every held download is cancelled (the view is closing, the window is ending).
    pub fn cancel_downloads(&self) {
        let ids: Vec<u64> = lock(&self.state).downloads.keys().copied().collect();
        for id in ids {
            self.answer_download(id, None);
        }
    }

    /// Asks for a fresh paint of the whole view.
    pub fn invalidate(&self) {
        if let Ok(h) = self.host() {
            h.invalidate(PaintElementType::VIEW);
        }
    }

    /// The main frame's address now.
    pub fn url(&self) -> String {
        self.browser
            .main_frame()
            .map(|f| CefString::from(&f.url()).to_string())
            .unwrap_or_default()
    }

    /// Closes the browser (forced: no unload question). [`ViewEvent::Closed`] follows.
    pub fn close(&self) {
        self.cancel_downloads();
        if let Ok(h) = self.host() {
            h.close_browser(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_devtools_call_is_one_message_with_the_callers_id_and_an_object_for_params() {
        let (id, msg) = cdp_message(7, "DOM.focus", &json!({"backendNodeId": 12})).unwrap();
        assert_eq!(id, 7);
        let v: Value = serde_json::from_slice(&msg).unwrap();
        assert_eq!(
            v,
            json!({"id": 7, "method": "DOM.focus", "params": {"backendNodeId": 12}})
        );
        let (_, msg) = cdp_message(8, "DOM.enable", &Value::Null).unwrap();
        let v: Value = serde_json::from_slice(&msg).unwrap();
        assert_eq!(v["params"], json!({}));
    }

    #[test]
    fn an_id_past_cefs_int_is_refused_not_wrapped() {
        assert!(cdp_message(u64::from(u32::MAX), "DOM.enable", &json!({})).is_err());
        assert!(cdp_message(i32::MAX as u64, "DOM.enable", &json!({})).is_ok());
    }

    #[test]
    fn a_devtools_error_reads_as_its_message_with_chromiums_object() {
        let e = cdp_error(r#"{"code":-32603,"message":"Internal error"}"#);
        assert!(e.starts_with("Internal error "), "{e}");
        assert!(e.contains("-32603"));
        assert_eq!(cdp_error(""), "the DevTools call failed");
        assert_eq!(cdp_error("not json"), "not json");
    }

    #[test]
    fn a_pixel_is_read_from_bgra_and_outside_the_buffer_is_none() {
        let p = Paint {
            width: 2,
            height: 1,
            bgra: Arc::new(vec![0xcc, 0x66, 0x33, 0xff, 1, 2, 3, 4]),
        };
        assert_eq!(p.rgba_at(0, 0), Some((0x33, 0x66, 0xcc, 0xff)));
        assert_eq!(p.rgba_at(1, 0), Some((3, 2, 1, 4)));
        assert_eq!(p.rgba_at(2, 0), None);
        assert_eq!(p.rgba_at(0, 1), None);
    }

    #[test]
    fn a_media_type_and_its_charset_go_to_cef_apart() {
        assert_eq!(
            split_mime("text/html; charset=utf-8"),
            ("text/html".into(), Some("utf-8".into()))
        );
        assert_eq!(split_mime("image/png"), ("image/png".into(), None));
        assert_eq!(
            split_mime("Text/Plain;Charset=\"UTF-8\""),
            ("text/plain".into(), Some("UTF-8".into()))
        );
    }

    #[test]
    fn a_premultiplied_pixel_comes_out_straight_and_a_still_is_a_png() {
        let p = Paint {
            width: 2,
            height: 1,
            bgra: Arc::new(vec![0, 0, 0x80, 0x80, 0x10, 0x20, 0x30, 0xff]),
        };
        assert_eq!(p.to_rgba(), vec![0xff, 0, 0, 0x80, 0x30, 0x20, 0x10, 0xff]);
        let png = p.png().unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(Paint::default().png().is_none());
    }

    #[test]
    fn an_open_select_list_is_laid_over_the_page_where_chromium_put_it() {
        let mut s = State {
            paint: Some(Paint {
                width: 3,
                height: 2,
                bgra: Arc::new(vec![0; 24]),
            }),
            ..Default::default()
        };
        s.popup = Popup {
            shown: true,
            x: 2,
            y: 1,
            paint: Some(Paint {
                width: 2,
                height: 2,
                bgra: Arc::new(vec![9; 16]),
            }),
        };
        let f = s.frame().unwrap();
        // Only (2, 1) of the popup falls inside the page; the rest is cut off.
        let painted: Vec<usize> = f
            .bgra
            .chunks(4)
            .enumerate()
            .filter(|(_, p)| p[0] == 9)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(painted, vec![5]);
        s.popup.shown = false;
        assert!(s.frame().unwrap().bgra.iter().all(|b| *b == 0));
    }

    #[test]
    fn devtools_events_become_new_documents_same_document_moves_and_binding_calls() {
        let mut s = State {
            armed: true,
            binding: Some("__flarePost".into()),
            url: "https://a.example/".into(),
            ..Default::default()
        };
        let main = page_event(
            &mut s,
            "Page.frameNavigated",
            &json!({"frame": {"id": "F1", "url": "https://a.example/", "urlFragment": "#x"}}),
        );
        assert_eq!(
            main,
            Some(ViewEvent::Url {
                url: "https://a.example/#x".into()
            })
        );
        // A child frame's navigation is not the page's address.
        let child = page_event(
            &mut s,
            "Page.frameNavigated",
            &json!({"frame": {"id": "F2", "parentId": "F1", "url": "https://b.example/"}}),
        );
        assert_eq!(child, None);
        let same = page_event(
            &mut s,
            "Page.navigatedWithinDocument",
            &json!({"frameId": "F1", "url": "https://a.example/p2"}),
        );
        assert_eq!(
            same,
            Some(ViewEvent::SameDocument {
                url: "https://a.example/p2".into()
            })
        );
        let other = page_event(
            &mut s,
            "Page.navigatedWithinDocument",
            &json!({"frameId": "F2", "url": "https://b.example/#y"}),
        );
        assert_eq!(other, None);
        let call = page_event(
            &mut s,
            "Runtime.bindingCalled",
            &json!({"name": "__flarePost", "payload": "{\"t\":1}"}),
        );
        assert_eq!(
            call,
            Some(ViewEvent::Binding {
                payload: "{\"t\":1}".into(),
                url: "https://a.example/".into()
            })
        );
        let stranger = page_event(
            &mut s,
            "Runtime.bindingCalled",
            &json!({"name": "other", "payload": "x"}),
        );
        assert_eq!(stranger, None);
        // Before the view is armed (its blank page), nothing is reported.
        s.armed = false;
        let blank = page_event(
            &mut s,
            "Page.frameNavigated",
            &json!({"frame": {"id": "F1", "url": "https://a.example/"}}),
        );
        assert_eq!(blank, None);
    }
}
