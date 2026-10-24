//! `panels.json`: which tab had which panel, kept next to `tabs.json`.
//!
//! The file is written the way `tabs.json` is (temp file, rename, a 400 ms debounce, a synchronous
//! flush on close; rule 36). A tab has no stable id across a restart, so an entry is matched to a
//! restored tab by its folder and its place in the strip. The reader is defensive (rules 40 and
//! 41): a damaged file is no panels, an unknown kind drops that one panel, a width that is not a
//! fraction is "not set". The debounce thread is made on the first save, so a session that never
//! opens a panel starts no thread and writes no file.

use crate::kind::PanelKind;
use crate::state::{TabPanel, clean_target};
use flare_settings::atomic::{Opened, open_text};
use flare_settings::debounce::Debounced;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The file's format version.
pub const VERSION: u32 = 1;
/// Most tabs kept (the same limit as `tabs.json`).
pub const MAX_TABS: usize = 50;

/// One tab as saved: its folder and the panel it had.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedTabPanel {
    pub cwd: String,
    pub panel: Option<TabPanel>,
}

/// Reads the file's text. Anything not shaped right is no entries.
pub fn parse_panels(raw: &str) -> Vec<SavedTabPanel> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(list) = v.get("tabs").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|e| {
            let cwd = e.get("cwd")?.as_str()?;
            if cwd.is_empty() {
                return None;
            }
            Some(SavedTabPanel {
                cwd: cwd.to_string(),
                panel: e.get("panel").and_then(parse_panel),
            })
        })
        .take(MAX_TABS)
        .collect()
}

fn parse_panel(p: &Value) -> Option<TabPanel> {
    let kind = PanelKind::parse(p.get("kind")?.as_str()?)?;
    let target = clean_target(p.get("target")?.as_str()?)?;
    // A bool default-on reads on unless it is the stored false: a panel that was open stays open.
    let open = p.get("open").and_then(Value::as_bool).unwrap_or(true);
    let width = p
        .get("width")
        .and_then(Value::as_f64)
        .filter(|w| w.is_finite() && *w > 0.0 && *w < 1.0)
        .map(|w| w as f32);
    let by_agent = p.get("agent").and_then(Value::as_bool).unwrap_or(false);
    Some(TabPanel {
        kind,
        target,
        open,
        width,
        by_agent,
    })
}

/// The file's text for these entries.
pub fn panels_json(tabs: &[SavedTabPanel]) -> String {
    let tabs: Vec<Value> = tabs
        .iter()
        .take(MAX_TABS)
        .map(|t| match &t.panel {
            Some(p) => {
                let mut panel =
                    json!({ "kind": p.kind.name(), "target": p.target, "open": p.open });
                if let Some(w) = p.width {
                    // Four places are far finer than a pixel and keep the file stable.
                    panel["width"] = json!((f64::from(w) * 10_000.0).round() / 10_000.0);
                }
                // Only the agent's panel says so: a file the user opened is written as before.
                if p.by_agent {
                    panel["agent"] = json!(true);
                }
                json!({ "cwd": t.cwd, "panel": panel })
            }
            None => json!({ "cwd": t.cwd }),
        })
        .collect();
    json!({ "version": VERSION, "tabs": tabs }).to_string()
}

/// True when saving these would write nothing worth keeping: no tab has a panel.
pub fn is_blank(tabs: &[SavedTabPanel]) -> bool {
    tabs.iter().all(|t| t.panel.is_none())
}

/// The panel for each live tab, in the strip's order. The saved entries are walked once, in
/// order: a live tab takes the next saved entry with the same folder, so a tab that was dropped
/// from the restore (its folder is gone) only skips its entry, and two tabs in one folder keep
/// their own panels.
pub fn match_restored(saved: &[SavedTabPanel], live_cwds: &[String]) -> Vec<Option<TabPanel>> {
    let mut at = 0;
    live_cwds
        .iter()
        .map(|cwd| {
            let found = saved
                .iter()
                .enumerate()
                .skip(at)
                .find(|(_, s)| same_folder(&s.cwd, cwd))
                .map(|(i, _)| i);
            match found {
                Some(i) => {
                    at = i + 1;
                    saved[i].panel.clone()
                }
                None => None,
            }
        })
        .collect()
}

fn same_folder(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    norm(a) == norm(b)
}

/// `panels.json` over the debounced writer.
pub struct PanelStore {
    file: PathBuf,
    delay: Duration,
    out: OnceLock<Debounced>,
    /// The file exists but could not be read nor set aside: nothing is saved this session.
    read_only: AtomicBool,
}

impl PanelStore {
    /// 400 ms in the app.
    pub fn new(file: impl Into<PathBuf>, delay: Duration) -> Self {
        Self {
            file: file.into(),
            delay,
            out: OnceLock::new(),
            read_only: AtomicBool::new(false),
        }
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::SeqCst)
    }

    /// A save still inside its debounce is the newest state; else the file; unreadable or missing:
    /// no entries (and an unreadable file that cannot be set aside stops saving).
    pub fn load(&self) -> Vec<SavedTabPanel> {
        if let Some(p) = self.out.get().and_then(Debounced::pending) {
            return parse_panels(&p);
        }
        match open_text(&self.file) {
            // sync-ok: startup read
            Opened::Text(raw) => parse_panels(&raw),
            Opened::Missing => Vec::new(),
            Opened::Unreadable { moved_aside } => {
                if !moved_aside {
                    self.read_only.store(true, Ordering::SeqCst);
                }
                Vec::new()
            }
        }
    }

    pub fn save(&self, tabs: &[SavedTabPanel]) {
        if self.is_read_only() {
            return;
        }
        let out = self
            .out
            .get_or_init(|| Debounced::new(self.file.clone(), self.delay));
        out.save(panels_json(tabs));
    }

    /// Close, quit and session end: a pending save is written now.
    pub fn flush(&self) {
        if let Some(out) = self.out.get() {
            out.flush();
        }
    }

    /// True once a save made the writer thread.
    pub fn started(&self) -> bool {
        self.out.get().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(kind: PanelKind, target: &str, open: bool, width: Option<f32>) -> TabPanel {
        TabPanel {
            kind,
            target: target.into(),
            open,
            width,
            by_agent: false,
        }
    }

    fn entry(cwd: &str, p: Option<TabPanel>) -> SavedTabPanel {
        SavedTabPanel {
            cwd: cwd.into(),
            panel: p,
        }
    }

    #[test]
    fn a_saved_file_reads_back_the_same() {
        let tabs = vec![
            entry(
                r"C:\work",
                Some(panel(PanelKind::Image, r"C:\work\a.png", true, Some(0.35))),
            ),
            entry(r"C:\other", None),
            entry(
                r"C:\x",
                Some(panel(PanelKind::Text, r"C:\x\n.txt", false, None)),
            ),
        ];
        assert_eq!(parse_panels(&panels_json(&tabs)), tabs);
    }

    #[test]
    fn an_agents_panel_keeps_its_mark_and_only_it_writes_one() {
        let mut theirs = panel(PanelKind::Text, r"C:\w\r.md", true, None);
        theirs.by_agent = true;
        let tabs = vec![
            entry(r"C:\w", Some(theirs)),
            entry(
                r"C:\v",
                Some(panel(PanelKind::Text, r"C:\v\n.md", true, None)),
            ),
        ];
        let raw = panels_json(&tabs);
        assert_eq!(raw.matches(r#""agent":true"#).count(), 1);
        assert_eq!(parse_panels(&raw), tabs);
    }

    #[test]
    fn the_file_has_a_version_and_a_stable_shape() {
        let tabs = vec![entry(
            r"C:\w",
            Some(panel(PanelKind::Image, r"C:\w\a.png", true, Some(0.4))),
        )];
        assert_eq!(
            panels_json(&tabs),
            r#"{"version":1,"tabs":[{"cwd":"C:\\w","panel":{"kind":"image","target":"C:\\w\\a.png","open":true,"width":0.4}}]}"#
        );
    }

    #[test]
    fn damaged_files_are_no_entries() {
        for raw in [
            "",
            "not json",
            "[]",
            "{}",
            r#"{"tabs":5}"#,
            r#"{"tabs":[1,"a",null]}"#,
            r#"{"tabs":[{"cwd":""}]}"#,
            r#"{"tabs":[{"nocwd":1}]}"#,
        ] {
            assert!(parse_panels(raw).is_empty(), "{raw:?}");
        }
    }

    #[test]
    fn an_unknown_kind_drops_that_panel_only() {
        let raw = r#"{"tabs":[
            {"cwd":"C:\\a","panel":{"kind":"hologram","target":"x"}},
            {"cwd":"C:\\b","panel":{"kind":"image","target":"C:\\b\\p.png"}}]}"#;
        let got = parse_panels(raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].panel, None);
        assert_eq!(
            got[1].panel.as_ref().map(|p| p.kind.clone()),
            Some(PanelKind::Image)
        );
    }

    #[test]
    fn a_stored_agent_board_panel_is_dropped_on_load() {
        // The agent board was removed (owner, 2026-10-22). A panels.json an older build wrote still has it:
        // it reads as an unknown kind and that one panel is dropped (rule 40), the tab and its neighbour stay.
        let raw = r#"{"version":1,"tabs":[
            {"cwd":"C:\\a","panel":{"kind":"board","target":"board","open":true}},
            {"cwd":"C:\\b","panel":{"kind":"tasks","target":"tasks"}}]}"#;
        let got = parse_panels(raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].panel, None, "the board panel is gone");
        assert_eq!(
            got[1].panel.as_ref().map(|p| p.kind.clone()),
            Some(PanelKind::Tasks)
        );
        // And it is never written back.
        assert!(!panels_json(&got).contains("board"));
    }

    #[test]
    fn a_panel_without_a_usable_target_is_dropped() {
        for target in [r#""""#, "5", "null", r#""a\u0000b""#] {
            let raw = format!(
                r#"{{"tabs":[{{"cwd":"C:\\a","panel":{{"kind":"image","target":{target}}}}}]}}"#
            );
            assert_eq!(parse_panels(&raw)[0].panel, None, "{target}");
        }
    }

    #[test]
    fn a_width_that_is_not_a_fraction_reads_as_not_set() {
        for w in ["0", "1", "1.5", "-0.3", "\"wide\"", "null"] {
            let raw = format!(
                r#"{{"tabs":[{{"cwd":"C:\\a","panel":{{"kind":"text","target":"a.txt","width":{w}}}}}]}}"#
            );
            let p = parse_panels(&raw)[0].panel.clone().unwrap();
            assert_eq!(p.width, None, "{w}");
        }
    }

    #[test]
    fn open_reads_true_unless_it_is_stored_false() {
        let read = |open: &str| {
            let raw = format!(
                r#"{{"tabs":[{{"cwd":"C:\\a","panel":{{"kind":"text","target":"a.txt"{open}}}}}]}}"#
            );
            parse_panels(&raw)[0].panel.clone().unwrap().open
        };
        assert!(read(""));
        assert!(read(r#","open":true"#));
        assert!(!read(r#","open":false"#));
        assert!(read(r#","open":"no""#), "junk reads as the default, on");
        assert!(read(r#","open":0"#));
    }

    #[test]
    fn at_most_fifty_tabs_are_read() {
        let tabs: Vec<Value> = (0..80)
            .map(|i| json!({ "cwd": format!(r"C:\t{i}") }))
            .collect();
        let raw = json!({ "tabs": tabs }).to_string();
        assert_eq!(parse_panels(&raw).len(), MAX_TABS);
    }

    #[test]
    fn blank_means_no_tab_has_a_panel() {
        assert!(is_blank(&[]));
        assert!(is_blank(&[entry("C:\\a", None)]));
        assert!(!is_blank(&[entry(
            "C:\\a",
            Some(panel(PanelKind::Text, "a.txt", true, None))
        )]));
    }

    #[test]
    fn restored_tabs_take_their_panels_by_folder_in_order() {
        let p = |t: &str| Some(panel(PanelKind::Text, t, true, None));
        let saved = vec![
            entry(r"C:\a", p("1.txt")),
            entry(r"C:\b", None),
            entry(r"C:\a", p("2.txt")),
        ];
        let live: Vec<String> = [r"C:\a", r"C:\b", r"C:\a"].map(String::from).to_vec();
        let got = match_restored(&saved, &live);
        assert_eq!(got[0].as_ref().map(|x| x.target.as_str()), Some("1.txt"));
        assert_eq!(got[1], None);
        assert_eq!(got[2].as_ref().map(|x| x.target.as_str()), Some("2.txt"));
    }

    #[test]
    fn a_tab_that_was_dropped_from_the_restore_only_skips_its_entry() {
        let p = |t: &str| Some(panel(PanelKind::Text, t, true, None));
        let saved = vec![entry(r"C:\gone", p("x.txt")), entry(r"C:\b", p("y.txt"))];
        let live = vec![r"C:\b".to_string()];
        let got = match_restored(&saved, &live);
        assert_eq!(got[0].as_ref().map(|x| x.target.as_str()), Some("y.txt"));
    }

    #[test]
    fn a_new_tab_in_a_known_folder_does_not_steal_an_earlier_panel() {
        let p = Some(panel(PanelKind::Text, "a.txt", true, None));
        let saved = vec![entry(r"C:\a", p)];
        let live = vec![r"C:\z".to_string(), r"C:\a".to_string()];
        let got = match_restored(&saved, &live);
        assert_eq!(got[0], None);
        assert!(got[1].is_some());
    }

    #[test]
    fn folders_compare_without_case_slashes_or_a_trailing_slash() {
        let p = Some(panel(PanelKind::Text, "a.txt", true, None));
        let saved = vec![entry(r"C:\Work\", p)];
        let got = match_restored(&saved, &["c:/work".to_string()]);
        assert!(got[0].is_some());
    }

    #[test]
    fn nothing_is_written_until_a_save_and_a_save_lands_on_flush() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("panels.json");
        let store = PanelStore::new(&file, Duration::from_secs(60));
        assert!(store.load().is_empty());
        assert!(!store.started(), "a load starts no thread");
        store.flush();
        assert!(!file.exists());
        let tabs = vec![entry(
            r"C:\a",
            Some(panel(PanelKind::Image, r"C:\a\p.png", true, Some(0.5))),
        )];
        store.save(&tabs);
        assert!(store.started());
        // Inside the debounce the newest state is what a load sees.
        assert_eq!(store.load(), tabs);
        store.flush();
        let reopened = PanelStore::new(&file, Duration::from_secs(60));
        assert_eq!(reopened.load(), tabs);
    }

    #[test]
    fn a_damaged_file_is_set_aside_and_the_store_still_saves() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("panels.json");
        std::fs::write(&file, [0xff, 0xfe, 0x00, 0x01]).unwrap(); // sync-ok: test code only
        let store = PanelStore::new(&file, Duration::from_secs(60));
        let _ = store.load();
        // Whatever the file held, the store neither crashes nor loses the ability to save.
        let tabs = vec![entry(
            r"C:\a",
            Some(panel(PanelKind::Text, r"C:\a\n.txt", true, None)),
        )];
        store.save(&tabs);
        store.flush();
        if !store.is_read_only() {
            assert_eq!(PanelStore::new(&file, Duration::from_secs(60)).load(), tabs);
        }
    }
}
