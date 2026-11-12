//! The Plugins page (Surfaces stage 8, plugins phase 2): the mods and plugins Flare found, a switch for each (off until
//! the user turns it on), the folders that are not mods and why, and a button that opens the plugins folder (the one
//! folder for both protocols). Plugins phase 7 adds the install rows (from a `.zip` file, or from an `https` address
//! typed in a text field); what the last install said is the install row's subtext. The page keeps its key `mods` and
//! is titled Plugins.
//!
//! Each mod is a card of ONE row (2026-11-08, `docs/superpowers/specs/2026-11-08-plugin-brand-and-sheet.md`): its logo
//! (the brand's, else the chip), name and version, its description, the switch, Update, an info button that opens
//! the details sheet (what it may do in plain words, who made it, the warnings and the person's choices, drawn by the
//! app) and a trash button (Remove). The rows are not in the closed row tables: there is one per mod found, so its id
//! is made from the mod's id (`mod:<id>`) and interned once. The contract is `docs/surfaces/plugins.md` (protocol 1:
//! `docs/surfaces/mods.md`).

use std::collections::HashSet;
use std::sync::Mutex;

use super::{BlockModel, Button, Control, Logo, PageEnv, PageModel, RowModel, SectionModel};

/// One mod as the page shows it. `permissions` are already plain words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub permissions: Vec<String>,
    pub enabled: bool,
    /// The mod was on and now asks for permissions the person never allowed, so it was turned off.
    pub needs_approval: bool,
    /// The mod has a panel (the Show panel button is offered while it is on).
    pub has_panel: bool,
    /// A protocol 2 plugin (`plugin.json`): its card adds who made it and the warnings.
    pub plugin: bool,
    /// Protocol 2: who made it and its `https://` address; empty when the manifest leaves them out.
    pub author: String,
    pub homepage: String,
    /// Protocol 2: the warnings for what its permissions add up to, as sentences, most serious first.
    pub warnings: Vec<String>,
    /// Protocol 2: it asks for a permission that hands over full control (`process`, `input`).
    pub full_control: bool,
    /// Protocol 2: its background page was stopped (it ended too often, or more run than may), with the sentence
    /// that says so. The switch row shows it in place of the description.
    pub stopped: Option<String>,
    /// Where Settings installed it from (an `https` address); empty for a file or a folder put there by hand.
    pub source: String,
    /// Protocol 2: its brand colour (`#rrggbb`) and logo path, empty without a brand (the card draws its chip).
    pub brand_color: String,
    pub logo: String,
}

/// A folder in the plugins folder that is not a mod (or one left in the old mods folder), and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Problem {
    /// The folder's name; empty for a problem with the plugins folder itself.
    pub folder: String,
    pub reason: String,
}

/// What the page shows. The app fills it from the hub.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModsInfo {
    /// A scan has finished (until then the page says it is looking).
    pub scanned: bool,
    /// The plugins folder, for the Open folder row.
    pub dir: String,
    pub mods: Vec<ModInfo>,
    pub problems: Vec<Problem>,
    /// Sentences about tools left out and a switch file that cannot be saved.
    pub notes: Vec<String>,
    /// The switch that could not be saved, and why (the last one).
    pub save_error: Option<String>,
    /// Every plugin is off for this start (safe mode, design 13), and why: shown at the top of the page.
    pub safe_mode: Option<String>,
    /// An install, update or removal is running: its buttons wait.
    pub installing: bool,
    /// What the last install, update or removal said (one sentence), and whether it failed.
    pub install_note: Option<(bool, String)>,
    /// The address typed in the install field (Settings keeps it while the page is open).
    pub url: String,
}

/// The id of the row that opens the plugins folder.
pub const FOLDER_ROW: &str = "plugins-folder";
/// The page's title in the rail and at its top.
pub const TITLE: &str = "Plugins";
/// The prefix of a mod's row (then the mod's id): the card's one row.
pub const SWITCH_PREFIX: &str = "mod:";
/// The id of a card's Details button: it opens the plugin's details sheet.
pub const INFO_BUTTON: &str = "info";
/// The words on a card's Details button.
pub const DETAILS: &str = "Details";
/// Where a removed plugin's folder goes, as the system names it: the Recycle Bin, or a Mac's Trash (rule 1135).
pub const BIN: &str = if cfg!(target_os = "macos") {
    "Trash"
} else {
    "Recycle Bin"
};
/// The tooltip of a card's trash button.
pub const REMOVE_TIP: &str = if cfg!(target_os = "macos") {
    "Remove (to the Trash)"
} else {
    "Remove (to the Recycle Bin)"
};
/// The tooltip of a card's Details button.
pub const INFO_TIP: &str = "Details and permissions";
/// The row that installs a plugin from a file, and the one with the address field.
pub const INSTALL_ROW: &str = "plugins-install";
pub const URL_ROW: &str = "plugins-url";
/// The install row's words while nothing was installed yet.
pub const INSTALL_SUB: &str =
    "From a .zip file, or from an https address below. A plugin is installed off.";
/// The install row's words while an install runs.
pub const INSTALLING: &str = "Working on it...";
/// The words under the folder row while no path is known.
pub const FOLDER_UNKNOWN: &str = "Where plugins are kept.";
/// The maker row when the plugin does not say who made it.
pub const NO_AUTHOR: &str = "The plugin does not say who made it.";
/// The words under a mod's switch when it asks for permissions that were never allowed.
pub const NEEDS_APPROVAL: &str =
    "Asks for new permissions, so it was turned off. Switch it on to allow what is listed below.";
/// The row that says every plugin is off for this start (safe mode), and its label.
pub const SAFE_ROW: &str = "mods-safe-mode";
pub const SAFE_LABEL: &str = "Plugins are off for this start";
/// The row shown while the first scan has not finished.
pub const LOOKING: &str = "Looking for plugins...";
/// The row shown when the plugins folder holds nothing.
pub const NONE_FOUND: &str =
    "No plugins yet. Put a plugin folder in the plugins folder, then open this page again.";

/// A static copy of a made-up id: rows and sections are named by `&'static str`, a mod's id is known only at
/// run time, and each distinct id is kept once for the life of the process (at most 64 mods per scan).
fn intern(s: String) -> &'static str {
    static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    if let Some(found) = set.get(s.as_str()) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.into_boxed_str());
    set.insert(leaked);
    leaked
}

fn plain(id: &'static str, icon: &'static str, label: String, sub: String) -> RowModel {
    RowModel {
        id,
        icon,
        label,
        sub,
        warn: false,
        off: false,
        tap: false,
        row_tip: String::new(),
        control: Control::None,
        reset: false,
        buttons: Vec::new(),
        logo: None,
    }
}

fn section(id: &'static str, title: String, blocks: Vec<BlockModel>) -> SectionModel {
    SectionModel {
        id,
        title,
        tag: None,
        action: None,
        blocks,
    }
}

/// The page model.
pub fn build(env: &PageEnv<'_>) -> PageModel {
    // A run with no mods (no port to ask) has an empty page.
    let Some(info) = env.mods else {
        return PageModel {
            id: "mods",
            label: TITLE,
            sections: Vec::new(),
        };
    };
    let mut sections = Vec::new();

    // The folder, and what is going on.
    let mut top = Vec::new();
    top.push(BlockModel::Row(RowModel {
        buttons: vec![Button {
            id: "open",
            label: "Open plugins folder".to_string(),
            alt: None,
            off: info.dir.is_empty(),
            ..Button::default()
        }],
        ..plain(
            FOLDER_ROW,
            "folder",
            "Plugins folder".to_string(),
            if info.dir.is_empty() {
                FOLDER_UNKNOWN.to_string()
            } else {
                info.dir.clone()
            },
        )
    }));
    if !info.scanned {
        top.push(BlockModel::Row(plain(
            "mods-looking",
            "chip",
            LOOKING.to_string(),
            "Plugins are off until you turn them on.".to_string(),
        )));
    } else if info.mods.is_empty() && info.problems.is_empty() {
        top.push(BlockModel::Row(plain(
            "mods-none",
            "chip",
            "No plugins found".to_string(),
            NONE_FOUND.to_string(),
        )));
    }
    if let Some(why) = &info.safe_mode {
        top.push(BlockModel::Row(RowModel {
            warn: true,
            ..plain(SAFE_ROW, "warn", SAFE_LABEL.to_string(), why.clone())
        }));
    }
    if let Some(why) = &info.save_error {
        top.push(BlockModel::Row(RowModel {
            warn: true,
            ..plain(
                "mods-save-error",
                "warn",
                "A switch was not saved".to_string(),
                why.clone(),
            )
        }));
    }
    let (failed, said) = match (&info.install_note, info.installing) {
        (_, true) => (false, INSTALLING.to_string()),
        (Some((failed, text)), false) => (*failed, text.clone()),
        (None, false) => (false, INSTALL_SUB.to_string()),
    };
    top.insert(
        1,
        BlockModel::Row(RowModel {
            warn: failed,
            buttons: vec![Button {
                id: "file",
                label: "Install from file".to_string(),
                alt: None,
                off: info.installing,
                ..Button::default()
            }],
            ..plain(
                INSTALL_ROW,
                "download",
                "Install a plugin".to_string(),
                said,
            )
        }),
    );
    top.insert(
        2,
        BlockModel::Row(RowModel {
            control: Control::Text {
                value: info.url.clone(),
                placeholder: "https://".to_string(),
            },
            buttons: vec![Button {
                id: "url",
                label: "Install".to_string(),
                alt: None,
                off: info.installing,
                ..Button::default()
            }],
            ..plain(
                URL_ROW,
                "globe",
                "From an address".to_string(),
                "An https link to a plugin's .zip file.".to_string(),
            )
        }),
    );
    sections.push(section("mods", String::new(), top));

    // One card per mod, one row: its logo, name and version, the description, the switch, Update, Details (the
    // details sheet: what it may do, who made it, the person's choices) and Remove. The card has no heading: its
    // row names it.
    for m in &info.mods {
        let switch_id = intern(format!("{SWITCH_PREFIX}{}", m.id));
        let mut switch = plain(
            switch_id,
            "chip",
            format!("{} {}", m.name, m.version),
            m.description.clone(),
        );
        if !m.logo.is_empty() {
            switch.logo = Some(Logo {
                path: m.logo.clone(),
                color: m.brand_color.clone(),
            });
        }
        if m.needs_approval {
            switch.sub = NEEDS_APPROVAL.to_string();
            switch.warn = true;
        } else if let Some(why) = m.stopped.as_ref().filter(|_| m.enabled) {
            switch.sub = why.clone();
            switch.warn = true;
        }
        switch.tap = true;
        switch.control = Control::Switch { on: m.enabled };
        // Update installs a newer copy (from where it came, or a file); Remove sends the folder to the Recycle Bin.
        switch.buttons.push(Button {
            id: "update",
            label: "Update".to_string(),
            off: info.installing,
            ..Button::default()
        });
        // Details opens the details sheet, the only way to it: the switch turns the plugin on by itself (Max,
        // 2026-11-12). A worded button, so it is seen, where an icon alone was missed.
        switch.buttons.push(Button {
            id: INFO_BUTTON,
            label: DETAILS.to_string(),
            tip: INFO_TIP.to_string(),
            ..Button::default()
        });
        switch.buttons.push(Button {
            id: "remove",
            off: info.installing,
            icon: "trash",
            tip: REMOVE_TIP.to_string(),
            ..Button::default()
        });
        sections.push(section(
            intern(format!("modcard:{}", m.id)),
            String::new(),
            vec![BlockModel::Row(switch)],
        ));
    }

    // What was not loaded, and tools left out.
    let mut bad = Vec::new();
    for (i, p) in info.problems.iter().enumerate() {
        bad.push(BlockModel::Row(RowModel {
            warn: true,
            ..plain(
                intern(format!("modbad:{i}")),
                "warn",
                if p.folder.is_empty() {
                    "The plugins folder".to_string()
                } else {
                    p.folder.clone()
                },
                p.reason.clone(),
            )
        }));
    }
    for (i, n) in info.notes.iter().enumerate() {
        bad.push(BlockModel::Row(RowModel {
            warn: true,
            ..plain(
                intern(format!("modnote:{i}")),
                "flag",
                "Left out".to_string(),
                n.clone(),
            )
        }));
    }
    if !bad.is_empty() {
        sections.push(section("mods-problems", "Not loaded".to_string(), bad));
    }

    PageModel {
        id: "mods",
        label: TITLE,
        sections,
    }
}

/// The mod a row id names.
pub fn mod_of_row(row: &str) -> Option<&str> {
    row.strip_prefix(SWITCH_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::test_env::with_env_mods;
    use crate::page::{all_row_ids, row_of};

    fn notes(enabled: bool) -> ModInfo {
        ModInfo {
            id: "notes".into(),
            name: "Notes".into(),
            version: "1.0.0".into(),
            description: "A notes panel the agent can add a line to.".into(),
            permissions: vec![
                "Show its panel in a tab".into(),
                "Answer the agent's tool calls".into(),
            ],
            enabled,
            needs_approval: false,
            has_panel: true,
            ..ModInfo::default()
        }
    }

    fn todo(enabled: bool) -> ModInfo {
        ModInfo {
            id: "todo".into(),
            name: "Session to-dos".into(),
            version: "1.0.0".into(),
            description: "Things to check per session.".into(),
            permissions: vec![
                "Put badges and progress on tabs".into(),
                "Type and run commands in your terminals (full control)".into(),
                "Read what you and Claude say in sessions".into(),
            ],
            enabled,
            has_panel: true,
            plugin: true,
            author: "someone".into(),
            homepage: "https://example.com/todo".into(),
            warnings: vec![
                "Full control: it can type and run commands in your terminals, as if you typed them.".into(),
                "With both, it can read what Claude says and steer Claude by typing to it.".into(),
            ],
            full_control: true,
            ..ModInfo::default()
        }
    }

    #[test]
    fn the_page_keeps_its_key_and_is_titled_plugins() {
        with_env_mods(info(vec![]), |env| {
            let page = build(env);
            assert_eq!((page.id, page.label), ("mods", "Plugins"));
        });
    }

    #[test]
    fn every_card_is_one_row_with_update_details_and_remove_and_no_heading() {
        for m in [notes(false), todo(true)] {
            with_env_mods(info(vec![m.clone()]), |env| {
                let page = build(env);
                let card = page
                    .sections
                    .iter()
                    .find(|s| s.id == format!("modcard:{}", m.id))
                    .expect("the mod's card");
                assert_eq!(card.title, "", "the row names it");
                assert_eq!(card.blocks.len(), 1);
                let id = format!("mod:{}", m.id);
                let sw = row_of(&page, &id).unwrap();
                assert_eq!(sw.label, format!("{} {}", m.name, m.version));
                assert_eq!(sw.sub, m.description);
                assert!(sw.tap);
                assert_eq!(sw.control, Control::Switch { on: m.enabled });
                let b: Vec<(&str, &str, &str)> = sw
                    .buttons
                    .iter()
                    .map(|b| (b.id, b.label.as_str(), b.icon))
                    .collect();
                assert_eq!(
                    b,
                    [
                        ("update", "Update", ""),
                        (INFO_BUTTON, DETAILS, ""),
                        ("remove", "", "trash")
                    ]
                );
                // The buttons say what they are; none is drawn as a warning.
                assert_eq!(sw.buttons[1].tip, INFO_TIP);
                assert_eq!(sw.buttons[2].tip, REMOVE_TIP);
                assert!(sw.buttons.iter().all(|b| !b.warn));
                assert!(sw.logo.is_none(), "no brand, the chip");
            });
        }
    }

    #[test]
    fn a_branded_plugin_card_draws_its_logo_in_its_colour() {
        let mut t = todo(false);
        t.brand_color = "#175ddc".into();
        t.logo = "M3 3h18v18H3z".into();
        with_env_mods(info(vec![t]), |env| {
            let page = build(env);
            assert_eq!(
                row_of(&page, "mod:todo").unwrap().logo,
                Some(Logo {
                    path: "M3 3h18v18H3z".into(),
                    color: "#175ddc".into()
                })
            );
        });
    }
    #[test]
    fn a_problem_with_the_plugins_folder_itself_names_it() {
        let mut i = info(vec![]);
        i.problems = vec![Problem {
            folder: String::new(),
            reason: "the plugins folder is not a folder".into(),
        }];
        with_env_mods(i, |env| {
            assert_eq!(
                row_of(&build(env), "modbad:0").unwrap().label,
                "The plugins folder"
            );
        });
    }

    #[test]
    fn a_mod_that_asks_for_new_permissions_says_so_under_its_off_switch() {
        let mut m = notes(false);
        m.needs_approval = true;
        with_env_mods(info(vec![m]), |env| {
            let page = build(env);
            let sw = row_of(&page, "mod:notes").unwrap();
            assert_eq!(sw.sub, NEEDS_APPROVAL);
            assert!(sw.warn);
            assert_eq!(sw.control, Control::Switch { on: false });
        });
    }

    fn info(mods: Vec<ModInfo>) -> ModsInfo {
        ModsInfo {
            scanned: true,
            dir: r"C:\Users\x\AppData\Roaming\Flare\plugins".into(),
            mods,
            ..ModsInfo::default()
        }
    }

    #[test]
    fn a_stopped_background_page_and_safe_mode_are_said_in_warning_rows() {
        let mut m = todo(true);
        m.stopped = Some("Stopped: its background page ended 3 times in 5 minutes.".into());
        let mut i = info(vec![m]);
        i.safe_mode = Some("Flare did not close properly twice in a row.".into());
        with_env_mods(i, |env| {
            let page = build(env);
            let sw = row_of(&page, "mod:todo").unwrap();
            assert!(sw.warn && sw.sub.starts_with("Stopped:"), "{}", sw.sub);
            let safe = row_of(&page, SAFE_ROW).unwrap();
            assert!(safe.warn);
            assert_eq!(safe.label, SAFE_LABEL);
            assert!(safe.sub.contains("twice"));
        });
        // A plugin that is off says what it is, stopped or not.
        let mut m = todo(false);
        m.stopped = Some("Stopped.".into());
        with_env_mods(info(vec![m]), |env| {
            let page = build(env);
            let sw = row_of(&page, "mod:todo").unwrap();
            assert!(!sw.warn && !sw.sub.starts_with("Stopped"));
            assert!(row_of(&page, SAFE_ROW).is_none());
        });
    }

    #[test]
    fn the_one_folder_row_comes_first_opens_the_plugins_folder_and_shows_its_path() {
        with_env_mods(info(vec![]), |env| {
            let page = build(env);
            let ids = all_row_ids(&page);
            assert_eq!(ids[0], FOLDER_ROW);
            assert_eq!(FOLDER_ROW, "plugins-folder");
            let r = row_of(&page, FOLDER_ROW).unwrap();
            assert_eq!(r.label, "Plugins folder");
            assert_eq!(r.sub, r"C:\Users\x\AppData\Roaming\Flare\plugins");
            assert_eq!(r.buttons[0].id, "open");
            assert_eq!(r.buttons[0].label, "Open plugins folder");
            assert!(!r.buttons[0].off);
            // No other folder row, and no word of a mods folder anywhere on the page.
            assert_eq!(ids.iter().filter(|i| i.ends_with("-folder")).count(), 1);
            let text = format!("{page:?}").to_lowercase();
            assert!(!text.contains("mods folder"), "{text}");
        });
        with_env_mods(ModsInfo::default(), |env| {
            let page = build(env);
            let r = row_of(&page, FOLDER_ROW).unwrap();
            assert_eq!(r.sub, FOLDER_UNKNOWN);
            assert!(r.buttons[0].off);
        });
    }

    #[test]
    fn the_page_says_when_it_is_looking_and_when_there_is_nothing() {
        with_env_mods(ModsInfo::default(), |env| {
            let ids = all_row_ids(&build(env));
            assert!(ids.contains(&"mods-looking"));
        });
        with_env_mods(info(vec![]), |env| {
            let ids = all_row_ids(&build(env));
            assert!(ids.contains(&"mods-none"));
            assert!(!ids.contains(&"mods-looking"));
        });
    }

    #[test]
    fn a_folder_that_is_not_a_mod_is_a_warning_row_with_its_reason() {
        let mut i = info(vec![notes(true)]);
        i.problems = vec![
            Problem {
                folder: "broken".into(),
                reason: "mod.json is not valid JSON: expected value at line 1 column 1".into(),
            },
            Problem {
                folder: String::new(),
                reason: "more than 64 folders; 3 were not read".into(),
            },
        ];
        i.notes = vec!["The tool name mod_a_b is claimed by two mods.".into()];
        with_env_mods(i, |env| {
            let page = build(env);
            let bad = page
                .sections
                .iter()
                .find(|s| s.id == "mods-problems")
                .expect("the problems section");
            assert_eq!(bad.title, "Not loaded");
            let r = row_of(&page, "modbad:0").unwrap();
            assert!(r.warn);
            assert_eq!(r.label, "broken");
            assert!(r.sub.contains("not valid JSON"));
            assert_eq!(
                row_of(&page, "modbad:1").unwrap().label,
                "The plugins folder"
            );
            let n = row_of(&page, "modnote:0").unwrap();
            assert!(n.warn && n.sub.contains("mod_a_b"));
            // A page with a problem is not "no mods found".
            assert!(!all_row_ids(&page).contains(&"mods-none"));
        });
    }

    #[test]
    fn a_save_error_is_shown_at_the_top() {
        let mut i = info(vec![notes(true)]);
        i.save_error = Some("mods.json could not be saved: access denied".into());
        with_env_mods(i, |env| {
            let page = build(env);
            let r = row_of(&page, "mods-save-error").unwrap();
            assert!(r.warn && r.sub.contains("access denied"));
        });
    }

    #[test]
    fn the_install_rows_follow_the_folder_and_say_what_the_last_install_said() {
        with_env_mods(info(vec![notes(false)]), |env| {
            let page = build(env);
            let ids = all_row_ids(&page);
            assert_eq!(&ids[..3], [FOLDER_ROW, INSTALL_ROW, URL_ROW]);
            let file = row_of(&page, INSTALL_ROW).unwrap();
            assert_eq!(file.sub, INSTALL_SUB);
            assert!(!file.warn);
            assert_eq!(file.buttons[0].id, "file");
            assert!(!file.buttons[0].off);
            let url = row_of(&page, URL_ROW).unwrap();
            assert_eq!(
                url.control,
                Control::Text {
                    value: String::new(),
                    placeholder: "https://".into()
                }
            );
            assert_eq!(url.buttons[0].id, "url");
            assert!(
                !url.buttons[0].off,
                "an empty address is answered with a sentence"
            );
            // Every card's row has Update, Details and Remove.
            let sw = row_of(&page, "mod:notes").unwrap();
            let b: Vec<&str> = sw.buttons.iter().map(|b| b.id).collect();
            assert_eq!(b, ["update", INFO_BUTTON, "remove"]);
        });
        let mut i = info(vec![notes(false)]);
        i.url = "https://example.com/p.zip".into();
        i.install_note = Some((
            true,
            "The zip entry \"../x\" goes up out of the plugin.".into(),
        ));
        with_env_mods(i.clone(), |env| {
            let page = build(env);
            let file = row_of(&page, INSTALL_ROW).unwrap();
            assert!(file.warn && file.sub.contains("goes up"), "{}", file.sub);
            assert!(!row_of(&page, URL_ROW).unwrap().buttons[0].off);
        });
        i.installing = true;
        with_env_mods(i, |env| {
            let page = build(env);
            assert_eq!(row_of(&page, INSTALL_ROW).unwrap().sub, INSTALLING);
            assert!(row_of(&page, INSTALL_ROW).unwrap().buttons[0].off);
            assert!(row_of(&page, URL_ROW).unwrap().buttons[0].off);
            assert!(
                row_of(&page, "mod:notes")
                    .unwrap()
                    .buttons
                    .iter()
                    .filter(|b| b.id != INFO_BUTTON)
                    .all(|b| b.off)
            );
        });
    }

    #[test]
    fn row_ids_are_unique_and_every_icon_is_known() {
        let mut a = notes(true);
        let mut b = notes(false);
        let mut c = todo(true);
        a.id = "alpha-mod".into();
        b.id = "beta-mod".into();
        c.id = "gamma-plugin".into();
        with_env_mods(info(vec![a, b, c]), |env| {
            let page = build(env);
            let ids = all_row_ids(&page);
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), ids.len(), "{ids:?}");
            for s in &page.sections {
                for b in &s.blocks {
                    if let BlockModel::Row(r) = b {
                        assert!(crate::icons::is_setting_icon(r.icon), "{} {}", r.id, r.icon);
                    }
                }
            }
        });
    }

    #[test]
    fn a_row_id_names_its_mod() {
        assert_eq!(mod_of_row("mod:notes"), Some("notes"));
        assert_eq!(mod_of_row("modperm:notes"), None);
        assert_eq!(mod_of_row("plugins-folder"), None);
        assert_eq!(mod_of_row("modbad:0"), None);
    }

    #[test]
    fn the_same_id_is_interned_once() {
        let a = intern("modcard:zz".to_string());
        let b = intern("modcard:zz".to_string());
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn no_text_has_an_em_dash() {
        for t in [
            LOOKING,
            NONE_FOUND,
            FOLDER_UNKNOWN,
            NO_AUTHOR,
            INFO_TIP,
            REMOVE_TIP,
        ] {
            assert!(!t.contains('\u{2014}'));
        }
    }
}
