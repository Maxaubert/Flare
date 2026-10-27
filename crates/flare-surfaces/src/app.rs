//! Mini-apps and diagrams (Surfaces stage 5): the logic of a page an agent writes and Flare shows in
//! the tab's web panel, with no COM and no Slint.
//!
//! A mini-app is one HTML page (a chart, a calculator, an explainer, a 3D viewer) or a Mermaid diagram.
//! Its content is written into a folder of its own under the profile (`<profile>\apps\a<n>`), served by
//! the browser from a host name of its own (`flare-app-<n>.local`: each app is a site of its own, so two
//! mini-apps never share an origin, storage or cookies, not even through a `Domain=` cookie) and shown in a browser view of its own profile (private, apart from the
//! web panel's: no cookies, no cache and no history shared).
//!
//! The sandbox is a Content-Security-Policy that Flare writes into the page itself as the first thing in
//! it (a `<meta http-equiv>`), because a policy delivered by a meta tag is fixed for the document once
//! the parser has seen it: a script that removes the tag later removes nothing, and a second policy the
//! page adds can only narrow it. No network by default; `allow_network` (and the user's switch) lets the
//! page load `https:` content. Navigating away from the app's address is cancelled by the browser glue
//! (`nav_allowed`), as is every request that is not the app's own, a `data:`/`blob:` one or, with the
//! network on, an `https:` one (`request_allowed`: a second wall behind the policy).
//!
//! Rules it holds: 13 and 14 (a page named by path is resolved again by the backend and must be an
//! `html` file as typed and as the file system names it), 34 (the writes here are for a worker thread
//! or the control channel's own call thread, never the UI thread), S4 (a separate profile, never the
//! web panel's or Edge's).

use std::io;
use std::path::{Path, PathBuf};

use crate::web::url_matches;

/// The most HTML a mini-app takes (bytes).
pub const HTML_MAX: usize = 4 * 1024 * 1024;
/// The most Mermaid text a diagram takes (bytes).
pub const DIAGRAM_MAX: usize = 200 * 1024;
/// What every mini-app host starts with (then the number of the app).
pub const HOST_PREFIX: &str = "flare-app-";
/// What every mini-app host ends with. `local` is a top-level name, so a cookie cannot be set for it,
/// and the site of an app is its whole host (a shared parent would be one site for all of them).
pub const HOST_SUFFIX: &str = ".local";
/// The page of every mini-app inside its folder.
pub const PAGE: &str = "index.html";
/// The Mermaid build, beside the diagram page.
pub const MERMAID_FILE: &str = "mermaid.min.js";
/// The name of the browser profile mini-apps use (private: it keeps nothing on disk).
pub const PROFILE_NAME: &str = "flare-app";
/// The folder inside the profile that holds the apps.
pub const FOLDER: &str = "apps";

/// What a browser view needs to show one mini-app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppSpec {
    /// The host name the folder is served on.
    pub host: String,
    /// The folder with the page (and the Mermaid build for a diagram).
    pub folder: PathBuf,
    /// The address of the page: `https://<host>/index.html`.
    pub url: String,
    /// The page may load `https:` content (the agent asked and the user's switch is on).
    pub allow_network: bool,
    /// A mod's view (stage 8): the mod, and the browser serves its files itself from `folder` on every
    /// request instead of mapping the folder. None for a mini-app or a diagram.
    pub module: Option<crate::modview::ModSpec>,
}

/// The host name of the `seq`th mini-app of the run.
pub fn app_host(seq: u64) -> String {
    format!("{HOST_PREFIX}{seq}{HOST_SUFFIX}")
}

/// The address of the page served on `host`.
pub fn app_url(host: &str) -> String {
    format!("https://{host}/{PAGE}")
}

/// Is this address a mini-app's page? (A stored panel of one is never restored: it has no content.)
pub fn is_app_url(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && u.host_str().is_some_and(|h| {
            h.strip_suffix(HOST_SUFFIX)
                .and_then(|p| p.strip_prefix(HOST_PREFIX))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// Run in every frame of an app before its own code: the page loses the peer-connection API. The
/// policy has no directive for it and its STUN/TURN and address lookups never reach the request wall,
/// so a page with the network off could still send data out through a name lookup.
pub const NO_WEBRTC_SCRIPT: &str = "(function(){for(var n of ['RTCPeerConnection','webkitRTCPeerConnection','RTCDataChannel','RTCIceCandidate','RTCSessionDescription','RTCDtlsTransport','RTCIceTransport','RTCSctpTransport']){try{delete window[n];Object.defineProperty(window,n,{value:undefined,writable:false,configurable:false})}catch(e){}}})();";

/// The Content-Security-Policy of a mini-app. Without the network, nothing but the app's own address,
/// inline code and `data:`/`blob:` content loads and nothing is sent anywhere (`connect-src`). With it,
/// `https:` content may load too. Plugins, `<base>` and form posts are off either way.
pub fn csp(allow_network: bool) -> String {
    let net = if allow_network { " https:" } else { "" };
    format!(
        "default-src 'self' 'unsafe-inline' data: blob:{net}; connect-src 'self' data: blob:{net}; \
         frame-src 'none'; child-src 'none'; worker-src 'self' blob:; \
         object-src 'none'; base-uri 'none'; form-action 'none'"
    )
}

/// The WebRTC address policy of every mini-app browser: no UDP that does not go through a proxy, so a
/// peer connection can send no STUN packet and gather no address on its own. Measured on WebView2 155
/// (the e2e `miniApp` "webrtc" checks): `--webrtc-ip-handling-policy` is the switch this runtime honours;
/// Chromium's `--force-webrtc-ip-handling-policy` alone did NOT change anything (STUN still left and
/// host candidates were gathered), so both are passed, the first for the runtime and the second for a
/// runtime that reads only the Chromium name.
pub const WEBRTC_POLICY_FLAG: &str = concat!(
    "--webrtc-ip-handling-policy=disable_non_proxied_udp ",
    "--force-webrtc-ip-handling-policy=disable_non_proxied_udp"
);
/// Sends every network request of a sealed mini-app browser to a proxy that is not there (port 9, the
/// discard port; nothing listens) and takes loopback out of the built-in bypass, so the browser has no
/// way out of its own even if the page and the request wall both missed something. The app's own
/// pages are served from a folder by a host mapping, which does not use the network.
///
/// Port 9 needs no privilege on Windows, so a local program could listen there and act as a proxy;
/// the second wall does not depend on the port staying free: the browser's own resolver maps every
/// name to "not found", so no lookup of its own can succeed (the app's page is a host mapping and
/// never uses the resolver).
pub const DEAD_PROXY_FLAGS: &str = concat!(
    "--proxy-server=http://127.0.0.1:9 --proxy-bypass-list=<-loopback> ",
    "--host-resolver-rules=\"MAP * ~NOTFOUND\""
);
/// Environment variables that REPLACE what Flare passes to the browser (the arguments, the profile
/// folder, the runtime folder). They are removed from the process before a mini-app browser starts.
pub const OVERRIDE_ENV_VARS: [&str; 3] = [
    "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
    "WEBVIEW2_USER_DATA_FOLDER",
    "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
];
/// Policy keys (under `Software\Policies\Microsoft\Edge\WebView2`, in HKCU and HKLM) that replace the
/// same things. Any value in one of them and a sealed browser is refused, not started without its walls.
pub const OVERRIDE_POLICY_KEYS: [&str; 3] = [
    "AdditionalBrowserArguments",
    "UserDataFolder",
    "BrowserExecutableFolder",
];

/// The refusal for a mini-app when a policy overrides the browser's arguments, or `None` when `found`
/// (the policy keys that hold a value) is empty.
pub fn override_refusal(found: &[String]) -> Option<String> {
    if found.is_empty() {
        None
    } else {
        Some(format!(
            "a Windows policy sets the WebView2 {} for every app, which would switch off the \
             mini-app's sandbox, so Flare will not start one",
            found.join(", ")
        ))
    }
}
/// The earliest browser an app environment asks for (the options object would otherwise demand the
/// version of the SDK Flare was built against and refuse an older, working runtime).
pub const APP_MIN_BROWSER: &str = "110.0.0.0";

/// The extra browser arguments of a mini-app environment. Always the WebRTC policy; the dead proxy
/// only when the app has no network (an app that was granted `https:` needs the real route).
pub fn browser_arguments(allow_network: bool) -> String {
    if allow_network {
        WEBRTC_POLICY_FLAG.to_string()
    } else {
        format!("{WEBRTC_POLICY_FLAG} {DEAD_PROXY_FLAGS}")
    }
}

/// The user data folder of a mini-app environment, beside the web panel's (`data_dir`): one for apps
/// with no network and one for apps with it (two environments cannot share a folder when their
/// browser arguments differ). The mods and plugins have a third, their own
/// (`modview::env_folder`). A mini-app keeps nothing on disk, so its sealed folder moved to `-mini-sealed`
/// (Phase 2b) without losing anything and left `-app-sealed` to the mods.
pub fn env_folder(data_dir: &Path, allow_network: bool) -> PathBuf {
    let tail = if allow_network {
        "app-net"
    } else {
        "mini-sealed"
    };
    sibling_folder(data_dir, tail)
}

/// `<data_dir>-<tail>`, beside the web panel's folder.
pub(crate) fn sibling_folder(data_dir: &Path, tail: &str) -> PathBuf {
    let name = data_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "WebView2".to_string());
    data_dir.with_file_name(format!("{name}-{tail}"))
}

/// The page as it is written: the policy first, so nothing the page says comes before it.
pub fn wrap_html(html: &str, allow_network: bool) -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{}\"><meta http-equiv=\"x-dns-prefetch-control\" content=\"off\">\n{html}",
        csp(allow_network)
    )
}

/// May the app's view go to `uri`? Only to its own page (a `#fragment` is the page's own business).
pub fn nav_allowed(uri: &str, app_url: &str) -> bool {
    url_matches(app_url, uri, false)
}

/// May a request of the app's view go out? Its own host, `data:` and `blob:` always; `https:` only when
/// the network is on. The browser's policy stops the rest before a request is made; this is the wall
/// behind it (a request the policy missed is answered with a refusal and never leaves the PC).
pub fn request_allowed(uri: &str, host: &str, allow_network: bool) -> bool {
    let Ok(u) = url::Url::parse(uri) else {
        return false;
    };
    match u.scheme() {
        "data" => true,
        "blob" => true,
        "https" => allow_network || u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(host)),
        _ => false,
    }
}

/// The panel's header for an app with no title given.
pub fn default_title(is_diagram: bool) -> &'static str {
    if is_diagram { "Diagram" } else { "Mini-app" }
}

/// The page that draws a Mermaid diagram. The text goes in as a JSON string inside an inert script
/// element (`<` is escaped, so nothing in it can end the element), the library is the file beside it,
/// `securityLevel` is `strict`. The page's `window.__flareDiagram` is a promise that settles to
/// `{ok: true}` or `{ok: false, error: "..."}`; on an error the panel shows the text as well.
pub fn diagram_page(mermaid: &str) -> String {
    let json = serde_json::Value::String(mermaid.to_string()).to_string();
    let escaped = json
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    format!(
        r##"<!doctype html><html><head><meta charset="utf-8"><title>Diagram</title>
<style>
:root {{ color-scheme: light dark; }}
body {{ margin: 0; padding: 16px; font: 14px system-ui, sans-serif; background: Canvas; color: CanvasText; }}
#out {{ display: flex; justify-content: center; }}
#out svg {{ max-width: 100%; height: auto; }}
#err {{ white-space: pre-wrap; font: 13px Consolas, monospace; color: #b00020; margin: 0 0 12px; }}
@media (prefers-color-scheme: dark) {{ #err {{ color: #ff8a80; }} }}
</style></head>
<body><pre id="err" hidden></pre><div id="out"></div>
<script id="src" type="application/json">{escaped}</script>
<script src="{MERMAID_FILE}"></script>
<script>
window.__flareDiagram = (async function () {{
  var err = document.getElementById('err');
  try {{
    var src = JSON.parse(document.getElementById('src').textContent);
    var dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    mermaid.initialize({{ startOnLoad: false, securityLevel: 'strict', theme: dark ? 'dark' : 'default' }});
    await mermaid.parse(src);
    var r = await mermaid.render('flare-diagram', src);
    document.getElementById('out').innerHTML = r.svg;
    return {{ ok: true }};
  }} catch (e) {{
    var m = String((e && e.message) || e);
    err.textContent = m;
    err.hidden = false;
    return {{ ok: false, error: m }};
  }}
}})();
</script></body></html>"##
    )
}

/// The most of a diagram's error text that is kept (a Mermaid parse error is a few lines).
pub const ERROR_MAX: usize = 800;

/// Reads the DevTools answer to `Runtime.evaluate` of `window.__flareDiagram` (awaited, by value): Ok
/// when the drawing worked, else the words to give the agent (Mermaid's own error text, plain text with
/// no control characters).
pub fn diagram_result(cdp: &str) -> Result<(), String> {
    let v: serde_json::Value =
        serde_json::from_str(cdp).map_err(|_| "the page gave no readable answer".to_string())?;
    if let Some(ex) = v.get("exceptionDetails") {
        let text = ex
            .pointer("/exception/description")
            .or_else(|| ex.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("the page threw");
        return Err(clean_error(text));
    }
    let value = v.pointer("/result/value");
    match value.and_then(|x| x.get("ok")).and_then(|b| b.as_bool()) {
        Some(true) => Ok(()),
        Some(false) => Err(clean_error(
            value
                .and_then(|x| x.get("error"))
                .and_then(|e| e.as_str())
                .unwrap_or("the drawing failed"),
        )),
        None => Err("the page gave no answer about the drawing".to_string()),
    }
}

/// Error text for the agent: hidden characters (the shared list) go, control characters become spaces, runs of blanks collapse, empty lines go
/// (the line breaks of the message stay) and the length is capped.
fn clean_error(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        let line: String = line
            .chars()
            .filter(|c| !flare_ctl::text::invisible(*c))
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
    }
    if out.chars().count() > ERROR_MAX {
        out = out.chars().take(ERROR_MAX).collect::<String>() + "...";
    }
    if out.is_empty() {
        "the drawing failed".to_string()
    } else {
        out
    }
}

/// The apps folder inside a profile.
pub fn apps_root(profile: &Path) -> PathBuf {
    profile.join(FOLDER)
}

/// Writes one mini-app into a folder of its own under `root` and says how to show it. `files` are the
/// folder's files by name (`index.html`, and for a diagram the Mermaid build); a name with a separator
/// is refused. The folder is new: a leftover of the same number is emptied first.
pub fn write_app(
    root: &Path,
    seq: u64,
    files: &[(&str, &[u8])],
    allow_network: bool,
) -> io::Result<AppSpec> {
    let host = app_host(seq);
    let folder = root.join(format!("a{seq}"));
    if folder.exists() {
        std::fs::remove_dir_all(&folder)?; // sync-ok: a worker or the channel call's own thread, never the UI thread
    }
    std::fs::create_dir_all(&folder)?; // sync-ok: a worker or the channel call's own thread, never the UI thread
    for (name, bytes) in files {
        if name.is_empty() || name.contains(['/', '\\', ':']) || *name == ".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a mini-app file name has a separator",
            ));
        }
        std::fs::write(folder.join(name), bytes)?; // sync-ok: a worker or the channel call's own thread, never the UI thread
    }
    Ok(AppSpec {
        url: app_url(&host),
        host,
        folder,
        allow_network,
        module: None,
    })
}

/// Removes an app's folder, but only a folder directly inside `root` (nothing else is ever deleted
/// here, whatever the caller passes). True when the folder is gone.
pub fn remove_app(root: &Path, folder: &Path) -> bool {
    if folder.parent() != Some(root) {
        return false;
    }
    match std::fs::remove_dir_all(folder) {
        // sync-ok: run off the UI thread (startup, a worker)
        Ok(()) => true,
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}

/// The start of the name an apps folder is renamed to before it is deleted.
const OLD_PREFIX: &str = "apps.old-";

/// Startup: the apps of an earlier run have no panel. The folder is renamed aside (one cheap call, so
/// nothing waits for a recursive delete and the channel can write a fresh root at once) and every
/// renamed folder, this one and any a crash left, is deleted on a worker. When the rename is refused
/// the folder is emptied in place instead.
pub fn retire_root(root: &Path) {
    let Some(parent) = root.parent().map(Path::to_path_buf) else {
        return;
    };
    if root.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let aside = parent.join(format!("{OLD_PREFIX}{}-{stamp}", std::process::id()));
        if let Err(e) = std::fs::rename(root, &aside) {
            // sync-ok: startup, one call
            log::warn!("the mini-app folder was not renamed ({e}); emptying it in place");
            clean_root(root);
        }
    }
    if let Err(e) = std::thread::Builder::new()
        .name("flare-app-clean".into())
        .spawn(move || {
            sweep_old(&parent);
        })
    {
        log::warn!("could not start the mini-app cleaner: {e}");
    }
}

/// Deletes every `apps.old-*` folder inside `parent` (a worker). Returns how many went.
pub fn sweep_old(parent: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(parent) else {
        // sync-ok: run off the UI thread (a worker)
        return 0;
    };
    let mut gone = 0;
    for entry in rd.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(OLD_PREFIX) {
            continue;
        }
        match std::fs::remove_dir_all(entry.path()) {
            // sync-ok: run off the UI thread (a worker)
            Ok(()) => gone += 1,
            Err(e) => log::warn!("an old mini-app folder was left: {e}"),
        }
    }
    gone
}

/// Empties the apps folder (startup: the apps of an earlier run have no panel). Returns how many
/// folders went. A folder that cannot be removed (a browser still holds a file) is left for the next
/// start and logged.
pub fn clean_root(root: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(root) else {
        // sync-ok: run off the UI thread (startup, a worker)
        return 0;
    };
    let mut gone = 0;
    for entry in rd.flatten() {
        let p = entry.path();
        let r = if p.is_dir() {
            std::fs::remove_dir_all(&p) // sync-ok: run off the UI thread (startup, a worker)
        } else {
            std::fs::remove_file(&p) // sync-ok: run off the UI thread (startup, a worker)
        };
        match r {
            Ok(()) => gone += 1,
            Err(e) => log::warn!("a mini-app folder was left: {e}"),
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_one_per_app_and_never_the_web_panels() {
        assert_eq!(app_host(7), "flare-app-7.local");
        assert_eq!(
            app_url("flare-app-7.local"),
            "https://flare-app-7.local/index.html"
        );
        assert!(is_app_url("https://flare-app-7.local/index.html"));
        assert!(is_app_url("https://FLARE-APP-12.LOCAL/index.html"));
        for u in [
            "https://flare-app.local/index.html",
            "https://flare-app-.local/index.html",
            "https://flare-app-x.local/",
            "https://a7.flare-app.local/",
            "https://x.flare-app-7.local/",
            "http://flare-app-7.local/",
            "https://flare-app-7.local.evil.example/",
            "https://evil-flare-app-7.local/",
            "https://flare-test.local/",
            "https://example.com/",
            "not a url",
        ] {
            assert!(!is_app_url(u), "{u}");
        }
    }

    #[test]
    fn the_policy_has_no_network_unless_asked() {
        let off = csp(false);
        assert!(off.contains("default-src 'self' 'unsafe-inline' data: blob:;"));
        assert!(off.contains("connect-src 'self' data: blob:;"));
        assert!(!off.contains("https:"), "{off}");
        assert!(!off.contains("http:"), "{off}");
        let on = csp(true);
        assert!(on.contains("default-src 'self' 'unsafe-inline' data: blob: https:;"));
        assert!(on.contains("connect-src 'self' data: blob: https:;"));
        assert!(!on.contains("http:"), "plain http never: {on}");
        for p in [&off, &on] {
            assert!(p.contains("object-src 'none'"));
            assert!(p.contains("base-uri 'none'"));
            assert!(p.contains("form-action 'none'"));
            assert!(!p.contains("unsafe-eval"));
        }
    }

    #[test]
    fn the_policy_comes_before_anything_the_page_says() {
        let page = wrap_html("<script>alert(1)</script>", false);
        let meta = page.find("Content-Security-Policy").unwrap();
        let script = page.find("<script>").unwrap();
        assert!(meta < script);
        assert!(page.starts_with("<!doctype html><meta charset=\"utf-8\">"));
        assert!(page.ends_with("<script>alert(1)</script>"));
        // The quotes in the policy sit inside a double-quoted attribute: no quote of ours ends it.
        let policy = csp(true);
        assert!(!policy.contains('"'));
    }

    #[test]
    fn a_view_stays_on_its_own_page() {
        let url = "https://flare-app-1.local/index.html";
        assert!(nav_allowed(url, url));
        assert!(nav_allowed("https://flare-app-1.local/index.html#top", url));
        assert!(nav_allowed("HTTPS://FLARE-APP-1.LOCAL/index.html", url));
        for u in [
            "https://flare-app-1.local/other.html",
            "https://flare-app-2.local/index.html",
            "https://example.com/",
            "http://flare-app-1.local/index.html",
            "about:blank",
            "data:text/html,hi",
            "file:///C:/x.html",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!nav_allowed(u, url), "{u}");
        }
    }

    #[test]
    fn a_request_leaves_only_as_far_as_the_network_switch_allows() {
        let host = "flare-app-1.local";
        for u in [
            "https://flare-app-1.local/index.html",
            "https://FLARE-APP-1.local/mermaid.min.js",
            "data:text/plain;base64,AAAA",
            "blob:https://flare-app-1.local/6f6c",
        ] {
            assert!(request_allowed(u, host, false), "{u}");
        }
        for u in [
            "https://example.com/x",
            "http://127.0.0.1:9/",
            "http://flare-app-1.local/",
            "https://flare-app-2.local/",
            "ws://example.com/",
            "ftp://example.com/",
            "file:///C:/x",
            "not a url",
        ] {
            assert!(!request_allowed(u, host, false), "{u}");
        }
        assert!(request_allowed(
            "https://cdn.example.com/lib.js",
            host,
            true
        ));
        assert!(!request_allowed(
            "http://cdn.example.com/lib.js",
            host,
            true
        ));
        assert!(!request_allowed("ws://cdn.example.com/", host, true));
    }

    #[test]
    fn the_diagram_text_cannot_end_its_element() {
        let evil = "graph TD\nA-->B\n</script><script>alert(1)</script>\u{2028}";
        let page = diagram_page(evil);
        // Exactly the elements the template has: two scripts with a src or code, and the data element.
        assert_eq!(page.matches("<script").count(), 3, "{page}");
        assert!(!page.contains("</script><script>alert"));
        assert!(page.contains("\\u003c/script>"));
        assert!(page.contains("securityLevel: 'strict'"));
        assert!(page.contains(r#"<script src="mermaid.min.js"></script>"#));
        // The data element parses back to the text.
        let start =
            page.find("type=\"application/json\">").unwrap() + "type=\"application/json\">".len();
        let end = page[start..].find("</script>").unwrap() + start;
        let back: String = serde_json::from_str(&page[start..end]).unwrap();
        assert_eq!(back, evil);
    }

    #[test]
    fn a_drawn_diagram_is_ok_and_an_error_carries_mermaids_words() {
        let ok = r#"{"result":{"type":"object","value":{"ok":true}}}"#;
        assert_eq!(diagram_result(ok), Ok(()));
        let bad = serde_json::json!({"result": {"type": "object", "value": {
            "ok": false,
            "error": "Parse error on line 2:\ngraph TD\n--^\nExpecting 'SEMI', got 'X'"
        }}})
        .to_string();
        let e = diagram_result(&bad).unwrap_err();
        assert!(e.starts_with("Parse error on line 2:"), "{e}");
        assert!(e.contains("Expecting 'SEMI'"), "{e}");
        assert_eq!(e.lines().count(), 4);
        // A throw outside the page's own catch.
        let thrown = r#"{"result":{"type":"object"},"exceptionDetails":{"text":"Uncaught","exception":{"description":"ReferenceError: mermaid is not defined"}}}"#;
        assert_eq!(
            diagram_result(thrown).unwrap_err(),
            "ReferenceError: mermaid is not defined"
        );
        // Anything else is an error, never a quiet yes.
        for junk in [
            "",
            "not json",
            "{}",
            r#"{"result":{"value":{"ok":"yes"}}}"#,
            r#"{"result":{"value":null}}"#,
        ] {
            assert!(diagram_result(junk).is_err(), "{junk:?}");
        }
    }

    #[test]
    fn an_error_is_plain_short_text() {
        let long = "x".repeat(2000);
        let cdp = serde_json::json!({"result": {"value": {
            "ok": false,
            "error": format!("a\u{1b}[31m\u{202E}b\u{200B}\n\n  c\t{long}")
        }}})
        .to_string();
        let e = diagram_result(&cdp).unwrap_err();
        assert!(!e.contains('\u{1b}'));
        assert!(e.starts_with("a [31mb\nc x"), "{e}");
        assert!(e.chars().count() <= ERROR_MAX + 3);
        assert!(e.ends_with("..."));
        let empty = r#"{"result":{"value":{"ok":false,"error":"  \n "}}}"#;
        assert_eq!(diagram_result(empty).unwrap_err(), "the drawing failed");
    }

    #[test]
    fn an_app_is_written_to_a_folder_of_its_own_and_removed_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = apps_root(dir.path());
        let spec = write_app(&root, 3, &[(PAGE, b"<p>hi"), ("x.js", b"1")], false).unwrap();
        assert_eq!(spec.host, "flare-app-3.local");
        assert_eq!(spec.url, "https://flare-app-3.local/index.html");
        assert_eq!(spec.folder, root.join("a3"));
        assert!(!spec.allow_network);
        assert_eq!(std::fs::read(spec.folder.join(PAGE)).unwrap(), b"<p>hi");
        assert!(spec.folder.join("x.js").is_file());
        // A second app of the same number replaces the first.
        let again = write_app(&root, 3, &[(PAGE, b"<p>new")], true).unwrap();
        assert!(again.allow_network);
        assert!(!again.folder.join("x.js").exists());
        assert_eq!(std::fs::read(again.folder.join(PAGE)).unwrap(), b"<p>new");
        assert!(remove_app(&root, &spec.folder));
        assert!(!spec.folder.exists());
        // Gone already is fine.
        assert!(remove_app(&root, &spec.folder));
    }

    #[test]
    fn a_file_name_with_a_separator_is_refused_and_leaves_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        let root = apps_root(dir.path());
        for bad in ["..\\x.txt", "../x.txt", "a/b.txt", "c:x.txt", "", ".."] {
            assert!(
                write_app(&root, 1, &[(bad, b"x")], false).is_err(),
                "{bad:?}"
            );
        }
        assert!(!dir.path().join("x.txt").exists());
    }

    #[test]
    fn only_a_folder_directly_inside_the_apps_root_is_ever_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = apps_root(dir.path());
        std::fs::create_dir_all(&root).unwrap();
        let keep = dir.path().join("keep");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("f"), b"x").unwrap();
        assert!(!remove_app(&root, &keep), "a sibling of the root");
        assert!(!remove_app(&root, dir.path()), "the profile");
        assert!(!remove_app(&root, &root), "the root itself");
        let nested = root.join("a1").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(!remove_app(&root, &nested), "not a direct child");
        assert!(keep.join("f").is_file());
        assert!(nested.is_dir());
    }

    #[test]
    fn startup_empties_the_apps_folder_and_leaves_the_profile_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = apps_root(dir.path());
        write_app(&root, 1, &[(PAGE, b"a")], false).unwrap();
        write_app(&root, 2, &[(PAGE, b"b")], false).unwrap();
        std::fs::write(root.join("stray.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("panels.json"), b"{}").unwrap();
        assert_eq!(clean_root(&root), 3);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert!(dir.path().join("panels.json").is_file());
        // No folder yet is no error.
        assert_eq!(clean_root(&dir.path().join("none")), 0);
    }

    #[test]
    fn retiring_the_root_renames_it_aside_and_the_sweep_deletes_every_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = apps_root(dir.path());
        write_app(&root, 1, &[(PAGE, b"a")], false).unwrap();
        std::fs::create_dir_all(dir.path().join("apps.old-crash-1/a9")).unwrap();
        std::fs::write(dir.path().join("panels.json"), b"{}").unwrap();
        retire_root(&root);
        assert!(!root.exists(), "the folder is aside at once");
        // The worker may still be running: the sweep is idempotent, so run it here as well.
        sweep_old(dir.path());
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["panels.json".to_string()]);
        // No folder yet is no error.
        retire_root(&dir.path().join("none"));
    }

    #[test]
    fn the_page_loses_peer_connections_and_prefetch_before_its_own_code() {
        for name in [
            "RTCPeerConnection",
            "webkitRTCPeerConnection",
            "RTCDataChannel",
        ] {
            assert!(NO_WEBRTC_SCRIPT.contains(name), "{name}");
        }
        assert!(wrap_html("", false).contains("x-dns-prefetch-control"));
    }

    #[test]
    fn the_policy_has_no_child_frames_and_only_own_or_blob_workers() {
        for net in [false, true] {
            let c = csp(net);
            assert!(c.contains("frame-src 'none'"), "{c}");
            assert!(c.contains("child-src 'none'"), "{c}");
            assert!(c.contains("worker-src 'self' blob:;"), "{c}");
        }
    }

    #[test]
    fn a_sealed_app_browser_has_the_webrtc_policy_and_a_dead_proxy_a_networked_one_only_the_policy()
    {
        let sealed = browser_arguments(false);
        assert!(sealed.starts_with("--webrtc-ip-handling-policy=disable_non_proxied_udp "));
        assert!(sealed.contains("--force-webrtc-ip-handling-policy=disable_non_proxied_udp"));
        assert!(sealed.contains("--proxy-server=http://127.0.0.1:9"));
        assert!(sealed.contains("--proxy-bypass-list=<-loopback>"));
        assert!(sealed.contains("--host-resolver-rules=\"MAP * ~NOTFOUND\""));
        let net = browser_arguments(true);
        assert!(net.contains("--webrtc-ip-handling-policy=disable_non_proxied_udp"));
        assert!(!net.contains("proxy"));
    }

    #[test]
    fn an_overriding_policy_refuses_the_app_and_none_lets_it_start() {
        assert!(override_refusal(&[]).is_none());
        let r = override_refusal(&["AdditionalBrowserArguments".to_string()]).unwrap();
        assert!(r.contains("AdditionalBrowserArguments"));
        assert!(OVERRIDE_ENV_VARS.contains(&"WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"));
        assert!(OVERRIDE_POLICY_KEYS.contains(&"AdditionalBrowserArguments"));
    }

    #[test]
    fn the_two_app_environments_have_folders_of_their_own_beside_the_panels() {
        let d = Path::new("C:/p/WebView2");
        let (a, b) = (env_folder(d, false), env_folder(d, true));
        assert_ne!(a, b);
        assert_ne!(a, d);
        assert_eq!(a.parent(), d.parent());
        assert_eq!(b.parent(), d.parent());
        assert!(a.ends_with("WebView2-mini-sealed") && b.ends_with("WebView2-app-net"));
    }
}
