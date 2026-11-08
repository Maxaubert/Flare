//! The AppKit and WebKit side of the spike: a windowless-on-screen NSApplication, a host window that is
//! never ordered in, the WKWebViews, one delegate object for every WebKit protocol, a run-loop pump, and
//! helpers that turn async WebKit calls into answers (the spike may wait; production code may not, rule 34).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapImageFileType,
    NSBitmapImageRep, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSImage, NSWindow,
    NSWindowOcclusionState, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGEvent, CGEventField, CGScrollEventUnit};
use objc2_foundation::{
    NSArray, NSData, NSDate, NSDefaultRunLoopMode, NSDictionary, NSError, NSHTTPURLResponse,
    NSNotFound, NSNumber, NSPoint, NSProcessInfo, NSRange, NSRect, NSSize, NSString, NSURL,
    NSURLResponse,
};
use objc2_web_kit::{
    WKContentWorld, WKDownload, WKDownloadDelegate, WKFrameInfo, WKMediaCaptureType, WKNavigation,
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationResponse,
    WKNavigationResponsePolicy, WKOpenPanelParameters, WKPermissionDecision, WKScriptMessage,
    WKScriptMessageHandler, WKSecurityOrigin, WKSnapshotConfiguration, WKUIDelegate,
    WKURLSchemeHandler, WKURLSchemeTask, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};

/// The tag the agent's own synthesized CGEvents carry in `kCGEventSourceUserData`.
pub const AGENT_TAG: i64 = 0x46_4C41_5245; // "FLARE"

/// An answer a completion handler fills in later.
type Slot<T> = Rc<RefCell<Option<Result<T, String>>>>;
/// A download's held destination question and the name the browser proposed.
pub type HeldDownload = (RcBlock<dyn Fn(*mut NSURL)>, String);

// ---- shared state the delegates write into ---------------------------------------------------------

#[derive(Default)]
pub struct St {
    pub nav: Vec<String>,
    pub finished: u32,
    pub failed: u32,
    pub status: Vec<i64>,
    pub probes: Vec<(String, String, Retained<WKFrameInfo>)>,
    pub messages: Vec<String>,
    pub dialogs: Vec<String>,
    pub open_panel: Vec<String>,
    pub open_panel_file: Option<PathBuf>,
    pub new_windows: Vec<String>,
    pub refused: Vec<String>,
    pub terminated: u32,
    pub media: Vec<String>,
    pub downloads: Vec<Retained<WKDownload>>,
    pub dl_events: Vec<String>,
    pub dl_pending: Option<HeldDownload>,
    pub scheme_hits: Vec<String>,
    pub app_root: PathBuf,
    /// The app view's address: its navigations may not leave the app scheme.
    pub app_view: usize,
    pub monitor: Vec<String>,
    pub page_keys_swallowed: u32,
    /// `webViewDidClose:` calls (the page ran `window.close()`).
    pub closed: u32,
    /// Every subframe navigation's target frame, as the navigation delegate saw it: a `WKFrameInfo` for
    /// frames the agent's helper cannot run in (a sandbox without allow-scripts).
    pub sub_frames: Vec<(String, Retained<WKFrameInfo>)>,
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St::default());
    pub static FAKE_KEY: Cell<bool> = const { Cell::new(true) };
    pub static FAKE_VISIBLE: Cell<bool> = const { Cell::new(true) };
}

pub fn st<R>(f: impl FnOnce(&mut St) -> R) -> R {
    ST.with(|s| f(&mut s.borrow_mut()))
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn url_str(u: Option<Retained<NSURL>>) -> String {
    u.and_then(|u| u.absoluteString())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

// ---- the host window -----------------------------------------------------------------------------

define_class!(
    // SAFETY: NSWindow may be subclassed; no Drop, no ivars.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "FlareSpikeWindow"]
    pub struct SpikeWindow;

    impl SpikeWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        // WebKit treats the page as focused only in the key window: the harness says so while the
        // app itself is never activated (it cannot be: activation policy Prohibited).
        #[unsafe(method(isKeyWindow))]
        fn is_key(&self) -> bool {
            FAKE_KEY.with(Cell::get)
        }

        // WebKit stops painting a page whose window is not visible; the window is never ordered in
        // (nothing reaches the screen), so it says it is.
        #[unsafe(method(isVisible))]
        fn is_visible(&self) -> bool {
            FAKE_VISIBLE.with(Cell::get)
        }

        #[unsafe(method(occlusionState))]
        fn occlusion(&self) -> NSWindowOcclusionState {
            if FAKE_VISIBLE.with(Cell::get) {
                NSWindowOcclusionState::Visible
            } else {
                NSWindowOcclusionState::empty()
            }
        }
    }
);

pub fn app(mtm: MainThreadMarker) -> Retained<NSApplication> {
    let app = NSApplication::sharedApplication(mtm);
    // Prohibited: no Dock icon, no menu bar, and the app can never become active or take the screen.
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    app.finishLaunching();
    app
}

pub fn window(mtm: MainThreadMarker, w: f64, h: f64) -> Retained<SpikeWindow> {
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h));
    let win: Retained<SpikeWindow> = unsafe {
        msg_send![SpikeWindow::alloc(mtm), initWithContentRect: rect, styleMask: NSWindowStyleMask::Borderless, backing: NSBackingStoreType::Buffered, defer: false]
    };
    unsafe { win.setReleasedWhenClosed(false) };
    // Never ordered in: no orderFront, no makeKeyAndOrderFront. Nothing appears on screen.
    win
}

// ---- the run loop ------------------------------------------------------------------------------

pub fn pump(app: &NSApplication, ms: f64) {
    let date = NSDate::dateWithTimeIntervalSinceNow(ms / 1000.0);
    let ev = unsafe {
        app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&date),
            NSDefaultRunLoopMode,
            true,
        )
    };
    if let Some(ev) = ev {
        app.sendEvent(&ev);
    }
}

pub fn spin(app: &NSApplication, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        pump(app, 5.0);
    }
}

pub fn wait_until(app: &NSApplication, ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        if cond() {
            return true;
        }
        pump(app, 5.0);
    }
    cond()
}

// ---- the delegate of everything --------------------------------------------------------------------

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "FlareSpikeSpy"]
    pub struct Spy;

    unsafe impl NSObjectProtocol for Spy {}

    unsafe impl WKNavigationDelegate for Spy {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_action(
            &self,
            wv: &WKWebView,
            action: &WKNavigationAction,
            handler: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let req = unsafe { action.request() };
            let url = req.URL();
            let scheme = url
                .as_ref()
                .and_then(|u| u.scheme())
                .map(|s| s.to_string().to_ascii_lowercase())
                .unwrap_or_default();
            let full = url_str(url);
            if let Some(t) = unsafe { action.targetFrame() }
                && !unsafe { t.isMainFrame() }
            {
                st(|s| s.sub_frames.push((full.clone(), t)));
            }
            let is_app_view = st(|s| s.app_view) == (wv as *const WKWebView as usize);
            if unsafe { action.shouldPerformDownload() } {
                st(|s| s.nav.push(format!("download-action {full}")));
                handler.call((WKNavigationActionPolicy::Download,));
                return;
            }
            let allowed = if is_app_view {
                scheme == "flare-app" || full == "about:blank" || full == "about:srcdoc"
            } else {
                matches!(
                    scheme.as_str(),
                    "http" | "https" | "about" | "blob" | "data"
                )
            };
            if allowed {
                handler.call((WKNavigationActionPolicy::Allow,));
            } else {
                st(|s| s.refused.push(full));
                handler.call((WKNavigationActionPolicy::Cancel,));
            }
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(
            &self,
            _wv: &WKWebView,
            resp: &WKNavigationResponse,
            handler: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            let r = unsafe { resp.response() };
            let mut attachment = false;
            if let Ok(h) = r.downcast::<NSHTTPURLResponse>() {
                let code = h.statusCode() as i64;
                if unsafe { resp.isForMainFrame() } {
                    st(|s| s.status.push(code));
                }
                if let Some(cd) = h.valueForHTTPHeaderField(&ns("Content-Disposition")) {
                    attachment = cd
                        .to_string()
                        .to_ascii_lowercase()
                        .starts_with("attachment");
                }
            }
            if attachment || !unsafe { resp.canShowMIMEType() } {
                handler.call((WKNavigationResponsePolicy::Download,));
            } else {
                handler.call((WKNavigationResponsePolicy::Allow,));
            }
        }

        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn did_start(&self, wv: &WKWebView, _n: Option<&WKNavigation>) {
            let u = url_str(unsafe { wv.URL() });
            st(|s| s.nav.push(format!("start {u}")));
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn did_commit(&self, wv: &WKWebView, _n: Option<&WKNavigation>) {
            let u = url_str(unsafe { wv.URL() });
            st(|s| s.nav.push(format!("commit {u}")));
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, wv: &WKWebView, _n: Option<&WKNavigation>) {
            let u = url_str(unsafe { wv.URL() });
            st(|s| {
                s.finished += 1;
                s.nav.push(format!("finish {u}"));
            });
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_prov(&self, _wv: &WKWebView, _n: Option<&WKNavigation>, e: &NSError) {
            let d = format!("{} {}", e.code(), e.localizedDescription());
            st(|s| {
                s.failed += 1;
                s.nav.push(format!("fail-provisional {d}"));
            });
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _wv: &WKWebView, _n: Option<&WKNavigation>, e: &NSError) {
            let d = format!("{} {}", e.code(), e.localizedDescription());
            st(|s| {
                s.failed += 1;
                s.nav.push(format!("fail {d}"));
            });
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn terminated(&self, _wv: &WKWebView) {
            st(|s| s.terminated += 1);
        }

        #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
        fn action_download(&self, _wv: &WKWebView, _a: &WKNavigationAction, d: &WKDownload) {
            unsafe { d.setDelegate(Some(ProtocolObject::from_ref(self))) };
            st(|s| {
                s.dl_events.push("became download (action)".into());
                s.downloads.push(d.retain());
            });
        }

        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        fn response_download(&self, _wv: &WKWebView, _r: &WKNavigationResponse, d: &WKDownload) {
            unsafe { d.setDelegate(Some(ProtocolObject::from_ref(self))) };
            st(|s| {
                s.dl_events.push("became download (response)".into());
                s.downloads.push(d.retain());
            });
        }
    }

    unsafe impl WKUIDelegate for Spy {
        #[unsafe(method(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            wv: &WKWebView,
            _c: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _f: &WKWindowFeatures,
        ) -> *mut WKWebView {
            let req = unsafe { action.request() };
            st(|s| s.new_windows.push(url_str(req.URL())));
            // No window of its own: the page goes to the same view, as on Windows.
            unsafe { wv.loadRequest(&req) };
            std::ptr::null_mut()
        }

        #[unsafe(method(webViewDidClose:))]
        fn did_close(&self, _wv: &WKWebView) {
            // As on Windows (WindowCloseRequested): noted, the view stays.
            st(|s| s.closed += 1);
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn alert(
            &self,
            _wv: &WKWebView,
            m: &NSString,
            _f: &WKFrameInfo,
            done: &DynBlock<dyn Fn()>,
        ) {
            st(|s| s.dialogs.push(format!("alert: {m}")));
            done.call(());
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn confirm(
            &self,
            _wv: &WKWebView,
            m: &NSString,
            _f: &WKFrameInfo,
            done: &DynBlock<dyn Fn(Bool)>,
        ) {
            st(|s| s.dialogs.push(format!("confirm: {m}")));
            done.call((Bool::NO,));
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn prompt(
            &self,
            _wv: &WKWebView,
            m: &NSString,
            d: Option<&NSString>,
            _f: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSString)>,
        ) {
            let def = d.map(|d| d.to_string()).unwrap_or_default();
            st(|s| s.dialogs.push(format!("prompt: {m} (default {def})")));
            let answer = ns("answered by the host");
            done.call((Retained::as_ptr(&answer).cast_mut(),));
        }

        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn media(
            &self,
            _wv: &WKWebView,
            o: &WKSecurityOrigin,
            _f: &WKFrameInfo,
            t: WKMediaCaptureType,
            done: &DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            let host = unsafe { o.host() }.to_string();
            st(|s| {
                s.media
                    .push(format!("media type {} from {host}: denied", t.0))
            });
            done.call((WKPermissionDecision::Deny,));
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn open_panel(
            &self,
            _wv: &WKWebView,
            p: &WKOpenPanelParameters,
            _f: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
        ) {
            let multi = unsafe { p.allowsMultipleSelection() };
            let file = st(|s| {
                s.open_panel.push(format!("open panel, multiple {multi}"));
                s.open_panel_file.clone()
            });
            match file {
                Some(f) => {
                    let u = NSURL::fileURLWithPath(&ns(&f.to_string_lossy()));
                    let arr = NSArray::from_retained_slice(&[u]);
                    done.call((Retained::as_ptr(&arr).cast_mut(),));
                }
                None => done.call((std::ptr::null_mut(),)),
            }
        }
    }

    unsafe impl WKDownloadDelegate for Spy {
        #[unsafe(method(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:))]
        fn decide_dest(
            &self,
            _d: &WKDownload,
            _r: &NSURLResponse,
            name: &NSString,
            done: &DynBlock<dyn Fn(*mut NSURL)>,
        ) {
            // Held, as on Windows: nothing is saved until the person (here the test) answers.
            let n = name.to_string();
            st(|s| {
                s.dl_events
                    .push(format!("decide destination, suggested {n}"));
                s.dl_pending = Some((done.copy(), n));
            });
        }

        #[unsafe(method(downloadDidFinish:))]
        fn dl_finish(&self, _d: &WKDownload) {
            st(|s| s.dl_events.push("finished".into()));
        }

        #[unsafe(method(download:didFailWithError:resumeData:))]
        fn dl_fail(&self, _d: &WKDownload, e: &NSError, _r: Option<&NSData>) {
            let d = format!("{} {}", e.code(), e.localizedDescription());
            st(|s| s.dl_events.push(format!("failed {d}")));
        }
    }

    unsafe impl WKScriptMessageHandler for Spy {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn message(&self, _c: &WKUserContentController, m: &WKScriptMessage) {
            let name = unsafe { m.name() }.to_string();
            let body = unsafe { m.body() };
            let text = body
                .downcast_ref::<NSString>()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "<not a string>".into());
            if name == "flareAgent" {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
                    && v["kind"] == "probe"
                {
                    let frame = unsafe { m.frameInfo() };
                    let nonce = v["nonce"].as_str().unwrap_or("").to_string();
                    let key = v["key"].as_str().unwrap_or("").to_string();
                    st(|s| s.probes.push((nonce, key, frame)));
                }
            } else {
                st(|s| s.messages.push(format!("{name}: {text}")));
            }
        }
    }

    unsafe impl WKURLSchemeHandler for Spy {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start_task(&self, _wv: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            let req = unsafe { task.request() };
            let Some(url) = req.URL() else { return };
            let host = url.host().map(|h| h.to_string()).unwrap_or_default();
            let path = url.path().map(|p| p.to_string()).unwrap_or_default();
            let full = url_str(Some(url.clone()));
            let root = st(|s| {
                s.scheme_hits.push(full.clone());
                s.app_root.clone()
            });
            // One app per host; a path is served only from inside the app's folder.
            let rel = path.trim_start_matches('/');
            let bad = host != "app"
                || rel
                    .split('/')
                    .any(|c| c == ".." || c.is_empty() && !rel.is_empty());
            let file = root.join(if rel.is_empty() { "index.html" } else { rel });
            let body = if bad { None } else { std::fs::read(&file).ok() };
            match body {
                Some(bytes) => {
                    let mime = match file.extension().and_then(|e| e.to_str()) {
                        Some("html") => "text/html",
                        Some("json") => "application/json",
                        Some("png") => "image/png",
                        _ => "application/octet-stream",
                    };
                    let headers = NSDictionary::from_retained_objects(
                        &[&*ns("Content-Type"), &*ns("Content-Length")],
                        &[ns(mime), ns(&bytes.len().to_string())],
                    );
                    let resp = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
                        NSHTTPURLResponse::alloc(),
                        &url,
                        200,
                        Some(&ns("HTTP/1.1")),
                        Some(&headers),
                    );
                    if let Some(resp) = resp {
                        unsafe {
                            task.didReceiveResponse(&resp);
                            task.didReceiveData(&NSData::with_bytes(&bytes));
                            task.didFinish();
                        }
                    }
                }
                None => {
                    let err = unsafe {
                        NSError::errorWithDomain_code_userInfo(&ns("FlareSpike"), 404, None)
                    };
                    unsafe { task.didFailWithError(&err) };
                }
            }
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop_task(&self, _wv: &WKWebView, _task: &ProtocolObject<dyn WKURLSchemeTask>) {}
    }
);

impl Spy {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Spy::alloc(mtm), init] }
    }
}

// ---- web views -----------------------------------------------------------------------------------

pub fn agent_world(mtm: MainThreadMarker) -> Retained<WKContentWorld> {
    unsafe { WKContentWorld::worldWithName(&ns("flare-agent"), mtm) }
}

pub fn user_script(
    mtm: MainThreadMarker,
    src: &str,
    main_only: bool,
    world: &WKContentWorld,
) -> Retained<WKUserScript> {
    unsafe {
        WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
            WKUserScript::alloc(mtm),
            &ns(src),
            WKUserScriptInjectionTime::AtDocumentStart,
            main_only,
            world,
        )
    }
}

pub fn make_view(
    mtm: MainThreadMarker,
    cfg: &WKWebViewConfiguration,
    w: f64,
    h: f64,
) -> Retained<WKWebView> {
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h));
    unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, cfg) }
}

pub fn load(wv: &WKWebView, url: &str) {
    let u = NSURL::URLWithString(&ns(url)).expect("url");
    let req = objc2_foundation::NSURLRequest::requestWithURL(&u);
    unsafe { wv.loadRequest(&req) };
}

/// Loads and waits for the next finished (or failed) navigation.
pub fn load_wait(app: &NSApplication, wv: &WKWebView, url: &str) -> bool {
    let before = st(|s| s.finished + s.failed);
    load(wv, url);
    wait_until(app, 10_000, || st(|s| s.finished + s.failed) > before)
}

fn ns_error_text(e: &NSError) -> String {
    let info = e.userInfo();
    let msg = info
        .objectForKey(&ns("WKJavaScriptExceptionMessage"))
        .and_then(|o| o.downcast::<NSString>().ok())
        .map(|s| s.to_string());
    msg.unwrap_or_else(|| format!("{} {}", e.code(), e.localizedDescription()))
}

/// What WebKit made of a script's value (NSString, NSNumber, NSNull, NSArray, NSDictionary, NSDate), as
/// JSON. The conversion is WebKit's own, not the page's JSON.stringify, so a page cannot change it.
pub fn to_json(obj: *mut AnyObject) -> serde_json::Value {
    use serde_json::Value;
    if obj.is_null() {
        return Value::Null;
    }
    let o: &AnyObject = unsafe { &*obj };
    if let Some(s) = o.downcast_ref::<NSString>() {
        return Value::String(s.to_string());
    }
    if let Some(n) = o.downcast_ref::<NSNumber>() {
        let t = unsafe { std::ffi::CStr::from_ptr(n.objCType().as_ptr()) };
        if t.to_bytes() == b"c" || t.to_bytes() == b"B" {
            return Value::Bool(n.boolValue());
        }
        let f = n.doubleValue();
        if f.fract() == 0.0 && f.abs() < 9.0e15 {
            return Value::from(f as i64);
        }
        return serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number);
    }
    if o.downcast_ref::<objc2_foundation::NSNull>().is_some() {
        return Value::Null;
    }
    if let Some(a) = o.downcast_ref::<NSArray>() {
        return Value::Array(
            a.iter()
                .map(|x| to_json(Retained::as_ptr(&x).cast_mut()))
                .collect(),
        );
    }
    if let Some(d) = o.downcast_ref::<NSDictionary>() {
        let mut m = serde_json::Map::new();
        for k in d.allKeys() {
            let key: Retained<NSString> = unsafe { msg_send![&*k, description] };
            let v = d.objectForKey(&k);
            m.insert(
                key.to_string(),
                v.map_or(Value::Null, |v| to_json(Retained::as_ptr(&v).cast_mut())),
            );
        }
        return Value::Object(m);
    }
    let d: Retained<NSString> = unsafe { msg_send![o, description] };
    Value::String(d.to_string())
}

/// Runs a script in a frame and world and waits for its answer (a string, number, or the error).
pub fn eval(
    app: &NSApplication,
    wv: &WKWebView,
    frame: Option<&WKFrameInfo>,
    world: &WKContentWorld,
    js: &str,
    ms: u64,
) -> Result<serde_json::Value, String> {
    let slot: Slot<serde_json::Value> = Rc::default();
    let s2 = Rc::clone(&slot);
    let block = RcBlock::new(move |v: *mut AnyObject, e: *mut NSError| {
        let r = if e.is_null() {
            Ok(to_json(v))
        } else {
            Err(ns_error_text(unsafe { &*e }))
        };
        *s2.borrow_mut() = Some(r);
    });
    unsafe {
        wv.evaluateJavaScript_inFrame_inContentWorld_completionHandler(
            &ns(js),
            frame,
            world,
            Some(&block),
        )
    };
    if !wait_until(app, ms, || slot.borrow().is_some()) {
        return Err(format!("no answer in {ms} ms"));
    }
    slot.borrow_mut().take().unwrap_or(Err("lost".into()))
}

/// `callAsyncJavaScript`: the body may `await`; its return value is the answer.
pub fn call_async(
    app: &NSApplication,
    wv: &WKWebView,
    frame: Option<&WKFrameInfo>,
    world: &WKContentWorld,
    body: &str,
    ms: u64,
) -> Result<serde_json::Value, String> {
    let slot: Slot<serde_json::Value> = Rc::default();
    let s2 = Rc::clone(&slot);
    let block = RcBlock::new(move |v: *mut AnyObject, e: *mut NSError| {
        let r = if e.is_null() {
            Ok(to_json(v))
        } else {
            Err(ns_error_text(unsafe { &*e }))
        };
        *s2.borrow_mut() = Some(r);
    });
    unsafe {
        wv.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &ns(body),
            None,
            frame,
            world,
            Some(&block),
        )
    };
    if !wait_until(app, ms, || slot.borrow().is_some()) {
        return Err(format!("no answer in {ms} ms"));
    }
    slot.borrow_mut().take().unwrap_or(Err("lost".into()))
}

// ---- pictures ------------------------------------------------------------------------------------

/// A still of `rect` (view points; None = the whole view), `width` points wide (None = as is), as PNG.
pub fn snapshot_png(
    app: &NSApplication,
    wv: &WKWebView,
    rect: Option<CGRect>,
    width: Option<f64>,
) -> Result<(Vec<u8>, usize, usize), String> {
    let mtm = MainThreadMarker::from(app);
    let cfg = unsafe { WKSnapshotConfiguration::new(mtm) };
    if let Some(r) = rect {
        unsafe { cfg.setRect(r) };
    }
    if let Some(w) = width {
        unsafe { cfg.setSnapshotWidth(Some(&NSNumber::new_f64(w))) };
    }
    let slot: Slot<Retained<NSImage>> = Rc::default();
    let s2 = Rc::clone(&slot);
    let block = RcBlock::new(move |img: *mut NSImage, e: *mut NSError| {
        let r = if img.is_null() {
            Err(if e.is_null() {
                "no image".to_string()
            } else {
                ns_error_text(unsafe { &*e })
            })
        } else {
            Ok(unsafe { Retained::retain(img) }.expect("image"))
        };
        *s2.borrow_mut() = Some(r);
    });
    unsafe { wv.takeSnapshotWithConfiguration_completionHandler(Some(&cfg), &block) };
    if !wait_until(app, 8_000, || slot.borrow().is_some()) {
        return Err("takeSnapshot never answered".into());
    }
    let img = slot.borrow_mut().take().unwrap_or(Err("lost".into()))?;
    let cg = unsafe { img.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
        .ok_or("no CGImage")?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    let (pw, ph) = (rep.pixelsWide() as usize, rep.pixelsHigh() as usize);
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or("no PNG")?;
    Ok((data.to_vec(), pw, ph))
}

// ---- synthesized input -------------------------------------------------------------------------------

fn now() -> f64 {
    NSProcessInfo::processInfo().systemUptime()
}

/// A CSS point of the view to the window's own coordinates (bottom left origin).
pub fn to_window(wv: &WKWebView, x: f64, y: f64) -> NSPoint {
    let zoom = unsafe { wv.pageZoom() };
    wv.convertPoint_toView(NSPoint::new(x * zoom, y * zoom), None)
}

/// A mouse event built as a CGEvent (so a middle button has button number 2 and the event carries the
/// agent's tag), placed in the host window. The window sits at the screen's origin, so its point is the
/// screen point; CG counts y from the top.
pub fn mouse_event(
    wv: &WKWebView,
    kind: NSEventType,
    x: f64,
    y: f64,
    clicks: isize,
    flags: NSEventModifierFlags,
) -> Retained<NSEvent> {
    use objc2_core_graphics::{CGEventFlags, CGEventType, CGMouseButton};
    let win = wv.window().expect("window");
    let p = to_window(wv, x, y);
    let screen_h = objc2_app_kit::NSScreen::mainScreen(MainThreadMarker::from(wv))
        .map(|s| s.frame().size.height)
        .unwrap_or(1000.0);
    let (t, b) = match kind {
        NSEventType::LeftMouseDown => (CGEventType::LeftMouseDown, CGMouseButton::Left),
        NSEventType::LeftMouseUp => (CGEventType::LeftMouseUp, CGMouseButton::Left),
        NSEventType::RightMouseDown => (CGEventType::RightMouseDown, CGMouseButton::Right),
        NSEventType::RightMouseUp => (CGEventType::RightMouseUp, CGMouseButton::Right),
        NSEventType::OtherMouseDown => (CGEventType::OtherMouseDown, CGMouseButton::Center),
        NSEventType::OtherMouseUp => (CGEventType::OtherMouseUp, CGMouseButton::Center),
        _ => (CGEventType::MouseMoved, CGMouseButton::Left),
    };
    let cg =
        CGEvent::new_mouse_event(None, t, CGPoint::new(p.x, screen_h - p.y), b).expect("cg mouse");
    CGEvent::set_integer_value_field(Some(&cg), CGEventField::MouseEventClickState, clicks as i64);
    CGEvent::set_integer_value_field(Some(&cg), CGEventField::EventSourceUserData, AGENT_TAG);
    let wn = win.windowNumber() as i64;
    CGEvent::set_integer_value_field(
        Some(&cg),
        CGEventField::MouseEventWindowUnderMousePointer,
        wn,
    );
    CGEvent::set_integer_value_field(
        Some(&cg),
        CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
        wn,
    );
    CGEvent::set_flags(Some(&cg), CGEventFlags(flags.bits() as u64));
    NSEvent::eventWithCGEvent(&cg).expect("ns mouse")
}

/// Sends a mouse event straight to the view: it never passes through NSApp's queue (so the user-input
/// monitor never sees it) and never activates the app or orders the window in.
pub fn deliver_mouse(wv: &WKWebView, ev: &NSEvent) {
    match ev.r#type() {
        NSEventType::LeftMouseDown => wv.mouseDown(ev),
        NSEventType::LeftMouseUp => wv.mouseUp(ev),
        NSEventType::RightMouseDown => wv.rightMouseDown(ev),
        NSEventType::RightMouseUp => wv.rightMouseUp(ev),
        NSEventType::OtherMouseDown => wv.otherMouseDown(ev),
        NSEventType::OtherMouseUp => wv.otherMouseUp(ev),
        NSEventType::MouseMoved => wv.mouseMoved(ev),
        NSEventType::LeftMouseDragged => wv.mouseDragged(ev),
        _ => {}
    }
}

/// A pixel wheel scroll at a CSS point (down and right positive, as CDP's deltaY). The host window sits
/// at the screen's origin, so the screen point is the window point.
pub fn wheel_event(wv: &WKWebView, x: f64, y: f64, dx: f64, dy: f64) -> Retained<NSEvent> {
    let p = to_window(wv, x, y);
    let screen_h = objc2_app_kit::NSScreen::mainScreen(MainThreadMarker::from(wv))
        .map(|s| s.frame().size.height)
        .unwrap_or(1000.0);
    let cg = CGEvent::new_scroll_wheel_event2(
        None,
        CGScrollEventUnit::Pixel,
        2,
        (-dy).round() as i32,
        (-dx).round() as i32,
        0,
    )
    .expect("wheel");
    CGEvent::set_location(Some(&cg), CGPoint::new(p.x, screen_h - p.y));
    CGEvent::set_integer_value_field(Some(&cg), CGEventField::EventSourceUserData, AGENT_TAG);
    NSEvent::eventWithCGEvent(&cg).expect("ns wheel")
}

pub fn key_event(
    wv: &WKWebView,
    down: bool,
    chars: &str,
    unmod: &str,
    code: u16,
    flags: NSEventModifierFlags,
) -> Retained<NSEvent> {
    let win = wv.window().expect("window");
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        if down { NSEventType::KeyDown } else { NSEventType::KeyUp },
        NSPoint::new(0.0, 0.0),
        flags,
        now(),
        win.windowNumber(),
        None,
        &ns(chars),
        &ns(unmod),
        false,
        code,
    )
    .expect("key event")
}

/// A mouse event as the window server would deliver the person's own: no agent tag.
pub fn user_mouse_event(wv: &WKWebView, kind: NSEventType, x: f64, y: f64) -> Retained<NSEvent> {
    let win = wv.window().expect("window");
    NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind,
        to_window(wv, x, y),
        NSEventModifierFlags::empty(),
        now(),
        win.windowNumber(),
        None,
        0,
        1,
        1.0,
    )
    .expect("mouse event")
}

/// A key event built as a CGEvent with the agent's tag, for the route through NSApp's queue.
pub fn tagged_key_event(code: u16, down: bool, text: &str, tag: bool) -> Retained<NSEvent> {
    let cg = CGEvent::new_keyboard_event(None, code, down).expect("cg key");
    let u: Vec<u16> = text.encode_utf16().collect();
    unsafe { CGEvent::keyboard_set_unicode_string(Some(&cg), u.len() as _, u.as_ptr()) };
    if tag {
        CGEvent::set_integer_value_field(Some(&cg), CGEventField::EventSourceUserData, AGENT_TAG);
    }
    NSEvent::eventWithCGEvent(&cg).expect("ns key")
}

pub fn tag_of(ev: &NSEvent) -> i64 {
    match ev.CGEvent() {
        Some(cg) => CGEvent::integer_value_field(Some(&cg), CGEventField::EventSourceUserData),
        None => 0,
    }
}

thread_local! {
    /// The agent's last key events: WebKit hands a key the page did not take back to AppKit
    /// (`[NSApp sendEvent:]`), where the monitor sees it again; the same event object is recognised.
    static AGENT_KEYS: RefCell<Vec<Retained<NSEvent>>> = const { RefCell::new(Vec::new()) };
}

pub fn is_agent_event(ev: &NSEvent) -> bool {
    tag_of(ev) == AGENT_TAG
        || AGENT_KEYS.with(|k| {
            k.borrow().iter().any(|a| {
                std::ptr::eq(&**a, ev)
                    || (a.timestamp() == ev.timestamp() && a.keyCode() == ev.keyCode())
            })
        })
}

pub fn deliver_key(wv: &WKWebView, ev: &NSEvent) {
    AGENT_KEYS.with(|k| {
        let mut k = k.borrow_mut();
        if k.len() > 64 {
            k.remove(0);
        }
        k.push(ev.retain());
    });
    match ev.r#type() {
        NSEventType::KeyDown => {
            if ev.modifierFlags().contains(NSEventModifierFlags::Command)
                && wv.performKeyEquivalent(ev)
            {
                return;
            }
            wv.keyDown(ev)
        }
        NSEventType::KeyUp => wv.keyUp(ev),
        _ => {}
    }
}

/// Text as the input method commits it (`Input.insertText` on Windows).
pub fn insert_text(wv: &WKWebView, text: &str) {
    let s = ns(text);
    let range = NSRange::new(NSNotFound as usize, 0);
    let _: () = unsafe { msg_send![wv, insertText: &*s, replacementRange: range] };
}

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

pub fn url_of(wv: &WKWebView) -> String {
    url_str(unsafe { wv.URL() })
}

pub fn title_of(wv: &WKWebView) -> String {
    unsafe { wv.title() }
        .map(|t| t.to_string())
        .unwrap_or_default()
}

// ---- more helpers for the scenarios ------------------------------------------------------------------

/// How many of this process's windows the window server has on screen (the screen rule's evidence).
pub fn onscreen_windows_of_self() -> usize {
    use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
    let Some(arr) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0) else {
        return 0;
    };
    let ptr = objc2_core_foundation::CFRetained::as_ptr(&arr).as_ptr();
    // CFArray and NSArray are toll-free bridged.
    let list: &NSArray = unsafe { &*(ptr.cast::<NSArray>()) };
    let me = i64::from(std::process::id());
    list.iter()
        .filter(|d| {
            d.downcast_ref::<NSDictionary>()
                .and_then(|d| d.objectForKey(&ns("kCGWindowOwnerPID")))
                .and_then(|p| p.downcast::<NSNumber>().ok())
                .is_some_and(|p| p.longLongValue() == me)
        })
        .count()
}

thread_local! {
    /// The monitor swallows mouse events (they would activate a real app) and Command+T (the app's chord).
    pub static MONITOR_ON: Cell<bool> = const { Cell::new(false) };
}

/// The user-input detector: a local monitor sees every event NSApp dispatches (the person's own, from
/// the window server) and never the agent's, which go straight to the view.
pub fn install_monitor() -> Retained<AnyObject> {
    let block = RcBlock::new(|ev: NonNull<NSEvent>| -> *mut NSEvent {
        let e: &NSEvent = unsafe { ev.as_ref() };
        if !MONITOR_ON.with(Cell::get) {
            return ev.as_ptr();
        }
        let kind = e.r#type();
        let who = if is_agent_event(e) { "agent" } else { "user" };
        let detail = match kind {
            NSEventType::KeyDown | NSEventType::KeyUp => format!(
                "key {:?} code {} chars {:?} cmd {}",
                kind.0,
                e.keyCode(),
                e.characters().map(|c| c.to_string()).unwrap_or_default(),
                e.modifierFlags().contains(NSEventModifierFlags::Command)
            ),
            _ => format!("event type {}", kind.0),
        };
        st(|s| s.monitor.push(format!("{who}: {detail}")));
        let swallow = matches!(
            kind,
            NSEventType::LeftMouseDown | NSEventType::LeftMouseUp | NSEventType::RightMouseDown
        ) || (kind == NSEventType::KeyDown
            && e.modifierFlags().contains(NSEventModifierFlags::Command)
            && e.charactersIgnoringModifiers()
                .is_some_and(|c| c.to_string() == "t"));
        if swallow {
            st(|s| s.page_keys_swallowed += 1);
            std::ptr::null_mut()
        } else {
            ev.as_ptr()
        }
    });
    unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Any, &block) }
        .expect("monitor")
}

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_endpoint_create_host(
        hostname: *const std::ffi::c_char,
        port: *const std::ffi::c_char,
    ) -> *mut AnyObject;
    fn nw_proxy_config_create_http_connect(
        endpoint: *mut AnyObject,
        tls: *mut AnyObject,
    ) -> *mut AnyObject;
}

/// Sends every request of the store through an HTTP CONNECT proxy that is not there (macOS 14+).
pub fn set_dead_proxy(store: &objc2_web_kit::WKWebsiteDataStore, port: u16) {
    let host = std::ffi::CString::new("127.0.0.1").expect("c");
    let p = std::ffi::CString::new(port.to_string()).expect("c");
    unsafe {
        let ep = nw_endpoint_create_host(host.as_ptr(), p.as_ptr());
        let cfg = nw_proxy_config_create_http_connect(ep, std::ptr::null_mut());
        let cfg: Retained<AnyObject> = Retained::from_raw(cfg).expect("proxy config");
        let _ep: Option<Retained<AnyObject>> = Retained::from_raw(ep);
        let arr = NSArray::from_retained_slice(&[cfg]);
        let _: () = msg_send![store, setProxyConfigurations: &*arr];
    }
}

pub fn compile_rules(
    app: &NSApplication,
    id: &str,
    json: &str,
) -> Result<Retained<objc2_web_kit::WKContentRuleList>, String> {
    let mtm = MainThreadMarker::from(app);
    let store = unsafe { objc2_web_kit::WKContentRuleListStore::defaultStore(mtm) }
        .ok_or("no rule store")?;
    let slot: Slot<Retained<objc2_web_kit::WKContentRuleList>> = Rc::default();
    let s2 = Rc::clone(&slot);
    let block = RcBlock::new(
        move |l: *mut objc2_web_kit::WKContentRuleList, e: *mut NSError| {
            let r = if l.is_null() {
                Err(if e.is_null() {
                    "no list".to_string()
                } else {
                    ns_error_text(unsafe { &*e })
                })
            } else {
                Ok(unsafe { Retained::retain(l) }.expect("list"))
            };
            *s2.borrow_mut() = Some(r);
        },
    );
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(&ns(id)),
            Some(&ns(json)),
            Some(&block),
        )
    };
    wait_until(app, 10_000, || slot.borrow().is_some());
    slot.borrow_mut()
        .take()
        .unwrap_or(Err("compile never answered".into()))
}

pub fn remove_rules(app: &NSApplication, id: &str) {
    let mtm = MainThreadMarker::from(app);
    if let Some(store) = unsafe { objc2_web_kit::WKContentRuleListStore::defaultStore(mtm) } {
        let done = Rc::new(Cell::new(false));
        let d2 = Rc::clone(&done);
        let block = RcBlock::new(move |_e: *mut NSError| d2.set(true));
        unsafe {
            store.removeContentRuleListForIdentifier_completionHandler(Some(&ns(id)), Some(&block))
        };
        wait_until(app, 3_000, || done.get());
    }
}

pub fn cookies(app: &NSApplication, store: &objc2_web_kit::WKWebsiteDataStore) -> Vec<String> {
    let slot: Rc<RefCell<Option<Vec<String>>>> = Rc::default();
    let s2 = Rc::clone(&slot);
    let block = RcBlock::new(
        move |arr: NonNull<NSArray<objc2_foundation::NSHTTPCookie>>| {
            let a = unsafe { arr.as_ref() };
            let v = a
                .iter()
                .map(|c| format!("{}={} ({})", c.name(), c.value(), c.domain()))
                .collect();
            *s2.borrow_mut() = Some(v);
        },
    );
    unsafe { store.httpCookieStore().getAllCookies(&block) };
    wait_until(app, 5_000, || slot.borrow().is_some());
    slot.borrow_mut().take().unwrap_or_default()
}

pub fn pdf(app: &NSApplication, wv: &WKWebView) -> Result<Vec<u8>, String> {
    let slot: Slot<Vec<u8>> = Rc::default();
    let s2 = Rc::clone(&slot);
    let block = RcBlock::new(move |d: *mut NSData, e: *mut NSError| {
        let r = if d.is_null() {
            Err(if e.is_null() {
                "no data".into()
            } else {
                ns_error_text(unsafe { &*e })
            })
        } else {
            Ok(unsafe { &*d }.to_vec())
        };
        *s2.borrow_mut() = Some(r);
    });
    unsafe { wv.createPDFWithConfiguration_completionHandler(None, &block) };
    wait_until(app, 10_000, || slot.borrow().is_some());
    slot.borrow_mut()
        .take()
        .unwrap_or(Err("never answered".into()))
}

/// The page's content process id: private SPI (`_webProcessIdentifier`), used here only to kill the
/// process in a test.
pub fn web_pid(wv: &WKWebView) -> i32 {
    unsafe { msg_send![wv, _webProcessIdentifier] }
}

pub fn progress_of(d: &WKDownload) -> (f64, i64, i64) {
    let p: Retained<NSObject> = unsafe { msg_send![d, progress] };
    let f: f64 = unsafe { msg_send![&*p, fractionCompleted] };
    let c: i64 = unsafe { msg_send![&*p, completedUnitCount] };
    let t: i64 = unsafe { msg_send![&*p, totalUnitCount] };
    (f, c, t)
}
