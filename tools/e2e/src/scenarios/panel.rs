//! Panels (Surfaces stage 1): `panelImage`.
//!
//! Prints the paths of a picture and a text file, left-clicks the picture, and checks what the
//! window shows through `panel.state` and `ui.rect` (the slot, the splitter, the body, the
//! picture), against the layout maths the probe also reports, and against the painted pixels:
//! the picture's colour in the middle of its box, the ground beside it. Then the 1:1 zoom, the
//! toggle chord, the text file, the splitter, an overlay, the close button, and that the terminal
//! refits and still answers. At 100 and 225 percent (the window is 1000 by 640 css px at both).
//!
//! Nothing here touches the user's windows: the app is the built exe, offscreen, in its own world.

use std::fs;
use std::path::Path;

use flare_e2e_proto::{Button, Request};
use serde_json::{json, Value};

use super::input_b::{
    click_on, device_pixel, gap, opened, path_texts, probe, prompt, rows, same_path, type_line,
};
use crate::ctx::{App, Ctx, Launch};
use crate::pixels::Rgb;

/// The picture's colour: nothing else on the screen is this.
const PICTURE_RGB: Rgb = [200, 40, 60];
const PICTURE_W: u32 = 1600;
const PICTURE_H: u32 = 800;
const NOTE: &str = "first line\nsecond line\nthird line\n";

pub(super) fn write_picture(path: &Path) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| format!("make the picture: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), PICTURE_W, PICTURE_H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let px: Vec<u8> = (0..PICTURE_W * PICTURE_H)
        .flat_map(|_| [PICTURE_RGB[0], PICTURE_RGB[1], PICTURE_RGB[2], 255])
        .collect();
    w.write_image_data(&px).map_err(|e| e.to_string())
}

/// The `panel.state` answer.
pub(super) fn state(app: &mut App) -> Value {
    probe(app, "panel.state", Value::Null)
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

/// A `ui.rect` in device pixels: x, y, w, h.
pub(super) fn dev_rect(app: &mut App, name: &str) -> Option<(i64, i64, i64, i64)> {
    let v = probe(app, "ui.rect", json!({ "name": name }));
    Some((
        v["dx"].as_i64()?,
        v["dy"].as_i64()?,
        v["dw"].as_i64()?,
        v["dh"].as_i64()?,
    ))
}

/// The front grid's columns and device width.
fn grid(app: &mut App) -> (u64, i64) {
    let d = probe(app, "dpi.state", Value::Null);
    (
        d["grid_cols"].as_u64().unwrap_or(0),
        d["grid_w_px"]
            .as_i64()
            .or_else(|| state(app)["grid_phys_w"].as_i64())
            .unwrap_or(0),
    )
}

pub fn panel_image(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    write_picture(&alpha.join("panel-pic.png"))?;
    fs::write(alpha.join("panel-note.txt"), NOTE).map_err(|e| format!("make the note: {e}"))?;
    for (dpi, label) in [(96u32, "100%"), (216, "225%")] {
        let mut app = ctx.launch(Launch {
            renderer: Some("gpu"),
            folders: vec![alpha.clone()],
            scale_dpi: Some(dpi),
            ..Launch::default()
        })?;
        let r = panel_on(ctx, &mut app, dpi, label);
        app.close();
        r?;
    }
    Ok(())
}

fn panel_on(ctx: &mut Ctx, app: &mut App, dpi: u32, at: &str) -> Result<(), String> {
    let scale = f64::from(dpi) / 96.0;
    // The window is 1000 by 640 css px at every scale.
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    let sized = ctx.until("the window to be 1000 css px wide", 5_000, &mut || {
        probe(app, "dpi.state", Value::Null)["client_w_px"].as_f64()
            == Some((1000.0 * scale).round())
    });
    ctx.ok(sized, format!("{at}: the window is 1000 css px wide"));
    prompt(app)?;
    type_line(
        app,
        "cls; Write-Host 'picture: panel-pic.png note: panel-note.txt'",
    )?;
    let painted = ctx.until("both paths to wear the link colour", 8_000, &mut || {
        let t = path_texts(app);
        ["panel-pic.png", "panel-note.txt"]
            .iter()
            .all(|w| t.iter().any(|p| p.contains(w)))
    });
    ctx.ok(painted, format!("{at}: the picture and the note are links"));

    // ---- before: no panel, the whole page is the terminal ----
    let s0 = state(app);
    ctx.ok(
        s0["shown"] == json!(false)
            && s0["open"] == json!(false)
            && s0["overlay_open"] == json!(false),
        format!("{at}: no panel is open before a click"),
    );
    let (cols0, _) = grid(app);
    let grid_w0 = s0["grid_phys_w"].as_i64().unwrap_or(0);
    let before_opens = opened(app).len();

    // ---- the click ----
    click_on(app, "panel-pic.png", 2, Button::Left)?;
    let shown = ctx.until("the panel to show the picture", 10_000, &mut || {
        let s = state(app);
        s["shown"] == json!(true) && s["kind"] == json!(1)
    });
    ctx.ok(
        shown,
        format!("{at}: a click on a picture path opens it in the panel"),
    );
    let s = state(app);
    ctx.ok(
        s["title"] == json!("panel-pic.png")
            && s["sub"] == json!(format!("{PICTURE_W} x {PICTURE_H}"))
            && s["picture"]["w"] == json!(PICTURE_W)
            && s["picture"]["h"] == json!(PICTURE_H),
        format!(
            "{at}: the header names the file and its size ({} / {})",
            s["title"], s["sub"]
        ),
    );
    let calls = opened(app);
    let png_s = ctx
        .ensure_world()
        .alpha
        .join("panel-pic.png")
        .to_string_lossy()
        .into_owned();
    ctx.ok(
        calls.len() == before_opens && !calls.iter().any(|(_, p)| same_path(p, &png_s)),
        format!("{at}: nothing was handed to Windows"),
    );
    ctx.ok(
        s["has_focus"] == json!(false),
        format!("{at}: the click left the keys in the terminal"),
    );

    // ---- where it is, against the maths ----
    let lay = &s["layout"];
    let slot_phys = s["slot_phys_w"].as_i64().unwrap_or(-1);
    let want_slot = lay["slot_w"].as_i64().unwrap_or(-2);
    let want_grid = lay["grid_w"].as_i64().unwrap_or(-2);
    let grid_phys = s["grid_phys_w"].as_i64().unwrap_or(-1);
    ctx.ok(
        lay["shown"] == json!(true)
            && (slot_phys - want_slot).abs() <= 1
            && (grid_phys - want_grid).abs() <= 1,
        format!(
            "{at}: the slot is {slot_phys} px and the grid {grid_phys} px; the maths says {want_slot} and {want_grid}"
        ),
    );
    let client_w = (1000.0 * scale).round() as i64;
    let panel = dev_rect(app, "panel");
    let gridr = dev_rect(app, "grid");
    ctx.ok(
        panel.is_some_and(|p| (p.0 + p.2 - client_w).abs() <= 1)
            && gridr.is_some_and(|g| panel.is_some_and(|p| g.0 + g.2 <= p.0 + 1)),
        format!("{at}: the slot ends at the window's right edge, the grid ends where it starts ({panel:?} {gridr:?})"),
    );
    let (cols1, _) = grid(app);
    ctx.ok(
        cols1 < cols0,
        format!("{at}: the terminal refit to fewer columns ({cols0} to {cols1})"),
    );
    ctx.ok(
        grid_phys < grid_w0,
        format!("{at}: the grid is narrower ({grid_w0} to {grid_phys})"),
    );

    // ---- the picture, drawn ----
    let body = dev_rect(app, "panel-body");
    let pic = dev_rect(app, "panel-image");
    let inside = match (body, pic) {
        (Some(b), Some(p)) => {
            p.0 >= b.0 - 1
                && p.1 >= b.1 - 1
                && p.0 + p.2 <= b.0 + b.2 + 1
                && p.1 + p.3 <= b.1 + b.3 + 1
        }
        _ => false,
    };
    let ratio = pic.map_or(0.0, |p| p.2 as f64 / p.3.max(1) as f64);
    ctx.ok(
        inside && (ratio - 2.0).abs() < 0.04,
        format!(
            "{at}: the picture is fitted inside the body and keeps its shape ({pic:?} in {body:?})"
        ),
    );
    ctx.ok(
        s["can_zoom"] == json!(true) && s["zoomed"] == json!(false),
        format!("{at}: it is fitted, not at full size"),
    );
    let shot = ctx.shot(app, &format!("panel-image-{dpi}"));
    ctx.ok(
        shot.is_some(),
        format!("{at}: a picture of the window was taken"),
    );
    if let Some(p) = pic {
        let mid = device_pixel(app, (p.0 + p.2 / 2) as f64, (p.1 + p.3 / 2) as f64);
        ctx.ok(
            mid.is_some_and(|m| gap(m, PICTURE_RGB) <= 12),
            format!("{at}: the middle of the picture is its colour ({mid:?})"),
        );
        let below = device_pixel(app, (p.0 + p.2 / 2) as f64, (p.1 + p.3 + 6) as f64);
        ctx.ok(
            below.is_some_and(|m| gap(m, PICTURE_RGB) > 60),
            format!("{at}: below the picture is the panel's ground ({below:?})"),
        );
    }

    // ---- the terminal still answers ----
    // The answer, not the typed line: the typed line may wrap in the narrower grid and split the word.
    type_line(app, "Write-Host ('panel-' + 'alive')")?;
    let alive = ctx.until("the shell to answer beside the panel", 8_000, &mut || {
        rows(app).contains("panel-alive")
    });
    ctx.ok(
        alive,
        format!("{at}: the terminal answers beside the panel"),
    );

    // ---- one pixel to one pixel ----
    if let Some(p) = pic {
        let (cx, cy) = (
            (p.0 + p.2 / 2) as f64 / scale,
            (p.1 + p.3 / 2) as f64 / scale,
        );
        app.session.click(cx as f32, cy as f32)?;
        let zoomed = ctx.until("the picture to zoom", 4_000, &mut || {
            state(app)["zoomed"] == json!(true)
        });
        let z = dev_rect(app, "panel-image");
        ctx.ok(
            zoomed && z.is_some_and(|z| (z.2 - i64::from(PICTURE_W)).abs() <= 1),
            format!("{at}: a click shows it at one image pixel to one device pixel ({z:?})"),
        );
        if let Some(b) = dev_rect(app, "panel-body") {
            app.session.click(
                ((b.0 + b.2 / 2) as f64 / scale) as f32,
                ((b.1 + b.3 / 2) as f64 / scale) as f32,
            )?;
        }
        let back = ctx.until("the picture to fit again", 4_000, &mut || {
            state(app)["zoomed"] == json!(false)
        });
        ctx.ok(back, format!("{at}: a second click fits it again"));
    }

    // ---- the toggle chord ----
    app.session.key("Ctrl+Shift+P")?;
    let closed = ctx.until("the panel to close", 4_000, &mut || {
        state(app)["shown"] == json!(false)
    });
    ctx.ok(closed, format!("{at}: Ctrl+Shift+P closes the panel"));
    let refit = ctx.until("the grid to take the page back", 4_000, &mut || {
        (state(app)["grid_phys_w"].as_i64().unwrap_or(0) - grid_w0).abs() <= 1
    });
    ctx.ok(
        refit,
        format!("{at}: the terminal has the whole page again"),
    );
    app.session.key("Ctrl+Shift+P")?;
    let back = ctx.until("the panel to come back", 6_000, &mut || {
        let s = state(app);
        s["shown"] == json!(true) && s["kind"] == json!(1)
    });
    ctx.ok(
        back,
        format!("{at}: Ctrl+Shift+P brings back what it showed"),
    );

    // ---- the keys move with Ctrl+Shift+O ----
    app.session.key("Ctrl+Shift+O")?;
    let focused = ctx.until("the panel to take the keys", 4_000, &mut || {
        state(app)["has_focus"] == json!(true)
    });
    ctx.ok(
        focused,
        format!("{at}: Ctrl+Shift+O gives the panel the keys"),
    );
    app.session.key("Escape")?;
    let returned = ctx.until("the keys to return", 4_000, &mut || {
        state(app)["has_focus"] == json!(false)
    });
    ctx.ok(
        returned,
        format!("{at}: Escape gives them back to the terminal"),
    );

    // ---- the text file ----
    click_on(app, "panel-note.txt", 2, Button::Left)?;
    let text = ctx.until("the panel to show the note", 10_000, &mut || {
        let s = state(app);
        s["kind"] == json!(2) && s["lines"] == json!(3)
    });
    let s = state(app);
    ctx.ok(
        text && s["title"] == json!("panel-note.txt")
            && s["sub"] == json!(format!("3 lines, {} B", NOTE.len())),
        format!(
            "{at}: a click on a text path shows it as lines ({} / {})",
            s["title"], s["sub"]
        ),
    );
    ctx.ok(
        num(&s["gutter_w"]) > 0.0 && s["picture"]["w"] == json!(0),
        format!("{at}: the picture is let go while a text file shows"),
    );
    let calls = opened(app);
    ctx.ok(
        calls.len() == before_opens,
        format!("{at}: the note was not handed to Windows either"),
    );

    // ---- the splitter ----
    if let Some(sp) = dev_rect(app, "splitter") {
        let w0 = state(app)["slot_phys_w"].as_i64().unwrap_or(0);
        let from = (
            (sp.0 + sp.2 / 2) as f64 / scale,
            (sp.1 + 200) as f64 / scale,
        );
        let to = (from.0 - 100.0, from.1);
        app.session.call(&Request::Drag {
            from: (from.0 as f32, from.1 as f32),
            to: (to.0 as f32, to.1 as f32),
            steps: 8,
            button: Button::Left,
        })?;
        let wider = ctx.until("the slot to widen", 4_000, &mut || {
            state(app)["slot_phys_w"].as_i64().unwrap_or(0) > w0 + (90.0 * scale) as i64
        });
        let s = state(app);
        let grew = s["slot_phys_w"].as_i64().unwrap_or(0) - w0;
        ctx.ok(
            wider && (grew - (100.0 * scale).round() as i64).abs() <= 3,
            format!("{at}: dragging the splitter 100 css px left widens the slot by {grew} px"),
        );
        let lay = &s["layout"];
        // The stored share lays out as the slot is drawn.
        let want = (s["page_phys_w"].as_f64().unwrap_or(0.0) * num(&s["share"])).round() as i64;
        ctx.ok(
            num(&s["share"]) > 0.0
                && (s["slot_phys_w"].as_i64().unwrap_or(0) - want).abs() <= 2
                && (lay["slot_w"].as_i64().unwrap_or(-9) - s["slot_phys_w"].as_i64().unwrap_or(0))
                    .abs()
                    <= 1,
            format!("{at}: the new width is the tab's share ({})", s["share"]),
        );
    } else {
        ctx.ok(false, format!("{at}: the splitter has a rectangle"));
    }

    // ---- an overlay ----
    if let Some(g) = dev_rect(app, "grid") {
        let (x, y) = (
            (g.0 + 60) as f32 / scale as f32,
            (g.1 + 60) as f32 / scale as f32,
        );
        app.session.right_click(x, y)?;
        let open = ctx.until("the terminal menu", 4_000, &mut || {
            state(app)["overlay_open"] == json!(true)
        });
        ctx.ok(
            open,
            format!("{at}: a menu over the terminal turns the overlay flag on"),
        );
        app.session.key("Escape")?;
        let shut = ctx.until("the menu to close", 4_000, &mut || {
            state(app)["overlay_open"] == json!(false)
        });
        ctx.ok(shut, format!("{at}: and closing it turns the flag off"));
    }

    // ---- the close button ----
    if let Some(c) = dev_rect(app, "panel-close") {
        app.session.click(
            ((c.0 + c.2 / 2) as f64 / scale) as f32,
            ((c.1 + c.3 / 2) as f64 / scale) as f32,
        )?;
        let gone = ctx.until("the panel to close", 4_000, &mut || {
            state(app)["shown"] == json!(false)
        });
        ctx.ok(gone, format!("{at}: the close button closes the panel"));
        let (cols2, _) = grid(app);
        ctx.ok(
            cols2 == cols0,
            format!("{at}: and the columns are back ({cols2} of {cols0})"),
        );
    }
    Ok(())
}
