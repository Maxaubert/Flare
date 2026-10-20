//! The M8 scaling images (Task 35): every dictation surface drawn at the eight display scalings on the
//! software renderer, one PNG per surface and scaling, for the contact sheets that a person reads.
//!
//! The surfaces are the components the M8 pixel tests draw (the pill, the tab mark, the Dictation page
//! controls, the widget card), in the states those tests use. `write_scaling_images` is an ignored test
//! that writes `<scene>-<tag>-software.png` into the folder named by `FLARE_SCALING_OUT`; `tools/dpi-matrix/
//! compose.ps1` makes the sheets from that folder. Nothing here opens a window or touches the microphone.

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use flare_dictation::route::OverlayRow;
use flare_dictation::service::{OverlayState, Tone};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::dictation_widget::{face_of, set_face};
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{
    DictationPageControls, DictationPillPreview, DictationTabMarkPreview, DictationWidget,
    SetButton, SetRow, WidgetItem,
};

/// One surface in one state: its file name, its css size and how to draw it at a scaling.
struct Scene {
    name: &'static str,
    css: (u32, u32),
    draw: Box<dyn Fn(f32) -> Frame>,
}

/// The number of surfaces and states the sheets show: four pill faces, the tab mark, three page
/// states, five widget faces.
const SCENE_COUNT: usize = 13;

/// Renders `make()` after `set` has set its state and the clock has moved `ms` milliseconds.
fn render_with<C: ComponentHandle>(
    scale: f32,
    css: (u32, u32),
    make: fn() -> C,
    set: fn(&C),
    ms: u64,
) -> Frame {
    let h = Harness::new(scale, css.0, css.1, make);
    set(&h.comp);
    h.advance(ms);
    h.render()
}

fn scene<C: ComponentHandle + 'static>(
    name: &'static str,
    css: (u32, u32),
    make: fn() -> C,
    set: fn(&C),
) -> Scene {
    Scene {
        name,
        css,
        draw: Box::new(move |s| render_with(s, css, make, set, 500)),
    }
}

// ---- the pill and the tab mark ---------------------------------------------------------------

fn pill_listening(c: &DictationPillPreview) {
    c.set_phase(1);
    c.set_live_text("the newest words of a long sentence".into());
    c.set_level(0.5);
}

fn pill_starting(c: &DictationPillPreview) {
    c.set_phase(2);
    c.set_starting(true);
}

fn pill_transcribing(c: &DictationPillPreview) {
    c.set_phase(2);
    c.set_starting(false);
}

fn pill_message(c: &DictationPillPreview) {
    c.set_phase(3);
    c.set_message("Typed into the tab in front".into());
}

fn tab_mark_on(c: &DictationTabMarkPreview) {
    c.set_active(true);
}

fn make_pill() -> DictationPillPreview {
    DictationPillPreview::new().unwrap()
}

fn make_mark() -> DictationTabMarkPreview {
    DictationTabMarkPreview::new().unwrap()
}

// ---- the Dictation page controls --------------------------------------------------------------

const MODEL_NAME: &str = "Whisper Base";

fn button(id: &str, label: &str) -> SetButton {
    SetButton {
        id: id.into(),
        label: label.into(),
        label_alt: Default::default(),
        off: false,
        alt: false,
    }
}

fn model(id: &str, state: &str, buttons: Vec<SetButton>) -> SetRow {
    SetRow {
        id: id.into(),
        label: MODEL_NAME.into(),
        sub: "Fast live text on any PC.".into(),
        vendor: "openai".into(),
        size_text: "148 MB".into(),
        state: state.into(),
        buttons: ModelRc::new(VecModel::from(buttons)),
        ..Default::default()
    }
}

/// The three model rows the page tests use: the one in use, a download at 50 percent, and one not
/// yet downloaded (Recommended).
fn page_models(c: &DictationPageControls) {
    let mut a = model("base", "active", vec![button("uninstall", "Uninstall")]);
    a.badge = "Active".into();
    a.button_title = "Delete Whisper Base from this PC.".into();
    let mut b = model("small", "downloading", vec![button("cancel", "Cancel")]);
    b.num = 50;
    let mut c_row = model("tiny", "absent", vec![button("download", "Download")]);
    c_row.recommended = true;
    c.set_row_a(a);
    c.set_row_b(b);
    c.set_row_c(c_row);
}

fn page_top(_c: &DictationPageControls) {}

fn page_ask(c: &DictationPageControls) {
    page_models(c);
    c.set_ask_open(true);
}

fn make_page() -> DictationPageControls {
    DictationPageControls::new().unwrap()
}

// ---- the widget card --------------------------------------------------------------------------

fn row(id: &str, n: u8, group: Option<&str>) -> OverlayRow {
    OverlayRow {
        id: id.into(),
        n,
        label: format!("tab {n}"),
        folder: "Flare".into(),
        state: None,
        disabled: None,
        lead: false,
        group: group.map(str::to_string),
    }
}

/// The widget card at `scale`, in a window as tall as the face needs (480 css px wide).
fn widget_frame(scale: f32, state: &OverlayState) -> Frame {
    let f = face_of(state);
    let h = Harness::new(scale, 480, f.height, || DictationWidget::new().unwrap());
    let model = Rc::new(VecModel::<WidgetItem>::default());
    h.comp.set_items(ModelRc::from(model.clone()));
    set_face(&h.comp, &model, &f);
    h.advance(500);
    h.render()
}

fn widget(name: &'static str, state: OverlayState) -> Scene {
    let height = face_of(&state).height;
    Scene {
        name,
        css: (480, height),
        draw: Box::new(move |s| widget_frame(s, &state)),
    }
}

// ---- the scene list ---------------------------------------------------------------------------

/// The pill's test harness size (css px); the tab mark's strip and the page's window.
const PILL_CSS: (u32, u32) = (400, 300);
const MARK_CSS: (u32, u32) = (400, 36);
const PAGE_CSS: (u32, u32) = (640, 520);

fn scenes() -> Vec<Scene> {
    let text = "check the build";
    vec![
        scene(
            "dictation-pill-listening",
            PILL_CSS,
            make_pill,
            pill_listening,
        ),
        scene(
            "dictation-pill-starting",
            PILL_CSS,
            make_pill,
            pill_starting,
        ),
        scene(
            "dictation-pill-transcribing",
            PILL_CSS,
            make_pill,
            pill_transcribing,
        ),
        scene("dictation-pill-message", PILL_CSS, make_pill, pill_message),
        scene("dictation-mark-on", MARK_CSS, make_mark, tab_mark_on),
        scene("dictation-page-top", PAGE_CSS, make_page, page_top),
        scene("dictation-page-models", PAGE_CSS, make_page, page_models),
        scene("dictation-page-ask", PAGE_CSS, make_page, page_ask),
        widget(
            "dictation-widget-listening",
            OverlayState::Listening {
                text: text.into(),
                level: 0.5,
                starting: false,
            },
        ),
        widget(
            "dictation-widget-starting",
            OverlayState::Listening {
                text: String::new(),
                level: 0.0,
                starting: true,
            },
        ),
        widget(
            "dictation-widget-pick",
            OverlayState::Pick {
                text: text.into(),
                rows: vec![
                    row("a", 1, Some("Flare")),
                    row("b", 2, Some("Flare")),
                    row("c", 3, Some("Flare (2)")),
                ],
            },
        ),
        widget(
            "dictation-widget-sent",
            OverlayState::Sent { to: "Flare".into() },
        ),
        widget(
            "dictation-widget-warning",
            OverlayState::Note {
                message: "No Claude session to send to".into(),
                tone: Tone::Warn,
            },
        ),
    ]
}

/// The file tag of a scaling, as `tools/dpi-matrix/run.ps1` writes it (1.25 is `1_25`).
fn tag(scale: f32) -> String {
    scale.to_string().replace('.', "_")
}

#[test]
fn every_surface_is_drawn_at_every_scale() {
    let scenes = scenes();
    assert_eq!(scenes.len(), SCENE_COUNT, "the M8 surfaces and states");
    for scene in &scenes {
        for s in SCALES {
            let f = (scene.draw)(s);
            let want_w = (scene.css.0 as f32 * s).round() as u32;
            assert!(
                f.w.abs_diff(want_w) <= 1,
                "{} at {s}: width {}, want {want_w}",
                scene.name,
                f.w
            );
            let shades: HashSet<&[u8; 4]> = f.rgba.as_chunks::<4>().0.iter().collect();
            assert!(shades.len() > 2, "{} at {s}: blank", scene.name);
        }
    }
}

#[test]
#[ignore = "writes the scaling images; set FLARE_SCALING_OUT to a folder and run with --run-ignored"]
fn write_scaling_images() {
    let out = std::env::var_os("FLARE_SCALING_OUT")
        .expect("set FLARE_SCALING_OUT to the folder the images go in");
    let out = Path::new(&out);
    std::fs::create_dir_all(out).expect("the images folder");
    for scene in scenes() {
        for s in SCALES {
            let f = (scene.draw)(s);
            let path = out.join(format!("{}-{}-software.png", scene.name, tag(s)));
            f.save_png(&path).expect("a scaling image");
        }
    }
}
