//! The device's UI tree, as the sim host reads it (`AXPTranslator`, PR 3), filtered into the list the agent sees
//! (rule 1315): zero-size elements and unlabelled full-screen wrappers dropped, label-less wrappers merged into
//! their children, the rest numbered `d1, d2, ...` in reading order and capped at [`MAX_REFS`]. Every label and value
//! is untrusted text and is cleaned here; the MCP side fences the list in the page-content markers (PR 5).
//!
//! Frames are device points in the current orientation, the same points a tap takes.

use std::collections::HashMap;

use flare_ctl::text::clean_text;
use serde::{Deserialize, Serialize};

use crate::geom::Size;

/// The most elements one look lists.
pub const MAX_REFS: usize = 300;
/// The longest label or value kept, in characters.
pub const TEXT_MAX: usize = 200;
/// The longest role kept.
const ROLE_MAX: usize = 40;
/// An element at least this share of the screen both ways is "full screen".
const FULL_SCREEN: f64 = 0.95;

/// A rectangle in device points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn is_empty(&self) -> bool {
        !(self.w > 0.0 && self.h > 0.0)
    }
}

/// One element as the sim host reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    /// "AXButton", "AXStaticText", "AXGroup" ...
    pub role: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub value: String,
    pub frame: Rect,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub children: Vec<UiNode>,
}

fn yes() -> bool {
    true
}

/// One element the agent sees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiRef {
    /// `d1`, `d2` ...: valid until the next look.
    #[serde(rename = "ref")]
    pub r: String,
    pub role: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    pub frame: Rect,
    pub enabled: bool,
}

/// What a look lists, and how many elements did not fit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiList {
    pub refs: Vec<UiRef>,
    pub left_out: usize,
}

impl UiList {
    /// Where each ref is, for the taps that follow (a ref not in it is stale: `not_found`).
    pub fn ref_map(&self) -> RefMap {
        RefMap(self.refs.iter().map(|r| (r.r.clone(), r.frame)).collect())
    }
}

/// The refs of the last look and their frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefMap(HashMap<String, Rect>);

impl RefMap {
    pub fn get(&self, r: &str) -> Option<Rect> {
        self.0.get(r).copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Roles a person can act on, listed even without a label (an icon button).
fn actionable(role: &str) -> bool {
    matches!(
        role,
        "AXButton"
            | "AXTextField"
            | "AXSecureTextField"
            | "AXTextArea"
            | "AXSearchField"
            | "AXSwitch"
            | "AXCheckBox"
            | "AXSlider"
            | "AXLink"
            | "AXCell"
            | "AXTab"
            | "AXPopUpButton"
            | "AXMenuItem"
            | "AXIncrementor"
    )
}

/// The list the agent sees of the tree under `root` on a `screen` sized screen.
pub fn filter_tree(root: &UiNode, screen: Size) -> UiList {
    let mut kept = Vec::new();
    walk(root, screen, &mut kept);
    // Reading order: top to bottom, then left to right (rows by whole points, so a 0.3 pt jitter keeps a row).
    kept.sort_by(|a, b| {
        let key = |r: &UiRef| (r.frame.y.round() as i64, r.frame.x.round() as i64);
        key(a).cmp(&key(b))
    });
    let left_out = kept.len().saturating_sub(MAX_REFS);
    kept.truncate(MAX_REFS);
    for (i, r) in kept.iter_mut().enumerate() {
        r.r = format!("d{}", i + 1);
    }
    UiList {
        refs: kept,
        left_out,
    }
}

fn walk(n: &UiNode, screen: Size, out: &mut Vec<UiRef>) {
    let label = clean_text(&n.label, TEXT_MAX);
    let value = clean_text(&n.value, TEXT_MAX);
    let role = clean_text(&n.role, ROLE_MAX);
    let full = n.frame.w >= screen.w * FULL_SCREEN && n.frame.h >= screen.h * FULL_SCREEN;
    let named = !label.is_empty() || !value.is_empty();
    // A zero-size element is not on screen; an unlabelled full-screen one is the app's wrapper; a label-less
    // element nobody can act on only groups its children. Each is dropped, its children are looked at.
    let listed = !n.frame.is_empty() && (named || (actionable(&role) && !full));
    if listed {
        out.push(UiRef {
            r: String::new(),
            role,
            label,
            value,
            frame: n.frame,
            enabled: n.enabled,
        });
    }
    for c in &n.children {
        walk(c, screen, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Size = Size { w: 402.0, h: 874.0 };

    fn node(role: &str, label: &str, frame: (f64, f64, f64, f64), children: Vec<UiNode>) -> UiNode {
        UiNode {
            role: role.into(),
            label: label.into(),
            value: String::new(),
            frame: Rect {
                x: frame.0,
                y: frame.1,
                w: frame.2,
                h: frame.3,
            },
            enabled: true,
            children,
        }
    }

    fn labels(l: &UiList) -> Vec<&str> {
        l.refs.iter().map(|r| r.label.as_str()).collect()
    }

    #[test]
    fn zero_size_and_unlabelled_full_screen_groups_are_dropped() {
        let root = node(
            "AXApplication",
            "",
            (0.0, 0.0, 402.0, 874.0),
            vec![node(
                "AXGroup",
                "",
                (0.0, 0.0, 402.0, 874.0),
                vec![
                    node(
                        "AXStaticText",
                        "Settings",
                        (16.0, 100.0, 120.0, 40.0),
                        vec![],
                    ),
                    node("AXButton", "Hidden", (16.0, 150.0, 0.0, 44.0), vec![]),
                    node("AXButton", "Flat", (16.0, 150.0, 80.0, 0.0), vec![]),
                    // A label-less wrapper inside: merged, its child listed.
                    node(
                        "AXGroup",
                        "",
                        (0.0, 200.0, 402.0, 300.0),
                        vec![node("AXCell", "Wi-Fi", (0.0, 200.0, 402.0, 44.0), vec![])],
                    ),
                    // An icon button with no label is still something to tap.
                    node("AXButton", "", (350.0, 60.0, 44.0, 44.0), vec![]),
                    // A labelled full-screen element is kept (it says something).
                    node("AXScrollArea", "Main", (0.0, 0.0, 402.0, 874.0), vec![]),
                    // A zero-size parent drops itself only.
                    node(
                        "AXGroup",
                        "Gone",
                        (0.0, 600.0, 0.0, 0.0),
                        vec![node(
                            "AXStaticText",
                            "Kept",
                            (20.0, 600.0, 50.0, 20.0),
                            vec![],
                        )],
                    ),
                ],
            )],
        );
        let l = filter_tree(&root, SCREEN);
        assert_eq!(l.left_out, 0);
        assert_eq!(labels(&l), vec!["Main", "", "Settings", "Wi-Fi", "Kept"]);
        assert_eq!(l.refs[1].role, "AXButton");
        assert!(
            l.refs
                .iter()
                .all(|r| r.role != "AXApplication" && r.role != "AXGroup")
        );
    }

    #[test]
    fn refs_are_numbered_in_reading_order_and_capped_at_300() {
        // Given bottom-up and right to left, numbered top-down and left to right.
        let root = node(
            "AXGroup",
            "",
            (0.0, 0.0, 402.0, 874.0),
            vec![
                node("AXButton", "C", (10.0, 300.0, 40.0, 40.0), vec![]),
                node("AXButton", "B", (200.0, 100.3, 40.0, 40.0), vec![]),
                node("AXButton", "A", (10.0, 100.0, 40.0, 40.0), vec![]),
            ],
        );
        let l = filter_tree(&root, SCREEN);
        assert_eq!(labels(&l), vec!["A", "B", "C"]);
        assert_eq!(
            l.refs.iter().map(|r| r.r.as_str()).collect::<Vec<_>>(),
            vec!["d1", "d2", "d3"]
        );
        let map = l.ref_map();
        assert_eq!(map.get("d2").map(|r| r.center()), Some((220.0, 120.3)));
        assert_eq!(map.get("d4"), None);
        // 350 elements: the first 300 in reading order are listed, the rest counted.
        let many = (0..350)
            .rev()
            .map(|i| {
                node(
                    "AXStaticText",
                    &format!("row {i}"),
                    (0.0, f64::from(i) * 2.0, 100.0, 2.0),
                    vec![],
                )
            })
            .collect();
        let l = filter_tree(&node("AXGroup", "", (0.0, 0.0, 402.0, 874.0), many), SCREEN);
        assert_eq!(l.refs.len(), MAX_REFS);
        assert_eq!(l.left_out, 50);
        assert_eq!(l.refs[0].label, "row 0");
        assert_eq!(l.refs[299].r, "d300");
        assert_eq!(l.refs[299].label, "row 299");
        assert_eq!(l.ref_map().len(), 300);
    }

    #[test]
    fn labels_are_cleaned() {
        let mut n = node(
            "AXStaticText",
            "Line one\nline\u{200b} two\t\u{1b}[31m",
            (0.0, 10.0, 100.0, 20.0),
            vec![],
        );
        n.value = "x".repeat(500);
        let mut b = node("AXButton\u{202e}", "  ", (0.0, 50.0, 40.0, 40.0), vec![]);
        b.enabled = false;
        let l = filter_tree(
            &node("AXGroup", "", (0.0, 0.0, 402.0, 874.0), vec![n, b]),
            SCREEN,
        );
        let r = &l.refs[0];
        assert!(
            !r.label.contains('\n') && !r.label.contains('\u{200b}') && !r.label.contains('\u{1b}')
        );
        assert!(r.label.starts_with("Line one line two"), "{:?}", r.label);
        assert!(r.value.chars().count() <= TEXT_MAX + 3, "cut, with a mark");
        assert!(r.value.ends_with("..."));
        // A blank label is no label; a role is cleaned too.
        assert_eq!(l.refs[1].label, "");
        assert_eq!(
            l.refs[1].role, "AXButton",
            "an invisible mark is no part of a role"
        );
        assert!(!l.refs[1].enabled);
        // A tree from the wire reads with its defaults.
        let n: UiNode =
            serde_json::from_str(r#"{"role":"AXButton","frame":{"x":1,"y":2,"w":3,"h":4}}"#)
                .unwrap();
        assert!(n.enabled && n.label.is_empty() && n.children.is_empty());
    }
}
