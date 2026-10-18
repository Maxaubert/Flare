//! M9 Task 10: the engine, driven against `FakeFeed`, a recording `Events` and a manual `Sleeper`.
//! No window, no network, no disk.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use flare_update::engine::{
    CHECK_EVERY_MS, Calls, Engine, Events, Fld, Mode, QUIT_AFTER_MS, Sleeper, mode_for,
};
use flare_update::feed::{FakeFeed, Latch};
use flare_update::preview::{Waiter, preview_update};
use flare_update::types::Offer;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

type Crumb = (String, Vec<(String, Fld)>);

/// Records everything the engine says.
#[derive(Default)]
struct Rec {
    offers: Mutex<Vec<Offer>>,
    pcts: Mutex<Vec<u8>>,
    crumbs: Mutex<Vec<Crumb>>,
    quits: Mutex<Vec<u64>>,
}

impl Events for Rec {
    fn offer(&self, o: Offer) {
        lock(&self.offers).push(o);
    }
    fn progress(&self, pct: u8) {
        lock(&self.pcts).push(pct);
    }
    fn crumb(&self, name: &str, fields: Vec<(String, Fld)>) {
        lock(&self.crumbs).push((name.to_string(), fields));
    }
    fn quit_in(&self, ms: u64) {
        lock(&self.quits).push(ms);
    }
    fn rss_mb(&self) -> Option<u64> {
        Some(42)
    }
}

/// Sleeps until the test hands out a permit; remembers every duration asked for.
#[derive(Default)]
struct Manual {
    st: Mutex<ManualState>,
    cv: Condvar,
    left: AtomicBool,
}

#[derive(Default)]
struct ManualState {
    asked: Vec<u64>,
    permits: u32,
    stopped: bool,
}

impl Manual {
    fn advance(&self) {
        lock(&self.st).permits += 1;
        self.cv.notify_all();
    }
    fn asked(&self) -> Vec<u64> {
        lock(&self.st).asked.clone()
    }
}

impl Sleeper for Manual {
    fn sleep(&self, ms: u64) -> bool {
        let mut st = lock(&self.st);
        st.asked.push(ms);
        self.cv.notify_all();
        loop {
            if st.stopped {
                self.left.store(true, Ordering::SeqCst);
                return false;
            }
            if st.permits > 0 {
                st.permits -= 1;
                return true;
            }
            st = self.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
    }
    fn stop(&self) {
        lock(&self.st).stopped = true;
        self.cv.notify_all();
    }
}

struct NoWait;
impl Waiter for NoWait {
    fn wait(&self, _ms: u64) {}
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

struct Rig {
    engine: Arc<Engine>,
    feed: FakeFeed,
    rec: Arc<Rec>,
    sleeper: Arc<Manual>,
}

fn rig_with(mode: Mode, feed: FakeFeed, dir: Option<&str>) -> Rig {
    let rec = Arc::new(Rec::default());
    let sleeper = Arc::new(Manual::default());
    let engine = Engine::new(
        mode,
        Arc::new(feed.clone()),
        "1.0.0",
        dir.map(Into::into),
        rec.clone(),
        sleeper.clone(),
        Arc::new(NoWait),
    );
    Rig {
        engine,
        feed,
        rec,
        sleeper,
    }
}

fn live(feed: FakeFeed) -> Rig {
    rig_with(Mode::Live, feed, None)
}

fn offered(version: &str) -> FakeFeed {
    FakeFeed::new().offering(version, "notes")
}

fn zero() -> Calls {
    Calls::default()
}

#[test]
fn mode_table() {
    for bits in 0u8..16 {
        let (packaged, e2e, flag, live_feature) =
            (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
        let want = if flag || (!packaged && !e2e) {
            Mode::Preview
        } else if packaged && !e2e && !flag && live_feature {
            Mode::Live
        } else {
            Mode::Off
        };
        assert_eq!(
            mode_for(packaged, e2e, flag, live_feature),
            want,
            "{bits:04b}"
        );
        if !live_feature {
            assert_ne!(mode_for(packaged, e2e, flag, live_feature), Mode::Live);
        }
    }
}

#[test]
fn a_default_build_has_no_live_mode() {
    const { assert!(!cfg!(feature = "live-updates")) };
    assert_eq!(
        mode_for(true, false, false, cfg!(feature = "live-updates")),
        Mode::Off
    );
}

#[test]
fn off_and_preview_make_no_checks_and_no_installs() {
    for mode in [Mode::Off, Mode::Preview] {
        let r = rig_with(mode, offered("1.2.0"), None);
        r.engine.start();
        r.engine.tick();
        assert_eq!(r.engine.calls(), zero());
        assert_eq!(r.feed.checks(), 0);
    }
}

#[test]
fn a_preview_offer_appears_at_once_and_never_replaces_a_real_one() {
    let r = rig_with(Mode::Preview, FakeFeed::new(), None);
    r.engine.start();
    let o = r.engine.offer().expect("the preview is on offer");
    assert!(o.mock);
    assert_eq!(lock(&r.rec.offers).len(), 1);

    let r = rig_with(Mode::Preview, FakeFeed::new(), None);
    let real = Offer {
        version: "2.0.0".into(),
        notes: String::new(),
        mock: false,
        token: 0,
    };
    r.engine.set_offer(real);
    r.engine.preview_if_none();
    r.engine.start();
    let o = r.engine.offer().expect("offer");
    assert!(!o.mock);
    assert_eq!(o.version, "2.0.0");
    assert_eq!(lock(&r.rec.offers).len(), 1);
}

#[test]
fn live_checks_at_launch_then_every_four_hours() {
    let r = live(offered("1.2.0"));
    r.engine.start();
    wait_until("the first check", || r.feed.checks() == 1);
    wait_until("the first sleep", || r.sleeper.asked().len() == 1);
    r.sleeper.advance();
    wait_until("the second check", || r.feed.checks() == 2);
    wait_until("the second sleep", || r.sleeper.asked().len() == 2);
    assert_eq!(r.sleeper.asked(), vec![CHECK_EVERY_MS, CHECK_EVERY_MS]);
    r.engine.stop();
}

#[test]
fn offline_is_silent() {
    let r = live(FakeFeed::new().check_error("offline"));
    r.engine.start();
    wait_until("the sleep", || r.sleeper.asked().len() == 1);
    assert!(r.engine.offer().is_none());
    let crumbs = lock(&r.rec.crumbs);
    assert_eq!(crumbs.len(), 1);
    assert!(
        crumbs[0]
            .1
            .contains(&("offered".to_string(), Fld::Bool(false)))
    );
    drop(crumbs);
    r.sleeper.advance();
    wait_until("the loop continues", || r.feed.checks() == 2);
    r.engine.stop();
}

#[test]
fn each_check_leaves_a_crumb() {
    let r = live(offered("1.2.0"));
    r.engine.tick();
    let crumbs = lock(&r.rec.crumbs);
    let (name, fields) = &crumbs[0];
    assert_eq!(name, "update-check");
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["offered", "fetchMs", "offerMs", "rssMb"]);
    assert_eq!(fields[0].1, Fld::Bool(true));
    assert_eq!(fields[3].1, Fld::Num(42));
}

#[test]
fn the_thread_does_not_keep_the_process_alive() {
    let r = live(offered("1.2.0"));
    r.engine.start();
    wait_until("the sleep", || r.sleeper.asked().len() == 1);
    let t = Instant::now();
    r.engine.stop();
    wait_until("the thread to leave its sleep", || {
        r.sleeper.left.load(Ordering::SeqCst)
    });
    assert!(t.elapsed() < Duration::from_secs(1));
    // Dropping an engine that was never stopped hangs nothing.
    let r2 = live(offered("1.2.0"));
    r2.engine.start();
    wait_until("the sleep", || r2.sleeper.asked().len() == 1);
    drop(r2.engine);
}

#[test]
fn installs_only_the_current_offer() {
    let r = live(offered("1.2.0"));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert!(!r.engine.install(None, &|| {}));
    assert!(!r.engine.install(Some(token + 1), &|| {}));
    assert_eq!(r.feed.downloads(), 0);
    assert_eq!(r.engine.calls().installs, 0);
    assert!(r.engine.install(Some(token), &|| {}));
    assert_eq!(r.feed.downloads(), 1);
    assert_eq!(r.engine.calls().installs, 1);
}

fn pcts_of(feed: FakeFeed) -> Vec<u8> {
    let r = live(feed);
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert!(r.engine.install(Some(token), &|| {}));
    lock(&r.rec.pcts).clone()
}

#[test]
fn progress_is_capped_at_99_while_downloading_then_100() {
    let mut want: Vec<u8> = (0..=19).map(|i| i * 5).collect();
    want.extend([99, 100]);
    assert_eq!(pcts_of(offered("1.2.0")), want);
    assert_eq!(
        pcts_of(offered("1.2.0").progress(&[50, 100])),
        [50, 99, 100]
    );
    assert_eq!(
        pcts_of(offered("1.2.0").progress(&[0, 35, 70, 100])),
        [0, 35, 70, 99, 100]
    );
}

#[test]
fn a_failed_download_discards_and_answers_false_with_no_quit() {
    let r = live(offered("1.2.0").fail_download_at(40));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert!(!r.engine.install(Some(token), &|| {}));
    assert_eq!(r.feed.discards(), 1);
    assert!(lock(&r.rec.quits).is_empty());
    assert_eq!(r.feed.applies(), 0);
}

#[test]
fn cancel_wins_even_after_the_last_byte() {
    // The events cancel as the last byte is reported, so the cancel lands after the download.
    let r = live_cancel_at_end();
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert!(!r.engine.install(Some(token), &|| {}));
    assert_eq!(r.feed.discards(), 1);
    assert_eq!(r.feed.applies(), 0);
}

/// A rig whose events cancel the engine the moment the raw 100 percent step is reported as 99.
fn live_cancel_at_end() -> Rig {
    struct CancelAt99 {
        rec: Arc<Rec>,
        engine: Mutex<Option<Arc<Engine>>>,
    }
    impl Events for CancelAt99 {
        fn offer(&self, o: Offer) {
            self.rec.offer(o);
        }
        fn progress(&self, pct: u8) {
            self.rec.progress(pct);
            if pct == 99
                && let Some(e) = lock(&self.engine).as_ref()
            {
                e.cancel();
            }
        }
        fn crumb(&self, n: &str, f: Vec<(String, Fld)>) {
            self.rec.crumb(n, f);
        }
        fn quit_in(&self, ms: u64) {
            self.rec.quit_in(ms);
        }
        fn rss_mb(&self) -> Option<u64> {
            None
        }
    }
    let rec = Arc::new(Rec::default());
    let hook = Arc::new(CancelAt99 {
        rec: rec.clone(),
        engine: Mutex::new(None),
    });
    let feed = offered("1.2.0").progress(&[100]);
    let sleeper = Arc::new(Manual::default());
    let engine = Engine::new(
        Mode::Live,
        Arc::new(feed.clone()),
        "1.0.0",
        None,
        hook.clone(),
        sleeper.clone(),
        Arc::new(NoWait),
    );
    *lock(&hook.engine) = Some(engine.clone());
    Rig {
        engine,
        feed,
        rec,
        sleeper,
    }
}

#[test]
fn a_cancel_during_the_download_leaves_nothing() {
    let latch = Latch::new();
    let r = live(offered("1.2.0").hold_download(&latch));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    let engine = r.engine.clone();
    let worker = thread::spawn(move || engine.install(Some(token), &|| {}));
    wait_until("the download to start", || r.feed.downloads() == 1);
    r.engine.cancel();
    latch.open();
    assert!(!worker.join().expect("worker"));
    assert!(!r.feed.files_left());
    assert_eq!(r.feed.applies(), 0);
}

#[test]
fn an_apply_that_cannot_start_counts_as_failed_and_the_app_does_not_quit() {
    let r = live(offered("1.2.0").fail_apply());
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert!(!r.engine.install(Some(token), &|| {}));
    assert_eq!(r.feed.discards(), 1);
    assert!(lock(&r.rec.quits).is_empty());
}

#[test]
fn a_successful_install_pre_answers_the_close_question_at_the_quit() {
    let r = live(offered("1.2.0"));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    let seen = Mutex::new(Vec::new());
    let agree = || {
        seen.lock().unwrap().push((
            r.feed.applies(),
            lock(&r.rec.quits).len(),
            r.feed.downloads(),
        ));
    };
    assert!(r.engine.install(Some(token), &agree));
    // Called once, after the hand-off (1 apply), before the quit was scheduled (0 quits).
    assert_eq!(*seen.lock().unwrap(), vec![(1, 0, 1)]);
    assert_eq!(*lock(&r.rec.quits), vec![QUIT_AFTER_MS]);
}

#[test]
fn the_app_quits_400_ms_after_a_successful_handoff() {
    assert_eq!(QUIT_AFTER_MS, 400);
    let r = live(offered("1.2.0"));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert!(r.engine.install(Some(token), &|| {}));
    assert_eq!(*lock(&r.rec.quits), vec![400]);
}

#[test]
fn restart_args_carry_the_profile_only_when_the_running_copy_had_one() {
    let r = rig_with(Mode::Live, offered("1.2.0"), Some("C:\\p q"));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert!(r.engine.install(Some(token), &|| {}));
    assert_eq!(
        r.feed.applied_args(),
        Some(vec!["--user-data-dir=C:\\p q".to_string()])
    );
    let r = live(offered("1.2.0"));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert!(r.engine.install(Some(token), &|| {}));
    assert_eq!(r.feed.applied_args(), Some(Vec::new()));
}

#[test]
fn a_whole_preview_leaves_the_counters_at_zero() {
    let r = rig_with(Mode::Preview, FakeFeed::new(), None);
    r.engine.set_offer(preview_update("1.0.0"));
    assert!(!r.engine.install(Some(0), &|| {}));
    assert_eq!(r.engine.calls(), zero());
    assert_eq!(r.feed.checks() + r.feed.downloads() + r.feed.applies(), 0);
    assert_eq!(r.feed.discards(), 0);
    assert_eq!(lock(&r.rec.pcts).last(), Some(&100));
}

#[test]
fn set_offer_takes_a_fresh_token_and_counts_no_check() {
    let r = rig_with(Mode::Off, FakeFeed::new(), None);
    r.engine.set_offer(preview_update("1.0.0"));
    r.engine.set_offer(preview_update("1.0.0"));
    assert_eq!(r.engine.offer().expect("offer").token, 2);
    assert_eq!(lock(&r.rec.offers).len(), 2);
    assert_eq!(r.engine.calls(), zero());
}

#[test]
fn one_install_at_a_time() {
    let latch = Latch::new();
    let r = live(offered("1.2.0").hold_download(&latch));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    let engine = r.engine.clone();
    let worker = thread::spawn(move || engine.install(Some(token), &|| {}));
    wait_until("the download to start", || r.feed.downloads() == 1);
    assert!(!r.engine.install(Some(token), &|| {}));
    latch.open();
    assert!(worker.join().expect("worker"));
}

#[test]
fn no_check_runs_while_an_install_runs() {
    let latch = Latch::new();
    let r = live(offered("1.2.0").hold_download(&latch));
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    assert_eq!((r.feed.checks(), r.engine.calls().checks), (1, 1));
    let engine = r.engine.clone();
    let worker = thread::spawn(move || engine.install(Some(token), &|| {}));
    wait_until("the download to start", || r.feed.downloads() == 1);
    r.engine.tick();
    assert_eq!((r.feed.checks(), r.engine.calls().checks), (1, 1));
    latch.open();
    assert!(worker.join().expect("worker"));
    r.engine.tick();
    assert_eq!((r.feed.checks(), r.engine.calls().checks), (2, 2));
}

#[test]
fn a_recheck_of_the_same_version_keeps_the_offer_and_its_token() {
    let r = live(offered("1.2.0"));
    r.engine.tick();
    let first = r.engine.offer().expect("offer");
    r.engine.tick();
    assert_eq!(lock(&r.rec.offers).len(), 1);
    assert_eq!(r.engine.offer().expect("offer").token, first.token);
    assert!(r.engine.install(Some(first.token), &|| {}));
}

#[test]
fn the_install_hands_the_feed_the_offered_version() {
    let r = live(offered("1.2.0"));
    r.engine.tick();
    assert_eq!(r.engine.offer().expect("offer").token, 1);
    r.feed.next_offering("1.3.0", "newer");
    r.engine.tick();
    assert_eq!(r.engine.offer().expect("offer").token, 2);
    assert!(!r.engine.install(Some(1), &|| {}));
    assert!(r.engine.install(Some(2), &|| {}));
    assert_eq!(r.engine.offer().expect("offer").version, "1.3.0");
    assert_eq!(r.feed.downloads(), 1);
    assert_eq!(r.feed.refused(), 0);
}

/// Review fix: a check already on the wire when the install starts must finish before the install
/// reads the offer. Otherwise the check swaps the feed's stored find mid-install: the apply of the
/// offered version is refused, the discard removes the NEW find's files and leaves the downloaded
/// package behind, and the window gets a new offer in the middle of the run.
#[test]
fn an_install_waits_for_a_check_already_in_flight() {
    let latch = Latch::new();
    let r = live(
        offered("1.2.0")
            .slow_check(Duration::from_millis(400))
            .hold_download(&latch),
    );
    r.engine.tick();
    let token = r.engine.offer().expect("offer").token;
    r.feed.next_offering("1.3.0", "newer");
    let checker = {
        let engine = r.engine.clone();
        thread::spawn(move || engine.tick())
    };
    wait_until("the second check to start", || r.feed.checks() == 2);
    let installer = {
        let engine = r.engine.clone();
        thread::spawn(move || engine.install(Some(token), &|| {}))
    };
    checker.join().expect("checker");
    latch.open();
    // The offer changed under the user: the old token installs nothing and downloads nothing.
    assert!(!installer.join().expect("installer"));
    assert_eq!(r.feed.downloads(), 0, "nothing downloaded for a stale offer");
    assert_eq!(r.feed.refused(), 0, "the feed was never asked for a stale version");
    assert!(!r.feed.files_left());
    assert_eq!(r.engine.offer().expect("offer").version, "1.3.0");
}
