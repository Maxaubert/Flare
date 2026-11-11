//! settingsLive (Flare's own, audit 2026-10-21): a setting takes effect on the main window while
//! Settings stays open, as every row does in Prism Terminal. The owner's bug: "if i go from
//! vertical to horizontal tabs it doesnt change before i click out of settings".
//!
//! Settings is opened from the cog and stays in front for the whole check (`settings.state` front
//! is read after every change). Tab position is picked by the pointer on its pill (the driver's
//! `pick_seg`), both ways, and the main window's layout is read from `sidebar.state` and `ui.rect`
//! (the sidebar's box, the strip's box). Panel edges is picked the same way and the main window's
//! edge tokens are read from `theme.tokens` (the colours the window paints its lines with). The run
//! starts at 100 percent, then the window moves to a 225 percent monitor and does it all again.

use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::driver::{goto_row, open_settings, pick_seg, row_is, settings_front};
use crate::ctx::{App, Ctx, Launch};

/// 225 percent (96 is 100 percent).
const DPI_225: u32 = 216;

fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

fn vertical(app: &mut App) -> bool {
    probe(app, "sidebar.state", Value::Null)["vertical"] == json!(true)
}

/// Whether the window has a box named `name` (`ui.rect` answers one only where it is drawn).
fn has_rect(app: &mut App, name: &str) -> bool {
    probe(app, "ui.rect", json!({ "name": name }))["w"]
        .as_f64()
        .is_some_and(|w| w > 0.0)
}

fn line_token(app: &mut App) -> String {
    probe(app, "theme.tokens", Value::Null)["line"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn alpha_of(s: &str) -> Option<u8> {
    let h = s.trim_start_matches('#');
    if h.len() >= 8 {
        u8::from_str_radix(&h[6..8], 16).ok()
    } else if h.len() == 6 {
        Some(255)
    } else {
        None
    }
}

fn scale(app: &mut App) -> f64 {
    probe(app, "app.state", Value::Null)["scale"]
        .as_f64()
        .unwrap_or(1.0)
}

/// The checks at one scale: the tabs move both ways and the edges change, Settings in front. The
/// first round starts on a fresh profile, so down the side (the default since 2026-11-11); each round
/// ends down the side again.
fn round(ctx: &mut Ctx, app: &mut App, at: &str) {
    let opened = open_settings(ctx, app);
    ctx.ok(opened, format!("{at}: Settings is in front"));
    ctx.ok(
        vertical(app) && has_rect(app, "sidebar") && !has_rect(app, "strip"),
        format!("{at}: the tabs start down the side"),
    );

    // TAB POSITION: along the top, then down the side again, Settings in front throughout.
    let picked = pick_seg(ctx, app, "tab-layout", &["horizontal"]);
    ctx.ok(
        picked,
        format!("{at}: Horizontal is picked on the Tab position pill"),
    );
    let top = ctx.until(&format!("{at}: the strip"), 4_000, &mut || {
        !vertical(app) && has_rect(app, "strip") && !has_rect(app, "sidebar")
    });
    let front = settings_front(app);
    ctx.ok(
        top && front,
        format!("{at}: the tabs went along the top while Settings is still open (front {front})"),
    );
    let picked = pick_seg(ctx, app, "tab-layout", &["vertical"]);
    ctx.ok(picked, format!("{at}: Vertical is picked again"));
    let side = ctx.until(&format!("{at}: the sidebar"), 4_000, &mut || {
        vertical(app) && has_rect(app, "sidebar") && !has_rect(app, "strip")
    });
    let front = settings_front(app);
    ctx.ok(
        side && front,
        format!(
            "{at}: the tabs came back down the side while Settings is still open (front {front})"
        ),
    );

    // PANEL EDGES: the window's line token follows the pill at once.
    let before = line_token(app);
    let picked = pick_seg(ctx, app, "window-edges", &["none"]);
    ctx.ok(
        picked,
        format!("{at}: None is picked on the Panel edges pill"),
    );
    let none = ctx.until(&format!("{at}: no edges"), 4_000, &mut || {
        alpha_of(&line_token(app)) == Some(0)
    });
    let front = settings_front(app);
    ctx.ok(
        none && front && line_token(app) != before,
        format!(
            "{at}: the window's lines went ({before} to {}) while Settings is still open (front {front})",
            line_token(app)
        ),
    );
    let picked = pick_seg(ctx, app, "window-edges", &["solid"]);
    ctx.ok(picked, format!("{at}: Solid is picked"));
    let solid = ctx.until(&format!("{at}: solid edges"), 4_000, &mut || {
        alpha_of(&line_token(app)).is_some_and(|a| a > 0x17)
    });
    let front = settings_front(app);
    ctx.ok(
        solid && front,
        format!(
            "{at}: the window's lines are solid ({}) while Settings is still open (front {front})",
            line_token(app)
        ),
    );
    // Back to the default, so the next round starts from the same window.
    pick_seg(ctx, app, "window-edges", &["hairline"]);
}

pub fn settings_live(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let ready = ctx.until("the first tab", 15_000, &mut || {
        probe(&mut app, "app.state", Value::Null)["tabs"]
            .as_array()
            .is_some_and(|t| !t.is_empty())
    });
    ctx.ok(ready, "a tab is open");
    round(ctx, &mut app, "100%");

    // A move to a 225 percent monitor, Settings still open, and the same again.
    let moved = app
        .session
        .call(&Request::DpiChange {
            dpi: DPI_225,
            x: None,
            y: None,
        })
        .is_ok()
        && ctx.until("225 percent", 6_000, &mut || scale(&mut app) > 2.2);
    ctx.ok(moved, "the window moved to a 225 percent monitor");
    round(ctx, &mut app, "225%");
    app.close();
    Ok(())
}

/// verticalStart (Flare's own, owner 2026-10-22: "there was a startup bug where it was on vertical
/// tabs but it was showing the horizontal style"): a profile whose settings say vertical opens with
/// the tabs down the side and no strip, at 100 and at 225 percent, on the first tab.
pub fn vertical_start(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, profile) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.profile.clone())
    };
    let _ = std::fs::create_dir_all(&profile);
    std::fs::write(
        profile.join("settings.json"),
        r#"{"flare.window.tabLayout":"vertical","flare.window.sidebar":"open","flare.window.sidebarWidth":"265","flare.window.edges":"faint","flare.term.fontPct":"130"}"#,
    )
    .map_err(|e| e.to_string())?;
    let saved = alpha.to_string_lossy().replace('\\', "\\\\");
    std::fs::write(
        profile.join("tabs.json"),
        format!(r#"{{"tabs":[{{"cwd":"{saved}"}}],"active":0}}"#),
    )
    .map_err(|e| e.to_string())?;
    let mut app = ctx.launch(Launch {
        folders: vec![],
        renderer: Some("gpu"),
        scale_dpi: Some(DPI_225),
        ..Launch::default()
    })?;
    let ready = ctx.until("the first tab", 15_000, &mut || {
        probe(&mut app, "app.state", Value::Null)["tabs"]
            .as_array()
            .is_some_and(|t| !t.is_empty())
    });
    ctx.ok(ready, "a tab is open");
    for at in ["100%", "225%"] {
        if at == "225%" {
            let moved = app
                .session
                .call(&Request::DpiChange {
                    dpi: DPI_225,
                    x: None,
                    y: None,
                })
                .is_ok()
                && ctx.until("225 percent", 6_000, &mut || scale(&mut app) > 2.2);
            ctx.ok(moved, "the window moved to a 225 percent monitor");
        }
        ctx.ok(
            vertical(&mut app),
            format!("{at}: the stored vertical layout is the one in force"),
        );
        ctx.ok(
            has_rect(&mut app, "sidebar") && !has_rect(&mut app, "strip"),
            format!(
                "{at}: the tabs are drawn down the side, with no strip, without touching Settings"
            ),
        );
    }
    app.close();
    fresh_profile_starts_vertical(ctx)
}

/// The second half of verticalStart (Max, 2026-11-11: vertical tabs by default): a profile with no
/// settings at all opens with the tabs down the side, the sidebar open at 220 px, and stores nothing
/// for the layout until somebody chooses.
fn fresh_profile_starts_vertical(ctx: &mut Ctx) -> Result<(), String> {
    let w = crate::world::World::new();
    let mut app = ctx.launch_in(
        &w,
        Launch {
            folders: vec![w.alpha.clone()],
            renderer: Some("software"),
            ..Launch::default()
        },
    )?;
    let ready = ctx.until("fresh: the first tab", 15_000, &mut || {
        probe(&mut app, "app.state", Value::Null)["tabs"]
            .as_array()
            .is_some_and(|t| !t.is_empty())
    });
    ctx.ok(ready, "fresh: a tab is open");
    let side = probe(&mut app, "sidebar.state", Value::Null);
    ctx.ok(
        vertical(&mut app) && side["open"] == json!(true),
        format!("fresh: the tabs are down the side and the sidebar is open ({side})"),
    );
    ctx.ok(
        has_rect(&mut app, "sidebar") && !has_rect(&mut app, "strip"),
        "fresh: the sidebar is drawn and there is no strip",
    );
    let width = probe(&mut app, "ui.rect", json!({ "name": "sidebar" }))["w"]
        .as_f64()
        .unwrap_or(0.0);
    ctx.ok(
        (width - 220.0).abs() < 1.0,
        format!("fresh: the sidebar is 220 px wide ({width})"),
    );
    let on_row = goto_row(ctx, &mut app, "tab-layout");
    let value = probe(&mut app, "settings.row", json!({ "id": "tab-layout" }))["value"]
        .as_str()
        .unwrap_or("")
        .to_string();
    ctx.ok(
        on_row && row_is(&mut app, "tab-layout", &["vertical"]),
        format!("fresh: Settings shows Vertical as the Tab position ({value})"),
    );
    app.close();
    let stored = std::fs::read_to_string(w.profile.join("settings.json")).unwrap_or_default();
    let layout = serde_json::from_str::<Value>(&stored)
        .ok()
        .and_then(|v| v.get("flare.window.tabLayout").cloned());
    ctx.ok(
        layout.is_none(),
        format!("fresh: nothing is stored for the layout until somebody chooses ({layout:?})"),
    );
    Ok(())
}
