//! Input B: selection and links scenarios (Task 30): selectionEdit, markerLinks (pathLinks until 2026-11-11), links,
//! osc8Links.
//!
//! PT: run.mjs:1399-1586 (selectionEdit), 3545-3640 (pathLinks), 4215-4337 (links), 4339-4420
//! (osc8Links). Flare answers PT's page queries through the probes `term.selection` and `term.links`
//! (the link marks the decoration paints, with their ink), `url.log` (the web addresses the opener
//! was asked for), `open.log` (the paths, folders and reveals), `clip.last`, `app.state` (the menu,
//! the Copied badge, the dialog) and `grid.text`. Character boxes come from `ui.rect` and
//! `dpi.state`, as `input_a` computes them. Flare's clipboard is in memory under `--e2e`, so there
//! is nothing to save and restore at the end.
//!
//! DIFF-6: a `.bat` is REVEALED in Explorer, never opened (the allowlist of O22), so PT's "shown
//! selected in Explorer" is checked as a reveal and "never run" as no open. PT's `sleep`s between
//! steps are `until` waits on the state the step produces, except the short "nothing happened"
//! holds, which are `settle` calls (at most 50 ms each).

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use flare_e2e_proto::{Button, PointerAction, Reply, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::pixels::{contrast, Rgb};
use crate::world::{prompt_timeout, rows_text, wait_for_prompt, Shell};

/// The link ink on the default theme (PT's LINK_BLUE, `#79a7d8`, as the decoration paints it).
const LINK_BLUE: &str = "#79a7d8";

/// The web address the osc8Links scenario labels (PT's `URL8`).
const URL8: &str = "https://example.com/osc8";

/// The program that holds the alternate screen until one key is pressed (PT's node probe): it
/// draws an OSC 8 label on the alternate screen, waits for a key, then leaves the alternate screen.
const ALT_SCRIPT: &str = "$e = [string][char]27\r\n\
$b = [string][char]7\r\n\
[Console]::Write($e + '[?1049h' + $e + '[2J' + $e + '[5;3H' + $e + ']8;;https://example.com/osc8' + $b + 'ALTLABEL' + $e + ']8;;' + $b + $e + '[9;1H')\r\n\
[void][Console]::ReadKey($true)\r\n\
[Console]::Write($e + '[?1049l')\r\n";

/// The renderers a run covers, from `FLARE_E2E_RENDERERS` (a list of `gpu` and `software`) when it
/// is set. Else GPU, and GPU then software for a scenario marked both (the gate runs both).
fn renderers(both: bool) -> Vec<&'static str> {
    pick_renderers(
        &std::env::var("FLARE_E2E_RENDERERS").unwrap_or_default(),
        both,
    )
}

/// The renderer choice for `asked` (the env list, or empty), see [`renderers`].
fn pick_renderers(asked: &str, both: bool) -> Vec<&'static str> {
    if asked.trim().is_empty() {
        return if both {
            vec!["gpu", "software"]
        } else {
            vec!["gpu"]
        };
    }
    ["gpu", "software"]
        .into_iter()
        .filter(|r| asked.split(',').any(|w| w.trim() == *r))
        .collect()
}

/// A probe's answer, `Null` when it fails (a check on it then reads false).
pub(super) fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The id of the tab in front ("" when the probe is not answered).
pub(super) fn front_tab(app: &mut App) -> String {
    app.session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default()
}

/// The text of the front tab's grid, one row per line.
pub(super) fn rows(app: &mut App) -> String {
    let tab = front_tab(app);
    rows_text(app, &tab)
}

/// The shell shows its PowerShell prompt again (PT `prompt()`). An Err ends the scenario, as PT's
/// timeout throw does.
pub(super) fn prompt(app: &mut App) -> Result<(), String> {
    let tab = front_tab(app);
    if wait_for_prompt(
        app,
        &tab,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    ) {
        Ok(())
    } else {
        Err("the PowerShell prompt did not come back".into())
    }
}

/// Types one line into the front shell and presses Enter (PT `typeLine`, after the prompt).
pub(super) fn type_line(app: &mut App, line: &str) -> Result<(), String> {
    app.session.type_text(line)?;
    app.session.key("Enter")
}

/// A `ui.rect` answer in css px: x, y, w, h.
pub(super) fn css(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// The css point of the character at `offset` after the LAST row holding `needle` (PT `box`),
/// `frac` of the way across its cell (0 is its left edge). Same geometry as `input_a`'s caret point.
fn cell_point(app: &mut App, needle: &str, offset: usize, frac: f64) -> Option<(f32, f32)> {
    let tab = front_tab(app);
    let grid = probe(app, "grid.text", json!({ "tab": tab }));
    let lines = grid["rows"].as_array()?;
    let row = lines
        .iter()
        .rposition(|r| r.as_str().is_some_and(|s| s.contains(needle)))?;
    // Columns count characters, not bytes: a row may hold a file-link mark (three bytes, one cell) before the needle.
    let text = lines[row].as_str()?;
    let col = text[..text.rfind(needle)?].chars().count() + offset;
    let d = probe(app, "dpi.state", Value::Null);
    let pad = d["padding"].as_f64()?;
    let cw = d["cell_w"].as_f64()?;
    let ch = d["cell_h"].as_f64()?;
    let k = d["scale"].as_f64()?;
    let (gx, gy, _, _) = css(&probe(app, "ui.rect", json!({ "name": "grid" })))?;
    let x = gx + (pad + (col as f64 + frac) * cw) / k;
    let y = gy + (pad + (row as f64 + 0.5) * ch) / k;
    Some((x as f32, y as f32))
}

/// Moves the pointer to a css point (no press).
pub(super) fn move_to(app: &mut App, x: f32, y: f32) -> Result<(), String> {
    app.session
        .call(&Request::Pointer {
            x,
            y,
            action: PointerAction::Move,
        })
        .map(|_| ())
}

/// A hand's click on a character: the pointer comes onto it, then the press (PT `clickOn`).
pub(super) fn click_on(
    app: &mut App,
    needle: &str,
    offset: usize,
    button: Button,
) -> Result<(), String> {
    let (x, y) =
        cell_point(app, needle, offset, 0.5).ok_or_else(|| format!("{needle} is not on screen"))?;
    move_to(app, x, y)?;
    match button {
        Button::Right => app.session.right_click(x, y),
        _ => app.session.click(x, y),
    }
}

/// The selection's text (`term.selection`).
fn sel_text(app: &mut App) -> String {
    probe(app, "term.selection", Value::Null)["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Drags from the left part of the character at `from` to the right part of the one at `to` (both on
/// the last row holding `needle`; `to` is included, as PT's `select`), then waits until the selection
/// reads exactly those characters. False when it did not take.
fn select(ctx: &mut Ctx, app: &mut App, needle: &str, from: usize, to: usize) -> bool {
    let (Some(a), Some(b)) = (
        cell_point(app, needle, from, 0.2),
        cell_point(app, needle, to, 0.8),
    ) else {
        return false;
    };
    let want = needle[from..=to].to_string();
    if app
        .session
        .call(&Request::Drag {
            from: a,
            to: b,
            steps: 6,
            button: Button::Left,
        })
        .is_err()
    {
        return false;
    }
    ctx.until("the selection", 2_000, &mut || sel_text(app) == want)
}

/// The link marks on the viewport (`term.links`): the marks and the ink the decoration paints them.
pub(super) fn marks(app: &mut App) -> (Vec<Value>, String) {
    let v = probe(app, "term.links", Value::Null);
    let links = v["links"].as_array().cloned().unwrap_or_default();
    (links, v["ink"].as_str().unwrap_or_default().to_string())
}

/// The web links on the viewport as (address, text, rows) (`term.links`, kind `web`).
fn web_marks(app: &mut App) -> Vec<(String, String, usize)> {
    marks(app)
        .0
        .iter()
        .filter(|m| m["kind"] == json!("web"))
        .map(|m| {
            (
                m["uri"].as_str().unwrap_or_default().to_string(),
                m["text"].as_str().unwrap_or_default().to_string(),
                m["pieces"].as_array().map_or(0, Vec::len),
            )
        })
        .collect()
}

/// True when an OSC 8 label reading `label` is a link mark on the decoration's link ink.
fn osc8_label_blue(app: &mut App, label: &str) -> bool {
    let (links, ink) = marks(app);
    ink == LINK_BLUE
        && links
            .iter()
            .any(|m| m["kind"] == json!("osc8") && m["text"] == json!(label))
}

/// The labels of the open right-click menu, empty while none is open.
fn menu_labels(app: &mut App) -> Vec<String> {
    probe(app, "app.state", Value::Null)["menu"]["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["label"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The icons of the open menu's rows (an empty string where a row has none).
fn menu_icons(app: &mut App) -> Vec<String> {
    probe(app, "app.state", Value::Null)["menu"]["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["icon"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The labels of the right-click menu once it is open (waits up to 4 s; empty when none opens).
fn menu_rows(ctx: &mut Ctx, app: &mut App) -> Vec<String> {
    let open = ctx.until("a right-click menu", 4_000, &mut || {
        !menu_labels(app).is_empty()
    });
    if open {
        menu_labels(app)
    } else {
        Vec::new()
    }
}

/// Clicks the menu row labelled `label`: the menu's box split into equal rows, as the menu draws
/// them. False when no such row is open.
fn click_menu_row(app: &mut App, label: &str) -> bool {
    let labels = menu_labels(app);
    let Some(idx) = labels.iter().position(|l| l == label) else {
        return false;
    };
    let Some((x, y, w, h)) = css(&probe(app, "ui.rect", json!({ "name": "menu" }))) else {
        return false;
    };
    let at_y = y + (idx as f64 + 0.5) * h / labels.len() as f64;
    app.session.click((x + w / 2.0) as f32, at_y as f32).is_ok()
}

/// How many times the opener was asked to open `url`.
fn url_count(app: &mut App, url: &str) -> usize {
    probe(app, "url.log", Value::Null)["urls"]
        .as_array()
        .map_or(0, |a| a.iter().filter(|u| u.as_str() == Some(url)).count())
}

/// How many web addresses the opener was asked to open, in all.
fn url_total(app: &mut App) -> usize {
    probe(app, "url.log", Value::Null)["urls"]
        .as_array()
        .map_or(0, Vec::len)
}

/// The clipboard's text (`clip.last`).
fn clip_text(app: &mut App) -> String {
    probe(app, "clip.last", Value::Null)["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `open.log` calls as (kind, path): `open`, `reveal` or `folder`.
pub(super) fn opened(app: &mut App) -> Vec<(String, String)> {
    probe(app, "open.log", Value::Null)["calls"]
        .as_array()
        .map(|calls| {
            calls
                .iter()
                .map(|c| {
                    (
                        c["kind"].as_str().unwrap_or_default().to_string(),
                        c["path"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// True when the Copied badge is on screen (`app.state` badge).
fn badge_shown(app: &mut App) -> bool {
    probe(app, "app.state", Value::Null)["badge"]["visible"] == json!(true)
}

/// Where the Copied badge sits: centred and under 60 css px from the bottom. The answer and what it
/// measured.
fn badge_position(app: &mut App) -> (bool, String) {
    let st = probe(app, "app.state", Value::Null);
    let k = st["scale"].as_f64().unwrap_or(1.0).max(0.1);
    let win_w = st["window"]["client_w"].as_f64().unwrap_or(0.0) / k;
    let win_h = st["window"]["client_h"].as_f64().unwrap_or(0.0) / k;
    let Some((x, y, w, h)) = css(&probe(app, "ui.rect", json!({ "name": "badge" }))) else {
        return (false, "the badge has no rectangle".into());
    };
    let cx = x + w / 2.0;
    let up = win_h - (y + h);
    (
        (cx - win_w / 2.0).abs() <= 2.0 && up < 60.0,
        format!("{cx:.0} of {win_w:.0}, {up:.0}px up"),
    )
}

/// True when `a` and `b` name the same Windows path (case, slashes and a trailing slash ignored).
pub(super) fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    norm(a) == norm(b)
}

/// True when `text` holds `echo <token>x` and, after white space, `<token>x`: the shell's own
/// Backspace took one character (PT's regex, read over rows joined with new lines).
fn echoed_twice(text: &str, token: &str) -> bool {
    let want = format!("echo {token}x");
    let Some(at) = text.find(&want) else {
        return false;
    };
    text[at + want.len()..]
        .trim_start()
        .starts_with(&format!("{token}x"))
}

/// The rgb of a `#rrggbb` (or `#rrggbbaa`) colour; black when it does not parse.
fn rgb_of(hex: &str) -> Rgb {
    let h = hex.trim_start_matches('#');
    let part = |i: usize| {
        h.get(i..i + 2)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .unwrap_or(0)
    };
    [part(0), part(2), part(4)]
}

/// Opens a fresh app on `alpha` with `renderer`, runs `body`, and quits it whatever `body` did.
fn with_app(
    ctx: &mut Ctx,
    renderer: &'static str,
    folder: &Path,
    args: Vec<String>,
    body: impl FnOnce(&mut Ctx, &mut App) -> Result<(), String>,
) -> Result<(), String> {
    let mut app = ctx.launch(Launch {
        renderer: Some(renderer),
        folders: vec![folder.to_path_buf()],
        args,
        ..Launch::default()
    })?;
    let result = body(ctx, &mut app);
    app.close();
    result
}

/// PT: run.mjs:1399-1586. Backspace over a selection on the edited line, the link menu (Copy link,
/// Open link, icons, the recorded opens), the Copied badge, plain text, marked text and Ctrl+C.
/// Both renderers (PT marks it both).
pub fn selection_edit(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    for renderer in renderers(true) {
        with_app(ctx, renderer, &alpha, Vec::new(), selection_edit_on)?;
    }
    Ok(())
}

fn selection_edit_on(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    prompt(app)?;

    // Backspace over a selection at the END of the line deletes just it.
    app.session.type_text("echo hello world")?;
    let typed = ctx.until("the typed line", 4_000, &mut || {
        rows(app).contains("echo hello world")
    });
    let sel = typed && select(ctx, app, "hello world", 6, 10); // "world"
    app.session.key("Backspace")?;
    app.session.type_text("there")?;
    app.session.key("Enter")?;
    let edited = ctx.until("the edited line", 8_000, &mut || {
        rows(app).contains("echo hello there")
    });
    ctx.ok(
        sel && edited,
        "Backspace over a selected word deletes it, and typing goes on from there",
    );

    // And in the MIDDLE: the caret walks to the selection's end first.
    prompt(app)?;
    app.session.type_text("echo abcdef")?;
    let typed = ctx.until("the typed line", 4_000, &mut || {
        rows(app).contains("echo abcdef")
    });
    let sel = typed && select(ctx, app, "abcdef", 2, 3); // "cd"
    app.session.key("Backspace")?;
    app.session.key("Enter")?;
    let edited = ctx.until("the edited line", 8_000, &mut || {
        rows(app).contains("echo abef")
    });
    ctx.ok(
        sel && edited,
        "Backspace over a selection in the middle of the line deletes exactly it",
    );

    // Old output is not the line being edited: Backspace there is the shell's. A fresh token, so
    // no history prediction can dress the line up.
    prompt(app)?;
    let token = format!(
        "k{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
            % 100_000
    );
    app.session.type_text(&format!("echo {token}xy"))?;
    let typed = ctx.until("the typed line", 4_000, &mut || {
        rows(app).contains(&format!("echo {token}xy"))
    });
    let sel = typed && select(ctx, app, "abef", 0, 1); // "ab", in the old output
    app.session.key("Backspace")?;
    app.session.key("Enter")?;
    let shell_key = ctx.until("the shell's own Backspace", 8_000, &mut || {
        let t = rows(app);
        echoed_twice(&t, &token) && !t.contains(&format!("{token}xy"))
    });
    ctx.ok(
        sel && shell_key,
        "a selection in old output leaves Backspace to the shell: it deletes one character, as ever",
    );

    // A LINK: right-click on it offers Copy link, which copies all of it.
    prompt(app)?;
    let url = "https://example.com/some/path?q=1";
    type_line(app, &format!("echo {url}"))?;
    // The command and its output both show before the next prompt is read (the old prompt line
    // would otherwise answer at once).
    ctx.until("the echo output", 8_000, &mut || {
        rows(app).matches(url).count() >= 2
    });
    prompt(app)?;
    let (x, y) = cell_point(app, url, 12, 0.2).ok_or("the link is not on screen")?;
    app.session.right_click(x, y)?;
    let menu = menu_rows(ctx, app);
    // A RIGHT-CLICK ON A LINK OPENS NOTHING (owner, 2026-09-28): the link finder hands over any
    // button's click, and the menu is the answer.
    ctx.settle(50);
    ctx.settle(50);
    ctx.ok(
        url_count(app, url) == 0,
        "a right-click on a link does not open it",
    );
    ctx.ok(
        menu.first().is_some_and(|l| l == "Copy link") && !menu.iter().any(|l| l == "Close tab"),
        format!("right-click on a link: Copy link first, no Close tab ({menu:?})"),
    );
    ctx.ok(
        menu.get(1).is_some_and(|l| l == "Open link"),
        format!("and Open link beside it ({menu:?})"),
    );
    // A GLYPH ON EVERY ROW (owner, 2026-09-28).
    let icons = menu_icons(app);
    ctx.ok(
        !icons.is_empty() && icons.iter().all(|i| !i.is_empty()),
        format!("every row of the menu has its icon ({icons:?})"),
    );
    ctx.shot(app, "term-menu-link");
    let copied = click_menu_row(app, "Copy link")
        && ctx.until("the whole link on the clipboard", 4_000, &mut || {
            clip_text(app) == url
        });
    // The Copied badge of that copy: read now, judged where PT judges it (below), since it leaves
    // by itself after 1.2 s and the steps between take longer than that.
    let badge_seen = ctx.until("the Copied badge", 3_000, &mut || badge_shown(app));
    let badge_text_ok =
        badge_seen && probe(app, "app.state", Value::Null)["badge"]["text"] == json!("Copied");
    let (badge_pos_ok, badge_pos) = if badge_seen {
        badge_position(app)
    } else {
        (false, "no badge".into())
    };
    ctx.ok(copied, "and it copies the whole link");

    // Open link opens it, recorded under --e2e.
    app.session.key("Escape")?;
    app.session.right_click(x, y)?;
    let _ = menu_rows(ctx, app);
    let opened_link = click_menu_row(app, "Open link")
        && ctx.until("the link recorded as opened", 4_000, &mut || {
            url_count(app, url) == 1
        });
    ctx.ok(opened_link, "Open link opens it (recorded under --e2e)");

    // A clicked link is recorded, and no browser is opened.
    move_to(app, x + 40.0, y + 40.0)?;
    move_to(app, x, y)?;
    app.session.click(x, y)?;
    let clicked = ctx.until("the clicked link recorded", 4_000, &mut || {
        url_count(app, url) == 2
    });
    ctx.ok(
        clicked,
        "a clicked link is recorded under --e2e, and no browser is opened",
    );

    // THE "COPIED" BADGE (owner, 2026-09-23): at the bottom centre, then gone.
    ctx.ok(badge_text_ok, "a copy shows the \"Copied\" badge");
    ctx.ok(
        badge_pos_ok,
        format!("at the bottom centre of the window ({badge_pos})"),
    );
    let left = ctx.until("the badge to leave", 3_000, &mut || !badge_shown(app));
    ctx.ok(left, "and it leaves by itself");

    // Off a link with nothing selected: neither Copy row.
    app.session.key("Escape")?;
    let plain = match cell_point(app, "abef", 1, 0.2) {
        Some((px, py)) => {
            app.session.right_click(px, py)?;
            menu_rows(ctx, app)
        }
        None => Vec::new(),
    };
    ctx.ok(
        !plain.is_empty() && !plain.iter().any(|l| l.starts_with("Copy")),
        format!("right-click on plain text offers no Copy ({plain:?})"),
    );
    // Owner, 2026-10-23: with that menu still open, one right click elsewhere closes it AND opens the
    // menu of what is under the pointer (it once only closed it, so every second click did nothing).
    // The menu hangs down and right from the click, so the target has to be outside its box: the link
    // when it is, else the plain text, which is then asked from the link's own menu.
    let link_at = cell_point(app, "example.com", 2, 0.2);
    let plain_at = cell_point(app, "abef", 1, 0.2);
    let inside = |app: &mut App, p: (f32, f32)| {
        css(&probe(app, "ui.rect", json!({ "name": "menu" }))).is_some_and(|(x, y, w, h)| {
            let (px, py) = (f64::from(p.0), f64::from(p.1));
            px >= x && px <= x + w && py >= y && py <= y + h
        })
    };
    let reopened = match (link_at, plain_at) {
        (Some(l), _) if !inside(app, l) => {
            app.session.right_click(l.0, l.1)?;
            ctx.until("the link's menu from one right click", 4_000, &mut || {
                menu_labels(app).iter().any(|r| r == "Copy link")
            })
        }
        (Some(l), Some(pl)) => {
            app.session.key("Escape")?;
            app.session.right_click(l.0, l.1)?;
            ctx.until("the link's menu", 4_000, &mut || {
                menu_labels(app).iter().any(|r| r == "Copy link")
            }) && !inside(app, pl)
                && {
                    app.session.right_click(pl.0, pl.1)?;
                    ctx.until("the plain menu from one right click", 4_000, &mut || {
                        let rows = menu_labels(app);
                        rows.iter().any(|r| r == "Paste") && !rows.iter().any(|r| r == "Copy link")
                    })
                }
        }
        _ => false,
    };
    ctx.ok(
        reopened,
        format!(
            "a right click outside an open menu opens the menu of what is under it ({:?})",
            menu_labels(app)
        ),
    );
    app.session.key("Escape")?;

    // TEXT MARKED: right-click offers Copy, which copies exactly the selection.
    let marked = select(ctx, app, "example.com", 0, 6); // "example"
    let (mx, my) =
        cell_point(app, "example.com", 2, 0.2).ok_or("the marked text is not on screen")?;
    app.session.right_click(mx, my)?;
    let rows_marked = menu_rows(ctx, app);
    ctx.ok(
        marked
            && rows_marked
                .iter()
                .any(|l| l.starts_with("Copy") && !l.starts_with("Copy link")),
        format!("right-click with text marked offers Copy ({rows_marked:?})"),
    );
    let copied_exact = click_menu_row(app, "Copy")
        && ctx.until("the selection on the clipboard", 4_000, &mut || {
            clip_text(app) == "example"
        });
    ctx.ok(
        copied_exact,
        format!("and it copies the selection exactly ({:?})", clip_text(app)),
    );

    // Ctrl+C over a selection raises the badge too.
    ctx.until("the earlier badge to leave", 3_000, &mut || {
        !badge_shown(app)
    });
    let sel = select(ctx, app, "example.com", 8, 10); // "com"
    app.session.key(&crate::chord::copy_chord("Ctrl+C"))?;
    let shown = ctx.until("the Copied badge after Ctrl+C", 3_000, &mut || {
        badge_shown(app)
    });
    ctx.ok(
        sel && shown,
        "Ctrl+C over a selection shows the badge as well",
    );
    let copied_com = ctx.until("com on the clipboard", 3_000, &mut || {
        clip_text(app) == "com"
    });
    ctx.ok(copied_com, "and copied it");
    ctx.shot(app, "term-menu");
    Ok(())
}

/// Spec 2026-11-11 (replaces PT's pathLinks, a deliberate difference): a file link is only a path a session writes
/// between the marks `⟦` and `⟧`. A marked path broken over two rows by hand with an indent is ONE link; a marked
/// path with a line opens the text in the panel at that line, marked; a click opens a file and a folder, reveals a
/// script and never runs it; a right-click offers Open, Show in Explorer and Copy path; a plain `dir` listing shows
/// no file links at all.
pub fn marker_links(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let sign_off = alpha.join("docs").join("sign-off");
    let deep = alpha.join("scratchpad").join("flare-pack").join("Releases");
    for dir in [&sign_off, &deep] {
        fs::create_dir_all(dir).map_err(|e| format!("make {}: {e}", dir.display()))?;
    }
    let pdf = sign_off.join("rapport.pdf");
    let bat = alpha.join("run.bat");
    let notes = alpha.join("marker-notes.txt");
    let cut = deep.join("Flare-notes.txt");
    fs::write(&pdf, b"%PDF-1.4").map_err(|e| format!("write the pdf: {e}"))?;
    fs::write(&bat, b"@echo off").map_err(|e| format!("write the script: {e}"))?;
    let body: String = (1..=40).map(|i| format!("note line {i}\n")).collect();
    fs::write(&notes, body).map_err(|e| format!("write the notes: {e}"))?;
    fs::write(&cut, b"x").map_err(|e| format!("write the cut notes: {e}"))?;
    for renderer in renderers(false) {
        with_app(ctx, renderer, &alpha, Vec::new(), |ctx, app| {
            marker_links_on(ctx, app, &pdf, &bat, &notes, &cut)
        })?;
    }
    Ok(())
}

/// The texts of the file-link marks on the viewport (marks included, as the cells print them).
pub(super) fn path_texts(app: &mut App) -> Vec<String> {
    marks(app)
        .0
        .iter()
        .filter(|m| m["kind"] == json!("path"))
        .filter_map(|m| m["text"].as_str().map(str::to_string))
        .collect()
}

/// The file-link marks as (the path they name, their line, how many rows they cover).
fn file_links(app: &mut App) -> Vec<(String, Option<u64>, usize)> {
    marks(app)
        .0
        .iter()
        .filter(|m| m["kind"] == json!("path"))
        .map(|m| {
            let rows: std::collections::BTreeSet<i64> = m["pieces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["row"].as_i64())
                .collect();
            (
                m["uri"].as_str().unwrap_or_default().to_string(),
                m["line"].as_u64(),
                rows.len(),
            )
        })
        .collect()
}

/// The PowerShell that sets `$o` and `$c` to the start and end marks, typed as ASCII.
const MARKS: &str = "$o = [char]0x27E6; $c = [char]0x27E7";

fn marker_links_on(
    ctx: &mut Ctx,
    app: &mut App,
    pdf: &Path,
    bat: &Path,
    notes: &Path,
    cut: &Path,
) -> Result<(), String> {
    prompt(app)?;
    // A plain listing first: every name in it exists, and none is a link (the anti-goal of the spec).
    type_line(
        app,
        "cls; dir docs\\sign-off; Write-Host 'run.bat docs/sign-off/rapport.pdf'",
    )?;
    let listed = ctx.until("the listing", 8_000, &mut || {
        rows(app).contains("rapport.pdf") && rows(app).contains("run.bat docs/sign-off")
    });
    ctx.settle(50);
    ctx.settle(50);
    let none = file_links(app);
    ctx.ok(
        listed && none.is_empty(),
        format!("a plain dir listing and plain paths show no file links ({none:?})"),
    );

    // Marked paths: a file, a folder, a script and a text file with a line.
    type_line(
        app,
        &format!(
            "cls; {MARKS}; Write-Host ('Rapport ' + $o + 'docs/sign-off/rapport.pdf' + $c + ', folder ' + $o + 'docs/sign-off/' + $c + ', script ' + $o + 'run.bat' + $c + ', notes ' + $o + (Resolve-Path marker-notes.txt).Path + ':30' + $c)"
        ),
    )?;
    let notes_s = notes.to_string_lossy().into_owned();
    let painted = ctx.until(
        "the marked paths to wear the link colour",
        8_000,
        &mut || {
            let t = file_links(app);
            ["docs/sign-off/rapport.pdf", "docs/sign-off/", "run.bat"]
                .iter()
                .all(|want| t.iter().any(|(p, _, _)| p == want))
                && t.iter()
                    .any(|(p, l, _)| same_path(p, &notes_s) && *l == Some(30))
        },
    );
    ctx.ok(
        painted,
        format!(
            "a file, a folder, a script and a file with :30 between marks are links ({:?})",
            file_links(app)
        ),
    );
    ctx.shot(app, "marker-links");

    click_on(app, "docs/sign-off/rapport.pdf", 2, Button::Left)?;
    let pdf_s = pdf.to_string_lossy().into_owned();
    let opened_pdf = ctx.until("the file opened", 4_000, &mut || {
        opened(app)
            .iter()
            .any(|(k, p)| k == "open" && same_path(p, &pdf_s))
    });
    ctx.ok(
        opened_pdf,
        "a click opens the file, resolved against the shell folder",
    );

    let folder_s = pdf
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The folder is the last `docs/sign-off/` on the row (`click_on` takes the last).
    click_on(app, "docs/sign-off/", 2, Button::Left)?;
    let opened_folder = ctx.until("the folder opened", 4_000, &mut || {
        opened(app)
            .iter()
            .any(|(k, p)| k == "folder" && same_path(p.trim_end_matches('\\'), &folder_s))
    });
    ctx.ok(opened_folder, "a folder opens through Windows' folder verb");

    let bat_s = bat.to_string_lossy().into_owned();
    click_on(app, "run.bat", 2, Button::Left)?;
    let bat_clicked = ctx.until("the script clicked", 4_000, &mut || {
        opened(app).iter().any(|(_, p)| same_path(p, &bat_s))
    });
    let calls = opened(app);
    let never_run = !calls
        .iter()
        .any(|(k, p)| k == "open" && same_path(p, &bat_s))
        && calls
            .iter()
            .any(|(k, p)| k == "reveal" && same_path(p, &bat_s));
    ctx.ok(
        bat_clicked && never_run,
        "a script is NEVER run: it is shown selected in Explorer instead (DIFF-6: revealed)",
    );

    // A right-click opens nothing and offers the path's rows.
    let before = opened(app).len();
    click_on(app, "docs/sign-off/rapport.pdf", 2, Button::Right)?;
    ctx.settle(50);
    ctx.settle(50);
    ctx.ok(opened(app).len() == before, "a right-click opens nothing");
    let rows_menu = menu_rows(ctx, app);
    ctx.ok(
        rows_menu.len() >= 3
            && rows_menu[0] == "Open"
            && rows_menu.iter().any(|r| r == "Show in Explorer")
            && rows_menu.iter().any(|r| r == "Copy path"),
        format!("the menu offers Open, Show in Explorer and Copy path ({rows_menu:?})"),
    );
    let copied = click_menu_row(app, "Copy path")
        && ctx.until("the path on the clipboard", 4_000, &mut || {
            same_path(&clip_text(app), &pdf_s)
        });
    ctx.ok(copied, "Copy path copies it whole and absolute");

    // The line: the text opens in the panel at line 30, marked.
    click_on(app, "marker-notes.txt", 2, Button::Left)?;
    let at_line = ctx.until("the notes at their line", 6_000, &mut || {
        let s = probe(app, "panel.state", Value::Null);
        s["kind"] == json!(2)
            && s["title"] == json!("marker-notes.txt")
            && s["sel"]["a_row"] == json!(29)
    });
    ctx.ok(
        at_line,
        format!(
            "a click on a marked path with :30 shows the text in the panel with line 30 marked ({})",
            probe(app, "panel.state", Value::Null)["sel"]
        ),
    );
    marker_cut_by_hand(ctx, app, cut)
}

/// BROKEN BY HAND AND INDENTED (the spec's screenshot, 2026-11-11): Claude Code cuts its own rows at the window's
/// width and indents the next one, so the second row is never marked wrapped and starts with spaces. Written the
/// same way here: the first row exactly as wide as the window, then a new line and two spaces.
fn marker_cut_by_hand(ctx: &mut Ctx, app: &mut App, cut: &Path) -> Result<(), String> {
    let cut_s = cut.to_string_lossy().into_owned();
    type_line(
        app,
        &format!(
            "cls; {MARKS}; $w = $Host.UI.RawUI.WindowSize.Width; $s = ('word ' * 100).Substring(0, $w - 21) + ' ' + $o + (Resolve-Path scratchpad\\flare-pack\\Releases\\Flare-notes.txt).Path + $c; Write-Host $s.Substring(0, $w); Write-Host ('  ' + $s.Substring($w))"
        ),
    )?;
    let mut seen = String::new();
    let one_link = ctx.until(
        "the cut path to wear the link colour on both rows",
        8_000,
        &mut || {
            let links = file_links(app);
            seen = format!("{links:?}");
            links
                .iter()
                .any(|(p, _, rows)| same_path(p, &cut_s) && *rows >= 2)
        },
    );
    ctx.ok(
        one_link,
        format!("a marked path cut at the edge and indented is ONE link over both rows ({seen})"),
    );
    ctx.shot(app, "marker-links-cut");
    click_on(app, "Flare-notes.txt", 2, Button::Left)?;
    let shown = ctx.until("the cut file shown in the panel", 4_000, &mut || {
        let s = probe(app, "panel.state", Value::Null);
        s["kind"] == json!(2) && s["title"] == json!("Flare-notes.txt") && s["lines"] == json!(1)
    });
    ctx.ok(
        shown
            && !opened(app)
                .iter()
                .any(|(k, p)| k == "open" && same_path(p, &cut_s)),
        format!(
            "and a click on its second row shows the whole file in the panel and hands it to nobody ({:?})",
            opened(app)
        ),
    );
    Ok(())
}
/// PT: run.mjs:4215-4337. A printed link wears the link blue, its full stop is not part of it, a
/// wrapped link is painted on every row, the alternate screen keeps the blue through a hover, a
/// row rewritten in place keeps no link colour, and on a light theme the link takes a colour that
/// reads there. The light half is a second launch on the paper preset (chrome's `theme` does the
/// same), since Flare has no probe for picking a theme card yet.
pub fn links(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    for renderer in renderers(false) {
        with_app(ctx, renderer, &alpha, Vec::new(), links_default)?;
        with_app(
            ctx,
            renderer,
            &alpha,
            vec!["--theme".into(), "paper".into()],
            links_paper,
        )?;
    }
    Ok(())
}

fn links_default(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    const URL: &str = "https://go.microsoft.com/fwlink/?LinkID=108518";
    prompt(app)?;
    // Write-Host, so the output row holds the sentence exactly as typed here.
    type_line(
        app,
        &format!("cls; Write-Host 'online at {URL}. Then more.'"),
    )?;
    let seen = ctx.until("the printed link", 8_000, &mut || {
        web_marks(app).iter().any(|(u, _, _)| u == URL)
    });
    let blue = marks(app).1 == LINK_BLUE;
    ctx.ok(seen && blue, "a printed link wears the link blue");
    ctx.shot(app, "links");
    // The mark's text is read from the rows as they are now: wait until the link's own text shows.
    ctx.until("the link's text on screen", 4_000, &mut || {
        web_marks(app).iter().any(|(u, t, _)| u == URL && t == URL)
    });
    let texts: Vec<String> = web_marks(app).into_iter().map(|(_, t, _)| t).collect();
    ctx.ok(
        texts.iter().all(|t| !t.ends_with('.')),
        format!("and the sentence's full stop is not part of it ({texts:?})"),
    );

    // A link longer than the window wraps; every row of it is the link.
    let long = format!("https://example.com/{}", "a".repeat(260));
    type_line(app, &format!("cls; Write-Host '{long}'"))?;
    let wrapped = ctx.until("the wrapped link", 8_000, &mut || {
        web_marks(app)
            .iter()
            .any(|(u, t, _)| *u == long && *t == long)
    });
    ctx.ok(
        wrapped,
        "a link that wraps over rows is painted on every one of them",
    );
    let rows_of_wrap = web_marks(app)
        .iter()
        .find(|(u, _, _)| *u == long)
        .map_or(0, |(_, _, n)| *n);
    ctx.ok(rows_of_wrap >= 2, "and it really did wrap");

    // ON THE ALTERNATE SCREEN TOO (owner, 2026-09-28): markers do not exist there, the rows are
    // scanned as they are drawn.
    let alt = "https://example.org/alt/path?x=1";
    type_line(
        app,
        &format!(
            "Write-Host -NoNewline \"$([char]27)[?1049h$([char]27)[H\"; Write-Host 'open {alt} here'"
        ),
    )?;
    let alt_seen = ctx.until("the alternate-screen link", 8_000, &mut || {
        web_marks(app).iter().any(|(u, _, _)| u == alt)
    });
    let alt_blue = marks(app).1 == LINK_BLUE;
    ctx.ok(
        alt_seen && alt_blue,
        "on the alternate screen a printed link wears the link blue",
    );

    // HOVER AND LEAVE KEEP IT (#163): the pointer comes onto the link, then goes away.
    match cell_point(app, "example.org", 2, 0.5) {
        Some((hx, hy)) => {
            move_to(app, hx, hy)?;
            ctx.settle(50);
            ctx.settle(50);
            let kept = web_marks(app).iter().any(|(u, _, _)| u == alt) && marks(app).1 == LINK_BLUE;
            ctx.ok(
                kept,
                "a hovered link on the alternate screen keeps the link blue",
            );
            move_to(app, 10.0, 10.0)?;
            ctx.settle(50);
            ctx.settle(50);
            let kept_after =
                web_marks(app).iter().any(|(u, _, _)| u == alt) && marks(app).1 == LINK_BLUE;
            ctx.ok(
                kept_after,
                "and keeps it once the pointer leaves, with no scroll",
            );
        }
        None => ctx.ok(false, "the alternate-screen link is on screen to hover"),
    }

    // A row rewritten in place keeps no link colour (typed on the alternate screen, as PT does).
    app.session.type_text(
        "Write-Host -NoNewline \"$([char]27)[H$([char]27)[2Kplain words where the link was\"",
    )?;
    app.session.key("Enter")?;
    ctx.until("the rewritten row", 8_000, &mut || {
        rows(app).contains("plain words where the link was")
    });
    // PT waits for the repaint (`until`): the marks follow the rows once the app draws again.
    let plain_clear = ctx.until("the rewritten row to lose its link", 8_000, &mut || {
        !marks(app)
            .0
            .iter()
            .filter_map(|m| m["text"].as_str())
            .any(|t| t.contains("plain") || t.contains("words"))
    });
    ctx.ok(
        plain_clear,
        "and a row rewritten in place keeps no link colour",
    );

    // Out again, typed as it is: the prompt now sits at the top of the alternate screen.
    app.session
        .type_text("Write-Host -NoNewline \"$([char]27)[?1049l\"")?;
    app.session.key("Enter")?;
    prompt(app)?;
    type_line(app, &format!("cls; Write-Host 'see {URL}'"))?;
    ctx.until("the link back on the normal screen", 8_000, &mut || {
        web_marks(app).iter().any(|(u, _, _)| u == URL)
    });
    Ok(())
}

/// The light half of `links` (see there): the link on the paper preset must read on the ground.
fn links_paper(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    const URL: &str = "https://go.microsoft.com/fwlink/?LinkID=108518";
    prompt(app)?;
    type_line(app, &format!("cls; Write-Host 'see {URL}'"))?;
    let seen = ctx.until("the link on the paper theme", 8_000, &mut || {
        web_marks(app).iter().any(|(u, _, _)| u == URL)
    });
    let ink = marks(app).1;
    let ground = probe(app, "theme.tokens", Value::Null)["ground_solid"]
        .as_str()
        .unwrap_or("#000000")
        .to_string();
    let ratio = contrast(rgb_of(&ink), rgb_of(&ground));
    let rgb = rgb_of(&ink);
    ctx.ok(
        seen && ratio >= 4.5,
        format!("on a light theme the link takes another colour that reads there ({ink} on {ground} at {ratio:.1}:1)"),
    );
    ctx.ok(
        seen && rgb[2] > rgb[0],
        format!("and it is still a blue ({ink})"),
    );
    Ok(())
}

/// PT: run.mjs:4339-4420. OSC 8 links: FORCE_HYPERLINK is not set in the shell; a label wears the
/// link colour; a right click offers Copy link and opens nothing; a left click opens once; a
/// `file://` label is not a link; on the alternate screen a label is coloured and a click opens it;
/// no dialog ever.
pub fn osc8_links(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let script = alpha.join("osc8-probe.ps1");
    fs::write(&script, ALT_SCRIPT).map_err(|e| format!("write the probe program: {e}"))?;
    for renderer in renderers(false) {
        with_app(ctx, renderer, &alpha, Vec::new(), |ctx, app| {
            osc8_on(ctx, app, &script)
        })?;
    }
    Ok(())
}

/// One OSC 8 sequence to print: the label `text` linked to `uri`, as PT's `osc8` helper writes it.
fn osc8_line(uri: &str, text: &str) -> String {
    let esc = |s: &str| format!("[char]27 + '{s}'");
    format!(
        "[Console]::Write({} + [char]7 + '{text}' + {} + [char]7); Write-Host ''",
        esc(&format!("]8;;{uri}")),
        esc("]8;;")
    )
}

fn osc8_on(ctx: &mut Ctx, app: &mut App, script: &Path) -> Result<(), String> {
    prompt(app)?;
    // #173 set FORCE_HYPERLINK; the review of 2026-10-11 took it out: no shell is told it.
    type_line(app, "Write-Host \"FH-$env:FORCE_HYPERLINK-END\"")?;
    let fh = ctx.until("the FORCE_HYPERLINK line", 8_000, &mut || {
        rows(app).contains("FH--END")
    });
    ctx.ok(fh, "FORCE_HYPERLINK is not set in the shell");

    type_line(app, &format!("cls; {}", osc8_line(URL8, "OSC8LABEL")))?;
    let label = ctx.until(
        "the OSC 8 label to wear the link colour",
        8_000,
        &mut || osc8_label_blue(app, "OSC8LABEL"),
    );
    ctx.ok(label, "an OSC 8 label wears the link colour");
    if let Some((x, y)) = cell_point(app, "OSC8LABEL", 0, 0.5) {
        app.session.right_click(x, y)?;
        let menu = menu_rows(ctx, app);
        ctx.settle(50);
        ctx.settle(50);
        ctx.ok(
            menu.iter().any(|l| l.contains("Copy link")),
            format!("a right click on it offers Copy link ({menu:?})"),
        );
        ctx.ok(url_total(app) == 0, "and opens nothing");
        app.session.key("Escape")?;
        move_to(app, x + 60.0, y + 60.0)?;
        move_to(app, x, y)?;
        app.session.click(x, y)?;
        let once = ctx.until("the one open", 4_000, &mut || url_count(app, URL8) == 1);
        ctx.ok(once, "a left click opens it, once (recorded under --e2e)");
    }

    // Any other scheme stays inert: nothing is painted and nothing opens.
    type_line(
        app,
        &format!("cls; {}", osc8_line("file:///C:/x", "FILELABEL")),
    )?;
    let file_seen = ctx.until("the file:// label on screen", 8_000, &mut || {
        rows(app).contains("FILELABEL")
    });
    if file_seen {
        // A mark is read from the rows as they are now: wait until no mark sits on the label.
        let not_linked = ctx.until("the file:// label to stop being a link", 4_000, &mut || {
            !marks(app).0.iter().any(|m| {
                m["text"].as_str().is_some_and(|t| t.contains("FILELABEL"))
                    || m["uri"].as_str().is_some_and(|u| u.starts_with("file:"))
            })
        });
        ctx.ok(not_linked, "a file:// OSC 8 label is not painted as a link");
        if let Some((x, y)) = cell_point(app, "FILELABEL", 0, 0.5) {
            move_to(app, x + 60.0, y + 60.0)?;
            move_to(app, x, y)?;
            app.session.click(x, y)?;
        }
        ctx.settle(50);
        ctx.settle(50);
        let total = url_total(app);
        ctx.ok(
            total == 1,
            format!("and a click on it opens nothing ({total} opened)"),
        );
    } else {
        ctx.ok(false, "the file:// label is on screen");
    }

    // The alternate screen, as in Claude Code's fullscreen view.
    type_line(app, &alt_screen_line(script))?;
    let alt = ctx.until("the alternate-screen label", 8_000, &mut || {
        osc8_label_blue(app, "ALTLABEL")
    });
    ctx.ok(
        alt,
        "on the alternate screen an OSC 8 label wears the link colour too",
    );
    if alt {
        if let Some((x, y)) = cell_point(app, "ALTLABEL", 0, 0.5) {
            move_to(app, x + 60.0, y + 60.0)?;
            move_to(app, x, y)?;
            app.session.click(x, y)?;
            let twice = ctx.until("the second open", 4_000, &mut || url_count(app, URL8) == 2);
            ctx.ok(twice, "and a left click opens it");
        }
    }
    // The program leaves the alternate screen on any key; the prompt comes back.
    app.session.type_text("q")?;
    ctx.settle(50);
    let dialog = probe(app, "app.state", Value::Null)["dialog"]["open"] == json!(true);
    ctx.ok(!dialog, "no dialog ever");
    Ok(())
}

/// The colour of the device pixel (x, y) of the last snapshot.
pub(super) fn device_pixel(app: &mut App, x: f64, y: f64) -> Option<Rgb> {
    let (x, y) = (x.round().max(0.0) as u32, y.round().max(0.0) as u32);
    match app.session.call(&Request::Pixel { x, y }) {
        Ok(Reply::Pixel { rgba }) => Some([rgba[0], rgba[1], rgba[2]]),
        _ => None,
    }
}

/// The sum of the channel gaps between two colours.
pub(super) fn gap(a: Rgb, b: Rgb) -> i32 {
    (0..3)
        .map(|i| (i32::from(a[i]) - i32::from(b[i])).abs())
        .sum()
}

/// Owner bug 2026-10-21: Claude Code's status line (Powerline chevron, round caps, a bar cell) drew
/// a tofu box and a star. Prints the exact sequence (thin chevron on red, left cap, one bar cell,
/// right cap) and reads the pixels: the shapes are Flare's own, edge to edge, in the cell's colours,
/// with no seam between a cap and the bar. Both renderers.
pub fn powerline_glyphs(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    for renderer in renderers(true) {
        with_app(ctx, renderer, &alpha, Vec::new(), |ctx, app| {
            powerline_on(ctx, app, renderer)
        })?;
    }
    Ok(())
}

fn powerline_on(ctx: &mut Ctx, app: &mut App, renderer: &str) -> Result<(), String> {
    prompt(app)?;
    type_line(
        app,
        "cls; $e = [char]27; Write-Host (\"$e[38;2;255;255;255;48;2;200;0;0m\" + [char]0xE0B1 + \"$e[0m$e[38;2;64;160;255m\" + [char]0xE0B6 + \"$e[0m$e[48;2;64;160;255m $e[0m$e[38;2;64;160;255m\" + [char]0xE0B4 + \"$e[0m\")",
    )?;
    let tab = front_tab(app);
    let mut at = None;
    let shown = ctx.until("the status line row", 8_000, &mut || {
        let grid = probe(app, "grid.text", json!({ "tab": tab }));
        at = grid["rows"].as_array().and_then(|rows| {
            rows.iter().enumerate().find_map(|(i, r)| {
                let col = r.as_str()?.chars().position(|c| c == '\u{E0B1}')?;
                Some((i, col))
            })
        });
        at.is_some()
    });
    ctx.ok(
        shown,
        format!("{renderer}: the status line sequence is on screen (tab {tab}, tabs {:?}, front {:?}, at {at:?})", app.session.state().ok().map(|s| s.tabs.len()), app.session.state().ok().and_then(|s| s.front)),
    );
    let (Some((row, col)), true) = (at, shown) else {
        return Ok(());
    };
    // A software snapshot can lag the grid by a frame, so take snapshots until the whole check holds.
    let mut result = (false, String::new());
    let (shots, scenario) = (ctx.shots.clone(), ctx.name);
    ctx.until(
        "the snapshot to show the whole status line",
        4_000,
        &mut || {
            let name = format!("powerline-{renderer}");
            if crate::ctx::shot_file(&shots, scenario, &name, |p| app.session.snapshot(p)).is_none()
            {
                return false;
            }
            result = powerline_measure(app, row, col);
            result.0
        },
    );
    let (ok, note) = result;
    ctx.ok(
        ok,
        format!("{renderer}: chevron, cap, bar, cap are drawn as shapes in the cell colours with no seam ({note})"),
    );
    Ok(())
}

/// Reads the status line's four cells off the last snapshot: (all as wanted, what was not).
fn powerline_measure(app: &mut App, row: usize, col: usize) -> (bool, String) {
    const BLUE: Rgb = [64, 160, 255];
    const RED: Rgb = [200, 0, 0];
    let d = probe(app, "dpi.state", Value::Null);
    let (pad, cw, ch, k) = (
        d["padding"].as_f64().unwrap_or(0.0),
        d["cell_w"].as_f64().unwrap_or(0.0),
        d["cell_h"].as_f64().unwrap_or(0.0),
        d["scale"].as_f64().unwrap_or(1.0),
    );
    let (gx, gy, _, _) = css(&probe(app, "ui.rect", json!({ "name": "grid" }))).unwrap_or_default();
    let cell = |c: usize| {
        (
            gx * k + pad + (col + c) as f64 * cw,
            gy * k + pad + row as f64 * ch,
        )
    };
    let px = |app: &mut App, c: usize, fx: f64, fy: f64| {
        let (ox, oy) = cell(c);
        device_pixel(
            app,
            ox + (fx * cw).clamp(0.0, cw - 1.0),
            oy + (fy * ch).clamp(0.0, ch - 1.0),
        )
    };
    let mut ok = true;
    let mut note = String::new();
    let mut check = |name: &str, pass: bool, v: String| {
        if !pass {
            ok = false;
            note.push_str(&format!(" {name}={v}"));
        }
    };

    // Cell 0, the thin chevron: ink at the tip, none in the open side's middle, the cell's colours.
    let tip = px(app, 0, 1.0, 0.5).or(px(app, 0, 1.0, 0.45));
    let open = px(app, 0, 0.0, 0.5);
    check(
        "chevron open side is the cell bg",
        open.is_some_and(|p| gap(p, RED) < 30),
        format!("{open:?}"),
    );
    check(
        "chevron tip is ink",
        tip.is_some_and(|p| gap(p, RED) > 150),
        format!("{tip:?}"),
    );
    // Ink count in the cell: a stroke, not a filled box or an outlined tofu.
    let (ox, oy) = cell(0);
    let mut inked = 0u32;
    let mut total = 0u32;
    for y in 0..ch as u32 {
        for x in 0..cw as u32 {
            if let Some(p) = device_pixel(app, ox + f64::from(x), oy + f64::from(y)) {
                total += 1;
                if gap(p, RED) > 150 {
                    inked += 1;
                }
            }
        }
    }
    check(
        "chevron ink share",
        inked > 0 && inked * 3 < total,
        format!("{inked}/{total}"),
    );

    // Cell 1, the left cap: round, its flat side on the bar.
    let ground = px(app, 1, 0.0, 0.0);
    let mid = px(app, 1, 0.5, 0.5);
    check(
        "left cap middle is fg",
        mid.is_some_and(|p| gap(p, BLUE) < 40),
        format!("{mid:?}"),
    );
    check(
        "left cap corner is ground",
        matches!((ground, mid), (Some(g), Some(m)) if gap(g, m) > 150),
        format!("{ground:?}"),
    );
    for fy in [0.1, 0.25, 0.5, 0.75, 0.9] {
        let edge = px(app, 1, 1.0, fy);
        check(
            "left cap flat side meets the bar",
            edge.is_some_and(|p| gap(p, BLUE) < 60),
            format!("{fy}:{edge:?}"),
        );
    }
    // Cell 2, the bar: its background, and no seam against either cap.
    for fy in [0.1, 0.5, 0.9] {
        for fx in [0.0, 0.5, 1.0] {
            let p = px(app, 2, fx, fy);
            check(
                "bar cell is its bg",
                p.is_some_and(|p| gap(p, BLUE) < 30),
                format!("{fx},{fy}:{p:?}"),
            );
        }
    }
    // Cell 3, the right cap.
    for fy in [0.1, 0.25, 0.5, 0.75, 0.9] {
        let edge = px(app, 3, 0.0, fy);
        check(
            "right cap flat side meets the bar",
            edge.is_some_and(|p| gap(p, BLUE) < 60),
            format!("{fy}:{edge:?}"),
        );
    }
    let far = px(app, 3, 1.0, 0.0);
    check(
        "right cap far corner is ground",
        matches!((far, mid), (Some(g), Some(m)) if gap(g, m) > 150),
        format!("{far:?}"),
    );
    (ok, note)
}

/// The line that runs the alternate-screen script: Windows PowerShell on Windows, pwsh on macOS (rule 1109).
#[cfg(windows)]
fn alt_screen_line(script: &Path) -> String {
    format!(
        "& powershell.exe -NoProfile -ExecutionPolicy Bypass -File '{}'",
        script.display()
    )
}

#[cfg(not(windows))]
fn alt_screen_line(script: &Path) -> String {
    crate::standins::ps_file_line(script)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_renderers_follow_the_list_and_the_both_flag() {
        assert_eq!(pick_renderers("", false), vec!["gpu"]);
        assert_eq!(pick_renderers("", true), vec!["gpu", "software"]);
        assert_eq!(pick_renderers("software", true), vec!["software"]);
        assert_eq!(
            pick_renderers("software,gpu", false),
            vec!["gpu", "software"]
        );
    }

    #[test]
    fn a_path_is_the_same_path_whatever_its_case_and_slashes() {
        assert!(same_path(
            r"C:\Work\Docs\Rapport.pdf",
            "c:/work/docs/rapport.pdf"
        ));
        assert!(same_path(r"C:\Work\png\", r"C:\Work\png"));
        assert!(!same_path(r"C:\Work\a.pdf", r"C:\Work\b.pdf"));
    }

    #[test]
    fn the_shells_own_backspace_is_one_character_in_old_output() {
        assert!(echoed_twice("PS C:\\a> echo k1x\nk1x\nPS C:\\a>", "k1"));
        assert!(
            !echoed_twice("PS C:\\a> echo k1xy\nPS C:\\a>", "k1"),
            "the typed line still holds its y"
        );
    }

    #[test]
    fn a_colour_reads_back_from_its_hex() {
        assert_eq!(rgb_of("#79a7d8"), [0x79, 0xa7, 0xd8]);
        assert_eq!(rgb_of("#f4f4f4ff"), [0xf4, 0xf4, 0xf4]);
    }

    #[test]
    fn the_osc8_line_wraps_its_label_in_the_link_and_its_close() {
        let line = osc8_line("https://example.com/x", "LBL");
        assert!(line.contains("]8;;https://example.com/x"));
        assert!(line.contains("'LBL'"));
        assert!(line.contains("']8;;'"));
    }
}
