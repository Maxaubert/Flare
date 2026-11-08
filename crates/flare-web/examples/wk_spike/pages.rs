//! The spike's test pages. Each page logs, in its own page world, every input event with its
//! `isTrusted` flag into `window.__log`, so a test reads back what the page really saw.

/// The page-world event logger every test page carries.
pub const LOGGER: &str = r#"<script>
window.__log = [];
(function () {
  function L(e) {
    var t = e.target || {};
    window.__log.push({t: e.type, tr: e.isTrusted, id: (t.id || (t.nodeType === 9 ? 'document' : '')),
      k: e.key || '', c: e.code || '', ctrl: !!e.ctrlKey, meta: !!e.metaKey, shift: !!e.shiftKey,
      alt: !!e.altKey, btn: (e.button === undefined ? -1 : e.button), d: e.data || '',
      it: e.inputType || '', x: e.clientX, y: e.clientY, dy: e.deltaY, det: e.detail});
  }
  ['click','dblclick','mousedown','mouseup','auxclick','contextmenu','keydown','keyup','keypress',
   'beforeinput','input','wheel','scroll','focus','change'].forEach(function (t) {
    document.addEventListener(t, L, true);
  });
})();
</script>"#;

pub fn main(port: u16) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Spike main</title>
<style>body{{font:15px -apple-system,sans-serif;margin:8px}} iframe{{width:380px;height:120px;border:2px solid #888;display:inline-block}}
#tall{{height:2600px;background:linear-gradient(#fff,#bcd)}} #ce{{border:1px solid #999;min-height:22px;width:300px}}</style>
{LOGGER}</head><body>
<h1>Agent contract</h1>
<p>Some intro text for the reader.</p>
<button id="b1" onclick="document.getElementById('out').textContent='clicked b1 '+event.isTrusted">Press me</button>
<a id="l1" href="/page2">Go to page two</a>
<label for="t1">Name</label> <input id="t1" placeholder="your name">
<input type="checkbox" id="c1" aria-label="Agree">
<select id="s1" aria-label="Fruit"><option>Apple</option><option selected>Pear</option></select><br>
<textarea id="ta" aria-label="Notes" rows="2"></textarea>
<div id="ce" contenteditable="true" aria-label="Editor"></div>
<button id="aux" aria-label="Middle target">Middle</button>
<button id="dis" disabled>Disabled one</button>
<div id="out" role="status">nothing yet</div>
<div id="host"></div>
<iframe id="fs" src="/same" title="Same origin frame"></iframe>
<iframe id="fx" src="http://localhost:{port}/cross" title="Cross origin frame"></iframe>
<p><a id="dl" href="/dl.bin">Download file</a> | <a id="mail" href="mailto:someone@example.com">Mail someone</a> |
<a id="blank" href="/page2" target="_blank">Open in new window</a> |
<button id="wo" onclick="window.open('/page2?wo=1')">window.open</button></p>
<p><button id="dlg" onclick="alert('hello alert'); window.__confirm = confirm('sure?'); window.__prompt = prompt('your name?', 'def');">Dialogs</button>
<input type="file" id="file" aria-label="Upload" onchange="window.__file = this.files.length ? this.files[0].name : ''">
<button id="cam" onclick="navigator.mediaDevices.getUserMedia({{video:true}}).then(function(){{window.__cam='granted'}},function(e){{window.__cam='denied: '+e.name}})">Camera</button></p>
<div id="tall">tall area to scroll</div>
<p id="bottom">The bottom line</p>
<script>
var sh = document.getElementById('host').attachShadow({{mode: 'closed'}});
sh.innerHTML = '<button id="shbtn">Shadow button</button>';
console.log('main page console line');
</script>
</body></html>"#
    )
}

pub const SAME: &str = concat!(
    r#"<!doctype html><html><head><meta charset="utf-8"><title>same frame</title>"#,
    r#"<script>window.__log=[];['click','mousedown','keydown','input','beforeinput'].forEach(function(t){document.addEventListener(t,function(e){window.__log.push({t:e.type,tr:e.isTrusted,id:(e.target&&e.target.id)||'',d:e.data||''});},true);});</script>"#,
    r#"</head><body style="margin:4px;font:14px sans-serif"><button id="fb" onclick="this.textContent='Frame clicked'">Frame button</button> <input id="fi" aria-label="Frame input"></body></html>"#
);

pub const CROSS: &str = concat!(
    r#"<!doctype html><html><head><meta charset="utf-8"><title>cross frame</title>"#,
    r#"<script>window.__log=[];['click','mousedown','keydown','input','beforeinput'].forEach(function(t){document.addEventListener(t,function(e){window.__log.push({t:e.type,tr:e.isTrusted,id:(e.target&&e.target.id)||'',d:e.data||''});},true);});</script>"#,
    r#"</head><body style="margin:4px;font:14px sans-serif;background:#fed"><button id="xb" onclick="this.textContent='Cross clicked'">Cross button</button> <input id="xi" aria-label="Cross input"></body></html>"#
);

pub const PAGE2: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Page two</title></head><body><h1>Page two</h1><a href="/main">back to main</a><button onclick="history.pushState({}, '', '/page2#pushed')">push</button></body></html>"#;

pub const SLOW: &str = r#"<!doctype html><html><head><title>Slow page</title></head><body><p>slow content arrived</p><script>setTimeout(function(){var d=document.createElement('div');d.id='late';d.textContent='late text appears';document.body.appendChild(d);},800);</script></body></html>"#;

/// The mini-app: everything it tries must fail (no network, no file://) except its own scheme.
pub fn app(port: u16, udp: u16) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Mini app</title>
<link rel="stylesheet" href="http://127.0.0.1:{port}/sbx-css">
<link rel="prefetch" href="http://127.0.0.1:{port}/sbx-prefetch">
</head><body><h1>Mini app</h1><img src="http://127.0.0.1:{port}/sbx-img"><img src="app.png">
<iframe src="http://127.0.0.1:{port}/sbx-iframe"></iframe>
<iframe src="file:///etc/hosts" id="ff"></iframe>
<script>
window.__r = {{}};
function note(k, v) {{ window.__r[k] = v; }}
fetch('http://127.0.0.1:{port}/sbx-fetch').then(function(){{note('fetch','LEAK')}}, function(e){{note('fetch','blocked: '+e)}});
fetch('http://localhost:{port}/sbx-fetch-localhost').then(function(){{note('fetch_localhost','LEAK')}}, function(e){{note('fetch_localhost','blocked: '+e)}});
fetch('https://example.com/sbx-remote').then(function(){{note('fetch_remote','LEAK')}}, function(e){{note('fetch_remote','blocked: '+e)}});
fetch('file:///etc/hosts').then(function(r){{return r.text()}}).then(function(t){{note('file','LEAK '+t.length)}}, function(e){{note('file','blocked: '+e)}});
fetch('app.json').then(function(r){{return r.text()}}).then(function(t){{note('own','ok '+t)}}, function(e){{note('own','failed: '+e)}});
fetch('flare-app://other/secret.txt').then(function(r){{return r.text()}}).then(function(t){{note('other_app','LEAK '+t)}}, function(e){{note('other_app','blocked: '+e)}});
fetch('flare-app://app/../../../../etc/hosts').then(function(r){{return r.text()}}).then(function(t){{note('traversal', t.indexOf('localhost')>=0 ? 'LEAK' : 'served app text: '+t.slice(0,30))}}, function(e){{note('traversal','blocked: '+e)}});
try {{ var x = new XMLHttpRequest(); x.open('GET','http://127.0.0.1:{port}/sbx-xhr'); x.onload=function(){{note('xhr','LEAK')}}; x.onerror=function(){{note('xhr','blocked')}}; x.send(); }} catch (e) {{ note('xhr','threw '+e); }}
try {{ var w = new WebSocket('ws://127.0.0.1:{port}/sbx-ws'); w.onopen=function(){{note('ws','LEAK')}}; w.onerror=function(){{note('ws','blocked')}}; }} catch (e) {{ note('ws','threw '+e); }}
try {{ var es = new EventSource('http://127.0.0.1:{port}/sbx-es'); es.onerror=function(){{note('es','blocked'); es.close();}}; }} catch (e) {{ note('es','threw '+e); }}
try {{ note('beacon', String(navigator.sendBeacon('http://127.0.0.1:{port}/sbx-beacon', 'x'))); }} catch (e) {{ note('beacon','threw '+e); }}
note('secure_context', String(window.isSecureContext));
note('subtle', String(!!(window.crypto && crypto.subtle)));
note('rtc_type', typeof RTCPeerConnection);
try {{
  var pc = new RTCPeerConnection({{iceServers: [{{urls: 'stun:127.0.0.1:{udp}'}}]}});
  pc.createDataChannel('x');
  pc.createOffer().then(function(o){{return pc.setLocalDescription(o)}}).then(function(){{note('rtc','offer set')}}, function(e){{note('rtc','failed '+e)}});
}} catch (e) {{ note('rtc','threw '+e); }}
// The classic way round a removed API: a fresh about:blank frame's own copy.
try {{
  var f = document.createElement('iframe'); document.body.appendChild(f);
  note('rtc_in_blank_frame', typeof f.contentWindow.RTCPeerConnection);
}} catch (e) {{ note('rtc_in_blank_frame','threw '+e); }}
try {{ var wk = new Worker(URL.createObjectURL(new Blob(["fetch('http://127.0.0.1:{port}/sbx-worker').then(function(){{postMessage('LEAK')}},function(e){{postMessage('blocked: '+e)}})"]))); wk.onmessage=function(m){{note('worker', m.data)}}; wk.onerror=function(e){{note('worker','error '+e.message)}}; }} catch (e) {{ note('worker','threw '+e); }}
setTimeout(function(){{ try {{ location.href = 'http://127.0.0.1:{port}/sbx-nav'; }} catch (e) {{ note('nav','threw '+e); }} }}, 1200);
</script></body></html>"#
    )
}
