//! The Dictation page model (M8 Task 24).
//!
//! The page is DATA like every other page: the cards in Prism Terminal's order (master, listening,
//! while, models, gpu, then "From any app"), each row built from the row table with its live control.
//! What the page shows about the models and the GPU pack comes in as a `DictationData` that the app
//! fills from the catalogue, the store and the stored preferences; this module reads nothing itself.
//! Source: Prism Terminal's `core/renderer/settings/sections/DictationPage.tsx` at `e5848cb`, with
//! D13 (the models wording) and N12 (the From any app card).

use super::{
    BlockModel, Button, Control, Opt, PageEnv, RowModel, SectionModel, opts, plain_row, section,
    switch_row,
};
use crate::rows::{RowDef, row};

/// Said under the master row while no speech model is installed.
pub const NO_MODEL: &str = "Needs a speech model before it works.";
/// The sub of a large model's row (and its warn) while the GPU pack is not installed.
pub const SLOW_NO_GPU: &str = "Slow without GPU acceleration.";
/// The sub of a model that picks its own language.
pub const LIMITED_SUB: &str = "This model detects the language itself.";
/// The tooltip of the language row of such a model, and the one-line download question.
pub const LIMITED_TEXT: &str = "This model doesn't support all languages.";
/// The title of the GPU row's Disable button.
pub const GPU_OFF_TITLE: &str = "Turns GPU acceleration off and frees 675 MB of disk space.";
/// The GPU row's note after the engine fell back to the CPU for this session.
pub const GPU_FELL_BACK: &str = "It could not start, the CPU is used.";
/// The mode row's sub while the key is a toggle.
const TOGGLE_SUB: &str = "One press starts, the next stops.";
/// The id of the GPU pack in the store's status list.
pub const GPU_PACK_ID: &str = "gpu-pack";

/// Whose mark a model wears: Whisper is OpenAI's, Parakeet and the GPU pack NVIDIA's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Openai,
    Nvidia,
}

/// A model's or the pack's state as the page draws it (`Active` only for a model that is in use).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelState {
    Absent,
    Downloading { pct: u32 },
    Installed,
    Active,
}

/// What the store says of one catalogue entry (`Downloading` carries the percentage).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemState {
    Absent,
    Downloading { pct: u32 },
    Installed,
}

/// The fixed facts of one speech model, from the catalogue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// "1.6 GB" or "148 MB" (the catalogue's `size_text`).
    pub size_text: String,
    pub vendor: Vendor,
    pub needs_gpu: bool,
    pub limited: bool,
    pub note: &'static str,
}

/// One catalogue entry's state and the reason its last download failed (if it did).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemInfo {
    pub state: ItemState,
    pub failure: Option<String>,
}

/// The stored dictation preferences as the page shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub enabled: bool,
    /// `true` for Toggle, `false` for Hold.
    pub toggle: bool,
    /// The key's words, as `format_hotkey` writes them ("Ctrl+Win").
    pub key_text: String,
    /// `""` is the system default microphone.
    pub mic: String,
    /// `auto` or a language code.
    pub language: String,
    pub pause_media: bool,
    pub sounds: bool,
    /// The active model's id, `""` when none was chosen.
    pub model: String,
    pub anywhere: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            enabled: false,
            toggle: false,
            key_text: "Ctrl+Win".to_string(),
            mic: String::new(),
            language: "auto".to_string(),
            pause_media: false,
            sounds: true,
            model: String::new(),
            anywhere: true,
        }
    }
}

/// The question a download of a model that does not support every language asks first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub model_id: String,
    pub text: &'static str,
}

/// Everything the Dictation page is drawn from besides the environment.
#[derive(Clone, Debug, PartialEq)]
pub struct DictationData {
    /// The visible models, in the manager's order.
    pub models: Vec<ModelInfo>,
    /// The id Recommended is drawn on (`base`, or `large-v3-turbo` once the pack is installed).
    pub recommended: &'static str,
    /// The store's state by catalogue id (models and `gpu-pack`); a missing id is absent.
    pub items: Vec<(String, ItemInfo)>,
    /// The microphones found, by id and name (the system default is added by the page).
    pub mics: Vec<(String, String)>,
    /// The languages the picker offers, by code and name.
    pub languages: Vec<(&'static str, &'static str)>,
    pub prefs: Prefs,
    /// The key is being captured (the row shows its prompt).
    pub capturing: bool,
    pub mic_testing: bool,
    pub mic_level: f32,
    pub mic_problem: Option<String>,
    /// The engine fell back to the CPU for this session (the GPU row's note).
    pub gpu_fell_back: bool,
    pub ask: Option<Ask>,
    /// Whether the stored model picks its own language, as the catalogue says (e2e-only models included).
    pub stored_limited: bool,
    /// Why the media pause cannot work on this system (macOS: no public API); the switch is then off and dimmed.
    pub pause_media_unavailable: Option<String>,
    /// What stops dictating from any app with this key (macOS: Input Monitoring for a modifier key); it replaces
    /// the row's sub. None on Windows.
    pub anywhere_problem: Option<String>,
    /// The bundled engine has a GPU of its own (Metal on a Mac, rule 313): no model is slow for want of the pack,
    /// which is not offered there. False on Windows.
    pub gpu_built_in: bool,
}

impl DictationData {
    /// The state of one catalogue entry; a missing one is absent with no failure.
    fn item(&self, id: &str) -> ItemInfo {
        self.items
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.clone())
            .unwrap_or(ItemInfo {
                state: ItemState::Absent,
                failure: None,
            })
    }

    fn installed(&self, id: &str) -> bool {
        self.item(id).state == ItemState::Installed
    }

    fn any_model_installed(&self) -> bool {
        self.models.iter().any(|m| self.installed(m.id))
    }

    /// Whether the stored model is one that picks its own language (the caller's catalogue answer, or a
    /// listed model that says so).
    fn limited_model(&self) -> bool {
        self.stored_limited
            || self
                .models
                .iter()
                .any(|m| m.id == self.prefs.model && m.limited)
    }
}

/// A speech model's row in the Speech models card.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRow {
    pub id: &'static str,
    pub name: &'static str,
    pub size_text: String,
    pub vendor: Vendor,
    pub recommended: bool,
    /// "Active" (filled) for the model in use.
    pub badge: Option<&'static str>,
    pub sub: String,
    pub warn: bool,
    pub state: ModelState,
    pub failure: Option<String>,
    pub buttons: Vec<Button>,
    /// The Uninstall button's title ("Delete <name> from this PC.", D13).
    pub uninstall_title: String,
}

/// The NVIDIA GPU pack's row in the GPU card.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuRow {
    pub state: ModelState,
    pub failure: Option<String>,
    /// "Enabled" (filled) once the pack is installed.
    pub badge: Option<&'static str>,
    pub sub: String,
    pub buttons: Vec<Button>,
    /// The Disable button's title, once the pack is installed.
    pub title: String,
}

fn def(id: &str) -> &'static RowDef {
    row(id).expect("the dictation page draws only rows of the tables")
}

fn button(id: &'static str, label: &str) -> Button {
    Button {
        id,
        label: label.to_string(),
        alt: None,
        off: false,
        ..Button::default()
    }
}

/// The master row: on or off, and the warning while it is on and no model is installed (PT's
/// `noModelYet = d.enabled && d.noModel`: switched off, the row keeps its plain line).
fn master(d: &DictationData) -> RowModel {
    let warn = d.prefs.enabled && !d.any_model_installed();
    RowModel {
        warn,
        sub: if warn {
            NO_MODEL.to_string()
        } else {
            def("dictation-enabled").sub.to_string()
        },
        ..switch_row(def("dictation-enabled"), d.prefs.enabled)
    }
}

/// A row of the Listening, While dictating or From any app cards: dimmed and off while dictation is off.
fn dimmed(mut r: RowModel, on: bool) -> RowModel {
    r.off = !on;
    r
}

fn listening_rows(d: &DictationData) -> Vec<BlockModel> {
    let on = d.prefs.enabled;
    let p = &d.prefs;

    let mode = RowModel {
        sub: if p.toggle {
            TOGGLE_SUB.to_string()
        } else {
            def("dictation-mode").sub.to_string()
        },
        ..plain_row(
            def("dictation-mode"),
            Control::Segmented {
                value: if p.toggle { "toggle" } else { "hold" }.to_string(),
                options: opts(&[("hold", "Hold"), ("toggle", "Toggle")]),
            },
        )
    };
    let key = plain_row(
        def("dictation-hotkey"),
        Control::Key {
            text: p.key_text.clone(),
            capturing: d.capturing,
        },
    );
    let mut mic_options = vec![Opt {
        id: String::new(),
        name: "System default".to_string(),
        face: String::new(),
    }];
    mic_options.extend(d.mics.iter().map(|(id, name)| Opt {
        id: id.clone(),
        name: name.clone(),
        face: String::new(),
    }));
    // A saved microphone that is not listed (unplugged, or the listing has not arrived) reads as System default,
    // as PT's MicField does: the select draws only the option whose id equals the value.
    let mic_value = if mic_options.iter().any(|o| o.id == p.mic) {
        p.mic.clone()
    } else {
        String::new()
    };
    let mic = plain_row(
        def("dictation-mic"),
        Control::Mic {
            value: mic_value,
            options: mic_options,
            testing: d.mic_testing,
            level: d.mic_level,
            problem: d.mic_problem.clone(),
        },
    );
    let limited = d.limited_model();
    let language = RowModel {
        sub: if limited {
            LIMITED_SUB.to_string()
        } else {
            def("dictation-language").sub.to_string()
        },
        row_tip: if limited {
            LIMITED_TEXT.to_string()
        } else {
            String::new()
        },
        ..plain_row(
            def("dictation-language"),
            Control::Select {
                value: if limited {
                    "auto".to_string()
                } else {
                    p.language.clone()
                },
                options: d
                    .languages
                    .iter()
                    .map(|(code, name)| Opt {
                        id: code.to_string(),
                        name: name.to_string(),
                        face: String::new(),
                    })
                    .collect(),
            },
        )
    };
    [mode, key, mic, language]
        .into_iter()
        .map(|r| {
            // A limited model picks its own language, so its picker is disabled (PT's `disabled={ownLanguage}`).
            let pinned = limited && r.id == "dictation-language";
            let mut row = dimmed(r, on);
            row.off |= pinned;
            BlockModel::Row(row)
        })
        .collect()
}

fn while_rows(d: &DictationData) -> Vec<BlockModel> {
    let on = d.prefs.enabled;
    vec![
        BlockModel::Row(pause_media_row(d)),
        BlockModel::Row(dimmed(
            switch_row(def("dictation-sounds"), d.prefs.sounds),
            on,
        )),
    ]
}

/// The media pause switch: off and dimmed, with the reason as its sub, where the system cannot pause (macOS).
fn pause_media_row(d: &DictationData) -> RowModel {
    let on = d.prefs.enabled;
    match &d.pause_media_unavailable {
        None => dimmed(
            switch_row(def("dictation-pause-media"), d.prefs.pause_media),
            on,
        ),
        Some(why) => {
            let mut row = dimmed(switch_row(def("dictation-pause-media"), false), on);
            row.off = true;
            row.sub = why.clone();
            row
        }
    }
}

/// One speech model's row: its state, its buttons, and the note that replaces its sub on a failure.
pub fn model_row(m: &ModelInfo, d: &DictationData) -> ModelRow {
    let item = d.item(m.id);
    let pack = d.installed(GPU_PACK_ID);
    let warn = m.needs_gpu && !pack && !d.gpu_built_in && item.failure.is_none();
    let (state, buttons) = match &item.state {
        ItemState::Downloading { pct } => (
            ModelState::Downloading { pct: *pct },
            vec![button("cancel", "Cancel")],
        ),
        ItemState::Installed if d.prefs.model == m.id => {
            (ModelState::Active, vec![button("uninstall", "Uninstall")])
        }
        ItemState::Installed => (
            ModelState::Installed,
            vec![button("use", "Use"), button("uninstall", "Uninstall")],
        ),
        ItemState::Absent => (ModelState::Absent, vec![button("download", "Download")]),
    };
    let sub = match &item.failure {
        Some(text) => text.clone(),
        None if warn => SLOW_NO_GPU.to_string(),
        None => m.note.to_string(),
    };
    ModelRow {
        id: m.id,
        name: m.name,
        size_text: m.size_text.clone(),
        vendor: m.vendor,
        recommended: m.id == d.recommended,
        badge: (state == ModelState::Active).then_some("Active"),
        sub,
        warn,
        state,
        failure: item.failure.clone(),
        buttons,
        uninstall_title: format!("Delete {} from this PC.", m.name),
    }
}

/// The GPU pack's row: Enable while absent, Enabled and Disable once installed.
pub fn gpu_row(d: &DictationData) -> GpuRow {
    let item = d.item(GPU_PACK_ID);
    let (state, badge, buttons) = match &item.state {
        ItemState::Downloading { pct } => (
            ModelState::Downloading { pct: *pct },
            None,
            vec![button("cancel", "Cancel")],
        ),
        ItemState::Installed => (
            ModelState::Installed,
            Some("Enabled"),
            vec![Button {
                alt: Some(GPU_OFF_TITLE.to_string()),
                ..button("disable", "Disable")
            }],
        ),
        ItemState::Absent => (ModelState::Absent, None, vec![button("enable", "Enable")]),
    };
    let failure = item
        .failure
        .clone()
        .or_else(|| d.gpu_fell_back.then(|| GPU_FELL_BACK.to_string()));
    GpuRow {
        state,
        failure,
        badge,
        sub: def("dictation-gpu").sub.to_string(),
        buttons,
        title: GPU_OFF_TITLE.to_string(),
    }
}

fn anywhere_row(d: &DictationData) -> RowModel {
    let on = d.prefs.enabled;
    let mut row = RowModel {
        row_tip: "Press the dictation key with any app in front. A widget shows what you say, then lets you pick a Claude session with a click or its number. Not in windows run as administrator.".to_string(),
        ..switch_row(def("dictation-anywhere"), d.prefs.anywhere && on)
    };
    if let Some(problem) = d
        .anywhere_problem
        .as_ref()
        .filter(|_| on && d.prefs.anywhere)
    {
        row.sub = problem.clone();
    }
    dimmed(row, on)
}

/// The Dictation page's cards in Prism Terminal's order: master, listening, while, models, then the
/// GPU card (NVIDIA only), then From any app.
pub fn sections(env: &PageEnv<'_>, d: &DictationData) -> Vec<SectionModel> {
    let mut out = vec![section("dictation", vec![BlockModel::Row(master(d))])];
    out.push(section("listening", listening_rows(d)));
    out.push(section("while", while_rows(d)));
    let models: Vec<BlockModel> = d
        .models
        .iter()
        .map(|m| BlockModel::Model(model_row(m, d)))
        .collect();
    out.push(section("models", models));
    if env.nvidia {
        out.push(section("gpu", vec![BlockModel::Gpu(gpu_row(d))]));
    }
    out.push(section("anywhere", vec![BlockModel::Row(anywhere_row(d))]));
    out
}

/// The map from a catalogue id to its state, for callers that build `DictationData` from a store.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::PrefStore;
    use crate::look::LookStore;
    use crate::page::{DiagInfo, GlassSupport, ThemeColours};
    use crate::verb::VerbState;

    fn face(_: &str) -> String {
        String::new()
    }

    /// A page environment with the given NVIDIA state; everything else is empty.
    fn with_env<R>(nvidia: bool, f: impl FnOnce(&PageEnv<'_>) -> R) -> R {
        let prefs = PrefStore::memory();
        let look = LookStore::new(PrefStore::memory());
        let face_of: &dyn Fn(&str) -> String = &face;
        let env = PageEnv {
            prefs: &prefs,
            look: &look,
            shells: None,
            verb: VerbState::Busy,
            home: String::new(),
            version: "0.0.0".to_string(),
            copyright: String::new(),
            glass: GlassSupport::Ok,
            diag: DiagInfo {
                verbose: false,
                dir: String::new(),
            },
            diag_marked: false,
            nvidia,
            dictation_wired: true,
            theme_colours: ThemeColours {
                accent: String::new(),
                background: String::new(),
                secondary: String::new(),
            },
            agent_colours: [String::new(), String::new(), String::new()],
            agent_key_labels: String::new(),
            face_of,
            mods: None,
        };
        f(&env)
    }

    fn models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "base",
                name: "Whisper Base",
                size_text: "148 MB".to_string(),
                vendor: Vendor::Openai,
                needs_gpu: false,
                limited: false,
                note: "Fast live text on any PC.",
            },
            ModelInfo {
                id: "large-v3-turbo",
                name: "Whisper Large v3 Turbo",
                size_text: "1.6 GB".to_string(),
                vendor: Vendor::Openai,
                needs_gpu: true,
                limited: false,
                note: "Close to Large v3 at half the download.",
            },
            ModelInfo {
                id: "parakeet-v3",
                name: "Parakeet v3",
                size_text: "356 MB".to_string(),
                vendor: Vendor::Nvidia,
                needs_gpu: false,
                limited: true,
                note: "Very fast on an NVIDIA card.",
            },
        ]
    }

    fn data(enabled: bool) -> DictationData {
        DictationData {
            models: models(),
            recommended: "base",
            items: Vec::new(),
            mics: vec![("mic-1".to_string(), "Headset".to_string())],
            languages: vec![("auto", "Auto-detect"), ("en", "English"), ("de", "German")],
            prefs: Prefs {
                enabled,
                ..Prefs::default()
            },
            capturing: false,
            mic_testing: false,
            mic_level: 0.0,
            mic_problem: None,
            gpu_fell_back: false,
            ask: None,
            stored_limited: false,
            pause_media_unavailable: None,
            anywhere_problem: None,
            gpu_built_in: false,
        }
    }

    fn installed(id: &str) -> (String, ItemInfo) {
        (
            id.to_string(),
            ItemInfo {
                state: ItemState::Installed,
                failure: None,
            },
        )
    }

    fn ids_of(sections: &[SectionModel]) -> Vec<&'static str> {
        sections.iter().map(|s| s.id).collect()
    }

    fn rows(sections: &[SectionModel]) -> Vec<&RowModel> {
        sections
            .iter()
            .flat_map(|s| s.blocks.iter())
            .filter_map(|b| match b {
                BlockModel::Row(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Every text the page says, except the uninstall and the pack's titles (the only words allowed to
    /// name a removal).
    fn texts(sections: &[SectionModel]) -> Vec<String> {
        let mut out = Vec::new();
        for s in sections {
            out.push(s.title.clone());
            for b in &s.blocks {
                match b {
                    BlockModel::Row(r) => {
                        out.push(r.label.clone());
                        out.push(r.sub.clone());
                        out.push(r.row_tip.clone());
                        out.extend(r.buttons.iter().map(|x| x.label.clone()));
                    }
                    BlockModel::Model(m) => {
                        out.push(m.name.to_string());
                        out.push(m.sub.clone());
                        out.extend(m.buttons.iter().map(|x| x.label.clone()));
                        out.extend(m.badge.map(str::to_string));
                    }
                    BlockModel::Gpu(g) => {
                        out.push(g.sub.clone());
                        out.extend(g.buttons.iter().map(|x| x.label.clone()));
                        out.extend(g.badge.map(str::to_string));
                    }
                    BlockModel::Note(t) => out.push(t.clone()),
                    _ => {}
                }
            }
        }
        out
    }

    #[test]
    fn the_cards_come_in_pt_order_then_from_any_app() {
        with_env(true, |env| {
            let s = sections(env, &data(true));
            assert_eq!(
                ids_of(&s),
                [
                    "dictation",
                    "listening",
                    "while",
                    "models",
                    "gpu",
                    "anywhere"
                ]
            );
        });
        with_env(false, |env| {
            let s = sections(env, &data(true));
            assert_eq!(
                ids_of(&s),
                ["dictation", "listening", "while", "models", "anywhere"]
            );
        });
    }

    #[test]
    fn rows_dim_while_off() {
        with_env(false, |env| {
            let off = sections(env, &data(false));
            for r in rows(&off) {
                assert_eq!(r.off, r.id != "dictation-enabled", "{}", r.id);
            }
            for s in &off {
                for b in &s.blocks {
                    if let BlockModel::Model(m) = b {
                        assert!(
                            m.buttons.iter().all(|b| !b.off),
                            "model {} buttons stay live while off (F2)",
                            m.id
                        );
                    }
                }
            }
            // The GPU card is live too (PT's DictationPage disables only Listening and While).
            with_env(true, |env| {
                let mut d = data(false);
                d.items.push(installed(GPU_PACK_ID));
                let mut seen = 0;
                for s in &sections(env, &d) {
                    for b in &s.blocks {
                        if let BlockModel::Gpu(g) = b {
                            assert!(
                                g.buttons.iter().all(|b| !b.off),
                                "the GPU row stays live while off (F2)"
                            );
                            seen += 1;
                        }
                    }
                }
                assert_eq!(seen, 1);
            });
            let on = sections(env, &data(true));
            assert!(rows(&on).iter().all(|r| !r.off));
        });
    }

    #[test]
    fn no_model_shows_the_warning() {
        with_env(false, |env| {
            let none = sections(env, &data(true));
            let master = rows(&none)[0];
            assert!(master.warn);
            assert_eq!(master.sub, NO_MODEL);
            let mut d = data(true);
            d.items.push(installed("base"));
            let some = sections(env, &d);
            let master = rows(&some)[0];
            assert!(!master.warn);
            assert_eq!(master.sub, "Speech to text on this PC, never sent.");
        });
    }

    #[test]
    fn the_warning_waits_for_the_switch_as_pt_does() {
        // PT: `noModelYet = d.enabled && d.noModel`. Off with no model reads the plain line.
        with_env(false, |env| {
            let off = sections(env, &data(false));
            let master = rows(&off)[0];
            assert!(!master.warn);
            assert_eq!(master.sub, "Speech to text on this PC, never sent.");
        });
    }

    #[test]
    fn the_limited_language_row_keeps_the_stored_value() {
        with_env(false, |env| {
            let mut d = data(true);
            d.prefs.language = "de".to_string();
            d.prefs.model = "parakeet-v3".to_string();
            d.items.push(installed("parakeet-v3"));
            let s = sections(env, &d);
            let lang = rows(&s)
                .into_iter()
                .find(|r| r.id == "dictation-language")
                .expect("language row");
            assert_eq!(lang.sub, LIMITED_SUB);
            assert_eq!(lang.row_tip, LIMITED_TEXT);
            assert!(matches!(
                &lang.control,
                Control::Select { value, .. } if value == "auto"
            ));
            // The stored choice is not touched by the page, so a Whisper model brings it back.
            assert_eq!(d.prefs.language, "de");
        });
    }

    #[test]
    fn the_limited_language_picker_is_disabled_with_dictation_on() {
        // PT's language select is `disabled={ownLanguage}` (M8 Task 29): a limited model picks its own language.
        with_env(false, |env| {
            let mut d = data(true);
            d.prefs.model = "parakeet-v3".to_string();
            d.items.push(installed("parakeet-v3"));
            let s = sections(env, &d);
            let lang = rows(&s)
                .into_iter()
                .find(|r| r.id == "dictation-language")
                .expect("language row");
            assert!(lang.off, "a limited model disables the picker");
            // Another model leaves the picker on.
            d.prefs.model = "base".to_string();
            let s = sections(env, &d);
            let lang = rows(&s)
                .into_iter()
                .find(|r| r.id == "dictation-language")
                .expect("language row");
            assert!(!lang.off);
        });
    }

    #[test]
    fn a_saved_microphone_that_is_not_listed_reads_as_system_default() {
        with_env(false, |env| {
            let mic_value = |d: &DictationData| {
                let s = sections(env, d);
                let r = rows(&s)
                    .into_iter()
                    .find(|r| r.id == "dictation-mic")
                    .expect("mic row")
                    .clone();
                match r.control {
                    Control::Mic { value, .. } => value,
                    other => panic!("not a mic: {other:?}"),
                }
            };
            let mut d = data(true);
            d.prefs.mic = "mic-1".to_string();
            assert_eq!(mic_value(&d), "mic-1", "listed: shown");
            d.prefs.mic = "gone".to_string();
            assert_eq!(mic_value(&d), "", "replugged under a new id");
            d.mics.clear();
            d.prefs.mic = "mic-1".to_string();
            assert_eq!(mic_value(&d), "", "the listing has not arrived");
            d.prefs.mic.clear();
            assert_eq!(mic_value(&d), "");
        });
    }

    #[test]
    fn the_gpu_row_needs_nvidia() {
        with_env(false, |env| {
            let s = sections(env, &data(true));
            assert!(!ids_of(&s).contains(&"gpu"));
        });
        with_env(true, |env| {
            let mut d = data(true);
            d.items.push(installed(GPU_PACK_ID));
            let s = sections(env, &d);
            let gpu: Vec<&GpuRow> = s
                .iter()
                .flat_map(|sec| sec.blocks.iter())
                .filter_map(|b| match b {
                    BlockModel::Gpu(g) => Some(g),
                    _ => None,
                })
                .collect();
            assert_eq!(gpu.len(), 1);
            assert_eq!(gpu[0].badge, Some("Enabled"));
            assert_eq!(gpu[0].buttons[0].label, "Disable");
            assert_eq!(gpu[0].title, GPU_OFF_TITLE);
            assert!(GPU_OFF_TITLE.contains("675 MB"));
        });
    }

    #[test]
    fn nothing_says_delete_or_remove_but_the_uninstall_title() {
        with_env(true, |env| {
            let mut d = data(true);
            d.items.push(installed("base"));
            d.items.push(installed(GPU_PACK_ID));
            d.prefs.model = "base".to_string();
            let s = sections(env, &d);
            for t in texts(&s) {
                let lower = t.to_lowercase();
                assert!(!lower.contains("delete"), "{t}");
                assert!(!lower.contains("remove"), "{t}");
            }
            let base = s
                .iter()
                .flat_map(|sec| sec.blocks.iter())
                .find_map(|b| match b {
                    BlockModel::Model(m) if m.id == "base" => Some(m),
                    _ => None,
                })
                .expect("base row");
            assert_eq!(base.uninstall_title, "Delete Whisper Base from this PC.");
        });
    }

    #[test]
    fn the_anywhere_switch_is_off_while_dictation_is_off() {
        with_env(false, |env| {
            let off = sections(env, &data(false));
            let any = rows(&off)
                .into_iter()
                .find(|r| r.id == "dictation-anywhere")
                .expect("anywhere row");
            assert!(any.off);
            assert_eq!(any.control, Control::Switch { on: false });
            let on = sections(env, &data(true));
            let any = rows(&on)
                .into_iter()
                .find(|r| r.id == "dictation-anywhere")
                .expect("anywhere row");
            assert!(!any.off);
            assert_eq!(any.control, Control::Switch { on: true });
            assert!(any.row_tip.starts_with("Press the dictation key"));
        });
    }

    #[test]
    fn where_media_cannot_be_paused_the_switch_is_off_and_says_why() {
        with_env(false, |env| {
            let row = |d: &DictationData| {
                rows(&sections(env, d))
                    .into_iter()
                    .find(|r| r.id == "dictation-pause-media")
                    .expect("pause row")
                    .clone()
            };
            let mut d = data(true);
            d.prefs.pause_media = true;
            let win = row(&d);
            assert!(!win.off);
            assert_eq!(win.control, Control::Switch { on: true });
            assert_eq!(win.sub, "Resumes when you stop.");
            // macOS: no public API; a stored "on" (an import from Windows) shows off, with the reason.
            d.pause_media_unavailable = Some("Not on a Mac.".to_string());
            let mac = row(&d);
            assert!(mac.off);
            assert_eq!(mac.control, Control::Switch { on: false });
            assert_eq!(mac.sub, "Not on a Mac.");
        });
    }

    #[test]
    fn what_stops_dictating_from_any_app_replaces_the_rows_sub_only_while_it_is_on() {
        with_env(false, |env| {
            let row = |d: &DictationData| {
                rows(&sections(env, d))
                    .into_iter()
                    .find(|r| r.id == "dictation-anywhere")
                    .expect("anywhere row")
                    .clone()
            };
            let mut d = data(true);
            assert_eq!(row(&d).sub, "The key works with another app in front.");
            d.anywhere_problem = Some("Needs Input Monitoring.".to_string());
            assert_eq!(row(&d).sub, "Needs Input Monitoring.");
            assert!(
                !row(&d).off,
                "the switch still works: the window reads the key meanwhile"
            );
            d.prefs.anywhere = false;
            assert_eq!(row(&d).sub, "The key works with another app in front.");
        });
    }

    #[test]
    fn the_toggle_mode_names_its_own_sub() {
        with_env(false, |env| {
            let mut d = data(true);
            d.prefs.toggle = true;
            let s = sections(env, &d);
            let mode = rows(&s)
                .into_iter()
                .find(|r| r.id == "dictation-mode")
                .expect("mode row");
            assert_eq!(mode.sub, TOGGLE_SUB);
        });
    }

    #[test]
    fn a_large_model_without_the_pack_warns_and_the_active_one_offers_uninstall() {
        with_env(false, |env| {
            let mut d = data(true);
            d.items.push(installed("large-v3-turbo"));
            d.prefs.model = "large-v3-turbo".to_string();
            let s = sections(env, &d);
            let turbo = s
                .iter()
                .flat_map(|sec| sec.blocks.iter())
                .find_map(|b| match b {
                    BlockModel::Model(m) if m.id == "large-v3-turbo" => Some(m),
                    _ => None,
                })
                .expect("turbo row");
            assert!(turbo.warn);
            assert_eq!(turbo.sub, SLOW_NO_GPU);
            assert_eq!(turbo.state, ModelState::Active);
            assert_eq!(turbo.badge, Some("Active"));
            assert_eq!(turbo.buttons.len(), 1);
            assert_eq!(turbo.buttons[0].id, "uninstall");
        });
    }

    #[test]
    fn a_large_model_never_warns_where_the_engine_has_a_gpu_of_its_own() {
        // macOS (rule 313): Metal is in the bundled engine, there is no pack, and Turbo is not slow.
        with_env(false, |env| {
            let mut d = data(true);
            d.gpu_built_in = true;
            d.recommended = "large-v3-turbo";
            let s = sections(env, &d);
            let rows: Vec<&ModelRow> = s
                .iter()
                .flat_map(|sec| sec.blocks.iter())
                .filter_map(|b| match b {
                    BlockModel::Model(m) => Some(m),
                    _ => None,
                })
                .collect();
            let turbo = rows
                .iter()
                .find(|m| m.id == "large-v3-turbo")
                .expect("turbo");
            assert!(!turbo.warn);
            assert_ne!(turbo.sub, SLOW_NO_GPU);
            assert!(turbo.recommended);
            assert!(rows.iter().all(|m| !m.warn), "no row warns");
            // No NVIDIA card, so no GPU card.
            assert!(s.iter().all(|sec| sec.id != "gpu"));
        });
    }
}
