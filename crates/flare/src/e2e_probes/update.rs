//! The update chip and window probe (Task 39): `update.window`. Read-only: it asks the window for
//! what it shows and where, and sets nothing.
//!
//! Prism Terminal's `updateWindow`, `updateGuard` and `updateNotes` read the dialog's DOM (headings,
//! entries, buttons, boxes, the progress track). Flare answers with the same facts from the
//! window's own properties, and the rectangles from the boxes `app.slint` exports.

use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::{ProbeCtx, ProbeRegistry};
use crate::AppWindow;

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("update.window", update_window);
}

/// One rectangle in css pixels and in device pixels (css times the scale, rounded).
fn rect(x: f32, y: f32, w: f32, h: f32, scale: f32) -> Value {
    let d = |v: f32| (v * scale).round() as i64;
    json!({
        "x": x, "y": y, "w": w, "h": h,
        "dx": d(x), "dy": d(y), "dw": d(w), "dh": d(h),
    })
}

/// The phase the window is in, from what it shows: idle when nothing runs, installing once the
/// status line says so, downloading otherwise (a cancel in flight is still a download).
pub fn phase_of(running: bool, status: &str) -> &'static str {
    if !running {
        "idle"
    } else if status.starts_with("Installing") {
        "installing"
    } else {
        "downloading"
    }
}

/// `update.window`: the chip, the notice and the window, as the e2e scenarios read them.
fn update_window(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    Ok(window_json(ctx.app))
}

/// The whole answer, from the window alone (so a test can drive it with no controller).
pub fn window_json(a: &AppWindow) -> Value {
    let k = a.window().scale_factor();
    let sections: Vec<Value> = a
        .get_update_sections()
        .iter()
        .map(|s| {
            json!({
                "name": s.heading.to_string(),
                "items": s.entries.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let status = a.get_update_status().to_string();
    let running = a.get_update_running();
    let track = rect(
        a.get_update_track_x(),
        a.get_update_track_y(),
        a.get_update_track_w(),
        a.get_update_track_h(),
        k,
    );
    let fill = rect(
        a.get_update_track_x(),
        a.get_update_track_y(),
        a.get_update_fill_w(),
        a.get_update_track_h(),
        k,
    );
    json!({
        "open": a.get_update_open(),
        "title": a.get_update_title().to_string(),
        "subtitle": a.get_update_sub().to_string(),
        "sections": sections,
        "more": a.get_update_more().to_string(),
        "empty_note": a.get_update_empty_note().to_string(),
        "status": status,
        "phase": phase_of(running, &status),
        "pct": a.get_update_pct(),
        "pct_text": a.get_update_pct_text().to_string(),
        "running": running,
        "active": a.get_update_active(),
        "focus_idx": a.get_update_focus_idx(),
        "buttons": [
            { "label": a.get_update_cancel_label().to_string(), "enabled": a.get_update_cancel_enabled() },
            { "label": "Install", "enabled": a.get_update_install_enabled() },
        ],
        "box": rect(a.get_update_box_x(), a.get_update_box_y(), a.get_update_box_w(), a.get_update_box_h(), k),
        "track": track,
        "fill": fill,
        "notes": {
            "y": a.get_update_notes_y(),
            "h": a.get_update_notes_h(),
            "content_h": a.get_update_notes_content_h(),
        },
        "install": rect(a.get_update_install_x(), a.get_update_install_y(), a.get_update_install_w(), a.get_update_install_h(), k),
        "cancel": rect(a.get_update_cancel_x(), a.get_update_cancel_y(), a.get_update_cancel_w(), a.get_update_cancel_h(), k),
        "notice": a.get_update_notice().to_string(),
        "chip": {
            "shown": a.get_update_chip_shown(),
            "label": a.get_update_chip_label().to_string(),
            "aria": a.get_update_chip_aria().to_string(),
            "tip": a.get_update_chip_tip().to_string(),
            "busy": a.get_update_chip_busy(),
            "rect": rect(a.get_update_chip_x(), a.get_update_chip_y(), a.get_update_chip_w(), a.get_update_chip_h(), k),
            "icon_h": a.get_update_chip_icon_h(),
        },
    })
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use slint::{ModelRc, SharedString, VecModel};

    use super::*;
    use crate::NoteSection;
    use crate::ui_harness::Harness;

    fn window(scale: f32) -> Harness<AppWindow> {
        Harness::new(scale, 1000, 640, || AppWindow::new().unwrap())
    }

    fn section(name: &str, items: &[&str]) -> NoteSection {
        NoteSection {
            heading: name.into(),
            entries: ModelRc::new(VecModel::from(
                items
                    .iter()
                    .map(|s| SharedString::from(*s))
                    .collect::<Vec<_>>(),
            )),
        }
    }

    #[test]
    fn the_phase_follows_what_the_window_shows() {
        assert_eq!(
            phase_of(false, "Preview only: nothing was installed"),
            "idle"
        );
        assert_eq!(phase_of(true, "Downloading the update"), "downloading");
        assert_eq!(phase_of(true, "Cancelling\u{2026}"), "downloading");
        assert_eq!(
            phase_of(true, "Installing (preview: nothing is installed)"),
            "installing"
        );
    }

    #[test]
    fn the_probe_names_the_sections_the_buttons_and_a_box_inside_the_window() {
        for scale in [1.0_f32, 2.25] {
            let h = window(scale);
            let model = Rc::new(VecModel::from(vec![
                section("New features", &["One", "Two"]),
                section("Bug fixes", &["Three"]),
            ]));
            h.comp.set_update_sections(ModelRc::from(model));
            h.comp.set_update_title("Update to 0.56.0".into());
            h.comp.set_update_sub("You have 0.55.6".into());
            h.comp.set_update_open(true);
            h.advance(0);
            let v = window_json(&h.comp);
            assert_eq!(v["open"], true);
            assert_eq!(v["title"], "Update to 0.56.0");
            assert_eq!(v["subtitle"], "You have 0.55.6");
            assert_eq!(v["sections"][0]["name"], "New features");
            assert_eq!(v["sections"][0]["items"][1], "Two");
            assert_eq!(v["sections"][1]["items"][0], "Three");
            assert_eq!(v["buttons"][0]["label"], "Cancel");
            assert_eq!(v["buttons"][1]["label"], "Install");
            let (bw, bh) = (
                v["box"]["w"].as_f64().unwrap(),
                v["box"]["h"].as_f64().unwrap(),
            );
            assert!(
                bw > 100.0 && bh > 100.0,
                "an open window has a box ({bw} x {bh})"
            );
            assert!(v["box"]["x"].as_f64().unwrap() >= 0.0);
            assert!(
                v["box"]["y"].as_f64().unwrap() + bh <= 640.0,
                "the box fits the window at scale {scale}"
            );
            assert_eq!(
                v["track"]["h"].as_f64().unwrap().round(),
                6.0,
                "the progress track is six css pixels high, in the layout from the start"
            );
            assert_eq!(v["phase"], "idle");
        }
    }

    #[test]
    fn a_closed_window_with_no_offer_says_so() {
        let h = window(1.0);
        h.advance(0);
        let v = window_json(&h.comp);
        assert_eq!(v["open"], false);
        assert_eq!(v["chip"]["shown"], false);
        assert_eq!(v["notice"], "");
    }

    #[test]
    fn the_fill_follows_the_percentage_while_running() {
        let h = window(1.0);
        h.comp.set_update_open(true);
        h.comp.set_update_running(true);
        h.comp.set_update_active(true);
        h.comp.set_update_pct(50);
        h.comp.set_update_status("Downloading the update".into());
        h.advance(1000);
        let v = window_json(&h.comp);
        assert_eq!(v["phase"], "downloading");
        let (t, f) = (
            v["track"]["w"].as_f64().unwrap(),
            v["fill"]["w"].as_f64().unwrap(),
        );
        assert!(
            (f / t - 0.5).abs() < 0.05,
            "half the track is filled ({f} of {t})"
        );
    }
}
