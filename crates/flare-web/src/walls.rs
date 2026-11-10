//! The walls of every kind of view, as decisions: which address a frame may go to, what a request gets, which
//! profile a view runs in. The Windows host puts them up through WebView2's events (`host.rs`); the macOS host
//! (`macos/host.rs`) asks these functions from CEF's handlers, so the two say the same thing and the decisions are
//! tested here, on every platform, without a browser.

use std::path::{Path, PathBuf};

use flare_surfaces::app::{self, AppSpec, PROFILE_NAME};
use flare_surfaces::modview::{self, Route};
use flare_surfaces::web::navigation_allowed;

/// Which browser a view runs in (the Windows host's `EnvKind`): a mini-app never shares one with the web panel, an
/// app with the network and one without differ, the mods and plugins have their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Web,
    AppSealed,
    AppNet,
    Plugin,
    PluginNet,
}

impl Kind {
    pub fn of(app: Option<&AppSpec>) -> Kind {
        match app {
            None => Kind::Web,
            Some(a) if a.module.is_some() && a.allow_network => Kind::PluginNet,
            Some(a) if a.module.is_some() => Kind::Plugin,
            Some(a) if a.allow_network => Kind::AppNet,
            Some(_) => Kind::AppSealed,
        }
    }

    /// Every request goes through a proxy: a dead one (no network) or Flare's filtering one (`https:` granted).
    /// The web panel's goes direct.
    pub fn proxied(self) -> bool {
        self != Kind::Web
    }

    pub fn networked(self) -> bool {
        matches!(self, Kind::AppNet | Kind::PluginNet)
    }
}

/// The proxy a sealed page's profile uses: nothing listens on the discard port, and loopback is taken out of the
/// bypass list, so not even this machine is reached directly (`app::DEAD_PROXY_FLAGS` on Windows).
pub const DEAD_PROXY: &str = "http://127.0.0.1:9";
/// The bypass list every proxied profile has: none, not even loopback.
pub const NO_BYPASS: &str = "<-loopback>";

/// The profile a view runs in: its name (as `ControllerProbe::profile` reports it) and whether it keeps nothing on
/// disk. The web panel's is the default profile (empty name) on disk; a mini-app's is the apps' private one; a
/// mod's or plugin's is its own, kept between runs.
pub fn profile_of(app: Option<&AppSpec>) -> (String, bool) {
    match app {
        None => (String::new(), false),
        Some(a) => match &a.module {
            Some(m) => (modview::profile_name(&m.id), false),
            None => (PROFILE_NAME.to_string(), true),
        },
    }
}

/// The name the macOS host keys a view's CEF profile by. A mod's or plugin's is its profile (one per mod, on
/// disk). A mini-app's is the apps' private profile and its own host: every mini-app has an in-memory profile of
/// its own (rule 1029), because a profile's request wall for the requests no browser makes (a shared worker's) is
/// the wall of the app it was made for, and one profile shared by two apps would judge the second app's worker
/// by the first app's folder. WebView2 maps each app's folder on its own view, so Windows needs no such key.
pub fn context_name(app: Option<&AppSpec>) -> String {
    match app {
        Some(a) if a.module.is_none() => format!("{PROFILE_NAME}/{}", a.host),
        _ => profile_of(app).0,
    }
}

/// Where a held download's bytes wait for the person's yes (Chromium's default download folder in every profile):
/// under the engine's root folder `root`, never `~/Downloads` (rule 1031).
pub fn download_staging(root: &Path) -> PathBuf {
    root.join("download-staging")
}

/// Where a profile kept on disk lives, under the engine's root folder `root` (CEF wants every profile under it).
pub fn profile_folder(root: &Path, kind: Kind, name: &str) -> PathBuf {
    match kind {
        Kind::Web => root.join("Default"),
        Kind::PluginNet => root.join("plugins-net").join(name),
        _ => root.join("mods").join(name),
    }
}

/// May a frame of a view of `app` (None: the web panel) navigate to `url`? The web panel's main frame goes to
/// http, https and about:blank, its frames anywhere; a mini-app's main frame only to its own page, a mod's to its
/// own host; their frames to their own host, `about:blank` or `about:srcdoc` (the Windows host's frame wall).
pub fn nav_allowed(app: Option<&AppSpec>, url: &str, main: bool) -> bool {
    let Some(a) = app else {
        return !main || navigation_allowed(url);
    };
    let own = match &a.module {
        Some(_) => modview::nav_allowed(url, &a.host),
        None => app::nav_allowed(url, &a.url),
    };
    if main {
        own
    } else {
        own || url == "about:blank" || url == "about:srcdoc"
    }
}

/// What one request of a view gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The browser fetches it (through the profile's proxy, if it has one).
    Pass,
    /// Answered with a 403 and no body: it never leaves the process.
    Refuse,
    /// A file of a folder, answered from the disk: (folder, entry page, decoded path).
    Serve {
        folder: PathBuf,
        entry: String,
        path: String,
        /// The mod's headers (its policy) rather than a mini-app's plain ones.
        module: bool,
    },
}

/// The request wall: what a request of a view of `app` for `url` gets. `virtual_hosts` are the e2e's local pages
/// (host to folder), served to the web panel.
pub fn decide(app: Option<&AppSpec>, virtual_hosts: &[(String, PathBuf)], url: &str) -> Decision {
    let Some(a) = app else {
        for (host, folder) in virtual_hosts {
            if let Some(path) = path_on(url, host) {
                return Decision::Serve {
                    folder: folder.clone(),
                    entry: app::PAGE.to_string(),
                    path,
                    module: false,
                };
            }
        }
        return Decision::Pass;
    };
    if let Some(m) = &a.module {
        // A plugin that holds `network` reaches public https hosts, and nothing local (plugins phase 6).
        return match modview::route_request_net(url, &a.host, a.allow_network) {
            Route::Pass => Decision::Pass,
            Route::Refuse => Decision::Refuse,
            Route::Serve(path) => Decision::Serve {
                folder: a.folder.clone(),
                entry: m.entry.clone(),
                path,
                module: true,
            },
        };
    }
    // A mini-app: its folder on its own host (WebView2's folder mapping), `data:` and `blob:`, and `https:` only
    // when the network is on.
    if let Some(path) = path_on(url, &a.host) {
        return Decision::Serve {
            folder: a.folder.clone(),
            entry: app::PAGE.to_string(),
            path,
            module: false,
        };
    }
    if app::request_allowed(url, &a.host, a.allow_network) {
        Decision::Pass
    } else {
        Decision::Refuse
    }
}

/// The decoded path of a request to `host` over https (no user part, the default port), or None.
fn path_on(url: &str, host: &str) -> Option<String> {
    modview::request_path(url, host)
}

/// The script that runs before a mini-app's or a mod's own: the peer-connection API taken away (unless the e2e
/// asked to test the walls under it) and, for a mod, its bridge over the binding.
pub fn init_script(app: &AppSpec, without_webrtc: bool) -> Option<String> {
    let mut text = String::new();
    if !without_webrtc {
        text.push_str(app::NO_WEBRTC_SCRIPT);
    }
    if app.module.is_some() {
        text.push('\n');
        text.push_str(WEBVIEW_SHIM);
        text.push('\n');
        text.push_str(flare_mods::bridge::BRIDGE_JS);
    }
    (!text.is_empty()).then_some(text)
}

/// The global function a mod's page posts its messages through (CDP `Runtime.addBinding`); the shim hides it.
pub const BINDING: &str = "__flarePost";

/// WebView2's message channel (`window.chrome.webview`: `postMessage` and the `message` event) over CEF's binding,
/// so the bridge (`flare_mods::bridge::BRIDGE_JS`) runs unchanged. The binding is taken off `window` into the
/// shim; the host's messages come in through `__flareDeliver` as JSON text, parsed as WebView2's
/// `PostWebMessageAsJson` hands them over (an object in `event.data`). Neither name can be replaced by the page.
pub const WEBVIEW_SHIM: &str = r#"(function () {
  'use strict';
  var post = window.__flarePost;
  if (typeof post !== 'function') { return; }
  try { delete window.__flarePost; } catch (e) {}
  var listeners = [];
  var wv = {
    postMessage: function (m) { post(typeof m === 'string' ? m : JSON.stringify(m)); },
    addEventListener: function (t, f) { if (t === 'message' && typeof f === 'function') { listeners.push(f); } },
    removeEventListener: function (t, f) { var i = listeners.indexOf(f); if (i >= 0) { listeners.splice(i, 1); } }
  };
  Object.freeze(wv);
  Object.defineProperty(window, '__flareDeliver', {
    value: function (text) {
      var data;
      try { data = JSON.parse(text); } catch (e) { return; }
      listeners.slice().forEach(function (f) { try { f({ data: data }); } catch (e) {} });
    },
    enumerable: false, configurable: false, writable: false
  });
  if (!window.chrome) { window.chrome = {}; }
  Object.defineProperty(window.chrome, 'webview', { value: wv, enumerable: true, configurable: false, writable: false });
})();"#;

/// The script that hands `json` to a mod's page (the bridge's `message` event): the text as a JS string literal.
pub fn deliver_script(json: &str) -> String {
    let lit = serde_json_string(json);
    format!("window.__flareDeliver && window.__flareDeliver({lit});")
}

/// `s` as a JSON (and JS) string literal.
fn serde_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Old JS engines end a line on these; written escaped either way.
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Parses `Name: value` lines (`flare_mods::serve::headers_for`) into pairs.
pub fn header_pairs(headers: &str) -> Vec<(String, String)> {
    headers
        .split("\r\n")
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_surfaces::app::{app_host, app_url};
    use flare_surfaces::modview::{mod_spec, plugin_spec};

    fn mini(net: bool) -> AppSpec {
        let host = app_host(3);
        AppSpec {
            url: app_url(&host),
            host,
            folder: PathBuf::from("/apps/3"),
            allow_network: net,
            module: None,
            source: None,
        }
    }

    #[test]
    fn every_kind_of_view_has_the_browser_windows_gives_it() {
        assert_eq!(Kind::of(None), Kind::Web);
        assert_eq!(Kind::of(Some(&mini(false))), Kind::AppSealed);
        assert_eq!(Kind::of(Some(&mini(true))), Kind::AppNet);
        let m = mod_spec("notes", Path::new("/mods/notes"), "index.html");
        assert_eq!(Kind::of(Some(&m)), Kind::Plugin);
        let p = plugin_spec("feed", Path::new("/p/feed"), "index.html", true);
        assert_eq!(Kind::of(Some(&p)), Kind::PluginNet);
        assert!(!Kind::Web.proxied() && Kind::AppSealed.proxied() && Kind::Plugin.proxied());
        assert!(Kind::AppNet.networked() && !Kind::AppSealed.networked());
        assert_eq!(profile_of(None), (String::new(), false));
        assert_eq!(
            profile_of(Some(&mini(false))),
            (PROFILE_NAME.to_string(), true)
        );
        assert_eq!(profile_of(Some(&m)), ("flare-mod-notes".to_string(), false));
        // Two mini-apps never share a profile (each has its own wall); a mod keeps its one.
        let mut other = mini(false);
        other.host = "flare-app-99.local".into();
        assert_ne!(context_name(Some(&mini(false))), context_name(Some(&other)));
        assert!(context_name(Some(&other)).starts_with(PROFILE_NAME));
        assert_eq!(context_name(Some(&m)), "flare-mod-notes");
        assert_eq!(context_name(None), "");
        assert_eq!(
            download_staging(Path::new("/d")),
            PathBuf::from("/d/download-staging")
        );
        let root = Path::new("/r");
        assert_eq!(profile_folder(root, Kind::Web, ""), root.join("Default"));
        assert_eq!(
            profile_folder(root, Kind::Plugin, "flare-mod-notes"),
            root.join("mods/flare-mod-notes")
        );
    }

    #[test]
    fn the_web_panel_goes_to_web_addresses_and_its_frames_anywhere() {
        assert!(nav_allowed(None, "https://a.example/", true));
        assert!(nav_allowed(None, "http://a.example/", true));
        assert!(nav_allowed(None, "about:blank", true));
        for bad in [
            "mailto:a@b.c",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "zoommtg://x",
        ] {
            assert!(!nav_allowed(None, bad, true), "{bad}");
        }
        assert!(nav_allowed(None, "https://ads.example/frame", false));
    }

    #[test]
    fn a_mini_app_and_a_mod_stay_on_their_own_page_and_frames() {
        let a = mini(false);
        assert!(nav_allowed(Some(&a), &a.url, true));
        assert!(!nav_allowed(Some(&a), "https://a.example/", true));
        assert!(!nav_allowed(Some(&a), "about:blank", true));
        assert!(nav_allowed(Some(&a), "about:srcdoc", false));
        assert!(!nav_allowed(Some(&a), "https://a.example/", false));
        let m = mod_spec("notes", Path::new("/mods/notes"), "index.html");
        assert!(nav_allowed(
            Some(&m),
            &format!("https://{}/x.html", m.host),
            true
        ));
        assert!(!nav_allowed(
            Some(&m),
            "https://flare-mod-other.local/",
            true
        ));
        assert!(!nav_allowed(Some(&m), "mailto:x@y.z", true));
        assert!(nav_allowed(Some(&m), "about:blank", false));
    }

    #[test]
    fn a_sealed_mini_app_gets_its_folder_and_nothing_of_the_network() {
        let a = mini(false);
        assert_eq!(
            decide(Some(&a), &[], &format!("https://{}/index.html", a.host)),
            Decision::Serve {
                folder: a.folder.clone(),
                entry: "index.html".into(),
                path: "/index.html".into(),
                module: false
            }
        );
        for out in [
            "https://a.example/x.png",
            "http://127.0.0.1:47103/advx-ping",
            "http://advdns-x.local/",
            "wss://a.example/",
        ] {
            assert_eq!(decide(Some(&a), &[], out), Decision::Refuse, "{out}");
        }
        assert_eq!(decide(Some(&a), &[], "data:text/plain,hi"), Decision::Pass);
        // With the network granted, https goes out (through Flare's filtering proxy), http never does.
        let n = mini(true);
        assert_eq!(
            decide(Some(&n), &[], "https://a.example/x.png"),
            Decision::Pass
        );
        assert_eq!(decide(Some(&n), &[], "http://a.example/"), Decision::Refuse);
    }

    #[test]
    fn a_mods_files_are_served_by_path_and_a_plugin_with_network_reaches_public_https_only() {
        let m = mod_spec("notes", Path::new("/mods/notes"), "index.html");
        assert!(matches!(
            decide(Some(&m), &[], &format!("https://{}/a%20b.js", m.host)),
            Decision::Serve { path, module: true, .. } if path == "/a b.js"
        ));
        assert_eq!(
            decide(Some(&m), &[], "https://a.example/"),
            Decision::Refuse
        );
        let p = plugin_spec("feed", Path::new("/p/feed"), "index.html", true);
        assert_eq!(decide(Some(&p), &[], "https://a.example/"), Decision::Pass);
        assert_eq!(
            decide(Some(&p), &[], "https://127.0.0.1/"),
            Decision::Refuse
        );
    }

    #[test]
    fn the_e2es_local_pages_are_served_to_the_web_panel_and_the_rest_passes() {
        let hosts = vec![("flare-test.local".to_string(), PathBuf::from("/t"))];
        assert!(matches!(
            decide(None, &hosts, "https://flare-test.local/page.html"),
            Decision::Serve { path, .. } if path == "/page.html"
        ));
        assert_eq!(decide(None, &hosts, "https://a.example/"), Decision::Pass);
    }

    #[test]
    fn a_mods_script_carries_the_shim_and_the_bridge_and_a_mini_apps_only_the_webrtc_wall() {
        let m = mod_spec("notes", Path::new("/mods/notes"), "index.html");
        let s = init_script(&m, false).unwrap();
        assert!(s.contains("RTCPeerConnection") && s.contains("__flareDeliver"));
        assert!(s.find("__flareDeliver") < s.find("window.flare"));
        let a = mini(false);
        let s = init_script(&a, false).unwrap();
        assert!(s.contains("RTCPeerConnection") && !s.contains("__flareDeliver"));
        assert_eq!(init_script(&a, true), None);
    }

    #[test]
    fn a_message_to_a_page_is_one_string_literal_whatever_it_holds() {
        let s = deliver_script(r#"{"t":"tool","x":"a\"b</script> "}"#);
        assert!(s.starts_with("window.__flareDeliver && window.__flareDeliver(\""));
        assert!(!s.contains("</script>"));
        assert!(!s.contains('\u{2028}'));
        assert_eq!(
            serde_json_string("a\"b\\c\n\u{1}"),
            "\"a\\\"b\\\\c\\n\\u0001\""
        );
    }

    #[test]
    fn the_mods_headers_read_as_pairs() {
        let h = header_pairs(&flare_mods::serve::headers_for("text/html", false));
        assert!(
            h.iter()
                .any(|(k, v)| k == "Content-Type" && v == "text/html")
        );
        assert!(
            h.iter()
                .any(|(k, v)| k == "Content-Security-Policy" && v.contains("default-src 'none'"))
        );
    }
}
