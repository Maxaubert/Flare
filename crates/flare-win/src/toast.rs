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
//! The system part sits behind [`NotifyBackend`] (a test hands its own to [`show_async_with`]). macOS:
//! `UNUserNotificationCenter` refuses an app that is not a signed bundle, and Flare has no Developer ID
//! yet, so [`MacBackend`] shows nothing and answers [`MAC_UNAVAILABLE`]; the caller then bounces the Dock
//! icon, as it flashes the taskbar when Windows refuses a toast (regression rule 242).
//! TODO(port phase 8): with the signed `Flare.app`, ask for authorisation once and post a
//! `UNNotificationRequest` (identifier: the tab's tag, so a newer one replaces the older; a delegate
//! hands the click back as `on_click(tab)`), then test it for real.

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

/// The platform's backend: the WinRT toast on Windows, [`MacBackend`] on macOS.
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

/// What the macOS backend answers until Flare is a signed app (port phase 8).
pub const MAC_UNAVAILABLE: &str =
    "notifications are unavailable until the app is signed (macOS needs a signed Flare.app)";

/// Why the system cannot take a notification request at all, known before any worker starts: None on
/// Windows (whether a toast is then displayed is Windows' business), [`MAC_UNAVAILABLE`] on macOS until
/// the signed app exists (port phase 8). The caller then falls back at once, with no worker per question
/// and no warning per question.
pub fn system_unavailable() -> Option<&'static str> {
    if cfg!(windows) {
        None
    } else {
        Some(MAC_UNAVAILABLE)
    }
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

/// macOS: `UNUserNotificationCenter` needs a signed app bundle with a bundle id, which Flare does not have
/// yet. It shows nothing and says so; it never reports a notification it did not post.
#[cfg(not(windows))]
pub struct MacBackend;

#[cfg(not(windows))]
impl NotifyBackend for MacBackend {
    fn show(
        &self,
        _aumid: &str,
        _req: &ToastRequest,
        _on_click: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<(), String> {
        Err(MAC_UNAVAILABLE.to_string())
    }
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

/// macOS: [`MacBackend`] (unavailable until the app is signed, port phase 8).
#[cfg(not(windows))]
fn show_now(
    aumid: &str,
    req: &ToastRequest,
    on_click: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<(), String> {
    MacBackend.show(aumid, req, on_click)
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

    /// Runs `show_async_with` and waits for its two callbacks (at most 5 s).
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
            .recv_timeout(std::time::Duration::from_secs(5))
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

    #[cfg(not(windows))]
    #[test]
    fn macos_says_notifications_wait_for_the_signed_app_and_shows_nothing() {
        let (clicks, done) = run(Arc::new(SystemBackend));
        let err = done.expect_err("never a success it did not earn");
        assert_eq!(err, MAC_UNAVAILABLE);
        assert!(err.contains("signed"));
        assert!(clicks.is_empty());
    }

    #[test]
    fn the_system_says_up_front_whether_it_can_take_a_notification() {
        if cfg!(windows) {
            assert_eq!(system_unavailable(), None);
        } else {
            assert_eq!(system_unavailable(), Some(MAC_UNAVAILABLE));
        }
        let once = FirstTime::new();
        assert!(once.take());
        assert!(!once.take());
        assert!(!once.take());
    }

    #[test]
    fn the_aumid_is_the_one_the_installer_gives_the_shortcut() {
        assert_eq!(crate::names::AUMID, "com.flare.terminal");
    }
}
