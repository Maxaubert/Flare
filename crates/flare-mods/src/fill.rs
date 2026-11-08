//! A saved login filled into a web page, and a secret asked of the person (the `fill` permission, 2026-10-28), the
//! part with no window: the params of `browser.fill` and `secret.ask`, the rule that says whether a login is for the
//! page's site, the asks a plugin may make (six a minute, none once the person said "Deny for this session"), and the
//! script that types the values into the page with the values as JSON data.
//!
//! The values are secrets. They live in [`Secret`], whose `Debug` prints nothing of them; no error, log line or
//! answer made here carries one, and the script's own answer names the kinds of field it filled, never a value.
//! Design: `docs/superpowers/specs/2026-10-28-plugin-fill.md`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use flare_ctl::{CtlError, code};
use serde_json::{Value, json};

/// How many dialogs (fills and secret asks together) one plugin may put up in [`ASK_WINDOW`].
pub const ASKS_PER_WINDOW: usize = 6;
/// The window of [`ASKS_PER_WINDOW`].
pub const ASK_WINDOW: Duration = Duration::from_secs(60);
/// How long Flare's dialog waits for the person before it counts as a Deny. Under the channel's `FILL_TIMEOUT`, so
/// the answer is Flare's.
pub const ASK_WAIT: Duration = Duration::from_secs(60);
/// The most addresses one login may name.
pub const MAX_LOGIN_URIS: usize = 32;
/// The longest address of a login.
pub const MAX_URI_CHARS: usize = 2048;
/// The longest value (a user name, a password, a code).
pub const MAX_VALUE_CHARS: usize = 1024;
/// The longest login name the dialog shows.
pub const MAX_LOGIN_NAME_CHARS: usize = 120;
/// The longest title of `secret.ask`.
pub const MAX_TITLE_CHARS: usize = 80;
/// The longest label of `secret.ask`.
pub const MAX_LABEL_CHARS: usize = 120;

/// A value that must never be shown, logged or kept: a password, a user name, a code, a typed master password. Its
/// `Debug` prints only that it is one; its memory is written over when it is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    /// The value itself, for the one place it goes (the page, or the plugin that asked).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.iter_mut().for_each(|b| *b = 0);
        // Seen as used, so the writes are not left out for a buffer that is about to be freed.
        std::hint::black_box(&bytes);
    }
}

/// What to type: each one optional, at least one given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Values {
    pub username: Option<Secret>,
    pub password: Option<Secret>,
    pub totp: Option<Secret>,
}

/// `browser.fill`'s params, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillRequest {
    /// The login's name as the dialog shows it (clean, one line).
    pub login_name: String,
    /// The login's saved addresses, as the plugin sent them.
    pub login_uris: Vec<String>,
    pub values: Values,
}

/// `secret.ask`'s params, checked. `sub` (a line under the title, such as the account) and `button` (the word on the
/// button) are empty when the plugin leaves them out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretRequest {
    pub title: String,
    pub label: String,
    pub sub: String,
    pub button: String,
}

/// The longest word `secret.ask` may put on its button.
pub const MAX_BUTTON_CHARS: usize = 24;
/// The keys `secret.ask` takes.
const SECRET_KEYS: [&str; 4] = ["title", "label", "sub", "button"];

fn bad(m: impl Into<String>) -> CtlError {
    CtlError::new(code::BAD_PARAMS, m.into())
}

/// One line of text a person reads: no control or hidden formatting character, not blank, at most `max` characters.
fn clean_line(params: &Value, key: &str, max: usize, required: bool) -> Result<String, CtlError> {
    let s = match params.get(key) {
        None | Some(Value::Null) if !required => return Ok(String::new()),
        None | Some(Value::Null) => return Err(bad(format!("{key} is required"))),
        Some(Value::String(s)) => s,
        Some(_) => return Err(bad(format!("{key} must be text"))),
    };
    if s.trim().is_empty() {
        return Err(bad(format!("{key} must not be blank")));
    }
    if s.chars().count() > max {
        return Err(bad(format!("{key} is longer than {max} characters")));
    }
    if s.chars().any(|c| c.is_control() || is_hidden_format(c)) {
        return Err(bad(format!(
            "{key} holds a control or hidden formatting character"
        )));
    }
    Ok(s.trim().to_string())
}

/// The invisible characters that could make a name read as another (zero-width, bidi overrides, the BOM).
fn is_hidden_format(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// One value: text of 1 to [`MAX_VALUE_CHARS`] characters with no control character. The message names the key and
/// never the value.
fn value(params: &Value, key: &str) -> Result<Option<Secret>, CtlError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if s.is_empty() || s.chars().count() > MAX_VALUE_CHARS {
                return Err(bad(format!("{key} is 1 to {MAX_VALUE_CHARS} characters")));
            }
            if s.chars().any(char::is_control) {
                return Err(bad(format!("{key} holds a control character")));
            }
            Ok(Some(Secret::new(s.clone())))
        }
        Some(_) => Err(bad(format!("{key} must be text"))),
    }
}

/// The keys `browser.fill` takes. Any other is refused, so a misspelt value is never dropped quietly.
const FILL_KEYS: [&str; 6] = [
    "tab",
    "login_name",
    "login_uris",
    "username",
    "password",
    "totp",
];

/// Reads `browser.fill`'s params (`tab` is the dispatcher's).
pub fn parse_fill(params: &Value) -> Result<FillRequest, CtlError> {
    let obj = params
        .as_object()
        .ok_or_else(|| bad("params must be an object"))?;
    if let Some(k) = obj.keys().find(|k| !FILL_KEYS.contains(&k.as_str())) {
        return Err(bad(format!(
            "unknown param \"{}\"; browser.fill takes {}",
            k.chars().take(40).collect::<String>(),
            FILL_KEYS.join(", ")
        )));
    }
    let login_name = clean_line(params, "login_name", MAX_LOGIN_NAME_CHARS, true)?;
    let uris = match params.get("login_uris") {
        Some(Value::Array(a)) => a,
        _ => {
            return Err(bad(
                "login_uris is required: a list of the login's addresses",
            ));
        }
    };
    if uris.is_empty() || uris.len() > MAX_LOGIN_URIS {
        return Err(bad(format!(
            "login_uris holds 1 to {MAX_LOGIN_URIS} addresses"
        )));
    }
    let mut login_uris = Vec::new();
    for u in uris {
        match u.as_str() {
            Some(s) if !s.trim().is_empty() && s.chars().count() <= MAX_URI_CHARS => {
                login_uris.push(s.trim().to_string());
            }
            _ => {
                return Err(bad(format!(
                    "each of login_uris is an address of 1 to {MAX_URI_CHARS} characters"
                )));
            }
        }
    }
    let values = Values {
        username: value(params, "username")?,
        password: value(params, "password")?,
        totp: value(params, "totp")?,
    };
    if values.username.is_none() && values.password.is_none() && values.totp.is_none() {
        return Err(bad("give at least one of username, password or totp"));
    }
    Ok(FillRequest {
        login_name,
        login_uris,
        values,
    })
}

/// Reads `secret.ask`'s params.
pub fn parse_secret(params: &Value) -> Result<SecretRequest, CtlError> {
    let obj = params
        .as_object()
        .ok_or_else(|| bad("params must be an object"))?;
    if let Some(k) = obj.keys().find(|k| !SECRET_KEYS.contains(&k.as_str())) {
        return Err(bad(format!(
            "unknown param \"{}\"; secret.ask takes title, label, sub and button",
            k.chars().take(40).collect::<String>()
        )));
    }
    Ok(SecretRequest {
        title: clean_line(params, "title", MAX_TITLE_CHARS, true)?,
        label: clean_line(params, "label", MAX_LABEL_CHARS, true)?,
        sub: clean_line(params, "sub", MAX_LABEL_CHARS, false)?,
        button: clean_line(params, "button", MAX_BUTTON_CHARS, false)?,
    })
}

/// The site of a page Flare may fill: an `http` or `https` address with a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// `https` or `http`.
    pub scheme: String,
    /// The host as the browser names it (lower case; an IPv6 address in brackets).
    pub host: String,
    pub port: Option<u16>,
}

/// The site of the page's address, or None for an address Flare never fills (a file, `about:`, a mini-app, no host).
pub fn page_site(page_url: &str) -> Option<Site> {
    let u = url::Url::parse(page_url).ok()?;
    if !matches!(u.scheme(), "https" | "http") {
        return None;
    }
    let host = u.host_str()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(Site {
        scheme: u.scheme().to_string(),
        host,
        port: u.port(),
    })
}

/// A login's saved address as a site. A bare `example.com` (as password managers often keep it) is read as
/// `https://example.com`; an address of another kind (`androidapp://`, `file:`) is no site.
fn login_site(uri: &str) -> Option<Site> {
    // `example.com:8443` would parse with the scheme `example.com`: an address with no `://` is a bare host.
    if uri.contains("://") {
        page_site(uri)
    } else {
        page_site(&format!("https://{uri}"))
    }
}

/// Whether the page's site is a site of the login (Bitwarden's default rule, without the public suffix list): the
/// login's host or a subdomain of it, a leading `www.` of the login's host left out; `https` unless the login's
/// address says `http` (an `http` login may fill its `https` page, never the other way round); and the login's port
/// when it names one. An IP address matches only itself.
pub fn site_matches(page: &Site, login: &Site) -> bool {
    let scheme_ok = page.scheme == "https" || (page.scheme == "http" && login.scheme == "http");
    if !scheme_ok {
        return false;
    }
    if login.port.is_some() && login.port != page.port {
        return false;
    }
    let ip = |h: &str| h.starts_with('[') || h.parse::<std::net::Ipv4Addr>().is_ok();
    if ip(&login.host) || ip(&page.host) {
        return page.host == login.host;
    }
    let base = login.host.strip_prefix("www.").unwrap_or(&login.host);
    // A login for a bare top-level name (`com`) matches nothing but itself.
    if !base.contains('.') {
        return page.host == login.host;
    }
    page.host == login.host
        || page.host == base
        || page
            .host
            .strip_suffix(base)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// The page's site when one of the login's addresses is for it, else `forbidden` with "this login is not for
/// <host>". A page Flare never fills is `forbidden` too.
pub fn check_login_for(page_url: &str, login_uris: &[String]) -> Result<Site, CtlError> {
    let Some(page) = page_site(page_url) else {
        return Err(CtlError::new(
            code::FORBIDDEN,
            "Flare fills only an http or https web page",
        ));
    };
    if login_uris
        .iter()
        .filter_map(|u| login_site(u))
        .any(|l| site_matches(&page, &l))
    {
        Ok(page)
    } else {
        Err(CtlError::new(
            code::FORBIDDEN,
            format!("this login is not for {}", page.host),
        ))
    }
}

/// The asks each plugin made in the last minute, and the plugins the person told "Deny for this session" (until
/// Flare quits). Held by the hub, so the call threads and the window share it.
#[derive(Debug, Default)]
pub struct Asks {
    times: HashMap<String, VecDeque<Instant>>,
    denied: HashSet<String>,
}

impl Asks {
    /// One more ask of `plugin` at `now`, or why not (`rate_limited`, or the session's Deny).
    pub fn admit(&mut self, plugin: &str, now: Instant) -> Result<(), CtlError> {
        let q = self.times.entry(plugin.to_string()).or_default();
        while q
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= ASK_WINDOW)
        {
            q.pop_front();
        }
        if q.len() >= ASKS_PER_WINDOW {
            return Err(CtlError::new(
                code::RATE_LIMITED,
                format!("at most {ASKS_PER_WINDOW} asks a minute; wait before asking again"),
            ));
        }
        q.push_back(now);
        Ok(())
    }

    /// The person chose "Deny for this session" on one of the plugin's fills.
    pub fn deny_for_session(&mut self, plugin: &str) {
        self.denied.insert(plugin.to_string());
    }

    /// Whether the plugin's fills are denied until Flare quits.
    pub fn denied(&self, plugin: &str) -> bool {
        self.denied.contains(plugin)
    }
}

/// The kinds of field a fill types into, in the order the answer names them.
pub const FIELDS: [&str; 3] = ["username", "password", "totp"];

/// A text as a JS string literal: `serde_json`'s string, with the line and paragraph separators escaped as well
/// (a string literal may hold them since ES2019).
fn js_string(s: &str) -> String {
    serde_json::to_string(s)
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// The script that types the values into the page. It runs in the page's own JS world, where the page may have
/// replaced any function (`JSON.parse`, `String.prototype.toLowerCase`), and the tab may have moved to another site
/// since Flare last looked (the agent can navigate it while the person reads the dialog). So it first checks, with
/// nothing but `!==` on the `location` a page cannot redefine, that the document is still on `site` (scheme, host
/// and port), and stops there if not, before anything of the values is touched. Only then are the values read: one
/// JSON text, itself encoded as a JS string literal and read with `JSON.parse`, so nothing a value holds can become
/// code. It finds the fields (the `autocomplete` tokens first: `username`, `email`, `current-password`,
/// `one-time-code`; then the visible text and password fields), types each value as the native setter does and fires
/// `input` and `change`. It answers `{ok, filled, missing}` (kinds of field only) or `{ok: false, why}`. The script
/// holds the values, so it is a [`Secret`] too.
pub fn fill_script(values: &Values, site: &Site) -> Secret {
    let data = Secret::new(
        json!({
            "u": values.username.as_ref().map(Secret::expose),
            "p": values.password.as_ref().map(Secret::expose),
            "t": values.totp.as_ref().map(Secret::expose),
        })
        .to_string(),
    );
    let literal = Secret::new(js_string(data.expose()));
    // `location.port` is empty for the scheme's own port, as `Site::port` is None.
    let port = site.port.map_or_else(String::new, |p| p.to_string());
    Secret::new(format!(
        "(function () {{ if (location.protocol !== {} || location.hostname !== {} || location.port !== {}) \
         {{ return {{ ok: false, why: 'moved' }}; }} return (function (d) {{ {FILL_JS} }})(JSON.parse({})); }})()",
        js_string(&format!("{}:", site.scheme)),
        js_string(&site.host),
        js_string(&port),
        literal.expose()
    ))
}

/// The body of [`fill_script`], run once the page's site is checked: `d` is `{u, p, t}`.
const FILL_JS: &str = r#"
try {
  var vis = function (e) {
    if (e.disabled || e.readOnly || (e.type || '').toLowerCase() === 'hidden') { return false; }
    if (!e.getClientRects().length) { return false; }
    var s = getComputedStyle(e);
    return s.visibility !== 'hidden' && s.display !== 'none' && parseFloat(s.opacity || '1') > 0;
  };
  var all = Array.prototype.slice.call(document.querySelectorAll('input')).filter(vis);
  var tokens = function (e) { return (e.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/); };
  var has = function (e, t) { return tokens(e).indexOf(t) >= 0; };
  var typeOf = function (e) { return (e.getAttribute('type') || 'text').toLowerCase(); };
  var texty = function (e) { var t = typeOf(e); return t === 'text' || t === 'email' || t === 'tel' || t === ''; };
  var words = function (e) {
    return [e.name, e.id, e.getAttribute('placeholder'), e.getAttribute('aria-label')].join(' ');
  };
  var first = function (f) { for (var i = 0; i < all.length; i++) { if (f(all[i])) { return all[i]; } } return null; };
  var otp = /one.?time|otp|totp|2fa|mfa|two.?factor|verification|auth.?code|security.?code/i;
  var pw = first(function (e) { return has(e, 'current-password'); })
    || first(function (e) { return typeOf(e) === 'password' && !has(e, 'new-password'); })
    || first(function (e) { return typeOf(e) === 'password'; });
  var code = first(function (e) { return has(e, 'one-time-code'); })
    || first(function (e) { return (texty(e) || typeOf(e) === 'number') && otp.test(words(e)); });
  var user = first(function (e) { return texty(e) && (has(e, 'username') || has(e, 'email')); });
  if (!user) {
    var f = document.activeElement;
    if (f && f.tagName === 'INPUT' && texty(f) && vis(f) && f !== code) { user = f; }
  }
  if (!user && pw) {
    var before = all.filter(function (e) {
      return texty(e) && e !== code && (e.compareDocumentPosition(pw) & Node.DOCUMENT_POSITION_FOLLOWING);
    });
    user = before.length ? before[before.length - 1] : null;
  }
  if (!user) {
    user = first(function (e) {
      return texty(e) && e !== code && (typeOf(e) === 'email' || /user|email|login|account|identifier/i.test(words(e)));
    });
  }
  if (user && code && user === code) { if (d.t != null) { user = null; } else { code = null; } }
  var put = function (e, v) {
    e.focus();
    var desc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
    if (desc && desc.set) { desc.set.call(e, v); } else { e.value = v; }
    e.dispatchEvent(new Event('input', { bubbles: true }));
    e.dispatchEvent(new Event('change', { bubbles: true }));
  };
  var filled = [], missing = [];
  [['username', d.u, user], ['password', d.p, pw], ['totp', d.t, code]].forEach(function (x) {
    if (x[1] == null) { return; }
    if (x[2]) { put(x[2], x[1]); filled.push(x[0]); } else { missing.push(x[0]); }
  });
  d = null;
  return { ok: true, filled: filled, missing: missing };
} catch (e) {
  d = null;
  return { ok: false, why: 'error' };
}
"#;

/// What a fill did: the kinds of field it typed into and the ones it found no field for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filled {
    pub filled: Vec<&'static str>,
    pub missing: Vec<&'static str>,
}

/// Reads the script's answer (ExecuteScript's JSON). Only the known kinds of field are kept, so nothing else the
/// page could have put there reaches the plugin.
pub fn read_fill_answer(json_text: &str) -> Result<Filled, CtlError> {
    let v: Value = serde_json::from_str(json_text)
        .map_err(|_| CtlError::new(code::INTERNAL, "the page's answer was not read"))?;
    if v["ok"] != Value::Bool(true) {
        return Err(match v["why"].as_str() {
            Some("moved") => CtlError::new(
                code::INTERRUPTED,
                "the page moved to another address before it was filled",
            ),
            _ => CtlError::new(code::UNAVAILABLE, "the page could not be filled"),
        });
    }
    let kinds = |key: &str| -> Vec<&'static str> {
        FIELDS
            .into_iter()
            .filter(|f| {
                v[key]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(f)))
            })
            .collect()
    };
    Ok(Filled {
        filled: kinds("filled"),
        missing: kinds("missing"),
    })
}

/// The one log line of a fill: the plugin, the page's host and the outcome, never a value or a login's name.
pub fn log_line(plugin: &str, host: &str, outcome: &str) -> String {
    format!("fill: plugin {plugin} on {host}: {outcome}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(url: &str) -> Site {
        page_site(url).unwrap()
    }

    fn matches_login(page: &str, login: &str) -> bool {
        check_login_for(page, &[login.to_string()]).is_ok()
    }

    #[test]
    fn the_login_host_and_its_subdomains_match_and_nothing_else() {
        for (page, login) in [
            ("https://example.com/login", "https://example.com"),
            ("https://example.com/login", "example.com"),
            ("https://accounts.example.com/x", "https://example.com/"),
            ("https://a.b.example.com/", "example.com"),
            ("https://EXAMPLE.com/", "https://example.COM/path?q=1"),
            ("https://example.com/", "https://www.example.com/login"),
            ("https://login.example.com/", "https://www.example.com/"),
            ("https://example.com:443/", "https://example.com"),
            ("https://example.com/", "example.com/sign-in"),
        ] {
            assert!(matches_login(page, login), "{page} should match {login}");
        }
        for (page, login) in [
            // Another site, or one that only looks like it.
            ("https://example.org/", "https://example.com"),
            ("https://notexample.com/", "https://example.com"),
            ("https://example.com.evil.net/", "https://example.com"),
            ("https://evil.net/example.com", "https://example.com"),
            ("https://example.com/", "https://accounts.example.com"),
            // A login for a subdomain does not fill its sibling.
            ("https://mail.example.com/", "https://accounts.example.com"),
            // A top-level name alone matches nothing else.
            ("https://example.com/", "https://com"),
            // An address that is not a web page.
            ("https://example.com/", "androidapp://com.example"),
            ("https://example.com/", ""),
        ] {
            assert!(!matches_login(page, login), "{page} must not match {login}");
        }
    }

    #[test]
    fn https_is_required_unless_the_login_says_http() {
        assert!(!matches_login("http://example.com/", "https://example.com"));
        assert!(!matches_login("http://example.com/", "example.com"));
        assert!(matches_login("http://example.com/", "http://example.com"));
        // An http login may fill its https page; never the other way round.
        assert!(matches_login("https://example.com/", "http://example.com"));
        for page in [
            "file:///C:/x.html",
            "about:blank",
            "data:text/html,x",
            "ftp://example.com/",
        ] {
            let e = check_login_for(page, &["example.com".into()]).unwrap_err();
            assert_eq!(e.code, code::FORBIDDEN, "{page}");
        }
    }

    #[test]
    fn a_port_in_the_login_must_be_the_pages_and_ip_addresses_match_only_themselves() {
        assert!(matches_login(
            "https://example.com:8443/",
            "https://example.com:8443"
        ));
        assert!(!matches_login(
            "https://example.com/",
            "https://example.com:8443"
        ));
        assert!(matches_login(
            "https://example.com:8443/",
            "https://example.com"
        ));
        assert!(matches_login(
            "http://127.0.0.1:8123/",
            "http://127.0.0.1:8123/"
        ));
        assert!(!matches_login(
            "http://127.0.0.2:8123/",
            "http://127.0.0.1:8123/"
        ));
        assert!(!matches_login("http://1.127.0.0.1/", "http://127.0.0.1/"));
        assert!(matches_login("https://[::1]/", "https://[::1]/"));
    }

    #[test]
    fn a_mismatch_says_the_pages_host_and_one_matching_address_is_enough() {
        let e = check_login_for(
            "https://phish.example.net/login",
            &["https://example.com".into()],
        )
        .unwrap_err();
        assert_eq!(e.code, code::FORBIDDEN);
        assert_eq!(e.message, "this login is not for phish.example.net");
        let ok = check_login_for(
            "https://id.example.com/",
            &[
                "androidapp://x".into(),
                "https://other.org".into(),
                "example.com".into(),
            ],
        )
        .unwrap();
        assert_eq!(ok, site("https://id.example.com/"));
    }

    #[test]
    fn six_asks_a_minute_then_rate_limited_and_the_window_slides() {
        let mut a = Asks::default();
        let t0 = Instant::now();
        for i in 0..ASKS_PER_WINDOW {
            a.admit("bw", t0 + Duration::from_secs(i as u64)).unwrap();
        }
        let e = a.admit("bw", t0 + Duration::from_secs(10)).unwrap_err();
        assert_eq!(e.code, code::RATE_LIMITED);
        // Another plugin has its own count.
        a.admit("other", t0 + Duration::from_secs(10)).unwrap();
        // A minute after the first ask, one more fits; the refused one did not count.
        a.admit("bw", t0 + Duration::from_secs(60)).unwrap();
        assert!(a.admit("bw", t0 + Duration::from_secs(60)).is_err());
        a.admit("bw", t0 + Duration::from_secs(62)).unwrap();
    }

    #[test]
    fn deny_for_this_session_holds_for_that_plugin_only() {
        let mut a = Asks::default();
        assert!(!a.denied("bw"));
        a.deny_for_session("bw");
        assert!(a.denied("bw") && !a.denied("other"));
    }

    fn fill_params(extra: Value) -> Value {
        let mut v = json!({"tab": "t1", "login_name": "Example", "login_uris": ["https://example.com"], "password": "hunter2"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn fill_params_are_checked_and_a_refusal_never_holds_a_value() {
        let r = parse_fill(&fill_params(
            json!({"username": "max@example.com", "totp": "123456"}),
        ))
        .unwrap();
        assert_eq!(r.login_name, "Example");
        assert_eq!(
            r.values.password.as_ref().map(Secret::expose),
            Some("hunter2")
        );
        assert_eq!(r.values.totp.as_ref().map(Secret::expose), Some("123456"));
        for (extra, says) in [
            (json!({"login_name": null}), "login_name is required"),
            (json!({"login_name": "two\nlines"}), "control"),
            (json!({"login_name": "Bank\u{202E}"}), "hidden formatting"),
            (json!({"login_uris": []}), "1 to 32"),
            (json!({"login_uris": "example.com"}), "list"),
            (json!({"login_uris": [5]}), "each of login_uris"),
            (json!({"password": null}), "at least one"),
            (json!({"password": 5}), "password must be text"),
            (
                json!({"password": "SECRET-VALUE\u{0007}"}),
                "password holds a control",
            ),
            (
                json!({"password": "x".repeat(MAX_VALUE_CHARS + 1)}),
                "password is 1 to",
            ),
            (
                json!({"pasword": "SECRET-VALUE"}),
                "unknown param \"pasword\"",
            ),
        ] {
            let e = parse_fill(&fill_params(extra.clone())).unwrap_err();
            assert_eq!(e.code, code::BAD_PARAMS, "{extra}");
            assert!(e.message.contains(says), "{extra}: {}", e.message);
            assert!(!e.message.contains("SECRET-VALUE") && !e.message.contains("hunter2"));
        }
    }

    #[test]
    fn secret_params_are_one_clean_line_each() {
        let r = parse_secret(&json!({"title": "Unlock Bitwarden", "label": "Master password"}))
            .unwrap();
        assert_eq!(r.title, "Unlock Bitwarden");
        assert!(parse_secret(&json!({"title": "x"})).is_err());
        assert!(parse_secret(&json!({"title": "x", "label": "y", "default": "z"})).is_err());
        assert!(parse_secret(&json!({"title": "x\u{200B}", "label": "y"})).is_err());
        // `sub` and `button` are optional clean lines; the button is short.
        assert_eq!((r.sub.as_str(), r.button.as_str()), ("", ""));
        let r = parse_secret(&json!({"title": "Your vault is locked", "label": "Master password", "sub": "me@example.com", "button": "Unlock"}))
            .unwrap();
        assert_eq!(
            (r.sub.as_str(), r.button.as_str()),
            ("me@example.com", "Unlock")
        );
        let long = "b".repeat(MAX_BUTTON_CHARS + 1);
        assert!(parse_secret(&json!({"title": "x", "label": "y", "button": long})).is_err());
        assert!(parse_secret(&json!({"title": "x", "label": "y", "sub": "a\nb"})).is_err());
        assert!(parse_secret(&json!({"title": "x", "label": "y", "button": 3})).is_err());
    }

    #[test]
    fn a_secret_prints_nothing_of_itself() {
        let s = Secret::new("hunter2".into());
        assert_eq!(format!("{s:?}"), "Secret(..)");
        let v = Values {
            password: Some(s),
            ..Values::default()
        };
        assert!(!format!("{v:?}").contains("hunter2"));
        let r = parse_fill(&fill_params(json!({}))).unwrap();
        assert!(!format!("{r:?}").contains("hunter2"));
    }

    /// The values reach the script as data only: the script text holds them inside one JSON string literal that
    /// `JSON.parse` reads, so a value made to break out of a string (quotes, a backslash, a closing script tag, a
    /// line separator) stays a value.
    #[test]
    fn the_script_carries_the_values_as_one_json_string_and_never_as_code() {
        let nasty = "a\"); alert(1); (\"\\'`${x}</script>\u{2028}\u{2029}";
        let values = Values {
            username: Some(Secret::new("max".into())),
            password: Some(Secret::new(nasty.into())),
            totp: None,
        };
        let script = fill_script(&values, &site("https://example.com/"));
        assert_eq!(format!("{script:?}"), "Secret(..)");
        let js = script.expose();
        assert!(js.starts_with("(function () {") && js.contains("(function (d) {"));
        let tail = js
            .rsplit_once("(JSON.parse(")
            .map(|(_, t)| t)
            .expect("the data is parsed from a string");
        let literal = tail
            .strip_suffix(")); })()")
            .expect("the call ends the script");
        // The literal is one JSON string; read back, it is the JSON object with the values exactly as given.
        // A JS string literal read as JSON: the separators escaped as ` ` read back the same.
        let text: String = serde_json::from_str(literal).unwrap();
        let data: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(data["p"], nasty);
        assert_eq!(data["u"], "max");
        assert_eq!(data["t"], Value::Null);
        assert_eq!(data.as_object().map(|o| o.len()), Some(3));
        // Nothing of the value appears outside the literal, and the literal has no raw line separator.
        let code_part = js.replace(literal, "");
        assert!(!code_part.contains("alert") && !code_part.contains("max"));
        assert!(!literal.contains('\u{2028}') && !literal.contains('\u{2029}'));
    }

    /// The page may have moved to another site, whose own script replaced `JSON.parse` or `toLowerCase` to catch
    /// the values: the script checks the site first, with `!==` on `location` alone, and reads the values only after.
    #[test]
    fn the_script_checks_the_site_before_it_reads_the_values() {
        let values = Values {
            password: Some(Secret::new("hunter2".into())),
            ..Values::default()
        };
        let script = fill_script(&values, &site("https://id.example.com:8443/login"));
        let js = script.expose();
        let check = "if (location.protocol !== \"https:\" || location.hostname !== \"id.example.com\" \
                     || location.port !== \"8443\") { return { ok: false, why: 'moved' }; }";
        let at = js.find(check).expect("the site check");
        let parse = js.find("JSON.parse(").expect("the values are read");
        assert!(at < parse, "the check comes before the values are read");
        assert!(js[..at].find("hunter2").is_none());
        // Nothing the page could replace is called before the check.
        assert!(!js[..at + check.len()].contains("toLowerCase"));
        // The scheme's own port is the empty `location.port`.
        let js = fill_script(&values, &site("https://example.com:443/"));
        assert!(js.expose().contains("location.port !== \"\")"));
    }

    #[test]
    fn the_script_finds_fields_by_autocomplete_first_and_fires_input_and_change() {
        let js = fill_script(
            &Values {
                password: Some(Secret::new("p".into())),
                ..Values::default()
            },
            &site("https://example.com/"),
        );
        let js = js.expose();
        for uses in [
            "'current-password'",
            "'one-time-code'",
            "'username'",
            "'email'",
            "new Event('input', { bubbles: true })",
            "new Event('change', { bubbles: true })",
            "HTMLInputElement.prototype, 'value'",
            "why: 'moved'",
        ] {
            assert!(js.contains(uses), "the script uses {uses}");
        }
        assert!(js.find("'current-password'") < js.find("typeOf(e) === 'password'"));
    }

    #[test]
    fn the_answer_keeps_only_the_kinds_of_field() {
        let f = read_fill_answer(
            r#"{"ok":true,"filled":["username","password","hunter2"],"missing":["totp","x"]}"#,
        )
        .unwrap();
        assert_eq!(f.filled, ["username", "password"]);
        assert_eq!(f.missing, ["totp"]);
        let e = read_fill_answer(r#"{"ok":false,"why":"moved"}"#).unwrap_err();
        assert_eq!(e.code, code::INTERRUPTED);
        let e = read_fill_answer("null").unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(read_fill_answer("not json").is_err());
    }

    #[test]
    fn the_log_line_has_the_host_and_the_outcome_only() {
        let l = log_line("bitwarden", "example.com", "filled username, password");
        assert_eq!(
            l,
            "fill: plugin bitwarden on example.com: filled username, password"
        );
    }
}
