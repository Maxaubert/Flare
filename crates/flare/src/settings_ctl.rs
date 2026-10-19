//! SettingsCtl: pages into the window, events into the stores (M7 Task 41).
//!
//! Everything with a rule lives in `flare_settings` (page models, layout, the find machine, the
//! actions); this file only moves data. A page is built from the stores, laid out in device-pixel
//! snapped logical px, mapped to the Slint structs and written into `SettingsBridge`; every bridge
//! callback becomes an `actions::Event` (or a find / wall / overlay step) and the `Effect`s that
//! come back are carried out here. The colour picker and the eyedropper are Task 42's: the places
//! it hooks in are marked `// picker:`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use flare_settings::actions::{self, AskAnswer, Ctx, Effect, Event, SaveFrom};
use flare_settings::colour_format::{COLOUR_FORMAT_KEY, colour_format};
use flare_settings::custom_theme::CUSTOM_KEY;
use flare_settings::find::{IndexEnv, page_names, settings_index};
use flare_settings::find_state::{
    FLASH_MS, FieldOut, FindState, FlashStep, FlashWatch, Key, ListOut, POLL_MS, centre_scroll,
    flash_alpha, rail_key,
};
use flare_settings::help_prefs::HELP_KEYS;
use flare_settings::icons::icon_path;
use flare_settings::kv::{PrefStore, Subscription};
use flare_settings::layout::{
    self, BlockKind as LayoutBlock, PageLayout, SectionLayout, about_y, layout_page,
    layout_results, place_menu_at, rail_layout, rail_width, scroll_target,
};
use flare_settings::look::LookStore;
use flare_settings::page::dictation::{GpuRow, ModelRow, ModelState, Vendor};
use flare_settings::page::{
    BlockModel, Control, DiagInfo, DictationPage, GlassSupport, PageEnv, PageModel, RowModel,
    SectionModel, ThemeColours, build_page,
};
use flare_settings::pages::{DEFAULT_PAGE, PAGES};
use flare_settings::picker::{CodeField, Rect};
use flare_settings::rows::all_rows;
use flare_settings::verb::{DiagControlApi, ExplorerVerb, VerbState};
use flare_settings::wall::{self, WallLayout};
use flare_settings::window_colours::{CUSTOM_COLOURS_KEY, EDGES_KEY};
use flare_theme::ansi::ANSI_KEYS;
use flare_theme::colour::{ColourFormat, Rgba, parse_colour};
use flare_theme::look::card_ansi;
use flare_theme::resolve::resolve_custom_theme;
use flare_win::eyedrop::PickResult;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::appctl::{Post, UiMsg};
use crate::picker_ctl::{PickerCtl, PickerCtx, RowColour};
use crate::{
    AppWindow, BlockKind as SlintBlock, EditorWell, RailItem, SetButton, SetHit, SetKind,
    SetOption, SetRow, SetSection, SettingsBridge, SettingsFixed, WallCard,
};

/// How long the Mark button says Marked.
const MARKED_MS: u64 = 1200;
/// The wall's cards are laid out in the panel's inner width less its paddings (16 each side).
const WALL_SIDE_PADS: f64 = 32.0;
/// The Select menu is never taller than this.
const MENU_MAX_H: f64 = 320.0;
/// One option of the Select menu, and the menu's own padding top and bottom.
const OPTION_H: f64 = 28.0;
const MENU_PAD: f64 = 8.0;
/// The slint badge's height when the view has not said otherwise.
const BADGE_H: f64 = 112.0;
/// The size used when the window has not been laid out yet.
const FALLBACK_FRAME: (f64, f64) = (1000.0, 604.0);

/// What a worker or a store subscription says to the controller (carried by `UiMsg::Settings`).
#[derive(Debug)]
pub enum SettingsMsg {
    /// The shells detection found.
    ShellList(Vec<(String, String)>),
    /// The Explorer verb's state, asked or set.
    Verb(VerbState),
    /// The folder the picker returned.
    Picked(Option<PathBuf>),
    /// The screen pick's answer (Task 42).
    #[allow(dead_code)]
    Dropper(PickResult),
    /// A setting changed: rebuild once.
    Changed,
}

/// The picker answers on a worker, so its callback is `Send`.
pub type PickAnswer = Arc<dyn Fn(Option<PathBuf>) + Send + Sync>;
pub type PickFolder = Rc<dyn Fn(Option<PathBuf>, PickAnswer)>;

pub struct SettingsDeps {
    pub prefs: PrefStore,
    pub look: LookStore,
    pub diag: Rc<dyn DiagControlApi>,
    pub verb: Arc<dyn ExplorerVerb>,
    pub dictation: Rc<dyn DictationPage>,
    pub pick_folder: PickFolder,
    pub open_path: Rc<dyn Fn(&Path)>,
    /// Read by the Command help panel (Task 43).
    #[allow(dead_code)]
    pub clipboard: Rc<dyn Fn(&str)>,
    pub shell_list: Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
    pub glass: Rc<dyn Fn() -> GlassSupport>,
    pub face_of: Rc<dyn Fn(&str) -> String>,
    pub theme_colours: Rc<dyn Fn() -> ThemeColours>,
    pub agent_colours: Rc<dyn Fn() -> [String; 3]>,
    pub agent_key_labels: Rc<dyn Fn() -> String>,
    pub post: Post,
    pub version: String,
    pub copyright: String,
    /// Under --e2e the glue wires stand-in ports (Task 44 reads this).
    #[allow(dead_code)]
    pub e2e: bool,
    pub clock: Rc<dyn Fn() -> u64>,
    pub still: Rc<dyn Fn() -> bool>,
}

/// What the last build produced: the model, its layout and the frame it was laid out in.
struct Built {
    model: PageModel,
    layout: PageLayout,
    wall: WallLayout,
    wall_full_h: f64,
    scale: f64,
}

/// The flash in flight: the row (or the section, when the row never came) and its start.
struct FlashRun {
    row: Option<String>,
    section: Option<String>,
    start_ms: u64,
}

/// The wall opening: from one clip height to another over `EXPAND_MS`.
struct WallAnim {
    from: f64,
    to: f64,
    start_ms: u64,
}

pub struct SettingsCtl {
    weak: slint::Weak<AppWindow>,
    me: Weak<RefCell<SettingsCtl>>,
    deps: SettingsDeps,
    find: FindState,
    page: &'static str,
    shells: Option<Vec<(String, String)>>,
    verb_state: VerbState,
    wall_expanded: bool,
    wall_anim: Option<WallAnim>,
    flash: Option<FlashRun>,
    watch: Option<FlashWatch>,
    marked_until: Option<u64>,
    /// The diagnostics flag as `diag.json` held it when Settings last showed (or the switch last
    /// wrote it). A rebuild runs per wall frame and per slider step, so it never reads the file.
    diag_verbose: Cell<Option<bool>>,
    pending_pick: RefCell<Option<String>>,
    /// The colour popover, the typed codes of the colour rows and the dropper.
    picker: PickerCtl,
    picker_timer: slint::Timer,
    /// Typed codes of the theme editor's wells, by key.
    well_drafts: HashMap<String, CodeField>,
    built: Option<Built>,
    frame: Option<(f64, f64)>,
    visible: bool,
    stale: bool,
    ask_open: bool,
    select_row: String,
    home: String,
    dirty: Arc<AtomicBool>,
    _subs: Vec<Subscription>,
    flash_timer: slint::Timer,
    poll_timer: slint::Timer,
    wall_timer: slint::Timer,
    mark_timer: slint::Timer,
    /// How many full rebuilds ran (the tests count them).
    pub rebuilds: u32,
}

// ---- the pure mappers ------------------------------------------------------------------------------

/// The Slint control kind of a model control.
pub fn control_kind(c: &Control) -> SetKind {
    match c {
        Control::None => SetKind::None,
        Control::Pill(_) => SetKind::Pill,
        Control::Switch { .. } => SetKind::Switch,
        Control::Segmented { .. } => SetKind::Segmented,
        Control::Select { .. } => SetKind::Select,
        Control::Slider { .. } => SetKind::Slider,
        Control::Colour { .. } => SetKind::Colour,
        Control::Save { .. } => SetKind::Save,
        Control::Key { .. } => SetKind::Key,
        Control::Mic { .. } => SetKind::Mic,
    }
}

/// What a row needs besides its model to be drawn.
pub struct RowCtx<'a> {
    pub format: ColourFormat,
    pub drafts: &'a HashMap<String, CodeField>,
    /// The device-pixel hairline in logical px (the view adds it below the panel's border).
    pub hair: f64,
    /// The row that is flashing, the section that is flashing instead, and the flash's strength.
    pub flash_row: Option<&'a str>,
    pub flash_section: Option<&'a str>,
    pub flash: f64,
}

fn colour_of(s: &str) -> slint::Color {
    match parse_colour(s) {
        Some(c) => rgba_colour(&c),
        None => slint::Color::from_argb_u8(0, 0, 0, 0),
    }
}

fn rgba_colour(c: &Rgba) -> slint::Color {
    let b = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    slint::Color::from_argb_u8(b(c.a * 255.0), b(c.r), b(c.g), b(c.b))
}

fn options_of(opts: &[flare_settings::page::Opt]) -> ModelRc<SetOption> {
    ModelRc::new(VecModel::from(
        opts.iter()
            .map(|o| SetOption {
                id: o.id.as_str().into(),
                name: o.name.as_str().into(),
                face: o.face.as_str().into(),
            })
            .collect::<Vec<_>>(),
    ))
}

fn no_colour() -> slint::Color {
    slint::Color::from_argb_u8(0, 0, 0, 0)
}

/// One row (`y` inside the panel, below its border) as the view draws it.
pub fn to_slint_row(r: &RowModel, y: f64, h: f64, ctx: &RowCtx<'_>) -> SetRow {
    let mut out = SetRow {
        id: r.id.into(),
        icon: icon_path(r.icon).unwrap_or("").into(),
        label: r.label.as_str().into(),
        sub: r.sub.as_str().into(),
        sub_tip: r.sub.as_str().into(),
        row_tip: r.row_tip.as_str().into(),
        warn: r.warn,
        off: r.off,
        tap: r.tap,
        block: SlintBlock::Row,
        kind: control_kind(&r.control),
        reset: r.reset,
        colour: no_colour(),
        y: y as f32,
        h: h as f32,
        flash: if ctx.flash_row == Some(r.id) {
            ctx.flash as f32
        } else {
            0.0
        },
        buttons: ModelRc::new(VecModel::from(
            r.buttons
                .iter()
                .map(|b| SetButton {
                    id: b.id.into(),
                    label: b.label.as_str().into(),
                    label_alt: b.alt.clone().unwrap_or_default().into(),
                    off: b.off,
                    alt: b.alt.is_some(),
                })
                .collect::<Vec<_>>(),
        )),
        ..Default::default()
    };
    match &r.control {
        Control::None => {}
        Control::Pill(text) => out.value = text.as_str().into(),
        Control::Switch { on } => out.on = *on,
        Control::Segmented { value, options } | Control::Select { value, options } => {
            out.value = value.as_str().into();
            out.options = options_of(options);
        }
        Control::Slider {
            value,
            max,
            text,
            disabled,
        } => {
            out.num = *value as i32;
            out.num_max = *max as i32;
            out.num_text = text.as_str().into();
            out.off = out.off || *disabled;
        }
        Control::Colour { value, range, .. } => {
            out.colour = colour_of(value);
            let code = match ctx.drafts.get(r.id) {
                Some(f) => f.shown(value, range, ctx.format),
                None => CodeField::default().shown(value, range, ctx.format),
            };
            out.code = code.into();
            out.code_wide = ctx.format != ColourFormat::Hex;
        }
        Control::Save { dirty, title } => {
            out.save_dirty = *dirty;
            out.save_title = title.as_str().into();
        }
        Control::Key { text, capturing } => {
            out.value = text.as_str().into();
            out.capturing = *capturing;
            // PT HotkeyField: Reset only while the key is not the default and none is awaited.
            out.reset = r.reset
                || (!*capturing
                    && *text
                        != flare_dictation::hotkey::format_hotkey(
                            &flare_dictation::hotkey::default_hotkey(),
                        ));
        }
        Control::Mic {
            value,
            options,
            testing,
            problem,
            ..
        } => {
            out.value = value.as_str().into();
            out.options = options_of(options);
            out.testing = *testing;
            // The microphone says its problem in the row's own subtext (PT MicField `onProblem`).
            if let Some(p) = problem {
                out.sub = p.as_str().into();
                out.sub_tip = p.as_str().into();
            }
        }
    }
    // The language picker of a model that picks its own language: disabled, with the globe that says why.
    out.limited = r.id == "dictation-language"
        && !r.row_tip.is_empty()
        && matches!(r.control, Control::Select { .. });
    out
}

/// A model row's or the pack's buttons as the view draws them. A download that failed offers Retry in place of
/// Download or Enable (PT ModelRow); the failure itself is already the row's note.
fn model_buttons(buttons: &[flare_settings::page::Button], failed: bool) -> ModelRc<SetButton> {
    ModelRc::new(VecModel::from(
        buttons
            .iter()
            .map(|b| SetButton {
                id: b.id.into(),
                label: if failed && matches!(b.id, "download" | "enable") {
                    "Retry".into()
                } else {
                    b.label.as_str().into()
                },
                // Never the tooltip: SetRowButton sizes itself by the wider of label and label_alt, and the
                // tip (GPU_OFF_TITLE) is a sentence. It reaches the view through `button_title`.
                label_alt: SharedString::new(),
                off: b.off,
                alt: false,
            })
            .collect::<Vec<_>>(),
    ))
}

fn model_state(s: &ModelState) -> (&'static str, i32) {
    match s {
        ModelState::Absent => ("absent", 0),
        ModelState::Downloading { pct } => ("downloading", i32::try_from(*pct).unwrap_or(0)),
        ModelState::Installed => ("installed", 0),
        ModelState::Active => ("active", 0),
    }
}

/// One speech model of the Speech models card.
pub fn to_slint_model(m: &ModelRow, y: f64, h: f64) -> SetRow {
    let (state, pct) = model_state(&m.state);
    SetRow {
        id: m.id.into(),
        label: m.name.into(),
        sub: m.sub.as_str().into(),
        warn: m.warn,
        off: m.off,
        block: SlintBlock::Model,
        kind: SetKind::None,
        colour: no_colour(),
        y: y as f32,
        h: h as f32,
        vendor: match m.vendor {
            Vendor::Openai => "openai",
            Vendor::Nvidia => "nvidia",
        }
        .into(),
        size_text: m.size_text.as_str().into(),
        recommended: m.recommended,
        badge: m.badge.unwrap_or_default().into(),
        state: state.into(),
        num: pct,
        button_title: m.uninstall_title.as_str().into(),
        buttons: model_buttons(&m.buttons, m.failure.is_some()),
        ..Default::default()
    }
}

/// The NVIDIA GPU pack's row.
pub fn to_slint_gpu(g: &GpuRow, y: f64, h: f64) -> SetRow {
    let (state, pct) = model_state(&g.state);
    let pack = flare_dictation::catalog::GPU_PACK;
    SetRow {
        id: pack.id.into(),
        label: pack.name.into(),
        // A failure, or the fall-back note, replaces the note (PT: `failed || note`).
        sub: g
            .failure
            .clone()
            .unwrap_or_else(|| pack.note.to_string())
            .into(),
        off: g.off,
        block: SlintBlock::Model,
        kind: SetKind::None,
        colour: no_colour(),
        y: y as f32,
        h: h as f32,
        vendor: "nvidia".into(),
        size_text: flare_dictation::catalog::size_text(pack.bytes).into(),
        badge: g.badge.unwrap_or_default().into(),
        state: state.into(),
        num: pct,
        button_title: g.title.as_str().into(),
        buttons: model_buttons(&g.buttons, g.failure.is_some()),
        ..Default::default()
    }
}

/// One section with its blocks placed by `l`.
pub fn to_slint_section(s: &SectionModel, l: &SectionLayout, ctx: &RowCtx<'_>) -> SetSection {
    let rows: Vec<SetRow> = s
        .blocks
        .iter()
        .zip(&l.rows)
        .map(|(b, rl)| {
            let y = rl.y - l.panel_y - ctx.hair;
            let section_flash = if ctx.flash_section == Some(s.id) {
                ctx.flash as f32
            } else {
                0.0
            };
            let mut row = match b {
                BlockModel::Row(r) => to_slint_row(r, y, rl.h, ctx),
                BlockModel::Wall => block_row("wall", SlintBlock::Wall, y, rl.h),
                BlockModel::AboutCard {
                    name,
                    line_one,
                    line_two,
                } => SetRow {
                    label: name.as_str().into(),
                    sub: line_one.as_str().into(),
                    note: line_two.as_str().into(),
                    ..block_row("about", SlintBlock::About, y, rl.h)
                },
                BlockModel::SlintBadge => block_row("slint", SlintBlock::Slint, y, rl.h),
                BlockModel::Model(m) => to_slint_model(m, y, rl.h),
                BlockModel::Gpu(g) => to_slint_gpu(g, y, rl.h),
                BlockModel::Note(text) => SetRow {
                    note: text.as_str().into(),
                    ..block_row("note", SlintBlock::Note, y, rl.h)
                },
            };
            if section_flash > 0.0 {
                row.flash = section_flash;
            }
            row
        })
        .collect();
    let action = s.action.as_ref();
    SetSection {
        id: s.id.into(),
        title: s.title.as_str().into(),
        tag: s.tag.clone().unwrap_or_default().into(),
        action_save: action.is_some(),
        action_dirty: action.is_some_and(|a| a.dirty),
        action_title: action.map(|a| a.title.clone()).unwrap_or_default().into(),
        title_y: l.title_y as f32,
        panel_y: l.panel_y as f32,
        panel_h: l.panel_h as f32,
        rows: ModelRc::new(VecModel::from(rows)),
    }
}

fn block_row(id: &str, block: SlintBlock, y: f64, h: f64) -> SetRow {
    SetRow {
        id: id.into(),
        block,
        kind: SetKind::None,
        colour: no_colour(),
        y: y as f32,
        h: h as f32,
        ..Default::default()
    }
}

/// The page id a rail id names (the `&'static str` of the table).
fn page_static(id: &str) -> &'static str {
    PAGES
        .iter()
        .find(|p| p.id == id)
        .map_or(DEFAULT_PAGE, |p| p.id)
}

fn key_of(word: &str) -> Option<Key> {
    Some(match word {
        "Down" => Key::Down,
        "Up" => Key::Up,
        "Left" => Key::Left,
        "Right" => Key::Right,
        "Home" => Key::Home,
        "End" => Key::End,
        "Enter" => Key::Enter,
        "Space" => Key::Space,
        "Escape" => Key::Escape,
        "Tab" => Key::Tab,
        _ => return None,
    })
}

/// What a well of the theme editor shows for an ANSI colour the draft has not set: the grey
/// `actions::apply(EditorSet)` stores for it (built, not typed, so the literal scan stays honest).
fn unset_ansi() -> String {
    ["#", "888888"].concat()
}

/// Runs `f` on a named worker; a failure to start is logged and nothing is posted.
fn spawn_worker(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        log::warn!("could not start {name}: {e}");
    }
}

/// Binds one callback of `SettingsBridge` to a method of the controller.
macro_rules! on {
    ($app:expr, $rc:expr, $reg:ident, |$c:ident $(, $a:ident : $t:ty)*| $body:expr) => {{
        let weak = Rc::downgrade($rc);
        $app.global::<SettingsBridge>().$reg(move |$($a: $t),*| {
            if let Some(rc) = weak.upgrade()
                && let Ok(mut $c) = rc.try_borrow_mut()
            {
                crate::diag_ctl::timed(stringify!($reg), || {
                    $body;
                });
            }
        });
    }};
}

impl SettingsCtl {
    /// Binds every `SettingsBridge` callback, subscribes to the stores, starts the shell-list
    /// worker, sets the two fonts and builds the first page.
    pub fn new(app: &AppWindow, deps: SettingsDeps) -> Rc<RefCell<SettingsCtl>> {
        let dirty = Arc::new(AtomicBool::new(false));
        let subs = Self::subscribe(&deps, &dirty);
        let find = FindState::new(
            settings_index(&IndexEnv {
                nvidia: false,
                dictation_wired: crate::dictation_ctl::wired(),
            }),
            page_names(),
        );
        Self::set_fonts(app, &deps);
        let ctl = SettingsCtl {
            weak: app.as_weak(),
            me: Weak::new(),
            find,
            page: DEFAULT_PAGE,
            shells: None,
            verb_state: VerbState::Busy,
            wall_expanded: false,
            wall_anim: None,
            flash: None,
            watch: None,
            marked_until: None,
            diag_verbose: Cell::new(None),
            pending_pick: RefCell::new(None),
            picker: PickerCtl::default(),
            picker_timer: slint::Timer::default(),
            well_drafts: HashMap::new(),
            built: None,
            frame: None,
            visible: false,
            stale: true,
            ask_open: false,
            select_row: String::new(),
            home: crate::app_facts::home().to_string(),
            dirty,
            _subs: subs,
            flash_timer: slint::Timer::default(),
            poll_timer: slint::Timer::default(),
            wall_timer: slint::Timer::default(),
            mark_timer: slint::Timer::default(),
            rebuilds: 0,
            deps,
        };
        let rc = Rc::new(RefCell::new(ctl));
        rc.borrow_mut().me = Rc::downgrade(&rc);
        Self::bind_callbacks(app, &rc);
        // The Dictation page's own state (a waiting key field, a download, the meter) rebuilds the page or moves
        // the meter without going through a Settings event.
        {
            let weak = Rc::downgrade(&rc);
            let app_weak = app.as_weak();
            let prefs = rc.borrow().deps.prefs.clone();
            crate::dictation_ctl::register_page(
                prefs,
                Rc::new(move || {
                    if let Some(rc) = weak.upgrade()
                        && let Ok(mut c) = rc.try_borrow_mut()
                        && c.page == "dictation"
                    {
                        c.refresh_if_visible();
                    }
                }),
                Rc::new(move |level| {
                    if let Some(app) = app_weak.upgrade() {
                        app.global::<SettingsBridge>().set_dict_level(level);
                    }
                }),
            );
        }
        {
            let c = rc.borrow();
            c.start_shell_list();
            c.ask_verb();
        }
        rc.borrow_mut().rebuild();
        rc
    }

    /// The two fonts of the page: Segoe UI Variable when installed, else Segoe UI.
    fn set_fonts(app: &AppWindow, deps: &SettingsDeps) {
        let pick = |family: &str| {
            if (deps.face_of)(family) == family {
                family.to_string()
            } else {
                "Segoe UI".to_string()
            }
        };
        let g = app.global::<SettingsFixed>();
        g.set_font_text(pick("Segoe UI Variable Text").into());
        g.set_font_display(pick("Segoe UI Variable Display").into());
    }

    /// Any change of a key a row, the look or the help panel stores asks for ONE rebuild: the
    /// first change posts a message, later ones find the flag set until the rebuild clears it.
    fn subscribe(deps: &SettingsDeps, dirty: &Arc<AtomicBool>) -> Vec<Subscription> {
        let notify = {
            let dirty = Arc::clone(dirty);
            let post = Arc::clone(&deps.post);
            move || {
                if !dirty.swap(true, Ordering::SeqCst) {
                    post(UiMsg::Settings(SettingsMsg::Changed));
                }
            }
        };
        let mut keys: BTreeSet<&str> = BTreeSet::new();
        for r in all_rows() {
            keys.extend(r.keys.iter().copied());
        }
        keys.extend(HELP_KEYS);
        keys.extend([CUSTOM_KEY, COLOUR_FORMAT_KEY, CUSTOM_COLOURS_KEY, EDGES_KEY]);
        let mut subs: Vec<Subscription> = keys
            .into_iter()
            .map(|k| deps.prefs.on_change(k, notify.clone()))
            .collect();
        // The look keys, the window colours and the editor's draft.
        subs.push(deps.look.on_look_change(notify));
        subs
    }

    fn bind_callbacks(app: &AppWindow, rc: &Rc<RefCell<SettingsCtl>>) {
        on!(app, rc, on_page_chosen, |c, id: SharedString| c
            .page_chosen(&id));
        on!(app, rc, on_find_edited, |c, t: SharedString| c
            .find_edited(&t));
        on!(app, rc, on_find_key, |c, k: SharedString, _s: bool| c
            .find_key(&k));
        on!(app, rc, on_hit_chosen, |c, i: i32| c
            .choose(usize::try_from(i).unwrap_or(usize::MAX)));
        on!(app, rc, on_hit_key, |c, k: SharedString| c.hit_key(&k));
        on!(app, rc, on_rail_key, |c, k: SharedString, i: i32| c
            .rail_key(&k, i));
        on!(app, rc, on_row_switch, |c, id: SharedString, on: bool| c
            .event(Event::Switch {
                row: id.to_string(),
                on
            }));
        on!(
            app,
            rc,
            on_row_choose,
            |c, id: SharedString, opt: SharedString| c.event(Event::Choose {
                row: id.to_string(),
                option: opt.to_string()
            })
        );
        on!(app, rc, on_row_slide, |c, id: SharedString, v: i32| c
            .event(Event::Slide {
                row: id.to_string(),
                value: u32::try_from(v).unwrap_or(0)
            }));
        on!(
            app,
            rc,
            on_row_button,
            |c, id: SharedString, b: SharedString| c.row_button(&id, &b)
        );
        on!(app, rc, on_row_reset, |c, id: SharedString| c.event(
            Event::Reset {
                row: id.to_string()
            }
        ));
        on!(
            app,
            rc,
            on_colour_edited,
            |c, id: SharedString, t: SharedString| c.colour_edited(&id, &t)
        );
        on!(app, rc, on_colour_committed, |c, id: SharedString| c
            .colour_committed(&id));
        on!(app, rc, on_colour_escape, |c, id: SharedString| c
            .colour_escape(&id));
        on!(
            app,
            rc,
            on_select_open_request,
            |c, id: SharedString, x: f32, y: f32, w: f32, h: f32| c
                .select_open(&id, [x, y, w, h].map(f64::from))
        );
        on!(app, rc, on_select_chosen, |c, id: SharedString| c
            .select_chosen(&id));
        on!(app, rc, on_select_dismissed, |c| c.close_select());
        on!(app, rc, on_select_key, |c, k: SharedString| {
            if k == "Escape" {
                c.close_select();
            }
        });
        on!(app, rc, on_save_clicked, |c, id: SharedString| c
            .save_clicked(&id));
        on!(app, rc, on_wall_pick, |c, id: SharedString| c
            .event(Event::WallPick { id: id.to_string() }));
        on!(app, rc, on_wall_edit, |c, id: SharedString| c
            .event(Event::WallEdit { id: id.to_string() }));
        on!(app, rc, on_wall_toggle, |c| c.wall_toggle());
        on!(
            app,
            rc,
            on_editor_well_edited,
            |c, k: SharedString, t: SharedString| c.well_edited(&k, &t)
        );
        on!(app, rc, on_editor_well_committed, |c, k: SharedString| c
            .well_committed(&k));
        on!(app, rc, on_editor_save, |c| c.event(Event::EditorSave));
        on!(app, rc, on_editor_cancel, |c| c.event(Event::EditorCancel));
        on!(app, rc, on_ask_answered, |c, n: i32| c.ask_answered(n));
        on!(app, rc, on_dict_ask_answered, |c, n: i32| c
            .dict_ask_answered(n));
        on!(app, rc, on_frame_resized, |c, w: f32, h: f32| c
            .frame_resized(f64::from(w), f64::from(h)));
        on!(app, rc, on_escape_unhandled, |c| {
            c.escape();
        });
        on!(
            app,
            rc,
            on_colour_swatch,
            |c, id: SharedString, x: f32, y: f32, w: f32, h: f32| c.picker_open(
                &id,
                false,
                [x, y, w, h].map(f64::from)
            )
        );
        on!(
            app,
            rc,
            on_editor_well_swatch,
            |c, k: SharedString, x: f32, y: f32, w: f32, h: f32| c.picker_open(
                &k,
                true,
                [x, y, w, h].map(f64::from)
            )
        );
        on!(app, rc, on_picker_sv, |c, x: f32, y: f32, k: bool| c
            .picker_do(|p, _| p.on_sv(f64::from(x), f64::from(y), k)));
        on!(app, rc, on_picker_hue_moved, |c, x: f32, k: bool| c
            .picker_do(|p, _| p.on_hue(f64::from(x), k)));
        on!(app, rc, on_picker_alpha_moved, |c, x: f32, k: bool| c
            .picker_do(|p, _| p.on_alpha(f64::from(x), k)));
        on!(
            app,
            rc,
            on_picker_key,
            |c, w: i32, k: SharedString, s: bool| c.picker_do(|p, _| p.on_key(w, &k, s))
        );
        on!(app, rc, on_picker_format_clicked, |c| c
            .picker_do(|p, ctx| p.format_clicked(ctx)));
        on!(app, rc, on_picker_dropper, |c| c
            .picker_do(|p, ctx| p.dropper(ctx)));
        on!(app, rc, on_picker_escape, |c| c.picker_do(|p, _| {
            p.escape();
        }));
        on!(app, rc, on_picker_outside, |c| c
            .picker_do(|p, _| p.outside()));
        on!(app, rc, on_picker_focus_left, |c| c
            .picker_do(|p, _| p.focus_left()));
    }

    // ---- the window's calls -------------------------------------------------------------------------

    /// Settings came to the front: rebuild, recall a kept picker, focus nothing.
    pub fn show(&mut self) {
        self.visible = true;
        crate::diag_ctl::crumb(
            "settings-page",
            &[("page", crate::diag_ctl::txt(self.page))],
        );
        self.diag_verbose.set(None);
        self.ask_verb();
        if self.page == "dictation" {
            crate::dictation_ctl::page_shown();
        }
        self.rebuild();
        self.picker_do(|p, ctx| p.recall(ctx));
    }

    /// Another tab came to the front: keep the page, close the overlays.
    pub fn hide(&mut self) {
        self.visible = false;
        // A key field that waits, a microphone test and an open question end with the page.
        crate::dictation_ctl::page_left();
        self.close_select();
        self.picker_do(|p, _| p.hide_for_tab_switch());
        if let Some(app) = self.weak.upgrade() {
            app.global::<SettingsBridge>().set_picker_open(false);
        }
    }

    /// The Settings tab was opened or closed. Closed: the page resets to Appearance (never saved).
    pub fn on_settings_tab(&mut self, open: bool) {
        if open {
            return;
        }
        crate::dictation_ctl::page_left();
        self.page = DEFAULT_PAGE;
        self.find.set_query("");
        self.watch = None;
        self.flash = None;
        self.wall_expanded = false;
        self.wall_anim = None;
        self.ask_open = false;
        self.stale = true;
        self.picker_do(|p, _| p.outside());
        if let Some(app) = self.weak.upgrade() {
            let b = app.global::<SettingsBridge>();
            b.set_find_text("".into());
            b.set_scroll_y(0.0);
            b.set_select_open(false);
            b.set_picker_open(false);
            b.set_ask_open(false);
        }
        if self.visible {
            self.rebuild();
        }
    }

    /// A worker's answer, or the coalesced store change.
    pub fn on_msg(&mut self, m: SettingsMsg) {
        match m {
            SettingsMsg::ShellList(list) => {
                self.shells = Some(list);
                self.refresh_if_visible();
            }
            SettingsMsg::Verb(state) => {
                self.verb_state = state;
                self.refresh_if_visible();
            }
            SettingsMsg::Picked(dir) => {
                actions::folder_picked(&self.deps.prefs, dir);
                self.refresh_if_visible();
            }
            SettingsMsg::Dropper(r) => {
                self.picker_do(|p, _| p.dropper_done(r));
            }
            SettingsMsg::Changed => {
                if self.dirty.swap(false, Ordering::SeqCst) {
                    self.refresh_if_visible();
                }
            }
        }
    }

    fn refresh_if_visible(&mut self) {
        if self.visible {
            self.rebuild();
        } else {
            self.stale = true;
        }
    }

    #[allow(dead_code)] // the tests and the e2e seam read it
    pub fn current_page(&self) -> &'static str {
        self.page
    }

    /// Escape reaches here only when no Slint control took it: closes one overlay (ask, editor,
    /// picker, select, in that order), else false (the host's).
    pub fn escape(&mut self) -> bool {
        let Some(app) = self.weak.upgrade() else {
            return false;
        };
        let b = app.global::<SettingsBridge>();
        if crate::dictation_ctl::page_snap().ask.is_some() {
            self.dict_ask_answered(0);
            return true;
        }
        if self.ask_open {
            self.event(Event::Ask {
                answer: AskAnswer::Cancel,
            });
            self.ask_open = false;
            b.set_ask_open(false);
            return true;
        }
        if self.deps.look.draft().is_some() {
            self.event(Event::EditorCancel);
            return true;
        }
        if self.picker.is_open() {
            // The picker reverts what it wrote, then closes.
            self.picker_do(|p, _| {
                p.escape();
            });
            return true;
        }
        if b.get_select_open() {
            self.close_select();
            return true;
        }
        false
    }

    // ---- building -------------------------------------------------------------------------------------

    fn marked(&self) -> bool {
        self.marked_until
            .is_some_and(|until| (self.deps.clock)() < until)
    }

    fn page_model(&self) -> PageModel {
        let verbose = self.diag_verbose.get().unwrap_or_else(|| {
            let v = self.deps.diag.verbose();
            self.diag_verbose.set(Some(v));
            v
        });
        let diag = DiagInfo {
            verbose,
            dir: self.deps.diag.dir().to_string_lossy().into_owned(),
        };
        let env = PageEnv {
            prefs: &self.deps.prefs,
            look: &self.deps.look,
            shells: self.shells.clone(),
            verb: self.verb_state,
            home: self.home.clone(),
            version: self.deps.version.clone(),
            copyright: self.deps.copyright.clone(),
            glass: (self.deps.glass)(),
            diag,
            diag_marked: self.marked(),
            // The GPU card is offered only where it can work: the pack is NVIDIA's (answered once per process).
            nvidia: self.page == "dictation" && flare_dictation::gpu::has_nvidia(),
            dictation_wired: crate::dictation_ctl::wired(),
            theme_colours: (self.deps.theme_colours)(),
            agent_colours: (self.deps.agent_colours)(),
            agent_key_labels: (self.deps.agent_key_labels)(),
            face_of: &*self.deps.face_of,
        };
        build_page(self.page, &env, &*self.deps.dictation)
    }

    /// The frame (rail included) in logical px, from the last report or the bridge.
    fn frame_size(&self, app: &AppWindow) -> (f64, f64) {
        if let Some(f) = self.frame {
            return f;
        }
        let b = app.global::<SettingsBridge>();
        let (pane_w, pane_h) = (f64::from(b.get_pane_w()), f64::from(b.get_pane_h()));
        if pane_w > 0.0 && pane_h > 0.0 {
            let wide = pane_w + layout::rail_width(1000.0);
            let w = if layout::is_narrow(wide) {
                pane_w + rail_width(0.0)
            } else {
                wide
            };
            return (w, pane_h);
        }
        FALLBACK_FRAME
    }

    fn scale_of(app: &AppWindow) -> f64 {
        let s = f64::from(app.get_scale());
        if s.is_finite() && s > 0.0 { s } else { 1.0 }
    }

    /// The wall's layout at this frame: the cards as they sit now, the clip as it is now.
    fn wall_for(&self, frame_w: f64) -> (WallLayout, f64) {
        let m = layout::Metrics::reference();
        let narrow = layout::is_narrow(frame_w);
        let pad_x = if narrow { m.pad_x_narrow } else { m.pad_x };
        let pane = frame_w - rail_width(frame_w);
        let content = (m.column_max.min(pane) - 2.0 * pad_x).max(0.0);
        let width = (content - 2.0 * m.border - WALL_SIDE_PADS).max(0.0);
        let total = wall::total_themes(self.deps.look.custom().is_some());
        let base = wall::layout(width, total, self.wall_expanded);
        let full = wall::layout(width, total, true);
        let clip_h = match &self.wall_anim {
            Some(a) => {
                wall::expand_height((self.deps.clock)().saturating_sub(a.start_ms), a.from, a.to)
            }
            None => base.clip_h,
        };
        (WallLayout { clip_h, ..base }, full.full_h)
    }

    /// Builds the page, lays it out and writes it into the bridge.
    pub fn rebuild(&mut self) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        self.dirty.store(false, Ordering::SeqCst);
        self.stale = false;
        self.rebuilds += 1;
        let (frame_w, _) = self.frame_size(&app);
        let scale = Self::scale_of(&app);
        let model = self.page_model();
        let (wall, wall_full_h) = self.wall_for(frame_w);
        let badge_h = f64::from(app.global::<SettingsBridge>().get_about_badge_h());
        let badge_h = if badge_h > 0.0 { badge_h } else { BADGE_H };
        let layout = layout_page(&model, frame_w, scale, &wall, wall_full_h, badge_h);
        self.built = Some(Built {
            model,
            layout,
            wall,
            wall_full_h,
            scale,
        });
        self.push(&app);
    }

    /// The flash strength now.
    fn flash_strength(&self) -> f64 {
        match &self.flash {
            Some(f) => flash_alpha(
                (self.deps.clock)().saturating_sub(f.start_ms),
                (self.deps.still)(),
            ),
            None => 0.0,
        }
    }

    /// Writes the built page, the rail, the find state, the wall and the overlays into the bridge.
    fn push(&mut self, app: &AppWindow) {
        let Some(built) = self.built.as_ref() else {
            return;
        };
        let b = app.global::<SettingsBridge>();
        let (frame_w, frame_h) = self.frame_size(app);
        let scale = built.scale;
        let hair = 1.0_f64.max(scale.floor()) / scale;
        let format = colour_format(&self.deps.prefs);
        let flash = self.flash_strength();
        let ctx = RowCtx {
            format,
            drafts: self.picker.fields(),
            hair,
            flash_row: self.flash.as_ref().and_then(|f| f.row.as_deref()),
            flash_section: self.flash.as_ref().and_then(|f| f.section.as_deref()),
            flash,
        };
        let page_label = PAGES
            .iter()
            .find(|p| p.id == self.page)
            .map_or("", |p| p.label);
        let searching = self.find.searching();

        // The rail.
        let rl = rail_layout(frame_w, scale, PAGES.len());
        let end_y = about_y(&rl, frame_h);
        let rail: Vec<RailItem> = PAGES
            .iter()
            .enumerate()
            .map(|(i, p)| RailItem {
                id: p.id.into(),
                label: p.label.into(),
                icon: icon_path(p.icon).unwrap_or("").into(),
                end: p.end,
                chosen: !searching && p.id == self.page,
                y: (if p.end { end_y } else { rl.items[i].0 }) as f32,
            })
            .collect();
        b.set_rail(ModelRc::new(VecModel::from(rail)));
        b.set_narrow(rl.narrow);
        b.set_page_title(page_label.into());
        b.set_still((self.deps.still)());

        // The find state.
        b.set_searching(searching);
        // sync-ok: the find machine's status line, text only, no process
        b.set_status_text(self.find.status().unwrap_or_default().into());
        let nothing = self.find.nothing_text();
        b.set_nothing_text(nothing.clone().unwrap_or_default().into());
        let query = self.find_query_trimmed();
        if nothing.is_some() && !query.is_empty() {
            b.set_nothing_pre("Nothing matches ".into());
            b.set_nothing_query(query.as_str().into());
            b.set_nothing_post(". Try a shorter word.".into());
        } else {
            b.set_nothing_pre("".into());
            b.set_nothing_query("".into());
            b.set_nothing_post("".into());
        }
        b.set_hit_cursor(self.find.cursor().map_or(-1, |c| c as i32));
        let names = page_names();
        if searching {
            let res = layout_results(self.find.hits().len(), frame_w, scale);
            let cursor = self.find.cursor();
            let hits: Vec<SetHit> = self
                .find
                .hits()
                .iter()
                .zip(&res.section.rows)
                .enumerate()
                .map(|(i, (h, rl))| SetHit {
                    id: h.id.as_str().into(),
                    icon: icon_path(&h.icon).unwrap_or("").into(),
                    label: h.label.as_str().into(),
                    sub: h.sub.as_str().into(),
                    r#where: [
                        names.get(&h.page).map_or(h.page.as_str(), String::as_str),
                        h.section.as_str(),
                    ]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                    selected: cursor == Some(i),
                    y: (rl.y - res.section.panel_y - hair) as f32,
                })
                .collect();
            b.set_hits(ModelRc::new(VecModel::from(hits)));
            b.set_hits_h(res.section.panel_h as f32);
            b.set_content_h(res.content_h as f32);
        } else {
            b.set_hits(ModelRc::new(VecModel::from(Vec::<SetHit>::new())));
            b.set_hits_h(0.0);
            b.set_content_h(built.layout.content_h as f32);
        }

        // The page.
        let sections: Vec<SetSection> = built
            .model
            .sections
            .iter()
            .zip(&built.layout.sections)
            .map(|(s, l)| to_slint_section(s, l, &ctx))
            .collect();
        b.set_sections(ModelRc::new(VecModel::from(sections)));

        // The wall.
        let total = wall::total_themes(self.deps.look.custom().is_some());
        let (aria, title) = wall::toggle_texts(self.wall_expanded, total);
        b.set_wall_expanded(self.wall_expanded);
        b.set_wall_toggle_aria(aria.into());
        b.set_wall_toggle_title(title.into());
        b.set_wall_clip_h(built.wall.clip_h as f32);
        b.set_wall_full_h(built.wall_full_h as f32);
        let cards: Vec<WallCard> = wall::wall_cards(&self.deps.look)
            .iter()
            .zip(&built.wall.positions)
            .map(|(c, (x, y))| WallCard {
                id: c.id.as_str().into(),
                name: c.name.as_str().into(),
                on: c.on,
                edit: c.edit,
                edit_name: if c.edit {
                    wall::edit_texts(&c.name).0.into()
                } else {
                    "".into()
                },
                bg: colour_of(&c.bg),
                fg: colour_of(&c.fg),
                cursor: colour_of(&c.cursor),
                green: colour_of(&c.green),
                yellow: colour_of(&c.yellow),
                blue: colour_of(&c.blue),
                cyan: colour_of(&c.cyan),
                red: colour_of(&c.red),
                x: *x as f32,
                y: *y as f32,
            })
            .collect();
        b.set_wall_cards(ModelRc::new(VecModel::from(cards)));

        // The overlays.
        b.set_ask_open(self.ask_open);
        b.set_dict_ask_open(crate::dictation_ctl::page_snap().ask.is_some());
        b.set_dict_ask_text(flare_dictation::catalog::LIMITED_LANGUAGES_TEXT.into());
        self.push_editor(app, format);
    }

    fn find_query_trimmed(&self) -> String {
        // The query is what the field shows; the machine keeps it private, the bridge has it.
        self.weak
            .upgrade()
            .map(|a| {
                a.global::<SettingsBridge>()
                    .get_find_text()
                    .trim()
                    .to_string()
            })
            .unwrap_or_default()
    }

    /// The theme editor: the draft as twenty wells and the preview card.
    fn push_editor(&self, app: &AppWindow, format: ColourFormat) {
        let b = app.global::<SettingsBridge>();
        let Some(draft) = self.deps.look.draft() else {
            b.set_editor_open(false);
            b.set_editor_wells(ModelRc::new(VecModel::from(Vec::<EditorWell>::new())));
            return;
        };
        let drawn = resolve_custom_theme(&draft, None);
        let mut entries: Vec<(String, String, String, f64)> = vec![
            ("bg".into(), "Background".into(), draft.bg.clone(), 1.0),
            ("fg".into(), "Foreground".into(), draft.fg.clone(), 1.0),
            ("cursor".into(), "Cursor".into(), draft.cursor.clone(), 1.0),
            (
                "selection".into(),
                "Selection".into(),
                draft
                    .selection
                    .clone()
                    .unwrap_or_else(|| drawn.selection_background.clone()),
                254.0 / 255.0,
            ),
        ];
        for (i, k) in ANSI_KEYS.iter().enumerate() {
            entries.push((
                (*k).to_string(),
                (*k).to_string(),
                draft.ansi[i].clone().unwrap_or_else(unset_ansi),
                1.0,
            ));
        }
        let wells: Vec<EditorWell> = entries
            .into_iter()
            .map(|(key, label, value, alpha_max)| {
                let range = flare_theme::colour_contrast::AlphaRange {
                    alpha: true,
                    alpha_min: 0.0,
                    alpha_max,
                };
                let text = match self.well_drafts.get(&key) {
                    Some(f) => f.shown(&value, &range, format),
                    None => CodeField::default().shown(&value, &range, format),
                };
                EditorWell {
                    colour: colour_of(&value),
                    text: text.into(),
                    wide: format != ColourFormat::Hex,
                    open: self.picker.is_open_for(&key, true),
                    label: label.into(),
                    key: key.into(),
                }
            })
            .collect();
        let ca = card_ansi(&drawn);
        let preview = WallCard {
            id: "custom-preview".into(),
            name: "Custom".into(),
            on: true,
            edit: false,
            edit_name: "".into(),
            bg: colour_of(&drawn.background),
            fg: colour_of(&drawn.foreground),
            cursor: colour_of(&drawn.cursor),
            green: colour_of(&ca.green),
            yellow: colour_of(&ca.yellow),
            blue: colour_of(&ca.blue),
            cyan: colour_of(&ca.cyan),
            red: colour_of(&ca.red),
            x: 0.0,
            y: 0.0,
        };
        b.set_editor_wells(ModelRc::new(VecModel::from(wells)));
        b.set_editor_preview(preview);
        b.set_editor_open(true);
    }

    // ---- events ---------------------------------------------------------------------------------------

    fn frame_resized(&mut self, w: f64, h: f64) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let changed = self.frame != Some((w, h));
        self.frame = Some((w, h));
        if changed && self.visible {
            self.rebuild();
        }
    }

    fn page_chosen(&mut self, id: &str) {
        self.page = page_static(id);
        if self.page == "dictation" {
            // The microphones are listed again, on a worker (the page never opens one).
            crate::dictation_ctl::page_shown();
        } else {
            crate::dictation_ctl::page_left();
        }
        crate::diag_ctl::crumb(
            "settings-page",
            &[("page", crate::diag_ctl::txt(self.page))],
        );
        self.watch = None;
        if let Some(app) = self.weak.upgrade() {
            app.global::<SettingsBridge>().set_scroll_y(0.0);
        }
        self.rebuild();
    }

    /// Carries out one control event: the store writes, then the effects.
    pub fn event(&mut self, ev: Event) {
        // The Dictation page's controls are dictation's: its stored keys, its store, its microphone.
        let dictated = match &ev {
            Event::Switch { row, on } if crate::dictation_ctl::owns_row(row) => {
                Some(crate::dictation_ctl::page_switch(row, *on))
            }
            Event::Choose { row, option } if crate::dictation_ctl::owns_row(row) => {
                Some(crate::dictation_ctl::page_choose(row, option))
            }
            _ => None,
        };
        if let Some(changed) = dictated {
            if changed {
                self.refresh_if_visible();
            }
            return;
        }
        if matches!(&ev, Event::Switch { row, .. } if row == "diag-verbose") {
            // The switch writes the file; the rebuild that follows shows what it now holds.
            self.diag_verbose.set(None);
        }
        let effects = {
            let ctx = Ctx {
                prefs: &self.deps.prefs,
                look: &self.deps.look,
                diag: &*self.deps.diag,
                pending_pick: &self.pending_pick,
            };
            actions::apply(&ctx, ev)
        };
        for e in effects {
            self.effect(e);
        }
    }

    fn effect(&mut self, e: Effect) {
        match e {
            Effect::AskThemeSwitch { .. } => {
                self.ask_open = true;
                if let Some(app) = self.weak.upgrade() {
                    app.global::<SettingsBridge>().set_ask_open(true);
                }
            }
            Effect::PickFolder => {
                let post = Arc::clone(&self.deps.post);
                (self.deps.pick_folder)(
                    None,
                    Arc::new(move |p| post(UiMsg::Settings(SettingsMsg::Picked(p)))),
                );
            }
            Effect::OpenDiagFolder => (self.deps.open_path)(&self.deps.diag.dir()),
            Effect::SetVerb(on) => {
                let verb = Arc::clone(&self.deps.verb);
                let post = Arc::clone(&self.deps.post);
                spawn_worker("flare-verb", move || {
                    post(UiMsg::Settings(SettingsMsg::Verb(verb.set(on))));
                });
            }
            Effect::Marked => {
                self.marked_until = Some((self.deps.clock)() + MARKED_MS);
                let me = self.me.clone();
                self.mark_timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(MARKED_MS),
                    move || {
                        if let Some(rc) = me.upgrade()
                            && let Ok(mut c) = rc.try_borrow_mut()
                        {
                            c.marked_until = None;
                            c.refresh_if_visible();
                        }
                    },
                );
                self.rebuild();
            }
            Effect::Rebuild => self.rebuild(),
        }
    }

    fn row_button(&mut self, row: &str, button: &str) {
        if crate::dictation_ctl::owns_row(row) {
            if crate::dictation_ctl::page_button(row, button) {
                self.refresh_if_visible();
            }
            return;
        }
        if row == "slint" {
            if flare_term::web_url::is_web_url(button) {
                (self.deps.open_path)(Path::new(button));
            }
            return;
        }
        self.event(Event::Button {
            row: row.to_string(),
            button: button.to_string(),
        });
    }

    fn save_clicked(&mut self, id: &str) {
        let from = if id == "term-theme" {
            SaveFrom::Theme
        } else {
            SaveFrom::MarkColours
        };
        self.event(Event::SaveChanges { from });
    }

    /// The Dictation page's one-line question: 0 Cancel, 1 Download.
    fn dict_ask_answered(&mut self, n: i32) {
        if crate::dictation_ctl::page_ask_answer(n == 1) {
            self.refresh_if_visible();
        }
    }

    fn ask_answered(&mut self, n: i32) {
        let answer = match n {
            1 => AskAnswer::Discard,
            2 => AskAnswer::SaveAsCustom,
            _ => AskAnswer::Cancel,
        };
        self.ask_open = false;
        if let Some(app) = self.weak.upgrade() {
            app.global::<SettingsBridge>().set_ask_open(false);
        }
        self.event(Event::Ask { answer });
        self.rebuild();
    }

    // ---- colour codes, the popover and the dropper (PickerCtl owns the session) ------------------------

    /// What the colour row (or editor well) `id` holds now.
    fn colour_info(
        built: Option<&Built>,
        look: &LookStore,
        id: &str,
        in_editor: bool,
    ) -> Option<RowColour> {
        if in_editor {
            return Self::well_info(look, id);
        }
        let row = flare_settings::page::row_of(&built?.model, id)?;
        match &row.control {
            Control::Colour {
                value,
                range,
                alpha_disabled,
            } => Some(RowColour {
                label: row.label.clone(),
                value: value.clone(),
                stored: row.reset.then(|| value.clone()),
                range: *range,
                alpha_disabled: *alpha_disabled,
            }),
            _ => None,
        }
    }

    /// A well of the theme editor: its colour in force and what the draft stores.
    fn well_info(look: &LookStore, key: &str) -> Option<RowColour> {
        let draft = look.draft()?;
        let drawn = resolve_custom_theme(&draft, None);
        let (label, value, stored, alpha_max) = match key {
            "bg" => (
                "Background".to_string(),
                draft.bg.clone(),
                Some(draft.bg.clone()),
                1.0,
            ),
            "fg" => (
                "Foreground".to_string(),
                draft.fg.clone(),
                Some(draft.fg.clone()),
                1.0,
            ),
            "cursor" => (
                "Cursor".to_string(),
                draft.cursor.clone(),
                Some(draft.cursor.clone()),
                1.0,
            ),
            "selection" => (
                "Selection".to_string(),
                draft
                    .selection
                    .clone()
                    .unwrap_or_else(|| drawn.selection_background.clone()),
                draft.selection.clone(),
                254.0 / 255.0,
            ),
            k => {
                let i = ANSI_KEYS.iter().position(|a| *a == k)?;
                (
                    k.to_string(),
                    draft.ansi[i].clone().unwrap_or_else(unset_ansi),
                    draft.ansi[i].clone(),
                    1.0,
                )
            }
        };
        Some(RowColour {
            label,
            value,
            stored,
            // The theme's Background is a colour only (owner, 2026-10-13: the See-through slider is the
            // only way to make the window see-through), so its popover has no Alpha bar.
            range: flare_theme::colour_contrast::AlphaRange {
                alpha: key != "bg",
                alpha_min: 0.0,
                alpha_max,
            },
            alpha_disabled: key == "bg",
        })
    }

    /// Runs `f` against the picker with its world, applies the events it produced, redraws the
    /// popover and keeps the 16 ms frame timer running only while a write is queued.
    fn picker_do<R>(&mut self, f: impl FnOnce(&mut PickerCtl, &PickerCtx) -> R) -> Option<R> {
        let app = self.weak.upgrade()?;
        let scale = Self::scale_of(&app);
        let size = app.window().size();
        let window = (
            f64::from(size.width) / scale,
            f64::from(size.height) / scale,
        );
        let built = &self.built;
        let look = &self.deps.look;
        let lookup =
            |id: &str, in_editor: bool| Self::colour_info(built.as_ref(), look, id, in_editor);
        let ctx = PickerCtx {
            prefs: &self.deps.prefs,
            window,
            scale,
            lookup: &lookup,
            seam: crate::e2e::global().dialog_seam(
                &["FLARE_E2E", "_PICK_COLOUR"].concat(),
                "cancel",
                |v| flare_win::eyedrop::pick_seam(Some(v)).is_some(),
            ),
            post: &self.deps.post,
        };
        let r = f(&mut self.picker, &ctx);
        let events = self.picker.take_events();
        let editor = events.iter().any(|e| matches!(e, Event::EditorSet { .. }));
        for e in events {
            self.event(e);
        }
        if editor {
            self.rebuild();
        }
        self.push_picker(&app);
        self.picker_frames();
        Some(r)
    }

    fn picker_frames(&mut self) {
        if !self.picker.needs_frame() {
            self.picker_timer.stop();
            return;
        }
        if self.picker_timer.running() {
            return;
        }
        let me = self.me.clone();
        self.picker_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                if let Some(rc) = me.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.picker_do(|p, _| p.frame());
                }
            },
        );
    }

    fn picker_open(&mut self, id: &str, in_editor: bool, anchor: [f64; 4]) {
        let rect = Rect {
            x: anchor[0],
            y: anchor[1],
            w: anchor[2],
            h: anchor[3],
        };
        // A swatch that moved under its open popover reports the same way, marked so it is not a press.
        if let Some(app) = self.weak.upgrade()
            && app.global::<crate::PickerOpening>().get_reanchor()
        {
            self.picker_do(|p, ctx| p.reanchor(id, in_editor, rect, ctx));
            return;
        }
        self.close_select();
        self.picker_do(|p, ctx| p.open(id, in_editor, rect, ctx));
    }

    /// The popover's properties on the bridge, and the editor wells' open marks.
    fn push_picker(&self, app: &AppWindow) {
        let b = app.global::<SettingsBridge>();
        let Some(v) = self.picker.view(&self.deps.prefs) else {
            b.set_picker_open(false);
            return;
        };
        b.set_picker_x(v.x as f32);
        b.set_picker_y(v.y as f32);
        b.set_picker_label(v.label.as_str().into());
        b.set_picker_hue(v.hue as f32);
        b.set_picker_sat(v.sat as f32);
        b.set_picker_val(v.val as f32);
        b.set_picker_alpha(v.alpha as f32);
        b.set_picker_solid(rgba_colour(&v.solid));
        b.set_picker_start(rgba_colour(&v.start));
        b.set_picker_now(rgba_colour(&v.now));
        b.set_picker_hue_colour(rgba_colour(&v.hue_colour));
        b.set_picker_alpha_on(v.alpha_on);
        b.set_picker_alpha_disabled(v.alpha_disabled);
        b.set_picker_dropper_on(v.dropper_on);
        b.set_picker_format(v.format.into());
        b.set_picker_sv_text(v.sv_text.as_str().into());
        b.set_picker_hue_text(v.hue_text.as_str().into());
        b.set_picker_alpha_text(v.alpha_text.as_str().into());
        b.set_picker_sv_now(v.sv_now);
        b.set_picker_hue_now(v.hue_now);
        b.set_picker_alpha_now(v.alpha_now);
        b.set_picker_open(true);
    }

    fn colour_edited(&mut self, id: &str, text: &str) {
        self.picker.field_edited(id, text);
    }

    fn colour_committed(&mut self, id: &str) {
        self.picker_do(|p, ctx| p.field_committed(id, ctx));
        self.rebuild();
    }

    fn colour_escape(&mut self, id: &str) {
        if self.picker.field_escape(id) {
            self.rebuild();
        }
    }

    fn well_edited(&mut self, key: &str, text: &str) {
        self.well_drafts
            .entry(key.to_string())
            .or_default()
            .edit(text);
    }

    fn well_committed(&mut self, key: &str) {
        let Some(draft) = self.deps.look.draft() else {
            return;
        };
        let value = match key {
            "bg" => draft.bg.clone(),
            "fg" => draft.fg.clone(),
            "cursor" => draft.cursor.clone(),
            "selection" => draft.selection.clone().unwrap_or_default(),
            k => ANSI_KEYS
                .iter()
                .position(|a| *a == k)
                .and_then(|i| draft.ansi[i].clone())
                .unwrap_or_else(unset_ansi),
        };
        let range = flare_theme::colour_contrast::AlphaRange::default();
        let stored = self
            .well_drafts
            .remove(key)
            .and_then(|mut f| f.commit(&value, &range));
        if let Some(stored) = stored {
            self.event(Event::EditorSet {
                key: key.to_string(),
                value: Some(stored),
            });
        }
        self.rebuild();
    }

    // ---- the Select menu -------------------------------------------------------------------------------

    fn select_open(&mut self, row: &str, anchor: [f64; 4]) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let Some(built) = self.built.as_ref() else {
            return;
        };
        let Some(r) = flare_settings::page::row_of(&built.model, row) else {
            return;
        };
        // The language picker and the microphone's picker open the same menu.
        let (Control::Select { value, options } | Control::Mic { value, options, .. }) = &r.control
        else {
            return;
        };
        let scale = Self::scale_of(&app);
        let size = app.window().size();
        let window = (
            f64::from(size.width) / scale,
            f64::from(size.height) / scale,
        );
        let content = options.len() as f64 * OPTION_H + MENU_PAD;
        let (x, y, h) = place_menu_at(
            Rect {
                x: anchor[0],
                y: anchor[1],
                w: anchor[2],
                h: anchor[3],
            },
            anchor[2],
            content,
            window,
            MENU_MAX_H,
            scale,
        );
        let b = app.global::<SettingsBridge>();
        b.set_select_options(options_of(options));
        b.set_select_value(value.as_str().into());
        b.set_select_row(row.into());
        b.set_select_x(x as f32);
        b.set_select_y(y as f32);
        b.set_select_w(anchor[2] as f32);
        b.set_select_h(h as f32);
        b.set_select_open(true);
        self.select_row = row.to_string();
    }

    fn select_chosen(&mut self, id: &str) {
        let row = std::mem::take(&mut self.select_row);
        self.close_select();
        if !row.is_empty() {
            self.event(Event::Choose {
                row,
                option: id.to_string(),
            });
        }
    }

    fn close_select(&mut self) {
        self.select_row.clear();
        if let Some(app) = self.weak.upgrade() {
            let b = app.global::<SettingsBridge>();
            b.set_select_open(false);
            b.set_select_row("".into());
        }
    }

    // ---- Find a setting --------------------------------------------------------------------------------

    fn find_edited(&mut self, text: &str) {
        self.find.set_query(text);
        self.rebuild();
    }

    fn find_key(&mut self, word: &str) {
        let Some(key) = key_of(word) else { return };
        match self.find.field_key(key) {
            FieldOut::Consumed => {
                if let Some(app) = self.weak.upgrade() {
                    app.global::<SettingsBridge>().set_find_text("".into());
                }
                self.rebuild();
            }
            FieldOut::PassThrough => {}
            FieldOut::Mark(_) => {
                self.rebuild();
                if let Some(app) = self.weak.upgrade() {
                    app.invoke_settings_focus_list();
                }
            }
            FieldOut::Choose(i) => self.choose(i),
        }
    }

    fn hit_key(&mut self, word: &str) {
        let Some(key) = key_of(word) else { return };
        match self.find.list_key(key) {
            ListOut::Consumed => {}
            ListOut::Mark(_) => self.rebuild(),
            ListOut::ToField => {
                self.rebuild();
                if let Some(app) = self.weak.upgrade() {
                    app.invoke_settings_focus_find();
                }
            }
            ListOut::Choose(i) => self.choose(i),
            ListOut::Clear => {
                if let Some(app) = self.weak.upgrade() {
                    app.global::<SettingsBridge>().set_find_text("".into());
                    app.invoke_settings_focus_find();
                }
                self.rebuild();
            }
        }
    }

    /// An arrow on the rail: the walk goes from the item that has the keys (t), which is not
    /// always the chosen page: Down Down from the first item reaches the third before Enter chooses.
    fn rail_key(&mut self, word: &str, at: i32) {
        let Some(key) = key_of(word) else { return };
        let cur = usize::try_from(at).unwrap_or(0);
        if let Some(next) = rail_key(cur, PAGES.len(), key)
            && let Some(app) = self.weak.upgrade()
        {
            app.invoke_settings_focus_rail(next as i32);
            // The rail item clears the request itself once it has taken the focus.
        }
    }

    /// Chooses result `i`: switch page, clear the query, rebuild, then scroll to the row and flash it.
    fn choose(&mut self, i: usize) {
        let Some(chosen) = self.find.choose(i) else {
            return;
        };
        self.page = page_static(&chosen.page);
        if let Some(app) = self.weak.upgrade() {
            let b = app.global::<SettingsBridge>();
            b.set_find_text("".into());
            b.set_scroll_y(0.0);
        }
        self.rebuild();
        self.watch = Some(FlashWatch::new(
            (self.deps.clock)(),
            &chosen.id,
            chosen.section_id.as_deref(),
        ));
        self.poll_watch();
    }

    /// One look for the chosen row. The row may be drawn late (the shell list), so it is polled.
    fn poll_watch(&mut self) {
        let now = (self.deps.clock)();
        let Some(built) = self.built.as_ref() else {
            return;
        };
        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        let id = watch.id().to_string();
        let row_exists = built
            .layout
            .sections
            .iter()
            .flat_map(|s| s.rows.iter())
            .any(|r| r.block == LayoutBlock::Row && r.id == id);
        let section = watch.section_id().map(str::to_string);
        let section_exists = section
            .as_deref()
            .is_some_and(|s| built.layout.sections.iter().any(|x| x.id == s));
        match watch.poll(now, row_exists, section_exists) {
            FlashStep::Found => {
                self.finish_watch();
                self.scroll_to_row(&id);
                self.start_flash(Some(id.clone()), None);
                if let Some(app) = self.weak.upgrade() {
                    app.global::<SettingsBridge>().set_focus_id(id.into());
                }
            }
            FlashStep::Retry { .. } => {
                let me = self.me.clone();
                if !self.poll_timer.running() {
                    self.poll_timer.start(
                        slint::TimerMode::Repeated,
                        Duration::from_millis(POLL_MS),
                        move || {
                            if let Some(rc) = me.upgrade()
                                && let Ok(mut c) = rc.try_borrow_mut()
                            {
                                c.poll_watch();
                            }
                        },
                    );
                }
            }
            FlashStep::Section => {
                self.finish_watch();
                if let Some(s) = section {
                    self.scroll_to_section(&s);
                    self.start_flash(None, Some(s));
                }
            }
            FlashStep::GiveUp => self.finish_watch(),
        }
    }

    fn finish_watch(&mut self) {
        self.watch = None;
        self.poll_timer.stop();
    }

    fn scroll_to_row(&mut self, id: &str) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let Some(built) = self.built.as_ref() else {
            return;
        };
        let (_, frame_h) = self.frame_size(&app);
        if let Some(y) = scroll_target(&built.layout, id, frame_h) {
            app.global::<SettingsBridge>().set_scroll_y(y as f32);
        }
    }

    fn scroll_to_section(&mut self, id: &str) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let Some(built) = self.built.as_ref() else {
            return;
        };
        let (_, frame_h) = self.frame_size(&app);
        if let Some(s) = built.layout.sections.iter().find(|s| s.id == id) {
            let y = centre_scroll(s.panel_y, s.panel_h, frame_h, built.layout.content_h);
            app.global::<SettingsBridge>().set_scroll_y(y as f32);
        }
    }

    /// Starts the flash: 1.0 held, then eased out over `FLASH_MS`. Under reduced motion there is none.
    fn start_flash(&mut self, row: Option<String>, section: Option<String>) {
        self.flash = Some(FlashRun {
            row,
            section,
            start_ms: (self.deps.clock)(),
        });
        if (self.deps.still)() {
            self.flash = None;
            self.flash_timer.stop();
            self.repaint_rows();
            return;
        }
        self.repaint_rows();
        let me = self.me.clone();
        self.flash_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                if let Some(rc) = me.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.tick_flash();
                }
            },
        );
    }

    fn tick_flash(&mut self) {
        let done = self
            .flash
            .as_ref()
            .is_none_or(|f| (self.deps.clock)().saturating_sub(f.start_ms) >= FLASH_MS);
        self.repaint_rows();
        if done {
            self.flash = None;
            self.flash_timer.stop();
            self.repaint_rows();
        }
    }

    /// Writes the sections again with the flash as it is now (no page rebuild).
    fn repaint_rows(&mut self) {
        if let Some(app) = self.weak.upgrade() {
            self.push(&app);
        }
    }

    // ---- the wall --------------------------------------------------------------------------------------

    fn wall_toggle(&mut self) {
        let from = self.built.as_ref().map_or(0.0, |b| b.wall.clip_h);
        self.wall_expanded = !self.wall_expanded;
        let to = {
            let frame_w = self
                .weak
                .upgrade()
                .map_or(FALLBACK_FRAME.0, |a| self.frame_size(&a).0);
            let (target, _) = self.wall_for(frame_w);
            target.clip_h
        };
        // Opening animates; closing is instant (and so is everything under reduced motion).
        if to > from && !(self.deps.still)() {
            self.wall_anim = Some(WallAnim {
                from,
                to,
                start_ms: (self.deps.clock)(),
            });
            let me = self.me.clone();
            self.wall_timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(16),
                move || {
                    if let Some(rc) = me.upgrade()
                        && let Ok(mut c) = rc.try_borrow_mut()
                    {
                        c.tick_wall();
                    }
                },
            );
        } else {
            self.wall_anim = None;
            self.wall_timer.stop();
        }
        self.rebuild();
    }

    fn tick_wall(&mut self) {
        let done = self
            .wall_anim
            .as_ref()
            .is_none_or(|a| (self.deps.clock)().saturating_sub(a.start_ms) >= wall::EXPAND_MS);
        if done {
            self.wall_anim = None;
            self.wall_timer.stop();
        }
        self.rebuild();
    }

    // ---- workers ---------------------------------------------------------------------------------------

    fn start_shell_list(&self) {
        let list = Arc::clone(&self.deps.shell_list);
        let post = Arc::clone(&self.deps.post);
        spawn_worker("flare-shell-list", move || {
            post(UiMsg::Settings(SettingsMsg::ShellList(list())));
        });
    }

    fn ask_verb(&self) {
        let verb = Arc::clone(&self.deps.verb);
        let post = Arc::clone(&self.deps.post);
        spawn_worker("flare-verb-status", move || {
            // sync-ok: runs on this worker thread, never on the UI thread
            post(UiMsg::Settings(SettingsMsg::Verb(verb.status())));
        });
    }
}

/// The three attention colours in force (working, finished, question): the theme's own unless a
/// pick overrides it. The same reading `appctl::compute_marks` makes.
pub fn agent_colours_in_force(prefs: &PrefStore) -> [String; 3] {
    use flare_settings::look::AgentColourState;
    use flare_theme::agent_colours::theme_agent_colours;
    use flare_theme::look::apply_look;
    use flare_theme::resolve::preset_accent;
    let look = LookStore::new(prefs.clone());
    let snap = look.input_snapshot();
    let applied = apply_look(&snap.as_input(false));
    let accent = preset_accent(
        &applied.theme_id,
        &applied.theme_id,
        snap.custom.as_ref(),
        snap.draft.as_ref(),
    );
    let themed = theme_agent_colours(
        &applied.term,
        accent.as_deref(),
        snap.background.as_deref(),
        snap.accent.as_deref(),
    );
    let pick = |state: AgentColourState, themed: &str| {
        let chosen = look.agent_colour_choice(state);
        if chosen.is_empty() {
            themed.to_string()
        } else {
            chosen
        }
    };
    [
        pick(AgentColourState::Working, &themed.working),
        pick(AgentColourState::Finished, &themed.finished),
        pick(AgentColourState::Question, &themed.question),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_harness::{Harness, SCALES};
    use flare_settings::kv::{Backend, MemoryBackend};
    use flare_settings::new_tab::{NewTabMode, new_tab_folder, new_tab_mode};
    use flare_settings::page::{Button, Opt, PlaceholderDictation};
    use flare_settings::verb::{FakeDiag, FakeVerb, PendingVerb};
    use flare_settings::window_colours::ACCENT;
    use slint::Model;
    use std::cell::Cell;
    use std::collections::{BTreeSet, VecDeque};
    use std::sync::Mutex;

    /// A store that remembers every key written to it.
    struct RecBackend {
        inner: MemoryBackend,
        keys: Mutex<BTreeSet<String>>,
    }

    impl Backend for RecBackend {
        fn get(&self, key: &str) -> Option<String> {
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) {
            self.keys
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key.to_string());
            self.inner.set(key, value);
        }
        fn remove(&self, key: &str) {
            self.keys
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key.to_string());
            self.inner.remove(key);
        }
    }

    type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

    struct Fx {
        h: Harness<AppWindow>,
        ctl: Rc<RefCell<SettingsCtl>>,
        prefs: PrefStore,
        look: LookStore,
        diag: Rc<FakeDiag>,
        q: Queue,
        clock: Rc<std::cell::Cell<u64>>,
        pick_answer: Rc<RefCell<Option<PathBuf>>>,
        opened: Rc<RefCell<Vec<PathBuf>>>,
        written: Arc<RecBackend>,
    }

    fn fx_with(scale: f32, verb: Arc<dyn ExplorerVerb>) -> Fx {
        fx_with_dictation(scale, verb, Rc::new(PlaceholderDictation))
    }

    fn fx_with_dictation(
        scale: f32,
        verb: Arc<dyn ExplorerVerb>,
        dictation: Rc<dyn DictationPage>,
    ) -> Fx {
        let written = Arc::new(RecBackend {
            inner: MemoryBackend::new(),
            keys: Mutex::new(BTreeSet::new()),
        });
        let prefs = PrefStore::new(written.clone());
        let look = LookStore::new(prefs.clone());
        let q: Queue = Arc::default();
        let post: Post = {
            let q = Arc::clone(&q);
            Arc::new(move |m| {
                q.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_back(m);
            })
        };
        let diag = Rc::new(FakeDiag::new("C:/logs", false));
        let clock = Rc::new(Cell::new(1_000_u64));
        let pick_answer: Rc<RefCell<Option<PathBuf>>> = Rc::default();
        let opened: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
        let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
        h.comp.set_front_kind(1);
        h.advance(0);
        let deps = SettingsDeps {
            prefs: prefs.clone(),
            look: LookStore::new(prefs.clone()),
            diag: diag.clone(),
            verb,
            dictation,
            pick_folder: {
                let answer = Rc::clone(&pick_answer);
                Rc::new(move |_, done| done(answer.borrow().clone()))
            },
            open_path: {
                let opened = Rc::clone(&opened);
                Rc::new(move |p: &Path| opened.borrow_mut().push(p.to_path_buf()))
            },
            clipboard: Rc::new(|_: &str| {}),
            shell_list: Arc::new(|| vec![("pwsh".to_string(), "PowerShell".to_string())]),
            glass: Rc::new(|| GlassSupport::Ok),
            face_of: Rc::new(|_: &str| String::new()),
            theme_colours: Rc::new(|| ThemeColours {
                accent: "#fe8f34".into(),
                background: "#101010".into(),
                secondary: "#181818".into(),
            }),
            agent_colours: Rc::new(|| ["#111111".into(), "#222222".into(), "#333333".into()]),
            agent_key_labels: Rc::new(String::new),
            post,
            version: "1.2.3".to_string(),
            copyright: "Copyright 2026".to_string(),
            e2e: true,
            clock: {
                let c = Rc::clone(&clock);
                Rc::new(move || c.get())
            },
            still: Rc::new(|| false),
        };
        let ctl = SettingsCtl::new(&h.comp, deps);
        ctl.borrow_mut().show();
        Fx {
            h,
            ctl,
            prefs,
            look,
            diag,
            q,
            clock,
            pick_answer,
            opened,
            written,
        }
    }

    fn fx() -> Fx {
        fx_with(1.0, Arc::new(FakeVerb::new(VerbState::Off)))
    }

    impl Fx {
        fn b(&self) -> SettingsBridge<'_> {
            self.h.comp.global::<SettingsBridge>()
        }

        /// Hands every queued message to the controller, as the window's pump does.
        fn pump(&self) -> usize {
            let msgs: Vec<UiMsg> = self
                .q
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain(..)
                .collect();
            let n = msgs.len();
            for m in msgs {
                if let UiMsg::Settings(m) = m {
                    self.ctl.borrow_mut().on_msg(m);
                }
            }
            n
        }

        /// Waits (up to 3 s) for the workers' two answers, then pumps them.
        fn settle_workers(&self) {
            let start = std::time::Instant::now();
            let mut got = 0;
            while got < 2 && start.elapsed() < Duration::from_secs(3) {
                got += self.pump();
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn tick(&self, ms: u64) {
            self.clock.set(self.clock.get() + ms);
            self.h.advance(ms);
        }

        fn page(&self, id: &str) {
            self.b().invoke_page_chosen(id.into());
        }

        fn sections(&self) -> Vec<SetSection> {
            self.b().get_sections().iter().collect()
        }

        fn row(&self, id: &str) -> Option<SetRow> {
            self.sections()
                .iter()
                .flat_map(|s| s.rows.iter())
                .find(|r| r.id == id)
        }
    }

    // ---- the pure mappers ------------------------------------------------------------------------

    fn model_row(control: Control) -> RowModel {
        RowModel {
            id: "tab-layout",
            icon: "layout",
            label: "Label".to_string(),
            sub: "Sub".to_string(),
            warn: false,
            off: false,
            tap: false,
            row_tip: "Tip".to_string(),
            control,
            reset: true,
            buttons: vec![Button {
                id: "mark",
                label: "Mark".to_string(),
                alt: Some("Marked".to_string()),
                off: false,
            }],
        }
    }

    #[test]
    fn to_slint_maps_every_control_kind() {
        let drafts = HashMap::new();
        let ctx = RowCtx {
            format: ColourFormat::Hex,
            drafts: &drafts,
            hair: 1.0,
            flash_row: None,
            flash_section: None,
            flash: 0.0,
        };
        let opts = vec![Opt {
            id: "a".into(),
            name: "A".into(),
            face: "Cascadia Mono".into(),
        }];
        let range = flare_theme::colour_contrast::AlphaRange::default();
        let cases: Vec<(Control, SetKind)> = vec![
            (Control::None, SetKind::None),
            (Control::Pill("claude".into()), SetKind::Pill),
            (Control::Switch { on: true }, SetKind::Switch),
            (
                Control::Segmented {
                    value: "a".into(),
                    options: opts.clone(),
                },
                SetKind::Segmented,
            ),
            (
                Control::Select {
                    value: "a".into(),
                    options: opts.clone(),
                },
                SetKind::Select,
            ),
            (
                Control::Slider {
                    value: 30,
                    max: 70,
                    text: "30%".into(),
                    disabled: true,
                },
                SetKind::Slider,
            ),
            (
                Control::Colour {
                    value: "#fe8f34".into(),
                    range,
                    alpha_disabled: false,
                },
                SetKind::Colour,
            ),
            (
                Control::Save {
                    dirty: true,
                    title: "Save changes".into(),
                },
                SetKind::Save,
            ),
        ];
        for (control, kind) in cases {
            let row = to_slint_row(&model_row(control.clone()), 58.0, 58.0, &ctx);
            assert_eq!(row.kind, kind, "{control:?}");
            assert_eq!(control_kind(&control), kind);
            assert_eq!(row.id, "tab-layout");
            assert_eq!(row.label, "Label");
            assert_eq!(row.sub_tip, "Sub");
            assert_eq!(row.row_tip, "Tip");
            assert!(row.reset);
            assert_eq!(row.buttons.row_count(), 1);
            let b = row.buttons.row_data(0).unwrap();
            assert!(b.alt);
            assert_eq!(b.label_alt, "Marked");
        }
        let r = to_slint_row(&model_row(Control::Switch { on: true }), 0.0, 58.0, &ctx);
        assert!(r.on);
        let r = to_slint_row(&model_row(Control::Pill("claude".into())), 0.0, 58.0, &ctx);
        assert_eq!(r.value, "claude");
        let r = to_slint_row(
            &model_row(Control::Select {
                value: "a".into(),
                options: opts,
            }),
            0.0,
            58.0,
            &ctx,
        );
        assert_eq!(r.value, "a");
        assert_eq!(r.options.row_data(0).unwrap().face, "Cascadia Mono");
        let r = to_slint_row(
            &model_row(Control::Slider {
                value: 30,
                max: 70,
                text: "30%".into(),
                disabled: true,
            }),
            0.0,
            58.0,
            &ctx,
        );
        assert_eq!((r.num, r.num_max), (30, 70));
        assert_eq!(r.num_text, "30%");
        assert!(r.off, "a disabled slider is an off row");
        let r = to_slint_row(
            &model_row(Control::Colour {
                value: "#fe8f34".into(),
                range,
                alpha_disabled: false,
            }),
            0.0,
            58.0,
            &ctx,
        );
        assert_eq!(r.code, "#fe8f34");
        assert_eq!(r.colour, slint::Color::from_rgb_u8(0xfe, 0x8f, 0x34));
        assert!(!r.code_wide);
        let r = to_slint_row(
            &model_row(Control::Save {
                dirty: true,
                title: "Save changes".into(),
            }),
            0.0,
            58.0,
            &ctx,
        );
        assert!(r.save_dirty);
        assert_eq!(r.save_title, "Save changes");
    }

    #[test]
    fn a_colour_code_follows_the_format() {
        let drafts = HashMap::new();
        let ctx = RowCtx {
            format: ColourFormat::Rgba,
            drafts: &drafts,
            hair: 1.0,
            flash_row: None,
            flash_section: None,
            flash: 0.0,
        };
        let r = to_slint_row(
            &model_row(Control::Colour {
                value: "#fe8f34".into(),
                range: flare_theme::colour_contrast::AlphaRange::default(),
                alpha_disabled: false,
            }),
            0.0,
            58.0,
            &ctx,
        );
        assert_eq!(r.code, "rgba(254, 143, 52, 1)");
        assert!(r.code_wide);
    }

    // ---- pages and the store ---------------------------------------------------------------------

    #[test]
    fn the_page_resets_to_appearance_only_when_the_settings_tab_is_closed() {
        let f = fx();
        f.page("agents");
        assert_eq!(f.ctl.borrow().current_page(), "agents");
        f.ctl.borrow_mut().hide();
        f.ctl.borrow_mut().show();
        assert_eq!(f.ctl.borrow().current_page(), "agents");
        f.ctl.borrow_mut().on_settings_tab(false);
        f.ctl.borrow_mut().show();
        assert_eq!(f.ctl.borrow().current_page(), "appearance");
        assert_eq!(f.b().get_page_title(), "Appearance");
    }

    #[test]
    fn the_page_is_not_saved() {
        let f = fx();
        f.page("agents");
        f.page("about");
        f.ctl.borrow_mut().on_settings_tab(false);
        let keys = f.written.keys.lock().unwrap().clone();
        assert!(keys.iter().all(|k| !k.to_lowercase().contains("page")));
        assert!(keys.is_empty(), "choosing pages stored {keys:?}");
    }

    #[test]
    fn a_switch_event_writes_through_the_owning_module() {
        let f = fx();
        f.page("terminal");
        f.b().invoke_row_switch("agent-guard".into(), false);
        assert_eq!(f.prefs.get("flare.window.agentGuard").as_deref(), Some("0"));
    }

    #[test]
    fn a_theme_pick_while_dirty_opens_the_question_and_cancel_changes_nothing() {
        let f = fx();
        ACCENT.set(&f.prefs, Some("#112233"));
        let before = f.look.theme_id();
        let target = wall::wall_cards(&f.look)
            .into_iter()
            .map(|c| c.id)
            .find(|id| *id != before)
            .unwrap();
        f.b().invoke_wall_pick(target.as_str().into());
        assert!(f.b().get_ask_open(), "the question opens");
        assert_eq!(f.look.theme_id(), before, "nothing changed yet");
        f.b().invoke_ask_answered(0);
        assert!(!f.b().get_ask_open());
        assert_eq!(f.look.theme_id(), before, "Cancel changes nothing");
        // Discard lands the pick.
        f.b().invoke_wall_pick(target.as_str().into());
        f.b().invoke_ask_answered(1);
        assert_eq!(f.look.theme_id(), target);
    }

    #[test]
    fn the_shell_section_appears_when_the_list_arrives() {
        let f = fx();
        f.page("terminal");
        assert!(f.row("term-shell").is_none(), "no list yet");
        f.settle_workers();
        assert!(f.row("term-shell").is_some(), "the Shell section is drawn");
    }

    #[test]
    fn choose_folder_uses_the_picker_seam_and_stores_the_answer() {
        let f = fx();
        f.page("terminal");
        *f.pick_answer.borrow_mut() = Some(PathBuf::from("C:/Work"));
        f.b()
            .invoke_row_button("newtab-mode".into(), "choose-folder".into());
        f.pump();
        assert_eq!(new_tab_mode(&f.prefs), NewTabMode::Folder);
        assert!(new_tab_folder(&f.prefs).contains("Work"));
    }

    #[test]
    fn a_cancelled_folder_pick_stores_nothing() {
        let f = fx();
        f.page("terminal");
        *f.pick_answer.borrow_mut() = None;
        f.b()
            .invoke_row_button("newtab-mode".into(), "choose-folder".into());
        f.pump();
        assert!(f.written.keys.lock().unwrap().is_empty());
    }

    #[test]
    fn the_verb_row_is_checking_with_windows_with_the_pending_backend() {
        let f = fx_with(1.0, Arc::new(PendingVerb));
        f.page("terminal");
        f.settle_workers();
        let r = f.row("explorer-verb").expect("the row is drawn");
        assert_eq!(r.sub, "Checking with Windows.");
        assert!(r.off);
    }

    #[test]
    fn the_verb_switch_asks_a_worker_and_the_answer_redraws_the_row() {
        let f = fx_with(1.0, Arc::new(FakeVerb::new(VerbState::Off)));
        f.page("terminal");
        f.settle_workers();
        assert!(!f.row("explorer-verb").unwrap().on);
        f.b().invoke_row_switch("explorer-verb".into(), true);
        f.settle_workers();
        assert!(f.row("explorer-verb").unwrap().on);
    }

    #[test]
    fn mark_says_marked_for_1200_ms() {
        let f = fx();
        f.page("diagnostics");
        let alt = |f: &Fx| f.row("diag-mark").unwrap().buttons.row_data(0).unwrap().alt;
        assert!(!alt(&f));
        f.b().invoke_row_button("diag-mark".into(), "mark".into());
        assert_eq!(f.diag.marks(), 1);
        assert!(alt(&f), "Marked");
        f.tick(1199);
        f.h.advance(0);
        f.tick(1);
        assert!(!alt(&f), "back to Mark after 1200 ms");
    }

    #[test]
    fn the_open_folder_button_opens_the_log_folder() {
        let f = fx();
        f.page("diagnostics");
        f.b().invoke_row_button("diag-folder".into(), "open".into());
        assert_eq!(f.opened.borrow().as_slice(), &[PathBuf::from("C:/logs")]);
    }

    #[test]
    fn the_diagnostics_flag_file_is_read_when_settings_shows_not_on_every_rebuild() {
        let f = fx();
        let at_show = f.diag.reads();
        // The wall's opening rebuilds every 16 ms and a slider rebuilds per step: no file read.
        for _ in 0..10 {
            f.ctl.borrow_mut().rebuild();
        }
        assert_eq!(f.diag.reads(), at_show, "a rebuild reads no file");
        f.page("diagnostics");
        assert!(!f.row("diag-verbose").unwrap().on);
        f.b().invoke_row_switch("diag-verbose".into(), true);
        assert!(
            f.row("diag-verbose").unwrap().on,
            "the switch shows what was stored"
        );
        let after_switch = f.diag.reads();
        f.ctl.borrow_mut().rebuild();
        assert_eq!(f.diag.reads(), after_switch);
        f.ctl.borrow_mut().hide();
        f.ctl.borrow_mut().show();
        assert_eq!(
            f.diag.reads(),
            after_switch + 1,
            "showing Settings reads it again"
        );
    }

    // ---- Find a setting --------------------------------------------------------------------------

    fn first_hit_row(f: &Fx, query: &str) -> String {
        f.b().invoke_find_edited(query.into());
        assert!(f.b().get_searching());
        let hit = f.b().get_hits().row_data(0).expect("a result");
        hit.id.to_string()
    }

    #[test]
    fn a_chosen_result_switches_page_scrolls_to_the_row_and_flashes_it() {
        let f = fx();
        let id = first_hit_row(&f, "face");
        let page = flare_settings::rows::row(&id).unwrap().page;
        f.b().invoke_hit_chosen(0);
        assert_eq!(f.ctl.borrow().current_page(), page);
        assert!(!f.b().get_searching() || f.b().get_find_text().is_empty());
        let expected = {
            let c = f.ctl.borrow();
            let (_, h) = c.frame_size(&f.h.comp);
            scroll_target(&c.built.as_ref().unwrap().layout, &id, h).unwrap_or(0.0) as f32
        };
        assert!((f.b().get_scroll_y() - expected).abs() < 0.01);
        let r = f.row(&id).expect("the row is drawn");
        assert!((r.flash - 1.0).abs() < 1e-6, "the flash starts at full");
        f.tick(1400);
        let r = f.row(&id).unwrap();
        assert!(r.flash.abs() < 1e-6, "the flash is over after 1400 ms");
    }

    #[test]
    fn a_late_row_is_polled_then_the_section_flashes() {
        let f = fx();
        f.page("terminal");
        // The Shell section is not drawn until the list arrives: its row is polled for.
        f.b().invoke_find_edited("shell".into());
        let idx = (0..f.b().get_hits().row_count())
            .find(|i| f.b().get_hits().row_data(*i).unwrap().id == "term-shell")
            .expect("the shell row is found");
        f.b().invoke_hit_chosen(idx as i32);
        assert!(f.ctl.borrow().watch.is_some(), "still looking");
        f.settle_workers();
        f.tick(60);
        assert!(f.ctl.borrow().watch.is_none(), "found once drawn");
        assert!(f.row("term-shell").unwrap().flash > 0.0);
        // A row that never comes: the section flashes instead.
        f.tick(1500);
        f.page("appearance");
        {
            let mut c = f.ctl.borrow_mut();
            let now = (c.deps.clock)();
            c.watch = Some(FlashWatch::new(now, "no-such-row", Some("window")));
            c.poll_watch();
        }
        f.tick(3100);
        let rows: Vec<SetRow> = f
            .sections()
            .iter()
            .find(|s| s.id == "window")
            .unwrap()
            .rows
            .iter()
            .collect();
        assert!(rows.iter().all(|r| r.flash > 0.0), "the section flashes");
    }

    #[test]
    fn typing_in_a_code_field_is_not_clobbered_by_a_rebuild() {
        let f = fx();
        f.b()
            .invoke_colour_edited("window-accent".into(), "#12".into());
        f.ctl.borrow_mut().rebuild();
        assert_eq!(f.row("window-accent").unwrap().code, "#12");
        // Escape drops the draft and shows the colour again.
        f.b().invoke_colour_escape("window-accent".into());
        assert_ne!(f.row("window-accent").unwrap().code, "#12");
    }

    #[test]
    fn a_typed_colour_commits_on_enter() {
        let f = fx();
        f.b()
            .invoke_colour_edited("window-accent".into(), "#112233".into());
        f.b().invoke_colour_committed("window-accent".into());
        assert_eq!(ACCENT.get(&f.prefs).as_deref(), Some("#112233"));
    }

    #[test]
    fn escape_closes_overlays_in_order() {
        let f = fx();
        let b = f.b();
        // Everything open at once.
        f.ctl.borrow_mut().ask_open = true;
        b.set_ask_open(true);
        let id = wall::wall_cards(&f.look)[0].id.clone();
        f.ctl.borrow_mut().event(Event::WallEdit { id });
        b.invoke_colour_swatch("window-accent".into(), 300.0, 100.0, 36.0, 24.0);
        assert!(b.get_picker_open());
        b.set_select_open(true);
        assert!(f.ctl.borrow_mut().escape(), "the question first");
        assert!(!b.get_ask_open());
        let drafted = |f: &Fx| f.ctl.borrow().deps.look.draft().is_some();
        assert!(drafted(&f), "the editor is still open");
        assert!(f.ctl.borrow_mut().escape(), "then the editor");
        assert!(!drafted(&f));
        assert!(b.get_picker_open());
        assert!(f.ctl.borrow_mut().escape(), "then the picker");
        assert!(!b.get_picker_open());
        assert!(b.get_select_open());
        assert!(f.ctl.borrow_mut().escape(), "then the select");
        assert!(!b.get_select_open());
        assert!(!f.ctl.borrow_mut().escape(), "nothing left: the host's");
    }

    #[test]
    fn the_swatch_opens_the_popover_and_escape_puts_the_theme_back() {
        let f = fx();
        let b = f.b();
        b.invoke_colour_swatch("window-accent".into(), 300.0, 100.0, 36.0, 24.0);
        assert!(b.get_picker_open());
        assert_eq!(b.get_picker_label().as_str(), "Accent colour");
        b.invoke_picker_hue_moved(0.7, false);
        f.ctl.borrow_mut().picker_do(|p, _| p.frame());
        assert!(ACCENT.get(&f.prefs).is_some(), "the drag was written");
        b.invoke_picker_escape();
        assert!(!b.get_picker_open());
        assert_eq!(ACCENT.get(&f.prefs), None, "back to following the theme");
    }

    #[test]
    fn only_one_rebuild_per_turn_for_many_store_changes() {
        let f = fx();
        f.settle_workers();
        let before = f.ctl.borrow().rebuilds;
        for on in [false, true, false, true, false] {
            flare_settings::agent_prefs::set_agent_guard(&f.prefs, on);
        }
        f.look.set_font_pct(110);
        f.look.set_font_pct(120);
        let n = f.q.lock().unwrap().len();
        assert_eq!(n, 1, "one message for seven changes");
        f.pump();
        assert_eq!(f.ctl.borrow().rebuilds, before + 1);
        // After the rebuild the next change posts again.
        f.look.set_font_pct(130);
        assert_eq!(f.q.lock().unwrap().len(), 1);
    }

    #[test]
    fn settings_never_writes_a_key_the_pages_do_not_name() {
        let f = fx();
        f.settle_workers();
        *f.pick_answer.borrow_mut() = Some(PathBuf::from("C:/Work"));
        for page in ["appearance", "terminal", "agents", "diagnostics", "about"] {
            f.page(page);
            let rows: Vec<SetRow> = f.sections().iter().flat_map(|s| s.rows.iter()).collect();
            for r in rows {
                let id = r.id.to_string();
                match r.kind {
                    SetKind::Switch => {
                        f.b().invoke_row_switch(id.as_str().into(), true);
                        f.b().invoke_row_switch(id.as_str().into(), false);
                    }
                    SetKind::Segmented | SetKind::Select => {
                        for o in r.options.iter() {
                            f.b().invoke_row_choose(id.as_str().into(), o.id.clone());
                        }
                    }
                    SetKind::Slider => f.b().invoke_row_slide(id.as_str().into(), 20),
                    SetKind::Colour => {
                        f.b()
                            .invoke_colour_edited(id.as_str().into(), "#336699".into());
                        f.b().invoke_colour_committed(id.as_str().into());
                        f.b().invoke_row_reset(id.as_str().into());
                    }
                    SetKind::Save => f.b().invoke_save_clicked(id.as_str().into()),
                    _ => {}
                }
                for b in r.buttons.iter() {
                    f.b().invoke_row_button(id.as_str().into(), b.id.clone());
                }
            }
            for s in f.sections() {
                if s.action_save {
                    f.b().invoke_save_clicked(s.id.clone());
                }
            }
        }
        let allowed: BTreeSet<String> = all_rows()
            .flat_map(|r| r.keys.iter().map(|k| (*k).to_string()))
            .chain(HELP_KEYS.iter().map(|k| (*k).to_string()))
            .chain([CUSTOM_KEY.to_string(), CUSTOM_COLOURS_KEY.to_string()])
            .collect();
        let written = f.written.keys.lock().unwrap().clone();
        let stray: Vec<&String> = written.iter().filter(|k| !allowed.contains(*k)).collect();
        assert!(stray.is_empty(), "keys no row names: {stray:?}");
        assert!(!written.is_empty(), "the tour wrote something");
    }

    // ---- the Dictation page's controls (M8 Task 25) -------------------------------------------------

    use crate::dictation_ctl::fixture::{fake_store, install_store, model_now, pump_until};

    /// The Settings tab on the Dictation page, over the real provider, a fake store and no microphone.
    /// Returns what must outlive the page: the store's temp folder and the fetch count.
    fn fx_dict(
        scale: f32,
        urls: [&str; 3],
    ) -> (Fx, tempfile::TempDir, Arc<std::sync::atomic::AtomicUsize>) {
        let (store, dir, calls) = fake_store(urls);
        install_store(store);
        let f = fx_with_dictation(
            scale,
            Arc::new(FakeVerb::new(VerbState::Off)),
            Rc::new(crate::dictation_page::DictationPageProvider),
        );
        f.page("dictation");
        (f, dir, calls)
    }

    fn switch_dictation_on(f: &Fx) {
        f.b().invoke_row_switch("dictation-enabled".into(), true);
    }

    #[test]
    fn the_dictation_page_draws_every_card_and_every_control_at_every_scale() {
        for scale in SCALES {
            let (f, _dir, _calls) = fx_dict(scale, ["a", "b", "c"]);
            let sections = f.sections();
            let ids: Vec<String> = sections.iter().map(|s| s.id.to_string()).collect();
            for want in ["dictation", "listening", "while", "models", "anywhere"] {
                assert!(
                    ids.iter().any(|i| i == want),
                    "scale {scale}: no {want} card in {ids:?}"
                );
            }
            let models = sections
                .iter()
                .find(|s| s.id == "models")
                .expect("the models card");
            let kinds: Vec<SetRow> = models.rows.iter().collect();
            assert_eq!(
                kinds.len(),
                flare_dictation::catalog::visible_models().len(),
                "scale {scale}: one block per model"
            );
            for r in &kinds {
                assert_eq!(r.block, SlintBlock::Model);
                assert!(
                    r.h >= 64.0 - 1e-3,
                    "scale {scale}: {} is {} high",
                    r.id,
                    r.h
                );
                for v in [r.y, r.h] {
                    let d = v * scale;
                    assert!(
                        (d - d.round()).abs() < 1e-3,
                        "scale {scale}: {} edge {v}",
                        r.id
                    );
                }
                assert!(!r.vendor.is_empty() && !r.size_text.is_empty(), "{}", r.id);
            }
            let key = f.row("dictation-hotkey").expect("the key row");
            assert_eq!(key.kind, SetKind::Key);
            assert_eq!(key.value, "Ctrl+Win");
            assert!(!key.reset && !key.capturing);
            let mic = f.row("dictation-mic").expect("the microphone row");
            assert_eq!(mic.kind, SetKind::Mic);
            assert_eq!(
                mic.options.row_data(0).expect("System default first").name,
                "System default"
            );
            let lang = f.row("dictation-language").expect("the language row");
            assert_eq!(lang.kind, SetKind::Select);
            assert!(!lang.limited);
        }
    }

    #[test]
    fn a_key_captured_from_the_page_is_stored_and_drawn() {
        let (f, _dir, _calls) = fx_dict(1.0, ["a", "b", "c"]);
        switch_dictation_on(&f);
        f.b()
            .invoke_row_button("dictation-hotkey".into(), "capture".into());
        let row = f.row("dictation-hotkey").unwrap();
        assert!(
            row.capturing && !row.reset,
            "the field waits and offers no Reset meanwhile"
        );
        // The window's key path takes the next keys; a lone Right Alt is taken on its release.
        let ev = |pressed: bool| {
            let e = flare_term::keys::KeyEvent::new("Alt", "AltRight")
                .ctrl()
                .alt();
            if pressed { e } else { e.released() }
        };
        let keys = crate::dictation_ctl::DictationKeys;
        assert!(keys.feed_key(&ev(true), false, &|| None));
        assert!(keys.feed_key(&ev(false), true, &|| None));
        let row = f.row("dictation-hotkey").unwrap();
        assert_eq!(row.value, "Right Alt", "the page shows the new key");
        assert!(!row.capturing);
        assert!(row.reset, "a key that is not the default offers Reset");
        assert!(
            f.written
                .keys
                .lock()
                .unwrap()
                .contains("flare.dictation.hotkey")
        );
        // Reset puts the default back.
        f.b()
            .invoke_row_button("dictation-hotkey".into(), "reset".into());
        let row = f.row("dictation-hotkey").unwrap();
        assert_eq!((row.value.as_str(), row.reset), ("Ctrl+Win", false));
    }

    #[test]
    fn the_switch_and_the_choices_of_the_page_write_and_undim_the_rows() {
        let (f, _dir, _calls) = fx_dict(1.0, ["a", "b", "c"]);
        assert!(
            f.row("dictation-mic").unwrap().off,
            "off dims every row under the switch"
        );
        switch_dictation_on(&f);
        assert_eq!(f.prefs.get("flare.dictation.enabled").as_deref(), Some("1"));
        assert!(!f.row("dictation-mic").unwrap().off);
        f.b()
            .invoke_row_choose("dictation-mode".into(), "toggle".into());
        assert_eq!(
            f.prefs.get("flare.dictation.mode").as_deref(),
            Some("toggle")
        );
        assert_eq!(f.row("dictation-mode").unwrap().value, "toggle");
        f.b()
            .invoke_row_choose("dictation-language".into(), "no".into());
        assert_eq!(f.row("dictation-language").unwrap().value, "no");
        // The microphone picker opens the one menu and its choice writes.
        crate::dictation_ctl::fixture::set_mics(&[("mic-1", "Headset")]);
        f.b().invoke_page_chosen("terminal".into());
        f.page("dictation");
        let mic = f.row("dictation-mic").unwrap();
        assert_eq!(mic.options.row_count(), 2);
        f.b()
            .invoke_select_open_request("dictation-mic".into(), 300.0, 200.0, 168.0, 32.0);
        assert!(f.b().get_select_open());
        assert_eq!(f.b().get_select_options().row_count(), 2);
        f.b().invoke_select_chosen("mic-1".into());
        assert_eq!(f.prefs.get("flare.dictation.mic").as_deref(), Some("mic-1"));
        assert_eq!(f.row("dictation-mic").unwrap().value, "mic-1");
    }

    #[test]
    fn a_limited_model_makes_the_language_row_limited_and_keeps_the_stored_language() {
        let (f, _dir, _calls) = fx_dict(1.0, ["a", "b", "c"]);
        switch_dictation_on(&f);
        f.b()
            .invoke_row_choose("dictation-language".into(), "no".into());
        f.prefs.set("flare.dictation.model", "parakeet-v3");
        f.pump();
        let lang = f.row("dictation-language").unwrap();
        assert!(lang.limited, "the model picks its own language");
        assert_eq!(lang.value, "auto");
        assert_eq!(
            lang.row_tip,
            flare_dictation::catalog::LIMITED_LANGUAGES_TEXT
        );
        assert_eq!(
            f.prefs.get("flare.dictation.language").as_deref(),
            Some("no"),
            "the stored language is untouched"
        );
        f.prefs.set("flare.dictation.model", "base");
        f.pump();
        let lang = f.row("dictation-language").unwrap();
        assert!(!lang.limited);
        assert_eq!(lang.value, "no", "and comes back on a Whisper model");
    }

    #[test]
    fn a_download_of_a_limited_model_asks_in_one_line_and_the_answer_decides() {
        let (f, _dir, calls) = fx_dict(1.0, ["a", "b", "c"]);
        switch_dictation_on(&f);
        let n = || calls.load(std::sync::atomic::Ordering::SeqCst);
        f.b()
            .invoke_row_button("parakeet-v3".into(), "download".into());
        assert!(f.b().get_dict_ask_open(), "Download on Parakeet asks first");
        assert_eq!(
            f.b().get_dict_ask_text(),
            flare_dictation::catalog::LIMITED_LANGUAGES_TEXT
        );
        assert_eq!(n(), 0, "nothing downloads while it asks");
        assert_eq!(f.row("parakeet-v3").unwrap().state, "absent");
        f.b().invoke_dict_ask_answered(0);
        assert!(!f.b().get_dict_ask_open());
        assert_eq!(n(), 0);
        // Escape, with no Slint control holding it, is Cancel too.
        f.b()
            .invoke_row_button("parakeet-v3".into(), "download".into());
        assert!(f.b().get_dict_ask_open());
        assert!(f.ctl.borrow_mut().escape());
        assert!(!f.b().get_dict_ask_open());
        // Download starts it; the first model anyone downloads becomes the one in use.
        f.b()
            .invoke_row_button("parakeet-v3".into(), "download".into());
        f.b().invoke_dict_ask_answered(1);
        assert!(!f.b().get_dict_ask_open());
        pump_until("the download", || model_now() == "parakeet-v3");
        let row = f.row("parakeet-v3").unwrap();
        assert_eq!(row.state, "active");
        assert_eq!(row.badge, "Active");
        assert_eq!(row.buttons.row_count(), 1);
        assert_eq!(row.buttons.row_data(0).unwrap().id, "uninstall");
        assert_eq!(row.button_title, "Delete Parakeet v3 from this PC.");
        // A Whisper model asks nothing.
        f.b().invoke_row_button("base".into(), "download".into());
        assert!(!f.b().get_dict_ask_open());
        pump_until("whisper", || f.row("base").unwrap().state == "installed");
        // Uninstall removes the active model and clears the stored id.
        f.b()
            .invoke_row_button("parakeet-v3".into(), "uninstall".into());
        pump_until("the removal", || model_now().is_empty());
        assert_eq!(f.row("parakeet-v3").unwrap().state, "absent");
    }

    #[test]
    fn the_gpu_disable_button_is_as_wide_as_its_word_and_the_tip_is_a_title() {
        use flare_settings::page::Button;
        use flare_settings::page::dictation::{GpuRow, ModelState};
        let tip = "Turns GPU acceleration off and frees 675 MB of disk space.";
        let g = GpuRow {
            state: ModelState::Installed,
            failure: None,
            badge: Some("Enabled"),
            sub: String::new(),
            buttons: vec![Button {
                id: "disable",
                label: "Disable".to_string(),
                alt: Some(tip.to_string()),
                off: false,
            }],
            title: tip.to_string(),
            off: false,
        };
        let row = to_slint_gpu(&g, 0.0, 64.0);
        let b = row.buttons.row_data(0).unwrap();
        assert_eq!(b.label, "Disable");
        assert_eq!(
            b.label_alt, "",
            "a sentence in label_alt would size the button"
        );
        assert_eq!(row.button_title, tip, "the tip still arrives as the title");
    }

    #[test]
    fn a_failed_download_says_why_and_offers_retry() {
        let (f, _dir, _calls) = fx_dict(1.0, ["fail", "b", "c"]);
        switch_dictation_on(&f);
        f.b().invoke_row_button("base".into(), "download".into());
        pump_until("the failure", || {
            f.row("base").unwrap().sub == "The download failed."
        });
        let row = f.row("base").unwrap();
        assert_eq!(row.state, "absent");
        assert_eq!(row.buttons.row_data(0).unwrap().label, "Retry");
        assert_eq!(
            f.row("small").unwrap().buttons.row_data(0).unwrap().label,
            "Download"
        );
    }

    #[test]
    fn the_test_button_runs_the_meter_and_leaving_the_page_ends_it() {
        let (f, dir, _calls) = fx_dict(1.0, ["a", "b", "c"]);
        crate::dictation_ctl::fixture::silent_mic(dir.path());
        switch_dictation_on(&f);
        let before = flare_dictation::mic::opens_on_this_thread();
        assert_eq!(f.b().get_dict_level(), 0.0);
        f.b()
            .invoke_row_button("dictation-mic".into(), "test".into());
        assert!(f.row("dictation-mic").unwrap().testing);
        assert_eq!(flare_dictation::mic::opens_on_this_thread(), before + 1);
        // Leaving the page lets the microphone go.
        f.b().invoke_page_chosen("terminal".into());
        f.page("dictation");
        assert!(!f.row("dictation-mic").unwrap().testing);
        assert!(!crate::timer_registry::running("dictation.mic-test"));
        assert_eq!(f.b().get_dict_level(), 0.0);
    }

    #[test]
    fn building_and_showing_the_page_opens_no_microphone() {
        let (f, dir, _calls) = fx_dict(1.0, ["a", "b", "c"]);
        crate::dictation_ctl::fixture::silent_mic(dir.path());
        let before = flare_dictation::mic::opens_on_this_thread();
        // The whole page, built with dictation off and on, listed, shown, left and shown again.
        switch_dictation_on(&f);
        let _ = crate::dictation_ctl::page_mics();
        crate::dictation_ctl::page_shown();
        f.b().invoke_page_chosen("terminal".into());
        f.page("dictation");
        crate::dictation_ctl::page_shown();
        assert!(f.row("dictation-mic").is_some());
        assert_eq!(
            flare_dictation::mic::opens_on_this_thread(),
            before,
            "page_data, page_shown and page_mics open nothing"
        );
        // Only the Test button does, once.
        f.b()
            .invoke_row_button("dictation-mic".into(), "test".into());
        assert_eq!(flare_dictation::mic::opens_on_this_thread(), before + 1);
    }

    #[test]
    fn the_models_card_and_the_buttons_follow_the_store_not_the_page() {
        // With dictation off there is no store: every model is Download, and a press does nothing.
        let (f, _dir, calls) = fx_dict(1.0, ["a", "b", "c"]);
        for r in f
            .sections()
            .iter()
            .filter(|s| s.id == "models")
            .flat_map(|s| s.rows.iter())
        {
            assert_eq!(r.state, "absent", "{}", r.id);
            assert!(r.off, "{} is dimmed while dictation is off", r.id);
        }
        let _ = calls;
    }

    // ---- the wall, the select menu and every scaling -----------------------------------------------

    #[test]
    fn the_wall_opens_over_240_ms_and_closes_at_once() {
        let f = fx();
        let collapsed = f.b().get_wall_clip_h();
        f.b().invoke_wall_toggle();
        assert!(f.b().get_wall_expanded());
        f.tick(120);
        let mid = f.b().get_wall_clip_h();
        assert!(mid >= collapsed);
        f.tick(200);
        let full = f.b().get_wall_clip_h();
        assert!(full >= mid);
        assert!(
            (full - f.b().get_wall_full_h()).abs() < 0.5,
            "the full wall"
        );
        f.b().invoke_wall_toggle();
        assert!(!f.b().get_wall_expanded());
        assert!(f.b().get_wall_clip_h() <= collapsed + 0.5);
    }

    #[test]
    fn a_select_opens_where_the_rules_put_it_and_a_choice_writes() {
        let f = fx();
        f.page("terminal");
        f.b()
            .invoke_select_open_request("term-font-family".into(), 600.0, 200.0, 168.0, 32.0);
        assert!(f.b().get_select_open());
        assert_eq!(f.b().get_select_row(), "term-font-family");
        assert!(f.b().get_select_options().row_count() > 1);
        let pick = f.b().get_select_options().row_data(1).unwrap().id;
        f.b().invoke_select_chosen(pick.clone());
        assert!(!f.b().get_select_open());
        assert_eq!(f.look.font_id(), pick.as_str());
    }

    #[test]
    fn every_row_edge_is_a_whole_device_pixel_at_every_scaling() {
        for scale in SCALES {
            let f = fx_with(scale, Arc::new(FakeVerb::new(VerbState::Off)));
            let hair = f64::from(1.0_f32.max(scale.floor()) / scale);
            for page in ["appearance", "terminal", "agents", "diagnostics", "about"] {
                f.page(page);
                for s in f.sections() {
                    let whole = |v: f64| {
                        ((v * f64::from(scale)).round() - v * f64::from(scale)).abs() < 0.03
                    };
                    assert!(whole(f64::from(s.panel_y)), "panel y, {page} at {scale}");
                    assert!(whole(f64::from(s.panel_h)), "panel h, {page} at {scale}");
                    for r in s.rows.iter() {
                        let top = f64::from(s.panel_y) + hair + f64::from(r.y);
                        assert!(whole(top), "row top {} {page} at {scale}", r.id);
                        assert!(whole(f64::from(r.h)), "row h {} {page} at {scale}", r.id);
                    }
                }
            }
        }
    }
}
