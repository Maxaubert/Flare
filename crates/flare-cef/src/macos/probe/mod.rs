//! The engine tests' browser process. It runs inside the dev bundle, starts CEF headless, runs one scenario and
//! writes a JSON report (`FLARE_CEF_PROBE_OUT`); the test (`tests/engine_macos.rs`) builds the bundle, starts it
//! and reads the report. The scenario comes from `FLARE_CEF_PROBE_SCENARIO` (environment, not arguments: CEF owns
//! the command line).
//!
//! - `paint`: a page with a known colour comes out of `on_paint` as pixels.
//! - `cdp`: the real `flare_surfaces::agent_job::Job` (the state machine Windows runs) drives look, click, type,
//!   press, scroll, wait and eval over `SendDevToolsMessage`; every one of the 11 DevTools methods it uses answers
//!   in Chromium's shapes.
//! - `runaway`: `Runtime.evaluate`'s timeout stops `while(true){}`, a page function that never returns and a page
//!   getter set as a trap, and the page answers again afterwards with its state kept (the gap that ruled out
//!   WKWebView).
//!
//! Every report carries the window server's count of this process's on-screen windows at the start and the end,
//! the activation policy and whether the app ever became active.

mod pages;
mod server;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use flare_surfaces::agent_job::{Ids, Job, JobError, Op, Out, Page, RefMap, Target};
use flare_surfaces::agent_web::{Button, WaitFor, parse_key};
use serde_json::{Value, json};

use crate::macos::engine::{Engine, EngineConfig};
use crate::macos::view::{View, ViewSize};
use crate::macos::{app, screen};

/// The 11 DevTools methods `agent_job` sends (the spike's shim answered exactly these).
pub const AGENT_METHODS: [&str; 11] = [
    "DOM.enable",
    "Page.getLayoutMetrics",
    "Accessibility.getFullAXTree",
    "Runtime.evaluate",
    "Page.captureScreenshot",
    "DOM.getContentQuads",
    "DOM.scrollIntoViewIfNeeded",
    "DOM.focus",
    "Input.dispatchMouseEvent",
    "Input.dispatchKeyEvent",
    "Input.insertText",
];

/// The view every scenario uses: the spike's 983x700 at device scale 2.
const SIZE: ViewSize = ViewSize {
    width: 983,
    height: 700,
    scale: 2.0,
};

/// The report: named checks with their evidence, plus the run's facts.
pub struct Report {
    pub v: Value,
}

impl Report {
    fn new(scenario: &str) -> Self {
        Report {
            v: json!({"scenario": scenario, "checks": [], "notes": []}),
        }
    }

    fn check(&mut self, id: &str, ok: bool, what: &str, evidence: Value) {
        if let Some(a) = self.v["checks"].as_array_mut() {
            a.push(json!({"id": id, "ok": ok, "what": what, "evidence": evidence}));
        }
    }

    fn note(&mut self, s: impl Into<String>) {
        if let Some(a) = self.v["notes"].as_array_mut() {
            a.push(Value::String(s.into()));
        }
    }

    fn set(&mut self, k: &str, v: Value) {
        self.v[k] = v;
    }
}

fn ms_since(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Drives `agent_job` jobs against one view, the way the window will: every call of an `Out::Calls` goes out at
/// once, answers are fed back as they come, the clock ticks between pumps.
struct Driver<'a> {
    engine: &'a Engine,
    view: &'a View,
    counter: u64,
    /// The first answer of each method, parsed (the shapes are checked on these).
    raw: BTreeMap<String, Result<Value, String>>,
    /// Every method sent, and every one that failed.
    sent: BTreeSet<String>,
    failed: BTreeMap<String, String>,
}

impl<'a> Driver<'a> {
    fn new(engine: &'a Engine, view: &'a View) -> Self {
        Driver {
            engine,
            view,
            counter: 1,
            raw: BTreeMap::new(),
            sent: BTreeSet::new(),
            failed: BTreeMap::new(),
        }
    }

    fn page(&self) -> (String, String) {
        let s = self.view.state();
        (s.url.clone(), s.title.clone())
    }

    /// Runs one job to its end (or 60 s); its result, the refs a look handed out, and how long it took.
    fn run(&mut self, op: Op, refs: &RefMap) -> (Result<Value, JobError>, Option<RefMap>, u64) {
        let t0 = Instant::now();
        let (job, out) = Job::start(op, refs, 0, &mut Ids(&mut self.counter));
        let Some(mut job) = job else {
            let r = match out {
                Out::Done(r) => r,
                _ => Err(Job::interrupted()),
            };
            return (r, None, ms_since(t0));
        };
        job.set_glide_ms(0);
        let mut in_flight: HashMap<u64, String> = HashMap::new();
        let mut queue: Vec<Out> = vec![out];
        loop {
            while let Some(o) = queue.pop() {
                match o {
                    Out::Done(r) => return (r, job.take_refs(), ms_since(t0)),
                    Out::Idle => {}
                    Out::Calls(calls) => {
                        for c in calls {
                            self.sent.insert(c.method.clone());
                            match self.view.cdp_send(c.id, &c.method, &c.params) {
                                Ok(()) => {
                                    in_flight.insert(c.id, c.method);
                                }
                                Err(e) => {
                                    let (url, title) = self.page();
                                    let page = Page {
                                        url: &url,
                                        title: &title,
                                    };
                                    let o = job.on_reply(
                                        c.id,
                                        Err(e),
                                        page,
                                        ms_since(t0),
                                        &mut Ids(&mut self.counter),
                                    );
                                    queue.push(o);
                                }
                            }
                        }
                    }
                }
            }
            if job.take_point().is_some() {
                job.glide_for(0, ms_since(t0));
            }
            self.engine.pump();
            let ids: Vec<u64> = in_flight.keys().copied().collect();
            for id in ids {
                let Some(answer) = self.view.cdp_take(id) else {
                    continue;
                };
                let method = in_flight.remove(&id).unwrap_or_default();
                if let Err(e) = &answer {
                    self.failed
                        .entry(method.clone())
                        .or_insert_with(|| e.clone());
                }
                self.raw.entry(method).or_insert_with(|| {
                    answer
                        .clone()
                        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
                });
                let (url, title) = self.page();
                let page = Page {
                    url: &url,
                    title: &title,
                };
                let o = job.on_reply(id, answer, page, ms_since(t0), &mut Ids(&mut self.counter));
                queue.push(o);
            }
            if queue.is_empty() {
                queue.push(job.on_tick(ms_since(t0), &mut Ids(&mut self.counter)));
            }
            if ms_since(t0) > 60_000 {
                return (
                    Err(JobError {
                        fail: flare_surfaces::agent_job::Fail::Timeout,
                        message: "the probe gave up after 60 s".into(),
                    }),
                    None,
                    ms_since(t0),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// One raw DevTools call outside any job (the probe's own questions); its parsed answer.
    fn raw_call(&mut self, method: &str, params: Value, wait_ms: u64) -> Result<Value, String> {
        self.counter += 1;
        let id = self.counter;
        self.view.cdp_send(id, method, &params)?;
        let mut got = None;
        let view = self.view;
        self.engine.pump_until(wait_ms, || {
            got = view.cdp_take(id);
            got.is_some()
        });
        match got {
            None => Err(format!("{method}: no answer in {wait_ms} ms")),
            Some(r) => r.and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string())),
        }
    }

    /// The value of an expression in the page's world (a probe question, 3 s timeout).
    fn js(&mut self, expr: &str) -> Result<Value, String> {
        let v = self.raw_call(
            "Runtime.evaluate",
            json!({"expression": expr, "returnByValue": true, "timeout": 3000}),
            5_000,
        )?;
        if let Some(ex) = v.get("exceptionDetails") {
            return Err(format!("threw: {ex}"));
        }
        Ok(v.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }
}

fn names(look: &Value) -> Vec<String> {
    look["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            format!(
                "{} {} \"{}\"",
                e["ref"].as_str().unwrap_or(""),
                e["role"].as_str().unwrap_or(""),
                e["name"].as_str().unwrap_or("")
            )
        })
        .collect()
}

/// The ref of the element with this name, a control before plain text of the same words.
fn ref_named(look: &Value, name: &str) -> Option<String> {
    let all = look["elements"].as_array()?;
    all.iter()
        .find(|e| e["name"].as_str() == Some(name) && e["role"] != "text")
        .or_else(|| all.iter().find(|e| e["name"].as_str() == Some(name)))
        .and_then(|e| e["ref"].as_str().map(String::from))
}

/// A look's answer without the picture's base64 (kept small in the report), with the picture's size.
fn brief_look(r: &Result<Value, JobError>) -> Value {
    match r {
        Ok(v) => {
            let mut v = v.clone();
            if let Some(o) = v.get_mut("image").and_then(Value::as_object_mut)
                && let Some(d) = o.remove("data")
            {
                o.insert("data_len".into(), json!(d.as_str().map_or(0, str::len)));
            }
            v
        }
        Err(e) => json!({"error": e.message, "fail": format!("{:?}", e.fail)}),
    }
}

fn job_text(r: &Result<Value, JobError>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"error": e.message, "fail": format!("{:?}", e.fail)}),
    }
}

/// Opens a view on `url` and pumps until its main frame loaded (and painted once).
fn open(engine: &Engine, url: &str, rep: &mut Report) -> Result<View, String> {
    let view = View::open(url, SIZE)?;
    let t = Instant::now();
    let loaded = engine.pump_until(15_000, || {
        let s = view.state();
        !s.loading && !s.loads.is_empty() && s.paints > 0
    });
    rep.note(format!("loaded and painted after {} ms", ms_since(t)));
    if !loaded {
        let s = view.state();
        return Err(format!(
            "{url} did not load in 15 s (loads {:?}, errors {:?}, paints {})",
            s.loads, s.load_errors, s.paints
        ));
    }
    rep.note(format!("opened {url}"));
    Ok(view)
}

/// Pumps until a page with this title is loaded.
fn navigate(engine: &Engine, view: &View, url: &str, title: &str) -> bool {
    let before = view.state().loads.len();
    view.navigate(url);
    engine.pump_until(15_000, || {
        let s = view.state();
        !s.loading && s.loads.len() > before && s.title == title
    })
}

// ---- scenarios ---------------------------------------------------------------------------------------------

fn paint(engine: &Engine, srv: &server::Server, rep: &mut Report) -> Result<(), String> {
    let view = open(engine, &srv.url("/paint"), rep)?;
    // A paint after the square is laid out.
    let before = view.state().paints;
    engine.pump_until(3_000, || view.state().paints > before);
    let s = view.state();
    let p = s.paint.clone().unwrap_or_default();
    let w = u32::try_from(SIZE.width).unwrap_or(0);
    let h = u32::try_from(SIZE.height).unwrap_or(0);
    let scale = 2;
    let sq = p.rgba_at(100 * scale, 50 * scale);
    let white = p.rgba_at(600 * scale, 400 * scale);
    rep.check(
        "paint.buffer",
        p.width == w * scale
            && p.height == h * scale
            && p.bgra.len() == (p.width * p.height * 4) as usize,
        "on_paint hands the whole view as device pixels (983x700 CSS at scale 2 is 1966x1400 BGRA)",
        json!({"width": p.width, "height": p.height, "bytes": p.bgra.len(), "paints": s.paints}),
    );
    rep.check(
        "paint.pixels",
        sq == Some((0x33, 0x66, 0xcc, 0xff)) && white == Some((0xff, 0xff, 0xff, 0xff)),
        "the page's #3366cc square and its white ground are in the buffer where the page put them",
        json!({"square": format!("{sq:?}"), "ground": format!("{white:?}")}),
    );
    rep.check(
        "paint.no-popup-or-window",
        s.popup_paints == 0 && s.refused == Default::default(),
        "nothing asked for a popup layer, a window, a menu or a dialog",
        json!({"popup_paints": s.popup_paints, "refused": format!("{:?}", s.refused)}),
    );
    drop(s);
    view.close();
    engine.pump_for(300);
    Ok(())
}

fn shape(raw: &BTreeMap<String, Result<Value, String>>, m: &str) -> (bool, Value) {
    let Some(Ok(v)) = raw.get(m) else {
        return (false, json!(format!("{:?}", raw.get(m))));
    };
    let num = |p: &str| v.pointer(p).is_some_and(Value::is_number);
    let empty = v.as_object().is_some_and(serde_json::Map::is_empty);
    match m {
        "Page.getLayoutMetrics" => (
            num("/cssLayoutViewport/clientWidth")
                && num("/cssLayoutViewport/clientHeight")
                && num("/cssVisualViewport/clientWidth")
                && num("/cssVisualViewport/pageY")
                && num("/cssContentSize/height"),
            json!({"cssLayoutViewport": v["cssLayoutViewport"], "cssVisualViewport": v["cssVisualViewport"], "cssContentSize": v["cssContentSize"]}),
        ),
        "Accessibility.getFullAXTree" => {
            let nodes = v["nodes"].as_array().cloned().unwrap_or_default();
            let ok = !nodes.is_empty()
                && nodes[0]["nodeId"].is_string()
                && nodes[0]
                    .pointer("/role/value")
                    .is_some_and(Value::is_string)
                && nodes.iter().any(|n| n["backendDOMNodeId"].is_number())
                && nodes
                    .iter()
                    .any(|n| n.pointer("/name/value").is_some_and(Value::is_string));
            (ok, json!({"nodes": nodes.len(), "first": nodes.first()}))
        }
        "Runtime.evaluate" => (
            v.pointer("/result/type").is_some_and(Value::is_string),
            v.clone(),
        ),
        "Page.captureScreenshot" => {
            let data = v["data"].as_str().unwrap_or("");
            let size = flare_surfaces::agent_web::png_size_b64(data);
            (
                size.is_some(),
                json!({"data_len": data.len(), "png_size": format!("{size:?}")}),
            )
        }
        "DOM.getContentQuads" => {
            let q = v["quads"].as_array().cloned().unwrap_or_default();
            let ok = !q.is_empty()
                && q.iter().all(|q| {
                    q.as_array()
                        .is_some_and(|a| a.len() == 8 && a.iter().all(Value::is_number))
                });
            (ok, v.clone())
        }
        // The rest answer an empty object in Chromium.
        _ => (empty, v.clone()),
    }
}

fn cdp(engine: &Engine, srv: &server::Server, rep: &mut Report) -> Result<(), String> {
    let view = open(engine, &srv.url("/main"), rep)?;
    let mut d = Driver::new(engine, &view);

    // browser_look
    let (look, refs, look_ms) = d.run(Op::Look { picture: true }, &RefMap::default());
    let refs = refs.unwrap_or_default();
    let lv = look.as_ref().cloned().unwrap_or(Value::Null);
    let shot = lv
        .pointer("/image/data")
        .and_then(Value::as_str)
        .unwrap_or("");
    let png = flare_surfaces::agent_web::png_size_b64(shot);
    rep.check(
        "cdp.look",
        look.is_ok()
            && ref_named(&lv, "Press me").is_some()
            && ref_named(&lv, "Name").is_some()
            && ref_named(&lv, "Agree").is_some()
            && lv.pointer("/viewport/device_scale").and_then(Value::as_f64) == Some(2.0)
            && png.is_some(),
        "browser_look: the numbered list, the device scale and the picture",
        json!({"ms": look_ms, "same_origin_frame_listed": ref_named(&lv, "Frame button").is_some(), "names": names(&lv), "viewport": lv["viewport"], "png": format!("{png:?}"),
               "look": brief_look(&look)}),
    );

    // browser_click by ref
    let b1 = ref_named(&lv, "Press me").unwrap_or_default();
    let (click, _, _) = d.run(
        Op::Click {
            target: Target::Ref(b1),
            button: Button::Left,
            double: false,
        },
        &refs,
    );
    let out = d.js("document.getElementById('out').textContent");
    rep.check(
        "cdp.click",
        click.is_ok() && out.as_ref().ok() == Some(&json!("clicked b1 true")),
        "browser_click by ref: a trusted click lands on the button",
        json!({"job": job_text(&click), "out": format!("{out:?}")}),
    );

    // browser_type into a ref
    let t1 = ref_named(&lv, "Name").unwrap_or_default();
    let typed = "héllo 日本 🙂";
    let (ty, _, _) = d.run(
        Op::Type {
            text: typed.into(),
            target: Some(t1),
        },
        &refs,
    );
    let val = d.js("document.getElementById('t1').value");
    let focused = d.js("document.activeElement && document.activeElement.id");
    rep.check(
        "cdp.type",
        ty.is_ok() && val.as_ref().ok() == Some(&json!(typed)),
        "browser_type into a ref: focus, then insertText (accents, CJK, emoji)",
        json!({"job": job_text(&ty), "value": format!("{val:?}"), "focused": format!("{focused:?}")}),
    );

    // browser_press
    let key = parse_key("Enter").map_err(|e| e.to_string())?;
    let (pr, _, _) = d.run(Op::Press { key }, &refs);
    let log = d.js(
        "JSON.stringify(window.__log.filter(function(e){return e.t==='keydown'||e.t==='keyup'}))",
    );
    let keys: Vec<Value> = log
        .as_ref()
        .ok()
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let trusted_enter = keys
        .iter()
        .any(|e| e["t"] == "keydown" && e["k"] == "Enter" && e["c"] == "Enter" && e["tr"] == true);
    rep.check(
        "cdp.press",
        pr.is_ok() && trusted_enter,
        "browser_press Enter: a trusted keydown with Windows' key and code",
        json!({"job": job_text(&pr), "keys": keys}),
    );

    // browser_scroll (no target: the middle of the page)
    let (sc, _, _) = d.run(
        Op::Scroll {
            dx: 0.0,
            dy: 600.0,
            target: None,
        },
        &refs,
    );
    engine.pump_for(400);
    let y = d.js("window.scrollY");
    rep.check(
        "cdp.scroll",
        sc.is_ok()
            && y.as_ref()
                .ok()
                .and_then(Value::as_f64)
                .is_some_and(|y| y >= 500.0),
        "browser_scroll dy 600: a wheel event scrolls the page",
        json!({"job": job_text(&sc), "scrollY": format!("{y:?}")}),
    );

    // browser_wait (text there) and an unmet one
    let (w1, _, _) = d.run(
        Op::Wait {
            what: WaitFor::Text("The bottom line".into()),
            timeout_ms: 3_000,
        },
        &refs,
    );
    let (w2, _, w2_ms) = d.run(
        Op::Wait {
            what: WaitFor::Text("never on this page".into()),
            timeout_ms: 1_000,
        },
        &refs,
    );
    rep.check(
        "cdp.wait",
        w1.as_ref().is_ok_and(|v| v["met"] == true) && w2.as_ref().is_ok_and(|v| v["met"] == false),
        "browser_wait: text there is met, text never there is not met after its timeout",
        json!({"met": job_text(&w1), "unmet": job_text(&w2), "unmet_ms": w2_ms}),
    );

    // browser_eval: a value, a promise, a throw
    let (e1, _, _) = d.run(
        Op::Eval {
            expression: "document.title".into(),
        },
        &refs,
    );
    let (e2, _, _) = d.run(
        Op::Eval {
            expression: "new Promise(function(r){setTimeout(function(){r({a:1,b:[2,3]})},50)})"
                .into(),
        },
        &refs,
    );
    let (e3, _, _) = d.run(
        Op::Eval {
            expression: "(function(){throw new Error('boom')})()".into(),
        },
        &refs,
    );
    rep.check(
        "cdp.eval",
        e1.as_ref()
            .is_ok_and(|v| v.to_string().contains("Engine main"))
            && e2.as_ref().is_ok_and(|v| v.to_string().contains("\"b\""))
            && e3.as_ref().is_err_and(|e| e.message.contains("boom")),
        "browser_eval: a value, an awaited promise with an object, a throw",
        json!({"value": job_text(&e1), "promise": job_text(&e2), "throw": job_text(&e3)}),
    );

    // The 11 methods, each sent, each answered, each in Chromium's shape.
    let mut shapes = serde_json::Map::new();
    let mut all_ok = true;
    for m in AGENT_METHODS {
        let (ok, ev) = shape(&d.raw, m);
        let sent = d.sent.contains(m);
        all_ok &= ok && sent && !d.failed.contains_key(m);
        shapes.insert(
            m.into(),
            json!({"sent": sent, "shape_ok": ok, "answer": ev}),
        );
    }
    let extra: Vec<&String> = d
        .sent
        .iter()
        .filter(|m| !AGENT_METHODS.contains(&m.as_str()))
        .collect();
    rep.check(
        "cdp.eleven-methods",
        all_ok && extra.is_empty(),
        "agent_job's 11 DevTools methods all went through SendDevToolsMessage and answered in Chromium's shapes",
        json!({"methods": shapes, "failed": d.failed, "other_methods": extra}),
    );
    let s = view.state();
    rep.check(
        "cdp.nothing-native",
        s.refused.popups == 0 && s.refused.context_menus == 0 && s.popup_paints == 0,
        "no popup, menu or popup layer on the way",
        json!({"refused": format!("{:?}", s.refused), "popup_paints": s.popup_paints, "events": s.cdp_events}),
    );
    drop(s);
    view.close();
    engine.pump_for(300);
    Ok(())
}

/// One runaway: the job's answer and time, then whether the page answers (its state) and a look works.
fn runaway_case(d: &mut Driver<'_>, expression: &str, state_expr: &str) -> Value {
    let (r, _, ms) = d.run(
        Op::Eval {
            expression: expression.into(),
        },
        &RefMap::default(),
    );
    let t = Instant::now();
    let after = d.js(state_expr);
    let after_ms = ms_since(t);
    let (look, _, look_ms) = d.run(Op::Look { picture: true }, &RefMap::default());
    json!({
        "job": job_text(&r),
        "job_ms": ms,
        "state_after": format!("{after:?}"),
        "state_after_value": after.clone().unwrap_or(Value::Null),
        "state_after_ms": after_ms,
        "look_after_ok": look.is_ok(),
        "look_after_ms": look_ms,
        "look_after": names(look.as_ref().unwrap_or(&Value::Null)),
    })
}

fn runaway(engine: &Engine, srv: &server::Server, rep: &mut Report) -> Result<(), String> {
    let view = open(engine, &srv.url("/runaway"), rep)?;
    let mut d = Driver::new(engine, &view);
    let born = d.js("window.__state.born");
    // State the agent itself set, which must survive too.
    let set = d.js("window.__agentMark = 'kept'; window.__state.n += 1; window.__state.n");
    let loads_before = view.state().loads.len();

    let a = runaway_case(
        &mut d,
        "while(true){}",
        "[window.__state.n, window.__agentMark, window.__state.born]",
    );
    let ok = |c: &Value, want: &Value| {
        c["job"].get("error").is_some()
            && c["job_ms"].as_u64().is_some_and(|m| m < 15_000)
            && &c["state_after_value"] == want
            && c["state_after_ms"].as_u64().is_some_and(|m| m < 3_000)
            && c["look_after_ok"] == true
    };
    let want = json!([42, "kept", born.clone().unwrap_or(Value::Null)]);
    rep.check(
        "runaway.while-true",
        set.as_ref().ok() == Some(&json!(42)) && ok(&a, &want),
        "browser_eval of while(true){}: Runtime.evaluate's timeout stops it, the page answers again at once with its state (and the agent's mark), and a look works",
        a,
    );
    let b = runaway_case(
        &mut d,
        "hang()",
        "[window.__state.n, window.__agentMark, window.__state.born]",
    );
    rep.check(
        "runaway.page-function",
        ok(&b, &want),
        "browser_eval of a page function that never returns: stopped the same way, state kept",
        b,
    );
    let loads_after = view.state().loads.len();
    rep.check(
        "runaway.same-document",
        loads_after == loads_before,
        "no reload or new document was needed: the page that ran away is the page that answers",
        json!({"loads_before": loads_before, "loads_after": loads_after}),
    );

    // The trap: a page getter that never returns, for whatever the agent touches.
    let nav = navigate(engine, &view, &srv.url("/trap"), "Trap");
    let mut d = Driver::new(engine, &view);
    let (l1, _, l1_ms) = d.run(Op::Look { picture: true }, &RefMap::default());
    let (l2, _, l2_ms) = d.run(Op::Look { picture: true }, &RefMap::default());
    let (w, _, w_ms) = d.run(
        Op::Wait {
            what: WaitFor::Text("trap text".into()),
            timeout_ms: 3_000,
        },
        &RefMap::default(),
    );
    let l2v = l2.as_ref().cloned().unwrap_or(Value::Null);
    rep.check(
        "runaway.trap-look",
        nav && l1.is_ok() && l2.is_ok() && ref_named(&l2v, "Trap button").is_some()
            && w.as_ref().is_ok_and(|v| v["met"] == true),
        "a page whose devicePixelRatio getter never returns: two looks and a wait still answer (the fixed question's 2 s timeout stops the getter)",
        json!({"navigated": nav, "first_ms": l1_ms, "second_ms": l2_ms, "first": brief_look(&l1),
               "second_names": names(&l2v), "wait": job_text(&w), "wait_ms": w_ms}),
    );
    let c = runaway_case(&mut d, "window.devicePixelRatio", "window.__kept");
    rep.check(
        "runaway.trap-eval",
        ok(&c, &json!(7)),
        "browser_eval that runs into the page's trap: stopped by the timeout, the page answers again with its state",
        c,
    );
    let s = view.state();
    rep.check(
        "runaway.nothing-native",
        s.refused == Default::default() && s.popup_paints == 0,
        "nothing native was asked for on the way",
        json!({"refused": format!("{:?}", s.refused)}),
    );
    drop(s);
    view.close();
    engine.pump_for(300);
    Ok(())
}

/// The probe's `main`: the exit code.
pub fn main() -> i32 {
    let scenario = std::env::var("FLARE_CEF_PROBE_SCENARIO").unwrap_or_default();
    let out = std::env::var_os("FLARE_CEF_PROBE_OUT").map(PathBuf::from);
    let mut rep = Report::new(&scenario);
    rep.set("windows_at_start", json!(screen::windows_of_self()));
    let root = std::env::temp_dir().join(format!("flare-cef-probe-{}", std::process::id()));
    let cfg = EngineConfig {
        root_cache: root.clone(),
        log_file: std::env::var_os("FLARE_CEF_PROBE_LOG").map(PathBuf::from),
        schedule: None,
        adopt_app: false,
        net_log: None,
    };
    let write = |rep: &Report| {
        if let Some(p) = &out {
            let _ = std::fs::write(p, serde_json::to_string_pretty(&rep.v).unwrap_or_default());
        }
    };
    let engine = match Engine::start(&cfg) {
        Ok(e) => e,
        Err(e) => {
            rep.set("fatal", json!(e));
            write(&rep);
            return 2;
        }
    };
    rep.set("cef_app_protocol", json!(app::conforms(&engine.app)));
    rep.set(
        "cef_app_protocol_parts",
        json!(
            app::conformance(&engine.app)
                .into_iter()
                .collect::<BTreeMap<_, _>>()
        ),
    );
    rep.set(
        "activation_policy",
        json!(format!("{:?}", engine.app.activationPolicy())),
    );
    let t = Instant::now();
    let result = server::start().and_then(|srv| {
        let r = match scenario.as_str() {
            "paint" => paint(&engine, &srv, &mut rep),
            "cdp" => cdp(&engine, &srv, &mut rep),
            "runaway" => runaway(&engine, &srv, &mut rep),
            other => Err(format!("no scenario {other:?}")),
        };
        rep.set("requests", json!(srv.requests()));
        r
    });
    if let Err(e) = result {
        rep.set("fatal", json!(e));
    }
    rep.set("scenario_ms", json!(ms_since(t)));
    engine.pump_for(200);
    rep.set("active_at_end", json!(engine.app.isActive()));
    rep.set("windows_at_end", json!(screen::windows_of_self()));
    write(&rep);
    engine.shutdown();
    let _ = std::fs::remove_dir_all(&root);
    rep.set("shutdown", json!(true));
    write(&rep);
    0
}
