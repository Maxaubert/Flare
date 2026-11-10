//! A Windows toast notification (Surfaces stage 7, questions as notifications).
//!
//! The toast is shown through the WinRT `ToastNotificationManager` under Flare's own AppUserModelID
//! (`names::AUMID`). The installed app has a Start menu shortcut that carries that id (Velopack's
//! `--aumid`, `crates/flare/tests/release_files.rs`), which is what lets an unpackaged desktop app show a toast at
//! all; a build run from a folder has no such shortcut, and Windows then drops the toast without an error. The
//! caller therefore flashes the taskbar button too when the toast could not be asked for (an `Err` here), and
//! says so in its log; nothing here can tell "shown" from "dropped by Windows".
//!
//! A click arrives as the toast's `Activated` event while Flare runs, on a thread-pool thread: `on_click` is
//! called there with the tab id, and the caller moves it to the UI thread. A toast Flare did not show, or one
//! left in the action centre after Flare quit, does nothing when clicked beyond starting the app (no activator is
//! registered, by design: a stale toast must not be able to steer a new session).
//!
//! Everything that touches WinRT runs on a short-lived worker, never on the UI thread (rule 34).
//!
//! The system part sits behind [`NotifyBackend`] (a test hands its own to [`show_async_with`]). macOS (port
//! phase 8, step 2): [`CenterBackend`] over `UNUserNotificationCenter` (`macos/notify.rs`), only when Flare runs
//! from its own signed `Flare.app` ([`bundle_verdict`]; a `cargo run` or a test binary is not a bundle, and the
//! center would abort it). Permission is asked once a run, lazily, the first time Flare has something to say; each
//! tab has one notification (its identifier is the tab's, so a newer one replaces the older); a click hands the tab
//! back as `on_click(tab)`, only for a notification this run posted. When the system cannot take the request the
//! answer says why ([`MAC_NOT_IN_APP`], [`MAC_UNSIGNED`], [`MAC_DENIED`]) and the caller bounces the Dock icon, as
//! it flashes the taskbar when Windows refuses a toast (regression rules 1015 and 1044).

use std::sync::Arc;
#[cfg(windows)]
use std::sync::Mutex;

/// What to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastRequest {
    /// The tab the toast is about; the click hands it back, and a newer toast for the same tab replaces the older.
    pub tab: String,
    pub title: String,
    pub body: String,
}

/// The toast's XML: the generic template with a title and a body line. Both are escaped and cleaned of control
/// characters, so nothing in them can become markup or an action.
pub fn toast_xml(title: &str, body: &str) -> String {
    format!(
        "<toast activationType=\"foreground\" scenario=\"default\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_text(title),
        xml_text(body)
    )
}

/// Text for an XML element: markup characters escaped, every control character made a space.
pub fn xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// The tag a toast of this tab wears: a newer toast with the same tag and group replaces the older one in the action
/// centre, so one tab never stacks up several. At most 64 characters (the tag limit), letters and digits only.
pub fn tag_of(tab: &str) -> String {
    tab.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(48)
        .collect::<String>()
}

/// The toasts shown by this process, kept so their events can still be delivered. Oldest dropped first.
#[cfg(windows)]
static LIVE: Mutex<Vec<windows::UI::Notifications::ToastNotification>> = Mutex::new(Vec::new());
#[cfg(windows)]
const LIVE_MAX: usize = 24;

/// The system's notification call, run on the notification worker. `Ok` means the system took the request
/// (not that it was displayed); `Err` says why not, and the caller falls back to the taskbar or the Dock.
pub trait NotifyBackend: Send + Sync {
    fn show(
        &self,
        aumid: &str,
        req: &ToastRequest,
        on_click: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<(), String>;
}

/// The platform's backend: the WinRT toast on Windows, [`CenterBackend`] over `UNUserNotificationCenter` on macOS.
pub struct SystemBackend;

impl NotifyBackend for SystemBackend {
    fn show(
        &self,
        aumid: &str,
        req: &ToastRequest,
        on_click: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<(), String> {
        show_now(aumid, req, on_click)
    }
}

/// What macOS answers in a build that is not an app bundle (a `cargo run`, a test binary).
pub const MAC_NOT_IN_APP: &str = "notifications are unavailable: this build does not run from Flare.app (a dev \
build run with cargo run); macOS shows notifications only for an app bundle";
/// What macOS answers when the bundle's own signature does not check out.
pub const MAC_UNSIGNED: &str = "notifications are unavailable: Flare.app's signature is not valid (macOS needs a signed Flare.app)";
/// What macOS answers when the person said no, now or earlier.
pub const MAC_DENIED: &str =
    "notifications are turned off for Flare (System Settings, Notifications, Flare)";
/// What macOS answers on a system that has no notification center Flare can use.
pub const OTHER_UNAVAILABLE: &str = "notifications are unavailable on this system";

/// Why the system cannot take a notification request at all: None on Windows (whether a toast is then displayed is
/// Windows' business), and on macOS when Flare runs from its own signed `Flare.app` ([`bundle_verdict`], read once a
/// run). It may block on macOS the first time (the bundle and signature read, up to about a second), so the UI
/// thread asks [`known_unavailable`] instead, after [`warm_up`].
pub fn system_unavailable() -> Option<&'static str> {
    #[cfg(windows)]
    {
        None
    }
    #[cfg(target_os = "macos")]
    {
        *MAC_VERDICT.get_or_init(|| {
            let f = crate::notify::bundle_facts();
            bundle_verdict(f.path.as_deref(), f.id.as_deref(), f.signature)
        })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Some(OTHER_UNAVAILABLE)
    }
}

#[cfg(target_os = "macos")]
static MAC_VERDICT: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();

/// [`system_unavailable`] if it is already known, never blocking: `Some(why)` when the system is known to refuse
/// (the caller falls back at once, no worker per question, rule 1015), None when it may take the request or the
/// answer is not read yet (the worker then reads it and answers through `on_done`).
pub fn known_unavailable() -> Option<&'static str> {
    #[cfg(windows)]
    {
        None
    }
    #[cfg(target_os = "macos")]
    {
        MAC_VERDICT.get().copied().flatten()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Some(OTHER_UNAVAILABLE)
    }
}

/// Reads [`system_unavailable`] on a worker of its own (macOS), so the first question finds it known. Nothing on
/// Windows.
pub fn warm_up() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::thread::Builder::new()
            .name("flare-toast-verdict".into())
            .spawn(|| {
                let _ = system_unavailable();
            });
    }
}

/// Whether a process may use `UNUserNotificationCenter`: it runs from an app bundle (`*.app`) whose id is Flare's
/// (`com.flare.terminal`) and whose signature checks out (`SecCodeCheckValidity`). None: yes; else why not.
pub fn bundle_verdict(
    path: Option<&str>,
    id: Option<&str>,
    signature: Result<(), i32>,
) -> Option<&'static str> {
    let in_app = path.is_some_and(|p| p.trim_end_matches('/').ends_with(".app"));
    if !in_app || id != Some(crate::names::AUMID) {
        return Some(MAC_NOT_IN_APP);
    }
    if signature.is_err() {
        return Some(MAC_UNSIGNED);
    }
    None
}

/// True the first time it is asked, false ever after: a reason worth one log line per run, not one per
/// question.
pub struct FirstTime(std::sync::atomic::AtomicBool);

impl FirstTime {
    pub const fn new() -> FirstTime {
        FirstTime(std::sync::atomic::AtomicBool::new(true))
    }

    /// True once.
    pub fn take(&self) -> bool {
        self.0.swap(false, std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for FirstTime {
    fn default() -> Self {
        FirstTime::new()
    }
}

/// What a notification center says of Flare's permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Never asked: the first notification asks (the system's one prompt).
    NotAsked,
    Denied,
    Allowed,
}

/// One notification as the center gets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posted {
    /// The request's identifier: [`notification_id`] of the tab, so a newer one for the tab replaces the older.
    pub id: String,
    pub title: String,
    pub body: String,
    /// The tab, whole (the identifier keeps only its letters and digits).
    pub tab: String,
    /// The run that posted it ([`CenterBackend`]'s session): a click on an older run's notification steers nothing.
    pub session: String,
}

/// A click on a notification, as the center's delegate read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clicked {
    pub id: String,
    pub session: String,
    pub tab: String,
}

/// Where the center sends a click.
pub type ClickRoute = Arc<dyn Fn(Clicked) + Send + Sync>;

/// The keys of a notification's user info.
pub const KEY_SESSION: &str = "flare-session";
pub const KEY_TAB: &str = "flare-tab";
/// The thread the notifications are grouped under (Windows' toast group).
pub const THREAD: &str = "agent-question";

/// The identifier a tab's notification wears: one per tab, a newer one replaces the older.
pub fn notification_id(tab: &str) -> String {
    format!("{THREAD}.{}", tag_of(tab))
}

/// A system notification center (the real one is `notify::SystemCenter` on macOS; the tests use a fake, since
/// asking for the permission puts a system prompt on the screen). Every call may block on the worker.
pub trait NotificationCenter: Send + Sync {
    /// The permission as it stands. Never prompts.
    fn permission(&self) -> Result<Permission, String>;
    /// The system prompt: true when the person allowed it.
    fn ask_permission(&self) -> Result<bool, String>;
    /// Posts (or replaces) the notification.
    fn post(&self, n: &Posted) -> Result<(), String>;
    /// From now on a click comes to `route`.
    fn route_clicks(&self, route: ClickRoute);
}

/// What a click on a notification calls, with its tab.
pub type OnClick = Arc<dyn Fn(String) + Send + Sync>;

/// The notification path over a center: nothing is asked of it until the first notification, the permission is
/// asked at most once a run, the click route is set with the first post, and a click reaches `on_click` only for a
/// notification of this run.
pub struct CenterBackend<C: NotificationCenter> {
    center: C,
    session: String,
    state: std::sync::Mutex<CenterState>,
    on_click: Arc<std::sync::Mutex<Option<OnClick>>>,
}

#[derive(Default)]
struct CenterState {
    asked: bool,
    routed: bool,
}

impl<C: NotificationCenter> CenterBackend<C> {
    /// Calls nothing on the center.
    pub fn new(center: C) -> CenterBackend<C> {
        CenterBackend {
            center,
            session: uuid::Uuid::new_v4().simple().to_string(),
            state: std::sync::Mutex::new(CenterState::default()),
            on_click: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// This run's session, written in each notification it posts.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The center.
    pub fn center(&self) -> &C {
        &self.center
    }
}

impl<C: NotificationCenter> NotifyBackend for CenterBackend<C> {
    fn show(
        &self,
        _aumid: &str,
        req: &ToastRequest,
        on_click: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<(), String> {
        // One notification at a time: a second question waits while the first one's prompt is up.
        let mut st = self
            .state
            .lock()
            .map_err(|_| "notification state poisoned")?;
        let mut perm = self.center.permission()?;
        if perm == Permission::NotAsked && !st.asked {
            st.asked = true;
            perm = if self.center.ask_permission()? {
                Permission::Allowed
            } else {
                Permission::Denied
            };
        }
        match perm {
            Permission::Allowed => {}
            Permission::Denied => return Err(MAC_DENIED.to_string()),
            Permission::NotAsked => {
                return Err(
                    "notifications were not allowed yet (the prompt was not answered)".into(),
                );
            }
        }
        if let Ok(mut c) = self.on_click.lock() {
            *c = Some(on_click);
        }
        if !st.routed {
            st.routed = true;
            let session = self.session.clone();
            let slot = Arc::clone(&self.on_click);
            self.center.route_clicks(Arc::new(move |c: Clicked| {
                if c.session != session || c.id != notification_id(&c.tab) {
                    return;
                }
                let f = slot.lock().ok().and_then(|f| f.clone());
                if let Some(f) = f {
                    f(c.tab);
                }
            }));
        }
        self.center.post(&Posted {
            id: notification_id(&req.tab),
            title: req.title.clone(),
            body: req.body.clone(),
            tab: req.tab.clone(),
            session: self.session.clone(),
        })
    }
}

/// The one real notification path of this run (macOS).
#[cfg(target_os = "macos")]
fn mac_backend() -> &'static CenterBackend<crate::notify::SystemCenter> {
    static B: std::sync::OnceLock<CenterBackend<crate::notify::SystemCenter>> =
        std::sync::OnceLock::new();
    B.get_or_init(|| CenterBackend::new(crate::notify::SystemCenter))
}

/// Shows the toast on a worker thread. `on_click(tab)` is called when it is clicked; `on_done(result)` once, with
/// whether Windows accepted the request (it says nothing of whether the toast was then displayed).
pub fn show_async(
    aumid: &'static str,
    req: ToastRequest,
    on_click: Arc<dyn Fn(String) + Send + Sync>,
    on_done: Arc<dyn Fn(Result<(), String>) + Send + Sync>,
) {
    show_async_with(Arc::new(SystemBackend), aumid, req, on_click, on_done);
}

/// [`show_async`] through `backend`.
pub fn show_async_with(
    backend: Arc<dyn NotifyBackend>,
    aumid: &'static str,
    req: ToastRequest,
    on_click: Arc<dyn Fn(String) + Send + Sync>,
    on_done: Arc<dyn Fn(Result<(), String>) + Send + Sync>,
) {
    let job = move || {
        let r = backend.show(aumid, &req, on_click);
        on_done(r);
    };
    if let Err(e) = std::thread::Builder::new()
        .name("flare-toast".into())
        .spawn(job)
    {
        log::warn!("could not start the notification worker: {e}");
    }
}

#[cfg(windows)]
fn show_now(
    aumid: &str,
    req: &ToastRequest,
    on_click: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<(), String> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::core::{HSTRING, IInspectable};

    // SAFETY: CoInitializeEx takes no pointers we own; this thread is the worker and is made MTA. A changed-mode
    // answer is fine, COM still works.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let err = |what: &str, e: windows::core::Error| format!("{what}: {e}");
    let doc = XmlDocument::new().map_err(|e| err("xml", e))?;
    doc.LoadXml(&HSTRING::from(toast_xml(&req.title, &req.body)))
        .map_err(|e| err("load", e))?;
    let toast = ToastNotification::CreateToastNotification(&doc).map_err(|e| err("toast", e))?;
    let tag = tag_of(&req.tab);
    if !tag.is_empty() {
        let _ = toast.SetTag(&HSTRING::from(tag.as_str()));
        let _ = toast.SetGroup(&HSTRING::from("agent-question"));
    }
    let tab = req.tab.clone();
    let handler = TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, _| {
        on_click(tab.clone());
        Ok(())
    });
    toast.Activated(&handler).map_err(|e| err("activated", e))?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid))
        .map_err(|e| err("notifier", e))?;
    notifier.Show(&toast).map_err(|e| err("show", e))?;
    if let Ok(mut live) = LIVE.lock() {
        live.push(toast);
        if live.len() > LIVE_MAX {
            live.remove(0);
        }
    }
    Ok(())
}

/// macOS: the user notification center, only from the signed `Flare.app` ([`system_unavailable`]).
#[cfg(target_os = "macos")]
fn show_now(
    aumid: &str,
    req: &ToastRequest,
    on_click: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<(), String> {
    if let Some(why) = system_unavailable() {
        return Err(why.to_string());
    }
    mac_backend().show(aumid, req, on_click)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show_now(
    _aumid: &str,
    _req: &ToastRequest,
    _on_click: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<(), String> {
    Err(OTHER_UNAVAILABLE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_xml_is_the_generic_template_with_two_lines() {
        let x = toast_xml("proj", "Which colour?");
        assert!(x.starts_with("<toast "));
        assert!(x.contains("template=\"ToastGeneric\""));
        assert!(x.contains("<text>proj</text><text>Which colour?</text>"));
        assert!(x.ends_with("</toast>"));
        // No action, no launch argument, no input: a notification can only be clicked.
        for banned in ["<actions", "launch=", "<input", "<image", "protocol"] {
            assert!(!x.contains(banned), "{banned}");
        }
    }

    #[test]
    fn nothing_in_a_title_or_a_body_can_become_markup() {
        let x = toast_xml(
            "</text><actions><action content=\"x\" arguments=\"y\"/></actions><text>",
            "a & b < c > d \" e ' f\u{0}\u{7}\n",
        );
        assert_eq!(x.matches("<text>").count(), 2);
        assert!(!x.contains("<actions"));
        assert!(x.contains("a &amp; b &lt; c &gt; d &quot; e &apos; f   "));
        assert!(x.contains("&lt;/text&gt;&lt;actions&gt;"));
    }

    #[test]
    fn the_tag_is_short_plain_letters_and_digits() {
        assert_eq!(tag_of("t-12_ab"), "t12ab");
        assert_eq!(tag_of(""), "");
        assert!(tag_of(&"a".repeat(200)).len() <= 48);
        assert_eq!(tag_of("a\u{e9}\u{202e}b"), "ab");
    }

    /// A backend that records what it was asked and answers `answer`; `click` presses the toast at once.
    struct Fake {
        asked: std::sync::Mutex<Vec<ToastRequest>>,
        answer: Result<(), String>,
        click: bool,
    }

    impl NotifyBackend for Fake {
        fn show(
            &self,
            _aumid: &str,
            req: &ToastRequest,
            on_click: Arc<dyn Fn(String) + Send + Sync>,
        ) -> Result<(), String> {
            self.asked.lock().unwrap().push(req.clone());
            if self.click {
                on_click(req.tab.clone());
            }
            self.answer.clone()
        }
    }

    fn req() -> ToastRequest {
        ToastRequest {
            tab: "t1".into(),
            title: "proj".into(),
            body: "Which colour?".into(),
        }
    }

    /// Runs `show_async_with` and waits for its two callbacks (at most 30 s: the macOS bundle read can take seconds
    /// on a loaded machine).
    fn run(backend: Arc<dyn NotifyBackend>) -> (Vec<String>, Result<(), String>) {
        let (click_tx, click_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let click_tx = std::sync::Mutex::new(click_tx);
        let done_tx = std::sync::Mutex::new(done_tx);
        show_async_with(
            backend,
            crate::names::AUMID,
            req(),
            Arc::new(move |tab| {
                let _ = click_tx.lock().unwrap().send(tab);
            }),
            Arc::new(move |r| {
                let _ = done_tx.lock().unwrap().send(r);
            }),
        );
        let done = done_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("on_done is called once");
        (click_rx.try_iter().collect(), done)
    }

    #[test]
    fn the_backend_gets_the_request_and_its_answer_and_click_come_back() {
        let fake = Arc::new(Fake {
            asked: std::sync::Mutex::new(Vec::new()),
            answer: Ok(()),
            click: true,
        });
        let (clicks, done) = run(fake.clone());
        assert_eq!(done, Ok(()));
        assert_eq!(clicks, vec!["t1".to_string()]);
        assert_eq!(*fake.asked.lock().unwrap(), vec![req()]);
        // A refusal reaches the caller as an error, so it falls back to the taskbar or the Dock.
        let refusing = Arc::new(Fake {
            asked: std::sync::Mutex::new(Vec::new()),
            answer: Err("no".into()),
            click: false,
        });
        assert_eq!(run(refusing).1, Err("no".into()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_dev_build_says_notifications_need_flare_app_and_shows_nothing() {
        // The test binary is not in an app bundle: the real center is never reached (it would abort the process).
        let (clicks, done) = run(Arc::new(SystemBackend));
        let err = done.expect_err("never a success it did not earn");
        assert_eq!(err, MAC_NOT_IN_APP);
        assert!(err.contains("Flare.app") && err.contains("cargo run"));
        assert!(clicks.is_empty());
    }

    #[test]
    fn the_system_says_up_front_whether_it_can_take_a_notification() {
        if cfg!(windows) {
            assert_eq!(system_unavailable(), None);
            assert_eq!(known_unavailable(), None);
        } else if cfg!(target_os = "macos") {
            assert_eq!(system_unavailable(), Some(MAC_NOT_IN_APP));
            // Once read, the UI thread's question has the answer without blocking.
            assert_eq!(known_unavailable(), Some(MAC_NOT_IN_APP));
        } else {
            assert_eq!(system_unavailable(), Some(OTHER_UNAVAILABLE));
        }
        let once = FirstTime::new();
        assert!(once.take());
        assert!(!once.take());
        assert!(!once.take());
    }

    #[test]
    fn only_flares_own_signed_app_bundle_may_use_the_center() {
        let app = Some("/Applications/Flare.app");
        let id = Some("com.flare.terminal");
        assert_eq!(bundle_verdict(app, id, Ok(())), None);
        assert_eq!(
            bundle_verdict(Some("/Applications/Flare.app/"), id, Ok(())),
            None
        );
        // cargo run, a test binary: the main bundle is the executable's folder, with no id.
        assert_eq!(
            bundle_verdict(Some("/Users/x/Flare/target/debug"), None, Ok(())),
            Some(MAC_NOT_IN_APP)
        );
        assert_eq!(bundle_verdict(None, None, Ok(())), Some(MAC_NOT_IN_APP));
        // Another app's bundle (a helper, a copy renamed) is not Flare.
        assert_eq!(
            bundle_verdict(app, Some("com.flare.terminal.helper"), Ok(())),
            Some(MAC_NOT_IN_APP)
        );
        assert_eq!(bundle_verdict(app, id, Err(-67061)), Some(MAC_UNSIGNED));
    }

    /// A center that records every call; `perm` is what it says, `grant` what the prompt answers.
    struct FakeCenter {
        perm: std::sync::Mutex<Permission>,
        grant: bool,
        calls: std::sync::Mutex<Vec<String>>,
        posted: std::sync::Mutex<Vec<Posted>>,
        route: std::sync::Mutex<Option<ClickRoute>>,
    }

    impl FakeCenter {
        fn new(perm: Permission, grant: bool) -> FakeCenter {
            FakeCenter {
                perm: std::sync::Mutex::new(perm),
                grant,
                calls: std::sync::Mutex::new(Vec::new()),
                posted: std::sync::Mutex::new(Vec::new()),
                route: std::sync::Mutex::new(None),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn click(&self, c: Clicked) {
            let r = self.route.lock().unwrap().clone();
            if let Some(r) = r {
                r(c);
            }
        }
    }

    impl NotificationCenter for FakeCenter {
        fn permission(&self) -> Result<Permission, String> {
            self.calls.lock().unwrap().push("permission".into());
            Ok(*self.perm.lock().unwrap())
        }
        fn ask_permission(&self) -> Result<bool, String> {
            self.calls.lock().unwrap().push("ask".into());
            *self.perm.lock().unwrap() = if self.grant {
                Permission::Allowed
            } else {
                Permission::Denied
            };
            Ok(self.grant)
        }
        fn post(&self, n: &Posted) -> Result<(), String> {
            self.calls.lock().unwrap().push("post".into());
            self.posted.lock().unwrap().push(n.clone());
            Ok(())
        }
        fn route_clicks(&self, route: ClickRoute) {
            self.calls.lock().unwrap().push("route".into());
            *self.route.lock().unwrap() = Some(route);
        }
    }

    fn ask(tab: &str) -> ToastRequest {
        ToastRequest {
            tab: tab.into(),
            title: "proj".into(),
            body: format!("question in {tab}"),
        }
    }

    fn clicks_into(seen: &Arc<std::sync::Mutex<Vec<String>>>) -> Arc<dyn Fn(String) + Send + Sync> {
        let seen = Arc::clone(seen);
        Arc::new(move |t| seen.lock().unwrap().push(t))
    }

    #[test]
    fn the_permission_is_asked_once_lazily_and_only_when_flare_first_notifies() {
        let b = CenterBackend::new(FakeCenter::new(Permission::NotAsked, true));
        // Making the backend asks the center nothing: no prompt at launch.
        assert!(b.center().calls().is_empty());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        assert_eq!(
            b.show(crate::names::AUMID, &ask("t1"), clicks_into(&seen)),
            Ok(())
        );
        assert_eq!(b.center().calls(), ["permission", "ask", "route", "post"]);
        assert_eq!(
            b.show(crate::names::AUMID, &ask("t2"), clicks_into(&seen)),
            Ok(())
        );
        assert_eq!(
            b.center().calls(),
            ["permission", "ask", "route", "post", "permission", "post"]
        );
        // Already allowed (an earlier run said yes): no prompt at all.
        let b = CenterBackend::new(FakeCenter::new(Permission::Allowed, true));
        assert_eq!(
            b.show(crate::names::AUMID, &ask("t1"), clicks_into(&seen)),
            Ok(())
        );
        assert!(!b.center().calls().contains(&"ask".to_string()));
    }

    #[test]
    fn a_no_is_honoured_for_the_run_and_nothing_is_posted() {
        let b = CenterBackend::new(FakeCenter::new(Permission::NotAsked, false));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        for tab in ["t1", "t2", "t3"] {
            let r = b.show(crate::names::AUMID, &ask(tab), clicks_into(&seen));
            assert_eq!(
                r,
                Err(MAC_DENIED.to_string()),
                "the caller bounces the Dock instead"
            );
        }
        let calls = b.center().calls();
        assert_eq!(calls.iter().filter(|c| *c == "ask").count(), 1, "{calls:?}");
        assert!(!calls.contains(&"post".to_string()) && !calls.contains(&"route".to_string()));
        assert!(MAC_DENIED.contains("System Settings"));
        // Turned off in System Settings before this run: never asked, never posted.
        let b = CenterBackend::new(FakeCenter::new(Permission::Denied, true));
        assert!(
            b.show(crate::names::AUMID, &ask("t1"), clicks_into(&seen))
                .is_err()
        );
        assert_eq!(b.center().calls(), ["permission"]);
    }

    #[test]
    fn one_notification_per_tab_the_newer_replaces_the_older() {
        let b = CenterBackend::new(FakeCenter::new(Permission::Allowed, true));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        for tab in ["t-1", "t-1", "t-2"] {
            b.show(crate::names::AUMID, &ask(tab), clicks_into(&seen))
                .unwrap();
        }
        let posted = b.center().posted.lock().unwrap().clone();
        assert_eq!(posted.len(), 3);
        assert_eq!(
            posted[0].id, posted[1].id,
            "same identifier: the center replaces it"
        );
        assert_ne!(posted[1].id, posted[2].id);
        assert_eq!(posted[0].id, "agent-question.t1");
        assert_eq!(posted[0].tab, "t-1", "the tab itself rides along whole");
        assert!(posted.iter().all(|p| p.session == b.session()));
        assert_eq!(posted[2].title, "proj");
        assert_eq!(posted[2].body, "question in t-2");
    }

    #[test]
    fn a_click_brings_back_the_tab_of_this_runs_notification_only() {
        let b = CenterBackend::new(FakeCenter::new(Permission::Allowed, true));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        // Before the first notification no click route is set: a click on an older run's notification steers
        // nothing (as on Windows, where no activator is registered).
        b.center().click(Clicked {
            id: notification_id("t1"),
            session: b.session().into(),
            tab: "t1".into(),
        });
        b.show(crate::names::AUMID, &ask("t-1"), clicks_into(&seen))
            .unwrap();
        let own = Clicked {
            id: notification_id("t-1"),
            session: b.session().into(),
            tab: "t-1".into(),
        };
        b.center().click(own.clone());
        // An older run's notification, and one whose tab does not match its identifier, do nothing.
        b.center().click(Clicked {
            session: "an-older-run".into(),
            ..own.clone()
        });
        b.center().click(Clicked {
            tab: "t-9".into(),
            ..own
        });
        assert_eq!(*seen.lock().unwrap(), ["t-1"]);
    }

    /// The bundle check on a real bundle: a copy of this test binary in a `Flare.app`, signed ad hoc, run as a
    /// child that only reads its own bundle and signature (no notification center, no window, no keychain).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_in_a_signed_flare_app_is_told_from_a_dev_build() {
        if std::env::var_os("FLARE_TOAST_VERDICT_PROBE").is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let verdict_of = |signed: bool, id: &str| -> String {
            let app = dir.path().join(format!("{}-{id}/Flare.app", signed));
            let macos = app.join("Contents/MacOS");
            std::fs::create_dir_all(&macos).unwrap();
            let exe = macos.join("Flare");
            std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
            std::fs::write(
                app.join("Contents/Info.plist"),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
                     \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\
                     <key>CFBundleIdentifier</key><string>{id}</string>\
                     <key>CFBundleExecutable</key><string>Flare</string>\
                     <key>CFBundlePackageType</key><string>APPL</string></dict></plist>\n"
                ),
            )
            .unwrap();
            if signed {
                let st = std::process::Command::new("/usr/bin/codesign")
                    .args(["--force", "--sign", "-", "--identifier", id])
                    .arg(&app)
                    .output()
                    .unwrap();
                assert!(
                    st.status.success(),
                    "{}",
                    String::from_utf8_lossy(&st.stderr)
                );
            }
            let out = std::process::Command::new(&exe)
                .args([
                    "--exact",
                    "toast::tests::verdict_probe",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("FLARE_TOAST_VERDICT_PROBE", "1")
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            text.lines()
                .find_map(|l| l.split_once("VERDICT=").map(|(_, v)| v.trim().to_string()))
                .unwrap_or_else(|| panic!("no verdict: {text}"))
        };
        assert_eq!(verdict_of(true, crate::names::AUMID), "ok");
        assert_eq!(verdict_of(true, "com.example.other"), MAC_NOT_IN_APP);
        // The linker's own signature does not seal the bundle: an app that was never signed is refused.
        assert_eq!(verdict_of(false, crate::names::AUMID), MAC_UNSIGNED);
    }

    /// Prints this process's bundle verdict, when the test above runs it as a child.
    #[cfg(target_os = "macos")]
    #[test]
    fn verdict_probe() {
        if std::env::var_os("FLARE_TOAST_VERDICT_PROBE").is_none() {
            return;
        }
        println!("VERDICT={}", system_unavailable().unwrap_or("ok"));
    }

    #[test]
    fn the_aumid_is_the_one_the_installer_gives_the_shortcut() {
        assert_eq!(crate::names::AUMID, "com.flare.terminal");
    }
}
