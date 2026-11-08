//! The second round: the cases a skeptical review of the first verdict raised (2026-11-08). Strict CSP
//! sites, a page that traps the agent's own questions, hover and pointer events, more ways out of a
//! no-network mini-app, what bypasses the networked apps' proxy (and the rule list that stops it),
//! sandboxed iframes, permissions without a public delegate, `window.close()` and `beforeunload`.
//!
//! `adv-exfil` and `adv-proxy` need the network observers (`observers.py`, started here as a child
//! process): an mDNS sniffer, a raw TCP accept logger, a logging CONNECT proxy and a dual-stack HTTP
//! server.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Instant;

use flare_surfaces::agent_job::{Op, RefMap, Target};
use flare_surfaces::agent_web::{Button, WaitFor};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_web_kit::{
    WKContentRuleList, WKContentWorld, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use serde_json::{Value, json};

use crate::mac;
use crate::shim::{Shim, run_job};
use crate::tests::{Env, Report, panel_config};

const TCP: u16 = 47101;
const PROXY: u16 = 47102;
const HTTP: u16 = 47103;

/// The extra server pages: (extra headers, body).
pub fn page(route: &str) -> Option<(String, String)> {
    let logger = r#"<script>window.__log=[];['pointerover','pointerenter','mouseover','mouseenter','pointermove','mousemove','pointerdown','mousedown','pointerup','mouseup','click','mouseout','pointerout','mouseleave'].forEach(function(t){document.addEventListener(t,function(e){var t2=e.target||{};window.__log.push(e.type+':'+e.isTrusted+':'+(t2.id||''));},true);});</script>"#;
    let p = |h: &str, b: String| Some((h.to_string(), b));
    match route {
        "/adv/csp" => p(
            "Content-Security-Policy: script-src 'self'; object-src 'none'\r\n",
            "<!doctype html><title>CSP page</title><h1>Strict</h1><button id=b>Go</button><p>hello csp</p>".into(),
        ),
        "/adv/trap" => p(
            "",
            "<!doctype html><title>Trap</title><button>Trap button</button><p>trap text</p><script>Object.defineProperty(window,'devicePixelRatio',{get:function(){while(true){}}});</script>".into(),
        ),
        "/adv/hover" => p(
            "",
            format!(
                "<!doctype html><title>Hover</title>{logger}<style>#h:hover{{background:red}} #m ul{{display:none}} #m:hover ul{{display:block}} #m{{width:200px;height:40px;border:1px solid}}</style>\
<button id=h style='margin:40px'>Hover me</button><div id=m role=button aria-label='Menu'>Menu<ul><li><a id=hid href='#hidden'>Hidden item</a></li></ul></div>\
<script>document.getElementById('h').addEventListener('click',function(){{window.__hover=document.getElementById('h').matches(':hover');}});</script>"
            ),
        ),
        "/adv/pointerui" => p(
            "",
            "<!doctype html><title>Pointer UI</title><button id=p>Open menu</button><div id=out>closed</div><script>document.getElementById('p').addEventListener('pointerdown',function(e){if(e.isTrusted)document.getElementById('out').textContent='menu opened by pointerdown';});</script>".into(),
        ),
        "/adv/sbframe" => p(
            "",
            "<!doctype html><title>Sandboxed frame</title><p>top</p><iframe sandbox='allow-same-origin' style='width:400px;height:150px' src='/adv/inner'></iframe><iframe sandbox style='width:400px;height:100px' srcdoc=\"<button>Srcdoc button</button> <a href='/page2?from=srcdoc'>Srcdoc link</a>\"></iframe><iframe style='width:400px;height:100px' src='/adv/inner2'></iframe>".into(),
        ),
        "/adv/inner2" => p("", "<!doctype html><title>inner2</title><button>Plain frame button</button>".into()),
        "/adv/inner" => p(
            "",
            "<!doctype html><title>inner</title><a href='/page2'>Inner link</a><button>Inner button</button>".into(),
        ),
        "/adv/close" => p(
            "",
            "<!doctype html><title>Close</title><button id=c onclick=\"window.__closeCalled=1;window.close()\">Close me</button><a href='/page2'>Leave</a><script>addEventListener('beforeunload',function(e){console.log('beforeunload ran');e.preventDefault();e.returnValue='stay';return 'stay';});</script>".into(),
        ),
        "/adv/perms" => p("", "<!doctype html><title>Perms</title><button id=g>Go</button>".into()),
        _ => None,
    }
}

fn names(v: &Value) -> Vec<String> {
    v["elements"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| format!("{} {} {:?}", e["ref"], e["role"], e["name"]))
                .collect()
        })
        .unwrap_or_default()
}

fn ref_named(v: &Value, n: &str) -> Option<String> {
    v["elements"].as_array()?.iter().find_map(|e| {
        (e["name"].as_str() == Some(n)).then(|| e["ref"].as_str().unwrap_or("").to_string())
    })
}

fn look(shim: &mut Shim<'_>) -> (Value, RefMap, u128) {
    let t = Instant::now();
    let (r, refs) = run_job(shim, Op::Look { picture: false }, &RefMap::default());
    let v = r.unwrap_or_else(|e| json!({"error": e.message}));
    (v, refs.unwrap_or_default(), t.elapsed().as_millis())
}

fn click(shim: &mut Shim<'_>, refs: &RefMap, target: Target) -> String {
    let (r, _) = run_job(
        shim,
        Op::Click {
            target,
            button: Button::Left,
            double: false,
        },
        refs,
    );
    format!("{r:?}")
}

fn pw(env: &Env) -> Retained<WKContentWorld> {
    unsafe { WKContentWorld::pageWorld(env.mtm) }
}

fn jstr(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"err": e}),
    }
}

/// A web panel view of its own (its own content process), shown in the host window.
fn own_view(env: &Env, extra_page_js: Option<&str>) -> Retained<WKWebView> {
    let store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(env.mtm) };
    let cfg = panel_config(env.mtm, &env.spy, &store);
    if let Some(js) = extra_page_js {
        let ucc = unsafe { cfg.userContentController() };
        unsafe { ucc.addUserScript(&mac::user_script(env.mtm, js, false, &pw(env))) };
    }
    let wv = mac::make_view(env.mtm, &cfg, 1000.0, 700.0);
    unsafe {
        wv.setNavigationDelegate(Some(ProtocolObject::from_ref(&*env.spy)));
        wv.setUIDelegate(Some(ProtocolObject::from_ref(&*env.spy)));
    }
    env.win.setContentView(Some(&wv));
    wv
}

fn drop_view(env: &Env, wv: Retained<WKWebView>, kill: bool) {
    env.win.setContentView(Some(&env.wv));
    unsafe {
        wv.setNavigationDelegate(None);
        wv.setUIDelegate(None);
    }
    let pid = mac::web_pid(&wv);
    drop(wv);
    // Test hygiene only: a hung content process is killed (the pid is SPI, used nowhere else).
    if kill && pid > 0 {
        let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
    }
    mac::spin(&env.app, 500);
}

// ---- A. strict CSP: browser_eval, browser_wait and the look's ratio ----------------------------------

pub fn csp(env: &Env, rep: &mut Report) {
    let ok = mac::load_wait(&env.app, &env.wv, &format!("{}/adv/csp", env.base()));
    mac::spin(&env.app, 500);
    let mut shim = Shim::new(&env.app, &env.wv);
    // The old route, for the record: a string handed to eval() in the page's world.
    let old = mac::call_async(
        &env.app,
        &env.wv,
        None,
        &pw(env),
        "return (0, eval)('1+1')",
        3000,
    );
    let t = Instant::now();
    let (w, _) = run_job(
        &mut shim,
        Op::Wait {
            what: WaitFor::Text("hello csp".into()),
            timeout_ms: 3000,
        },
        &RefMap::default(),
    );
    let wait_ms = t.elapsed().as_millis();
    let ev = |shim: &mut Shim<'_>, e: &str| {
        let (r, _) = run_job(
            shim,
            Op::Eval {
                expression: e.into(),
            },
            &RefMap::default(),
        );
        r.map_err(|e| e.message)
    };
    let e_title = ev(&mut shim, "document.title");
    let e_promise = ev(
        &mut shim,
        "new Promise(r => setTimeout(() => r({n: 3}), 50))",
    );
    let e_stmt = ev(&mut shim, "var q = 5; q + 1");
    let e_throw = ev(&mut shim, "(() => { throw new Error('boom') })()");
    let e_stmt_promise = ev(&mut shim, "var p = Promise.resolve(7); p");
    let e_multi = ev(&mut shim, "var a1 = 4;\nvar a2 = a1 * 2;\na2\n+ 1");
    let e_global = ev(&mut shim, "a1");
    let e_block = ev(&mut shim, "if (true) { 9 }");
    let e_str = ev(&mut shim, "var s1 = 'x; y'; s1");
    let (lv, _, _) = look(&mut shim);
    let scale = env.win.backingScaleFactor();
    let ok_all = ok
        && w.as_ref().is_ok_and(|v| v["met"] == true)
        && e_title
            .as_ref()
            .is_ok_and(|v| v.to_string().contains("CSP page"))
        && e_promise
            .as_ref()
            .is_ok_and(|v| v.to_string().contains('3'))
        && e_stmt.as_ref().is_ok_and(|v| v.to_string().contains('6'))
        && e_throw.as_ref().is_err_and(|e| e.contains("boom"))
        && lv["viewport"]["device_scale"].as_f64() == Some(scale);
    rep.add(
        "A.csp-eval-wait",
        ok_all,
        "on a page whose CSP has no 'unsafe-eval': browser_wait finds text, browser_eval answers expressions, promises, statements and exceptions, the look's device_scale is right",
        json!({"old_route_eval_string": jstr(old), "wait": format!("{w:?}"), "wait_ms": wait_ms,
               "eval_title": e_title, "eval_promise": e_promise, "eval_statements": e_stmt, "eval_throw": e_throw,
               "look_viewport": lv["viewport"], "backing_scale": scale}),
    );
    let has = |r: &Result<Value, String>, x: &str| {
        r.as_ref()
            .is_ok_and(|v| serde_json::from_str::<Value>(x).is_ok_and(|x| v["value"] == x))
    };
    rep.add(
        "A.csp-statement-promise",
        has(&e_stmt_promise, "7") && has(&e_multi, "9") && has(&e_global, "4") && has(&e_block, "9") && has(&e_str, "\"x; y\""),
        "statement scripts as CDP runs them: a completion value that is a promise is awaited, the last expression spans lines, var makes a global, a block's completion value, a ; inside a string",
        json!({"stmt_promise": e_stmt_promise, "multi_line": e_multi, "global_after": e_global, "block": e_block, "string_with_semicolon": e_str}),
    );
    let ok2 = mac::load_wait(&env.app, &env.wv, "https://github.com/login");
    mac::spin(&env.app, 1500);
    let mut shim = Shim::new(&env.app, &env.wv);
    let (w2, _) = run_job(
        &mut shim,
        Op::Wait {
            what: WaitFor::Selector("input".into()),
            timeout_ms: 4000,
        },
        &RefMap::default(),
    );
    let e2 = ev(&mut shim, "location.host");
    let e3 = ev(
        &mut shim,
        "fetch('/login', {method: 'HEAD'}).then(r => r.status)",
    );
    let (lv2, _, _) = look(&mut shim);
    rep.add(
        "A.github-wait-eval",
        w2.as_ref().is_ok_and(|v| v["met"] == true)
            && e2.as_ref().is_ok_and(|v| v.to_string().contains("github.com"))
            && e3.is_ok()
            && lv2["viewport"]["device_scale"].as_f64() == Some(scale),
        "github.com/login (strict CSP): browser_wait selector, browser_eval (value and an awaited fetch), device_scale",
        json!({"loaded": ok2, "wait": format!("{w2:?}"), "eval_host": e2, "eval_fetch": e3,
               "look_viewport": lv2["viewport"], "elements": lv2["elements"].as_array().map(Vec::len)}),
    );
}

// ---- B. a page that traps a getter the agent touches --------------------------------------------------

pub fn trap(env: &Env, rep: &mut Report) {
    let wv = own_view(env, None);
    let ok = mac::load_wait(&env.app, &wv, &format!("{}/adv/trap", env.base()));
    mac::spin(&env.app, 500);
    let mut shim = Shim::new(&env.app, &wv);
    let (v1, _, ms1) = look(&mut shim);
    let (v2, _, ms2) = look(&mut shim);
    let (w, _) = run_job(
        &mut shim,
        Op::Wait {
            what: WaitFor::Text("trap text".into()),
            timeout_ms: 3000,
        },
        &RefMap::default(),
    );
    let scale = env.win.backingScaleFactor();
    rep.add(
        "B.trap-fixed-questions",
        v1.get("error").is_none()
            && v2.get("error").is_none()
            && ref_named(&v2, "Trap button").is_some()
            && v2["viewport"]["device_scale"].as_f64() == Some(scale)
            && w.as_ref().is_ok_and(|v| v["met"] == true),
        "a page whose devicePixelRatio getter never returns: the look (twice) and a wait still answer, with the right ratio (fixed questions in the agent's world)",
        json!({"loaded": ok, "first_ms": ms1, "first": names(&v1), "second_ms": ms2, "second": names(&v2),
               "viewport": v2["viewport"], "wait": format!("{w:?}")}),
    );
    // browser_eval is the page's world by design (the agent asks about the page's own state). A getter that
    // never returns, or a page function that never returns, is where Windows and the Mac part.
    let t = Instant::now();
    let (e, _) = run_job(
        &mut shim,
        Op::Eval {
            expression: "window.devicePixelRatio".into(),
        },
        &RefMap::default(),
    );
    let eval_ms = t.elapsed().as_millis();
    let agent_after = mac::eval(&env.app, &wv, None, &mac::agent_world(env.mtm), "1", 3000);
    let (v3, _, ms3) = look(&mut shim);
    let stopped = unsafe { wv.isLoading() };
    unsafe { wv.stopLoading() };
    let agent_after_stop = mac::eval(&env.app, &wv, None, &mac::agent_world(env.mtm), "1", 2000);
    rep.add(
        "B.trap-eval",
        agent_after.is_ok() && v3.get("error").is_none(),
        "browser_eval that runs into a page getter that never returns: Windows' CDP timeout stops the script and the page answers again; does WebKit's page?",
        json!({"eval": format!("{e:?}"), "eval_ms": eval_ms, "agent_world_after": jstr(agent_after),
               "look_after_ms": ms3, "look_after": v3.get("error").cloned().unwrap_or(json!(names(&v3))),
               "was_loading": stopped, "agent_world_after_stopLoading": jstr(agent_after_stop),
               "public_api_that_stops_a_script": "none (WKWebView has no terminate, interrupt or watchdog; stopLoading, reload and loadRequest queue behind the script)"}),
    );
    drop_view(env, wv, true);
}

// ---- C. pointer events and hover from an agent click ------------------------------------------------

fn hover_round(env: &Env, rep: &mut Report, label: &str) {
    let ok = mac::load_wait(&env.app, &env.wv, &format!("{}/adv/hover", env.base()));
    mac::spin(&env.app, 400);
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs, _) = look(&mut shim);
    let c = click(
        &mut shim,
        &refs,
        Target::Ref(ref_named(&v, "Hover me").unwrap_or_default()),
    );
    mac::spin(&env.app, 300);
    let log = mac::eval(
        &env.app,
        &env.wv,
        None,
        &pw(env),
        "JSON.stringify(window.__log.splice(0))",
        2000,
    );
    let hover = mac::eval(
        &env.app,
        &env.wv,
        None,
        &pw(env),
        "String(window.__hover)",
        2000,
    );
    let (v2, refs2, _) = look(&mut shim);
    let c2 = click(
        &mut shim,
        &refs2,
        Target::Ref(ref_named(&v2, "Menu").unwrap_or_default()),
    );
    mac::spin(&env.app, 300);
    let (v3, _, _) = look(&mut shim);
    let menu_open = ref_named(&v3, "Hidden item").is_some();
    let log_s = match &log {
        Ok(Value::String(s)) => s.clone(),
        o => format!("{o:?}"),
    };
    let has = |t: &str| log_s.contains(&format!("{t}:true:h"));
    let pointer_ok =
        has("pointerdown") && has("pointerup") && has("pointerover") && has("mouseover");
    rep.add(
        &format!("C.hover-{label}"),
        pointer_ok && hover == Ok(json!("true")) && menu_open,
        "an agent click fires trusted pointer and hover events, :hover holds, a CSS hover menu opens",
        json!({"loaded": ok, "click": c, "events": log_s, "hover_at_click": jstr(hover),
               "menu_click": c2, "hover_menu_item_listed": menu_open}),
    );
    let _ = mac::load_wait(&env.app, &env.wv, &format!("{}/adv/pointerui", env.base()));
    mac::spin(&env.app, 300);
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v4, refs4, _) = look(&mut shim);
    let c4 = click(
        &mut shim,
        &refs4,
        Target::Ref(ref_named(&v4, "Open menu").unwrap_or_default()),
    );
    mac::spin(&env.app, 200);
    let out = mac::eval(
        &env.app,
        &env.wv,
        None,
        &pw(env),
        "document.getElementById('out').textContent",
        2000,
    );
    rep.add(
        &format!("C.pointerdown-{label}"),
        out == Ok(json!("menu opened by pointerdown")),
        "a control that acts on a trusted pointerdown (Radix-style menus) opens from an agent click",
        json!({"click": c4, "out": jstr(out)}),
    );
}

pub fn hover(env: &Env, rep: &mut Report) {
    hover_round(env, rep, "key-visible");
    mac::FAKE_KEY.with(|k| k.set(false));
    hover_round(env, rep, "not-key");
    mac::FAKE_VISIBLE.with(|k| k.set(false));
    hover_round(env, rep, "not-key-not-visible");
    mac::FAKE_KEY.with(|k| k.set(true));
    mac::FAKE_VISIBLE.with(|k| k.set(true));
}

// ---- the network observers -----------------------------------------------------------------------------

struct Observers {
    child: Child,
    dir: PathBuf,
}

impl Observers {
    fn start(out: &Path) -> Option<Self> {
        let dir = out.join("netlogs");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/wk_spike/observers.py"
        );
        let child = Command::new("python3")
            .arg("-I")
            .arg(script)
            .arg(&dir)
            .args([TCP, PROXY, HTTP].map(|p| p.to_string()))
            .arg("900")
            .spawn()
            .ok()?;
        let up = dir.join("start.log");
        let t = Instant::now();
        while !up.exists() && t.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        up.exists().then_some(Observers { child, dir })
    }

    fn lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.dir.join(format!("{name}.log")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for Observers {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn cmd_out(c: &str, args: &[&str]) -> String {
    Command::new(c)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn lan_address() -> String {
    ["en0", "en1", "en2"]
        .iter()
        .map(|i| cmd_out("ipconfig", &["getifaddr", i]))
        .find(|a| !a.is_empty())
        .unwrap_or_default()
}

// ---- D. more ways out of a no-network mini-app ----------------------------------------------------------

const RULES: &str = r#"[{"trigger":{"url-filter":".*"},"action":{"type":"block"}},{"trigger":{"url-filter":"^flare-app://app/"},"action":{"type":"ignore-previous-rules"}},{"trigger":{"url-filter":"^blob:flare-app://app/"},"action":{"type":"ignore-previous-rules"}},{"trigger":{"url-filter":"^data:"},"action":{"type":"ignore-previous-rules"}}]"#;

fn exfil_page(port: u16, tok: &str) -> String {
    let u = |p: &str| format!("http://127.0.0.1:{port}/advx-{p}");
    format!(
        r##"<!doctype html><html><head><meta charset="utf-8"><title>Exfil</title>
<link rel="preconnect" href="http://127.0.0.1:{tcp}/">
<link rel="dns-prefetch" href="//advdns-{tok}-a.local">
<link rel="preload" as="image" href="{pre}">
<link rel="modulepreload" href="{modpre}">
<link rel="icon" href="{icon}">
<style>@import url({imp}); @font-face{{font-family:F;src:url({font})}} body{{font-family:F}} .bg{{background:url({bg})}}</style>
</head><body><h1>Exfil</h1>
<a id="pl" href="#x" ping="{ping}">ping</a>
<a href="http://advdns-{tok}-b.local/">anchor</a>
<div class="bg">bg</div>
<video src="{video}" autoplay muted></video><audio src="{audio}" autoplay></audio>
<object data="{object}"></object><embed src="{embed}">
<svg width="10" height="10"><image href="{svgimg}" width="10" height="10"/></svg>
<img srcset="{srcset} 1x"><input type="image" src="{inputimg}">
<iframe srcdoc="<img src='{srcdoc}'>"></iframe>
<form id="f" action="{form}" method="post"><input name="a" value="b"></form>
<script>
window.__r = {{}};
function note(k, v) {{ window.__r[k] = String(v); }}
import('{dyn}').then(function(){{note('import','LEAK')}}, function(e){{note('import','blocked '+e)}});
try {{ var w = new Worker('{worker}'); w.onerror=function(){{note('worker_url','error')}}; }} catch (e) {{ note('worker_url','threw '+e); }}
try {{ document.getElementById('pl').click(); }} catch (e) {{}}
try {{ var img = new Image(); img.src = 'http://advdns-{tok}-c.local/x.png'; }} catch (e) {{}}
try {{ fetch('http://advdns-{tok}-d.local/').then(function(){{note('mdns_fetch','LEAK')}},function(e){{note('mdns_fetch','blocked')}}); }} catch (e) {{}}
setTimeout(function(){{ try {{ document.getElementById('f').submit(); }} catch (e) {{ note('form','threw '+e); }} }}, 1500);
</script>
</body></html>"##,
        tcp = TCP,
        pre = u("preload"),
        modpre = u("modpre"),
        icon = u("icon"),
        imp = u("import-css"),
        font = u("font"),
        bg = u("bg"),
        ping = u("ping"),
        video = u("video"),
        audio = u("audio"),
        object = u("object"),
        embed = u("embed"),
        svgimg = u("svgimg"),
        srcset = u("srcset"),
        inputimg = u("inputimg"),
        srcdoc = u("srcdoc"),
        form = u("form"),
        dyn = u("dyn.js"),
        worker = u("worker.js"),
    )
}

fn app_view(
    env: &Env,
    rules: &[&Retained<WKContentRuleList>],
    csp: bool,
    proxy: Option<u16>,
    html: &str,
    root: &Path,
) -> Retained<WKWebView> {
    let store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(env.mtm) };
    if let Some(p) = proxy {
        mac::set_dead_proxy(&store, p);
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
    for r in rules {
        unsafe { ucc.addContentRuleList(r) };
    }
    unsafe {
        ucc.addUserScript(&mac::user_script(
            env.mtm,
            flare_surfaces::app::NO_WEBRTC_SCRIPT,
            false,
            &pw(env),
        ))
    };
    let page = if csp {
        flare_surfaces::app::wrap_html(html, false)
    } else {
        html.to_string()
    };
    let _ = std::fs::write(root.join("index.html"), page);
    let view = mac::make_view(env.mtm, &cfg, 800.0, 600.0);
    unsafe {
        view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*env.spy)));
        view.setUIDelegate(Some(ProtocolObject::from_ref(&*env.spy)));
    }
    env.win.setContentView(Some(&view));
    mac::st(|s| {
        s.app_view = Retained::as_ptr(&view) as usize;
        s.app_root = root.to_path_buf();
        s.refused.clear();
        s.scheme_hits.clear();
    });
    view
}

fn teardown(env: &Env, view: Retained<WKWebView>) {
    mac::load(&view, "about:blank");
    mac::spin(&env.app, 300);
    unsafe {
        view.setNavigationDelegate(None);
        view.setUIDelegate(None);
    }
    env.win.setContentView(Some(&env.wv));
    drop(view);
    mac::spin(&env.app, 1500);
    mac::st(|s| s.app_view = 0);
}

pub fn exfil(env: &Env, rep: &mut Report) {
    let Some(obs) = Observers::start(&rep.out) else {
        rep.add(
            "D.observers",
            false,
            "the network observers started",
            json!({}),
        );
        return;
    };
    let root = rep.out.join("advapp");
    let _ = std::fs::create_dir_all(&root);
    let rules = mac::compile_rules(&env.app, "adv-app", RULES).ok();
    for (name, use_rules, csp) in [
        ("A-control", false, false),
        ("B-rules", true, false),
        ("D-rules-csp", true, true),
    ] {
        let tok = format!(
            "{}{}",
            name.chars().next().unwrap_or('x').to_ascii_lowercase(),
            std::process::id()
        );
        let html = exfil_page(env.srv.port, &tok);
        let l0 = env.srv.log.all().len();
        let t0 = obs.lines("tcp").len();
        let m0 = obs.lines("mdns").len();
        let r: Vec<&Retained<WKContentRuleList>> = if use_rules {
            rules.iter().collect()
        } else {
            vec![]
        };
        let view = app_view(env, &r, csp, None, &html, &root);
        mac::load(&view, "flare-app://app/index.html");
        mac::spin(&env.app, 5000);
        let page = mac::eval(
            &env.app,
            &view,
            None,
            &pw(env),
            "JSON.stringify(window.__r)",
            3000,
        );
        let hits: Vec<String> = env.srv.log.all()[l0..]
            .iter()
            .filter(|l| l.contains("advx-"))
            .cloned()
            .collect();
        let tcp = obs.lines("tcp")[t0..].to_vec();
        let mdns: Vec<String> = obs.lines("mdns")[m0..]
            .iter()
            .filter(|l| l.contains(&tok))
            .cloned()
            .collect();
        let sealed = hits.is_empty() && tcp.is_empty() && mdns.is_empty();
        let ev = json!({"server_hits": hits, "preconnect_tcp_accepts": tcp, "mdns_queries_for_this_page": mdns,
                        "page": jstr(page), "token": tok});
        if name.starts_with('A') {
            rep.note(
                &format!("D.exfil-{name}"),
                "control (no walls): which channels leave",
                ev,
            );
        } else {
            rep.add(&format!("D.exfil-{name}"), sealed, "no-network mini-app: preconnect, dns-prefetch, .local names, ping, preload, fonts, media, object, svg, srcset, srcdoc, form, import(), worker URL leave nothing", ev);
        }
        teardown(env, view);
    }
    mac::remove_rules(&env.app, "adv-app");
}

// ---- E. networked apps: the proxy, and the rule list that keeps address literals off the direct path --

/// One block rule per address literal a networked app (or a mod or plugin view) must not reach
/// directly: loopback, "this host", private ranges, link-local, CGNAT, every IPv6 literal and
/// `localhost` names. Names go to the proxy, which checks what they resolve to (rebinding). WebKit's
/// content-rule regexes have no alternation, hence one rule per range.
pub fn net_rules() -> String {
    let s = "^[a-z]+://";
    // The rest of a dotted address literal (WebKit's URL parser has already turned 127.1, 2130706433,
    // 0x7f000001 and 0177.0.0.1 into dotted form), so a name such as 127.0.0.1.nip.io is not caught
    // here and goes to the proxy, which checks what it resolves to.
    let d3 = "[0-9]+\\.[0-9]+\\.[0-9]+[:/]";
    let d2 = "[0-9]+\\.[0-9]+[:/]";
    let filters = [
        format!("{s}127\\.{d3}"),
        format!("{s}0\\.{d3}"),
        format!("{s}10\\.{d3}"),
        format!("{s}169\\.254\\.{d2}"),
        format!("{s}192\\.168\\.{d2}"),
        format!("{s}172\\.1[6-9]\\.{d2}"),
        format!("{s}172\\.2[0-9]\\.{d2}"),
        format!("{s}172\\.3[01]\\.{d2}"),
        format!("{s}100\\.6[4-9]\\.{d2}"),
        format!("{s}100\\.[7-9][0-9]\\.{d2}"),
        format!("{s}100\\.1[01][0-9]\\.{d2}"),
        format!("{s}100\\.12[0-7]\\.{d2}"),
        format!("{s}\\["),
        format!("{s}localhost[.:/]"),
        format!("{s}[^/:]*\\.localhost[.:/]"),
    ];
    let rules: Vec<Value> = filters
        .iter()
        .map(|f| json!({"trigger": {"url-filter": f}, "action": {"type": "block"}}))
        .collect();
    Value::Array(rules).to_string()
}

pub fn proxy(env: &Env, rep: &mut Report) {
    let Some(obs) = Observers::start(&rep.out) else {
        rep.add(
            "E.observers",
            false,
            "the network observers started",
            json!({}),
        );
        return;
    };
    let root = rep.out.join("advnet");
    let _ = std::fs::create_dir_all(&root);
    let lan = lan_address();
    let host = cmd_out("scutil", &["--get", "LocalHostName"]);
    let targets: Vec<(&str, String)> = vec![
        ("v4loop", format!("http://127.0.0.1:{HTTP}/px-v4loop")),
        ("v4loop2", format!("http://127.0.0.2:{HTTP}/px-v4loop2")),
        ("v4short", format!("http://127.1:{HTTP}/px-v4short")),
        (
            "v4decimal",
            format!("http://2130706433:{HTTP}/px-v4decimal"),
        ),
        ("v4hex", format!("http://0x7f000001:{HTTP}/px-v4hex")),
        ("v4octal", format!("http://0177.0.0.1:{HTTP}/px-v4octal")),
        ("zero", format!("http://0.0.0.0:{HTTP}/px-zero")),
        ("localhost", format!("http://localhost:{HTTP}/px-localhost")),
        (
            "localhost_dot",
            format!("http://localhost.:{HTTP}/px-localhostdot"),
        ),
        (
            "sub_localhost",
            format!("http://a.localhost:{HTTP}/px-sublocalhost"),
        ),
        ("v6loop", format!("http://[::1]:{HTTP}/px-v6loop")),
        (
            "v6mapped",
            format!("http://[::ffff:127.0.0.1]:{HTTP}/px-v6mapped"),
        ),
        ("lan", format!("http://{lan}:{HTTP}/px-lan")),
        (
            "mdnsname",
            format!("http://{host}.local:{HTTP}/px-mdnsname"),
        ),
        (
            "rebind_localtest",
            format!("http://localtest.me:{HTTP}/px-rebind1"),
        ),
        (
            "rebind_nip",
            format!("http://127.0.0.1.nip.io:{HTTP}/px-rebind2"),
        ),
        ("public", "http://example.com/px-public".to_string()),
        (
            "public_https",
            "https://example.com/px-publichttps".to_string(),
        ),
    ];
    let ws: Vec<(&str, String)> = vec![
        ("ws_v4loop", format!("ws://127.0.0.1:{HTTP}/px-ws")),
        ("ws_v6loop", format!("ws://[::1]:{HTTP}/px-ws6")),
        ("ws_lan", format!("ws://{lan}:{HTTP}/px-wslan")),
        (
            "ws_localhost",
            format!("ws://localhost:{HTTP}/px-wslocalhost"),
        ),
        ("ws_rebind", format!("ws://localtest.me:{HTTP}/px-wsrebind")),
    ];
    let mut js = String::from("window.__r={};function note(k,v){window.__r[k]=String(v);}\n");
    for (k, u) in &targets {
        js.push_str(&format!(
            "fetch('{u}',{{mode:'no-cors'}}).then(function(){{note('{k}','answered')}},function(e){{note('{k}','failed '+e)}});\n"
        ));
    }
    for (k, u) in &ws {
        js.push_str(&format!(
            "try{{var w_{k}=new WebSocket('{u}');w_{k}.onerror=function(){{note('{k}','error')}};w_{k}.onopen=function(){{note('{k}','open')}};}}catch(e){{note('{k}','threw '+e)}}\n"
        ));
    }
    let html = format!("<!doctype html><title>Net app</title><h1>net</h1><script>{js}</script>");
    let rules = mac::compile_rules(&env.app, "adv-net", &net_rules());
    let compiled = rules.is_ok();
    for (name, with_rules) in [("proxy-only", false), ("proxy-and-rules", true)] {
        let h0 = obs.lines("http").len();
        let p0 = obs.lines("proxy").len();
        let r: Vec<&Retained<WKContentRuleList>> = match (&rules, with_rules) {
            (Ok(r), true) => vec![r],
            _ => vec![],
        };
        let view = app_view(env, &r, false, Some(PROXY), &html, &root);
        mac::load(&view, "flare-app://app/index.html");
        mac::spin(&env.app, 8000);
        let page = mac::eval(
            &env.app,
            &view,
            None,
            &pw(env),
            "JSON.stringify(window.__r)",
            3000,
        );
        let direct = obs.lines("http")[h0..].to_vec();
        let via_proxy = obs.lines("proxy")[p0..].to_vec();
        let ev = json!({"rules_compiled": compiled, "rules_error": rules.as_ref().err(),
                        "lan": lan, "host": host, "direct_to_local_server": direct,
                        "seen_by_proxy": via_proxy, "page": jstr(page)});
        if with_rules {
            let proxied = |s: &str| via_proxy.iter().any(|l| l.contains(s));
            rep.add(
                &format!("E.{name}"),
                compiled
                    && direct.is_empty()
                    && proxied("example.com")
                    && proxied("localtest.me")
                    && proxied("nip.io"),
                "networked app with the proxy plus the address-literal rule list: nothing reaches a local server directly (v4, v6, private range, odd v4 spellings, ws), names still go to the proxy",
                ev,
            );
        } else {
            rep.note(
                &format!("E.{name}"),
                "networked app with the proxy alone: which requests skip it",
                ev,
            );
        }
        teardown(env, view);
    }
    mac::remove_rules(&env.app, "adv-net");
}

// ---- F. sandboxed iframes without allow-scripts -------------------------------------------------------

pub fn sbframe(env: &Env, rep: &mut Report) {
    mac::st(|s| s.sub_frames.clear());
    let ok = mac::load_wait(&env.app, &env.wv, &format!("{}/adv/sbframe", env.base()));
    mac::spin(&env.app, 1000);
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs, _) = look(&mut shim);
    let listed =
        ref_named(&v, "Inner button").is_some() && ref_named(&v, "Srcdoc button").is_some();
    // A ref click on the opaque frame's link (no script runs there, so a navigation is the evidence).
    let n0 = mac::st(|s| s.sub_frames.len());
    let c_src = click(
        &mut shim,
        &refs,
        Target::Ref(ref_named(&v, "Srcdoc link").unwrap_or_default()),
    );
    let src_nav = mac::wait_until(&env.app, 3000, || {
        mac::st(|s| {
            s.sub_frames[n0..]
                .iter()
                .any(|(u, _)| u.ends_with("from=srcdoc"))
        })
    });
    rep.add(
        "F.sandboxed-iframe-listed",
        listed && src_nav,
        "controls inside <iframe sandbox> (no allow-scripts; same-origin and opaque srcdoc) are in the list, and a ref click on the opaque frame's link follows it",
        json!({"loaded": ok, "elements": names(&v), "frames": shim.frame_labels(), "refs": refs.len(),
               "srcdoc_link_click": c_src, "srcdoc_frame_navigated": src_nav,
               "shim_notes": shim.notes.clone()}),
    );
    // Closure 1: a same-origin sandboxed frame is reachable from its parent's agent world.
    let probe = "(function(){var f=document.querySelectorAll('iframe')[0];var d=null;try{d=f.contentDocument}catch(e){return JSON.stringify({err:String(e)})}if(!d)return JSON.stringify({err:'no contentDocument'});var a=d.querySelector('a');var r=a.getBoundingClientRect();var fr=f.getBoundingClientRect();var cs=getComputedStyle(f);return JSON.stringify({controls:d.querySelectorAll('a,button').length,name:a.textContent,x:fr.left+f.clientLeft+parseFloat(cs.paddingLeft)+r.left+r.width/2,y:fr.top+f.clientTop+parseFloat(cs.paddingTop)+r.top+r.height/2});})()";
    let reach = mac::eval(
        &env.app,
        &env.wv,
        None,
        &mac::agent_world(env.mtm),
        probe,
        3000,
    );
    let reach_v: Value = match &reach {
        Ok(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let n0 = mac::st(|s| s.sub_frames.len());
    let c = match (reach_v["x"].as_f64(), reach_v["y"].as_f64()) {
        (Some(x), Some(y)) => click(&mut shim, &refs, Target::Point(x, y)),
        _ => "no point".into(),
    };
    let navigated = mac::wait_until(&env.app, 3000, || {
        mac::st(|s| {
            s.sub_frames[n0..]
                .iter()
                .any(|(u, _)| u.ends_with("/page2"))
        })
    });
    rep.add(
        "F.sandboxed-same-origin-reachable",
        reach_v["controls"].as_i64() == Some(2) && navigated,
        "closure for a same-origin sandboxed frame: the parent's agent world reads its document and boxes, a trusted click at the point follows its link",
        json!({"reach": reach_v, "click": c, "frame_navigated": navigated}),
    );
    // Closure 2 (attempt): the opaque-origin srcdoc frame through the WKFrameInfo the navigation
    // delegate gave for it.
    let infos = mac::st(|s| s.sub_frames.clone());
    let srcdoc = infos
        .iter()
        .find(|(u, _)| u == "about:srcdoc")
        .map(|(_, i)| i.clone());
    let (agent_r, page_r) = match &srcdoc {
        Some(i) => (
            mac::eval(
                &env.app,
                &env.wv,
                Some(i),
                &mac::agent_world(env.mtm),
                "document.body.innerHTML",
                3000,
            ),
            mac::eval(
                &env.app,
                &env.wv,
                Some(i),
                &pw(env),
                "document.body.innerHTML",
                3000,
            ),
        ),
        None => (Err("no WKFrameInfo".into()), Err("no WKFrameInfo".into())),
    };
    let parent_r = mac::eval(
        &env.app,
        &env.wv,
        None,
        &mac::agent_world(env.mtm),
        "(function(){try{var d=document.querySelectorAll('iframe')[1].contentDocument;return d?d.body.innerHTML:'null'}catch(e){return 'threw '+e}})()",
        3000,
    );
    rep.add(
        "F.sandboxed-opaque-reachable",
        agent_r.is_ok() || page_r.is_ok(),
        "closure attempt for an opaque-origin sandboxed srcdoc frame: evaluateJavaScript in that frame (agent world or page world) with the delegate's WKFrameInfo",
        json!({"subframe_navigations": infos.iter().map(|(u, _)| u.clone()).collect::<Vec<_>>(),
               "agent_world": jstr(agent_r), "page_world": jstr(page_r), "parent_contentDocument": jstr(parent_r)}),
    );
}

// ---- G. permissions with no public delegate: clipboard read, location, notifications ------------------

/// Page world, every frame, document start: the APIs WebView2's PermissionRequested denies and WKWebView
/// has no public delegate for answer "denied" without asking the system (no Paste callout, no location
/// prompt). Media capture keeps its public delegate.
pub const DENY_JS: &str = "(function(){function lock(o,k,v){try{Object.defineProperty(o,k,{value:v,writable:false,configurable:false})}catch(e){}}function no(){return Promise.reject(new DOMException('denied','NotAllowedError'))}if(window.Clipboard){lock(Clipboard.prototype,'read',no);lock(Clipboard.prototype,'readText',no)}if(window.Geolocation){var d=function(ok,err){if(typeof err==='function')setTimeout(function(){err({code:1,PERMISSION_DENIED:1,message:'denied'})},0);return 0};lock(Geolocation.prototype,'getCurrentPosition',d);lock(Geolocation.prototype,'watchPosition',d)}if(window.Notification){lock(Notification,'requestPermission',function(cb){if(typeof cb==='function')setTimeout(function(){cb('denied')},0);return Promise.resolve('denied')});try{Object.defineProperty(Notification,'permission',{get:function(){return 'denied'},configurable:false})}catch(e){}}})();";

/// Asks each API only when the wall is in place (a native clipboard read can open WebKit's Paste menu, a
/// native location request a system prompt: neither may reach the screen). Native descriptors are
/// writable and configurable; the wall's are neither.
const PERM_PROBE: &str = "return await (async function(){var out={};function locked(o,k){var d=o&&Object.getOwnPropertyDescriptor(o,k);return !d?'absent':(!d.writable&&!d.configurable&&!d.get)?'walled':'NATIVE'}async function tryit(w,tag){var c=locked(w.Clipboard&&w.Clipboard.prototype,'readText');var g=locked(w.Geolocation&&w.Geolocation.prototype,'getCurrentPosition');var n=locked(w.Notification,'requestPermission');out[tag+'walls']=c+'/'+g+'/'+n;if(c==='walled'){try{await w.navigator.clipboard.readText();out[tag+'clip']='READ'}catch(e){out[tag+'clip']=e.name}}if(g==='walled'){out[tag+'geo']=await new Promise(function(r){w.navigator.geolocation.getCurrentPosition(function(){r('GOT')},function(e){r('error '+e.code)});setTimeout(function(){r('no answer')},2000)})}if(n==='walled'){out[tag+'notif']=String(await w.Notification.requestPermission())}}await tryit(window,'main_');var f=document.createElement('iframe');document.body.appendChild(f);await tryit(f.contentWindow,'fresh_frame_');var g=document.createElement('iframe');g.srcdoc='x';document.body.appendChild(g);await new Promise(function(r){g.onload=r});await tryit(g.contentWindow,'srcdoc_frame_');return JSON.stringify(out)})()";

pub fn perms(env: &Env, rep: &mut Report) {
    // Without the wall, only what exists is read: calling the native APIs can put WebKit's Paste menu or
    // a system prompt on screen (a first attempt here with a clipboard read and no gesture blocked the run
    // loop, the sign of a modal menu, and was killed after 100 s).
    let ok = mac::load_wait(&env.app, &env.wv, &format!("{}/adv/perms", env.base()));
    let raw = mac::eval(
        &env.app,
        &env.wv,
        None,
        &pw(env),
        "JSON.stringify({clipboard: typeof navigator.clipboard, geolocation: typeof navigator.geolocation, Notification: typeof Notification, mediaDevices: typeof navigator.mediaDevices})",
        4000,
    );
    rep.note(
        "G.permissions-raw",
        "without a wall: which permission APIs exist (not called: see above)",
        json!({"loaded": ok, "page": jstr(raw)}),
    );
    let wv = own_view(env, Some(DENY_JS));
    let ok2 = mac::load_wait(&env.app, &wv, &format!("{}/adv/perms", env.base()));
    // A trusted click first, so the clipboard read has the user activation that would open the callout.
    let mut shim = Shim::new(&env.app, &wv);
    let (v, refs, _) = look(&mut shim);
    let c = click(
        &mut shim,
        &refs,
        Target::Ref(ref_named(&v, "Go").unwrap_or_default()),
    );
    let r = mac::call_async(&env.app, &wv, None, &pw(env), PERM_PROBE, 12_000);
    let parsed: Value = match &r {
        Ok(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let all_denied = ["main_", "fresh_frame_", "srcdoc_frame_"].iter().all(|t| {
        let walls = parsed[format!("{t}walls")].as_str().unwrap_or("NATIVE");
        !walls.contains("NATIVE")
            && parsed[format!("{t}clip")] == "NotAllowedError"
            && parsed[format!("{t}geo")] == "error 1"
    });
    rep.add(
        "G.permissions-denied",
        all_denied && mac::onscreen_windows_of_self() == 0,
        "with the page-world deny script in every frame: clipboard read (after a trusted click), location and notifications answer denied in the main frame, a fresh about:blank frame and a srcdoc frame; the page cannot restore them; nothing appeared",
        json!({"loaded": ok2, "click": c, "page": parsed, "raw": jstr(r),
               "onscreen_windows": mac::onscreen_windows_of_self()}),
    );
    drop_view(env, wv, false);
}

// ---- H. window.close() and beforeunload -----------------------------------------------------------------

pub fn close(env: &Env, rep: &mut Report) {
    mac::st(|s| {
        s.closed = 0;
        s.dialogs.clear();
        s.messages.clear();
    });
    let ok = mac::load_wait(&env.app, &env.wv, &format!("{}/adv/close", env.base()));
    mac::spin(&env.app, 300);
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v, refs, _) = look(&mut shim);
    let c = click(
        &mut shim,
        &refs,
        Target::Ref(ref_named(&v, "Close me").unwrap_or_default()),
    );
    mac::spin(&env.app, 400);
    let closed = mac::st(|s| s.closed);
    let called = mac::eval(
        &env.app,
        &env.wv,
        None,
        &pw(env),
        "String(window.__closeCalled)",
        2000,
    );
    let (v2, _, _) = look(&mut shim);
    rep.add(
        "H.window-close",
        called == Ok(json!("1")) && v2.get("error").is_none() && mac::url_of(&env.wv).ends_with("/adv/close"),
        "window.close() from the page: webViewDidClose tells the host (WindowCloseRequested on Windows), the host keeps the view, the page still answers",
        json!({"load": ok, "click": c, "webViewDidClose_calls": closed, "page_called_close": jstr(called),
               "look_after": v2.get("error").cloned().unwrap_or(json!(names(&v2)))}),
    );
    // beforeunload with sticky user activation (the click above): a link the agent clicks, then the
    // host's own navigation.
    let (v3, refs3, _) = look(&mut shim);
    let before = mac::st(|s| s.finished);
    let c2 = click(
        &mut shim,
        &refs3,
        Target::Ref(ref_named(&v3, "Leave").unwrap_or_default()),
    );
    let left = mac::wait_until(&env.app, 5000, || mac::st(|s| s.finished) > before)
        && mac::url_of(&env.wv).ends_with("/page2");
    let _ = mac::load_wait(&env.app, &env.wv, &format!("{}/adv/close", env.base()));
    let mut shim = Shim::new(&env.app, &env.wv);
    let (v4, refs4, _) = look(&mut shim);
    let _ = click(
        &mut shim,
        &refs4,
        Target::Ref(ref_named(&v4, "Close me").unwrap_or_default()),
    );
    let host_nav = mac::load_wait(&env.app, &env.wv, &format!("{}/page2", env.base()))
        && mac::url_of(&env.wv).ends_with("/page2");
    let (msgs, dialogs) = mac::st(|s| (s.messages.clone(), s.dialogs.clone()));
    let ran = msgs
        .iter()
        .filter(|m| m.contains("beforeunload ran"))
        .count();
    rep.add(
        "H.beforeunload",
        left && host_nav && dialogs.is_empty() && mac::onscreen_windows_of_self() == 0,
        "a page that asks to stay (beforeunload, after a user gesture) neither holds a navigation nor shows a dialog: the seal's dismissed leave-page question, by WebKit's default (no public beforeunload delegate)",
        json!({"link_click": c2, "left_by_link": left, "left_by_host_navigation": host_nav,
               "beforeunload_handler_ran": ran, "dialogs": dialogs, "onscreen_windows": mac::onscreen_windows_of_self()}),
    );
}
