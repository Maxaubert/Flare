//! M9 Task 20: the stall watchdog and the work timer, on a fake clock and a manual tick.

use flare_diag::stall::*;
use flare_diag::writer::LineSink;
use serde_json::{json, Map, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Cap(Mutex<Vec<(String, Value)>>);

impl LineSink for Cap {
    fn emit(&self, _src: &'static str, kind: &str, _at: u64, fields: Map<String, Value>) {
        self.0
            .lock()
            .unwrap()
            .push((kind.to_owned(), Value::Object(fields)));
    }
}

impl Cap {
    fn kinds(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|l| l.0.clone()).collect()
    }
    fn of(&self, kind: &str) -> Vec<Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.0 == kind)
            .map(|l| l.1.clone())
            .collect()
    }
    fn all_text(&self) -> String {
        format!("{:?}", self.0.lock().unwrap())
    }
}

struct Rig {
    sink: Arc<Cap>,
    clock: Arc<ManualClock>,
    work: Arc<WorkTable>,
    crumbs: Arc<CrumbRing>,
}

fn rig() -> Rig {
    Rig {
        sink: Arc::new(Cap::default()),
        clock: Arc::new(ManualClock::new(0)),
        work: Arc::new(WorkTable::new()),
        crumbs: Arc::new(CrumbRing::new()),
    }
}

fn watch_with(
    r: &Rig,
    canary: Arc<dyn Fn() + Send + Sync>,
    sampler: Option<Arc<dyn StackSampler>>,
) -> StallWatch {
    let mut d = StallDeps::new(
        r.sink.clone(),
        r.work.clone(),
        r.clock.clone(),
        r.crumbs.clone(),
        canary,
    );
    d.sampler = sampler;
    StallWatch::new(d)
}

fn watch(r: &Rig) -> StallWatch {
    watch_with(r, Arc::new(|| {}), None)
}

fn timer(r: &Rig, verbose: bool) -> WorkTimer {
    WorkTimer::new(
        r.work.clone(),
        r.clock.clone(),
        r.sink.clone(),
        Arc::new(move || verbose),
    )
}

#[test]
fn logs_main_lag_when_the_beat_came_100_ms_or_more_late_with_the_calls_in_flight() {
    let r = rig();
    let w = watch(&r);
    w.beat();
    w.step();
    let id = r.work.begin("folder-size", WorkClass::Normal, 0);
    r.clock.set(260);
    w.beat();
    w.step();
    r.work.end(id, 260);
    let lags = r.sink.of("main-lag");
    assert_eq!(lags.len(), 1);
    assert_eq!(lags[0]["ms"], json!(210));
    assert_eq!(lags[0]["inflight"][0]["name"], json!("folder-size"));
    assert_eq!(lags[0]["inflight"][0]["ms"], json!(260));
}

#[test]
fn a_beat_inside_the_limit_writes_nothing() {
    let r = rig();
    let w = watch(&r);
    w.beat();
    w.step();
    r.clock.set(140);
    w.beat();
    w.step();
    assert!(r.sink.kinds().is_empty());
}

#[test]
fn logs_fs_slow_when_one_canary_took_500_ms_or_more_and_never_runs_two_at_once() {
    let r = rig();
    let slot: Arc<Mutex<Option<StallWatch>>> = Arc::new(Mutex::new(None));
    let nested = Arc::new(Mutex::new(Vec::new()));
    let (clock, slot2, nested2) = (r.clock.clone(), slot.clone(), nested.clone());
    let w = watch_with(
        &r,
        Arc::new(move || {
            clock.advance(600);
            let inner = slot2.lock().unwrap().clone().unwrap();
            nested2.lock().unwrap().push(inner.canary_once());
        }),
        None,
    );
    *slot.lock().unwrap() = Some(w.clone());
    assert!(w.canary_once());
    assert_eq!(*nested.lock().unwrap(), vec![false]);
    let slow = r.sink.of("fs-slow");
    assert_eq!(slow.len(), 1);
    assert_eq!(slow[0]["ms"], json!(600));
}

#[test]
fn says_nothing_about_a_quick_canary() {
    let r = rig();
    let clock = r.clock.clone();
    let w = watch_with(&r, Arc::new(move || clock.advance(499)), None);
    assert!(w.canary_once());
    assert!(r.sink.kinds().is_empty());
}

struct Frames;
impl StackSampler for Frames {
    fn sample(&self) -> Option<Vec<String>> {
        Some(vec!["flare+0x10".into(), "slint+0x20".into()])
    }
}

#[test]
fn writes_ui_stack_once_after_a_2_s_gap() {
    let r = rig();
    let w = watch_with(&r, Arc::new(|| {}), Some(Arc::new(Frames)));
    w.beat();
    w.step();
    r.clock.set(1999);
    w.step();
    assert!(r.sink.of("ui-stack").is_empty());
    r.clock.set(2000);
    w.step();
    r.clock.set(2500);
    w.step();
    let s = r.sink.of("ui-stack");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0]["ms"], json!(2000));
    assert_eq!(s[0]["stack"], json!("flare+0x10\nslint+0x20"));
}

#[test]
fn ui_stack_without_a_sampler_names_the_work() {
    let r = rig();
    let w = watch(&r);
    w.beat();
    r.work.begin("thumbs", WorkClass::Normal, 0);
    r.clock.set(2100);
    w.step();
    let s = r.sink.of("ui-stack");
    assert!(s[0].get("stack").is_none());
    assert_eq!(s[0]["inflight"][0]["name"], json!("thumbs"));
}

#[test]
fn a_stall_that_ends_before_two_seconds_writes_no_stack() {
    let r = rig();
    let w = watch(&r);
    w.beat();
    w.step();
    r.clock.set(1500);
    w.beat();
    w.step();
    r.clock.set(3000);
    w.beat();
    w.step();
    assert!(r.sink.of("ui-stack").is_empty());
}

#[test]
fn does_not_watch_a_ui_thread_that_has_never_beaten() {
    let r = rig();
    let w = watch(&r);
    r.clock.set(60_000);
    w.step();
    assert!(r.sink.kinds().is_empty());
}

#[test]
fn logs_nothing_for_a_sleep_not_a_lag_not_a_stack_not_a_slow_canary() {
    let r = rig();
    let clock = r.clock.clone();
    let w = watch_with(&r, Arc::new(move || clock.advance(10 * 60_000)), None);
    w.beat();
    w.step();
    w.power_suspend();
    r.clock.set(10 * 60_000);
    w.step();
    w.canary_once();
    w.power_resume();
    w.step();
    r.clock.advance(50);
    w.beat();
    w.step();
    assert!(r.sink.kinds().is_empty(), "{:?}", r.sink.kinds());
}

#[test]
fn logs_no_lag_while_the_screen_is_off_and_starts_over_when_it_is_back() {
    let r = rig();
    let w = watch(&r);
    w.beat();
    w.screen(false);
    // Windows slows the timers of a screen that is off: the beat comes every ~260 ms all night.
    for _ in 0..20 {
        r.clock.advance(263);
        w.beat();
        w.step();
    }
    r.clock.advance(5000);
    w.step();
    w.screen(true);
    w.step();
    r.clock.advance(50);
    w.beat();
    w.step();
    assert!(r.sink.kinds().is_empty(), "{:?}", r.sink.kinds());
    // Back on, a late beat is a lag again.
    r.clock.advance(300);
    w.beat();
    w.step();
    assert_eq!(r.sink.kinds(), vec!["main-lag"]);
}

#[test]
fn writes_at_most_twenty_lags_a_minute_and_the_next_says_how_many_were_held() {
    let r = rig();
    let w = watch(&r);
    w.beat();
    for _ in 0..30 {
        r.clock.advance(300);
        w.beat();
        w.step();
    }
    assert_eq!(r.sink.of("main-lag").len(), 20);
    r.clock.set(60_000);
    w.beat();
    r.clock.advance(300);
    w.beat();
    w.step();
    let lags = r.sink.of("main-lag");
    assert_eq!(lags.len(), 21);
    assert_eq!(lags[20]["held"], 10);
    assert!(lags[0].get("held").is_none());
}

#[test]
fn a_wake_with_the_screen_still_off_stays_quiet() {
    let r = rig();
    let w = watch(&r);
    w.beat();
    w.screen(false);
    w.power_suspend();
    w.power_resume();
    r.clock.advance(400);
    w.beat();
    w.step();
    assert!(r.sink.kinds().is_empty(), "{:?}", r.sink.kinds());
}

#[test]
fn writes_a_stack_when_the_window_is_hung() {
    let r = rig();
    let w = watch_with(&r, Arc::new(|| {}), Some(Arc::new(Frames)));
    w.hung(true);
    w.hung(true);
    r.clock.set(3000);
    w.hung(false);
    assert_eq!(r.sink.kinds(), vec!["unresponsive", "responsive"]);
    assert_eq!(
        r.sink.of("unresponsive")[0]["stack"],
        json!("flare+0x10\nslint+0x20")
    );
    assert_eq!(r.sink.of("responsive")[0]["ms"], json!(3000));
}

#[test]
fn ends_every_thread() {
    let r = rig();
    let w = watch(&r);
    let h = w.clone().spawn();
    w.stop();
    let t = Instant::now();
    h.join().unwrap();
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn frame_slow_carries_the_last_five_crumbs() {
    let r = rig();
    for (i, name) in ["a", "b", "c", "d", "e", "f", "g"].iter().enumerate() {
        r.crumbs.push(1000 + i as u64 * 10, name, Map::new());
    }
    r.crumbs.push(5000, "during", Map::new());
    let w = watch(&r);
    r.clock.set(1500);
    w.frame(199);
    assert!(r.sink.of("frame-slow").is_empty());
    w.frame(300); // started at 1200
    let f = &r.sink.of("frame-slow")[0];
    assert_eq!(f["ms"], json!(300));
    let names: Vec<&str> = f["crumbs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["a"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["c", "d", "e", "f", "g"]);
    assert_eq!(f["crumbs"][4]["ago"], json!(1200 - 1060));
}

#[test]
fn logs_a_work_item_over_500_ms_with_its_name_and_time() {
    let r = rig();
    let t = timer(&r, false);
    let c = r.clock.clone();
    let out = t.time("folder-size", false, WorkClass::Normal, || {
        c.advance(500);
        7
    });
    assert_eq!(out, 7);
    let c2 = r.clock.clone();
    t.time("quick", false, WorkClass::Normal, || c2.advance(499));
    let s = r.sink.of("work-slow");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0]["name"], json!("folder-size"));
    assert_eq!(s[0]["ms"], json!(500));
    assert_eq!(s[0]["sync"], json!(false));
}

#[test]
fn quiet_is_silent_verbose_logs_every_item() {
    let r = rig();
    timer(&r, false).time("a", false, WorkClass::Normal, || ());
    assert!(r.sink.kinds().is_empty());
    timer(&r, true).time("a", false, WorkClass::Normal, || ());
    let w = r.sink.of("work");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["name"], json!("a"));
}

#[test]
fn never_times_the_log_itself() {
    let r = rig();
    let t = timer(&r, true);
    let c = r.clock.clone();
    t.time("diag", false, WorkClass::Normal, || c.advance(900));
    assert!(r.sink.kinds().is_empty());
}

#[test]
fn a_panic_in_a_work_item_is_logged_with_its_stack_and_propagates() {
    let r = rig();
    let t = timer(&r, false);
    let res = catch_unwind(AssertUnwindSafe(|| {
        t.time("boom", false, WorkClass::Normal, || -> u8 {
            panic!("it broke")
        })
    }));
    let payload = res.unwrap_err();
    assert_eq!(*payload.downcast_ref::<&str>().unwrap(), "it broke");
    let e = &r.sink.of("work-error")[0];
    assert_eq!(e["name"], json!("boom"));
    assert!(e["msg"].as_str().unwrap().contains("it broke"));
    assert!(e["stack"].is_string());
    assert!(r.work.inflight(0, 0).is_empty());
}

#[test]
fn a_sync_item_over_100_ms_is_slow() {
    let r = rig();
    let t = timer(&r, false);
    let c = r.clock.clone();
    t.time("stat", true, WorkClass::Normal, || c.advance(100));
    let c = r.clock.clone();
    t.time("async-stat", false, WorkClass::Normal, || c.advance(100));
    let s = r.sink.of("work-slow");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0]["sync"], json!(true));
}

#[test]
fn inflight_is_longest_first() {
    let t = WorkTable::new();
    t.begin("young", WorkClass::Normal, 900);
    t.begin("old", WorkClass::Normal, 100);
    t.begin("middle", WorkClass::Normal, 500);
    let names: Vec<String> = t.inflight(0, 1000).into_iter().map(|i| i.name).collect();
    assert_eq!(names, vec!["old", "middle", "young"]);
}

#[test]
fn an_item_that_ended_inside_the_lag_is_named_done() {
    let t = WorkTable::new();
    let id = t.begin("term-spawn", WorkClass::Normal, 0);
    t.end(id, 1837);
    let list = t.inflight(2000, 1840);
    assert_eq!(
        list,
        vec![Inflight {
            name: "term-spawn".into(),
            ms: 1837,
            done: true
        }]
    );
    assert!(t.inflight(2, 1840).is_empty());
    assert!(t.inflight(0, 1840).is_empty());
}

#[test]
fn waits_are_never_slow_and_never_in_inflight() {
    let r = rig();
    let t = timer(&r, false);
    let (c, work) = (r.clock.clone(), r.work.clone());
    t.time("folder-picker", false, WorkClass::Waits, || {
        c.advance(60_000);
        assert!(work.inflight(0, 60_000).is_empty());
    });
    assert!(r.sink.kinds().is_empty());
    assert!(r.work.inflight(100_000, 60_000).is_empty());
}

#[test]
fn opaque_items_log_sizes_only() {
    let r = rig();
    let t = timer(&r, false);
    let res = catch_unwind(AssertUnwindSafe(|| {
        t.time("input", false, WorkClass::Opaque, || -> u8 {
            panic!("hunter2 typed text")
        })
    }));
    assert!(res.is_err());
    let e = &r.sink.of("work-error")[0];
    assert_eq!(e["msg"], json!("(18 chars)"));
}

#[test]
fn a_clipboard_copy_is_never_written_down() {
    let r = rig();
    let t = timer(&r, true);
    let secret = String::from("MARKER-secret-token-176");
    let c = r.clock.clone();
    t.time("clipboard-write", false, WorkClass::Opaque, || {
        let _copied = secret.clone();
        c.advance(900);
    });
    let res = catch_unwind(AssertUnwindSafe(|| {
        t.time("clipboard-term", false, WorkClass::Opaque, || -> u8 {
            panic!("{}", "MARKER-secret-token-176")
        })
    }));
    assert!(res.is_err());
    assert!(
        !r.sink.all_text().contains("MARKER-secret"),
        "{}",
        r.sink.all_text()
    );
}

#[test]
fn the_watchdog_takes_no_lock_the_ui_thread_holds() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/stall.rs"))
        .unwrap()
        .replace("\r\n", "\n"); // LF whatever the checkout's line endings are
    let start = src.find("pub fn beat(&self)").expect("beat exists");
    let body = &src[start..];
    let end = body.find("\n    }\n").expect("beat ends");
    assert!(!body[..end].contains("lock("), "beat must not lock");
    assert!(!body[..end].contains("wait"), "beat must not wait");

    let r = rig();
    let w = watch(&r);
    w.beat();
    let stop = Arc::new(AtomicBool::new(false));
    let stepper = {
        let (w, stop) = (w.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                w.step();
            }
        })
    };
    let beaters: Vec<_> = (0..8)
        .map(|_| {
            let w = w.clone();
            std::thread::spawn(move || (0..2000).for_each(|_| w.beat()))
        })
        .collect();
    for b in beaters {
        b.join().unwrap();
    }
    stop.store(true, Ordering::SeqCst);
    stepper.join().unwrap();
}
