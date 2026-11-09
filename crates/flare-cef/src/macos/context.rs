//! Request contexts: CEF's profiles. One per kind of browser Windows starts an environment for (the web panel's on
//! disk, a mini-app's in memory, each mod's own folder), each with the walls a whole profile can hold: the proxy it
//! must use (a dead one for a sealed page, Flare's filtering proxy for a networked one), the WebRTC address policy,
//! and a request wall for the requests no browser makes (a service worker's). Windows passes the same walls as
//! browser arguments per environment; one CEF process holds every profile, so they are the profile's preferences
//! here, set when the context is initialized, before any browser of it is made.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cef::*;

use crate::macos::view::{RequestRule, context_wall};

/// What a profile is made with.
#[derive(Clone, Default)]
pub struct Profile {
    /// Its folder (under the engine's root cache), or None for one in memory that keeps nothing.
    pub cache: Option<PathBuf>,
    /// Every request through this proxy (`http://127.0.0.1:9`), with `bypass` as its bypass list (`<-loopback>`:
    /// not even this machine goes direct).
    pub proxy: Option<String>,
    pub bypass: Option<String>,
    /// WebRTC may send no UDP that does not go through a proxy (`disable_non_proxied_udp`).
    pub webrtc_sealed: bool,
    /// The wall for requests of no browser.
    pub wall: Option<RequestRule>,
    /// Where Chromium keeps a download's bytes while it is held for the person's yes (its default download
    /// folder): Flare's own staging folder, never `~/Downloads` (rule 262). None leaves Chromium's default.
    pub staging: Option<PathBuf>,
}

/// The WebRTC preference and its sealed value (Chromium's `prefs::kWebRTCIPHandlingPolicy`).
pub const WEBRTC_PREF: &str = "webrtc.ip_handling_policy";
pub const WEBRTC_SEALED: &str = "disable_non_proxied_udp";
/// Chromium's per-profile proxy preference (`proxy_config::prefs::kProxy`, a `ProxyConfigDictionary`).
pub const PROXY_PREF: &str = "proxy";
/// Chromium's default download folder and the "save as" folder (`prefs::kDownloadDefaultDirectory`,
/// `prefs::kSaveFileDefaultDirectory`): a held download's intermediate file goes in the first.
pub const DOWNLOAD_DIR_PREF: &str = "download.default_directory";
pub const SAVE_DIR_PREF: &str = "savefile.default_directory";
/// Chromium's printing switch (`prefs::kPrintingEnabled`): off, `window.print()` and Cmd+P do nothing, so the
/// native print panel is never asked for (rule 265).
pub const PRINTING_PREF: &str = "printing.enabled";

type Ready = Arc<Mutex<Option<Result<(), String>>>>;

/// A profile, made; [`Context::ready`] says when its preferences are in.
#[derive(Clone)]
pub struct Context {
    pub ctx: RequestContext,
    ready: Ready,
}

fn set(ctx: &RequestContext, name: &str, value: &mut Value) -> Result<(), String> {
    let mut err = CefString::from("");
    if ctx.set_preference(Some(&name.into()), Some(value), Some(&mut err)) == 1 {
        Ok(())
    } else {
        Err(format!("the profile refused {name}: {err}"))
    }
}

fn apply(ctx: &RequestContext, p: &Profile) -> Result<(), String> {
    let mut off = value_create().ok_or("no CEF value")?;
    off.set_bool(0);
    set(ctx, PRINTING_PREF, &mut off)?;
    if let Some(dir) = &p.staging {
        let dir = dir.to_string_lossy().into_owned();
        for name in [DOWNLOAD_DIR_PREF, SAVE_DIR_PREF] {
            let mut v = value_create().ok_or("no CEF value")?;
            v.set_string(Some(&dir.as_str().into()));
            set(ctx, name, &mut v)?;
        }
    }
    if p.webrtc_sealed {
        let mut v = value_create().ok_or("no CEF value")?;
        v.set_string(Some(&WEBRTC_SEALED.into()));
        set(ctx, WEBRTC_PREF, &mut v)?;
    }
    if let Some(server) = &p.proxy {
        let d = dictionary_value_create().ok_or("no CEF dictionary")?;
        d.set_string(Some(&"mode".into()), Some(&"fixed_servers".into()));
        d.set_string(Some(&"server".into()), Some(&server.as_str().into()));
        if let Some(b) = &p.bypass {
            d.set_string(Some(&"bypass_list".into()), Some(&b.as_str().into()));
        }
        let mut v = value_create().ok_or("no CEF value")?;
        let mut d = d;
        v.set_dictionary(Some(&mut d));
        set(ctx, PROXY_PREF, &mut v)?;
    }
    Ok(())
}

wrap_request_context_handler! {
    struct Handler {
        profile: Profile,
        ready: Ready,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            let r = match request_context {
                Some(ctx) => apply(ctx, &self.profile),
                None => Err("CEF initialized no profile".to_string()),
            };
            *self.ready.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        }

        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            self.profile.wall.clone().map(context_wall)
        }
    }
}

impl Context {
    /// Makes the profile (on the main thread, CEF started). Its preferences go in when CEF initializes it.
    pub fn create(p: Profile) -> Result<Context, String> {
        let cache = p
            .cache
            .as_ref()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default();
        let settings = RequestContextSettings {
            cache_path: cache.as_str().into(),
            persist_session_cookies: i32::from(p.cache.is_some()),
            ..Default::default()
        };
        let ready: Ready = Arc::default();
        let mut handler = Handler::new(p, ready.clone());
        let ctx = request_context_create_context(Some(&settings), Some(&mut handler))
            .ok_or("CEF made no profile")?;
        Ok(Context { ctx, ready })
    }

    /// None while CEF initializes the profile; then whether its preferences are in. A browser of the profile is
    /// made only after Ok.
    pub fn ready(&self) -> Option<Result<(), String>> {
        self.ready.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A string preference, a boolean one as `true` or `false`, or a dictionary one as `key=value` pairs sorted by
    /// key (the tests read the walls back).
    pub fn preference(&self, name: &str) -> String {
        let Some(v) = self.ctx.preference(Some(&name.into())) else {
            return String::new();
        };
        if v.get_type() == ValueType::BOOL {
            return (v.bool() != 0).to_string();
        }
        if let Some(d) = v.dictionary() {
            let mut keys = CefStringList::new();
            d.keys(Some(&mut keys));
            let mut pairs: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let val = CefString::from(&d.string(Some(&k.as_str().into()))).to_string();
                    format!("{k}={val}")
                })
                .collect();
            pairs.sort();
            return pairs.join(";");
        }
        CefString::from(&v.string()).to_string()
    }
}
