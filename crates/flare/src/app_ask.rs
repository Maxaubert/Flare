//! The app-link question (rule 252): a page went to a link to an app (`bankid:`, `swish:`, `mailto:`), the browser was
//! stopped, and Flare asks the person whether to open it. Same dialog as the download question, with Cancel the
//! default; this file is its words, its queue and its limits (a page that asks again and again is held back), and the
//! seam that opens the app (`AppLauncher`, a recording one in every test so no test opens a real app).

use std::collections::VecDeque;
use std::sync::Mutex;

use flare_tabs::modal::escape_markdown;

/// One app link waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAsk {
    pub id: u64,
    /// The tab whose page (or a pop-up over it) asked.
    pub tab: String,
    /// The link as the page gave it: opened only after the person's Open, and never shown or logged.
    pub uri: String,
    /// Its scheme in lower case (`flare_surfaces::web::external_scheme`).
    pub scheme: String,
    /// The asking page's site (`flare_surfaces::web::site_of`).
    pub site: String,
}

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppView {
    pub title: &'static str,
    pub body: String,
    pub primary: &'static str,
}

/// The longest site or scheme the question prints (the rest is cut with an ellipsis).
const SHOWN_MAX: usize = 80;

fn shown(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    let cut: String = clean.chars().take(SHOWN_MAX).collect();
    let text = if clean.chars().count() > SHOWN_MAX {
        format!("{cut}...")
    } else {
        cut
    };
    escape_markdown(&text)
}

/// The question. `ink` is the theme's text colour (the site and the app wear it).
pub fn app_view(a: &AppAsk, ink: &str) -> AppView {
    let named = |s: &str| format!("<font color=\"{ink}\">{}</font>", shown(s));
    let site = if a.site.is_empty() {
        "A page"
    } else {
        a.site.as_str()
    };
    AppView {
        title: "Open an app?",
        body: format!(
            "{} wants to open {} on this PC.",
            named(site),
            named(&a.scheme)
        ),
        primary: "Open",
    }
}

/// The bar's words for an app link nothing on the PC opens (no `URL Protocol` handler for its scheme).
pub fn no_handler_notice(scheme: &str) -> String {
    let s: String = scheme
        .chars()
        .filter(|c| !c.is_control())
        .take(32)
        .collect();
    format!("Nothing on this PC opens {s}:")
}

/// The bar's words for an app link that was not asked about: a limit, or the person's Cancel for it on this site.
pub const APP_LINK_HELD: &str = "App link not opened: asked too often";

/// The bar's words when the shell could not open a link the person allowed (its worker answered).
pub const APP_NOT_OPENED: &str = "The app could not be opened";

/// The bar's words for an app link while an agent uses the page: only the person may allow one (rule 252).
pub const APP_LINK_AGENT: &str = "App link not opened while an agent uses the page";

/// What an agent's waiting call answers when its page went to an app link.
pub const APP_LINK_AGENT_ANSWER: &str = "only the person can allow opening an app";

/// Open does nothing for this long after the question shows: a click meant for the page lands on nothing.
pub const OPEN_GUARD_MS: u64 = 300;
/// One question per site and app in this long.
pub const PAIR_GAP_MS: u64 = 10_000;
/// At most this many questions in [`MINUTE_MS`], in all.
pub const PER_MINUTE: usize = 5;
const MINUTE_MS: u64 = 60_000;
/// At most this many questions wait behind the one on the screen.
pub const WAITING_MAX: usize = 2;

/// What became of an app link offered to the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offered {
    /// It is the one to ask about now.
    Now,
    /// It waits behind another question.
    Waits,
    /// Not asked: its tab has a question already, a limit holds, or the person cancelled it on this site.
    Held,
}

/// The app links waiting for an answer (the first is the one on the screen), and the limits.
#[derive(Debug, Default)]
pub struct AppAsks {
    waiting: VecDeque<AppAsk>,
    next_id: u64,
    /// When the first was put on the screen (None: it is not showing, another question has the dialog).
    shown_at: Option<u64>,
    /// When each question of the last minute was asked.
    recent: VecDeque<u64>,
    /// The last question for each (site, scheme).
    pairs: Vec<((String, String), u64)>,
    /// (tab, site, scheme) the person cancelled: not asked again until the tab's page is on another site.
    muted: Vec<(String, String, String)>,
}

impl AppAsks {
    /// A page of `tab` on `site` went to `uri` for `scheme`, at `now` (ms).
    pub fn offer(&mut self, tab: &str, uri: &str, scheme: &str, site: &str, now: u64) -> Offered {
        let muted = self
            .muted
            .iter()
            .any(|(t, s, k)| t == tab && s == site && k == scheme);
        if muted || self.waiting.iter().any(|a| a.tab == tab) {
            return Offered::Held;
        }
        while self
            .recent
            .front()
            .is_some_and(|&t| now.saturating_sub(t) >= MINUTE_MS)
        {
            self.recent.pop_front();
        }
        self.pairs
            .retain(|(_, at)| now.saturating_sub(*at) < PAIR_GAP_MS);
        let pair = (site.to_string(), scheme.to_string());
        if self.pairs.iter().any(|(p, _)| *p == pair)
            || self.recent.len() >= PER_MINUTE
            || self.waiting.len() > WAITING_MAX
        {
            return Offered::Held;
        }
        self.next_id += 1;
        self.waiting.push_back(AppAsk {
            id: self.next_id,
            tab: tab.to_string(),
            uri: uri.to_string(),
            scheme: scheme.to_string(),
            site: site.to_string(),
        });
        self.recent.push_back(now);
        self.pairs.push((pair, now));
        if self.waiting.len() == 1 {
            Offered::Now
        } else {
            Offered::Waits
        }
    }

    /// The one being asked about.
    pub fn current(&self) -> Option<&AppAsk> {
        self.waiting.front()
    }

    /// The first is on the screen from `now` (the 0.3 s guard starts); a question already showing keeps its time.
    pub fn shown(&mut self, now: u64) {
        if self.shown_at.is_none() && !self.waiting.is_empty() {
            self.shown_at = Some(now);
        }
    }

    /// Another question took the dialog: the first is not on the screen (its guard starts again when it is).
    pub fn unshown(&mut self) {
        self.shown_at = None;
    }

    pub fn is_shown(&self) -> bool {
        self.shown_at.is_some()
    }

    /// The person pressed Open at `now`. None while the guard holds (nothing happens, the question stays) or when
    /// nothing is on the screen; else the link to open, which leaves the queue.
    pub fn open(&mut self, now: u64) -> Option<AppAsk> {
        let at = self.shown_at?;
        if now.saturating_sub(at) < OPEN_GUARD_MS {
            return None;
        }
        self.shown_at = None;
        self.waiting.pop_front()
    }

    /// The person cancelled the first: it leaves the queue, and its site and app are not asked about again in its tab
    /// until the page is on another site.
    pub fn cancel(&mut self) -> Option<AppAsk> {
        self.shown_at = None;
        let a = self.waiting.pop_front()?;
        self.muted
            .push((a.tab.clone(), a.site.clone(), a.scheme.clone()));
        Some(a)
    }

    /// The page of `tab` is now on `site`: the Cancels of another site's links are forgotten.
    pub fn page_at(&mut self, tab: &str, site: &str) {
        self.muted.retain(|(t, s, _)| t != tab || s == site);
    }

    /// Keeps the questions and Cancels of the tabs `alive` says yes to (a closed tab's go). True when the first
    /// changed.
    pub fn prune(&mut self, mut alive: impl FnMut(&str) -> bool) -> bool {
        let head = self.current().map(|a| a.id);
        self.waiting.retain(|a| alive(&a.tab));
        self.muted.retain(|(t, _, _)| alive(t));
        let changed = self.current().map(|a| a.id) != head;
        if changed {
            self.shown_at = None;
        }
        changed
    }

    /// Every one, at once (the window is closing).
    pub fn drain(&mut self) -> Vec<AppAsk> {
        self.shown_at = None;
        self.waiting.drain(..).collect()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
}

/// What opens an app link once the person said Open: the Windows shell on the bare link in production
/// (`flare_win::shell_open::open_app_uri`), a recorder everywhere else.
pub trait AppLauncher {
    /// Has Windows an app for `scheme` (`HKEY_CLASSES_ROOT\<scheme>` with `URL Protocol`)?
    fn has_handler(&self, scheme: &str) -> bool;
    /// Opens the link with that app and tells `done` how it went. The UI thread never waits for it (rule 34): the shell
    /// may start a handler or hold a DDE talk with one before it answers.
    fn open(&self, uri: &str, done: Opened);
}

/// How an app link's open went, told from wherever it ran.
pub type Opened = Box<dyn FnOnce(Result<(), String>) + Send>;

/// The real one: the shell on a worker thread of its own, one per Open.
pub struct ShellLauncher;

impl AppLauncher for ShellLauncher {
    fn has_handler(&self, scheme: &str) -> bool {
        // A read of one registry value, no process and no disk wait beyond the loaded hive (rule 34 is about waits).
        flare_win::shell_open::url_handler_registered(scheme)
    }

    fn open(&self, uri: &str, done: Opened) {
        let uri = uri.to_string();
        // The worker owns `done` until it answers; if it cannot start, the answer is the failure, here.
        let slot = std::sync::Arc::new(Mutex::new(Some(done)));
        let theirs = std::sync::Arc::clone(&slot);
        let spawned = std::thread::Builder::new()
            .name("flare-app-link".into())
            .spawn(move || {
                let r = flare_win::shell_open::open_app_uri_on_worker(&uri);
                let done = theirs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(done) = done {
                    done(r);
                }
            });
        if let Err(e) = spawned {
            let done = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(done) = done {
                done(Err(format!("could not start the worker: {e}")));
            }
        }
    }
}

/// Records what it was asked to open and opens nothing (the tests, `--e2e`, and every window until startup hands in
/// the real one). It says it has an app for every scheme except those in `missing`, and answers at once: a failure when
/// `fails` is set.
#[derive(Debug, Default)]
pub struct RecordingLauncher {
    pub opened: Mutex<Vec<String>>,
    pub missing: Mutex<Vec<String>>,
    pub fails: Mutex<bool>,
}

impl RecordingLauncher {
    #[cfg(test)]
    pub fn opened(&self) -> Vec<String> {
        self.opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl AppLauncher for RecordingLauncher {
    fn has_handler(&self, scheme: &str) -> bool {
        !self
            .missing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|m| m == scheme)
    }

    fn open(&self, uri: &str, done: Opened) {
        self.opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(uri.to_string());
        let fails = *self
            .fails
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        done(if fails {
            Err("the recorder was told to fail".to_string())
        } else {
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 34: the shell's open runs on a worker and answers from there, never on the caller's thread. A path is refused
    /// before Windows sees it, so nothing opens.
    #[test]
    fn the_shell_launcher_opens_on_a_worker_and_answers_from_it() {
        let (tx, rx) = std::sync::mpsc::channel();
        ShellLauncher.open(
            "C:\\x.exe",
            Box::new(move |r| {
                let _ = tx.send((std::thread::current().id(), r));
            }),
        );
        let (thread, r) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker answers");
        assert_ne!(
            thread,
            std::thread::current().id(),
            "answered from a worker"
        );
        assert!(r.is_err(), "a path is refused");
    }

    fn ask(tab: &str, scheme: &str, site: &str) -> AppAsk {
        AppAsk {
            id: 1,
            tab: tab.into(),
            uri: format!("{scheme}:///?token=secret"),
            scheme: scheme.into(),
            site: site.into(),
        }
    }

    fn offer(q: &mut AppAsks, tab: &str, scheme: &str, site: &str, now: u64) -> Offered {
        q.offer(tab, &format!("{scheme}:///?t=1"), scheme, site, now)
    }

    #[test]
    fn the_question_names_the_site_and_the_app_and_never_the_link() {
        let v = app_view(&ask("t1", "bankid", "bank.example"), "#ffffff");
        assert_eq!(v.title, "Open an app?");
        assert_eq!(v.primary, "Open");
        assert_eq!(
            v.body,
            "<font color=\"#ffffff\">bank\\.example</font> wants to open <font color=\"#ffffff\">bankid</font> on this PC."
        );
        assert!(!v.body.contains("secret"));
        let v = app_view(&ask("t1", "x<b>y</b>\u{7}", ""), "#fff");
        assert!(
            !v.body.contains("<b>") && !v.body.contains('\u{7}'),
            "{}",
            v.body
        );
        assert!(v.body.contains("A page"));
        assert_eq!(
            no_handler_notice("swish"),
            "Nothing on this PC opens swish:"
        );
    }

    #[test]
    fn one_question_a_tab_and_two_waiting_behind_the_one_shown() {
        let mut q = AppAsks::default();
        assert_eq!(offer(&mut q, "t1", "bankid", "a.example", 0), Offered::Now);
        assert_eq!(
            offer(&mut q, "t1", "swish", "a.example", 1),
            Offered::Held,
            "its tab has a question already"
        );
        assert_eq!(
            offer(&mut q, "t2", "bankid", "b.example", 2),
            Offered::Waits
        );
        assert_eq!(
            offer(&mut q, "t3", "bankid", "c.example", 3),
            Offered::Waits
        );
        assert_eq!(
            offer(&mut q, "t4", "bankid", "d.example", 4),
            Offered::Held,
            "two wait at most"
        );
        assert_eq!(q.current().map(|a| a.tab.as_str()), Some("t1"));
    }

    #[test]
    fn one_question_per_site_and_app_every_ten_seconds_and_five_a_minute() {
        let mut q = AppAsks::default();
        assert_eq!(offer(&mut q, "t1", "bankid", "a.example", 0), Offered::Now);
        q.shown(0);
        q.cancel();
        // Another tab on the same site, too soon for that site and app.
        assert_eq!(
            offer(&mut q, "t2", "bankid", "a.example", 9_999),
            Offered::Held
        );
        assert_eq!(
            offer(&mut q, "t2", "bankid", "a.example", 10_000),
            Offered::Now
        );
        q.shown(10_000);
        q.cancel();
        let mut q = AppAsks::default();
        for (i, at) in [0u64, 100, 200, 300, 400].into_iter().enumerate() {
            let tab = format!("t{i}");
            assert_ne!(
                offer(&mut q, &tab, "bankid", &format!("s{i}.example"), at),
                Offered::Held
            );
            q.shown(at);
            q.open(at + OPEN_GUARD_MS);
        }
        assert_eq!(
            offer(&mut q, "t9", "bankid", "s9.example", 59_999),
            Offered::Held,
            "five a minute in all"
        );
        assert_eq!(
            offer(&mut q, "t9", "bankid", "s9.example", 60_000),
            Offered::Now
        );
    }

    #[test]
    fn open_does_nothing_for_the_first_three_tenths_of_a_second() {
        let mut q = AppAsks::default();
        offer(&mut q, "t1", "bankid", "a.example", 1_000);
        assert_eq!(q.open(1_000), None, "not on the screen yet");
        q.shown(1_000);
        assert_eq!(q.open(1_000), None);
        assert_eq!(q.open(1_299), None);
        assert!(q.current().is_some(), "the question stays");
        assert_eq!(q.open(1_300).map(|a| a.scheme), Some("bankid".into()));
        assert!(q.is_empty());
        // Hidden by another question and shown again: the guard starts again.
        offer(&mut q, "t2", "swish", "b.example", 20_000);
        q.shown(20_000);
        q.unshown();
        q.shown(30_000);
        assert_eq!(q.open(30_100), None);
        assert!(q.open(30_300).is_some());
    }

    #[test]
    fn a_cancel_mutes_that_site_and_app_in_its_tab_until_the_page_changes_site() {
        let mut q = AppAsks::default();
        offer(&mut q, "t1", "bankid", "a.example", 0);
        q.shown(0);
        q.cancel();
        assert_eq!(
            offer(&mut q, "t1", "bankid", "a.example", 20_000),
            Offered::Held
        );
        assert_eq!(
            offer(&mut q, "t2", "bankid", "a.example", 20_001),
            Offered::Now,
            "another tab is not muted"
        );
        q.shown(20_001);
        q.open(20_400);
        q.page_at("t1", "a.example");
        assert_eq!(
            offer(&mut q, "t1", "bankid", "a.example", 40_000),
            Offered::Held
        );
        q.page_at("t1", "shop.example");
        assert_eq!(
            offer(&mut q, "t1", "bankid", "a.example", 40_001),
            Offered::Now
        );
    }

    #[test]
    fn a_closed_tabs_questions_and_cancels_go() {
        let mut q = AppAsks::default();
        offer(&mut q, "t1", "bankid", "a.example", 0);
        offer(&mut q, "t2", "swish", "b.example", 1);
        q.shown(1);
        assert!(q.prune(|t| t != "t1"), "the first changed");
        assert!(!q.is_shown(), "the next one has not been shown yet");
        assert_eq!(q.current().map(|a| a.tab.as_str()), Some("t2"));
        assert!(!q.prune(|t| t != "t1"));
        q.shown(2);
        q.cancel();
        q.prune(|t| t != "t2");
        assert_eq!(
            offer(&mut q, "t2", "swish", "b.example", 50_000),
            Offered::Now,
            "a closed tab's Cancel is forgotten with it"
        );
        assert_eq!(q.drain().len(), 1);
        assert!(q.is_empty());
    }

    #[test]
    fn the_recording_launcher_opens_nothing_and_says_which_apps_are_missing() {
        let r = RecordingLauncher::default();
        r.missing.lock().unwrap().push("swish".into());
        assert!(r.has_handler("bankid") && !r.has_handler("swish"));
        let (tx, rx) = std::sync::mpsc::channel();
        r.open(
            "bankid:///x",
            Box::new(move |res| {
                let _ = tx.send(res);
            }),
        );
        assert_eq!(rx.try_recv().unwrap(), Ok(()), "answered at once");
        assert_eq!(r.opened(), vec!["bankid:///x".to_string()]);
    }
}
