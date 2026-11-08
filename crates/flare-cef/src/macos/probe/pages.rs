//! The engine tests' pages, from the phase 1 spike (`wk_spike/pages.rs` and the trap of `adv.rs`), so the CEF run
//! answers the same questions the WKWebView run did. Each page logs, in its own world, every input event with its
//! `isTrusted` flag into `window.__log`, so a test reads back what the page really saw.

/// The page-world event logger.
pub const LOGGER: &str = r#"<script>
window.__log = [];
(function () {
  function L(e) {
    var t = e.target || {};
    window.__log.push({t: e.type, tr: e.isTrusted, id: (t.id || (t.nodeType === 9 ? 'document' : '')),
      k: e.key || '', c: e.code || '', d: e.data || '', btn: (e.button === undefined ? -1 : e.button)});
  }
  ['click','mousedown','mouseup','keydown','keyup','beforeinput','input','wheel','focus'].forEach(function (t) {
    document.addEventListener(t, L, true);
  });
})();
</script>"#;

/// The agent contract page (the spike's main page, without the parts that would open something native).
pub fn main() -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Engine main</title>
<style>body{{font:15px -apple-system,sans-serif;margin:8px}} #tall{{height:2600px;background:linear-gradient(#fff,#bcd)}}
#ce{{border:1px solid #999;min-height:22px;width:300px}}</style>
{LOGGER}</head><body>
<h1>Agent contract</h1>
<p>Some intro text for the reader.</p>
<button id="b1" onclick="document.getElementById('out').textContent='clicked b1 '+event.isTrusted">Press me</button>
<a id="l1" href="/page2">Go to page two</a>
<label for="t1">Name</label> <input id="t1" placeholder="your name">
<input type="checkbox" id="c1" aria-label="Agree">
<textarea id="ta" aria-label="Notes" rows="2"></textarea>
<div id="ce" contenteditable="true" aria-label="Editor"></div>
<button id="dis" disabled>Disabled one</button>
<div id="out" role="status">nothing yet</div>
<iframe id="fs" src="/same" title="Same origin frame" style="width:380px;height:80px"></iframe>
<div id="tall">tall area to scroll</div>
<p id="bottom">The bottom line</p>
</body></html>"#
    )
}

pub const SAME: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>same frame</title></head><body style="margin:4px;font:14px sans-serif"><button id="fb" onclick="this.textContent='Frame clicked'">Frame button</button></body></html>"#;

pub const PAGE2: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Page two</title></head><body><h1>Page two</h1><a href="/main">back to main</a></body></html>"#;

/// A known colour in a known place, for the pixel buffer.
pub const PAINT: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Paint</title>
<style>html,body{margin:0;background:#ffffff} #sq{position:absolute;left:0;top:0;width:200px;height:100px;background:#3366cc}</style>
</head><body><div id="sq"></div><p style="position:absolute;top:150px;left:10px">painted text</p></body></html>"#;

/// State a runaway must not lose, and a page function that never returns.
pub const RUNAWAY: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Runaway</title></head><body>
<button id="rb">Runaway button</button><p id="txt">runaway text</p>
<script>window.__state = {n: 41, born: Date.now()}; function hang() { while (true) {} }</script></body></html>"#;

/// The spike's trap (`adv.rs`, `/adv/trap`): a `devicePixelRatio` getter that never returns, set by the page for
/// whatever the agent touches. Plus state to keep.
pub const TRAP: &str = "<!doctype html><title>Trap</title><button>Trap button</button><p>trap text</p>\
<script>window.__kept = 7; Object.defineProperty(window,'devicePixelRatio',{get:function(){while(true){}}});</script>";
