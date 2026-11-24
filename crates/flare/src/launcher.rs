//! The panel launcher (2026-11-24, rules 1372 to 1376): the round ghost button in the top right of a tab's terminal
//! area while its side panel is closed, the menu it blooms into, and the panel header's "Open in" button.
//!
//! Everything here is pure and takes its time from the caller, so the tests hold the hover rules without a window or
//! a clock: `appctl/launcher_glue.rs` feeds it the pointer, the keys and the timer, and pushes what it says.

use crate::panel_ctl::PanelSnap;

/// How long the pointer may be off both the button and the menu before the menu closes (Max, 2026-11-24).
pub const GRACE_MS: u64 = 250;

/// The hover rules of the launcher's menu (rule 1373). The menu opens only on the pointer entering the BUTTON (or a
/// click, or the keys); a closed menu takes no pointer, so the pointer passing where the menu would be does nothing.
/// The menu's invisible bridge over the gap belongs to the menu. Leaving the button or the menu starts a grace;
/// entering either cancels it; once it has run out with the pointer on neither, the menu closes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hover {
    open: bool,
    on_button: bool,
    on_menu: bool,
    /// When the grace runs out, while one runs.
    close_at: Option<u64>,
    /// The menu holds the keys (opened by a click or the keys, not by the pointer alone).
    keyed: bool,
}

impl Hover {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The menu holds the arrows, Enter and Escape (it was opened by a click or the keys).
    pub fn keyed(&self) -> bool {
        self.open && self.keyed
    }

    /// When the running grace ends; None while none runs.
    pub fn due(&self) -> Option<u64> {
        self.close_at
    }

    fn open_now(&mut self, keyed: bool) -> bool {
        self.close_at = None;
        if self.open {
            self.keyed |= keyed;
            return false;
        }
        self.open = true;
        self.keyed = keyed;
        true
    }

    /// The pointer came onto (`on`) or left the button. True when the menu opened.
    pub fn button(&mut self, on: bool, now: u64) -> bool {
        self.on_button = on;
        if on {
            return self.open_now(false);
        }
        self.start_grace(now);
        false
    }

    /// The pointer came onto (`on`) or left the menu or its bridge. A closed menu takes no pointer: this does nothing
    /// then. Never opens the menu.
    pub fn menu(&mut self, on: bool, now: u64) {
        if !self.open {
            self.on_menu = false;
            return;
        }
        self.on_menu = on;
        if on {
            self.close_at = None;
        } else {
            self.start_grace(now);
        }
    }

    fn start_grace(&mut self, now: u64) {
        if self.open && !self.on_button && !self.on_menu {
            self.close_at = Some(now + GRACE_MS);
        }
    }

    /// A click on the button (or its accessible action): a closed menu opens with the keys; a menu the pointer opened
    /// takes the keys and stays; a menu a click or the keys opened closes. True when the menu opened or closed.
    pub fn click(&mut self) -> bool {
        if self.open && self.keyed {
            return self.close();
        }
        if self.open {
            self.keyed = true;
            self.close_at = None;
            return false;
        }
        self.open_now(true)
    }

    /// Closes the menu (Escape, a pick, the button going away). True when it was open.
    pub fn close(&mut self) -> bool {
        let was = self.open;
        self.open = false;
        self.keyed = false;
        self.close_at = None;
        self.on_menu = false;
        was
    }

    /// The clock moved on: the menu closes once its grace has run out with the pointer on neither part. True when it
    /// closed.
    pub fn tick(&mut self, now: u64) -> bool {
        match self.close_at {
            Some(at) if now >= at && !self.on_button && !self.on_menu => self.close(),
            _ => false,
        }
    }
}

/// What decides whether the launcher shows on the tab in front (rule 1372).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Where {
    /// The tab in front is a shell tab (not Settings, the start screen or a placeholder).
    pub shell_front: bool,
    /// Its side panel is open.
    pub panel_open: bool,
    /// The window is wide enough for a panel slot at all.
    pub panel_fits: bool,
    /// A remote tab's fitted conversation view covers the terminal.
    pub remote_view: bool,
    /// The resume skeleton covers the terminal.
    pub skeleton: bool,
    /// The find bar is open: it sits in the same top right corner (review fix, 2026-11-24).
    pub find_open: bool,
}

/// The launcher shows only where a panel could open and none is open: a shell tab in front, its panel closed, room
/// for a slot, and nothing drawn over the terminal or in its top right corner (the find bar).
pub fn shown(w: Where) -> bool {
    w.shell_front && !w.panel_open && w.panel_fits && !w.remote_view && !w.skeleton && !w.find_open
}

/// One row of the launcher's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Browser,
    Device,
    File,
    Last,
}

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::Browser => "Browser",
            Item::Device => "Device",
            Item::File => "Open a file\u{2026}",
            Item::Last => "Last panel",
        }
    }

    /// The row's stroke icon on a 24 unit viewbox (the mockup's `globe`, `phone`, `file` and `recent`).
    pub fn icon(self) -> &'static str {
        match self {
            Item::Browser => {
                "M20.5 12a8.5 8.5 0 1 1-17 0a8.5 8.5 0 1 1 17 0zM3.5 12h17M12 3.5c2.6 2.5 3.8 5.3 3.8 8.5s-1.2 6-3.8 8.5M12 3.5C9.4 6 8.2 8.8 8.2 12s1.2 6 3.8 8.5"
            }
            Item::Device => {
                "M9.4 3h5.2a2.4 2.4 0 0 1 2.4 2.4v13.2a2.4 2.4 0 0 1-2.4 2.4H9.4A2.4 2.4 0 0 1 7 18.6V5.4A2.4 2.4 0 0 1 9.4 3zM10.5 6h3"
            }
            Item::File => "M6 3.5h8l4 4v13H6zM14 3.5v4h4",
            Item::Last => "M20.5 12a8.5 8.5 0 1 1-17 0a8.5 8.5 0 1 1 17 0zM12 7.5V12l3 2",
        }
    }
}

/// What the menu may offer on the tab in front.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    /// This machine offers the device panel.
    pub device: bool,
    /// The tab's folder is on this machine (a remote tab's is not: no Open dialog there).
    pub file: bool,
    /// The tab's panel showed something it can bring back (`last_ok`).
    pub last: bool,
}

/// The menu's rows, top to bottom: Browser, Device, Open a file..., then a rule and Last panel. A row that has nothing
/// to do is left out, never shown dimmed.
pub fn items(o: Offer) -> Vec<Item> {
    let mut v = vec![Item::Browser];
    if o.device {
        v.push(Item::Device);
    }
    if o.file {
        v.push(Item::File);
    }
    if o.last {
        v.push(Item::Last);
    }
    v
}

/// The menu's padding, one row's height, and the room a rule takes above the row it starts (3 + 1 + 3), in css px.
/// `launcher.slint` lays the rows out with the same numbers.
pub const PAD: f32 = 4.0;
pub const ROW_H: f32 = 28.0;
pub const RULE_H: f32 = 7.0;

/// A row starts a group: a rule sits above it (Last panel only).
pub fn ruled(item: Item) -> bool {
    item == Item::Last
}

/// Each row's top and height in the menu, in css px from the menu's top edge (the rule is part of its row).
pub fn tops(items: &[Item]) -> Vec<(f32, f32)> {
    let mut y = PAD;
    items
        .iter()
        .map(|i| {
            let h = ROW_H + if ruled(*i) { RULE_H } else { 0.0 };
            let at = (y, h);
            y += h;
            at
        })
        .collect()
}

/// The menu's height in css px (its border lies inside it).
pub fn menu_h(items: &[Item]) -> f32 {
    tops(items).last().map_or(2.0 * PAD, |(t, h)| t + h + PAD)
}

/// The row under `y` (css px from the menu's top edge), or None for the padding, a rule or outside.
pub fn row_at(items: &[Item], y: f32) -> Option<usize> {
    tops(items).iter().zip(items).position(|((top, h), item)| {
        let row_top = top + if ruled(*item) { RULE_H } else { 0.0 };
        y >= row_top && y < top + h
    })
}

/// The next row the keys land on from `from` (-1: none yet), `step` rows on, wrapping.
pub fn step(len: usize, from: i32, step: i32) -> i32 {
    if len == 0 {
        return -1;
    }
    let n = len as i32;
    if from < 0 {
        return if step >= 0 { 0 } else { n - 1 };
    }
    (from + step).rem_euclid(n)
}

/// What Last panel can bring back (rule 1375): a closed panel that showed a page address, a device or a file. Not a
/// note or text handed over (no path), the Changes view, nor an empty page that never went anywhere. A mini-app or a
/// mod's page is forgotten when its panel closes, so it never reaches here.
pub fn last_ok(snap: Option<&PanelSnap>) -> bool {
    let Some(s) = snap else { return false };
    if s.open {
        return false;
    }
    match (&s.url, &s.path) {
        (Some(u), _) => u != "about:blank" && !u.is_empty(),
        (None, Some(_)) => true,
        (None, None) => s.kind == "device",
    }
}

/// Where the panel header's "Open in" button sends what the panel shows (rule 1376).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenIn {
    /// A web page: the default browser, at this address (http or https only).
    Url(String),
    /// A file: its default app. `reveal` when the file is not on the click allowlist (rule 14): it is then shown in
    /// its folder instead, never run.
    File { path: String, reveal: bool },
}

/// What "Open in" does for the panel the tab shows; None where it has no button: the device panel, the Changes view,
/// a mini-app or a mod's page (no address of the person's), text handed over (no file), and a page that is not http
/// or https (about:blank, a file: address).
pub fn open_in(snap: Option<&PanelSnap>) -> Option<OpenIn> {
    let s = snap?;
    if !s.open {
        return None;
    }
    if let Some(u) = &s.url {
        return flare_term::web_url::is_web_url(u).then(|| OpenIn::Url(u.clone()));
    }
    let path = s.path.as_ref()?;
    Some(OpenIn::File {
        path: path.clone(),
        reveal: !flare_term::path_open::may_open(path),
    })
}

/// The button's words: "Open in browser", "Open in <app>" when the default app's name is known, else "Open in default
/// app", and for a file that is revealed "Show in Explorer" (Finder on a Mac). `compact` (a narrow panel) keeps only
/// the verb, "Open" or "Show".
pub fn open_in_label(o: &OpenIn, app: Option<&str>, mac: bool, compact: bool) -> String {
    match o {
        OpenIn::Url(_) if compact => "Open".to_string(),
        OpenIn::Url(_) => "Open in browser".to_string(),
        OpenIn::File { reveal: true, .. } if compact => "Show".to_string(),
        OpenIn::File { reveal: true, .. } if mac => "Show in Finder".to_string(),
        OpenIn::File { reveal: true, .. } => "Show in Explorer".to_string(),
        OpenIn::File { .. } if compact => "Open".to_string(),
        OpenIn::File { .. } => match app {
            Some(name) => format!("Open in {name}"),
            None => "Open in default app".to_string(),
        },
    }
}

/// The extension that keys the app-name cache: lower case, None for a name with none.
pub fn ext_key(path: &str) -> Option<String> {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let dot = name.rfind('.')?;
    let ext = &name[dot + 1..];
    (!ext.is_empty() && dot > 0).then(|| ext.to_lowercase())
}

/// The longest app name the button shows; a longer one is cut with an ellipsis.
pub const APP_NAME_MAX: usize = 32;

/// A default app's name as the system gave it, made fit for the button: control characters gone, spaces trimmed, at
/// most `APP_NAME_MAX` characters. None for nothing left, or a name that is a program's file name (the system had no
/// friendly name), which reads worse than "default app".
pub fn clean_app_name(raw: &str) -> Option<String> {
    let one: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let t = one.trim();
    if t.is_empty() || t.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    if t.chars().count() > APP_NAME_MAX {
        let cut: String = t.chars().take(APP_NAME_MAX - 1).collect();
        return Some(format!("{}\u{2026}", cut.trim_end()));
    }
    Some(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(kind: &str, path: Option<&str>, url: Option<&str>, open: bool) -> PanelSnap {
        PanelSnap {
            kind: kind.to_string(),
            title: String::new(),
            path: path.map(str::to_string),
            url: url.map(str::to_string),
            network: None,
            open,
            width: None,
        }
    }

    // ---- the hover rules (rule 1373) ----

    #[test]
    fn only_the_button_opens_the_menu_and_a_closed_menu_takes_no_pointer() {
        let mut h = Hover::default();
        // The pointer where the menu would be: nothing.
        h.menu(true, 0);
        assert!(!h.is_open());
        h.menu(false, 10);
        assert!(!h.is_open());
        assert_eq!(h.due(), None, "no grace runs for a closed menu");
        // Onto the button: it opens, without the keys.
        assert!(h.button(true, 20));
        assert!(h.is_open());
        assert!(!h.keyed());
    }

    #[test]
    fn crossing_the_gap_from_the_button_to_the_menu_keeps_it_open() {
        let mut h = Hover::default();
        h.button(true, 0);
        // Off the button: the grace starts.
        h.button(false, 100);
        assert_eq!(h.due(), Some(100 + GRACE_MS));
        // Onto the bridge 40 ms later: the grace is cancelled.
        h.menu(true, 140);
        assert_eq!(h.due(), None);
        assert!(!h.tick(100 + GRACE_MS + 1));
        assert!(h.is_open());
        // The other order (entered before left, in one dispatch) never starts a grace at all.
        let mut g = Hover::default();
        g.button(true, 0);
        g.menu(true, 50);
        g.button(false, 50);
        assert_eq!(g.due(), None);
    }

    #[test]
    fn off_both_for_the_grace_closes_it_and_coming_back_in_time_does_not() {
        let mut h = Hover::default();
        h.button(true, 0);
        h.menu(true, 10);
        h.button(false, 10);
        h.menu(false, 200);
        assert!(
            !h.tick(200 + GRACE_MS - 1),
            "249 ms is still inside the grace"
        );
        // Back onto the menu before it runs out.
        h.menu(true, 200 + GRACE_MS - 1);
        assert!(!h.tick(10_000));
        assert!(h.is_open());
        // Off again, and this time the grace runs out.
        h.menu(false, 20_000);
        assert!(!h.tick(20_000 + GRACE_MS - 1));
        assert!(h.tick(20_000 + GRACE_MS));
        assert!(!h.is_open());
        // Back onto the button: it opens again.
        assert!(h.button(true, 30_000));
    }

    #[test]
    fn a_click_opens_with_the_keys_takes_the_keys_from_a_hover_and_closes_a_keyed_menu() {
        let mut h = Hover::default();
        assert!(h.click());
        assert!(h.keyed());
        assert!(h.click(), "a second click closes it");
        assert!(!h.is_open());
        // Opened by the pointer: a click keeps it open and gives it the keys.
        h.button(true, 0);
        assert!(!h.click());
        assert!(h.is_open() && h.keyed());
        assert!(h.click());
        assert!(!h.is_open());
    }

    #[test]
    fn escape_and_a_pick_close_it_and_a_closed_menu_has_no_grace() {
        let mut h = Hover::default();
        h.button(true, 0);
        h.button(false, 5);
        assert!(h.close());
        assert_eq!(h.due(), None);
        assert!(!h.close(), "closing twice changes nothing");
        assert!(!h.tick(10_000));
    }

    // ---- where it shows (rule 1372) ----

    #[test]
    fn the_button_shows_only_where_a_panel_could_open_and_none_is_open() {
        let ok = Where {
            shell_front: true,
            panel_fits: true,
            ..Where::default()
        };
        assert!(shown(ok));
        assert!(!shown(Where {
            panel_open: true,
            ..ok
        }));
        assert!(!shown(Where {
            panel_fits: false,
            ..ok
        }));
        assert!(!shown(Where {
            shell_front: false,
            ..ok
        }));
        assert!(!shown(Where {
            remote_view: true,
            ..ok
        }));
        assert!(!shown(Where {
            skeleton: true,
            ..ok
        }));
        assert!(!shown(Where {
            find_open: true,
            ..ok
        }));
    }

    // ---- the rows (rule 1374) ----

    #[test]
    fn the_rows_follow_what_the_machine_and_the_tab_offer() {
        let all = Offer {
            device: true,
            file: true,
            last: true,
        };
        assert_eq!(
            items(all),
            vec![Item::Browser, Item::Device, Item::File, Item::Last]
        );
        assert_eq!(items(Offer::default()), vec![Item::Browser]);
        assert_eq!(
            items(Offer {
                file: true,
                ..Offer::default()
            }),
            vec![Item::Browser, Item::File]
        );
        assert_eq!(Item::File.label(), "Open a file\u{2026}");
    }

    #[test]
    fn the_pointer_picks_the_row_it_is_over_never_the_padding_or_the_rule() {
        let rows = items(Offer {
            device: true,
            file: true,
            last: true,
        });
        // 4 padding, then 28 px rows; the Last row has its 7 px rule above it.
        assert_eq!(
            tops(&rows),
            vec![(4.0, 28.0), (32.0, 28.0), (60.0, 28.0), (88.0, 35.0)]
        );
        assert_eq!(menu_h(&rows), 4.0 + 3.0 * 28.0 + 35.0 + 4.0);
        assert_eq!(row_at(&rows, 0.0), None, "the padding");
        assert_eq!(row_at(&rows, 4.0), Some(0));
        assert_eq!(row_at(&rows, 31.9), Some(0));
        assert_eq!(row_at(&rows, 32.0), Some(1));
        assert_eq!(row_at(&rows, 87.0), Some(2));
        assert_eq!(row_at(&rows, 90.0), None, "the rule");
        assert_eq!(row_at(&rows, 95.0), Some(3));
        assert_eq!(row_at(&rows, 122.9), Some(3));
        assert_eq!(row_at(&rows, 123.0), None);
        assert_eq!(row_at(&rows, -6.0), None, "the bridge over the gap");
        assert_eq!(menu_h(&[]), 8.0);
    }

    #[test]
    fn the_arrows_wrap_and_start_at_either_end() {
        assert_eq!(step(4, -1, 1), 0);
        assert_eq!(step(4, -1, -1), 3);
        assert_eq!(step(4, 3, 1), 0);
        assert_eq!(step(4, 0, -1), 3);
        assert_eq!(step(4, 1, 1), 2);
        assert_eq!(step(0, 0, 1), -1);
    }

    // ---- Last panel (rule 1375) ----

    #[test]
    fn last_panel_brings_back_a_page_a_device_or_a_file_and_nothing_else() {
        assert!(last_ok(Some(&snap(
            "web",
            None,
            Some("https://example.com/"),
            false
        ))));
        assert!(last_ok(Some(&snap("device", None, None, false))));
        assert!(last_ok(Some(&snap(
            "image",
            Some("C:\\a.png"),
            None,
            false
        ))));
        assert!(last_ok(Some(&snap("text", Some("C:\\a.md"), None, false))));
        // Not while it shows, not an empty page, not a note, not the Changes view, not nothing.
        assert!(!last_ok(Some(&snap(
            "image",
            Some("C:\\a.png"),
            None,
            true
        ))));
        assert!(!last_ok(Some(&snap(
            "web",
            None,
            Some("about:blank"),
            false
        ))));
        assert!(!last_ok(Some(&snap("text", None, None, false))));
        assert!(!last_ok(Some(&snap("changes", None, None, false))));
        assert!(!last_ok(None));
    }

    // ---- Open in (rule 1376) ----

    #[test]
    fn open_in_sends_a_page_to_the_browser_and_a_file_to_its_app() {
        assert_eq!(
            open_in(Some(&snap(
                "web",
                None,
                Some("https://example.com/a"),
                true
            ))),
            Some(OpenIn::Url("https://example.com/a".to_string()))
        );
        assert_eq!(
            open_in(Some(&snap("image", Some("C:\\p\\a.png"), None, true))),
            Some(OpenIn::File {
                path: "C:\\p\\a.png".to_string(),
                reveal: false
            })
        );
        // A text file a click would never open (a script) is shown in its folder, never run.
        assert_eq!(
            open_in(Some(&snap("text", Some("C:\\p\\run.ps1"), None, true))),
            Some(OpenIn::File {
                path: "C:\\p\\run.ps1".to_string(),
                reveal: true
            })
        );
    }

    #[test]
    fn open_in_has_no_button_where_nothing_outside_flare_shows_it() {
        // Only http and https reach the browser (the links' allowlist).
        for url in [
            "about:blank",
            "file:///C:/a.html",
            "javascript:alert(1)",
            "ftp://x/",
        ] {
            assert_eq!(
                open_in(Some(&snap("web", None, Some(url), true))),
                None,
                "{url}"
            );
        }
        // The device, the Changes view, a mini-app or a mod (no url, no path), text handed over, a closed panel.
        assert_eq!(open_in(Some(&snap("device", None, None, true))), None);
        assert_eq!(open_in(Some(&snap("changes", None, None, true))), None);
        assert_eq!(open_in(Some(&snap("app", None, None, true))), None);
        assert_eq!(open_in(Some(&snap("mod:x", None, None, true))), None);
        assert_eq!(open_in(Some(&snap("text", None, None, true))), None);
        assert_eq!(
            open_in(Some(&snap("image", Some("C:\\a.png"), None, false))),
            None
        );
        assert_eq!(open_in(None), None);
    }

    #[test]
    fn the_button_names_the_app_when_known_and_shortens_when_narrow() {
        let url = OpenIn::Url("https://x/".into());
        let file = OpenIn::File {
            path: "C:\\a.png".into(),
            reveal: false,
        };
        let shown = OpenIn::File {
            path: "C:\\a.ps1".into(),
            reveal: true,
        };
        assert_eq!(open_in_label(&url, None, false, false), "Open in browser");
        assert_eq!(
            open_in_label(&file, Some("Photos"), false, false),
            "Open in Photos"
        );
        assert_eq!(
            open_in_label(&file, None, false, false),
            "Open in default app"
        );
        assert_eq!(
            open_in_label(&shown, None, false, false),
            "Show in Explorer"
        );
        assert_eq!(open_in_label(&shown, None, true, false), "Show in Finder");
        assert_eq!(open_in_label(&url, None, false, true), "Open");
        assert_eq!(open_in_label(&file, Some("Photos"), false, true), "Open");
        assert_eq!(open_in_label(&shown, None, false, true), "Show");
    }

    #[test]
    fn an_app_name_is_cleaned_and_capped_and_a_bare_exe_is_no_name() {
        assert_eq!(clean_app_name("  Photos\r\n"), Some("Photos".to_string()));
        assert_eq!(clean_app_name("Paint\u{7}3D"), Some("Paint 3D".to_string()));
        assert_eq!(clean_app_name(""), None);
        assert_eq!(clean_app_name(" \t "), None);
        assert_eq!(clean_app_name("NOTEPAD.EXE"), None);
        let long = "A".repeat(80);
        let got = clean_app_name(&long).unwrap();
        assert_eq!(got.chars().count(), APP_NAME_MAX);
        assert!(got.ends_with('\u{2026}'));
    }

    #[test]
    fn the_cache_key_is_the_lower_case_extension() {
        assert_eq!(ext_key("C:\\a\\Photo.PNG").as_deref(), Some("png"));
        assert_eq!(ext_key("/u/me/notes.md").as_deref(), Some("md"));
        assert_eq!(ext_key("C:\\a\\Makefile"), None);
        assert_eq!(ext_key("C:\\a\\.env"), None);
        assert_eq!(ext_key("C:\\a.b\\file"), None);
        assert_eq!(ext_key("C:\\a\\x."), None);
    }
}
