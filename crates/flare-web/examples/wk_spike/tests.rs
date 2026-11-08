//! The scenarios: one per item of the agent browsing contract. Each records what happened (pass, fail or
//! note, plus evidence) into `report.json` in the output folder.

use std::path::{Path, PathBuf};

use flare_surfaces::agent_job::{Op, RefMap, Target};
use flare_surfaces::agent_web::{Button, WaitFor, parse_key};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSApplication;
use objc2_web_kit::{
    WKContentWorld, WKFrameInfo, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use serde_json::{Value, json};

use crate::mac::{self, Spy};
use crate::server::{self, Server};
use crate::shim::{Shim, run_job};

pub const AGENT_JS: &str = include_str!("agent.js");

pub struct Report {
    pub out: PathBuf,
    pub items: Vec<Value>,
}

impl Report {
    pub fn add(&mut self, id: &str, ok: bool, what: &str, evidence: Value) {
        println!("[{}] {id}: {what}", if ok { "PASS" } else { "FAIL" });
        self.items
            .push(json!({"id": id, "pass": ok, "what": what, "evidence": evidence}));
    }

    pub fn note(&mut self, id: &str, what: &str, evidence: Value) {
        println!("[NOTE] {id}: {what}");
        self.items
            .push(json!({"id": id, "pass": null, "what": what, "evidence": evidence}));
    }

    pub fn save_png(&self, name: &str, png: &[u8]) -> String {
        let p = self.out.join(name);
        let _ = std::fs::write(&p, png);
        p.to_string_lossy().into_owned()
    }
}

/// One scenario of the run.
type Scenario = fn(&Env, &mut Report);

pub struct Env {
    pub app: Retained<NSApplication>,
    pub mtm: MainThreadMarker,
    pub spy: Retained<Spy>,
    pub srv: Server,
    pub win: Retained<mac::SpikeWindow>,
    pub wv: Retained<WKWebView>,
}

impl Env {
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.srv.port)
    }
}

/// The web panel's configuration: its own data store, the agent's helper in every frame (in its own
/// content world) and the message handler it answers through.
pub fn panel_config(
    mtm: MainThreadMarker,
    spy: &Spy,
    store: &WKWebsiteDataStore,
) -> Retained<WKWebViewConfiguration> {
    let cfg = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe { cfg.setWebsiteDataStore(store) };
    let ucc = unsafe { cfg.userContentController() };
    let world = mac::agent_world(mtm);
    unsafe {
        ucc.addUserScript(&mac::user_script(mtm, AGENT_JS, false, &world));
        ucc.addScriptMessageHandler_contentWorld_name(
            ProtocolObject::from_ref(spy),
            &world,
            &objc2_foundation::NSString::from_str("flareAgent"),
        );
        // Console capture: the page's console lines are posted to the host (page world).
        let page = WKContentWorld::pageWorld(mtm);
        ucc.addUserScript(&mac::user_script(mtm, CONSOLE_JS, false, &page));
        ucc.addScriptMessageHandler_contentWorld_name(
            ProtocolObject::from_ref(spy),
            &page,
            &objc2_foundation::NSString::from_str("flareConsole"),
        );
    }
    cfg
}

pub const CONSOLE_JS: &str = "(function(){var h=window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.flareConsole;if(!h)return;['log','warn','error','info'].forEach(function(k){var o=console[k];console[k]=function(){try{h.postMessage(k+': '+Array.prototype.map.call(arguments,String).join(' '))}catch(e){}return o.apply(console,arguments)}})})();";

/// Closed shadow roots for the agent: the page world's `attachShadow` makes every root open underneath,
/// and the page world's `shadowRoot` getter still says null for the ones the page asked to be closed.
/// The agent's own world has its own, untouched prototypes, so it sees them all.
pub const SHADOW_JS: &str = "(function(){var o=Element.prototype.attachShadow;var closed=new WeakSet();Element.prototype.attachShadow=function(i){var r=o.call(this,Object.assign({},i,{mode:'open'}));if(i&&i.mode==='closed')closed.add(this);return r;};var d=Object.getOwnPropertyDescriptor(Element.prototype,'shadowRoot');Object.defineProperty(Element.prototype,'shadowRoot',{configurable:true,enumerable:d.enumerable,get:function(){return closed.has(this)?null:d.get.call(this);}});})();";

pub fn run(out: &Path, only: Option<&str>) {
    let _ = std::fs::create_dir_all(out);
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = mac::app(mtm);
    let srv = server::start();
    let spy = Spy::new(mtm);
    let win = mac::window(mtm, 1000.0, 700.0);
    let store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
    let cfg = panel_config(mtm, &spy, &store);
    let wv = mac::make_view(mtm, &cfg, 1000.0, 700.0);
    unsafe {
        wv.setNavigationDelegate(Some(ProtocolObject::from_ref(&*spy)));
        wv.setUIDelegate(Some(ProtocolObject::from_ref(&*spy)));
    }
    win.setContentView(Some(&wv));
    win.makeFirstResponder(Some(&wv));
    let env = Env {
        app,
        mtm,
        spy,
        srv,
        win,
        wv,
    };
    let mut rep = Report {
        out: out.to_path_buf(),
        items: Vec::new(),
    };
    rep.note(
        "env",
        "headless host",
        json!({
            "activation_policy": "Prohibited",
            "window_ordered_in": false,
            "onscreen_windows_of_this_process": mac::onscreen_windows_of_self(),
            "app_active": env.app.isActive(),
            "backing_scale": env.win.backingScaleFactor(),
            "port": env.srv.port,
        }),
    );
    let all: [(&str, Scenario); 17] = [
        ("look", look),
        ("input", input),
        ("nav", scroll_wait_nav),
        ("iframes", iframes),
        ("downloads", downloads),
        ("sandbox", sandbox),
        ("user", user_input),
        ("others", others),
        ("real", real_sites),
        ("adv-csp", crate::adv::csp),
        ("adv-trap", crate::adv::trap),
        ("adv-hover", crate::adv::hover),
        ("adv-exfil", crate::adv::exfil),
        ("adv-proxy", crate::adv::proxy),
        ("adv-sbframe", crate::adv::sbframe),
        ("adv-perms", crate::adv::perms),
        ("adv-close", crate::adv::close),
    ];
    for (name, f) in all {
        if only.is_none_or(|o| o == name || (o == "adv" && name.starts_with("adv-"))) {
            f(&env, &mut rep);
        }
    }
    rep.note(
        "env-end",
        "the app never became active",
        json!({"app_active": env.app.isActive(), "onscreen_windows_of_this_process": mac::onscreen_windows_of_self()}),
    );
    let text = serde_json::to_string_pretty(&json!({"items": rep.items})).unwrap_or_default();
    let _ = std::fs::write(out.join("report.json"), text);
    println!("report: {}", out.join("report.json").display());
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

/// The ref of the element with this name, a control before plain text of the same words (a label).
fn ref_named(look: &Value, name: &str) -> Option<String> {
    let all = look["elements"].as_array()?;
    all.iter()
        .find(|e| e["name"].as_str() == Some(name) && e["role"] != "text")
        .or_else(|| all.iter().find(|e| e["name"].as_str() == Some(name)))
        .and_then(|e| e["ref"].as_str().map(String::from))
}

pub fn do_look(env: &Env, shim: &mut Shim<'_>, rep: &mut Report, png: &str) -> (Value, RefMap) {
    let (r, refs) = run_job(shim, Op::Look { picture: true }, &RefMap::default());
    let mut v = r.unwrap_or_else(|e| json!({"error": e.message}));
    if let Some((bytes, _, _)) = shim.last_shot.take() {
        let p = rep.save_png(png, &bytes);
        v["image_saved"] = json!(p);
    }
    if let Some(img) = v.get_mut("image")
        && let Some(o) = img.as_object_mut()
    {
        o.remove("data");
    }
    let _ = env;
    (v, refs.unwrap_or_default())
}

// ---- 1. browser_look --------------------------------------------------------------------------

fn look(env: &Env, rep: &mut Report) {
    let ok = mac::load_wait(&env.app, &env.wv, &format!("{}/main", env.base()));
    mac::spin(&env.app, 800);
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs) = do_look(env, &mut shim, rep, "look-main.png");
    let list = names(&v);
    for l in &list {
        println!("    {l}");
    }
    let has = |n: &str| ref_named(&v, n).is_some();
    rep.add(
        "1.look",
        ok && v["image"]["width"].as_u64().unwrap_or(0) > 0 && has("Press me") && refs.len() > 5,
        "browser_look through the real agent_job::Job: PNG still plus the numbered list",
        json!({
            "image": v["image"], "image_saved": v["image_saved"], "image_error": v["image_error"],
            "viewport": v["viewport"], "count": list.len(), "truncated": v["truncated"],
            "elements": list, "cdp_methods": shim.methods.iter().fold(std::collections::BTreeMap::<String, usize>::new(), |mut m, k| { *m.entry(k.clone()).or_default() += 1; m }),
            "frames": shim.frame_labels(), "notes": shim.notes,
        }),
    );
    rep.add(
        "1.look-iframes",
        has("Frame button") && has("Cross button") && has("Cross input"),
        "same-origin and cross-origin iframe controls are in the list",
        json!({"same": has("Frame button"), "cross": has("Cross button"), "frames": shim.frame_labels()}),
    );
    rep.note(
        "1.look-closed-shadow",
        if has("Shadow button") {
            "a closed shadow root's button is listed"
        } else {
            "a closed shadow root's button is NOT listed without the shim"
        },
        json!({"listed": has("Shadow button")}),
    );
    // Without the harness's "visible" answer the window is what it really is: never on screen.
    mac::FAKE_VISIBLE.with(|f| f.set(false));
    mac::spin(&env.app, 500);
    pjs(
        env,
        "document.querySelector('h1').textContent = 'Changed while hidden'; 1",
    );
    let hidden_shot = mac::snapshot_png(&env.app, &env.wv, None, None).map(|(png, w, h)| {
        let p = rep.save_png("look-window-not-visible.png", &png);
        json!({"width": w, "height": h, "path": p})
    });
    mac::FAKE_VISIBLE.with(|f| f.set(true));
    mac::spin(&env.app, 500);
    rep.note(
        "1.look-hidden-window",
        "takeSnapshot from a window that is not visible (no isVisible override), after the page changed: a fresh paint or a stale frame? See the PNG",
        json!({"result": hidden_shot}),
    );

    // The closed-shadow shim (page world, document start), then the same look again.
    let ucc = unsafe { env.wv.configuration().userContentController() };
    let pw = page_world(env);
    unsafe { ucc.addUserScript(&mac::user_script(env.mtm, SHADOW_JS, false, &pw)) };
    mac::load_wait(&env.app, &env.wv, &format!("{}/main", env.base()));
    mac::spin(&env.app, 700);
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v2, refs2) = do_look(env, &mut shim, rep, "look-main-shadow-shim.png");
    let listed = ref_named(&v2, "Shadow button");
    let page_sees = pjs(env, "String(document.getElementById('host').shadowRoot)");
    plog(env);
    let click = listed.clone().map(|r| {
        act(
            &mut shim,
            Op::Click {
                target: Target::Ref(r),
                button: Button::Left,
                double: false,
            },
            &refs2,
        )
    });
    let log = plog(env);
    rep.add(
        "1.look-closed-shadow-shim",
        listed.is_some() && page_sees == "null" && trusted(&log, &["click"]),
        "with the attachShadow shim the closed root's button is listed and clickable, and the page still sees shadowRoot null",
        json!({"listed_as": listed, "page_world_shadowRoot": page_sees, "click": click, "events": brief(&log)}),
    );
}

// ---- helpers ------------------------------------------------------------------------------------------

fn page_world(env: &Env) -> Retained<WKContentWorld> {
    unsafe { WKContentWorld::pageWorld(env.mtm) }
}

/// A page-world value of the main frame (or of `frame`).
fn pjs_in(env: &Env, wv: &WKWebView, frame: Option<&WKFrameInfo>, js: &str) -> Value {
    mac::eval(&env.app, wv, frame, &page_world(env), js, 5_000)
        .unwrap_or_else(|e| json!({"error": e}))
}

fn pjs(env: &Env, js: &str) -> Value {
    pjs_in(env, &env.wv, None, js)
}

/// What the page logged since the last call (and the log is emptied).
fn plog_in(env: &Env, frame: Option<&WKFrameInfo>) -> Vec<Value> {
    match pjs_in(
        env,
        &env.wv,
        frame,
        "JSON.stringify(window.__log.splice(0))",
    ) {
        Value::String(s) => serde_json::from_str(&s).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn plog(env: &Env) -> Vec<Value> {
    plog_in(env, None)
}

/// Every event of these types is trusted, and each type was seen.
fn trusted(log: &[Value], types: &[&str]) -> bool {
    types.iter().all(|t| {
        let of: Vec<&Value> = log.iter().filter(|e| e["t"] == *t).collect();
        !of.is_empty() && of.iter().all(|e| e["tr"] == true)
    })
}

fn brief(log: &[Value]) -> Vec<String> {
    log.iter()
        .map(|e| {
            let mut s = format!(
                "{} trusted={} target={}",
                e["t"].as_str().unwrap_or(""),
                e["tr"],
                e["id"].as_str().unwrap_or("")
            );
            for k in ["k", "c", "d", "it"] {
                if let Some(v) = e[k].as_str()
                    && !v.is_empty()
                {
                    s.push_str(&format!(" {k}={v:?}"));
                }
            }
            for k in ["ctrl", "meta", "shift", "alt"] {
                if e[k] == true {
                    s.push_str(&format!(" {k}"));
                }
            }
            if e["t"] == "auxclick" || e["t"] == "mousedown" {
                s.push_str(&format!(" button={}", e["btn"]));
            }
            if e["t"] == "dblclick" || e["t"] == "click" {
                s.push_str(&format!(" detail={}", e["det"]));
            }
            if e["t"] == "wheel" {
                s.push_str(&format!(" deltaY={}", e["dy"]));
            }
            s
        })
        .collect()
}

fn fresh_look(env: &Env, rep: &mut Report, shim: &mut Shim<'_>, png: &str) -> (Value, RefMap) {
    mac::load_wait(&env.app, &env.wv, &format!("{}/main", env.base()));
    mac::spin(&env.app, 700);
    do_look(env, shim, rep, png)
}

fn act(shim: &mut Shim<'_>, op: Op, refs: &RefMap) -> Result<Value, String> {
    run_job(shim, op, refs)
        .0
        .map_err(|e| format!("{:?}: {}", e.fail, e.message))
}

fn click_ref(
    shim: &mut Shim<'_>,
    look: &Value,
    refs: &RefMap,
    name: &str,
) -> Result<Value, String> {
    let r = ref_named(look, name).ok_or(format!("no ref for {name}"))?;
    act(
        shim,
        Op::Click {
            target: Target::Ref(r),
            button: Button::Left,
            double: false,
        },
        refs,
    )
}

fn press(shim: &mut Shim<'_>, refs: &RefMap, key: &str) -> Result<Value, String> {
    act(
        shim,
        Op::Press {
            key: parse_key(key)?,
        },
        refs,
    )
}

// ---- 2. trusted clicks and keys -----------------------------------------------------------------

fn input(env: &Env, rep: &mut Report) {
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs) = fresh_look(env, rep, &mut shim, "input-look.png");
    plog(env);

    let r = click_ref(&mut shim, &v, &refs, "Press me");
    let log = plog(env);
    let out = pjs(env, "document.getElementById('out').textContent");
    rep.add(
        "2.click",
        r.is_ok() && out == "clicked b1 true" && trusted(&log, &["mousedown", "mouseup", "click"]),
        "browser_click on a ref: NSEvent mouseDown/mouseUp straight into the WKWebView",
        json!({"job": r, "out": out, "events": brief(&log)}),
    );

    let r = act(
        &mut shim,
        Op::Click {
            target: Target::Ref(
                ref_named(&v, "Some intro text for the reader.").unwrap_or_default(),
            ),
            button: Button::Left,
            double: true,
        },
        &refs,
    );
    let log = plog(env);
    let sel = pjs(env, "String(getSelection())");
    rep.add(
        "2.double-click",
        r.is_ok() && trusted(&log, &["dblclick"]) && sel.as_str().is_some_and(|s| !s.is_empty()),
        "double click: clickCount 2 gives a trusted dblclick and selects a word",
        json!({"job": r, "selection": sel, "events": brief(&log)}),
    );

    let r = act(
        &mut shim,
        Op::Click {
            target: Target::Ref(ref_named(&v, "Middle target").unwrap_or_default()),
            button: Button::Middle,
            double: false,
        },
        &refs,
    );
    let log = plog(env);
    let aux_ok = log
        .iter()
        .any(|e| e["t"] == "auxclick" && e["tr"] == true && e["btn"] == 1);
    rep.add(
        "2.middle-click",
        r.is_ok() && aux_ok,
        "middle click: otherMouseDown/Up gives a trusted auxclick with button 1",
        json!({"job": r, "events": brief(&log)}),
    );

    // A click at a point (the picture's coordinates), not a ref: the checkbox.
    let cb = v["elements"]
        .as_array()
        .and_then(|a| a.iter().find(|e| e["name"] == "Agree"))
        .map(|e| {
            (
                e["center"][0].as_f64().unwrap_or(0.0),
                e["center"][1].as_f64().unwrap_or(0.0),
            )
        });
    let r = cb.map(|(x, y)| {
        act(
            &mut shim,
            Op::Click {
                target: Target::Point(x, y),
                button: Button::Left,
                double: false,
            },
            &refs,
        )
    });
    let log = plog(env);
    let checked = pjs(env, "document.getElementById('c1').checked");
    rep.add(
        "2.click-point",
        checked == true && trusted(&log, &["click"]),
        "browser_click at x, y: the checkbox under the point is checked",
        json!({"point": cb, "job": r, "checked": checked, "events": brief(&log)}),
    );

    let text = "h\u{e9}llo w\u{f6}rld \u{2713} \u{65e5}\u{672c}\u{8a9e} \u{1f389} \u{5d0}\u{5d1}";
    let r = act(
        &mut shim,
        Op::Type {
            text: text.into(),
            target: ref_named(&v, "Name"),
        },
        &refs,
    );
    let log = plog(env);
    let val = pjs(env, "document.getElementById('t1').value");
    rep.add(
        "2.type-input",
        val == text && trusted(&log, &["beforeinput", "input"]),
        "browser_type into an input: unicode (accents, CJK, emoji, RTL) without an IME, trusted input events",
        json!({"job": r, "value": val, "events": brief(&log)}),
    );

    let r = act(
        &mut shim,
        Op::Type {
            text: "rich \u{2713} text".into(),
            target: ref_named(&v, "Editor"),
        },
        &refs,
    );
    let log = plog(env);
    let val = pjs(env, "document.getElementById('ce').innerText");
    rep.add(
        "2.type-contenteditable",
        val == "rich \u{2713} text" && trusted(&log, &["beforeinput", "input"]),
        "browser_type into contenteditable",
        json!({"job": r, "value": val, "events": brief(&log)}),
    );

    let r = act(
        &mut shim,
        Op::Type {
            text: "line1\nline2".into(),
            target: ref_named(&v, "Notes"),
        },
        &refs,
    );
    plog(env);
    let val = pjs(env, "document.getElementById('ta').value");
    rep.add(
        "2.type-textarea-newline",
        val == "line1\nline2",
        "browser_type with a newline into a textarea",
        json!({"job": r, "value": val}),
    );

    // Keys into the Name field: End, Backspace, Command+A then typing, letters with Shift.
    let _ = act(
        &mut shim,
        Op::Type {
            text: "abc".into(),
            target: ref_named(&v, "Name"),
        },
        &refs,
    );
    plog(env);
    let before = pjs(env, "document.getElementById('t1').value");
    let r1 = press(&mut shim, &refs, "End");
    let r2 = press(&mut shim, &refs, "Backspace");
    let after_bs = pjs(env, "document.getElementById('t1').value");
    let log_bs = plog(env);
    let r3 = press(&mut shim, &refs, "Meta+a");
    let sel = pjs(
        env,
        "(function(e){return e.selectionEnd - e.selectionStart})(document.getElementById('t1'))",
    );
    let log_meta = plog(env);
    let notes_before = shim.notes.len();
    let r4 = press(&mut shim, &refs, "x");
    let r5 = press(&mut shim, &refs, "Shift+B");
    let after_keys = pjs(env, "document.getElementById('t1').value");
    let log_keys = plog(env);
    let bs_ok = before
        .as_str()
        .zip(after_bs.as_str())
        .is_some_and(|(b, a)| a.chars().count() + 1 == b.chars().count());
    // A note, not a parity check: Meta+a selects only because the shim calls selectAll: itself (the
    // Command chord row of the verdict, closable), and the Windows tool text says Control+a.
    let ok = bs_ok
        && after_keys == "xB"
        && sel.as_i64().unwrap_or(0) > 10
        && trusted(&log_bs, &["keydown", "keyup"])
        && trusted(&log_keys, &["keydown", "input"]);
    rep.note(
        "2.press-editing-keys",
        "browser_press: End, Backspace, Meta+a (select all through the shim's selectAll:), x, Shift+B; trusted keydown/keyup with the right key and code",
        json!({
            "all_held": ok,
            "jobs": [r1, r2, r3, r4, r5], "before": before, "after_backspace": after_bs,
            "selected_after_meta_a": sel, "after_keys": after_keys, "shim_notes": shim.notes[..notes_before].iter().rev().take(3).collect::<Vec<_>>(),
            "events_backspace": brief(&log_bs), "events_meta_a": brief(&log_meta), "events_keys": brief(&log_keys),
        }),
    );

    let r = press(&mut shim, &refs, "Control+a");
    let log = plog(env);
    let ctrl_seen = log
        .iter()
        .any(|e| e["t"] == "keydown" && e["tr"] == true && e["ctrl"] == true && e["k"] == "a");
    rep.add(
        "2.press-control",
        ctrl_seen,
        "browser_press Control+a: a trusted keydown with ctrlKey and key a",
        json!({"job": r, "events": brief(&log)}),
    );

    let before_focus = pjs(env, "document.activeElement.id");
    let r = press(&mut shim, &refs, "Shift+Tab");
    let after_focus = pjs(env, "document.activeElement.id");
    let log = plog(env);
    rep.add(
        "2.press-shift-tab",
        before_focus != after_focus && trusted(&log, &["keydown"]),
        "browser_press Shift+Tab moves focus backwards",
        json!({"job": r, "focus_before": before_focus, "focus_after": after_focus, "events": brief(&log)}),
    );

    let mut arrows = Vec::new();
    for k in [
        "ArrowLeft",
        "Enter",
        "Escape",
        "PageDown",
        "F5",
        "Delete",
        "Home",
    ] {
        let r = press(&mut shim, &refs, k);
        let log = plog(env);
        let kd = log.iter().find(|e| e["t"] == "keydown").cloned();
        arrows.push(json!({"key": k, "job": r.is_ok(), "keydown": kd.map(|e| json!({"key": e["k"], "code": e["c"], "trusted": e["tr"]}))}));
    }
    let named_ok = arrows.iter().all(|a| {
        a["keydown"]["trusted"] == true
            && a["keydown"]["key"]
                == a["key"]
                    .as_str()
                    .map(|k| if k == "Escape" { "Escape" } else { k })
                    .unwrap_or("")
    });
    rep.add(
        "2.press-named-keys",
        named_ok,
        "named keys arrive as trusted keydowns with the DOM key name Windows sends",
        json!({"keys": arrows}),
    );

    // The same typing with the window NOT key (Flare in the background while the agent works).
    mac::FAKE_KEY.with(|k| k.set(false));
    mac::spin(&env.app, 200);
    let focus_flag = pjs(env, "document.hasFocus()");
    let r = act(
        &mut shim,
        Op::Type {
            text: "bg".into(),
            target: ref_named(&v, "Name"),
        },
        &refs,
    );
    let val = pjs(env, "document.getElementById('t1').value");
    let log = plog(env);
    let r2 = press(&mut shim, &refs, "z");
    let val2 = pjs(env, "document.getElementById('t1').value");
    let log2 = plog(env);
    let click = click_ref(&mut shim, &v, &refs, "Press me");
    let log3 = plog(env);
    mac::FAKE_KEY.with(|k| k.set(true));
    mac::spin(&env.app, 200);
    rep.add(
        "2.background-window",
        val.as_str().is_some_and(|s| s.contains("bg"))
            && val2.as_str().is_some_and(|s| s.contains("bgz"))
            && trusted(&log3, &["click"]),
        "with the host window not key (the app in the background): typing, keys and clicks still land",
        json!({"document_hasFocus": focus_flag, "type_job": r, "value_after_type": val, "events_type": brief(&log),
               "press_job": r2, "value_after_press": val2, "events_press": brief(&log2), "click": click, "events_click": brief(&log3)}),
    );
}

// ---- 3. scroll, wait, navigate --------------------------------------------------------------------

fn scroll_wait_nav(env: &Env, rep: &mut Report) {
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs) = fresh_look(env, rep, &mut shim, "nav-look.png");
    plog(env);
    let y0 = pjs(env, "scrollY");
    let r = act(
        &mut shim,
        Op::Scroll {
            dx: 0.0,
            dy: 600.0,
            target: None,
        },
        &refs,
    );
    mac::spin(&env.app, 600);
    let y1 = pjs(env, "scrollY");
    let log = plog(env);
    let moved = y1.as_f64().unwrap_or(0.0) - y0.as_f64().unwrap_or(0.0);
    let (_, refs2) = do_look(env, &mut shim, rep, "scrolled-look.png");
    rep.add(
        "3.scroll-wheel",
        r.is_ok() && moved >= 300.0 && trusted(&log, &["wheel"]),
        "browser_scroll: a pixel wheel CGEvent (as NSEvent) into the view scrolls the page; trusted wheel event",
        json!({"job": r, "scrollY_before": y0, "scrollY_after": y1, "events": brief(&log), "png": rep.out.join("scrolled-look.png")}),
    );
    let r = act(
        &mut shim,
        Op::Scroll {
            dx: 0.0,
            dy: -300.0,
            target: ref_named(&v, "The bottom line")
                .filter(|_| false)
                .or_else(|| Some("e1".into())),
        },
        &refs2,
    );
    let y2 = pjs(env, "scrollY");
    rep.add(
        "3.scroll-ref",
        r.is_ok(),
        "browser_scroll over a ref (scroll into view, then the wheel at its point)",
        json!({"job": r, "scrollY_after": y2}),
    );

    // wait: text, selector, pause, timeout.
    mac::load(&env.wv, &format!("{}/slow", env.base()));
    let t = std::time::Instant::now();
    let r = act(
        &mut shim,
        Op::Wait {
            what: WaitFor::Text("late text appears".into()),
            timeout_ms: 10_000,
        },
        &RefMap::default(),
    );
    let waited = t.elapsed().as_millis();
    let r2 = act(
        &mut shim,
        Op::Wait {
            what: WaitFor::Selector("#late".into()),
            timeout_ms: 2_000,
        },
        &RefMap::default(),
    );
    let t = std::time::Instant::now();
    let r3 = act(
        &mut shim,
        Op::Wait {
            what: WaitFor::Ms(300),
            timeout_ms: 300,
        },
        &RefMap::default(),
    );
    let paused = t.elapsed().as_millis();
    let r4 = act(
        &mut shim,
        Op::Wait {
            what: WaitFor::Text("never there".into()),
            timeout_ms: 700,
        },
        &RefMap::default(),
    );
    rep.add(
        "3.wait",
        r.as_ref().is_ok_and(|v| v["met"] == true)
            && r2.as_ref().is_ok_and(|v| v["met"] == true)
            && r3.is_ok()
            && r4.as_ref().is_ok_and(|v| v["met"] == false),
        "browser_wait: text across a slow navigation, selector, pause, and a timeout that is not met",
        json!({"text": r, "text_waited_ms": waited, "selector": r2, "pause": r3, "paused_ms": paused, "absent": r4}),
    );

    // navigate, history, reload, status, same-document.
    mac::st(|s| {
        s.nav.clear();
        s.status.clear();
    });
    let (v, refs) = fresh_look(env, rep, &mut shim, "nav-main.png");
    let fin0 = mac::st(|s| s.finished);
    let r = click_ref(&mut shim, &v, &refs, "Go to page two");
    mac::wait_until(&env.app, 5_000, || mac::st(|s| s.finished) > fin0);
    let after_link = mac::url_of(&env.wv);
    let title = mac::title_of(&env.wv);
    let can_back = unsafe { env.wv.canGoBack() };
    let f = mac::st(|s| s.finished);
    unsafe { env.wv.goBack() };
    mac::wait_until(&env.app, 5_000, || mac::st(|s| s.finished) > f);
    let after_back = mac::url_of(&env.wv);
    let can_fwd = unsafe { env.wv.canGoForward() };
    let f = mac::st(|s| s.finished);
    unsafe { env.wv.goForward() };
    mac::wait_until(&env.app, 5_000, || mac::st(|s| s.finished) > f);
    let after_fwd = mac::url_of(&env.wv);
    let f = mac::st(|s| s.finished);
    unsafe { env.wv.reload() };
    let reloaded = mac::wait_until(&env.app, 5_000, || mac::st(|s| s.finished) > f);
    let f = mac::st(|s| s.finished);
    pjs(env, "history.pushState({}, '', '/page2#pushed'); 1");
    mac::spin(&env.app, 300);
    let after_push = mac::url_of(&env.wv);
    let push_no_nav = mac::st(|s| s.finished) == f;
    mac::load_wait(&env.app, &env.wv, &format!("{}/status404", env.base()));
    mac::load(&env.wv, &format!("{}/slow", env.base()));
    mac::spin(&env.app, 200);
    unsafe { env.wv.stopLoading() };
    mac::spin(&env.app, 500);
    let nav = mac::st(|s| s.nav.clone());
    let status = mac::st(|s| s.status.clone());
    rep.add(
        "3.navigate-history",
        r.is_ok()
            && after_link.ends_with("/page2")
            && after_back.ends_with("/main")
            && after_fwd.ends_with("/page2")
            && reloaded
            && after_push.ends_with("#pushed")
            && push_no_nav
            && status.contains(&404),
        "navigate by a link click, back, forward, reload, pushState (address changes, no navigation), HTTP status, stop",
        json!({"link": after_link, "title": title, "can_back": can_back, "back": after_back, "can_forward": can_fwd,
               "forward": after_fwd, "reloaded": reloaded, "push_state_url": after_push, "push_state_raised_no_navigation": push_no_nav,
               "statuses": status, "navigation_events": nav}),
    );
}

// ---- 4. iframes -------------------------------------------------------------------------------------

fn iframes(env: &Env, rep: &mut Report) {
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs) = fresh_look(env, rep, &mut shim, "iframes-look.png");
    let same = shim.frame_info(&format!("http://127.0.0.1:{}", env.srv.port));
    let cross = shim.frame_info(&format!("http://localhost:{}", env.srv.port));
    plog_in(env, same.as_deref());
    plog_in(env, cross.as_deref());
    let r1 = click_ref(&mut shim, &v, &refs, "Frame button");
    let r2 = click_ref(&mut shim, &v, &refs, "Cross button");
    let r3 = act(
        &mut shim,
        Op::Type {
            text: "\u{fc}ber \u{2713}".into(),
            target: ref_named(&v, "Cross input"),
        },
        &refs,
    );
    let same_text = pjs_in(
        env,
        &env.wv,
        same.as_deref(),
        "document.getElementById('fb').textContent",
    );
    let cross_text = pjs_in(
        env,
        &env.wv,
        cross.as_deref(),
        "document.getElementById('xb').textContent",
    );
    let cross_val = pjs_in(
        env,
        &env.wv,
        cross.as_deref(),
        "document.getElementById('xi').value",
    );
    let cross_origin = pjs_in(env, &env.wv, cross.as_deref(), "location.origin");
    let main_can_reach = pjs(
        env,
        "(function(){try{return document.getElementById('fx').contentDocument===null?'blocked':'reachable'}catch(e){return 'blocked'}})()",
    );
    let ls = plog_in(env, same.as_deref());
    let lc = plog_in(env, cross.as_deref());
    rep.add(
        "4.iframes",
        r1.is_ok()
            && r2.is_ok()
            && same_text == "Frame clicked"
            && cross_text == "Cross clicked"
            && cross_val == "\u{fc}ber \u{2713}"
            && trusted(&ls, &["click"])
            && trusted(&lc, &["click", "input"]),
        "click and type inside a same-origin and a cross-origin iframe (refs from the look); read inside both by WKFrameInfo",
        json!({"jobs": [r1, r2, r3], "same_button": same_text, "cross_button": cross_text, "cross_input": cross_val,
               "cross_origin": cross_origin, "main_page_reaches_cross_doc": main_can_reach,
               "same_events": brief(&ls), "cross_events": brief(&lc), "frames": shim.frame_labels()}),
    );
}

// ---- 5. downloads ----------------------------------------------------------------------------------

fn downloads(env: &Env, rep: &mut Report) {
    let dir = rep.out.join("dl");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs) = fresh_look(env, rep, &mut shim, "dl-look.png");
    mac::st(|s| {
        s.dl_events.clear();
        s.dl_pending = None;
    });
    let r = click_ref(&mut shim, &v, &refs, "Download file");
    let asked = mac::wait_until(&env.app, 5_000, || mac::st(|s| s.dl_pending.is_some()));
    mac::spin(&env.app, 500);
    let files_while_held = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
    let (done, name) = mac::st(|s| s.dl_pending.take()).expect("pending");
    let dest = dir.join(&name);
    let u = objc2_foundation::NSURL::fileURLWithPath(&objc2_foundation::NSString::from_str(
        &dest.to_string_lossy(),
    ));
    done.call((Retained::as_ptr(&u).cast_mut(),));
    let mut samples = Vec::new();
    let t = std::time::Instant::now();
    while t.elapsed().as_secs() < 30 {
        if let Some(d) = mac::st(|s| s.downloads.last().cloned()) {
            let (f, c, tot) = mac::progress_of(&d);
            if samples.last().is_none_or(|l: &Value| l["completed"] != c) {
                samples.push(json!({"ms": t.elapsed().as_millis(), "fraction": (f * 1000.0).round() / 1000.0, "completed": c, "total": tot}));
            }
        }
        if mac::st(|s| {
            s.dl_events
                .iter()
                .any(|e| e == "finished" || e.starts_with("failed"))
        }) {
            break;
        }
        mac::spin(&env.app, 100);
    }
    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    let events = mac::st(|s| s.dl_events.clone());
    rep.add(
        "5.download-yes",
        r.is_ok() && asked && files_while_held == 0 && size == 4 * 1024 * 1024 && events.iter().any(|e| e == "finished"),
        "a download from an agent click: held until answered (nothing saved meanwhile), path chosen, progress, completion",
        json!({"job": r, "held_then_answered": asked, "files_while_held": files_while_held, "path": dest, "size": size,
               "progress_samples": samples.len(), "progress_first_last": [samples.first(), samples.last()], "events": events}),
    );

    mac::st(|s| s.dl_events.clear());
    let (v, refs) = fresh_look(env, rep, &mut shim, "dl-look2.png");
    let r = click_ref(&mut shim, &v, &refs, "Download file");
    let asked = mac::wait_until(&env.app, 5_000, || mac::st(|s| s.dl_pending.is_some()));
    if let Some((done, _)) = mac::st(|s| s.dl_pending.take()) {
        done.call((std::ptr::null_mut(),));
    }
    mac::wait_until(&env.app, 5_000, || {
        mac::st(|s| s.dl_events.iter().any(|e| e.starts_with("failed")))
    });
    let files = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
    let events = mac::st(|s| s.dl_events.clone());
    rep.add(
        "5.download-no",
        r.is_ok() && asked && files == 1,
        "a download answered no: cancelled, nothing saved",
        json!({"job": r, "files_in_folder": files, "events": events}),
    );
}

// ---- 6. the mini-app sandbox -------------------------------------------------------------------------

/// Block everything, then let the app's own scheme and in-memory content (blob:, data:) through, as the
/// Windows CSP does (`worker-src 'self' blob:`, `data:`).
const RULES: &str = r#"[{"trigger":{"url-filter":".*"},"action":{"type":"block"}},{"trigger":{"url-filter":"^flare-app://app/"},"action":{"type":"ignore-previous-rules"}},{"trigger":{"url-filter":"^blob:flare-app://app/"},"action":{"type":"ignore-previous-rules"}},{"trigger":{"url-filter":"^data:"},"action":{"type":"ignore-previous-rules"}}]"#;

fn sandbox(env: &Env, rep: &mut Report) {
    let root = rep.out.join("app");
    let _ = std::fs::create_dir_all(&root);
    let _ = std::fs::write(root.join("app.json"), r#"{"own":"file"}"#);
    mac::st(|s| s.app_root = root.clone());
    let rules = mac::compile_rules(&env.app, "flare-spike-app", RULES);
    rep.note(
        "6.rules-compiled",
        if rules.is_ok() {
            "WKContentRuleList compiled"
        } else {
            "rule list failed"
        },
        json!({"rules": RULES, "error": rules.as_ref().err()}),
    );
    let page = crate::pages::app(env.srv.port, env.srv.udp_port);
    let variants = [
        ("A-control-no-walls", false, false, false, false),
        ("B-rules-and-webrtc-script", true, false, false, true),
        ("C-dead-proxy-only", false, true, false, false),
        ("D-full-rules-proxy-csp-webrtc", true, true, true, true),
    ];
    let panel_store = unsafe { env.wv.configuration().websiteDataStore() };
    let mut app_store_for_isolation = None;
    for (name, use_rules, proxy, csp, rtc) in variants {
        let store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(env.mtm) };
        if proxy {
            mac::set_dead_proxy(&store, 9);
        }
        let cfg = unsafe { WKWebViewConfiguration::new(env.mtm) };
        unsafe {
            cfg.setWebsiteDataStore(&store);
            cfg.setURLSchemeHandler_forURLScheme(
                Some(ProtocolObject::from_ref(&*env.spy)),
                &objc2_foundation::NSString::from_str("flare-app"),
            );
        }
        let ucc = unsafe { cfg.userContentController() };
        if use_rules && let Ok(r) = &rules {
            unsafe { ucc.addContentRuleList(r) };
        }
        if rtc {
            let pw = page_world(env);
            unsafe {
                ucc.addUserScript(&mac::user_script(
                    env.mtm,
                    flare_surfaces::app::NO_WEBRTC_SCRIPT,
                    false,
                    &pw,
                ))
            };
        }
        let html = if csp {
            flare_surfaces::app::wrap_html(&page, false)
        } else {
            page.clone()
        };
        let _ = std::fs::write(root.join("index.html"), &html);
        let view = mac::make_view(env.mtm, &cfg, 800.0, 600.0);
        unsafe {
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*env.spy)));
            view.setUIDelegate(Some(ProtocolObject::from_ref(&*env.spy)));
        }
        env.win.setContentView(Some(&view));
        mac::st(|s| {
            s.app_view = Retained::as_ptr(&view) as usize;
            s.refused.clear();
            s.scheme_hits.clear();
        });
        let log0 = env.srv.log.all().len();
        let udp0 = env.srv.udp_log.all().len();
        mac::load(&view, "flare-app://app/index.html");
        mac::spin(&env.app, 4_000);
        let r = mac::eval(
            &env.app,
            &view,
            None,
            &page_world(env),
            "JSON.stringify(window.__r)",
            3_000,
        );
        let results: Value = match &r {
            Ok(Value::String(s)) => serde_json::from_str(s).unwrap_or(json!(s)),
            other => json!(format!("{other:?}")),
        };
        let hits: Vec<String> = env.srv.log.all()[log0..]
            .iter()
            .filter(|l| l.contains("sbx-"))
            .cloned()
            .collect();
        let udp: Vec<String> = env.srv.udp_log.all()[udp0..].to_vec();
        let refused = mac::st(|s| s.refused.clone());
        let scheme = mac::st(|s| s.scheme_hits.clone());
        let leaks_in_page: Vec<String> = results
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, v)| v.as_str().is_some_and(|s| s.contains("LEAK")))
            .map(|(k, _)| k.clone())
            .collect();
        let sealed = hits.is_empty() && udp.is_empty() && leaks_in_page.is_empty();
        let evidence = json!({"page_results": results, "server_hits": hits, "udp_packets": udp, "refused_navigations": refused,
                              "scheme_handler_requests": scheme, "leaks_seen_by_page": leaks_in_page, "final_url": mac::url_of(&view)});
        if name.starts_with('A') {
            rep.note(
                &format!("6.{name}"),
                if sealed {
                    "control leaked nothing (the test would prove nothing)"
                } else {
                    "control: without walls the page reaches the network (the test is sensitive)"
                },
                evidence,
            );
        } else if name.starts_with('C') {
            rep.note(
                &format!("6.{name}"),
                if sealed {
                    "the dead proxy alone stops everything, loopback included"
                } else {
                    "the dead proxy alone does not stop everything"
                },
                evidence,
            );
        } else {
            rep.add(&format!("6.{name}"), sealed, "mini-app: no request leaves (server and UDP see nothing), no file://, no other app, no WebRTC", evidence);
        }
        if name.starts_with('D') {
            app_store_for_isolation = Some(store);
        }
        // Tear the view down and let its process go, so a peer connection of this variant cannot
        // send into the next one's count.
        mac::load(&view, "about:blank");
        mac::spin(&env.app, 300);
        unsafe {
            view.setNavigationDelegate(None);
            view.setUIDelegate(None);
        }
        env.win.setContentView(Some(&env.wv));
        drop(view);
        mac::spin(&env.app, 3_000);
        mac::st(|s| s.app_view = 0);
    }
    env.win.setContentView(Some(&env.wv));
    env.win.makeFirstResponder(Some(&env.wv));

    // Isolation: the panel's cookie is not in the app's store.
    mac::load_wait(&env.app, &env.wv, &format!("{}/setcookie", env.base()));
    let panel_cookies = mac::cookies(&env.app, &panel_store);
    let app_cookies = app_store_for_isolation
        .as_ref()
        .map(|s| mac::cookies(&env.app, s))
        .unwrap_or_default();
    rep.add(
        "6.isolation",
        panel_cookies.iter().any(|c| c.starts_with("spike=panel")) && app_cookies.is_empty(),
        "the mini-app's non-persistent WKWebsiteDataStore shares nothing with the web panel's",
        json!({"panel_cookies": panel_cookies, "app_cookies": app_cookies}),
    );
    let https = unsafe {
        WKWebView::handlesURLScheme(&objc2_foundation::NSString::from_str("https"), env.mtm)
    };
    rep.note(
        "6.https-interception",
        "WKWebView handles https itself, so no WKURLSchemeHandler can serve https://*.local: mods and apps need a custom scheme origin",
        json!({"handlesURLScheme(https)": https}),
    );
    mac::remove_rules(&env.app, "flare-spike-app");
}

// ---- 7. stop on the person's own input -----------------------------------------------------------------

fn user_input(env: &Env, rep: &mut Report) {
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs) = fresh_look(env, rep, &mut shim, "user-look.png");
    let monitor = mac::install_monitor();
    mac::MONITOR_ON.with(|m| m.set(true));
    mac::st(|s| {
        s.monitor.clear();
        s.page_keys_swallowed = 0;
    });
    let a1 = click_ref(&mut shim, &v, &refs, "Press me");
    let a2 = act(
        &mut shim,
        Op::Type {
            text: "agent".into(),
            target: ref_named(&v, "Name"),
        },
        &refs,
    );
    let a3 = press(&mut shim, &refs, "Enter");
    let a4 = act(
        &mut shim,
        Op::Scroll {
            dx: 0.0,
            dy: 100.0,
            target: None,
        },
        &refs,
    );
    mac::spin(&env.app, 300);
    let seen_agent = mac::st(|s| s.monitor.clone());
    plog(env);

    // The person's own events come from the window server through NSApp's queue; postEvent stands in.
    let user_key = mac::tagged_key_event(7, true, "x", false);
    env.app.postEvent_atStart(&user_key, false);
    let tagged = mac::tagged_key_event(7, true, "x", true);
    env.app.postEvent_atStart(&tagged, false);
    let mouse = mac::user_mouse_event(
        &env.wv,
        objc2_app_kit::NSEventType::LeftMouseDown,
        30.0,
        30.0,
    );
    env.app.postEvent_atStart(&mouse, false);
    let cmd_t = mac::key_event(
        &env.wv,
        true,
        "t",
        "t",
        17,
        objc2_app_kit::NSEventModifierFlags::Command,
    );
    env.app.postEvent_atStart(&cmd_t, false);
    mac::spin(&env.app, 500);
    let seen = mac::st(|s| s.monitor.clone());
    let swallowed = mac::st(|s| s.page_keys_swallowed);
    let page_saw = plog(env);
    let buttons = objc2_app_kit::NSEvent::pressedMouseButtons();
    mac::MONITOR_ON.with(|m| m.set(false));
    unsafe { objc2_app_kit::NSEvent::removeMonitor(&monitor) };
    let user_lines = seen.iter().filter(|l| l.starts_with("user:")).count();
    let agent_lines = seen.iter().filter(|l| l.starts_with("agent:")).count();
    rep.add(
        "7.stop-on-user-input",
        seen_agent.is_empty() && user_lines >= 3 && agent_lines == 1 && !page_saw.iter().any(|e| e["meta"] == true && e["k"] == "t"),
        "a local NSEvent monitor sees the person's events and never the agent's (sent straight to the view); a tagged event is told apart; the app's chord is taken before the page",
        json!({"agent_jobs_ok": [a1.is_ok(), a2.is_ok(), a3.is_ok(), a4.is_ok()], "monitor_during_agent": seen_agent,
               "monitor_after_user_events": seen, "swallowed": swallowed, "page_saw_after": brief(&page_saw),
               "NSEvent.pressedMouseButtons": buttons}),
    );
}

// ---- 8. everything else the Windows host uses ------------------------------------------------------------

fn others(env: &Env, rep: &mut Report) {
    let mut shim = Shim::new(&env.app, &env.wv);
    mac::st(|s| s.messages.clear());
    let (v, refs) = fresh_look(env, rep, &mut shim, "others-look.png");
    let console = mac::st(|s| s.messages.clone());
    rep.add(
        "8.console",
        console.iter().any(|m| m.contains("main page console line")),
        "console capture: a page-world user script posts console lines to a WKScriptMessageHandler",
        json!({"messages": console}),
    );

    mac::st(|s| s.dialogs.clear());
    let r = click_ref(&mut shim, &v, &refs, "Dialogs");
    mac::spin(&env.app, 400);
    let dialogs = mac::st(|s| s.dialogs.clone());
    let conf = pjs(env, "window.__confirm");
    let prom = pjs(env, "window.__prompt");
    rep.add(
        "8.dialogs",
        dialogs.len() == 3 && conf == false && prom == "answered by the host",
        "alert, confirm and prompt reach the WKUIDelegate and are answered without any window",
        json!({"job": r, "dialogs": dialogs, "confirm_returned": conf, "prompt_returned": prom}),
    );

    let up = rep.out.join("upload-me.txt");
    let _ = std::fs::write(&up, "upload");
    mac::st(|s| {
        s.open_panel.clear();
        s.open_panel_file = Some(up.clone());
    });
    let r = click_ref(&mut shim, &v, &refs, "Upload");
    mac::spin(&env.app, 500);
    let panel = mac::st(|s| s.open_panel.clone());
    let file = pjs(env, "window.__file");
    rep.add(
        "8.file-upload",
        !panel.is_empty() && file == "upload-me.txt",
        "a file input's picker goes to runOpenPanelWithParameters; the host answers with a file, no panel is shown",
        json!({"job": r, "open_panel": panel, "page_got_file": file}),
    );

    mac::st(|s| s.media.clear());
    let r = click_ref(&mut shim, &v, &refs, "Camera");
    mac::wait_until(&env.app, 5_000, || {
        pjs(env, "window.__cam === undefined") == false
    });
    let media = mac::st(|s| s.media.clone());
    let cam = pjs(env, "window.__cam");
    let md = pjs(
        env,
        "typeof navigator.mediaDevices + ' / ' + (navigator.mediaDevices ? typeof navigator.mediaDevices.getUserMedia : '-')",
    );
    let console_now = mac::st(|s| s.messages.clone());
    // A note: in this unbundled binary navigator.mediaDevices is undefined, so the delegate is never
    // asked and nothing is proved about the deny path (retest in the signed .app).
    rep.note(
        "8.permissions",
        "camera request: denied by the WKUIDelegate, or the API absent (then the delegate is not exercised)",
        json!({"denied_or_absent": cam.as_str().is_some_and(|c| c.starts_with("denied")) || md.as_str().is_some_and(|m| m.starts_with("undefined")), "job": r, "delegate_calls": media, "page_result": cam, "mediaDevices": md, "console": console_now}),
    );

    mac::st(|s| s.new_windows.clear());
    let r1 = click_ref(&mut shim, &v, &refs, "Open in new window");
    mac::spin(&env.app, 1_000);
    let url1 = mac::url_of(&env.wv);
    let (v2, refs2) = fresh_look(env, rep, &mut shim, "others-look2.png");
    let r2 = click_ref(&mut shim, &v2, &refs2, "window.open");
    mac::spin(&env.app, 1_000);
    let url2 = mac::url_of(&env.wv);
    let nw = mac::st(|s| s.new_windows.clone());
    rep.add(
        "8.new-window",
        nw.len() == 2 && url1.ends_with("/page2") && url2.contains("wo=1"),
        "target=_blank and window.open reach createWebViewWithConfiguration; the page opens in the same view, no window",
        json!({"jobs": [r1, r2], "requests": nw, "url_after_blank": url1, "url_after_window_open": url2}),
    );

    mac::st(|s| s.refused.clear());
    let (v3, refs3) = fresh_look(env, rep, &mut shim, "others-look3.png");
    let r = click_ref(&mut shim, &v3, &refs3, "Mail someone");
    mac::spin(&env.app, 500);
    let refused = mac::st(|s| s.refused.clone());
    rep.add(
        "8.external-scheme",
        refused.iter().any(|u| u.starts_with("mailto:")),
        "mailto: and other schemes are cancelled in decidePolicyForNavigationAction (no app is launched)",
        json!({"job": r, "refused": refused}),
    );

    unsafe {
        env.wv
            .setCustomUserAgent(Some(&objc2_foundation::NSString::from_str(
                "FlareSpike/1.0 (Macintosh)",
            )))
    };
    mac::load_wait(&env.app, &env.wv, &format!("{}/page2", env.base()));
    let ua = pjs(env, "navigator.userAgent");
    unsafe { env.wv.setCustomUserAgent(None) };
    rep.add(
        "8.user-agent",
        ua == "FlareSpike/1.0 (Macintosh)",
        "customUserAgent",
        json!({"navigator.userAgent": ua}),
    );

    unsafe { env.wv.setPageZoom(1.5) };
    let (vz, refsz) = fresh_look(env, rep, &mut shim, "zoom-look.png");
    plog(env);
    let r = click_ref(&mut shim, &vz, &refsz, "Press me");
    let out = pjs(env, "document.getElementById('out').textContent");
    unsafe { env.wv.setPageZoom(1.0) };
    rep.add(
        "8.zoom",
        out == "clicked b1 true",
        "pageZoom 1.5 (Windows SetZoomFactor): the look's viewport shrinks in CSS pixels and a ref click still lands",
        json!({"viewport": vz["viewport"], "image": vz["image"], "job": r, "out": out}),
    );

    let pdf = mac::pdf(&env.app, &env.wv);
    let pdf_path = rep.out.join("page.pdf");
    if let Ok(b) = &pdf {
        let _ = std::fs::write(&pdf_path, b);
    }
    rep.note(
        "8.print-to-pdf",
        "createPDFWithConfiguration (Windows does not use print-to-PDF; recorded for completeness)",
        json!({"ok": pdf.is_ok(), "bytes": pdf.as_ref().map(Vec::len).ok(), "path": pdf_path}),
    );

    unsafe { env.wv.setInspectable(true) };
    let insp = unsafe { env.wv.isInspectable() };
    unsafe { env.wv.setInspectable(false) };
    rep.note(
        "8.devtools",
        "isInspectable toggles Web Inspector (Windows SetAreDevToolsEnabled); there is no DevTools protocol to call into",
        json!({"inspectable_after_set": insp}),
    );

    let fs = pjs(env, "document.fullscreenEnabled");
    rep.note(
        "8.fullscreen",
        "element full screen is off by default in WKWebView (WKPreferences.elementFullscreenEnabled); turning it on was not tested (it takes the screen)",
        json!({"document.fullscreenEnabled": fs}),
    );

    // eval: value, promise, exception, then a runaway script.
    let e1 = act(
        &mut shim,
        Op::Eval {
            expression: "document.title".into(),
        },
        &RefMap::default(),
    );
    let e2 = act(
        &mut shim,
        Op::Eval {
            expression:
                "new Promise(function(r){setTimeout(function(){r({a:[1,'x',true],n:null})},100)})"
                    .into(),
        },
        &RefMap::default(),
    );
    let e3 = act(
        &mut shim,
        Op::Eval {
            expression: "throw new Error('boom')".into(),
        },
        &RefMap::default(),
    );
    rep.add(
        "8.eval",
        e1.is_ok() && e2.as_ref().is_ok_and(|v| v.to_string().contains("[1,\"x\",true]")) && e3.as_ref().is_err_and(|e| e.contains("boom")),
        "browser_eval: callAsyncJavaScript in the page world, awaiting promises; exceptions come back as the script threw",
        json!({"title": e1, "promise": e2, "throw": e3}),
    );
    // A runaway script, in a view of its own: what recovers it with public API only?
    let cfg = unsafe { env.wv.configuration() };
    let tv = mac::make_view(env.mtm, &cfg, 800.0, 600.0);
    unsafe { tv.setNavigationDelegate(Some(ProtocolObject::from_ref(&*env.spy))) };
    env.win.setContentView(Some(&tv));
    mac::load_wait(&env.app, &tv, &format!("{}/main", env.base()));
    let pid_hung = mac::web_pid(&tv);
    let t = std::time::Instant::now();
    let e4 = {
        let mut s2 = Shim::new(&env.app, &tv);
        act(
            &mut s2,
            Op::Eval {
                expression: "while(true){}".into(),
            },
            &RefMap::default(),
        )
    };
    let runaway_ms = t.elapsed().as_millis();
    let alive = mac::eval(&env.app, &tv, None, &page_world(env), "1+1", 2_000);
    let f = mac::st(|s| s.finished + s.failed);
    unsafe { tv.reload() };
    let reload_finished =
        mac::wait_until(&env.app, 6_000, || mac::st(|s| s.finished + s.failed) > f);
    let alive_after_reload = mac::eval(&env.app, &tv, None, &page_world(env), "1+1", 2_000);
    // Public route: drop the view and make a new one (same configuration, same site).
    unsafe { tv.setNavigationDelegate(None) };
    drop(tv);
    let nv = mac::make_view(env.mtm, &cfg, 800.0, 600.0);
    unsafe { nv.setNavigationDelegate(Some(ProtocolObject::from_ref(&*env.spy))) };
    env.win.setContentView(Some(&nv));
    let loaded_new = mac::load_wait(&env.app, &nv, &format!("{}/main", env.base()));
    let alive_new_view = mac::eval(&env.app, &nv, None, &page_world(env), "1+1", 2_000);
    let pid_new = mac::web_pid(&nv);
    // Clean up: the hung process is killed (test hygiene; the SPI pid is only used here).
    let hung_still_running = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid_hung.to_string())
        .status()
        .is_ok_and(|s| s.success());
    let _ = std::process::Command::new("kill")
        .arg("-9")
        .arg(pid_hung.to_string())
        .status();
    unsafe { nv.setNavigationDelegate(None) };
    drop(nv);
    env.win.setContentView(Some(&env.wv));
    env.win.makeFirstResponder(Some(&env.wv));
    // The page answering again is what Windows gives (CDP's timeout stops the script); it fails here, and
    // a new view only recovers the panel, not the page. Recorded as a failed parity check: the gap.
    rep.add(
        "8.eval-runaway",
        alive.is_ok(),
        "a runaway browser_eval: CDP's timeout stops the script on Windows and the page answers again; WebKit has no public way to stop one (a new view, a new process, recovers the panel but loses the page)",
        json!({"job": e4, "ms": runaway_ms, "page_answers_after": alive, "reload_finished": reload_finished,
               "page_answers_after_reload": alive_after_reload, "new_view_loaded": loaded_new, "new_view_answers": alive_new_view,
               "pid_hung": pid_hung, "pid_new_view": pid_new, "hung_process_still_running_before_cleanup": hung_still_running}),
    );

    // Process failure (Windows ProcessFailed): kill the content process, see the delegate, reload.
    mac::load_wait(&env.app, &env.wv, &format!("{}/page2", env.base()));
    let term0 = mac::st(|s| s.terminated);
    let pid = mac::web_pid(&env.wv);
    let _ = std::process::Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .status();
    let told = mac::wait_until(&env.app, 5_000, || mac::st(|s| s.terminated) > term0);
    let back = mac::load_wait(&env.app, &env.wv, &format!("{}/main", env.base()));
    rep.add(
        "8.process-gone",
        told && back,
        "webViewWebContentProcessDidTerminate fires when the page's process dies; the next load starts a new one",
        json!({"pid": pid, "delegate_told": told, "reloaded": back, "new_pid": mac::web_pid(&env.wv)}),
    );

    // A per-profile persistent store (Windows SetProfileName): macOS 14+ dataStoreForIdentifier.
    let uuid = objc2_foundation::NSUUID::new();
    let store = unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&uuid, env.mtm) };
    let persistent = unsafe { store.isPersistent() };
    let id_back = unsafe { store.identifier() }.map(|u| u.UUIDString().to_string());
    let removed = std::rc::Rc::new(std::cell::Cell::new(false));
    let r2 = std::rc::Rc::clone(&removed);
    let block = block2::RcBlock::new(move |_e: *mut objc2_foundation::NSError| r2.set(true));
    drop(store);
    unsafe {
        WKWebsiteDataStore::removeDataStoreForIdentifier_completionHandler(&uuid, &block, env.mtm)
    };
    mac::wait_until(&env.app, 5_000, || removed.get());
    rep.add(
        "8.profiles",
        persistent && id_back.is_some(),
        "a persistent data store per profile (dataStoreForIdentifier, macOS 14+), removed again after the test",
        json!({"persistent": persistent, "identifier": id_back, "removed": removed.get()}),
    );
}

// ---- real pages (needs the internet; recorded as notes) -----------------------------------------------

fn real_sites(env: &Env, rep: &mut Report) {
    for (i, url) in [
        "https://en.wikipedia.org/wiki/WebKit",
        "https://github.com/Maxaubert",
        "https://news.ycombinator.com/",
    ]
    .iter()
    .enumerate()
    {
        let loaded = mac::load_wait(&env.app, &env.wv, url);
        mac::spin(&env.app, 1_500);
        let mut shim = Shim::new(&env.app, &env.wv);
        let t = std::time::Instant::now();
        let (v, refs) = do_look(env, &mut shim, rep, &format!("real-{i}.png"));
        let ms = t.elapsed().as_millis();
        let list = names(&v);
        rep.note(
            &format!("real.{i}"),
            &format!("a look at {url}"),
            json!({"loaded": loaded, "look_ms": ms, "count": list.len(), "refs": refs.len(), "truncated": v["truncated"],
                   "image": v["image"], "image_error": v["image_error"], "first_25": list.iter().take(25).collect::<Vec<_>>(),
                   "frames": shim.frame_labels(), "notes": shim.notes, "error": v["error"]}),
        );
    }
}
