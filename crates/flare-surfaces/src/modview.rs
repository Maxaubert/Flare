//! A mod's panel as a browser view (Surfaces stage 8): the host name, the address and the rules a view of a
//! mod's page holds, with no COM and no Slint. The mod's files are served by the host from the mod folder on
//! every request (`flare_mods::serve`), never by a folder mapping, so this module only decides names and which
//! addresses may be reached.
//!
//! A mod's page is its own site, `https://flare-mod-<id>.local/`: another mod, a mini-app and the web panel are
//! other origins, so storage and cookies are never shared. Its browser profile is its own too
//! (`flare-mod-<id>`), kept apart from the web panel's, the mini-apps' and the user's Edge.

use crate::app::AppSpec;

/// What every mod host starts with (then the mod's id).
pub const HOST_PREFIX: &str = "flare-mod-";
/// What every mod host ends with. `local` is a top-level name, so a cookie cannot be set for it.
pub const HOST_SUFFIX: &str = ".local";

/// A mod's view: which mod, and the panel entry that `/` means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModSpec {
    pub id: String,
    /// The manifest's panel entry (`index.html`).
    pub entry: String,
}

/// The host name a mod's page is served on.
pub fn mod_host(id: &str) -> String {
    format!("{HOST_PREFIX}{id}{HOST_SUFFIX}")
}

/// The address of a mod's page.
pub fn mod_url(host: &str) -> String {
    format!("https://{host}/")
}

/// The name of a mod's own browser profile.
pub fn profile_name(id: &str) -> String {
    format!("{HOST_PREFIX}{id}")
}

/// The user data folder of the mods' and plugins' browser, beside the web panel's (`data_dir`): an
/// environment of their own, apart from the web panel's and the mini-apps' (design 13). Its name is the one
/// the mods shared with the sealed mini-apps before Phase 2b: every mod's profile (`flare-mod-<id>`, its
/// storage) lives in it, so it must never change.
pub fn env_folder(data_dir: &std::path::Path) -> std::path::PathBuf {
    crate::app::sibling_folder(data_dir, "app-sealed")
}

/// The mod a host name belongs to, if it is a mod host.
pub fn mod_of_host(host: &str) -> Option<&str> {
    host.strip_suffix(HOST_SUFFIX)
        .and_then(|h| h.strip_prefix(HOST_PREFIX))
        .filter(|id| !id.is_empty())
}

/// Is this address a mod's page? (A stored panel of one is never restored: the mod must be on first.)
pub fn is_mod_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https" && u.host_str().is_some_and(|h| mod_of_host(h).is_some())
    })
}

/// The view spec of a mod: its folder (the canonical one the scan found), host and address, and no network.
pub fn mod_spec(id: &str, folder: &std::path::Path, entry: &str) -> AppSpec {
    let host = mod_host(id);
    AppSpec {
        url: mod_url(&host),
        host,
        folder: folder.to_path_buf(),
        allow_network: false,
        module: Some(ModSpec {
            id: id.to_string(),
            entry: entry.to_string(),
        }),
    }
}

/// May the mod's view navigate to `uri`? Only its own host over https, with no user part and the default port;
/// any path of it (the page serves its own files). Nothing else, whatever the page asks for.
pub fn nav_allowed(uri: &str, host: &str) -> bool {
    let Ok(u) = url::Url::parse(uri) else {
        return false;
    };
    u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && u.port().is_none()
        && u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(host))
}

/// The percent-decoded path of a request to the mod's own host, for `flare_mods::serve`. None for a request
/// to anything else (the caller refuses it), or whose path decodes to something that is not UTF-8 text.
pub fn request_path(uri: &str, host: &str) -> Option<String> {
    if !nav_allowed(uri, host) {
        return None;
    }
    let u = url::Url::parse(uri).ok()?;
    let decoded = percent_decode(u.path())?;
    Some(decoded)
}

fn percent_decode(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let v = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// What the host does with one request of the mod's view.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// A file of the mod, by its decoded path: answered by the host's file server.
    Serve(String),
    /// `data:` and `blob:` content, which never touches the network: the browser handles it.
    Pass,
    /// Anything else (https to another host, http, a bad escape in the mod's own path): refused. A mod has no
    /// network.
    Refuse,
}

/// Decides one request of the mod's view.
pub fn route_request(uri: &str, host: &str) -> Route {
    if let Some(path) = request_path(uri, host) {
        return Route::Serve(path);
    }
    match url::Url::parse(uri)
        .map(|u| u.scheme().to_string())
        .as_deref()
    {
        Ok("data" | "blob") => Route::Pass,
        _ => Route::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Storage survives: the plugins' browser keeps the folder the mods' profiles were made in, and it is no
    /// longer the mini-apps' or the web panel's.
    #[test]
    fn the_plugins_browser_keeps_the_mods_folder_and_shares_it_with_nobody() {
        let d = std::path::Path::new("C:/p/WebView2");
        let p = env_folder(d);
        assert!(p.ends_with("WebView2-app-sealed"), "{p:?}");
        assert_eq!(p.parent(), d.parent());
        assert_ne!(p, crate::app::env_folder(d, false));
        assert_ne!(p, crate::app::env_folder(d, true));
        assert_ne!(p, d);
    }

    #[test]
    fn a_mod_has_a_host_a_profile_and_an_address_of_its_own() {
        assert_eq!(mod_host("notes"), "flare-mod-notes.local");
        assert_eq!(
            mod_url("flare-mod-notes.local"),
            "https://flare-mod-notes.local/"
        );
        assert_eq!(profile_name("notes"), "flare-mod-notes");
        assert_eq!(mod_of_host("flare-mod-notes.local"), Some("notes"));
        assert_eq!(mod_of_host("flare-app-1.local"), None);
        assert_eq!(mod_of_host("flare-mod-.local"), None);
        assert!(is_mod_url("https://flare-mod-notes.local/"));
        assert!(!is_mod_url("https://flare-app-1.local/index.html"));
        assert!(!is_mod_url("http://flare-mod-notes.local/"));
        // A longest id still gives a legal host label (63 characters at most).
        assert!(mod_host(&"a".repeat(40)).split('.').next().unwrap().len() <= 63);
    }

    #[test]
    fn a_spec_has_no_network_and_names_its_mod() {
        let s = mod_spec("notes", std::path::Path::new("C:/m/notes"), "index.html");
        assert!(!s.allow_network);
        assert_eq!(s.url, "https://flare-mod-notes.local/");
        assert_eq!(s.module.as_ref().unwrap().id, "notes");
    }

    #[test]
    fn navigation_stays_on_the_mods_own_host() {
        let h = "flare-mod-notes.local";
        for ok in [
            "https://flare-mod-notes.local/",
            "https://flare-mod-notes.local/a/b.html?x=1#y",
            "https://FLARE-MOD-NOTES.local/",
        ] {
            assert!(nav_allowed(ok, h), "{ok}");
        }
        for bad in [
            "http://flare-mod-notes.local/",
            "https://flare-mod-other.local/",
            "https://flare-mod-notes.local.evil.com/",
            "https://user@flare-mod-notes.local/",
            "https://flare-mod-notes.local:8443/",
            "https://evil.com/",
            "file:///C:/x",
            "javascript:alert(1)",
            "about:blank",
            "not a url",
        ] {
            assert!(!nav_allowed(bad, h), "{bad}");
        }
    }

    #[test]
    fn a_request_path_is_decoded_for_the_file_server_and_only_for_the_own_host() {
        let h = "flare-mod-notes.local";
        assert_eq!(
            request_path("https://flare-mod-notes.local/", h).as_deref(),
            Some("/")
        );
        assert_eq!(
            request_path("https://flare-mod-notes.local/a%20b/c.js?x=1", h).as_deref(),
            Some("/a b/c.js")
        );
        // An encoded dot-dot is resolved by the address parser inside the host (it cannot climb out of the
        // root), and what is left never holds a '..' part: the file server's own syntax check is the second wall.
        let p = request_path("https://flare-mod-notes.local/%2e%2e/x", h).unwrap();
        assert!(!p.split('/').any(|part| part == ".."), "{p}");
        assert_eq!(request_path("https://evil.com/a", h), None);
        assert_eq!(request_path("https://flare-mod-notes.local/%zz", h), None);
        assert_eq!(request_path("https://flare-mod-notes.local/%ff", h), None);
    }

    #[test]
    fn a_mods_requests_never_reach_the_network() {
        let h = "flare-mod-notes.local";
        assert_eq!(
            route_request("https://flare-mod-notes.local/a.js", h),
            Route::Serve("/a.js".into())
        );
        assert_eq!(route_request("data:text/plain,hi", h), Route::Pass);
        assert_eq!(
            route_request("blob:https://flare-mod-notes.local/1", h),
            Route::Pass
        );
        for bad in [
            "https://example.com/",
            "http://flare-mod-notes.local/",
            "wss://flare-mod-notes.local/",
            "ftp://x/",
            "https://flare-mod-notes.local/%zz",
            "https://flare-mod-notes.local.evil.com/",
            "garbage",
        ] {
            assert_eq!(route_request(bad, h), Route::Refuse, "{bad}");
        }
    }
}
