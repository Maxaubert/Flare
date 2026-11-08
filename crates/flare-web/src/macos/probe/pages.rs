//! The web host tests' pages and their loopback server (one thread per connection, `GET` only, every path logged),
//! plus the exfiltration page of the phase 1 spike (`wk_spike/adv.rs`), so the CEF walls answer the questions the
//! WKWebView ones did.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The TCP accept logger, the CONNECT proxy and the dual-stack server of `tests/cef/observers.py`.
pub const OBS_TCP: u16 = 47201;
pub const OBS_PROXY: u16 = 47202;
pub const OBS_HTTP: u16 = 47203;

/// The panel's page: everything the person and the agent use, each control at a known place (CSS pixels), every
/// trusted input event logged into `window.__log`.
pub const MAIN: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Main</title>
<style>html,body{margin:0;font:15px -apple-system,sans-serif} #hit{position:absolute;left:20px;top:20px;width:200px;height:60px;background:#3366cc}
#t1{position:absolute;left:20px;top:120px;width:300px;height:30px;font-size:16px} #sel{position:absolute;left:20px;top:180px;width:200px}
#tall{position:absolute;top:300px;height:4000px;width:10px}</style>
<script>
window.__log = []; window.__clicks = 0; window.__keys = 0;
['click','mousedown','mouseup','keydown','input','wheel'].forEach(function (t) {
  document.addEventListener(t, function (e) { window.__log.push(t + ':' + e.isTrusted); if (t === 'keydown') window.__keys++; }, true);
});
</script></head><body>
<div id="hit" onclick="window.__clicks++"></div>
<input id="t1" aria-label="Name">
<select id="sel"><option>one</option><option>two</option><option>three</option></select>
<a id="p2" href="/page2">page two</a>
<a id="blank" target="_blank" href="/page2" style="position:absolute;left:20px;top:230px;width:200px;height:30px;display:block;background:#eee">new window</a>
<div id="tall"></div>
</body></html>"#;

pub const PAGE2: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Page two</title></head><body><h1>Page two</h1></body></html>"#;

/// A known colour in a known place.
pub const PAINT: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Paint</title>
<style>html,body{margin:0;background:#ffffff} #sq{position:absolute;left:0;top:0;width:200px;height:100px;background:#3366cc}</style>
</head><body><div id="sq"></div></body></html>"#;

/// The exfiltration page of the spike: every way a page has to reach out (preconnect, dns-prefetch, `.local` names,
/// ping, preload, fonts, media, object, svg, srcset, srcdoc, a form, `import()`, a worker URL, fetch, a beacon, a
/// websocket), aimed at this server's `advx-` paths, the TCP logger and mDNS.
pub fn exfil(port: u16, tok: &str) -> String {
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
import('{dyn}').then(function(){{note('import','LEAK')}}, function(e){{note('import','blocked')}});
try {{ var w = new Worker('{worker}'); w.onerror=function(){{note('worker_url','error')}}; }} catch (e) {{ note('worker_url','threw'); }}
try {{ document.getElementById('pl').click(); }} catch (e) {{}}
try {{ var img = new Image(); img.src = 'http://advdns-{tok}-c.local/x.png'; }} catch (e) {{}}
fetch('{fetch}').then(function(){{note('fetch','LEAK')}},function(){{note('fetch','blocked')}});
fetch('http://advdns-{tok}-d.local/').then(function(){{note('mdns_fetch','LEAK')}},function(){{note('mdns_fetch','blocked')}});
try {{ note('beacon', navigator.sendBeacon('{beacon}', 'x')); }} catch (e) {{ note('beacon','threw'); }}
try {{ var ws = new WebSocket('ws://127.0.0.1:{tcp}/'); ws.onerror=function(){{note('ws','error')}}; }} catch (e) {{ note('ws','threw'); }}
note('rtc', typeof RTCPeerConnection);
setTimeout(function(){{ try {{ document.getElementById('f').submit(); }} catch (e) {{ note('form','threw'); }} }}, 1500);
</script>
</body></html>"##,
        tcp = OBS_TCP,
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
        fetch = u("fetch"),
        beacon = u("beacon"),
    )
}

/// The agent's page (the spike's `main`, cut to what the tools use): a button, a field, a checkbox, a text area, an
/// editable block, a download link, a same-origin frame, a cross-origin one (localhost is another site than
/// 127.0.0.1, so Chromium puts it in another process), a sandboxed frame with no scripts and one with
/// `allow-same-origin`, a tall area. Every key and input event is logged into `window.__log`.
pub fn agent(port: u16) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Agent</title>
<style>body{{font:15px -apple-system,sans-serif;margin:8px}} iframe{{width:380px;height:90px;border:2px solid #888;display:block;margin:4px 0}}
#tall{{height:2600px;background:linear-gradient(#fff,#bcd)}}</style>
<script>window.__log=[];['keydown','keyup','input','beforeinput','click'].forEach(function(t){{document.addEventListener(t,function(e){{window.__log.push({{t:e.type,tr:e.isTrusted,k:e.key||'',c:e.code||'',m:(e.metaKey?'M':'')+(e.ctrlKey?'C':'')+(e.shiftKey?'S':'')+(e.altKey?'A':''),id:(e.target&&e.target.id)||''}});}},true);}});</script>
</head><body>
<h1>Agent contract</h1>
<button id="b1" onclick="document.getElementById('out').textContent='clicked b1 '+event.isTrusted">Press me</button>
<label for="t1">Name</label> <input id="t1">
<input type="checkbox" id="c1" aria-label="Agree">
<textarea id="ta" aria-label="Notes" rows="2" cols="30"></textarea>
<a id="dl" href="/report.txt">Download report</a>
<div id="out" role="status">nothing yet</div>
<iframe id="fs" src="/same" title="Same origin frame"></iframe>
<iframe id="fx" src="http://localhost:{port}/cross" title="Cross origin frame"></iframe>
<iframe id="fb" sandbox srcdoc="<body style='margin:4px;background:#efe'><a id=sl href='http://127.0.0.1:{port}/page2'>Sandboxed link</a></body>" title="Sandboxed frame"></iframe>
<iframe id="fo" sandbox="allow-same-origin" src="/same?sb=1" title="Sandboxed same origin frame"></iframe>
<div id="tall">tall area to scroll</div>
<p id="bottom">The bottom line</p>
</body></html>"#
    )
}

/// The frames of the agent's page: a button and a field. A click on the button and what is typed into the field
/// are told to the server (`/fclick-<id>`, `/ftype-<id>?v=`), so a cross-origin frame, which the page cannot read,
/// says what it got; a frame with no scripts says nothing and is read by its parent.
pub fn frame(title: &str, id: &str, label: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{title}</title></head>
<body style="margin:4px;font:14px sans-serif"><button id="{id}b" onclick="this.textContent='{label} clicked';fetch('/fclick-{id}-'+event.isTrusted)">{label} button</button>
<input id="{id}i" aria-label="{label} input" oninput="fetch('/ftype-{id}?v='+encodeURIComponent(this.value))"></body></html>"#
    )
}

/// Text that appears late, for `browser_wait`.
pub const SLOW: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Slow page</title></head><body><p>slow content arrived</p><script>setTimeout(function(){var d=document.createElement('div');d.id='late';d.textContent='late text appears';document.body.appendChild(d);},800);</script></body></html>"#;

/// The proxy page of the spike (`adv.rs`, `E.proxy-*`): every spelling of this machine and its network a page can
/// aim at (loopback in five IPv4 spellings, `0.0.0.0`, `localhost` and its forms, IPv6 loopback and the mapped
/// address, the LAN address, this Mac's `.local` name and one the mDNS sniffer
/// watches for (named by `tok`, so a view's query is told from an earlier one's retries), names that resolve to loopback) over http, https, ws and wss,
/// at the observers' dual-stack server. Nothing of it may reach that server from a view behind Flare's proxy.
pub fn literals(lan: &str, local_name: &str, tok: &str) -> String {
    let p = OBS_HTTP;
    let mut hosts: Vec<(&str, String)> = vec![
        ("v4loop", "127.0.0.1".into()),
        ("v4loop2", "127.0.0.2".into()),
        ("v4short", "127.1".into()),
        ("v4decimal", "2130706433".into()),
        ("v4hex", "0x7f000001".into()),
        ("v4octal", "0177.0.0.1".into()),
        ("zero", "0.0.0.0".into()),
        ("localhost", "localhost".into()),
        ("localhostdot", "localhost.".into()),
        ("sublocalhost", "a.localhost".into()),
        ("v6loop", "[::1]".into()),
        ("v6mapped", "[::ffff:127.0.0.1]".into()),
        ("mdnsname", format!("{local_name}.local")),
        ("advmdns", format!("advdns-{tok}.local")),
        ("rebind1", "localtest.me".into()),
        ("rebind2", "127.0.0.1.nip.io".into()),
    ];
    if !lan.is_empty() {
        hosts.push(("lan", lan.to_string()));
    }
    let mut js = String::from("window.__r={};function note(k,v){window.__r[k]=String(v);}\n");
    for (k, h) in &hosts {
        for scheme in ["http", "https"] {
            js.push_str(&format!(
                "fetch('{scheme}://{h}:{p}/px-{k}-{scheme}',{{mode:'no-cors'}}).then(function(){{note('{k}_{scheme}','answered')}},function(e){{note('{k}_{scheme}','failed')}});\n"
            ));
        }
        for scheme in ["ws", "wss"] {
            js.push_str(&format!(
                "try{{var w=new WebSocket('{scheme}://{h}:{p}/px-{k}-{scheme}');w.onerror=function(){{note('{k}_{scheme}','error')}};w.onopen=function(){{note('{k}_{scheme}','open')}};}}catch(e){{note('{k}_{scheme}','threw')}}\n"
            ));
        }
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Literals</title></head><body><h1>literals</h1><script>{js}</script></body></html>"
    )
}

/// This Mac's LAN address and its Bonjour name (empty when it has none).
pub fn lan_and_name() -> (String, String) {
    let out = |c: &str, a: &[&str]| {
        std::process::Command::new(c)
            .args(a)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let lan = ["en0", "en1", "en2"]
        .iter()
        .map(|i| out("/usr/sbin/ipconfig", &["getifaddr", i]))
        .find(|a| !a.is_empty())
        .unwrap_or_default();
    (lan, out("/usr/sbin/scutil", &["--get", "LocalHostName"]))
}

/// A mod's page: it posts through its bridge as soon as it runs and keeps what the host posts back.
pub const MOD_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Mod page</title></head><body>
<p>mod</p><script>
window.__got = [];
window.chrome.webview.addEventListener('message', function (e) { window.__got.push(e.data); });
window.chrome.webview.postMessage(JSON.stringify({t: 'call', id: 1, method: 'view.text', params: {}}));
</script></body></html>"#;

pub struct Server {
    pub port: u16,
    pub log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn requests(&self) -> Vec<String> {
        self.log.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn hits(&self, needle: &str) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|p| p.contains(needle))
            .collect()
    }
}

/// The server's own port, for the control page it serves (`/exfil`).
static PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// A colour input where a click lands (its native panel is what must stay off the screen) and a print button.
pub const PICKERS: &str = r##"<!doctype html><html><head><meta charset="utf-8"><title>Pickers</title></head>
<body style="margin:0"><input type="color" id="c" value="#3366cc"
 style="position:absolute;left:20px;top:20px;width:200px;height:80px">
<script>window.__printed = 0; window.__picked = 0;
window.addEventListener('afterprint', function () { window.__printed++; });
document.getElementById('c').addEventListener('click', function (e) { if (e.isTrusted) { window.__picked++; } });
</script>
</body></html>"##;

/// The second mini-app's page: a shared worker and a dedicated worker, both running [`cross_app_worker`].
pub const CROSS_APP_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>App B</title></head><body>B
<script>
window.__w = {};
try {
  var sw = new SharedWorker('/worker.js');
  sw.port.onmessage = function (e) { window.__w.shared = e.data; };
  sw.port.start();
  sw.port.postMessage('go');
} catch (e) { window.__w.shared = {own: 'threw ' + e}; }
try {
  var dw = new Worker('/worker.js');
  dw.onmessage = function (e) { window.__w.dedicated = e.data; };
  dw.postMessage('go');
} catch (e) { window.__w.dedicated = {own: 'threw ' + e}; }
</script></body></html>"#;

/// A worker that asks for its own app's file and for the other app's (`other_host`) file and script.
pub fn cross_app_worker(other_host: &str) -> String {
    format!(
        r#"async function t(u, o) {{
  try {{ const r = await fetch(u, o); let b = ''; try {{ b = (await r.text()).slice(0, 40); }} catch (e) {{}}
         return r.type + ' ' + r.status + (b ? ' ' + b : ''); }} catch (e) {{ return 'ERR ' + e; }}
}}
async function run() {{
  const out = {{}};
  out.own = await t('/own.txt');
  out.other = await t('https://{other_host}/secret.txt', {{mode: 'no-cors'}});
  try {{ importScripts('https://{other_host}/leak.js'); out.import = 'imported ' + self.__leak; }}
  catch (e) {{ out.import = 'ERR ' + e; }}
  return out;
}}
self.onconnect = function (e) {{ const port = e.ports[0]; run().then(function (o) {{ port.postMessage(o); }}); }};
self.onmessage = function () {{ run().then(function (o) {{ self.postMessage(o); }}); }};
"#
    )
}

pub fn start() -> Result<Server, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    PORT.store(port, std::sync::atomic::Ordering::SeqCst);
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = Arc::clone(&log);
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let l3 = Arc::clone(&l2);
            std::thread::spawn(move || handle(s, &l3));
        }
    });
    Ok(Server { port, log })
}

fn route(path: &str) -> (&'static str, Vec<(&'static str, String)>, String) {
    if path.split('?').next() == Some("/exfil") {
        let port = PORT.load(std::sync::atomic::Ordering::SeqCst);
        return (
            "200 OK",
            vec![("Content-Type", "text/html; charset=utf-8".to_string())],
            exfil(port, "ctl"),
        );
    }
    let p = path.split('?').next().unwrap_or(path);
    let html = |b: &str| {
        (
            "200 OK",
            vec![("Content-Type", "text/html; charset=utf-8".to_string())],
            b.to_string(),
        )
    };
    match p {
        "/" | "/main" => html(MAIN),
        "/page2" => html(PAGE2),
        "/paint" => html(PAINT),
        "/agent" => {
            let port = PORT.load(std::sync::atomic::Ordering::SeqCst);
            html(&agent(port))
        }
        "/same" => html(&frame("same frame", "s", "Frame")),
        "/cross" => html(&frame("cross frame", "x", "Cross")),
        "/slow" => html(SLOW),
        "/pickers" => html(PICKERS),
        "/literals" => {
            let (lan, name) = lan_and_name();
            html(&literals(&lan, &name, "lit-ctl"))
        }
        "/report.txt" => (
            "200 OK",
            vec![
                ("Content-Type", "text/plain".to_string()),
                (
                    "Content-Disposition",
                    "attachment; filename=\"report.txt\"".to_string(),
                ),
            ],
            "the report's text".to_string(),
        ),
        _ => (
            "404 Not Found",
            vec![("Content-Type", "text/html".to_string())],
            "<!doctype html><title>Missing</title>not here".to_string(),
        ),
    }
}

fn handle(mut s: TcpStream, log: &Arc<Mutex<Vec<String>>>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(clone) = s.try_clone() else { return };
    let mut r = BufReader::new(clone);
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    if let Ok(mut v) = log.lock() {
        v.push(path.clone());
    }
    if path == "/held.bin" {
        // A download that takes its time: its first part, then nothing for 8 s (it is held, and answered, meanwhile).
        let head = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                    Content-Disposition: attachment; filename=\"held.bin\"\r\nContent-Length: 4194304\r\n\
                    Cache-Control: no-store\r\nConnection: close\r\n\r\n";
        let _ = s.write_all(head.as_bytes());
        let _ = s.write_all(&[b'Z'; 65536]);
        let _ = s.flush();
        std::thread::sleep(Duration::from_secs(8));
        return;
    }
    let (status, headers, body) = route(&path);
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body.as_bytes());
}
