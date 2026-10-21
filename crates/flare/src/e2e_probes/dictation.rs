//! The dictation probes (M8 Task 27): `dict.view`, `dict.pill`, `dict.mark`, `dict.rows`, `dict.ask`,
//! `dict.lang`, `dict.widget`, `dict.role` and `dict.engine`, plus `app.pid` (the scenarios need the window's process
//! to name a copy in the link and the routed words). Read-only: each answers from state the dictation controller,
//! the widget or the Settings bridge already holds, and sets nothing. Their names are `flare_e2e_proto::dict_probes`.
//! Nothing here runs without --e2e (`e2e_glue::install` is the only caller).
//!
//! Each probe is a thin read over a pure builder (`*_json`, `*_face`, `*_tabs`, `role_name`, `engine_json`), and the
//! builders are what the unit tests pin, each with a second input that gives the other answer.

use flare_dictation::controller::{OUTSIDE, Role};
use flare_dictation::engine::EngineKind;
use flare_e2e_proto::dict_probes;
use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::diag::button_json;
use super::{ProbeCtx, ProbeRegistry};
use crate::dictation_ctl::{self, DictView};
use crate::dictation_widget::{self, Snapshot};
use crate::{BlockKind as SlintBlock, RowLook, SettingsBridge};
use std::collections::HashMap;
use std::sync::Mutex;

thread_local! {
    /// What each Dictation model row last said it drew (SettingsBridge.row-look), by row id.
    static LOOKS: Mutex<HashMap<String, RowLook>> = Mutex::new(HashMap::new());
}

/// Keeps what a model row drew; the latest word of a row replaces its earlier one.
pub fn note_look(l: RowLook) {
    LOOKS.with(|m| {
        m.lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(l.id.to_string(), l)
    });
}

fn hex(c: slint::Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        c.red(),
        c.green(),
        c.blue(),
        c.alpha()
    )
}

/// One button of a row's look: its rectangle in the row and its fill, border, radius and ink (css px, #rrggbbaa).
#[allow(clippy::too_many_arguments)]
fn button_look(
    id: &str,
    r: [f32; 4],
    fill: slint::Color,
    edge: slint::Color,
    radius: f32,
    ink: slint::Color,
) -> Value {
    json!({
        "id": id, "x": r[0], "y": r[1], "w": r[2], "h": r[3],
        "fill": hex(fill), "border": hex(edge), "radius": radius, "ink": hex(ink),
    })
}

/// A row's look as dict.rows carries it: the mark's and the name's place in the row, and each button present.
pub fn look_json(l: &RowLook) -> Value {
    let mut buttons = Vec::new();
    if !l.b0_id.is_empty() {
        buttons.push(button_look(
            &l.b0_id,
            [l.b0_x, l.b0_y, l.b0_w, l.b0_h],
            l.b0_fill,
            l.b0_edge,
            l.b0_radius,
            l.b0_ink,
        ));
    }
    if !l.b1_id.is_empty() {
        buttons.push(button_look(
            &l.b1_id,
            [l.b1_x, l.b1_y, l.b1_w, l.b1_h],
            l.b1_fill,
            l.b1_edge,
            l.b1_radius,
            l.b1_ink,
        ));
    }
    json!({
        "row_h": l.row_h,
        "mark": { "x": l.mark_x, "y": l.mark_y, "w": l.mark_w, "h": l.mark_h },
        "name_x": l.name_x,
        "buttons": buttons,
    })
}

/// Whether the pill is in the window's tree: it is mounted over the terminal area in `app.slint` and fed from
/// `dictation_ctl::view()` by the host's `view_changed`.
const PILL_MOUNTED: bool = true;

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add(dict_probes::VIEW, dict_view);
    r.add(dict_probes::PILL, dict_pill);
    r.add(dict_probes::MARK, dict_mark);
    r.add(dict_probes::ROWS, dict_rows);
    r.add(dict_probes::ASK, dict_ask);
    r.add(dict_probes::LANG, dict_lang);
    r.add(dict_probes::WIDGET, dict_widget);
    r.add(dict_probes::ROLE, dict_role);
    r.add(dict_probes::ENGINE, dict_engine);
    r.add("app.pid", app_pid);
}

// ---------------------------------------------------------------------------------------------------------------------
// The pure builders
// ---------------------------------------------------------------------------------------------------------------------

/// The session as a probe names it: `outside` for a recording from another app, else the tab's id, else null.
fn session_name(session: Option<&str>) -> Value {
    match session {
        Some(s) if s == OUTSIDE => json!("outside"),
        Some(s) => json!(s),
        None => Value::Null,
    }
}

/// `dict.view`: what the pill and the mark read.
pub fn view_json(v: &DictView) -> Value {
    json!({
        "active": v.active,
        "listening": v.listening,
        "transcribing": v.transcribing,
        "session": session_name(v.session.as_deref()),
        "live_text": v.live_text,
        "starting": v.starting,
        "message": v.message,
        "level": v.level,
    })
}

/// The pill's face (0 nothing, 1 listening, 2 transcribing, 3 a message), as the Slint `face` property counts it.
pub fn pill_face(v: &DictView) -> i32 {
    dictation_ctl::pill_phase(v)
}

/// `dict.pill`: whether the pill is mounted, its face and its meter.
pub fn pill_json(v: &DictView, mounted: bool) -> Value {
    json!({
        "mounted": mounted,
        "face": pill_face(v),
        "level": v.level,
        "bar_scale": dictation_widget::bar_scale(v.level),
    })
}

/// `dict.mark`: the tab ids that wear the dictation mark (a tab being dictated into, while a recording exists).
pub fn mark_tabs(v: &DictView) -> Vec<String> {
    match v.session.as_deref() {
        Some(s) if v.active && s != OUTSIDE => vec![s.to_string()],
        _ => Vec::new(),
    }
}

/// What one Dictation page row shows, as `dict.rows` reads it from the Settings bridge.
pub struct RowFacts<'a> {
    pub id: &'a str,
    pub state: &'a str,
    pub badge: &'a str,
    pub vendor: &'a str,
    pub recommended: bool,
    pub disabled: bool,
    pub title: &'a str,
    pub buttons: Vec<Value>,
    /// The row's name (the label beside the mark) and its sub line, as the Dictation page draws them.
    pub name: &'a str,
    pub sub: &'a str,
}

/// One Dictation page row as `dict.rows` answers it.
pub fn row_json(f: &RowFacts) -> Value {
    json!({
        "id": f.id,
        "state": f.state,
        "badge": f.badge,
        "vendor": f.vendor,
        "recommended": f.recommended,
        "disabled": f.disabled,
        "title": f.title,
        "buttons": f.buttons,
        "name": f.name,
        "sub": f.sub,
    })
}

/// `dict.ask`: the one-line question, as Settings draws it and as the page holds it.
pub fn ask_json(open: bool, text: &str, pending: Option<&str>) -> Value {
    json!({ "open": open, "text": text, "pending": pending })
}

/// The name a select shows for its stored value: the option whose id is the value, else the value itself.
pub fn option_name(options: &[(String, String)], value: &str) -> String {
    options
        .iter()
        .find(|(id, _)| id == value)
        .map_or_else(|| value.to_string(), |(_, name)| name.clone())
}

/// `dict.lang`: the language select's text, whether it is disabled, whether the model limits it, and its title.
pub fn lang_json(text: &str, disabled: bool, limited: bool, title: &str) -> Value {
    json!({ "text": text, "disabled": disabled, "limited": limited, "title": title })
}

/// `dict.widget`: the widget window's state; its picker rows (headers apart) and the header labels.
pub fn widget_json(s: &Snapshot) -> Value {
    let rows: Vec<Value> = s
        .items
        .iter()
        .filter(|i| !i.header)
        .map(|i| {
            json!({
                "id": i.id,
                "n": i.n,
                "label": i.label,
                "folder": i.folder,
                "chip": i.chip,
                "reason": i.reason,
                "disabled": i.disabled,
            })
        })
        .collect();
    let headers: Vec<&str> = s
        .items
        .iter()
        .filter(|i| i.header)
        .map(|i| i.label.as_str())
        .collect();
    json!({
        "exists": s.exists,
        "visible": s.visible,
        "focusable": s.focusable,
        "topmost": s.topmost,
        "phase": s.phase,
        "title": s.title,
        "text": s.text,
        "rows": rows,
        "headers": headers,
        "bounds": { "x": s.bounds.x, "y": s.bounds.y, "w": s.bounds.w, "h": s.bounds.h },
        "parked": s.parked,
        "height": s.height,
    })
}

/// `dict.role`: this copy's dictation role.
pub fn role_name(r: Role) -> &'static str {
    match r {
        Role::None => "none",
        Role::Service => "service",
        Role::Client => "client",
    }
}

/// `dict.engine`: whether an engine runs, its kind (null before any), and whether a GPU one fell back.
pub fn engine_json(running: bool, kind: Option<EngineKind>, fell_back: bool) -> Value {
    let kind = match kind {
        Some(EngineKind::Gpu) => json!("gpu"),
        Some(EngineKind::Cpu) => json!("cpu"),
        None => Value::Null,
    };
    json!({ "running": running, "kind": kind, "fell_back": fell_back })
}

// ---------------------------------------------------------------------------------------------------------------------
// The probes
// ---------------------------------------------------------------------------------------------------------------------

fn dict_view(_: &ProbeCtx, _: &Value) -> Result<Value, String> {
    Ok(view_json(&dictation_ctl::view()))
}

fn dict_pill(_: &ProbeCtx, _: &Value) -> Result<Value, String> {
    Ok(pill_json(&dictation_ctl::view(), PILL_MOUNTED))
}

fn dict_mark(_: &ProbeCtx, _: &Value) -> Result<Value, String> {
    Ok(json!({ "tabs": mark_tabs(&dictation_ctl::view()) }))
}

/// Whether a row of the window's Settings is one of the Dictation page's: its own id prefix, or a model block.
fn is_dictation_row(id: &str, block_is_model: bool) -> bool {
    id.starts_with("dictation-") || block_is_model
}

/// `dict.rows`: the Dictation page's rows by id, read from the Settings bridge.
fn dict_rows(ctx: &ProbeCtx, _: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    let mut out = Vec::new();
    for s in b.get_sections().iter() {
        for r in s.rows.iter() {
            if !is_dictation_row(r.id.as_str(), matches!(r.block, SlintBlock::Model)) {
                continue;
            }
            let buttons: Vec<Value> = r
                .buttons
                .iter()
                .map(|x| {
                    button_json(
                        x.id.as_str(),
                        x.label.as_str(),
                        x.label_alt.as_str(),
                        x.off,
                        x.alt,
                    )
                })
                .collect();
            let mut row = row_json(&RowFacts {
                id: r.id.as_str(),
                state: r.state.as_str(),
                badge: r.badge.as_str(),
                vendor: r.vendor.as_str(),
                recommended: r.recommended,
                disabled: r.off,
                title: r.button_title.as_str(),
                buttons,
                name: r.label.as_str(),
                sub: r.sub.as_str(),
            });
            row["look"] = LOOKS.with(|m| {
                m.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(r.id.as_str())
                    .map_or(Value::Null, look_json)
            });
            out.push(row);
        }
    }
    Ok(Value::Array(out))
}

fn dict_ask(ctx: &ProbeCtx, _: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    let pending = dictation_ctl::page_snap().ask;
    Ok(ask_json(
        b.get_dict_ask_open(),
        b.get_dict_ask_text().as_str(),
        pending.as_deref(),
    ))
}

/// `dict.lang`: the language row of the Dictation page, or null when the page has none.
fn dict_lang(ctx: &ProbeCtx, _: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    for s in b.get_sections().iter() {
        for r in s.rows.iter() {
            if r.id.as_str() == "dictation-language" {
                let options: Vec<(String, String)> = r
                    .options
                    .iter()
                    .map(|o| (o.id.to_string(), o.name.to_string()))
                    .collect();
                let text = option_name(&options, r.value.as_str());
                return Ok(lang_json(&text, r.off, r.limited, r.row_tip.as_str()));
            }
        }
    }
    Ok(Value::Null)
}

fn dict_widget(_: &ProbeCtx, _: &Value) -> Result<Value, String> {
    Ok(widget_json(&dictation_widget::snapshot()))
}

fn dict_role(_: &ProbeCtx, _: &Value) -> Result<Value, String> {
    Ok(json!({ "role": role_name(dictation_ctl::role_probe()) }))
}

fn dict_engine(_: &ProbeCtx, _: &Value) -> Result<Value, String> {
    let (running, kind, fell_back) = dictation_ctl::engine_probe();
    Ok(engine_json(running, kind, fell_back))
}

/// `app.pid`: this process, so a scenario can name the window in front (the routed words name it).
fn app_pid(_: &ProbeCtx, _: &Value) -> Result<Value, String> {
    Ok(json!(std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> DictView {
        DictView::default()
    }

    fn listening_into(tab: &str) -> DictView {
        DictView {
            active: true,
            listening: true,
            session: Some(tab.into()),
            ..DictView::default()
        }
    }

    #[test]
    fn the_view_says_listening_only_while_listening() {
        assert_eq!(view_json(&listening_into("tab-1"))["listening"], true);
        assert_eq!(view_json(&idle())["listening"], false);
    }

    #[test]
    fn an_outside_recording_is_named_outside_and_a_tab_by_its_id() {
        let outside = DictView {
            session: Some(OUTSIDE.into()),
            ..DictView::default()
        };
        assert_eq!(view_json(&outside)["session"], "outside");
        assert_eq!(view_json(&listening_into("tab-1"))["session"], "tab-1");
        assert!(view_json(&idle())["session"].is_null());
    }

    #[test]
    fn the_pill_face_follows_the_state_and_transcribing_wins_over_listening() {
        assert_eq!(pill_face(&idle()), 0);
        assert_eq!(pill_face(&listening_into("t")), 1);
        let transcribing = DictView {
            active: true,
            transcribing: true,
            ..DictView::default()
        };
        assert_eq!(pill_face(&transcribing), 2);
        let message = DictView {
            message: Some("Heard nothing".into()),
            ..DictView::default()
        };
        assert_eq!(pill_face(&message), 3);
    }

    #[test]
    fn the_pill_reports_whether_it_is_mounted() {
        assert_eq!(pill_json(&listening_into("t"), true)["mounted"], true);
        assert_eq!(pill_json(&listening_into("t"), false)["mounted"], false);
    }

    #[test]
    fn the_mark_wears_one_tab_while_recording_into_it_and_none_otherwise() {
        assert_eq!(
            mark_tabs(&listening_into("tab-1")),
            vec!["tab-1".to_string()]
        );
        assert!(mark_tabs(&idle()).is_empty());
        let outside = DictView {
            active: true,
            listening: true,
            session: Some(OUTSIDE.into()),
            ..DictView::default()
        };
        assert!(mark_tabs(&outside).is_empty());
    }

    #[test]
    fn a_row_says_it_is_disabled_only_when_it_is() {
        let facts = |disabled| RowFacts {
            id: "dictation-mode",
            state: "",
            badge: "",
            vendor: "",
            recommended: false,
            disabled,
            title: "",
            buttons: vec![],
            name: "",
            sub: "",
        };
        let on = row_json(&facts(false));
        let off = row_json(&facts(true));
        assert_eq!(on["disabled"], false);
        assert_eq!(off["disabled"], true);
    }

    #[test]
    fn the_picker_shows_the_name_of_its_stored_value() {
        let opts = vec![
            ("no".to_string(), "Norwegian".to_string()),
            ("auto".to_string(), "Auto-detect".to_string()),
        ];
        assert_eq!(option_name(&opts, "no"), "Norwegian");
        assert_eq!(option_name(&opts, "auto"), "Auto-detect");
        assert_eq!(option_name(&opts, "xx"), "xx");
    }

    #[test]
    fn a_look_names_the_mark_the_name_and_each_button_present() {
        let mut l = RowLook {
            row_h: 64.0,
            mark_x: 14.0,
            mark_y: 14.0,
            mark_w: 36.0,
            mark_h: 36.0,
            name_x: 64.0,
            b0_id: "uninstall".into(),
            b0_w: 90.0,
            b0_fill: slint::Color::from_argb_u8(255, 30, 30, 29),
            b0_radius: 8.0,
            ..Default::default()
        };
        let j = look_json(&l);
        assert_eq!(j["mark"]["w"], 36.0);
        assert_eq!(j["name_x"], 64.0);
        assert_eq!(j["buttons"].as_array().unwrap().len(), 1);
        assert_eq!(j["buttons"][0]["id"], "uninstall");
        assert_eq!(j["buttons"][0]["fill"], "#1e1e1dff");
        l.b1_id = "cancel".into();
        assert_eq!(look_json(&l)["buttons"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_row_carries_its_name_and_its_sub_line() {
        let r = row_json(&RowFacts {
            id: "base",
            state: "absent",
            badge: "",
            vendor: "openai",
            recommended: false,
            disabled: false,
            title: "",
            buttons: vec![],
            name: "Whisper Base",
            sub: "Downloaded once",
        });
        assert_eq!(r["name"], "Whisper Base");
        assert_eq!(r["sub"], "Downloaded once");
    }

    #[test]
    fn a_model_row_carries_its_badge_and_state() {
        let r = row_json(&RowFacts {
            id: "m1",
            state: "installed",
            badge: "Recommended",
            vendor: "openai",
            recommended: true,
            disabled: false,
            title: "Remove",
            buttons: vec![],
            name: "",
            sub: "",
        });
        assert_eq!(r["state"], "installed");
        assert_eq!(r["badge"], "Recommended");
        assert_eq!(r["recommended"], true);
        assert_eq!(r["title"], "Remove");
    }

    #[test]
    fn only_the_dictation_rows_are_listed() {
        assert!(is_dictation_row("dictation-key", false));
        assert!(is_dictation_row("whisper-small", true));
        assert!(!is_dictation_row("appearance", false));
    }

    #[test]
    fn the_ask_says_open_only_while_the_question_is_asked() {
        let asked = ask_json(true, "Download?", Some("m1"));
        let closed = ask_json(false, "", None);
        assert_eq!(asked["open"], true);
        assert_eq!(asked["pending"], "m1");
        assert_eq!(closed["open"], false);
        assert!(closed["pending"].is_null());
    }

    #[test]
    fn the_language_row_says_limited_only_when_the_model_limits_it() {
        assert_eq!(lang_json("Auto", false, true, "t")["limited"], true);
        assert_eq!(lang_json("Auto", false, false, "t")["limited"], false);
        assert_eq!(lang_json("Auto", true, false, "t")["disabled"], true);
    }

    #[test]
    fn the_widget_lists_headers_apart_from_its_rows() {
        let item = |header: bool, label: &str| dictation_widget::Item {
            header,
            id: String::new(),
            n: 0,
            label: label.into(),
            folder: String::new(),
            chip: String::new(),
            reason: String::new(),
            disabled: false,
        };
        let snap = Snapshot {
            exists: true,
            visible: true,
            focusable: false,
            topmost: true,
            phase: 3,
            title: "Send to".into(),
            text: String::new(),
            items: vec![item(true, "Copy A"), item(false, "one")],
            bounds: dictation_widget::Bounds {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            },
            parked: false,
            height: 80,
        };
        let v = widget_json(&snap);
        assert_eq!(v["rows"].as_array().unwrap().len(), 1);
        assert_eq!(v["headers"][0], "Copy A");
        assert_eq!(v["focusable"], false);
        let hidden = Snapshot {
            visible: false,
            ..snap
        };
        assert_eq!(widget_json(&hidden)["visible"], false);
    }

    #[test]
    fn the_role_names_tell_the_three_roles_apart() {
        assert_eq!(role_name(Role::None), "none");
        assert_eq!(role_name(Role::Service), "service");
        assert_eq!(role_name(Role::Client), "client");
    }

    #[test]
    fn the_engine_names_its_kind_and_a_fall_back() {
        let gpu = engine_json(true, Some(EngineKind::Gpu), false);
        let cpu = engine_json(true, Some(EngineKind::Cpu), true);
        let none = engine_json(false, None, false);
        assert_eq!(gpu["kind"], "gpu");
        assert_eq!(cpu["kind"], "cpu");
        assert_eq!(cpu["fell_back"], true);
        assert!(none["kind"].is_null());
        assert_eq!(none["running"], false);
    }

    #[test]
    fn the_app_pid_is_a_real_process_id() {
        assert_ne!(std::process::id(), 0);
    }
}
