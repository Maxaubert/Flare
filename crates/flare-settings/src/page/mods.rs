//! The Plugins page (Surfaces stage 8, plugins phase 2): the mods and plugins Flare found, what each may do in plain
//! words, a switch for each (off until the user turns it on), the folders that are not mods and why, and buttons
//! that open the plugins and mods folders. The page keeps its key `mods` and is titled Plugins. A protocol 1 mod's
//! rows are what they always were; a protocol 2 plugin adds who made it (`modby:<id>`) and, for the permissions
//! that reach furthest, the warnings (`modwarn:<id>`). The rows are not in the closed row tables: there is one set
//! per mod found, so their ids are made from the mod's id (`mod:<id>`, `modperm:<id>`) and interned once. The
//! contract is `docs/surfaces/mods.md`.

use std::collections::HashSet;
use std::sync::Mutex;

use super::{BlockModel, Button, Control, PageEnv, PageModel, RowModel, SectionModel};

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
}

/// A folder in the mods folder that is not a mod, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Problem {
    /// The folder's name; empty for a problem with the root folder itself.
    pub folder: String,
    pub reason: String,
    /// It is in the plugins folder, not the mods folder.
    pub plugins: bool,
}

/// What the page shows. The app fills it from the hub.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModsInfo {
    /// A scan has finished (until then the page says it is looking).
    pub scanned: bool,
    /// The mods folder, for the Open folder row.
    pub dir: String,
    /// The plugins folder, for its Open folder row.
    pub plugins_dir: String,
    pub mods: Vec<ModInfo>,
    pub problems: Vec<Problem>,
    /// Sentences about tools left out and a switch file that cannot be saved.
    pub notes: Vec<String>,
    /// The switch that could not be saved, and why (the last one).
    pub save_error: Option<String>,
}

/// The id of the row that opens the mods folder.
pub const FOLDER_ROW: &str = "mods-folder";
/// The id of the row that opens the plugins folder.
pub const PLUGINS_FOLDER_ROW: &str = "plugins-folder";
/// The prefix of a plugin's maker row (then the plugin's id).
pub const BY_PREFIX: &str = "modby:";
/// The prefix of a plugin's warnings row (then the plugin's id).
pub const WARN_PREFIX: &str = "modwarn:";
/// The page's title in the rail and at its top.
pub const TITLE: &str = "Plugins";
/// The prefix of a mod's switch row (then the mod's id).
pub const SWITCH_PREFIX: &str = "mod:";
/// The prefix of a mod's permissions row (then the mod's id).
pub const PERMS_PREFIX: &str = "modperm:";
/// The words under the folder row while no path is known.
pub const FOLDER_UNKNOWN: &str = "Where mods are kept.";
/// The words under the plugins folder row while no path is known.
pub const PLUGINS_FOLDER_UNKNOWN: &str = "Where plugins are kept.";
/// The maker row when the plugin does not say who made it.
pub const NO_AUTHOR: &str = "The plugin does not say who made it.";
/// The label of the warnings row of a plugin that asks for full control.
pub const FULL_CONTROL: &str = "Full control";
/// The label of the warnings row of a plugin whose permissions together reach further than each alone.
pub const TAKE_NOTE: &str = "Before you turn it on";
/// The words under a mod's switch when it asks for permissions that were never allowed.
pub const NEEDS_APPROVAL: &str =
    "Asks for new permissions, so it was turned off. Switch it on to allow what is listed below.";
/// The row shown while the first scan has not finished.
pub const LOOKING: &str = "Looking for plugins...";
/// The row shown when neither folder holds a plugin or a mod.
pub const NONE_FOUND: &str = "No plugins yet. Put a plugin folder in the plugins folder (or a mod in the mods folder), then open this page again.";

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

    // The folders, and what is going on.
    let mut top = Vec::new();
    top.push(BlockModel::Row(RowModel {
        buttons: vec![Button {
            id: "open",
            label: "Open plugins folder".to_string(),
            alt: None,
            off: info.plugins_dir.is_empty(),
        }],
        ..plain(
            PLUGINS_FOLDER_ROW,
            "folder",
            "Plugins folder".to_string(),
            if info.plugins_dir.is_empty() {
                PLUGINS_FOLDER_UNKNOWN.to_string()
            } else {
                info.plugins_dir.clone()
            },
        )
    }));
    top.push(BlockModel::Row(RowModel {
        buttons: vec![Button {
            id: "open",
            label: "Open mods folder".to_string(),
            alt: None,
            off: info.dir.is_empty(),
        }],
        ..plain(
            FOLDER_ROW,
            "folder",
            "Mods folder".to_string(),
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
    sections.push(section("mods", String::new(), top));

    // One card per mod: the switch, and what the mod may do.
    for m in &info.mods {
        let switch_id = intern(format!("{SWITCH_PREFIX}{}", m.id));
        let perms_id = intern(format!("{PERMS_PREFIX}{}", m.id));
        let words = if m.permissions.is_empty() {
            "Nothing: it only answers when asked.".to_string()
        } else {
            m.permissions.join(", ")
        };
        let mut switch = plain(
            switch_id,
            "chip",
            format!("{} {}", m.name, m.version),
            m.description.clone(),
        );
        if m.needs_approval {
            switch.sub = NEEDS_APPROVAL.to_string();
            switch.warn = true;
        }
        switch.tap = true;
        switch.control = Control::Switch { on: m.enabled };
        let mut perms = plain(perms_id, "key", "Allowed to".to_string(), words);
        if m.has_panel {
            perms.buttons.push(Button {
                id: "show",
                label: "Show panel".to_string(),
                alt: None,
                off: !m.enabled,
            });
        }
        let mut blocks = vec![BlockModel::Row(switch)];
        if m.plugin {
            blocks.push(BlockModel::Row(by_row(m)));
        }
        blocks.push(BlockModel::Row(perms));
        if m.plugin && !m.warnings.is_empty() {
            blocks.push(BlockModel::Row(RowModel {
                warn: true,
                ..plain(
                    intern(format!("{WARN_PREFIX}{}", m.id)),
                    "warn",
                    if m.full_control {
                        FULL_CONTROL.to_string()
                    } else {
                        TAKE_NOTE.to_string()
                    },
                    m.warnings.join(" "),
                )
            }));
        }
        sections.push(section(
            intern(format!("modcard:{}", m.id)),
            m.name.clone(),
            blocks,
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
                if p.folder.is_empty() && p.plugins {
                    "The plugins folder".to_string()
                } else if p.folder.is_empty() {
                    "The mods folder".to_string()
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

/// Who made a plugin and where it lives: `author, homepage`, or what is known of the two.
fn by_row(m: &ModInfo) -> RowModel {
    let sub = match (m.author.is_empty(), m.homepage.is_empty()) {
        (false, false) => format!("{}, {}", m.author, m.homepage),
        (false, true) => m.author.clone(),
        (true, false) => format!("{NO_AUTHOR} {}", m.homepage),
        (true, true) => NO_AUTHOR.to_string(),
    };
    plain(
        intern(format!("{BY_PREFIX}{}", m.id)),
        "about",
        "Made by".to_string(),
        sub,
    )
}

/// The mod a row id names (its switch or its permissions row).
pub fn mod_of_row(row: &str) -> Option<&str> {
    row.strip_prefix(SWITCH_PREFIX)
        .or_else(|| row.strip_prefix(PERMS_PREFIX))
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
    fn a_protocol_1_mod_has_exactly_its_two_rows_and_no_plugin_rows() {
        with_env_mods(info(vec![notes(true)]), |env| {
            let page = build(env);
            let card = page
                .sections
                .iter()
                .find(|s| s.id == "modcard:notes")
                .unwrap();
            let ids: Vec<&str> = card
                .blocks
                .iter()
                .filter_map(|b| match b {
                    BlockModel::Row(r) => Some(r.id),
                    _ => None,
                })
                .collect();
            assert_eq!(ids, ["mod:notes", "modperm:notes"]);
            assert_eq!(row_of(&page, "mod:notes").unwrap().label, "Notes 1.0.0");
        });
    }

    #[test]
    fn a_plugin_card_adds_who_made_it_and_the_warnings_in_plain_words() {
        with_env_mods(info(vec![todo(false)]), |env| {
            let page = build(env);
            let card = page
                .sections
                .iter()
                .find(|s| s.id == "modcard:todo")
                .unwrap();
            let ids: Vec<&str> = card
                .blocks
                .iter()
                .filter_map(|b| match b {
                    BlockModel::Row(r) => Some(r.id),
                    _ => None,
                })
                .collect();
            assert_eq!(
                ids,
                ["mod:todo", "modby:todo", "modperm:todo", "modwarn:todo"]
            );
            let sw = row_of(&page, "mod:todo").unwrap();
            assert_eq!(sw.label, "Session to-dos 1.0.0");
            assert_eq!(sw.control, Control::Switch { on: false });
            let by = row_of(&page, "modby:todo").unwrap();
            assert_eq!(by.label, "Made by");
            assert_eq!(by.sub, "someone, https://example.com/todo");
            let perms = row_of(&page, "modperm:todo").unwrap();
            assert!(perms.sub.contains("(full control)"), "{}", perms.sub);
            let warn = row_of(&page, "modwarn:todo").unwrap();
            assert!(warn.warn);
            assert_eq!(warn.label, FULL_CONTROL);
            assert!(warn.sub.starts_with("Full control:"), "{}", warn.sub);
            assert!(warn.sub.contains("steer Claude"), "{}", warn.sub);
        });
    }

    #[test]
    fn a_plugin_without_full_control_notes_the_combined_reach_and_one_without_warnings_has_no_row()
    {
        let mut t = todo(false);
        t.full_control = false;
        t.warnings = vec!["It can send what you and Claude say to the internet.".into()];
        with_env_mods(info(vec![t.clone()]), |env| {
            let page = build(env);
            assert_eq!(row_of(&page, "modwarn:todo").unwrap().label, TAKE_NOTE);
        });
        t.warnings.clear();
        with_env_mods(info(vec![t]), |env| {
            assert!(row_of(&build(env), "modwarn:todo").is_none());
        });
    }

    #[test]
    fn the_maker_row_says_what_is_known() {
        for (author, homepage, want) in [
            ("me", "", "me".to_string()),
            (
                "",
                "https://x.test/",
                format!("{NO_AUTHOR} https://x.test/"),
            ),
            ("", "", NO_AUTHOR.to_string()),
        ] {
            let mut t = todo(false);
            t.author = author.into();
            t.homepage = homepage.into();
            with_env_mods(info(vec![t]), |env| {
                assert_eq!(row_of(&build(env), "modby:todo").unwrap().sub, want);
            });
        }
    }

    #[test]
    fn the_plugins_folder_row_comes_first_and_opens_its_folder() {
        let mut i = info(vec![]);
        i.plugins_dir = r"C:\Users\x\AppData\Roaming\Flare\plugins".into();
        with_env_mods(i, |env| {
            let page = build(env);
            let ids = all_row_ids(&page);
            assert_eq!(&ids[..2], [PLUGINS_FOLDER_ROW, FOLDER_ROW]);
            let r = row_of(&page, PLUGINS_FOLDER_ROW).unwrap();
            assert_eq!(r.sub, r"C:\Users\x\AppData\Roaming\Flare\plugins");
            assert_eq!(r.buttons[0].label, "Open plugins folder");
            assert!(!r.buttons[0].off);
        });
        with_env_mods(ModsInfo::default(), |env| {
            let r = row_of(&build(env), PLUGINS_FOLDER_ROW).cloned().unwrap();
            assert_eq!(r.sub, PLUGINS_FOLDER_UNKNOWN);
            assert!(r.buttons[0].off);
        });
    }

    #[test]
    fn a_problem_with_the_plugins_folder_itself_names_it() {
        let mut i = info(vec![]);
        i.problems = vec![Problem {
            folder: String::new(),
            reason: "the plugins folder is not a folder".into(),
            plugins: true,
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
            dir: r"C:\Users\x\AppData\Roaming\Flare\mods".into(),
            mods,
            ..ModsInfo::default()
        }
    }

    #[test]
    fn a_mod_is_a_switch_row_and_a_permissions_row_under_its_name() {
        with_env_mods(info(vec![notes(false)]), |env| {
            let page = build(env);
            assert_eq!(page.id, "mods");
            assert_eq!(page.label, "Plugins");
            let card = page
                .sections
                .iter()
                .find(|s| s.id == "modcard:notes")
                .expect("the mod's card");
            assert_eq!(card.title, "Notes");
            let sw = row_of(&page, "mod:notes").unwrap();
            assert_eq!(sw.label, "Notes 1.0.0");
            assert_eq!(sw.sub, "A notes panel the agent can add a line to.");
            assert!(sw.tap);
            assert_eq!(sw.control, Control::Switch { on: false });
            let perms = row_of(&page, "modperm:notes").unwrap();
            assert_eq!(perms.label, "Allowed to");
            assert_eq!(
                perms.sub,
                "Show its panel in a tab, Answer the agent's tool calls"
            );
            assert_eq!(perms.control, Control::None);
        });
    }

    #[test]
    fn the_permissions_show_before_the_mod_is_turned_on_and_the_panel_button_waits_for_it() {
        with_env_mods(info(vec![notes(false)]), |env| {
            let off = build(env);
            let p = row_of(&off, "modperm:notes").unwrap();
            assert!(!p.sub.is_empty(), "the words are there while it is off");
            assert_eq!(p.buttons.len(), 1);
            assert_eq!(p.buttons[0].id, "show");
            assert!(p.buttons[0].off, "no panel while the mod is off");
        });
        with_env_mods(info(vec![notes(true)]), |env| {
            let on = build(env);
            assert!(!row_of(&on, "modperm:notes").unwrap().buttons[0].off);
            assert_eq!(
                row_of(&on, "mod:notes").unwrap().control,
                Control::Switch { on: true }
            );
        });
    }

    #[test]
    fn a_mod_without_a_panel_has_no_panel_button_and_one_without_permissions_says_so() {
        let mut m = notes(true);
        m.has_panel = false;
        m.permissions.clear();
        with_env_mods(info(vec![m]), |env| {
            let page = build(env);
            let p = row_of(&page, "modperm:notes").unwrap();
            assert!(p.buttons.is_empty());
            assert!(p.sub.starts_with("Nothing"), "{}", p.sub);
        });
    }

    #[test]
    fn the_folder_row_opens_the_folder_and_shows_its_path() {
        with_env_mods(info(vec![]), |env| {
            let page = build(env);
            let r = row_of(&page, FOLDER_ROW).unwrap();
            assert_eq!(r.sub, r"C:\Users\x\AppData\Roaming\Flare\mods");
            assert_eq!(r.buttons[0].id, "open");
            assert_eq!(r.buttons[0].label, "Open mods folder");
            assert!(!r.buttons[0].off);
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
                plugins: false,
            },
            Problem {
                folder: String::new(),
                reason: "more than 64 folders; 3 were not read".into(),
                plugins: false,
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
            assert_eq!(row_of(&page, "modbad:1").unwrap().label, "The mods folder");
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
        assert_eq!(mod_of_row("modperm:notes"), Some("notes"));
        assert_eq!(mod_of_row("mods-folder"), None);
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
            PLUGINS_FOLDER_UNKNOWN,
            NO_AUTHOR,
            FULL_CONTROL,
            TAKE_NOTE,
        ] {
            assert!(!t.contains('\u{2014}'));
        }
    }
}
