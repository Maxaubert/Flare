//! The resize-edges probe (`resizeEdges` scenario): sends real `WM_NCHITTEST` messages to the main
//! window at the points the scenario names, through the whole window procedure chain.

use flare_win::resize_edges::{report, send_hit_test};
use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("window.edges", window_edges);
}

/// `window.edges` with `{"points": [[dx, dy], ...]}` (device px from the window's top-left): the
/// rectangle, DPI, band, `WS_SIZEBOX`, minimum track size, and one `HT*` code per point.
fn window_edges(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let hwnd = crate::window::hwnd_of(ctx.app).ok_or("the window has no handle yet")?;
    let rep = report(hwnd);
    let codes: Vec<u32> = arg["points"]
        .as_array()
        .map(|pts| {
            pts.iter()
                .map(|p| {
                    let dx = p[0].as_i64().unwrap_or(0) as i32;
                    let dy = p[1].as_i64().unwrap_or(0) as i32;
                    send_hit_test(hwnd, rep.rect[0] + dx, rep.rect[1] + dy)
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "rect": rep.rect,
        "dpi": rep.dpi,
        "band": [rep.band.0, rep.band.1],
        "corner": rep.corner,
        "sizebox": rep.sizebox,
        "min_track": [rep.min_track.0, rep.min_track.1],
        "codes": codes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_edges_is_registered() {
        let mut r = ProbeRegistry::new();
        register(&mut r);
        assert_eq!(r.names(), vec!["window.edges"]);
    }
}
