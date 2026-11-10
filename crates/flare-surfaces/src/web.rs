//! The web panel's logic with no COM and no Slint (Surfaces stage 3): which addresses may be
//! opened, whether the page is live, a still or hidden, where the page sits in physical pixels and
//! where its profile lives. The WebView2 glue is `flare-web`; the window code is in `flare`.
//!
//! Rules it holds: S4 (a web panel never uses the user's own browser profile), 13 and 14 in spirit
//! (an address from a caller, a link in a page or a stored panel is vetted the same way: http,
//! https and `about:blank` only; `file:`, `javascript:`, `data:` and custom schemes are refused). A page's own link to an
//! app is the one exception: `external_scheme` says whether the person may be asked about it (rule 252), and `own_blob`
//! lets a page open a `blob:` it made itself (rule 253).

use std::path::{Path, PathBuf};

use crate::layout::{self, SlotLayout};
use crate::state::TARGET_MAX;

/// Why an address cannot be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    TooLong,
    /// A control character or a space in it.
    BadChars,
    /// A scheme other than http, https or `about:blank`.
    Scheme,
    /// No host, a login in the address, or it does not parse.
    Invalid,
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UrlError::Empty => "the address is empty",
            UrlError::TooLong => "the address is too long",
            UrlError::BadChars => "the address has a space or a control character",
            UrlError::Scheme => "only http, https and about:blank addresses open here",
            UrlError::Invalid => "that is not a web address",
        })
    }
}

/// The scheme `s` starts with, when it really has one (letters, digits, `+`, `-`, `.` before a
/// colon) and what follows the colon is not just a port (`localhost:3000` has no scheme).
fn split_scheme(s: &str) -> Option<&str> {
    let colon = s.find(':')?;
    let scheme = &s[..colon];
    let mut chars = scheme.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    if !first_ok
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        || scheme.len() > 32
    {
        return None;
    }
    // `host:3000` and `host:3000/path`: digits up to a slash, `?`, `#` or the end are a port.
    let rest = &s[colon + 1..];
    let port_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let port = &rest[..port_end];
    if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(scheme)
}

fn is_local_host(s: &str) -> bool {
    let host_end = s.find(['/', '?', '#']).unwrap_or(s.len());
    let hostport = &s[..host_end];
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        // `[::1]:3000`
        rest.split(']').next().unwrap_or("")
    } else {
        hostport.rsplit_once(':').map_or(hostport, |(h, _)| h)
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

/// What the address field and a caller's `url` become: the address a page may be opened at.
///
/// - Text with no scheme gets `https://` (a local host, `localhost` or `127.0.0.1`, gets `http://`:
///   a dev server has no certificate).
/// - Only `http`, `https` and exactly `about:blank` pass; every other scheme is [`UrlError::Scheme`].
/// - A host is required and a login (`user:pass@`) is refused: it is how an address is made to read
///   as another site.
/// - No space or control character, at most 2048 characters.
///
/// The answer is the parsed form (lower-case host, punycode for a name with accents).
pub fn normalize_url(input: &str) -> Result<String, UrlError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(UrlError::Empty);
    }
    if s.chars().count() > TARGET_MAX {
        return Err(UrlError::TooLong);
    }
    if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(UrlError::BadChars);
    }
    let full = match split_scheme(s) {
        Some(scheme) => {
            let lower = scheme.to_ascii_lowercase();
            if lower == "about" {
                return if s.eq_ignore_ascii_case("about:blank") {
                    Ok("about:blank".to_string())
                } else {
                    Err(UrlError::Scheme)
                };
            }
            if lower != "http" && lower != "https" {
                return Err(UrlError::Scheme);
            }
            s.to_string()
        }
        None => {
            // A leading `//` is a scheme-relative address; treat it as plain text after the slashes.
            let bare = s.trim_start_matches('/');
            if is_local_host(bare) {
                format!("http://{bare}")
            } else {
                format!("https://{bare}")
            }
        }
    };
    let url = url::Url::parse(&full).map_err(|_| UrlError::Invalid)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(UrlError::Invalid);
    }
    let out = url.as_str().to_string();
    if out.chars().count() > TARGET_MAX {
        return Err(UrlError::TooLong);
    }
    Ok(out)
}

/// Where a search typed in the address field goes: Google, with the words as the query (2026-10-28).
pub const SEARCH_URL: &str = "https://www.google.com/search?q=";

/// Schemes that are refused with a reason rather than searched for: typed on purpose, they are an address
/// that cannot open here, and the words are not sent to a search engine.
const REFUSED_SCHEMES: &[&str] = &[
    "file",
    "javascript",
    "data",
    "vbscript",
    "blob",
    "ftp",
    "ws",
    "wss",
    "chrome",
    "edge",
    "view-source",
];

/// What the person's address field opens (2026-10-28): an address as [`normalize_url`] makes it, or a
/// Google search for anything that is not one. Only the address field uses it; a caller's `url` and a
/// stored panel stay with [`normalize_url`], which never turns text into a search.
///
/// - An address: a scheme of http, https or `about:blank`; or, with no space, a host with a dot
///   (`youtube.com`, `a.b/path`), `localhost`, an IP (`127.0.0.1`, `[::1]`) or any `host:port`, with a
///   path after it or not.
/// - A scheme that is refused (`file:`, `javascript:`, any `x://`) is still the error [`normalize_url`]
///   gives; the text is not searched for.
/// - Everything else (`youtube`, `cat videos`, `what is rust?`) is a search. Text with no scheme that
///   looked like an address but does not parse (`me@example.com`) is a search too.
pub fn address_or_search(input: &str) -> Result<String, UrlError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(UrlError::Empty);
    }
    if s.chars().count() > TARGET_MAX {
        return Err(UrlError::TooLong);
    }
    if s.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return Err(UrlError::BadChars);
    }
    if !s.chars().any(char::is_whitespace) {
        if let Some(scheme) = split_scheme(s) {
            let lower = scheme.to_ascii_lowercase();
            let rest = &s[scheme.len() + 1..];
            if matches!(lower.as_str(), "http" | "https" | "about")
                || REFUSED_SCHEMES.contains(&lower.as_str())
                || rest.starts_with("//")
            {
                return normalize_url(s);
            }
        } else if looks_like_host(s)
            && let Ok(url) = normalize_url(s)
        {
            return Ok(url);
        }
    }
    let query: String = url::form_urlencoded::byte_serialize(s.as_bytes()).collect();
    let out = format!("{SEARCH_URL}{query}");
    if out.chars().count() > TARGET_MAX {
        return Err(UrlError::TooLong);
    }
    Ok(out)
}

/// True when text with no scheme and no space names a host: a dot between two parts with a last part of
/// letters (`youtube.com`, `bücher.de`, `a.b`), `localhost`, an IP literal, or anything with a port.
fn looks_like_host(s: &str) -> bool {
    let bare = s.trim_start_matches('/');
    let hostport = &bare[..bare.find(['/', '?', '#']).unwrap_or(bare.len())];
    if let Some(rest) = hostport.strip_prefix('[') {
        // `[::1]` and `[::1]:80`.
        return rest.split_once(']').is_some_and(|(ip, after)| {
            ip.parse::<std::net::Ipv6Addr>().is_ok() && port_after(after)
        });
    }
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (hostport, None),
    };
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::Ipv4Addr>().is_ok() {
        return port.is_none_or(is_port);
    }
    if let Some(p) = port {
        // `myserver:8080`: a port makes a host name an address even with no dot. The name needs a letter:
        // `10:30` and `3:16` are a time and a verse, not the IP `0.0.0.10` (2026-10-28 review).
        return is_port(p)
            && host.split('.').all(|l| !l.is_empty())
            && host
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.'))
            && host.chars().any(char::is_alphabetic);
    }
    let last = host.rsplit('.').next().unwrap_or("");
    // A last part of letters (`com`, `de`), or punycode for one (`xn--p1ai`, the Russian `.рф`).
    let tld = last.chars().all(|c| c.is_alphabetic() || c == '-')
        || (last.len() > 4
            && last
                .get(..4)
                .is_some_and(|p| p.eq_ignore_ascii_case("xn--"))
            && last.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    host.contains('.')
        && host.split('.').all(|l| !l.is_empty())
        && tld
        && last.chars().any(char::is_alphabetic)
}

fn is_port(p: &str) -> bool {
    !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit())
}

/// After an IPv6 literal's `]`: nothing, or a colon and a port.
fn port_after(after: &str) -> bool {
    after.is_empty() || after.strip_prefix(':').is_some_and(is_port)
}

/// The note at the end of the address field when the page's own latest navigation failed (2026-10-28):
/// "Could not open <host>" for the browser's failures to reach a site (`status` is WebView2's
/// `COREWEBVIEW2_WEB_ERROR_STATUS`). None for the rest: a navigation stopped or replaced (14), a stream
/// handed to the player or a download (9), a page that answered with an error of its own (0, the page
/// shows it) and a login asked for (17, 18).
pub fn failed_load_notice(url: &str, status: &str) -> Option<String> {
    let reached_nothing = matches!(status.trim().parse::<i32>(), Ok(1..=8 | 10..=13 | 15 | 16));
    if !reached_nothing || url.is_empty() || url.eq_ignore_ascii_case("about:blank") {
        return None;
    }
    let host: String = host_label(url).chars().take(60).collect();
    Some(format!("Could not open {host}"))
}

/// True when a navigation the page itself starts (a link, a redirect, a script) may go ahead. The
/// address is already a full one; the same schemes [`normalize_url`] lets through.
pub fn navigation_allowed(uri: &str) -> bool {
    let lower = uri.trim().to_ascii_lowercase();
    lower == "about:blank" || lower.starts_with("https://") || lower.starts_with("http://")
}

/// The longest app link the person is asked about (rule 252); a longer one is refused.
pub const APP_LINK_MAX: usize = 2048;

/// Schemes a page's link never opens, whatever is installed (rule 252): the browser's own (a script, inline data, a file,
/// a browser page) and Windows handlers that have run code or fetched files from a page. A scheme here is refused in
/// every form, so an Office scheme's `ofe|u|` and remote `|u|https://` commands go with it. Kept as data so a review
/// can read it and a test holds it.
pub const NEVER_SCHEMES: &[&str] = &[
    "javascript",
    "data",
    "file",
    "vbscript",
    "about",
    "blob",
    "filesystem",
    "view-source",
    "chrome",
    "chrome-extension",
    "edge",
    "devtools",
    "ftp",
    "ws",
    "wss",
    "ms-msdt",
    "ms-officecmd",
    "ms-appinstaller",
    "search-ms",
    "search",
    "ms-search",
    "ms-cxh",
    "ms-cxh-full",
    "shell",
    "mhtml",
    "ie.http",
    "res",
    "hcp",
    "its",
    "ms-its",
    "mk",
    "ms-excel",
    "ms-word",
    "ms-powerpoint",
    "ms-visio",
    "ms-access",
    "ms-project",
    "ms-publisher",
    "ms-infopath",
    "ms-spd",
];

/// What a page's navigation to an address the panel does not open is (rule 252).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalScheme {
    /// A link to an app (`bankid:`, `swish:`, `mailto:`): the person may be asked whether to open it. The scheme, in
    /// lower case.
    Offer(String),
    /// Never opened and never asked about: a scheme of [`NEVER_SCHEMES`], or text that is too long, holds a control
    /// character or a space, looks like a path, or has no proper scheme.
    Never,
    /// A web address (http, https, about:blank): the panel's own navigation, not an app's.
    NotApp,
}

/// Decides a page's navigation to `uri` for an app (rule 252). [`navigation_allowed`] is unchanged and still the only
/// rule for the address field and an agent's navigate: this is the page's own links only. Whether anything on the PC
/// opens the scheme is the caller's question (the registry), asked after this.
pub fn external_scheme(uri: &str) -> ExternalScheme {
    if navigation_allowed(uri) {
        return ExternalScheme::NotApp;
    }
    if uri.is_empty()
        || uri.chars().count() > APP_LINK_MAX
        || uri.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return ExternalScheme::Never;
    }
    let Some((scheme, rest)) = uri.split_once(':') else {
        return ExternalScheme::Never;
    };
    let scheme = scheme.to_ascii_lowercase();
    let proper = scheme.len() >= 2
        && scheme.len() <= 32
        && scheme.starts_with(|c: char| c.is_ascii_lowercase())
        && scheme
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'));
    // A path: a drive (`C:\x`, `C:/x`, one letter before the colon), a share (`\\host`), a slash or dot first, or a
    // backslash anywhere in what follows the scheme.
    let path_like = uri.starts_with(['\\', '/', '.', '~'])
        || scheme.len() == 1
        || rest.starts_with('\\')
        || rest.contains('\\');
    if !proper || path_like {
        return ExternalScheme::Never;
    }
    if NEVER_SCHEMES.contains(&scheme.as_str()) {
        return ExternalScheme::Never;
    }
    ExternalScheme::Offer(scheme)
}

/// The origin (scheme, host, port) of an http or https address, None for anything else.
fn web_origin(url: &str) -> Option<url::Origin> {
    let u = url::Url::parse(url.trim()).ok()?;
    matches!(u.scheme(), "http" | "https").then(|| u.origin())
}

/// True when `uri` is a `blob:` the page at `from` made itself (`blob:https://shop.example/<id>` on a page of
/// `https://shop.example`): a receipt or a file it built, which a page may open in its panel or pop-up (rule 253). A
/// blob of another origin, or one of a page that is not http or https, is false.
pub fn own_blob(uri: &str, from: &str) -> bool {
    let Some(inner) = uri
        .get(..5)
        .filter(|p| p.eq_ignore_ascii_case("blob:"))
        .map(|_| &uri[5..])
    else {
        return false;
    };
    if uri.chars().count() > APP_LINK_MAX || uri.chars().any(char::is_control) {
        return false;
    }
    match (web_origin(inner), web_origin(from)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The browser can tell one app link twice (its navigation, then its launch): the same address within this long is
/// the same link (rule 252).
pub const APP_LINK_DEDUPE_MS: u64 = 1_000;

/// Remembers the last app link a view told, so the same link said twice within [`APP_LINK_DEDUPE_MS`] is told once.
#[derive(Clone, Debug, Default)]
pub struct AppLinkDedupe {
    last: Option<(String, u64)>,
}

impl AppLinkDedupe {
    /// True when `uri` at `now_ms` is new (and it is remembered); false for the same address inside the window.
    pub fn fresh(&mut self, uri: &str, now_ms: u64) -> bool {
        if let Some((u, at)) = &self.last
            && u == uri
            && now_ms.saturating_sub(*at) < APP_LINK_DEDUPE_MS
        {
            return false;
        }
        self.last = Some((uri.to_string(), now_ms));
        true
    }
}

/// The address the browser is sent to: a PDF opens without the viewer's toolbar and side pane, so the
/// pages fill the panel (2026-10-25). An address that already has a fragment is left as it is.
pub fn pdf_view(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let is_pdf = path.to_ascii_lowercase().ends_with(".pdf");
    if is_pdf && !url.contains('#') && navigation_allowed(url) {
        format!("{url}#toolbar=0&navpanes=0")
    } else {
        url.to_string()
    }
}

/// What the web panel does now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebShow {
    /// Visible, on top of the slot.
    Live,
    /// Hidden, with a still of its last frame drawn in the slot (an overlay is open).
    Still,
    /// Hidden, nothing drawn for it (its tab is behind, the panel is closed, or there is no room).
    Hidden,
}

/// The decision, from what the window says. `overlay` is Slint's `overlay-open`, which is the
/// `Overlays::any()` that [`crate::overlay::draw_for`] decides on.
pub fn web_show(tab_in_front: bool, panel_shown: bool, overlay: bool) -> WebShow {
    if !tab_in_front || !panel_shown {
        WebShow::Hidden
    } else if overlay {
        WebShow::Still
    } else {
        WebShow::Live
    }
}

/// Where the front tab's web panel is in its life with the overlays: the state machine behind the
/// airspace rule. A web panel is a child window above everything Slint draws, so while an overlay is
/// open it is hidden and a still of its last frame sits in its place; the still is taken BEFORE the
/// window is hidden (a hidden window has no frame to take).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Not shown (the tab is behind, there is no room, or it was just made).
    Hidden,
    /// Shown, on top of the slot.
    Live,
    /// A still was asked for (this token); the window is still shown until it arrives.
    Capturing(u64),
    /// Hidden, the still (if one could be taken) is drawn in the slot.
    Still,
}

/// What the window must do to follow a [`Phase`] change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Show,
    Hide,
    /// Ask for a still with this token.
    Capture(u64),
    /// Let go of the still picture.
    DropStill,
}

/// The next phase and what to do, when the wish is `want`. `token` is the one a capture would carry.
pub fn step(phase: Phase, want: WebShow, token: u64) -> (Phase, Vec<Act>) {
    match (want, phase) {
        (WebShow::Live, Phase::Live) => (Phase::Live, vec![]),
        (WebShow::Live, _) => (Phase::Live, vec![Act::Show, Act::DropStill]),
        (WebShow::Still, Phase::Live) => (Phase::Capturing(token), vec![Act::Capture(token)]),
        (WebShow::Still, Phase::Hidden) => (Phase::Still, vec![]),
        (WebShow::Still, p) => (p, vec![]),
        (WebShow::Hidden, Phase::Hidden) => (Phase::Hidden, vec![]),
        (WebShow::Hidden, _) => (Phase::Hidden, vec![Act::Hide, Act::DropStill]),
    }
}

/// The still (or the failure to take one) arrived: the window is hidden now. None when it answers a
/// capture the window has since moved on from (the overlay closed, the tab changed).
pub fn captured(phase: Phase, token: u64) -> Option<(Phase, Vec<Act>)> {
    match phase {
        Phase::Capturing(t) if t == token => Some((Phase::Still, vec![Act::Hide])),
        _ => None,
    }
}

/// How long a still may take before the window gives up on it and hides the page anyway (ms). A
/// page that is busy or hung never answers a capture, and its window would stay on top of every
/// menu, dialog and bar for as long as it is stuck. The answer that comes late is dropped by the
/// token check in [`captured`].
pub const CAPTURE_TIMEOUT_MS: u64 = 250;

/// What a browser process failure means for a web panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The browser restarts it on its own and the page keeps working (the GPU, a utility process, a
    /// frame's renderer, a plug-in, an unresponsive page the browser itself asks about).
    Recovers,
    /// The page's renderer is gone: this panel cannot show its page any more.
    Page,
    /// The browser process is gone: every panel's page is gone and the environment is dead.
    Browser,
}

/// WebView2's `COREWEBVIEW2_PROCESS_FAILED_KIND` (the number) as the panel treats it. The numbers are
/// the SDK's: 0 browser, 1 render, 2 render unresponsive, 3 frame render, 4 utility, 5 sandbox
/// helper, 6 GPU, 7 and 8 plug-in, 9 unknown.
pub fn failure_of_kind(kind: i32) -> Failure {
    match kind {
        0 => Failure::Browser,
        1 => Failure::Page,
        _ => Failure::Recovers,
    }
}

/// The second Escape in a short time hands the keys back to the terminal; the first one is the page's
/// (a page that uses Escape keeps it). Any other key in between starts the count again.
#[derive(Clone, Copy, Debug, Default)]
pub struct EscTwice {
    last: Option<u64>,
}

/// How long a second Escape counts as the second (ms).
pub const ESC_WINDOW_MS: u64 = 600;

impl EscTwice {
    /// An Escape at `now_ms`: true when it is the second.
    pub fn escape(&mut self, now_ms: u64) -> bool {
        match self.last {
            Some(t) if now_ms.saturating_sub(t) <= ESC_WINDOW_MS => {
                self.last = None;
                true
            }
            _ => {
                self.last = Some(now_ms);
                false
            }
        }
    }

    /// Any other key.
    pub fn other_key(&mut self) {
        self.last = None;
    }
}

/// A rectangle in physical pixels of the window's client area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PhysRect {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// A logical rectangle (css px, as Slint reports it) in physical pixels. Both edges are rounded, not
/// the size, so two neighbours that touch in logical px touch in physical px and the rectangle never
/// drifts by a pixel from the one Slint paints.
pub fn phys_rect(x: f32, y: f32, w: f32, h: f32, scale: f32) -> PhysRect {
    let k = if scale.is_finite() && scale > 0.0 {
        f64::from(scale)
    } else {
        1.0
    };
    let edge = |v: f32| (f64::from(v) * k).round() as i32;
    let (x0, y0) = (edge(x), edge(y));
    PhysRect {
        x: x0,
        y: y0,
        w: (edge(x + w) - x0).max(0),
        h: (edge(y + h) - y0).max(0),
    }
}

/// What one step of a splitter drag asks of the settle timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragSettle {
    /// The slot did not change: the timer runs on as it was.
    Keep,
    /// The slot moved and the page is not at its width: the wait starts again.
    Restart,
    /// The slot moved back to the page's width: nothing is owed.
    Cancel,
}

/// A web page while the splitter beside it is held. A new width makes the page lay itself out again, which
/// flickers when it happens at every pointer step, so the page keeps its width and only its left edge follows
/// the splitter: narrowing, the page's right part goes past the window's edge (a child window is clipped there);
/// widening, the panel's ground shows at its right. The page is never over the terminal. It takes the slot's
/// width after a short pause ([`DragHold::settle`]) and when the splitter is let go (the hold ends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DragHold {
    width: i32,
    slot: Option<PhysRect>,
}

impl DragHold {
    /// Starts holding at the width the page has now.
    pub fn new(width: i32) -> Self {
        Self { width, slot: None }
    }

    /// The slot is at `slot` now: where the page goes, and what that asks of the settle timer.
    pub fn step(&mut self, slot: PhysRect) -> (PhysRect, DragSettle) {
        let moved = self.slot != Some(slot);
        self.slot = Some(slot);
        let settle = match (moved, slot.w == self.width) {
            (false, _) => DragSettle::Keep,
            (true, true) => DragSettle::Cancel,
            (true, false) => DragSettle::Restart,
        };
        (
            PhysRect {
                w: self.width,
                ..slot
            },
            settle,
        )
    }

    /// The pointer paused: the page may take the slot's width. True when that is a change.
    pub fn settle(&mut self) -> bool {
        match self.slot {
            Some(slot) if slot.w != self.width => {
                self.width = slot.w;
                true
            }
            _ => false,
        }
    }
}

/// How far the web view starts inside the slot's left edge, in physical pixels: past the splitter's whole
/// grab zone (half of `SPLITTER_HIT`, which is centred on the edge). A browser window is a child window and
/// takes the mouse above anything the window draws, so any part of the grab zone under it would click into
/// the page and never start a drag.
pub fn web_inset_px(scale: f64) -> i32 {
    let k = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (layout::SPLITTER_HIT / 2.0 * k).round() as i32
}

/// The hairline at the slot's left edge, in physical pixels.
pub fn hair_px(scale: f64) -> i32 {
    let k = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (k.floor() as i32).max(1)
}

/// Where the page sits, from the layout maths alone: the slot's right part, below the header and
/// the browser bar. `page` is the area right of the sidebar (left edge, top edge, width, height in
/// physical pixels); `header` is the header and the bar together. `None` when the page has no room
/// for a slot. The window's own numbers (`panel-web-*`) are held to this in the e2e.
pub fn web_rect(
    page_left: i32,
    page_top: i32,
    page_w: i32,
    page_h: i32,
    scale: f64,
    share: Option<f32>,
    header: i32,
) -> Option<PhysRect> {
    let l: SlotLayout = layout::slot_layout(page_w, scale, share);
    if !l.shown {
        return None;
    }
    let inset = web_inset_px(scale);
    Some(PhysRect {
        x: page_left + page_w - l.slot_w + inset,
        y: page_top + header,
        w: (l.slot_w - inset).max(0),
        h: (page_h - header).max(0),
    })
}

/// The WebView2 profile folder of the web panel: always `<profile>\WebView2`, under the profile folder of the run
/// (`--user-data-dir`, else `%APPDATA%\Flare`), so its cookies and storage (a site's login) last as long as the
/// profile. Never the install folder, which an installer replaces whole, and never the user's Edge profile (S4).
/// The mini-apps' and plugins' folders are its siblings (`WebView2-<kind>`).
pub fn webview_folder(profile_dir: &Path) -> PathBuf {
    profile_dir.join("WebView2")
}

/// Where the default run kept the web panel's profile before 2026-11-10: `%LOCALAPPDATA%\Flare\WebView2`, inside the
/// installed copy's root, which Velopack's Setup moves aside and deletes on a reinstall (and its uninstall deletes),
/// taking every login with it. None for a run with its own `--user-data-dir` (that one was always under its profile).
pub fn old_webview_folder(
    user_data_dir: Option<&Path>,
    local_appdata: Option<&str>,
) -> Option<PathBuf> {
    if user_data_dir.is_some() {
        return None;
    }
    local_appdata
        .filter(|a| !a.is_empty())
        .map(|a| PathBuf::from(a).join("Flare").join("WebView2"))
}

/// The browser folders a profile has: the web panel's, then its siblings (mini-apps and plugins).
fn browser_folders(web: &Path) -> Vec<PathBuf> {
    let mut v = vec![web.to_path_buf()];
    for tail in ["mini-sealed", "app-net", "app-sealed", "plugin-net"] {
        v.push(crate::app::sibling_folder(web, tail));
    }
    v
}

/// The web panel's folder this run's browsers use, with nothing moved: the old one (`old`) while it still waits to
/// move (it is there and its new place is not), so no browser makes an empty `new` beside the old logins and leaves
/// them behind for good; else `new`. The mini-apps' and plugins' folders follow it (its `-<kind>` siblings).
pub fn webview_place(old: Option<&Path>, new: &Path) -> PathBuf {
    match old {
        Some(old) if old.is_dir() && !new.exists() => old.to_path_buf(),
        _ => new.to_path_buf(),
    }
}

/// Moves the old browser folders (`old` and its `-<kind>` siblings) to their place beside `new`, all or none: each
/// is moved only when it exists and its new place does not, so nothing already there is ever replaced and a second
/// call does nothing. When one cannot move (a browser of the run before still holds a file, another disk), the ones
/// already moved go back, so the set stays together where `webview_place` finds it, and the next call tries again.
/// Gives how many were moved and the first error.
pub fn move_old_webview_folders(old: &Path, new: &Path) -> (usize, Option<String>) {
    // sync-ok: once at start and once when the first browser is made; a rename on the same disk, not a copy
    move_old_webview_folders_with(old, new, |from, to| std::fs::rename(from, to))
}

fn move_old_webview_folders_with(
    old: &Path,
    new: &Path,
    rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> (usize, Option<String>) {
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (from, to) in browser_folders(old).into_iter().zip(browser_folders(new)) {
        if !from.is_dir() || to.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            // sync-ok: once at start, before any browser; one folder made, nothing read
            let _ = std::fs::create_dir_all(parent);
        }
        match rename(&from, &to) {
            Ok(()) => moved.push((from, to)),
            Err(e) => {
                let mut error = format!("{}: {e}", from.display());
                let mut kept = 0;
                for (back_to, back_from) in moved.iter().rev() {
                    if let Err(e) = rename(back_from, back_to) {
                        kept += 1;
                        error.push_str(&format!("; {} did not go back: {e}", back_from.display()));
                    }
                }
                return (kept, Some(error));
            }
        }
    }
    (moved.len(), None)
}

/// Tries the move again and gives the web panel's folder to use now (`webview_place`): called when the first
/// browser is made, so folders a browser of the run before still held at start move as soon as it has let go, and
/// until then this run keeps using the old place with its logins. None `old` (a run with its own profile): `new`.
pub fn settle_webview_folder(old: Option<&Path>, new: &Path) -> (PathBuf, usize, Option<String>) {
    let Some(old_dir) = old else {
        return (new.to_path_buf(), 0, None);
    };
    let (moved, error) = move_old_webview_folders(old_dir, new);
    (webview_place(old, new), moved, error)
}

/// Is `got` the address `want` asked for? Both parse and compare as addresses (case of the host and a
/// default port do not matter); the fragment counts only when `with_fragment`. A text that does not
/// parse matches only itself.
pub fn url_matches(want: &str, got: &str, with_fragment: bool) -> bool {
    match (url::Url::parse(want), url::Url::parse(got)) {
        (Ok(mut a), Ok(mut b)) => {
            if !with_fragment {
                a.set_fragment(None);
                b.set_fragment(None);
            }
            a == b
        }
        _ => want == got,
    }
}

/// Where the page lives for a title: the host of the address, else the address.
pub fn host_label(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// The longest host the diagnostics log keeps (a DNS name is at most 253 characters).
const SITE_MAX: usize = 253;

/// The site an address belongs to, as the diagnostics log may keep it (rule 247): the host only, in lower case,
/// with no port, user name, password, path, query or fragment. An address with no host (`about:blank`, `data:`,
/// `blob:`) is its scheme alone. Anything else is `unparsed` (not an address) or `odd` (a host or scheme with
/// characters no site name has). It never returns the text it was given, unlike [`host_label`].
pub fn site_of(url: &str) -> String {
    let Ok(u) = url::Url::parse(url.trim()) else {
        return "unparsed".to_string();
    };
    match u.host_str().filter(|h| !h.is_empty()) {
        Some(host) => {
            let host = host.to_ascii_lowercase();
            let clean = host.len() <= SITE_MAX
                && host.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']')
                });
            if clean { host } else { "odd".to_string() }
        }
        None => {
            let scheme = u.scheme();
            let clean = scheme.len() <= 32
                && scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
            if clean {
                scheme.to_ascii_lowercase()
            } else {
                "odd".to_string()
            }
        }
    }
}

/// WebView2's `COREWEBVIEW2_WEB_ERROR_STATUS` (the number) as a fixed word for the diagnostics log.
pub fn web_error_name(status: i32) -> &'static str {
    match status {
        0 => "unknown",
        1 => "cert-name-mismatch",
        2 => "cert-expired",
        3 => "client-cert-error",
        4 => "cert-revoked",
        5 => "cert-invalid",
        6 => "server-unreachable",
        7 => "timeout",
        8 => "invalid-response",
        9 => "connection-aborted",
        10 => "connection-reset",
        11 => "disconnected",
        12 => "cannot-connect",
        13 => "name-not-resolved",
        14 => "cancelled",
        15 => "redirect-failed",
        16 => "unexpected-error",
        17 => "auth-required",
        18 => "proxy-auth-required",
        _ => "other",
    }
}

/// WebView2's `COREWEBVIEW2_PERMISSION_KIND` (the number) as a fixed word for the diagnostics log.
pub fn permission_word(kind: i32) -> &'static str {
    match kind {
        0 => "unknown",
        1 => "microphone",
        2 => "camera",
        3 => "location",
        4 => "notifications",
        5 => "sensors",
        6 => "clipboard-read",
        7 => "multiple-downloads",
        8 => "file-access",
        9 => "autoplay",
        10 => "local-fonts",
        11 => "midi-sysex",
        12 => "window-management",
        _ => "other",
    }
}

/// WebView2's `COREWEBVIEW2_PROCESS_FAILED_KIND` (the number, as [`failure_of_kind`] reads it) as a fixed word.
pub fn process_word(kind: i32) -> &'static str {
    match kind {
        0 => "browser",
        1 => "render",
        2 => "render-unresponsive",
        3 => "frame-render",
        4 => "utility",
        5 => "sandbox-helper",
        6 => "gpu",
        7 => "plugin",
        8 => "plugin-broker",
        9 => "unknown",
        _ => "other",
    }
}

/// WebView2's `COREWEBVIEW2_PROCESS_FAILED_REASON` (the number) as a fixed word.
pub fn process_reason_word(reason: i32) -> &'static str {
    match reason {
        0 => "unexpected",
        1 => "unresponsive",
        2 => "terminated",
        3 => "crashed",
        4 => "launch-failed",
        5 => "out-of-memory",
        6 => "profile-deleted",
        _ => "other",
    }
}

/// Should a failed navigation be logged? Not one Flare cancelled itself (14, its `web-refused` line says why) and
/// not a stream handed to the player or a download (9): neither is a page that failed.
pub fn load_failure_logged(ok: bool, status: i32) -> bool {
    !ok && !matches!(status, 9 | 14)
}

/// The most pop-ups one tab's web panel holds at once (waiting ones included).
pub const POPUPS_PER_TAB: usize = 2;
/// The most pop-ups the whole window holds at once.
pub const POPUPS_IN_ALL: usize = 4;

/// Why a page's request for a window of its own opened nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupRefusal {
    /// The address is not one the web panel opens (only http, https and about:blank).
    Scheme,
    /// No click or key of the person's started it: Chrome's blocker would stop it too ("Pop-up blocked").
    Blocked,
    /// The tab or the window holds as many pop-ups as it may ([`POPUPS_PER_TAB`], [`POPUPS_IN_ALL`]).
    Limit,
}

impl PopupRefusal {
    /// The word of the diagnostics note (`refused`, `blocked`, `limit`).
    pub fn word(self) -> &'static str {
        match self {
            PopupRefusal::Scheme => "refused",
            PopupRefusal::Blocked => "blocked",
            PopupRefusal::Limit => "limit",
        }
    }
}

/// What the web panel does with a page's request for a window of its own (`NewWindowRequested`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupPlan {
    /// The address opens in the view that asked, as a plain link does (`target=_blank`).
    InPlace,
    /// A real second view, shown over the page in the panel, that keeps its link to the page (`window.opener`).
    Popup,
    /// Nothing opens.
    Refuse(PopupRefusal),
}

/// Decides a request for a new window from the plain web panel (rule 249). `name` is the window's name, None when
/// the Runtime cannot say it (`ICoreWebView2NewWindowRequestedEventArgs2` missing: the size, position and address
/// decide alone). `features`: the request gave a size or a position. `user`: the person's click or key started it
/// (`IsUserInitiated`). `counts`: the pop-ups the tab holds and the window holds, waiting ones included.
///
/// Refused: an address the panel may not open, any request no click or key started (as Chrome's blocker does), and a
/// pop-up past the limits. A pop-up: a request with a size or a position, a named window (not `_blank`, `_self`,
/// `_parent` or `_top`), or one for `about:blank` or no address (a page that writes into the window it opened).
/// Anything else opens in place, as it always did.
pub fn popup_plan(
    uri: &str,
    name: Option<&str>,
    features: bool,
    user: bool,
    counts: (usize, usize),
) -> PopupPlan {
    let blank = uri.trim().is_empty() || uri.trim().eq_ignore_ascii_case("about:blank");
    if !blank && !navigation_allowed(uri) {
        return PopupPlan::Refuse(PopupRefusal::Scheme);
    }
    if !user {
        return PopupPlan::Refuse(PopupRefusal::Blocked);
    }
    let named = name.is_some_and(|n| {
        let n = n.trim();
        !n.is_empty()
            && !["_blank", "_self", "_parent", "_top"]
                .iter()
                .any(|t| n.eq_ignore_ascii_case(t))
    });
    if !(features || named || blank) {
        return PopupPlan::InPlace;
    }
    let (tab, all) = counts;
    if tab >= POPUPS_PER_TAB || all >= POPUPS_IN_ALL {
        return PopupPlan::Refuse(PopupRefusal::Limit);
    }
    PopupPlan::Popup
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{Overlay, Overlays, PanelDraw, draw_for};

    #[test]
    fn a_pdf_opens_without_the_viewers_toolbar_and_nothing_else_changes() {
        assert_eq!(
            pdf_view("http://127.0.0.1:8766/report.pdf"),
            "http://127.0.0.1:8766/report.pdf#toolbar=0&navpanes=0"
        );
        assert_eq!(
            pdf_view("https://x.org/A.PDF?dl=1"),
            "https://x.org/A.PDF?dl=1#toolbar=0&navpanes=0"
        );
        // A fragment someone chose (a page, a zoom) is theirs.
        assert_eq!(
            pdf_view("https://x.org/a.pdf#page=3"),
            "https://x.org/a.pdf#page=3"
        );
        assert_eq!(
            pdf_view("https://x.org/a.pdf.html"),
            "https://x.org/a.pdf.html"
        );
        assert_eq!(pdf_view("https://x.org/?f=a.pdf"), "https://x.org/?f=a.pdf");
        assert_eq!(pdf_view("about:blank"), "about:blank");
    }

    #[test]
    fn a_capture_that_never_answers_is_ended_by_its_timeout() {
        // The overlay opens: a capture is asked for.
        let (phase, acts) = step(Phase::Live, WebShow::Still, 7);
        assert_eq!(phase, Phase::Capturing(7));
        assert_eq!(acts, vec![Act::Capture(7)]);
        // The page is hung, so the timeout stands in for the answer: hidden, no picture.
        let (phase, acts) = captured(phase, 7).expect("the same token");
        assert_eq!(phase, Phase::Still);
        assert_eq!(acts, vec![Act::Hide]);
        // The real answer, if it ever comes, finds the window moved on.
        assert!(captured(phase, 7).is_none());
        const { assert!(CAPTURE_TIMEOUT_MS <= 500, "short enough to be unseen") };
    }

    #[test]
    fn only_the_fatal_process_failures_end_a_panel() {
        assert_eq!(failure_of_kind(0), Failure::Browser);
        assert_eq!(failure_of_kind(1), Failure::Page);
        for recoverable in [2, 3, 4, 5, 6, 7, 8, 9, 42] {
            assert_eq!(failure_of_kind(recoverable), Failure::Recovers);
        }
    }

    #[test]
    fn an_address_matches_itself_in_the_forms_a_browser_reports() {
        assert!(url_matches(
            "https://Example.com/a",
            "https://example.com/a",
            false
        ));
        assert!(url_matches(
            "https://example.com:443/",
            "https://example.com/",
            true
        ));
        assert!(url_matches("https://e.com/a", "https://e.com/a#x", false));
        assert!(!url_matches("https://e.com/a", "https://e.com/a#x", true));
        assert!(!url_matches("https://e.com/a", "https://e.com/b", false));
        assert!(url_matches("about:blank", "about:blank", true));
    }

    #[test]
    fn text_with_no_scheme_becomes_https() {
        assert_eq!(
            normalize_url("example.com").unwrap(),
            "https://example.com/"
        );
        assert_eq!(
            normalize_url("  Example.COM/Path?q=1 ").unwrap(),
            "https://example.com/Path?q=1"
        );
        assert_eq!(
            normalize_url("sub.example.com:8443/x").unwrap(),
            "https://sub.example.com:8443/x"
        );
        assert_eq!(
            normalize_url("//example.com").unwrap(),
            "https://example.com/"
        );
    }

    #[test]
    fn a_local_host_with_no_scheme_becomes_http() {
        assert_eq!(
            normalize_url("localhost:3000").unwrap(),
            "http://localhost:3000/"
        );
        assert_eq!(
            normalize_url("127.0.0.1:8080/app").unwrap(),
            "http://127.0.0.1:8080/app"
        );
        assert_eq!(normalize_url("localhost").unwrap(), "http://localhost/");
        assert_eq!(normalize_url("[::1]:5173").unwrap(), "http://[::1]:5173/");
    }

    #[test]
    fn http_and_https_pass_as_given() {
        assert_eq!(
            normalize_url("http://example.com").unwrap(),
            "http://example.com/"
        );
        assert_eq!(
            normalize_url("HTTPS://Example.com/a%20b").unwrap(),
            "https://example.com/a%20b"
        );
    }

    #[test]
    fn about_blank_passes_and_no_other_about() {
        assert_eq!(normalize_url("about:blank").unwrap(), "about:blank");
        assert_eq!(normalize_url("ABOUT:BLANK").unwrap(), "about:blank");
        assert_eq!(normalize_url("about:config"), Err(UrlError::Scheme));
        assert_eq!(normalize_url("about:blank#x"), Err(UrlError::Scheme));
    }

    #[test]
    fn every_other_scheme_is_refused() {
        for s in [
            "file:///C:/Windows/win.ini",
            "FILE://server/share/a.txt",
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "ftp://example.com/",
            "chrome://settings",
            "edge://flags",
            "mailto:a@b.c",
            "ms-settings:network",
            "blob:https://example.com/1",
            "view-source:https://example.com",
            "vbscript:x",
        ] {
            assert_eq!(normalize_url(s), Err(UrlError::Scheme), "{s}");
        }
    }

    #[test]
    fn bad_text_is_refused() {
        assert_eq!(normalize_url(""), Err(UrlError::Empty));
        assert_eq!(normalize_url("   "), Err(UrlError::Empty));
        assert_eq!(normalize_url("two words"), Err(UrlError::BadChars));
        assert_eq!(normalize_url("a\u{7}b.com"), Err(UrlError::BadChars));
        assert_eq!(normalize_url("a.com\n"), Ok("https://a.com/".to_string()));
        assert_eq!(normalize_url("a\nb.com"), Err(UrlError::BadChars));
        assert_eq!(
            normalize_url(&format!("https://a.com/{}", "x".repeat(2100))),
            Err(UrlError::TooLong)
        );
        assert_eq!(normalize_url("https://"), Err(UrlError::Invalid));
        // A host is always needed: these have none, whatever the rest says.
        for none in ["https://:80/", "http://?q=1", "https://#top"] {
            assert!(normalize_url(none).is_err(), "{none}");
        }
    }

    #[test]
    fn a_login_in_the_address_is_refused() {
        assert_eq!(
            normalize_url("https://google.com@evil.example/"),
            Err(UrlError::Invalid)
        );
        // Read as a scheme called `user`, which is refused too.
        assert!(normalize_url("user:pw@example.com").is_err());
        assert!(normalize_url("https://user@example.com/").is_err());
    }

    #[test]
    fn a_page_may_navigate_only_to_web_addresses() {
        assert!(navigation_allowed("https://example.com/x"));
        assert!(navigation_allowed("HTTP://example.com"));
        assert!(navigation_allowed("about:blank"));
        for s in [
            "file:///C:/x",
            "javascript:1",
            "data:text/html,x",
            "mailto:a@b.c",
            "ms-settings:",
            "blob:https://a/1",
            "ftp://a/",
            "about:srcdoc",
            "",
        ] {
            assert!(!navigation_allowed(s), "{s}");
        }
    }

    #[test]
    fn the_web_decision_follows_the_overlay_property() {
        // A tab behind, or no slot: nothing is drawn for it, overlay or not.
        for overlay in [false, true] {
            assert_eq!(web_show(false, true, overlay), WebShow::Hidden);
            assert_eq!(web_show(true, false, overlay), WebShow::Hidden);
        }
        assert_eq!(web_show(true, true, false), WebShow::Live);
        assert_eq!(web_show(true, true, true), WebShow::Still);
        // It is `draw_for` over the overlay list, for every overlay.
        for which in Overlay::ALL {
            let mut o = Overlays::default();
            assert_eq!(
                web_show(true, true, o.any()),
                match draw_for(false, &o) {
                    PanelDraw::Live => WebShow::Live,
                    PanelDraw::Still => WebShow::Still,
                }
            );
            o.set(which, true);
            assert_eq!(web_show(true, true, o.any()), WebShow::Still, "{which:?}");
            assert_eq!(draw_for(false, &o), PanelDraw::Still);
        }
    }

    #[test]
    fn an_overlay_takes_a_still_before_it_hides_the_window() {
        let (p, acts) = step(Phase::Hidden, WebShow::Live, 1);
        assert_eq!((p, acts), (Phase::Live, vec![Act::Show, Act::DropStill]));
        // The overlay opens: ask for the still first; the window is not hidden yet.
        let (p, acts) = step(Phase::Live, WebShow::Still, 7);
        assert_eq!((p, acts), (Phase::Capturing(7), vec![Act::Capture(7)]));
        // Asking again while it is on its way changes nothing.
        assert_eq!(step(p, WebShow::Still, 8), (Phase::Capturing(7), vec![]));
        // The still arrived: hide.
        assert_eq!(captured(p, 7), Some((Phase::Still, vec![Act::Hide])));
        // The overlay closes: show again and let go of the picture.
        assert_eq!(
            step(Phase::Still, WebShow::Live, 9),
            (Phase::Live, vec![Act::Show, Act::DropStill])
        );
    }

    #[test]
    fn a_late_still_is_dropped() {
        // The overlay closed while the still was on its way.
        let (p, _) = step(Phase::Capturing(3), WebShow::Live, 4);
        assert_eq!(p, Phase::Live);
        assert_eq!(captured(p, 3), None);
        // A still for an older capture.
        assert_eq!(captured(Phase::Capturing(5), 4), None);
        assert_eq!(captured(Phase::Hidden, 1), None);
    }

    #[test]
    fn leaving_the_tab_hides_from_any_phase() {
        for p in [Phase::Live, Phase::Capturing(2), Phase::Still] {
            assert_eq!(
                step(p, WebShow::Hidden, 3),
                (Phase::Hidden, vec![Act::Hide, Act::DropStill]),
                "{p:?}"
            );
        }
        assert_eq!(
            step(Phase::Hidden, WebShow::Hidden, 3),
            (Phase::Hidden, vec![])
        );
    }

    #[test]
    fn a_window_made_while_an_overlay_is_open_stays_hidden_with_no_still() {
        assert_eq!(
            step(Phase::Hidden, WebShow::Still, 1),
            (Phase::Still, vec![])
        );
    }

    #[test]
    fn the_second_escape_in_time_counts() {
        let mut e = EscTwice::default();
        assert!(!e.escape(1000));
        assert!(e.escape(1400));
        // The count starts over after the second.
        assert!(!e.escape(1500));
        assert!(!e.escape(1500 + ESC_WINDOW_MS + 1));
        assert!(e.escape(1500 + ESC_WINDOW_MS + 1 + 10));
    }

    #[test]
    fn another_key_between_two_escapes_breaks_the_pair() {
        let mut e = EscTwice::default();
        assert!(!e.escape(0));
        e.other_key();
        assert!(!e.escape(100));
        assert!(e.escape(200));
    }

    #[test]
    fn a_logical_rect_rounds_by_its_edges() {
        for k in [1.0f32, 1.25, 1.5, 2.0, 2.25, 3.0] {
            let r = phys_rect(100.5, 40.0, 300.25, 200.0, k);
            let right = (f64::from(100.5f32 + 300.25) * f64::from(k)).round() as i32;
            assert_eq!(r.x, (100.5f64 * f64::from(k)).round() as i32, "{k}");
            assert_eq!(r.right(), right, "{k}");
            assert_eq!(r.y, (40.0f64 * f64::from(k)).round() as i32);
        }
        // Two rectangles that touch in css px touch in device px.
        let a = phys_rect(0.0, 0.0, 33.3, 10.0, 2.25);
        let b = phys_rect(33.3, 0.0, 10.0, 10.0, 2.25);
        assert_eq!(a.right(), b.x);
        assert_eq!(phys_rect(0.0, 0.0, -5.0, 10.0, 1.0).w, 0);
        assert_eq!(
            phys_rect(1.0, 1.0, 1.0, 1.0, f32::NAN),
            phys_rect(1.0, 1.0, 1.0, 1.0, 1.0)
        );
    }

    #[test]
    fn the_hairline_is_at_least_one_device_pixel() {
        assert_eq!(hair_px(1.0), 1);
        assert_eq!(hair_px(1.5), 1);
        assert_eq!(hair_px(2.25), 2);
        assert_eq!(hair_px(3.0), 3);
        assert_eq!(hair_px(0.0), 1);
        assert_eq!(hair_px(f64::NAN), 1);
    }

    #[test]
    fn the_web_rect_is_the_slot_less_the_header_and_the_splitters_reach() {
        for scale in [1.0f64, 1.5, 2.25, 3.0] {
            let page_w = (1000.0 * scale).round() as i32;
            let page_h = (600.0 * scale).round() as i32;
            let header = (64.0 * scale).round() as i32;
            let r = web_rect(0, 0, page_w, page_h, scale, None, header).unwrap();
            let l = layout::slot_layout(page_w, scale, None);
            assert_eq!(r.right(), page_w, "the page's right edge at {scale}");
            assert_eq!(r.x, page_w - l.slot_w + web_inset_px(scale));
            assert!(
                r.x >= page_w - l.slot_w + (layout::SPLITTER_HIT / 2.0 * scale).floor() as i32,
                "no part of the grab zone is under the page at {scale}"
            );
            assert_eq!(r.y, header);
            assert_eq!(r.h, page_h - header);
            assert!(r.w > 0 && r.w < l.slot_w);
        }
    }

    #[test]
    fn a_page_without_room_has_no_web_rect() {
        assert_eq!(web_rect(0, 0, 300, 600, 1.0, None, 64), None);
        let r = web_rect(240, 31, 1000, 600, 1.0, Some(0.5), 64).unwrap();
        assert_eq!(
            (r.x, r.y, r.right(), r.h),
            (240 + 500 + 4, 31 + 64, 240 + 1000, 536)
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_profile_lives_under_the_profile_folder_never_the_install_folder() {
        assert_eq!(
            webview_folder(Path::new(r"C:\run\p")),
            PathBuf::from(r"C:\run\p").join("WebView2")
        );
        // The default run: the roaming profile, as Stable's `--user-data-dir` has always had it.
        let p = webview_folder(Path::new(r"C:\Users\x\AppData\Roaming\Flare"));
        assert_eq!(
            p,
            PathBuf::from(r"C:\Users\x\AppData\Roaming\Flare\WebView2")
        );
        let s = p.to_string_lossy().to_ascii_lowercase();
        assert!(!s.contains(r"appdata\local\flare"), "{s}");
    }

    #[cfg(windows)]
    #[test]
    fn the_old_place_is_only_the_default_runs_local_appdata_folder() {
        assert_eq!(
            old_webview_folder(None, Some(r"C:\Users\x\AppData\Local")),
            Some(PathBuf::from(r"C:\Users\x\AppData\Local\Flare\WebView2"))
        );
        assert_eq!(
            old_webview_folder(
                Some(Path::new(r"C:\run\p")),
                Some(r"C:\Users\x\AppData\Local")
            ),
            None
        );
        assert_eq!(old_webview_folder(None, None), None);
        assert_eq!(old_webview_folder(None, Some("")), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn the_profile_lives_under_the_profile_folder_never_the_install_folder_posix() {
        assert_eq!(
            webview_folder(Path::new("/run/p")),
            PathBuf::from("/run/p/WebView2")
        );
        let p = webview_folder(Path::new("/Users/x/Library/Application Support/Flare"));
        assert_eq!(
            p,
            PathBuf::from("/Users/x/Library/Application Support/Flare/WebView2")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn the_old_place_is_only_the_default_runs_local_appdata_folder_posix() {
        assert_eq!(
            old_webview_folder(None, Some("/Users/x/Library/Application Support")),
            Some(PathBuf::from(
                "/Users/x/Library/Application Support/Flare/WebView2"
            ))
        );
        assert_eq!(
            old_webview_folder(
                Some(Path::new("/run/p")),
                Some("/Users/x/Library/Application Support")
            ),
            None
        );
        assert_eq!(old_webview_folder(None, None), None);
        assert_eq!(old_webview_folder(None, Some("")), None);
    }

    #[test]
    fn the_old_folders_move_once_and_never_replace_what_is_there() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("local").join("Flare").join("WebView2");
        let new = d.path().join("roaming").join("Flare").join("WebView2");
        for f in [&old, &old.with_file_name("WebView2-app-sealed")] {
            std::fs::create_dir_all(f.join("EBWebView")).unwrap();
            std::fs::write(f.join("EBWebView").join("mark"), b"old").unwrap();
        }
        // The networked plugins' folder is already in its new place: it stays as it is.
        let old_net = old.with_file_name("WebView2-plugin-net");
        let new_net = new.with_file_name("WebView2-plugin-net");
        std::fs::create_dir_all(&old_net).unwrap();
        std::fs::create_dir_all(&new_net).unwrap();
        std::fs::write(new_net.join("mark"), b"new").unwrap();

        assert_eq!(move_old_webview_folders(&old, &new), (2, None));
        assert_eq!(
            std::fs::read(new.join("EBWebView").join("mark")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(
                new.with_file_name("WebView2-app-sealed")
                    .join("EBWebView")
                    .join("mark")
            )
            .unwrap(),
            b"old"
        );
        assert!(!old.exists());
        assert_eq!(std::fs::read(new_net.join("mark")).unwrap(), b"new");
        assert!(old_net.exists());
        // A second start finds nothing to move.
        assert_eq!(move_old_webview_folders(&old, &new), (0, None));
    }

    /// The old web panel folder with a login mark in it, under `d`; gives (old, new).
    fn old_profile_with_a_login(d: &Path) -> (PathBuf, PathBuf) {
        let old = d.join("local").join("Flare").join("WebView2");
        let new = d.join("roaming").join("Flare").join("WebView2");
        std::fs::create_dir_all(old.join("EBWebView")).unwrap();
        std::fs::write(old.join("EBWebView").join("Cookies"), b"login").unwrap();
        (old, new)
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_the_last_run_still_holds_stays_in_use_and_moves_when_it_is_let_go() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = tempfile::tempdir().unwrap();
        let (old, new) = old_profile_with_a_login(d.path());
        // The quitting run's browser still has its cookie file open, sharing nothing: the folder cannot be renamed.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(old.join("EBWebView").join("Cookies"))
            .unwrap();
        let (n, e) = move_old_webview_folders(&old, &new);
        assert_eq!(n, 0);
        assert!(e.is_some());
        // A panel opens: its browser is made in the old folder, with the login, and no empty new one appears.
        let (place, _, e) = settle_webview_folder(Some(&old), &new);
        assert!(e.is_some());
        assert_eq!(place, old);
        assert!(!new.exists());
        drop(held);
        // The next try (the next browser made, or the next start) moves it, login and all.
        assert_eq!(
            settle_webview_folder(Some(&old), &new),
            (new.clone(), 1, None)
        );
        assert_eq!(
            std::fs::read(new.join("EBWebView").join("Cookies")).unwrap(),
            b"login"
        );
        assert!(!old.exists());
    }

    #[test]
    fn a_sibling_that_cannot_move_takes_the_panels_folder_back_with_it() {
        let d = tempfile::tempdir().unwrap();
        let (old, new) = old_profile_with_a_login(d.path());
        let old_plugins = old.with_file_name("WebView2-plugin-net");
        std::fs::create_dir_all(&old_plugins).unwrap();
        let (n, e) = move_old_webview_folders_with(&old, &new, |from, to| {
            if from.ends_with("WebView2-plugin-net") {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            } else {
                std::fs::rename(from, to)
            }
        });
        assert_eq!(n, 0);
        assert!(e.unwrap().contains("WebView2-plugin-net"));
        // All stay together in the old place, which is the one in use until they can move.
        assert!(old.join("EBWebView").join("Cookies").is_file());
        assert!(old_plugins.is_dir());
        assert!(!new.exists());
        assert_eq!(webview_place(Some(&old), &new), old);
    }

    #[test]
    fn the_place_is_the_new_folder_once_nothing_waits_to_move() {
        let d = tempfile::tempdir().unwrap();
        let (old, new) = old_profile_with_a_login(d.path());
        assert_eq!(webview_place(None, &new), new);
        assert_eq!(webview_place(Some(&old), &new), old);
        std::fs::create_dir_all(&new).unwrap();
        // A new folder already there wins: it is never replaced, so the old one is not used either.
        assert_eq!(webview_place(Some(&old), &new), new);
        assert_eq!(settle_webview_folder(None, &new), (new.clone(), 0, None));
        let gone = d.path().join("none").join("WebView2");
        assert_eq!(webview_place(Some(&gone), &new), new);
    }

    #[test]
    fn the_profile_is_never_an_edge_folder() {
        for p in [
            webview_folder(Path::new(r"C:\Users\x\AppData\Roaming\Flare")),
            webview_folder(Path::new(r"D:\data")),
        ] {
            let s = p.to_string_lossy().to_ascii_lowercase();
            assert!(!s.contains("microsoft") && !s.contains("edge"), "{s}");
        }
    }

    #[test]
    fn the_host_is_the_label_of_an_address() {
        assert_eq!(host_label("https://example.com/a?b"), "example.com");
        assert_eq!(host_label("about:blank"), "about:blank");
    }

    fn search(q: &str) -> String {
        format!("{SEARCH_URL}{q}")
    }

    #[test]
    fn the_address_field_opens_an_address_as_an_address() {
        for (typed, url) in [
            ("youtube.com", "https://youtube.com/"),
            ("  youtube.com  ", "https://youtube.com/"),
            ("YouTube.com/watch?v=1#t", "https://youtube.com/watch?v=1#t"),
            ("a.b/path", "https://a.b/path"),
            ("https://x", "https://x/"),
            ("HTTPS://X/y", "https://x/y"),
            ("localhost", "http://localhost/"),
            ("localhost:3000", "http://localhost:3000/"),
            ("127.0.0.1:8080/a", "http://127.0.0.1:8080/a"),
            ("10.0.0.2", "https://10.0.0.2/"),
            ("[::1]:80", "http://[::1]/"),
            ("[::1]", "http://[::1]/"),
            ("myserver:8080", "https://myserver:8080/"),
            ("about:blank", "about:blank"),
            ("ABOUT:BLANK", "about:blank"),
            ("bücher.de", "https://xn--bcher-kva.de/"),
            ("//example.com", "https://example.com/"),
            ("example.com:8443/a", "https://example.com:8443/a"),
            ("127.0.0.1", "http://127.0.0.1/"),
            ("site.xn--p1ai", "https://site.xn--p1ai/"),
        ] {
            assert_eq!(address_or_search(typed).as_deref(), Ok(url), "{typed:?}");
        }
    }

    #[test]
    fn the_address_field_searches_for_anything_else() {
        for (typed, q) in [
            ("youtube", "youtube"),
            ("cat videos", "cat+videos"),
            ("  what is rust  ", "what+is+rust"),
            ("what is rust?", "what+is+rust%3F"),
            ("why?", "why%3F"),
            ("c#", "c%23"),
            ("rust #lifetimes", "rust+%23lifetimes"),
            ("a&b=c", "a%26b%3Dc"),
            ("100%", "100%25"),
            ("kaffe på søndag", "kaffe+p%C3%A5+s%C3%B8ndag"),
            ("日本", "%E6%97%A5%E6%9C%AC"),
            ("1.5", "1.5"),
            ("e.g.", "e.g."),
            ("note:this", "note%3Athis"),
            ("me@example.com", "me%40example.com"),
            ("http://example.com/a b", "http%3A%2F%2Fexample.com%2Fa+b"),
            ("tab\there", "tab%09here"),
            // A time or a verse is not an IP and a port (`10:30` parsed as `0.0.0.10:30`).
            ("10:30", "10%3A30"),
            ("3:16", "3%3A16"),
            ("1.2:80", "1.2%3A80"),
            ("a_b:80", "a_b%3A80"),
            ("ab.ü1x", "ab.%C3%BC1x"),
        ] {
            assert_eq!(
                address_or_search(typed).as_deref(),
                Ok(search(q).as_str()),
                "{typed:?}"
            );
        }
    }

    #[test]
    fn the_address_field_still_refuses_a_scheme_and_does_not_search_it() {
        for typed in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/html,hi",
            "ftp://example.com",
            "chrome://settings",
            "about:config",
        ] {
            assert_eq!(address_or_search(typed), Err(UrlError::Scheme), "{typed:?}");
        }
        assert_eq!(address_or_search(""), Err(UrlError::Empty));
        assert_eq!(address_or_search("   "), Err(UrlError::Empty));
        assert_eq!(address_or_search("a\u{7}b"), Err(UrlError::BadChars));
        assert_eq!(
            address_or_search(&"x ".repeat(TARGET_MAX)),
            Err(UrlError::TooLong)
        );
        assert_eq!(
            address_or_search(&"æ".repeat(TARGET_MAX / 2)),
            Err(UrlError::TooLong),
            "the encoded query is held to the same length"
        );
    }

    #[test]
    fn a_caller_and_a_stored_panel_never_get_a_search() {
        assert_eq!(normalize_url("cat videos"), Err(UrlError::BadChars));
        assert_eq!(normalize_url("youtube").as_deref(), Ok("https://youtube/"));
    }

    #[test]
    fn a_site_that_cannot_be_reached_is_named_and_the_rest_say_nothing() {
        assert_eq!(
            failed_load_notice("https://youtube/", "13").as_deref(),
            Some("Could not open youtube")
        );
        for status in ["6", "7", "12", "2", "16"] {
            assert_eq!(
                failed_load_notice("https://x.org/a", status).as_deref(),
                Some("Could not open x.org"),
                "{status}"
            );
        }
        // Stopped or replaced, a stream or download, the page's own error, a login, nothing known.
        for status in ["14", "9", "0", "17", "18", "", "x"] {
            assert_eq!(
                failed_load_notice("https://x.org/a", status),
                None,
                "{status}"
            );
        }
        assert_eq!(failed_load_notice("about:blank", "13"), None);
        assert_eq!(failed_load_notice("", "13"), None);
    }

    #[test]
    fn the_logs_site_is_the_host_alone_and_never_the_address() {
        let cases = [
            (
                "https://Bank.Example.COM:8443/acct?id=42&token=s3cret#frag",
                "bank.example.com",
            ),
            ("https://user:pw@shop.example/cart", "shop.example"),
            ("http://127.0.0.1:3000/x", "127.0.0.1"),
            ("http://[::1]:8080/", "[::1]"),
            ("https://xn--bcher-kva.example/", "xn--bcher-kva.example"),
            ("https://bücher.example/", "xn--bcher-kva.example"),
            ("about:blank", "about"),
            ("data:text/html,<b>secret</b>", "data"),
            ("blob:https://a.example/1234-5678", "blob"),
            ("mailto:someone@private.example", "mailto"),
            ("file:///C:/Users/me/secret.txt", "file"),
        ];
        for (url, site) in cases {
            assert_eq!(site_of(url), site, "{url}");
        }
        for not_an_address in ["", "secret words", "/just/a/path?q=1", "://"] {
            assert_eq!(site_of(not_an_address), "unparsed", "{not_an_address}");
        }
        // A host a non-web scheme carries as it is: whatever is not a site name's character is `odd`.
        assert_eq!(site_of("foo://we%20ird/x"), "odd");
        // Unlike the title's label, the raw text never comes back.
        assert_eq!(host_label("secret words"), "secret words");
        assert_ne!(site_of("secret words"), "secret words");
    }

    #[test]
    fn errors_permissions_and_processes_are_fixed_words() {
        assert_eq!(web_error_name(2), "cert-expired");
        assert_eq!(web_error_name(13), "name-not-resolved");
        assert_eq!(web_error_name(99), "other");
        assert_eq!(web_error_name(-1), "other");
        assert_eq!(permission_word(2), "camera");
        assert_eq!(permission_word(3), "location");
        assert_eq!(permission_word(42), "other");
        assert_eq!(process_word(0), "browser");
        assert_eq!(process_word(6), "gpu");
        assert_eq!(process_word(-1), "other");
        assert_eq!(process_reason_word(3), "crashed");
        assert_eq!(process_reason_word(77), "other");
        for n in -1..20 {
            for w in [
                web_error_name(n),
                permission_word(n),
                process_word(n),
                process_reason_word(n),
            ] {
                assert!(
                    w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                    "{w}"
                );
            }
        }
    }

    #[test]
    fn only_a_load_that_failed_on_its_own_is_logged() {
        assert!(load_failure_logged(false, 13));
        assert!(load_failure_logged(false, 2));
        assert!(load_failure_logged(false, 0));
        // Ours (a cancelled navigation) and the player's hand-off are not failures, and a good load is none.
        assert!(!load_failure_logged(false, 14));
        assert!(!load_failure_logged(false, 9));
        assert!(!load_failure_logged(true, 13));
    }

    const BANK: &str = "https://bank.example/3ds?session=1";

    #[test]
    fn a_request_that_looks_like_a_popup_opens_one_after_the_persons_click() {
        let none = (0, 0);
        // A size or position, a named window, a blank page to write into.
        assert_eq!(
            popup_plan(BANK, Some(""), true, true, none),
            PopupPlan::Popup
        );
        assert_eq!(
            popup_plan(BANK, Some("threeDS"), false, true, none),
            PopupPlan::Popup
        );
        for blank in ["about:blank", "ABOUT:BLANK", "", "  "] {
            assert_eq!(
                popup_plan(blank, Some(""), false, true, none),
                PopupPlan::Popup,
                "{blank:?}"
            );
        }
        // A runtime that cannot say the name: the size and the address decide alone.
        assert_eq!(popup_plan(BANK, None, true, true, none), PopupPlan::Popup);
        assert_eq!(
            popup_plan(BANK, None, false, true, none),
            PopupPlan::InPlace
        );
    }

    #[test]
    fn a_plain_new_tab_link_still_opens_in_the_panel() {
        for name in [
            Some(""),
            Some("_blank"),
            Some("_BLANK"),
            Some("_self"),
            Some("_parent"),
            Some("_top"),
            None,
        ] {
            assert_eq!(
                popup_plan(BANK, name, false, true, (0, 0)),
                PopupPlan::InPlace,
                "{name:?}"
            );
        }
        // The limits are the pop-ups': a plain link opens in place whatever is open.
        assert_eq!(
            popup_plan(
                BANK,
                Some("_blank"),
                false,
                true,
                (POPUPS_PER_TAB, POPUPS_IN_ALL)
            ),
            PopupPlan::InPlace
        );
    }

    #[test]
    fn a_window_no_click_of_the_persons_asked_for_is_blocked() {
        for (name, features) in [(Some("threeDS"), true), (Some(""), false), (None, true)] {
            assert_eq!(
                popup_plan(BANK, name, features, false, (0, 0)),
                PopupPlan::Refuse(PopupRefusal::Blocked)
            );
        }
        assert_eq!(PopupRefusal::Blocked.word(), "blocked");
    }

    #[test]
    fn an_address_the_panel_may_not_open_is_refused_before_anything_else() {
        for bad in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/html,x",
            "zoommtg://zoom.us/join",
        ] {
            assert_eq!(
                popup_plan(bad, Some("w"), true, true, (0, 0)),
                PopupPlan::Refuse(PopupRefusal::Scheme),
                "{bad}"
            );
            assert_eq!(
                popup_plan(bad, None, false, false, (9, 9)),
                PopupPlan::Refuse(PopupRefusal::Scheme)
            );
        }
        assert_eq!(PopupRefusal::Scheme.word(), "refused");
    }

    #[test]
    fn past_two_popups_a_tab_or_four_in_all_the_next_is_refused() {
        let ask = |counts| popup_plan(BANK, Some("w"), true, true, counts);
        assert_eq!(ask((0, 0)), PopupPlan::Popup);
        assert_eq!(ask((1, 3)), PopupPlan::Popup);
        assert_eq!(ask((2, 2)), PopupPlan::Refuse(PopupRefusal::Limit));
        assert_eq!(ask((0, 4)), PopupPlan::Refuse(PopupRefusal::Limit));
        assert_eq!(PopupRefusal::Limit.word(), "limit");
    }
}

#[cfg(test)]
mod app_link_tests {
    use super::*;

    fn offer(s: &str) -> ExternalScheme {
        ExternalScheme::Offer(s.to_string())
    }

    #[test]
    fn a_link_to_an_app_is_offered_by_its_scheme_in_lower_case() {
        assert_eq!(
            external_scheme("bankid:///?autostarttoken=abc&redirect=null"),
            offer("bankid")
        );
        assert_eq!(
            external_scheme("swish://paymentrequest?token=x"),
            offer("swish")
        );
        assert_eq!(external_scheme("mailto:a@b.example"), offer("mailto"));
        assert_eq!(external_scheme("tel:+4612345"), offer("tel"));
        assert_eq!(external_scheme("BankID:///x"), offer("bankid"));
        assert_eq!(external_scheme("ms-teams:/l/x"), offer("ms-teams"));
        assert_eq!(external_scheme("web+app:x"), offer("web+app"));
    }

    #[test]
    fn a_web_address_is_not_an_apps() {
        for u in ["https://a.example/", "http://a.example/x", "about:blank"] {
            assert_eq!(external_scheme(u), ExternalScheme::NotApp, "{u}");
        }
        // The panel's own rule is unchanged: none of these is a page the address bar or an agent may open.
        for u in [
            "bankid:///x",
            "mailto:a@b.example",
            "blob:https://a.example/1",
        ] {
            assert!(!navigation_allowed(u), "{u}");
        }
    }

    #[test]
    fn every_scheme_on_the_never_list_is_never_offered() {
        for s in NEVER_SCHEMES {
            assert_eq!(
                external_scheme(&format!("{s}:x")),
                ExternalScheme::Never,
                "{s}"
            );
            let upper = s.to_ascii_uppercase();
            assert_eq!(
                external_scheme(&format!("{upper}:x")),
                ExternalScheme::Never,
                "{upper}"
            );
        }
        for must in [
            "javascript",
            "data",
            "file",
            "vbscript",
            "about",
            "blob",
            "ms-msdt",
            "ms-officecmd",
            "ms-appinstaller",
            "search-ms",
            "search",
            "ms-search",
            "ms-cxh",
            "ms-cxh-full",
            "shell",
            "mhtml",
            "ie.http",
            "res",
            "ms-excel",
            "ms-word",
            "ms-powerpoint",
        ] {
            assert!(NEVER_SCHEMES.contains(&must), "{must} is on the list");
        }
        // An Office scheme's command forms go with the scheme.
        assert_eq!(
            external_scheme("ms-word:ofe|u|https://evil.example/a.docx"),
            ExternalScheme::Never
        );
        assert_eq!(
            external_scheme("ms-excel:ofv|u|\\\\evil\\share\\a.xlsx"),
            ExternalScheme::Never
        );
        assert_eq!(external_scheme("about:srcdoc"), ExternalScheme::Never);
    }

    #[test]
    fn long_odd_and_path_like_text_is_never_offered() {
        let long = format!("bankid:///?t={}", "a".repeat(APP_LINK_MAX));
        assert_eq!(external_scheme(&long), ExternalScheme::Never);
        let just = format!("bankid:{}", "a".repeat(APP_LINK_MAX - 7));
        assert_eq!(just.chars().count(), APP_LINK_MAX);
        assert_eq!(external_scheme(&just), offer("bankid"));
        for bad in [
            "",
            "bankid:x\u{0}y",
            "bankid:x\ny",
            "bankid:a b",
            "mailto:\"a\"\t@b",
            "C:\\Windows\\system32\\calc.exe",
            "c:/Windows/x.exe",
            "\\\\host\\share\\x.exe",
            "/etc/x",
            "./x:y",
            "nocolon",
            "1abc:x",
            "-abc:x",
            "bad_scheme:x",
            "b\u{e5}nk:x",
            ":x",
            "foo:\\\\host\\x",
            &format!("{}:x", "a".repeat(33)),
        ] {
            assert_eq!(external_scheme(bad), ExternalScheme::Never, "{bad:?}");
        }
    }

    #[test]
    fn a_page_may_open_its_own_blob_and_no_one_elses() {
        let page = "https://shop.example/order/7";
        assert!(own_blob("blob:https://shop.example/1b2c-3d", page));
        assert!(own_blob("BLOB:https://shop.example/1b2c-3d", page));
        assert!(
            !own_blob("blob:https://evil.example/1b2c", page),
            "another site's"
        );
        assert!(
            !own_blob("blob:http://shop.example/1b2c", page),
            "another scheme is another origin"
        );
        assert!(
            !own_blob("blob:https://shop.example:444/1b2c", page),
            "another port"
        );
        assert!(!own_blob("blob:null/1b2c", page));
        assert!(!own_blob("blob:https://shop.example/1", "about:blank"));
        assert!(!own_blob("blob:https://shop.example/1", "data:text/html,x"));
        assert!(!own_blob("https://shop.example/1", page), "not a blob");
        assert!(!own_blob("blob:file:///C:/x", "file:///C:/y"));
        // Still refused for the address bar and an agent's navigate.
        assert_eq!(
            address_or_search("blob:https://shop.example/1"),
            Err(UrlError::Scheme)
        );
        assert_eq!(
            normalize_url("blob:https://shop.example/1"),
            Err(UrlError::Scheme)
        );
    }

    #[test]
    fn the_same_app_link_told_twice_within_a_second_is_told_once() {
        let mut d = AppLinkDedupe::default();
        assert!(d.fresh("bankid:///x", 0));
        assert!(
            !d.fresh("bankid:///x", 999),
            "the launch after the navigation"
        );
        assert!(d.fresh("swish://y", 1_000), "another link is new");
        assert!(d.fresh("bankid:///x", 1_100));
        assert!(
            d.fresh("bankid:///x", 2_100),
            "a second later it is a new click"
        );
    }
}

#[cfg(test)]
mod inset_tests {
    use super::*;

    fn slot(x: i32, w: i32) -> PhysRect {
        PhysRect {
            x,
            y: 40,
            w,
            h: 600,
        }
    }

    #[test]
    fn a_held_page_keeps_its_width_while_its_left_edge_follows_the_slot() {
        let mut d = DragHold::new(500);
        for (x, w) in [(1000, 480), (1040, 440), (900, 580)] {
            let (r, settle) = d.step(slot(x, w));
            assert_eq!(r, slot(x, 500), "the width is held, the edge is the slot's");
            assert_eq!(settle, DragSettle::Restart);
        }
    }

    #[test]
    fn a_held_page_takes_the_slots_width_at_a_pause_and_holds_that_one_next() {
        let mut d = DragHold::new(500);
        d.step(slot(1000, 480));
        assert!(d.settle());
        let (r, settle) = d.step(slot(1000, 480));
        assert_eq!(r, slot(1000, 480), "the page is at the slot now");
        assert_eq!(settle, DragSettle::Keep, "the same place is no new step");
        assert!(!d.settle(), "a second pause changes nothing");
        assert_eq!(d.step(slot(960, 520)).0, slot(960, 480));
    }

    #[test]
    fn a_slot_back_at_the_pages_width_owes_no_settle() {
        let mut d = DragHold::new(500);
        d.step(slot(1000, 480));
        let (r, settle) = d.step(slot(980, 500));
        assert_eq!(r, slot(980, 500));
        assert_eq!(settle, DragSettle::Cancel);
        assert!(!d.settle());
    }

    #[test]
    fn a_hold_with_no_step_has_nothing_to_settle() {
        assert!(!DragHold::new(500).settle());
    }

    #[test]
    fn the_inset_is_half_the_grab_zone_at_every_scale() {
        assert_eq!(web_inset_px(1.0), 4);
        assert_eq!(web_inset_px(1.5), 6);
        assert_eq!(web_inset_px(2.25), 9);
        assert_eq!(web_inset_px(0.0), 4);
        assert_eq!(web_inset_px(f64::NAN), 4);
    }
}
