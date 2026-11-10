//! The `agent` scenario: the real `flare_surfaces::agent_job::Job` (the state machine the window runs on Windows)
//! drives a web panel of the real [`WebHost`] through [`WebBackend::cdp`], the way `web_ctl` does: every call of an
//! `Out::Calls` goes out at once, each `WebEvent::Cdp` goes back to the job, the clock ticks between turns. Each
//! agent tool is used on the spike's agent page and its effect read back from the page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use flare_surfaces::agent_job::{Fail, Ids, Job, JobError, Op, Out, Page, RefMap, Target};
use flare_surfaces::agent_web::{Button, WaitFor, parse_key, png_size_b64};
use serde_json::{Value, json};

use super::{BOUNDS, H, Report, pages};
use crate::backend::{KeyInput, KeyPress, Mods, Slot, WebBackend, WebEvent};

const TAB: &str = "ag";

fn ms_since(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The page's address and title as the events last said (what `web_ctl` hands a job).
fn page_of(h: &H) -> (String, String) {
    let mut url = String::new();
    let mut title = String::new();
    for e in &h.seen {
        match e {
            WebEvent::Url { tab, url: u } | WebEvent::SameDocument { tab, url: u }
                if tab == TAB =>
            {
                url = u.clone();
            }
            WebEvent::Title { tab, title: t } if tab == TAB => title = t.clone(),
            _ => {}
        }
    }
    (url, title)
}

struct Driver {
    counter: u64,
    /// Every DevTools method the jobs sent.
    sent: Vec<String>,
}

impl Driver {
    /// Runs one job to its end (or 60 s): its result, the refs a look handed out, how long it took.
    fn run(
        &mut self,
        h: &mut H,
        op: Op,
        refs: &RefMap,
    ) -> (Result<Value, JobError>, Option<RefMap>, u64) {
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
        let mut in_flight: HashMap<u64, ()> = HashMap::new();
        let mut queue: Vec<Out> = vec![out];
        loop {
            while let Some(o) = queue.pop() {
                match o {
                    Out::Done(r) => return (r, job.take_refs(), ms_since(t0)),
                    Out::Idle => {}
                    Out::Calls(calls) => {
                        for c in calls {
                            self.sent.push(c.method.clone());
                            in_flight.insert(c.id, ());
                            h.host.cdp(TAB, c.id, &c.method, &c.params.to_string());
                        }
                    }
                }
            }
            if job.take_point().is_some() {
                job.glide_for(0, ms_since(t0));
            }
            h.turn();
            // The answers, taken out of what the host reported, in the order they came.
            let mut answers = Vec::new();
            h.seen.retain(|e| match e {
                WebEvent::Cdp { tab, id, result } if tab == TAB && in_flight.contains_key(id) => {
                    answers.push((*id, result.clone()));
                    false
                }
                _ => true,
            });
            for (id, answer) in answers {
                in_flight.remove(&id);
                let (url, title) = page_of(h);
                let page = Page {
                    url: &url,
                    title: &title,
                };
                queue.push(job.on_reply(
                    id,
                    answer,
                    page,
                    ms_since(t0),
                    &mut Ids(&mut self.counter),
                ));
            }
            if queue.is_empty() {
                queue.push(job.on_tick(ms_since(t0), &mut Ids(&mut self.counter)));
            }
            if ms_since(t0) > 60_000 {
                return (
                    Err(JobError {
                        fail: Fail::Timeout,
                        message: "the probe gave up after 60 s".into(),
                    }),
                    None,
                    ms_since(t0),
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
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

/// The element with this name, a control before plain text of the same words.
fn named<'a>(look: &'a Value, name: &str) -> Option<&'a Value> {
    let all = look["elements"].as_array()?;
    all.iter()
        .find(|e| e["name"].as_str() == Some(name) && e["role"] != "text")
        .or_else(|| all.iter().find(|e| e["name"].as_str() == Some(name)))
}

fn ref_named(look: &Value, name: &str) -> Option<String> {
    named(look, name).and_then(|e| e["ref"].as_str().map(String::from))
}

fn job_text(r: &Result<Value, JobError>) -> Value {
    match r {
        Ok(v) => {
            let mut v = v.clone();
            if let Some(o) = v.get_mut("image").and_then(Value::as_object_mut)
                && let Some(d) = o.remove("data")
            {
                o.insert("data_len".into(), json!(d.as_str().map_or(0, str::len)));
            }
            json!({"ok": v})
        }
        Err(e) => json!({"error": e.message, "fail": format!("{:?}", e.fail)}),
    }
}

/// A page value read back through DevTools (a probe question, not the agent's).
fn js(h: &mut H, expr: &str) -> Value {
    h.eval(TAB, expr)
        .unwrap_or_else(|e| json!({"probe_error": e}))
}

/// A press of the agent's: its parsed key, run as `browser_press` does.
fn press(d: &mut Driver, h: &mut H, key: &str) -> Result<Value, JobError> {
    match parse_key(key) {
        Ok(k) => d.run(h, Op::Press { key: k }, &RefMap::default()).0,
        Err(e) => Err(JobError {
            fail: Fail::BadParams,
            message: e,
        }),
    }
}

/// The middle of a listed element's box (CSS pixels of the shown page).
fn centre(e: &Value) -> Option<(f64, f64)> {
    let c = &e["center"];
    Some((c[0].as_f64()?, c[1].as_f64()?))
}

fn frame_value(h: &mut H, frame: &str, expr: &str) -> Value {
    js(
        h,
        &format!(
            "(function(){{try{{var w=document.getElementById('{frame}').contentWindow;return {expr};}}catch(e){{return 'unreadable: '+e.name}}}})()"
        ),
    )
}

pub fn run(rep: &mut Report, data: &Path) -> Result<(), String> {
    let srv = pages::start()?;
    let mut h = H::new(&data.join("profile"), Vec::new());
    h.host.set_scale(2.0);
    h.host.set_downloads_dir(data.join("Downloads"));
    // The window's filter: the app takes Cmd+T (the backend's ctrl on a Mac). Every key it is asked about is kept.
    let asked: Rc<RefCell<Vec<KeyPress>>> = Rc::default();
    let a2 = Rc::clone(&asked);
    h.host.set_key_filter(Rc::new(move |k: &KeyPress| {
        a2.borrow_mut().push(*k);
        k.ctrl && k.vk == u32::from(b'T')
    }));
    let mut d = Driver {
        counter: 1,
        sent: Vec::new(),
    };
    let from = h.mark();
    h.host.open(TAB, &srv.url("/agent"), BOUNDS, true);
    let opened = h.opened(from, TAB, "Agent");
    h.wait(800);
    if !opened {
        return Err(format!("the agent page did not open: {:?}", h.since(from)));
    }

    // ---- browser_look: the numbered list, the device scale, the picture
    let (look, refs, look_ms) = d.run(&mut h, Op::Look { picture: true }, &RefMap::default());
    let refs = refs.unwrap_or_default();
    let lv = look.as_ref().cloned().unwrap_or(Value::Null);
    let png = lv
        .pointer("/image/data")
        .and_then(Value::as_str)
        .and_then(png_size_b64);
    rep.check(
        "agent.look",
        look.is_ok()
            && ["Press me", "Name", "Agree", "Notes", "Download report"]
                .iter()
                .all(|n| ref_named(&lv, n).is_some())
            && lv.pointer("/viewport/device_scale").and_then(Value::as_f64) == Some(2.0)
            && png.is_some_and(|(w, hh)| w > 0 && hh > 0),
        "browser_look through the host: the numbered list, the device scale and the picture",
        json!({"ms": look_ms, "names": names(&lv), "viewport": lv["viewport"], "png": format!("{png:?}")}),
    );

    // ---- browser_click by ref and by point
    let (click, _, _) = d.run(
        &mut h,
        Op::Click {
            target: Target::Ref(ref_named(&lv, "Press me").unwrap_or_default()),
            button: Button::Left,
            double: false,
        },
        &refs,
    );
    let out = js(&mut h, "document.getElementById('out').textContent");
    rep.check(
        "agent.click-ref",
        click.is_ok() && out == "clicked b1 true",
        "browser_click by ref: a trusted click lands on the button",
        json!({"job": job_text(&click), "out": out}),
    );
    let agree = named(&lv, "Agree").and_then(centre);
    let (pclick, _, _) = match agree {
        Some((x, y)) => d.run(
            &mut h,
            Op::Click {
                target: Target::Point(x, y),
                button: Button::Left,
                double: false,
            },
            &refs,
        ),
        None => (Err(Job::interrupted()), None, 0),
    };
    let checked = js(&mut h, "document.getElementById('c1').checked");
    rep.check(
        "agent.click-point",
        pclick.is_ok() && checked == true,
        "browser_click by point (a look's rect): the checkbox is checked",
        json!({"job": job_text(&pclick), "point": format!("{agree:?}"), "checked": checked}),
    );

    // ---- browser_type into a ref
    let typed = "héllo 日本 🙂";
    let (ty, _, _) = d.run(
        &mut h,
        Op::Type {
            text: typed.into(),
            target: ref_named(&lv, "Name"),
        },
        &refs,
    );
    let val = js(&mut h, "document.getElementById('t1').value");
    rep.check(
        "agent.type",
        ty.is_ok() && val == typed,
        "browser_type into a ref: focus, then the text as typed (accents, CJK, emoji)",
        json!({"job": job_text(&ty), "value": val}),
    );

    // ---- browser_press: the editing keys in a text area, and the agent's editing chords
    let _ = js(&mut h, "window.__log = []; 1");
    let (ty2, _, _) = d.run(
        &mut h,
        Op::Type {
            text: "abc".into(),
            target: ref_named(&lv, "Notes"),
        },
        &refs,
    );
    let mut steps = Vec::new();
    let ta = |h: &mut H| js(h, "document.getElementById('ta').value");
    let mut edit = |d: &mut Driver, h: &mut H, key: &str, then: Option<&str>| -> Value {
        let r = press(d, h, key);
        if let Some(t) = then {
            let _ = d.run(
                h,
                Op::Type {
                    text: t.into(),
                    target: None,
                },
                &RefMap::default(),
            );
        }
        let v = ta(h);
        steps.push(json!({"key": key, "then": then, "ok": r.is_ok(), "value": v}));
        v
    };
    let bs = edit(&mut d, &mut h, "Backspace", None);
    let left = edit(&mut d, &mut h, "ArrowLeft", Some("Z"));
    let home = edit(&mut d, &mut h, "Home", Some("H"));
    let end = edit(&mut d, &mut h, "End", Some("E"));
    let enter = edit(&mut d, &mut h, "Enter", Some("n"));
    let sel_ctrl = edit(&mut d, &mut h, "Control+a", Some("x"));
    let undo = edit(&mut d, &mut h, "Control+z", None);
    let _ = edit(&mut d, &mut h, "End", Some("q"));
    let sel_meta = edit(&mut d, &mut h, "Meta+a", Some("y"));
    let del = {
        let _ = edit(&mut d, &mut h, "Home", None);
        edit(&mut d, &mut h, "Delete", None)
    };
    let _ = d.run(
        &mut h,
        Op::Type {
            text: "one two".into(),
            target: None,
        },
        &RefMap::default(),
    );
    let word_bs = edit(&mut d, &mut h, "Control+Backspace", Some("three"));
    let word_left = edit(&mut d, &mut h, "Control+ArrowLeft", Some("X"));
    let undo2 = edit(&mut d, &mut h, "Control+z", None);
    let redo = edit(&mut d, &mut h, "Control+y", None);
    let keylog = js(
        &mut h,
        "JSON.stringify(window.__log.filter(function(e){return e.t==='keydown'}).map(function(e){return e.k+'/'+e.c+'/'+e.m+'/'+e.tr}))",
    );
    rep.check(
        "agent.press-editing",
        ty2.is_ok()
            && bs == "ab"
            && left == "aZb"
            && home == "HaZb"
            && end == "HaZbE"
            && enter == "HaZbE\nn"
            && sel_ctrl == "x"
            && undo == "HaZbE\nn"
            && sel_meta == "y"
            && del.as_str() == Some("")
            && word_bs == "one three"
            && word_left == "one Xthree"
            && undo2 == "one three"
            && redo == "one Xthree",
        "browser_press edits as on Windows: Backspace, the arrows, Home, End, Enter, Delete; Control+a selects all \
         (and Meta+a, the Mac's own), Control+z undoes and Control+y redoes, Control+Backspace takes a word and \
         Control+ArrowLeft moves by one (rule 1027)",
        json!({"steps": steps, "keydowns": keylog}),
    );

    // ---- browser_scroll
    let y0 = js(&mut h, "window.scrollY");
    let (sc, _, _) = d.run(
        &mut h,
        Op::Scroll {
            dx: 0.0,
            dy: 600.0,
            target: None,
        },
        &RefMap::default(),
    );
    h.wait(300);
    let y1 = js(&mut h, "window.scrollY");
    rep.check(
        "agent.scroll",
        sc.is_ok() && y1.as_f64().unwrap_or(0.0) > y0.as_f64().unwrap_or(0.0) + 300.0,
        "browser_scroll: a trusted wheel moves the page down",
        json!({"job": job_text(&sc), "before": y0, "after": y1}),
    );
    let _ = js(&mut h, "window.scrollTo(0,0); 1");
    h.wait(200);

    // ---- the frames: what the list holds, and use by point
    let (flook, frefs, _) = d.run(&mut h, Op::Look { picture: false }, &RefMap::default());
    let frefs = frefs.unwrap_or_default();
    let fv = flook.as_ref().cloned().unwrap_or(Value::Null);
    let listed = json!({
        "same_origin": ref_named(&fv, "Frame button").is_some(),
        "cross_origin": ref_named(&fv, "Cross button").is_some(),
        "sandboxed_no_scripts": ref_named(&fv, "Sandboxed link").is_some(),
        "frames_as_elements": names(&fv).into_iter().filter(|n| n.contains("frame")).collect::<Vec<_>>(),
    });
    let rect_js = |id: &str| {
        format!(
            "(function(){{var r=document.getElementById('{id}').getBoundingClientRect();return [r.left,r.top];}})()"
        )
    };
    let mut frames = Vec::new();
    let mut frames_ok = true;
    for (fid, what) in [
        ("fs", "same-origin"),
        ("fx", "cross-origin"),
        ("fo", "sandboxed same-origin, no scripts"),
    ] {
        let at = js(&mut h, &rect_js(fid));
        let (fx, fy) = (at[0].as_f64().unwrap_or(0.0), at[1].as_f64().unwrap_or(0.0));
        // The frame's button sits at its top left (2 px border, 4 px margin), its field to the right of it.
        let (c, _, _) = d.run(
            &mut h,
            Op::Click {
                target: Target::Point(fx + 30.0, fy + 16.0),
                button: Button::Left,
                double: false,
            },
            &frefs,
        );
        h.wait(200);
        let (fc, _, _) = d.run(
            &mut h,
            Op::Click {
                target: Target::Point(fx + 200.0, fy + 16.0),
                button: Button::Left,
                double: false,
            },
            &frefs,
        );
        let (ft, _, _) = d.run(
            &mut h,
            Op::Type {
                text: format!("über {fid}"),
                target: None,
            },
            &RefMap::default(),
        );
        h.wait(300);
        frames.push(json!({"frame": what, "at": at, "click": job_text(&c), "field_click": job_text(&fc), "type": job_text(&ft)}));
        frames_ok &= c.is_ok() && fc.is_ok() && ft.is_ok();
    }
    h.wait(300);
    let same = frame_value(
        &mut h,
        "fs",
        "w.document.getElementById('sb').textContent + '|' + w.document.getElementById('si').value",
    );
    let osb = frame_value(
        &mut h,
        "fo",
        "w.document.getElementById('sb').textContent + '|' + w.document.getElementById('si').value",
    );
    let cross_click = srv.hits("/fclick-x-true");
    let cross_typed = srv.hits("/ftype-x?v=%C3%BCber%20fx");
    rep.check(
        "agent.frames",
        frames_ok
            && same == "Frame clicked|über fs"
            && osb == "Frame button|über fo"
            && !cross_click.is_empty()
            && !cross_typed.is_empty(),
        "frames by point: a trusted click and typing land in a same-origin frame, a cross-origin frame (another \
         process) and a sandboxed one with no scripts (its button runs nothing, its field takes the text); the list \
         holds what Chromium's tree of the root frame holds, as on Windows",
        json!({"listed": listed, "frames": frames, "same": same, "sandboxed_same_origin": osb,
               "cross_click": cross_click, "cross_typed": cross_typed, "frame_requests": srv.hits("/f")}),
    );
    // A sandboxed frame with no scripts: its link, clicked by point, navigates that frame.
    let at = js(&mut h, &rect_js("fb"));
    let (bx, by) = (at[0].as_f64().unwrap_or(0.0), at[1].as_f64().unwrap_or(0.0));
    let p2_before = srv.hits("/page2").len();
    let (sbc, _, _) = d.run(
        &mut h,
        Op::Click {
            target: Target::Point(bx + 40.0, by + 14.0),
            button: Button::Left,
            double: false,
        },
        &frefs,
    );
    h.wait(1_000);
    let p2_after = srv.hits("/page2").len();
    rep.check(
        "agent.frame-sandboxed",
        sbc.is_ok() && p2_after > p2_before,
        "a sandboxed frame with no scripts: a click by point on its link navigates the frame",
        json!({"click": job_text(&sbc), "page2_requests": [p2_before, p2_after]}),
    );

    // ---- browser_wait
    let from = h.mark();
    h.host.navigate(TAB, &srv.url("/slow"));
    let slow = h.opened(from, TAB, "Slow page");
    let (w1, _, w1_ms) = d.run(
        &mut h,
        Op::Wait {
            what: WaitFor::Text("late text appears".into()),
            timeout_ms: 5_000,
        },
        &RefMap::default(),
    );
    let (w2, _, _) = d.run(
        &mut h,
        Op::Wait {
            what: WaitFor::Selector("#late".into()),
            timeout_ms: 2_000,
        },
        &RefMap::default(),
    );
    let (w3, _, w3_ms) = d.run(
        &mut h,
        Op::Wait {
            what: WaitFor::Text("never on this page".into()),
            timeout_ms: 600,
        },
        &RefMap::default(),
    );
    let met = |r: &Result<Value, JobError>| r.as_ref().ok().map(|v| v["met"].clone());
    rep.check(
        "agent.wait",
        slow && met(&w1) == Some(json!(true))
            && met(&w2) == Some(json!(true))
            && met(&w3) == Some(json!(false))
            && w3_ms >= 600,
        "browser_wait: late text and a selector met, unmet text answers met false at its timeout",
        json!({"text": job_text(&w1), "text_ms": w1_ms, "selector": job_text(&w2), "unmet": job_text(&w3), "unmet_ms": w3_ms}),
    );

    // ---- browser_navigate and browser_state: the host's moves and the events the window's snapshot reads
    let from = h.mark();
    h.host.navigate(TAB, &srv.url("/page2"));
    let nav = h.opened(from, TAB, "Page two");
    let (url, title) = page_of(&h);
    let from2 = h.mark();
    h.host.go(TAB, crate::backend::Nav::Back);
    let back = h.opened(from2, TAB, "Slow page");
    let hist = h.seen.iter().rev().find_map(|e| match e {
        WebEvent::History {
            tab,
            can_back,
            can_forward,
        } if tab == TAB => Some((*can_back, *can_forward)),
        _ => None,
    });
    rep.check(
        "agent.navigate-state",
        nav && url.ends_with("/page2") && title == "Page two" && back && hist.is_some_and(|(_, f)| f),
        "browser_navigate to an address and back, and the state the window reports (address, title, history)",
        json!({"url": url, "title": title, "back": back, "history": format!("{hist:?}")}),
    );

    // ---- browser_eval: a value, a promise, a throw, and a runaway stopped by its timeout
    let ev = |d: &mut Driver, h: &mut H, e: &str| {
        d.run(
            h,
            Op::Eval {
                expression: e.into(),
            },
            &RefMap::default(),
        )
    };
    let (e1, _, _) = ev(&mut d, &mut h, "document.title");
    let (e2, _, _) = ev(
        &mut d,
        &mut h,
        "new Promise(function(r){setTimeout(function(){r({a:1,b:[2,3]})},50)})",
    );
    let (e3, _, _) = ev(&mut d, &mut h, "(function(){throw new Error('boom')})()");
    let _ = js(&mut h, "window.__kept = 41; 1");
    let (e4, _, e4_ms) = ev(&mut d, &mut h, "while(true){}");
    let t = Instant::now();
    let after = js(&mut h, "window.__kept + 1");
    let after_ms = ms_since(t);
    rep.check(
        "agent.eval",
        e1.as_ref().is_ok_and(|v| v["value"] == "Slow page")
            && e2.as_ref().is_ok_and(|v| v["value"]["b"][1] == 3)
            && e3.as_ref().is_err_and(|e| e.message.contains("boom"))
            && e4.is_err()
            && after == 42
            && after_ms < 3_000,
        "browser_eval: a value, an awaited promise, a throw; while(true){} is stopped by the call's timeout and the \
         page answers at once with its state",
        json!({"value": job_text(&e1), "promise": job_text(&e2), "throw": job_text(&e3), "runaway": job_text(&e4),
               "runaway_ms": e4_ms, "after": after, "after_ms": after_ms}),
    );

    // ---- downloads: the agent clicks a download link; the file is held until the person's yes
    let from = h.mark();
    h.host.navigate(TAB, &srv.url("/agent"));
    let _ = h.opened(from, TAB, "Agent");
    h.wait(500);
    let (dlook, drefs, _) = d.run(&mut h, Op::Look { picture: false }, &RefMap::default());
    let drefs = drefs.unwrap_or_default();
    let dv = dlook.as_ref().cloned().unwrap_or(Value::Null);
    let from = h.mark();
    let (dc, _, _) = d.run(
        &mut h,
        Op::Click {
            target: Target::Ref(ref_named(&dv, "Download report").unwrap_or_default()),
            button: Button::Left,
            double: false,
        },
        &drefs,
    );
    let asked_dl = h.until(
        from,
        5_000,
        |e| matches!(e, WebEvent::Download { tab, .. } if tab == TAB),
    );
    let saved_dir = data.join("Downloads");
    let none_yet = std::fs::read_dir(&saved_dir).map_or(0, |r| r.count());
    if let Some(WebEvent::Download { id, .. }) = &asked_dl {
        h.host.answer_download(*id, true);
    }
    let file = saved_dir.join("report.txt");
    let t = Instant::now();
    while !file.exists() && t.elapsed() < Duration::from_secs(5) {
        h.wait(50);
    }
    let body = std::fs::read_to_string(&file).unwrap_or_default();
    rep.check(
        "agent.download",
        dc.is_ok()
            && matches!(&asked_dl, Some(WebEvent::Download { file, .. }) if file == "report.txt")
            && none_yet == 0
            && body == "the report's text",
        "a download the agent starts is held for the person's yes, as on Windows, and saved after it",
        json!({"click": job_text(&dc), "asked": asked_dl.as_ref().map(super::ev_text), "files_before_yes": none_yet, "saved": body}),
    );

    // ---- the agent's keys are not the person's; the person's go through the window's filter first
    asked.borrow_mut().clear();
    let _ = js(
        &mut h,
        "window.__log = []; document.getElementById('t1').focus(); 1",
    );
    let agent_t = press(&mut d, &mut h, "Meta+t");
    let after_agent = asked.borrow().len();
    let slot = Slot::panel(TAB);
    let from = h.mark();
    h.host.key(
        &slot,
        KeyInput {
            down: true,
            repeat: false,
            vk: u32::from(b'T'),
            mac_code: 17,
            text: "t".into(),
            mods: Mods {
                cmd: true,
                ..Mods::default()
            },
        },
    );
    let taken = h
        .until(
            from,
            2_000,
            |e| matches!(e, WebEvent::Key { tab, .. } if tab == TAB),
        )
        .is_some();
    h.wait(200);
    let page_keys = js(
        &mut h,
        "JSON.stringify(window.__log.filter(function(e){return e.t==='keydown'}).map(function(e){return e.k+'/'+e.m}))",
    );
    rep.check(
        "agent.input-is-not-the-persons",
        agent_t.is_ok()
            && after_agent == 0
            && taken
            && asked.borrow().len() == 1
            && page_keys.as_str().is_some_and(|s| s.contains("t/M") && s.matches("t/").count() == 1),
        "the agent's keys reach the page and never the window's filter (so they never end its own call or fire an \
         app chord); the person's Cmd+T is asked of the filter first and taken before the page sees it",
        json!({"agent_press": job_text(&agent_t), "filter_asked_by_agent": after_agent, "person_taken": taken,
               "filter_asked": asked.borrow().len(), "page_keydowns": page_keys}),
    );

    let mut methods: Vec<String> = d.sent.clone();
    methods.sort();
    methods.dedup();
    rep.set("agent_methods", json!(methods));
    h.host.close_all();
    h.wait(300);
    Ok(())
}
