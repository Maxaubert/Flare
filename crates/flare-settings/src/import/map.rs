//! The import key table: Prism Terminal's keys to Flare's (M7 Task 14).
//!
//! Source: the design spec's Appendix D (the table lives in the M7 plan) and the readers of the
//! modules that own each key. A value is written only when the owning module reads back exactly
//! the canonical text that would be stored: a value the module would read as its default because
//! it was not understood is skipped, never guessed. The one exception to "same text back" is the
//! documented clamp or step (a number the module clamps, a font size off the menu, a hex8
//! background written opaque, an indicator word that is an old name).
//!
//! Dictation keys have no owning module yet (M8 adds its readers), so they read back as stored.
//! This file, `legacy.rs` and the test fixtures of `import/` are the only places in
//! `crates/*/src` where the text of Prism Terminal's key prefix may be written.

use flare_theme::colour::{ColourFormat, canonical_stored};
use flare_theme::fonts::{font_id_or_default, font_pct_from};
use flare_theme::retired::live_theme_id;
use flare_theme::see_through::see_through_level;
use serde_json::Value;

use crate::agent_prefs::{
    agent_done_on, agent_failed_on, agent_guard, agent_hooks_on, agent_question_on, agent_rainbow,
    taskbar_badge_on,
};
use crate::colour_format::colour_format;
use crate::custom_theme::{parse_custom, serialize_custom};
use crate::help_prefs::{clamp_help_width, help_enabled, help_shell, help_width};
use crate::kv::PrefStore;
use crate::look::{AgentColourState, AgentIndicator, LookStore};
use crate::new_tab::{NewTabMode, new_tab_folder, new_tab_mode};
use crate::recent_roots::{pinned_roots, recent_roots};
use crate::sidebar::{SidebarMode, sidebar_width, valid_sidebar_mode, valid_sidebar_width_num};
use crate::tab_layout::{TabLayout, tab_layout};
use crate::tab_switch::{TabSwitch, tab_switch};
use crate::term_prefs::{agent_keys, saved_shell_id};
use crate::window_colours::{
    ACCENT, BACKGROUND, CUSTOM_COLOURS_KEY, SECONDARY, WindowColours, custom_window_colours,
    set_custom_window_colours, valid_colours, window_edges,
};

/// How a candidate value is checked before it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// One of the listed words, exactly.
    Enum(&'static [&'static str]),
    /// `0` or `1` only.
    Bool01,
    /// A colour in canonical form (six or eight digit hex); alpha is kept. Empty is skipped.
    Colour,
    /// A colour written opaque (the window background: Flare's Background has no alpha).
    ColourOpt,
    /// A theme id `[a-z0-9-]{1,64}`; a retired id is written as its replacement, `style` as `pt-default`.
    ThemeId,
    /// A saved Custom theme, re-serialised by the custom theme reader (drops retired fields).
    CustomJson,
    /// The window colours JSON, re-serialised by the window colours reader.
    ColoursJson,
    /// A listed face id.
    FontId,
    /// A number from 50 to 200, written as the step the font reader gives.
    FontPct,
    /// A number, clamped to 0 to 70 and rounded by the see-through reader.
    SeeThroughLevel,
    /// `minimal` and `ring` as they are; `off` and `full` become `ring`.
    IndicatorWord,
    /// A shell id made of plain characters (see `Charset::Id`).
    ShellId,
    /// A number, clamped to the sidebar's width range.
    SidebarWidth,
    /// A number, clamped to the help panel's width range (a number that is not above zero is skipped).
    HelpWidth,
    /// A folder path: no control characters, at most 1024 characters; empty means "the user's folder".
    FolderPath,
    /// A JSON array of path strings, capped at the given length, de-duplicated case-insensitively.
    PathList(usize),
    /// The dictation hotkey JSON, re-serialised in Prism Terminal's field order.
    HotkeyJson,
    /// `auto`, or two to four lower-case letters.
    Language,
    /// A plain string of at most `max` characters, all from `charset`.
    PlainString { max: usize, charset: Charset },
}

/// The character sets a plain string may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charset {
    /// ASCII letters, digits, `.`, `_` and `-`.
    Id,
    /// Lower-case ASCII letters, digits and `-`.
    LowerDash,
}

/// One row of the table: a Prism Terminal key, its Flare key and the check on the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub from: &'static str,
    pub to: &'static str,
    pub check: Check,
}

/// Appendix D, in its order.
pub const RULES: &[Rule] = &[
    Rule {
        from: "prism.term.theme",
        to: "flare.term.theme",
        check: Check::ThemeId,
    },
    Rule {
        from: "prism.term.custom",
        to: "flare.term.custom",
        check: Check::CustomJson,
    },
    Rule {
        from: "prism.term.font",
        to: "flare.term.font",
        check: Check::FontId,
    },
    Rule {
        from: "prism.term.fontPct",
        to: "flare.term.fontPct",
        check: Check::FontPct,
    },
    Rule {
        from: "prism.term.seeThrough",
        to: "flare.term.seeThrough",
        check: Check::SeeThroughLevel,
    },
    Rule {
        from: "prism.term.agentIndicator",
        to: "flare.term.agentIndicator",
        check: Check::IndicatorWord,
    },
    Rule {
        from: "prism.term.agentColor",
        to: "flare.term.agentColor",
        check: Check::Colour,
    },
    Rule {
        from: "prism.term.agentDoneColor",
        to: "flare.term.agentDoneColor",
        check: Check::Colour,
    },
    Rule {
        from: "prism.term.agentQuestionColor",
        to: "flare.term.agentQuestionColor",
        check: Check::Colour,
    },
    Rule {
        from: "prism.term.agentDoneOn",
        to: "flare.term.agentDoneOn",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.term.agentQuestionOn",
        to: "flare.term.agentQuestionOn",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.term.agentFailedOn",
        to: "flare.term.agentFailedOn",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.term.agentRainbow",
        to: "flare.term.agentRainbow",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.term.agentHooks",
        to: "flare.term.agentHooks",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.term.colourFormat",
        to: "flare.term.colourFormat",
        check: Check::Enum(&["hex", "rgba", "hsla"]),
    },
    Rule {
        from: "prism.term.shell",
        to: "flare.term.shell",
        check: Check::ShellId,
    },
    Rule {
        from: "prism.window.tabLayout",
        to: "flare.window.tabLayout",
        check: Check::Enum(&["horizontal", "vertical"]),
    },
    Rule {
        from: "prism.window.tabSwitch",
        to: "flare.window.tabSwitch",
        check: Check::Enum(&["order", "recent"]),
    },
    Rule {
        from: "prism.window.edges",
        to: "flare.window.edges",
        check: Check::Enum(&["none", "faint", "hairline", "solid"]),
    },
    Rule {
        from: "prism.window.background",
        to: "flare.window.background",
        check: Check::ColourOpt,
    },
    Rule {
        from: "prism.window.secondary",
        to: "flare.window.secondary",
        check: Check::Colour,
    },
    Rule {
        from: "prism.window.accent",
        to: "flare.window.accent",
        check: Check::Colour,
    },
    Rule {
        from: "prism.window.customColours",
        to: "flare.window.customColours",
        check: Check::ColoursJson,
    },
    Rule {
        from: "prism.window.taskbarBadge",
        to: "flare.window.taskbarBadge",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.window.agentGuard",
        to: "flare.window.agentGuard",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.window.agentKeys",
        to: "flare.window.agentKeys",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.window.sidebar",
        to: "flare.window.sidebar",
        check: Check::Enum(&["open", "collapsed"]),
    },
    Rule {
        from: "prism.window.sidebarWidth",
        to: "flare.window.sidebarWidth",
        check: Check::SidebarWidth,
    },
    Rule {
        from: "prism.newtab.mode",
        to: "flare.newtab.mode",
        check: Check::Enum(&["folder", "ask"]),
    },
    Rule {
        from: "prism.newtab.folder",
        to: "flare.newtab.folder",
        check: Check::FolderPath,
    },
    Rule {
        from: "prism.help.enabled",
        to: "flare.help.enabled",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.help.shell",
        to: "flare.help.shell",
        check: Check::Enum(&["powershell", "cmd", "bash"]),
    },
    Rule {
        from: "prism.help.panelWidth",
        to: "flare.help.panelWidth",
        check: Check::HelpWidth,
    },
    Rule {
        from: "prism.recentRoots",
        to: "flare.recentRoots",
        check: Check::PathList(12),
    },
    Rule {
        from: "prism.pinnedRoots",
        to: "flare.pinnedRoots",
        check: Check::PathList(100),
    },
    Rule {
        from: "prism.dictation.enabled",
        to: "flare.dictation.enabled",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.dictation.mode",
        to: "flare.dictation.mode",
        check: Check::Enum(&["hold", "toggle"]),
    },
    Rule {
        from: "prism.dictation.hotkey",
        to: "flare.dictation.hotkey",
        check: Check::HotkeyJson,
    },
    Rule {
        from: "prism.dictation.language",
        to: "flare.dictation.language",
        check: Check::Language,
    },
    Rule {
        from: "prism.dictation.pauseMedia",
        to: "flare.dictation.pauseMedia",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.dictation.sounds",
        to: "flare.dictation.sounds",
        check: Check::Bool01,
    },
    Rule {
        from: "prism.dictation.model",
        to: "flare.dictation.model",
        check: Check::PlainString {
            max: 64,
            charset: Charset::Id,
        },
    },
];

/// The two legacy keys. Read by `legacy.rs` only, folded into `seeThrough` and the Custom, never written.
pub const LEGACY_KEYS: [&str; 2] = ["prism.term.acrylic", "prism.term.opacity"];

/// Keys Prism Terminal removed, with the reason.
pub const RETIRED_KEYS: [(&str, &str); 3] = [
    ("prism.window.tabStyle", "retired in Prism Terminal"),
    ("prism.window.tabWidth", "retired in Prism Terminal"),
    ("prism.window.titleBar", "retired in Prism Terminal"),
];

/// Keys dropped on purpose, with the reason.
pub const DROPPED_KEYS: [(&str, &str); 1] = [("prism.dictation.mic", "device ids are Chromium's")];

/// What one source key comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Mapped {
    /// Write `value` under Flare's `key`.
    Write { key: &'static str, value: String },
    /// Not imported; `reason` says why.
    Skip { from: String, reason: &'static str },
    /// A legacy key, folded by `legacy.rs`.
    Legacy,
    /// A retired key, with the reason.
    Retired(&'static str),
    /// A dropped key, with the reason.
    Dropped(&'static str),
    /// Not in the table: counted in the report, not imported.
    Unknown,
}

/// Maps one Prism Terminal key and its raw value.
pub fn map_one(from: &str, raw: &str) -> Mapped {
    if LEGACY_KEYS.contains(&from) {
        return Mapped::Legacy;
    }
    if let Some((_, reason)) = RETIRED_KEYS.iter().find(|(k, _)| *k == from) {
        return Mapped::Retired(reason);
    }
    if let Some((_, reason)) = DROPPED_KEYS.iter().find(|(k, _)| *k == from) {
        return Mapped::Dropped(reason);
    }
    let Some(rule) = RULES.iter().find(|r| r.from == from) else {
        return Mapped::Unknown;
    };
    match accept(&rule.check, rule.to, raw) {
        Some(value) => Mapped::Write {
            key: rule.to,
            value,
        },
        None => Mapped::Skip {
            from: from.to_string(),
            reason: "not a valid value",
        },
    }
}

/// The canonical text to store for `raw`, or None when the check refuses it.
fn candidate(check: &Check, raw: &str) -> Option<String> {
    match *check {
        Check::Enum(words) => words.contains(&raw).then(|| raw.to_string()),
        Check::Bool01 => matches!(raw, "0" | "1").then(|| raw.to_string()),
        Check::Colour => canonical_stored(Some(raw)),
        Check::ColourOpt => {
            canonical_stored(Some(raw)).map(|c| if c.len() == 9 { c[..7].to_string() } else { c })
        }
        Check::ThemeId => {
            if raw == "style" {
                Some("pt-default".to_string())
            } else if plain(raw, 64, Charset::LowerDash) {
                Some(live_theme_id(raw).to_string())
            } else {
                None
            }
        }
        Check::CustomJson => {
            let c = parse_custom(raw)?;
            Some(serialize_custom(&c))
        }
        Check::ColoursJson => {
            let v: Value = serde_json::from_str(raw).ok()?;
            let c = valid_colours(&v);
            if c.background.is_none() && c.accent.is_none() && c.secondary.is_none() {
                return None;
            }
            colours_text(&c)
        }
        Check::FontId => (font_id_or_default(Some(raw)) == raw).then(|| raw.to_string()),
        Check::FontPct => {
            let n = finite(raw)?;
            (50.0..=200.0)
                .contains(&n)
                .then(|| font_pct_from(Some(raw)).to_string())
        }
        Check::SeeThroughLevel => {
            finite(raw)?;
            Some(see_through_level(Some(raw)).to_string())
        }
        Check::IndicatorWord => match raw {
            "minimal" => Some("minimal".to_string()),
            "ring" | "off" | "full" => Some("ring".to_string()),
            _ => None,
        },
        Check::ShellId => plain(raw, 64, Charset::Id).then(|| raw.to_string()),
        Check::SidebarWidth => {
            let n = finite(raw)?;
            Some(valid_sidebar_width_num(n).to_string())
        }
        Check::HelpWidth => {
            let n = finite(raw)?;
            (n > 0.0).then(|| clamp_help_width(n).to_string())
        }
        Check::FolderPath => (raw.chars().count() <= 1024 && !raw.chars().any(char::is_control))
            .then(|| raw.to_string()),
        Check::PathList(cap) => path_list(raw, cap),
        Check::HotkeyJson => hotkey(raw),
        Check::Language => {
            let plain_lang =
                (2..=4).contains(&raw.len()) && raw.bytes().all(|b| b.is_ascii_lowercase());
            (raw == "auto" || plain_lang).then(|| raw.to_string())
        }
        Check::PlainString { max, charset } => plain(raw, max, charset).then(|| raw.to_string()),
    }
}

/// Canonical text for `raw`, kept only when the owning module reads the same text back.
fn accept(check: &Check, to: &str, raw: &str) -> Option<String> {
    let cand = candidate(check, raw)?;
    let store = PrefStore::memory();
    store.set(to, &cand);
    (read_back(to, &store)? == cand).then_some(cand)
}

/// What the owning module reads for `to` from `s`, in the same text it would be stored in.
fn read_back(to: &str, s: &PrefStore) -> Option<String> {
    let look = || LookStore::new(s.clone());
    let on = |b: bool| Some(if b { "1" } else { "0" }.to_string());
    match to {
        "flare.term.theme" => Some(look().theme_id()),
        "flare.term.custom" => look().custom().map(|c| serialize_custom(&c)),
        "flare.term.font" => Some(look().font_id().to_string()),
        "flare.term.fontPct" => Some(look().font_pct().to_string()),
        "flare.term.seeThrough" => Some(look().see_through().to_string()),
        "flare.term.agentIndicator" => Some(
            match look().agent_indicator() {
                AgentIndicator::Minimal => "minimal",
                _ => "ring",
            }
            .to_string(),
        ),
        "flare.term.agentColor" => non_empty(look().agent_colour_choice(AgentColourState::Working)),
        "flare.term.agentDoneColor" => {
            non_empty(look().agent_colour_choice(AgentColourState::Finished))
        }
        "flare.term.agentQuestionColor" => {
            non_empty(look().agent_colour_choice(AgentColourState::Question))
        }
        "flare.term.agentDoneOn" => on(agent_done_on(s)),
        "flare.term.agentQuestionOn" => on(agent_question_on(s)),
        "flare.term.agentFailedOn" => on(agent_failed_on(s)),
        "flare.term.agentRainbow" => on(agent_rainbow(s)),
        "flare.term.agentHooks" => on(agent_hooks_on(s)),
        "flare.term.colourFormat" => Some(
            match colour_format(s) {
                ColourFormat::Hex => "hex",
                ColourFormat::Rgba => "rgba",
                ColourFormat::Hsla => "hsla",
            }
            .to_string(),
        ),
        "flare.term.shell" => saved_shell_id(s),
        "flare.window.tabLayout" => Some(
            match tab_layout(s) {
                TabLayout::Horizontal => "horizontal",
                TabLayout::Vertical => "vertical",
            }
            .to_string(),
        ),
        "flare.window.tabSwitch" => Some(
            match tab_switch(s) {
                TabSwitch::Order => "order",
                TabSwitch::Recent => "recent",
            }
            .to_string(),
        ),
        "flare.window.edges" => Some(window_edges(s).word().to_string()),
        "flare.window.background" => BACKGROUND.get(s),
        "flare.window.secondary" => SECONDARY.get(s),
        "flare.window.accent" => ACCENT.get(s),
        "flare.window.customColours" => colours_text(&custom_window_colours(s)),
        "flare.window.taskbarBadge" => on(taskbar_badge_on(s)),
        "flare.window.agentGuard" => on(agent_guard(s)),
        "flare.window.agentKeys" => on(agent_keys(s)),
        "flare.window.sidebar" => Some(
            match valid_sidebar_mode(s.get(to).as_deref()) {
                SidebarMode::Open => "open",
                SidebarMode::Collapsed => "collapsed",
            }
            .to_string(),
        ),
        "flare.window.sidebarWidth" => Some(sidebar_width(s).to_string()),
        "flare.newtab.mode" => Some(
            match new_tab_mode(s) {
                NewTabMode::Folder => "folder",
                NewTabMode::Ask => "ask",
            }
            .to_string(),
        ),
        "flare.newtab.folder" => Some(new_tab_folder(s)),
        "flare.help.enabled" => on(help_enabled(s)),
        "flare.help.shell" => help_shell(s),
        "flare.help.panelWidth" => Some(help_width(s).to_string()),
        "flare.recentRoots" => serde_json::to_string(&recent_roots(s)).ok(),
        "flare.pinnedRoots" => serde_json::to_string(&pinned_roots(s)).ok(),
        // The dictation keys have no owning reader until M8: they read back as stored.
        _ => s.get(to),
    }
}

/// The window colours JSON the window colours reader writes for `c`, or None when it writes nothing.
fn colours_text(c: &WindowColours) -> Option<String> {
    let store = PrefStore::memory();
    set_custom_window_colours(&store, Some(c));
    store.get(CUSTOM_COLOURS_KEY)
}

/// A number, finite; the text is trimmed first, as the readers trim it.
fn finite(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// A string of 1 to `max` characters, all from `charset`.
fn plain(raw: &str, max: usize, charset: Charset) -> bool {
    let n = raw.chars().count();
    (1..=max).contains(&n)
        && raw.chars().all(|c| match charset {
            Charset::Id => c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'),
            Charset::LowerDash => c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-',
        })
}

/// A JSON array of paths: non-string and empty entries, control characters and over-long paths
/// are dropped, case-insensitive repeats are dropped, the list is capped. Not an array is None.
fn path_list(raw: &str, cap: usize) -> Option<String> {
    let Value::Array(items) = serde_json::from_str::<Value>(raw).ok()? else {
        return None;
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let Value::String(p) = item else {
            continue;
        };
        if p.is_empty() || p.chars().count() > 1024 || p.chars().any(char::is_control) {
            continue;
        }
        let lower = p.to_lowercase();
        if out.iter().any(|x| x.to_lowercase() == lower) {
            continue;
        }
        out.push(p);
        if out.len() == cap {
            break;
        }
    }
    serde_json::to_string(&out).ok()
}

/// The dictation hotkey in Prism Terminal's field order. `code` is `[A-Za-z0-9]{1,32}`; each
/// modifier is on only when its JSON value is `true`.
fn hotkey(raw: &str) -> Option<String> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let obj = v.as_object()?;
    let code = obj.get("code")?.as_str()?;
    if !(1..=32).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let flag = |k: &str| obj.get(k).and_then(Value::as_bool) == Some(true);
    Some(format!(
        "{{\"code\":{},\"ctrl\":{},\"alt\":{},\"shift\":{},\"meta\":{}}}",
        serde_json::to_string(code).ok()?,
        flag("ctrl"),
        flag("alt"),
        flag("shift"),
        flag("meta"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table as Appendix D lists its source keys, in order.
    const APPENDIX_D: [&str; 42] = [
        "prism.term.theme",
        "prism.term.custom",
        "prism.term.font",
        "prism.term.fontPct",
        "prism.term.seeThrough",
        "prism.term.agentIndicator",
        "prism.term.agentColor",
        "prism.term.agentDoneColor",
        "prism.term.agentQuestionColor",
        "prism.term.agentDoneOn",
        "prism.term.agentQuestionOn",
        "prism.term.agentFailedOn",
        "prism.term.agentRainbow",
        "prism.term.agentHooks",
        "prism.term.colourFormat",
        "prism.term.shell",
        "prism.window.tabLayout",
        "prism.window.tabSwitch",
        "prism.window.edges",
        "prism.window.background",
        "prism.window.secondary",
        "prism.window.accent",
        "prism.window.customColours",
        "prism.window.taskbarBadge",
        "prism.window.agentGuard",
        "prism.window.agentKeys",
        "prism.window.sidebar",
        "prism.window.sidebarWidth",
        "prism.newtab.mode",
        "prism.newtab.folder",
        "prism.help.enabled",
        "prism.help.shell",
        "prism.help.panelWidth",
        "prism.recentRoots",
        "prism.pinnedRoots",
        "prism.dictation.enabled",
        "prism.dictation.mode",
        "prism.dictation.hotkey",
        "prism.dictation.language",
        "prism.dictation.pauseMedia",
        "prism.dictation.sounds",
        "prism.dictation.model",
    ];

    fn written(from: &str, raw: &str) -> Option<String> {
        match map_one(from, raw) {
            Mapped::Write { value, .. } => Some(value),
            _ => None,
        }
    }

    fn skipped(from: &str, raw: &str) -> bool {
        matches!(map_one(from, raw), Mapped::Skip { .. })
    }

    #[test]
    fn every_rule_target_starts_with_flare_and_no_source_does_not_start_with_prism() {
        for r in RULES {
            assert!(r.to.starts_with("flare."), "{}", r.to);
            assert!(r.from.starts_with("prism."), "{}", r.from);
        }
    }

    #[test]
    fn the_rule_table_is_appendix_d() {
        let froms: Vec<&str> = RULES.iter().map(|r| r.from).collect();
        assert_eq!(RULES.len(), 42);
        assert_eq!(froms, APPENDIX_D.to_vec());
    }

    #[test]
    fn a_theme_pick_maps_and_a_retired_id_maps_to_its_replacement() {
        assert_eq!(
            written("prism.term.theme", "dracula").as_deref(),
            Some("dracula")
        );
        assert_eq!(
            written("prism.term.theme", "bright-lights").as_deref(),
            Some("pitch")
        );
        assert_eq!(
            written("prism.term.theme", "style").as_deref(),
            Some("pt-default")
        );
        assert!(skipped("prism.term.theme", "Dracula!"));
    }

    #[test]
    fn a_custom_theme_is_reserialised_through_the_modules_reader() {
        let raw = r##"{"bg":"#121212","fg":"#dedacf","cursor":"#dedacf","acrylic":true,"opacity":0.9,"font":"comic","fontPct":150}"##;
        let value = written("prism.term.custom", raw).expect("a valid custom theme writes");
        assert!(value.contains("#121212"));
        assert!(!value.contains("acrylic"));
        assert!(!value.contains("opacity"));
        assert!(!value.contains("comic"));
        assert!(!value.contains("fontPct"));
        assert!(skipped("prism.term.custom", "{nope"));
    }

    #[test]
    fn indicator_words() {
        assert_eq!(
            written("prism.term.agentIndicator", "minimal").as_deref(),
            Some("minimal")
        );
        assert_eq!(
            written("prism.term.agentIndicator", "ring").as_deref(),
            Some("ring")
        );
        assert_eq!(
            written("prism.term.agentIndicator", "off").as_deref(),
            Some("ring")
        );
        assert_eq!(
            written("prism.term.agentIndicator", "full").as_deref(),
            Some("ring")
        );
        assert!(skipped("prism.term.agentIndicator", "soup"));
    }

    #[test]
    fn booleans_are_zero_or_one_only() {
        assert_eq!(
            written("prism.window.agentGuard", "0").as_deref(),
            Some("0")
        );
        assert!(skipped("prism.window.agentGuard", "2"));
        assert!(skipped("prism.window.agentGuard", "true"));
        assert!(skipped("prism.window.agentGuard", ""));
    }

    #[test]
    fn colours_are_canonical_hex_and_junk_is_skipped() {
        assert_eq!(
            written("prism.term.agentColor", "#E07A2F").as_deref(),
            Some("#e07a2f")
        );
        assert_eq!(
            written("prism.term.agentColor", "#22C55E80").as_deref(),
            Some("#22c55e80")
        );
        assert!(skipped("prism.term.agentColor", "#22c55e8"));
        assert!(skipped("prism.term.agentColor", "orange"));
        assert!(skipped("prism.term.agentColor", ""));
    }

    #[test]
    fn the_background_is_written_opaque() {
        assert_eq!(
            written("prism.window.background", "#22C55E80").as_deref(),
            Some("#22c55e")
        );
        assert_eq!(
            written("prism.window.secondary", "#22C55E80").as_deref(),
            Some("#22c55e80")
        );
    }

    #[test]
    fn window_colours_json_keeps_valid_fields_only() {
        let value = written(
            "prism.window.customColours",
            r##"{"background":"#171717","accent":null,"secondary":null}"##,
        )
        .expect("one valid field writes");
        let back: Value = serde_json::from_str(&value).expect("json");
        assert_eq!(back["background"], "#171717");
        assert!(skipped(
            "prism.window.customColours",
            r#"{"background":"blue"}"#
        ));
        assert!(skipped("prism.window.customColours", "nope"));
    }

    #[test]
    fn sidebar_and_edges_and_tab_words() {
        assert_eq!(
            written("prism.window.sidebar", "collapsed").as_deref(),
            Some("collapsed")
        );
        assert_eq!(
            written("prism.window.tabLayout", "vertical").as_deref(),
            Some("vertical")
        );
        assert_eq!(
            written("prism.window.tabSwitch", "recent").as_deref(),
            Some("recent")
        );
        assert_eq!(
            written("prism.window.edges", "faint").as_deref(),
            Some("faint")
        );
        assert!(skipped("prism.window.edges", "diagonal"));
        assert!(skipped("prism.window.sidebar", "hidden"));
        assert_eq!(
            written("prism.window.sidebarWidth", "265").as_deref(),
            Some("265")
        );
        assert_eq!(
            written("prism.window.sidebarWidth", "99999").as_deref(),
            Some("420")
        );
        assert!(skipped("prism.window.sidebarWidth", "soup"));
    }

    #[test]
    fn the_font_and_size() {
        assert_eq!(
            written("prism.term.font", "cascadia").as_deref(),
            Some("cascadia")
        );
        assert!(skipped("prism.term.font", "comic"));
        assert_eq!(written("prism.term.fontPct", "130").as_deref(), Some("130"));
        assert_eq!(written("prism.term.fontPct", "125").as_deref(), Some("130"));
        assert!(skipped("prism.term.fontPct", "733"));
        assert!(skipped("prism.term.fontPct", "abc"));
    }

    #[test]
    fn see_through_is_clamped_and_junk_is_skipped() {
        assert_eq!(
            written("prism.term.seeThrough", "40").as_deref(),
            Some("40")
        );
        assert_eq!(
            written("prism.term.seeThrough", "99").as_deref(),
            Some("70")
        );
        assert!(skipped("prism.term.seeThrough", "soup"));
    }

    #[test]
    fn shell_ids_must_look_like_ids() {
        assert_eq!(written("prism.term.shell", "pwsh").as_deref(), Some("pwsh"));
        assert_eq!(
            written("prism.term.shell", "wsl-Ubuntu").as_deref(),
            Some("wsl-Ubuntu")
        );
        assert!(skipped("prism.term.shell", "a b"));
        assert!(skipped("prism.term.shell", &"x".repeat(100)));
        assert!(skipped("prism.term.shell", "..\\x"));
    }

    #[test]
    fn recent_roots_are_validated_as_a_list() {
        let value = written("prism.recentRoots", r#"["C:\\Users\\Admin\\Documents"]"#)
            .expect("a list writes");
        assert_eq!(value, r#"["C:\\Users\\Admin\\Documents"]"#);
        assert!(skipped("prism.recentRoots", r#"{"a":1}"#));
        assert_eq!(
            written("prism.recentRoots", r#"["C:\\a", 5, "D:\\b", "c:\\A"]"#).as_deref(),
            Some(r#"["C:\\a","D:\\b"]"#)
        );
        let many: Vec<String> = (0..20).map(|i| format!("C:\\p{i}")).collect();
        let raw = serde_json::to_string(&many).expect("json");
        let capped: Vec<String> =
            serde_json::from_str(&written("prism.recentRoots", &raw).expect("writes"))
                .expect("json");
        assert_eq!(capped.len(), 12);
    }

    #[test]
    fn help_keys() {
        assert_eq!(written("prism.help.enabled", "0").as_deref(), Some("0"));
        assert_eq!(written("prism.help.shell", "bash").as_deref(), Some("bash"));
        assert_eq!(
            written("prism.help.panelWidth", "900").as_deref(),
            Some("900")
        );
        assert_eq!(
            written("prism.help.panelWidth", "9000").as_deref(),
            Some("1100")
        );
        assert!(skipped("prism.help.shell", "zsh"));
    }

    #[test]
    fn dictation_keys_are_copied_only_when_plain_and_mic_is_dropped() {
        assert_eq!(
            written("prism.dictation.mode", "toggle").as_deref(),
            Some("toggle")
        );
        assert!(skipped("prism.dictation.mode", "push"));
        assert_eq!(
            written("prism.dictation.model", "parakeet-v3").as_deref(),
            Some("parakeet-v3")
        );
        assert_eq!(
            written("prism.dictation.enabled", "1").as_deref(),
            Some("1")
        );
        assert_eq!(
            written("prism.dictation.language", "no").as_deref(),
            Some("no")
        );
        assert_eq!(
            written("prism.dictation.language", "auto").as_deref(),
            Some("auto")
        );
        assert!(skipped("prism.dictation.language", "en-US"));
        assert!(skipped("prism.dictation.language", "EN"));
        assert_eq!(
            written(
                "prism.dictation.hotkey",
                r#"{"code":"AltRight","ctrl":false,"alt":false,"shift":false,"meta":false}"#
            )
            .as_deref(),
            Some(r#"{"code":"AltRight","ctrl":false,"alt":false,"shift":false,"meta":false}"#)
        );
        assert_eq!(
            written("prism.dictation.hotkey", r#"{"code":"KeyR","ctrl":true}"#).as_deref(),
            Some(r#"{"code":"KeyR","ctrl":true,"alt":false,"shift":false,"meta":false}"#)
        );
        assert!(skipped("prism.dictation.hotkey", "{nope"));
        assert!(skipped("prism.dictation.hotkey", r#"{"code":""}"#));
        assert!(skipped("prism.dictation.hotkey", r#"{"code":5}"#));
        assert!(skipped("prism.dictation.hotkey", r#"{"code":"Key R"}"#));
        assert_eq!(
            map_one("prism.dictation.mic", "device-1"),
            Mapped::Dropped("device ids are Chromium's")
        );
    }

    #[test]
    fn retired_legacy_and_unknown_are_classified() {
        assert_eq!(
            map_one("prism.window.tabStyle", "x"),
            Mapped::Retired("retired in Prism Terminal")
        );
        assert_eq!(map_one("prism.term.opacity", "0.5"), Mapped::Legacy);
        assert_eq!(map_one("prism.something.else", "x"), Mapped::Unknown);
    }
}
