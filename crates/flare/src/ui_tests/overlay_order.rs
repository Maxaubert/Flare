//! The overlay order (parity M1 line 40), bottom to top: panel, pop, menu, picker, modal, toast, tip
//! (PT core/renderer/components/layers.ts LAYER). Slint paints later siblings over earlier ones, so
//! the order is the order of the declarations at the end of `ui/app.slint`: one scan pins the
//! source, one pixel test shows two overlays over each other at all eight display scales.

use slint::{ModelRc, VecModel};

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{AppWindow, MenuRow};

const APP: &str = include_str!("../../ui/app.slint");

/// Where each overlay component sits in PT's LAYER table. Two entries are a documented reading:
/// `OverflowList` is PT's `TabOverflowList`, which uses `LAYER.menu`; `ThemeEditor` and `AskThree`
/// host the colour picker (PT draws a picker inside a dialog at z-10 of that dialog, so it is over
/// it), so they share the picker's rank and the picker is declared after them. `DictAsk` (the Dictation page's
/// one-line download question) is a Settings dialog like `AskThree` and takes the same rank (M8 Task 25). The
/// Plugins dropdown (plugins phase 4) is a pop like the Settings dropdown: a section's header menu opens over it.
/// A plugin's question (`PluginAsk`, the `fill` permission) is a modal declared under the close question.
const RANKS: [(&str, u8, &str); 16] = [
    ("UpdateNotice", 1, "pop"),
    ("SelectMenu", 1, "pop"),
    ("PluginsDropdown", 1, "pop"),
    ("ContextMenu", 2, "menu"),
    ("OverflowList", 2, "menu"),
    ("ThemeEditor", 3, "picker"),
    ("AskThree", 3, "picker"),
    ("DictAsk", 3, "picker"),
    ("ColourPopover", 3, "picker"),
    ("UpdateWindow", 4, "modal"),
    ("PluginAsk", 4, "modal"),
    ("Dialog", 4, "modal"),
    ("CopiedBadge", 5, "toast"),
    ("Tip", 6, "tip"),
    ("Timer", 0, "not drawn"),
    ("Rectangle", 0, "the window edge"),
];

/// The component types declared directly in the overlay layer, in source order.
fn overlay_types() -> Vec<String> {
    let at = APP.find("// (4) OVERLAYS").expect("the overlay marker");
    let mut out = Vec::new();
    for line in APP[at..].lines().skip(1) {
        // The overlays sit at 8 spaces; their properties at 12, so a deeper line is never a type.
        if !line.starts_with("        ") || line.starts_with("         ") {
            continue;
        }
        let t = line.trim();
        let t = t.split(" := ").last().unwrap_or(t);
        if let Some(name) = t.strip_suffix(" {")
            && name.chars().all(|c| c.is_ascii_alphanumeric())
        {
            out.push(name.to_string());
        }
    }
    out
}

#[test]
fn the_overlays_in_app_slint_are_declared_in_layer_order() {
    let types = overlay_types();
    let mut last = 0u8;
    let mut last_name = "";
    for name in &types {
        let (_, rank, layer) = RANKS
            .iter()
            .find(|(n, _, _)| n == name)
            .unwrap_or_else(|| panic!("{name} is an overlay with no rank: add it to RANKS"));
        if *rank == 0 {
            continue;
        }
        assert!(
            *rank >= last,
            "{name} (layer {layer}) is declared after {last_name} but belongs under it"
        );
        last = *rank;
        last_name = name;
    }
    // Every layer above the panel is present, and the tip is last.
    for (n, rank, _) in RANKS.iter().filter(|r| r.1 > 0) {
        assert!(types.iter().any(|t| t == n), "{n} (rank {rank}) not found");
    }
    assert_eq!(
        types.iter().rev().find(|t| t.as_str() != "Rectangle"),
        Some(&"Tip".to_string()),
        "the tip is the last overlay"
    );
}

#[test]
fn the_panels_are_declared_before_the_overlay_layer_and_carry_no_z_above_it() {
    let at = APP.find("// (4) OVERLAYS").expect("the overlay marker");
    let page = &APP[..at];
    for panel in ["HelpPanel", "FindBar", "TabSidebar"] {
        assert!(
            page.contains(&format!("{panel} {{")),
            "{panel} is in the page"
        );
    }
    // The two z values in the page are the help panel against the find bar; the page itself is a
    // sibling below the overlays, so no z there can lift a panel over a menu.
    assert!(
        !APP[at..].contains("z:"),
        "no overlay uses z: the order is the source order"
    );
}

// ---- the pixel test ---------------------------------------------------------------------------

fn window(scale: f32, h: u32) -> Harness<AppWindow> {
    let hn = Harness::new(scale, 420, h, || {
        AppWindow::new().expect("the window builds")
    });
    hn.advance(0);
    hn
}

fn rows() -> ModelRc<MenuRow> {
    let row = |l: &str| MenuRow {
        label: l.into(),
        hint: "".into(),
        icon: "".into(),
        filled_icon: false,
        disabled: false,
        trailing_icon: "".into(),
        trailing_filled: false,
        trailing_tip: "".into(),
        keep_open: false,
        group: false,
        flyout: false,
        flyout_pick: false,
        parent: -1,
    };
    ModelRc::new(VecModel::from(vec![
        row("Copy"),
        row("Paste"),
        row("Rename"),
        row("Close"),
    ]))
}

/// The bounding box (device pixels) where `a` differs from `z`, or None.
fn footprint(a: &Frame, z: &Frame) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..a.h {
        for x in 0..a.w {
            if !Frame::near(a.px(x, y), z.px(x, y), 1) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x1 >= x0 && y1 >= y0).then_some((x0, y0, x1, y1))
}

/// What is shown. `At` ones move to where the caller says; the rest sit where the app puts them.
#[derive(Clone, Copy)]
enum Item {
    Notice,
    Menu,
    Dialog,
    Badge,
    Tip,
}

/// Where an item draws: its footprint against the blank window, except the dialog, whose scrim covers
/// the window, and the tip, whose shadow is see-through: those are their boxes.
fn bounds(
    h: &Harness<AppWindow>,
    item: Item,
    f: &Frame,
    blank: &Frame,
) -> Option<(u32, u32, u32, u32)> {
    if matches!(item, Item::Dialog) {
        let s = h.scale();
        let p = |v: f32| (v * s).round() as u32;
        let (x, y) = (h.comp.get_dialog_box_x(), h.comp.get_dialog_box_y());
        let (w, hh) = (h.comp.get_dialog_box_w(), h.comp.get_dialog_box_h());
        return Some((p(x), p(y), p(x + w) - 1, p(y + hh) - 1));
    }
    if matches!(item, Item::Tip) {
        let s = h.scale();
        let p = |v: f32| (v * s).round() as u32;
        let (x, y) = (h.comp.get_tip_x(), h.comp.get_tip_y());
        let (w, hh) = (h.comp.get_tip_w(), h.comp.get_tip_h());
        return Some((
            p(x + 3.0),
            p(y + 3.0),
            p(x + w - 3.0) - 1,
            p(y + hh - 3.0) - 1,
        ));
    }
    if matches!(item, Item::Menu) {
        let s = h.scale();
        let p = |v: f32| (v * s).round() as u32;
        let (x, y) = (h.comp.get_menu_panel_x(), h.comp.get_menu_panel_y());
        let (w, hh) = (h.comp.get_menu_w(), h.comp.get_menu_h());
        return Some((
            p(x + 1.0),
            p(y + 1.0),
            p(x + w - 1.0) - 1,
            p(y + hh - 1.0) - 1,
        ));
    }
    footprint(f, blank)
}

fn show(h: &Harness<AppWindow>, item: Item, at: (f32, f32)) {
    match item {
        Item::Notice => h.comp.set_update_notice("Flare 9.9.9 is ready".into()),
        Item::Menu => {
            h.comp.set_menu_rows(rows());
            h.comp.set_menu_x(at.0);
            h.comp.set_menu_y(at.1);
            h.comp.set_menu_open(true);
        }
        Item::Dialog => {
            h.comp.set_dialog_title("Close the tab?".into());
            h.comp.set_dialog_primary("Close tab".into());
            h.comp.set_dialog_open(true);
        }
        Item::Badge => h.comp.set_copied_phase(1),
        Item::Tip => {
            h.comp.set_tip_text("A tip over a badge".into());
            h.comp.set_tip_x(at.0);
            h.comp.set_tip_y(at.1);
            h.comp.set_tip_visible(true);
        }
    }
    h.advance(400);
}

/// Renders `upper` over `lower` and asserts the upper one wins wherever both draw and the two
/// differ: those pixels of the pair must equal the upper alone. A movable item is put 4 css px
/// inside the other one's top left corner.
fn upper_wins(scale: f32, h: u32, lower: Item, upper: Item, name: &str) {
    let blank = window(scale, h).render();
    // The fixed item's footprint decides where the movable one goes.
    let fixed = if matches!(upper, Item::Menu | Item::Tip) {
        lower
    } else {
        upper
    };
    let probe = window(scale, h);
    show(&probe, fixed, (0.0, 0.0));
    let (fx0, fy0, _, _) = bounds(&probe, fixed, &probe.render(), &blank)
        .unwrap_or_else(|| panic!("{name}: the fixed item draws nothing"));
    let at = (fx0 as f32 / scale + 4.0, fy0 as f32 / scale + 4.0);
    let up_h = window(scale, h);
    show(&up_h, upper, at);
    let up = up_h.render();
    let (ux0, uy0, ux1, uy1) =
        bounds(&up_h, upper, &up, &blank).unwrap_or_else(|| panic!("{name}: upper draws nothing"));
    let lo_h = window(scale, h);
    show(&lo_h, lower, at);
    let lo = lo_h.render();
    let both_h = window(scale, h);
    show(&both_h, lower, at);
    show(&both_h, upper, at);
    let both = both_h.render();
    // The Copied badge is a see-through pill: over a dialog it blends, so it only has to change what is under it.
    let translucent = matches!(upper, Item::Badge);
    let mut overlap = 0;
    for y in uy0..=uy1 {
        for x in ux0..=ux1 {
            let (l, u, b) = (lo.px(x, y), up.px(x, y), both.px(x, y));
            let lower_draws = !Frame::near(l, blank.px(x, y), 1);
            let upper_draws = !Frame::near(u, blank.px(x, y), 1);
            if lower_draws && upper_draws && !Frame::near(l, u, 3) {
                overlap += 1;
                assert!(
                    Frame::near(b, u, 3) || (translucent && !Frame::near(b, l, 0)),
                    "scale {scale}, {name}: pixel ({x}, {y}) is {b:?}, the lower shows {l:?}, the upper {u:?}"
                );
            }
        }
    }
    assert!(
        overlap >= 40,
        "scale {scale}, {name}: only {overlap} pixels overlap"
    );
}

#[test]
fn two_overlays_over_each_other_show_the_upper_one_at_every_scale() {
    for s in SCALES {
        // pop under menu: the update notice, then a context menu over it
        upper_wins(s, 260, Item::Notice, Item::Menu, "notice under menu");
        // menu under modal: the dialog box over a context menu
        upper_wins(s, 260, Item::Menu, Item::Dialog, "menu under dialog");
        // modal under toast: the Copied badge over the dialog's box
        upper_wins(s, 200, Item::Dialog, Item::Badge, "dialog under badge");
        // toast under tip: a tooltip over the Copied badge
        upper_wins(s, 260, Item::Badge, Item::Tip, "badge under tip");
    }
}
