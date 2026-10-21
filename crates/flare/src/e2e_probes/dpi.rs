//! The DPI matrix and monitor moves probes (Task 72): `render.size` and `dpi.hairlines`. `dpi.state`
//! (the cell, the padding, the client size) is a core probe; these two add what `dpiMatrix` and
//! `monitorMove` need to prove a scaling change reached the drawing and not only the numbers.

use serde_json::{Value, json};
use slint::ComponentHandle;

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("render.size", render_size);
    r.add("dpi.hairlines", dpi_hairlines);
}

/// `render.size`: the surface the window draws on, the grid area the layout gave the terminal and
/// the texture the grid host last handed the window, all in device pixels, and which painter drew
/// it (`gpu`, `software`, or `software (fallback: <why>)`, as `Hello` words it).
fn render_size(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let size = ctx.app.window().size();
    let tex = ctx.app.get_grid_image().size();
    let (told, _) = crate::e2e_hook::hello_info();
    let painter = if told.is_empty() {
        "unknown".to_string()
    } else {
        told
    };
    Ok(json!({
        "surface_w": size.width,
        "surface_h": size.height,
        "grid_w": ctx.app.get_grid_phys_width().max(0),
        "grid_h": ctx.app.get_grid_phys_height().max(0),
        "grid_tex_w": tex.width,
        "grid_tex_h": tex.height,
        "painter": painter,
    }))
}

/// `dpi.hairlines`: the width in device pixels of the strip's bottom line, a tab separator and the
/// sidebar's edge, as the window computes them from `Px.hair()`.
fn dpi_hairlines(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let hair = ctx.app.get_hair_phys().max(0);
    Ok(json!({
        "strip_bottom_px": hair,
        "tab_separator_px": hair,
        "sidebar_edge_px": hair,
    }))
}
