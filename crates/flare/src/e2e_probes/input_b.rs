//! Input B: selection and links probes (Task 30): `term.selection`, `term.links` and `url.log`.
//! Read-only: they look at the front shell's emulator, the decoration inputs and the URL log, and
//! set nothing.

use std::sync::Mutex;

use flare_render::decor::DecorInk;
use flare_term::link_scan::{LinkMark, LinkTarget};
use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Every web address the window's URL opener was asked to open, in order. Under `--e2e` the opener
/// only records (nothing reaches the owner's browser); the window's opener hands each address here.
static OPENED_URLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Records one address the app would have opened. The window's opener calls it under `--e2e`.
pub fn record_url(url: &str) {
    OPENED_URLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(url.to_string());
}

/// Every address recorded so far (what the url.log probe answers).
#[cfg(test)]
pub fn recorded_urls() -> Vec<String> {
    OPENED_URLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("term.selection", term_selection);
    r.add("term.links", term_links);
    r.add("url.log", url_log);
}

/// The ink the decoration paints link cells with: the ink the controller gives the decoration, or
/// the default blue when it gives none (`build_scene_decor` makes the same choice). As `#rrggbb`.
pub fn ink_of(ink: Option<DecorInk>) -> String {
    let l = ink.unwrap_or_else(DecorInk::pt_default).link;
    format!("#{:02x}{:02x}{:02x}", l.r, l.g, l.b)
}

/// The kind and the address of a link mark: `web` (the text is the address), `path` (the text as
/// printed, resolved by the path worker) or `osc8` (the cell's hyperlink).
pub fn describe(target: &LinkTarget) -> (&'static str, String) {
    match target {
        LinkTarget::Web(s) => ("web", s.clone()),
        LinkTarget::Path { text } => ("path", text.clone()),
        LinkTarget::Osc8 { uri } => ("osc8", uri.clone()),
    }
}

/// `term.selection`: `{ active, text, block, rows }`. `rows` counts the selection's rows from its
/// first row now; `block` is true for a block (rectangular) selection.
fn term_selection(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let h = ctx.front.clone().ok_or("no shell is in front")?;
    Ok(h.with_emu_quiet(|e| {
        json!({
            "active": e.has_selection(),
            "text": e.sel_text(),
            "block": e.sel_bounds().is_some_and(|b| b.2),
            "rows": e.sel_row_count(),
        })
    }))
}

/// `term.links`: `{ links: [ { kind, uri, text, pieces: [ { row, from, to } ] } ], ink, alt_screen }`.
/// One entry per link on the viewport (a link that wraps has one piece per row). `row` is a
/// viewport row, `from` and `to` are the cells it covers (`to` exclusive), `text` is what those
/// cells print, and `ink` is the colour the decoration paints the link cells with.
fn term_links(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let h = ctx.front.clone().ok_or("no shell is in front")?;
    let inputs = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "the controller is busy")?
        .decor_inputs();
    let ink = crate::term_ui::effective_ink(
        inputs
            .try_borrow()
            .map_err(|_| "the decoration inputs are busy")?
            .ink,
    );
    Ok(h.with_emu_quiet(|e| {
        let (rows, cols) = (e.rows(), e.cols());
        let top = e.viewport_top();
        let mut marks: Vec<LinkMark> = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                if let Some(m) = e.link_at_view(r, c)
                    && !marks.contains(&m)
                {
                    marks.push(m);
                }
            }
        }
        let links: Vec<Value> = marks
            .iter()
            .map(|m| {
                let (kind, uri) = describe(&m.target);
                let mut text = String::new();
                let pieces: Vec<Value> = m
                    .pieces
                    .iter()
                    .map(|p| {
                        let row = p.row as i64 - top as i64;
                        let line: Vec<char> = e.row_text(row as i32).chars().collect();
                        let piece: String = line.iter().skip(p.col).take(p.width).collect();
                        text.push_str(&piece);
                        json!({ "row": row, "from": p.col, "to": p.col + p.width })
                    })
                    .collect();
                json!({ "kind": kind, "uri": uri, "text": text, "pieces": pieces })
            })
            .collect();
        json!({
            "links": links,
            "ink": ink_of(ink),
            "alt_screen": e.mode().alt_screen,
        })
    }))
}

/// `url.log`: `{ urls: [...] }`, every web address the opener was asked to open, in order.
fn url_log(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let urls = OPENED_URLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Ok(json!({ "urls": urls }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_ink_is_the_decoration_blue_when_none_is_given() {
        assert_eq!(ink_of(None), "#79a7d8");
    }

    #[test]
    fn a_given_ink_is_the_one_read() {
        let mut ink = DecorInk::pt_default();
        ink.link = flare_render::color::Rgba::rgb(0x12, 0x34, 0x56);
        assert_eq!(ink_of(Some(ink)), "#123456");
    }

    #[test]
    fn each_kind_of_link_names_itself_and_its_address() {
        assert_eq!(
            describe(&LinkTarget::Web("https://a.io/x".into())),
            ("web", "https://a.io/x".to_string())
        );
        assert_eq!(
            describe(&LinkTarget::Path {
                text: "docs/a.pdf".into()
            }),
            ("path", "docs/a.pdf".to_string())
        );
        assert_eq!(
            describe(&LinkTarget::Osc8 {
                uri: "https://example.com/osc8".into()
            }),
            ("osc8", "https://example.com/osc8".to_string())
        );
    }

    #[test]
    fn a_light_theme_link_reports_the_ink_it_wears_not_the_default_blue() {
        // The paper theme's link: the decoration takes the worn theme link when the controller gives no ink.
        crate::term_ui::set_worn_link(&flare_theme::links::link_colour("#f6f4ee", "#24292e"));
        let hex = ink_of(crate::term_ui::effective_ink(None));
        assert_ne!(hex, "#79a7d8", "the probe must report the worn link");
        assert!(flare_theme::ansi::contrast_ratio(&hex, "#f6f4ee") >= 4.5);
    }

    #[test]
    fn a_recorded_address_reads_back_in_order() {
        record_url("https://task30.example/first");
        record_url("https://task30.example/second");
        let urls = OPENED_URLS.lock().unwrap().clone();
        let a = urls
            .iter()
            .position(|u| u == "https://task30.example/first");
        let b = urls
            .iter()
            .position(|u| u == "https://task30.example/second");
        assert!(a.is_some() && b.is_some() && a < b);
    }
}
