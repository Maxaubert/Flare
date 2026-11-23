//! The device's UI tree, as the sim host reads it (`AXPTranslator`, PR 3), filtered into the list the agent sees
//! (rule 1315): zero-size elements and unlabelled full-screen wrappers dropped, label-less wrappers merged into
//! their children, the rest numbered `d1, d2, ...` in reading order and capped at [`MAX_REFS`]. Every label and value
//! is untrusted text and is cleaned here; the MCP side fences the list in the page-content markers (PR 5).
//!
//! Frames are device points in the current orientation, the same points a tap takes (dp on Android, rule 1367).
//!
//! Android (rule 1360): the emu host reads the tree through the agent helper and gives each node a role from
//! [`android_role`], one of the roles below, so the filter, the refs and the agent's list work the same for both kinds.

use std::collections::HashMap;

use flare_ctl::text::clean_text;
use serde::{Deserialize, Serialize};

use crate::geom::{Orientation, Size};

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

/// What an Android node says about itself (`AccessibilityNodeInfo`, read by the agent helper), for [`android_role`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AndroidNode<'a> {
    /// The view's class name: `android.widget.Button`, `androidx.recyclerview.widget.RecyclerView`, or
    /// `android.view.View` for most Compose nodes.
    pub class: &'a str,
    pub clickable: bool,
    pub editable: bool,
    pub checkable: bool,
    pub scrollable: bool,
}

/// Android class names by their last part (after the last `.` or `$`), and the role each is. The first row whose
/// name ends the class wins, so `MaterialButton` is a button and `RadioButton` a check box. Kept short on purpose:
/// what is not here falls back on the node's flags.
const ANDROID_ROLES: &[(&str, &str)] = &[
    ("EditText", "AXTextField"),
    ("AutoCompleteTextView", "AXTextField"),
    ("SearchView", "AXSearchField"),
    ("Switch", "AXSwitch"),
    ("SwitchCompat", "AXSwitch"),
    ("SwitchMaterial", "AXSwitch"),
    ("MaterialSwitch", "AXSwitch"),
    ("ToggleButton", "AXSwitch"),
    ("CheckBox", "AXCheckBox"),
    ("RadioButton", "AXCheckBox"),
    ("CheckedTextView", "AXCheckBox"),
    ("Chip", "AXCheckBox"),
    ("SeekBar", "AXSlider"),
    ("RatingBar", "AXSlider"),
    ("Slider", "AXSlider"),
    ("NumberPicker", "AXIncrementor"),
    ("Spinner", "AXPopUpButton"),
    ("Button", "AXButton"),
    ("TabView", "AXTab"),
    ("WebView", "AXWebArea"),
    ("ProgressBar", "AXProgressIndicator"),
    ("TextView", "AXStaticText"),
    ("ImageView", "AXImage"),
    ("RecyclerView", "AXScrollArea"),
    ("ListView", "AXScrollArea"),
    ("GridView", "AXScrollArea"),
    ("ScrollView", "AXScrollArea"),
    ("ViewPager", "AXScrollArea"),
];

/// The role an Android node has in the tree model, so [`filter_tree`] lists and numbers both kinds the same way
/// (rule 1360): Android classes map onto the roles the iOS tree already uses. A node that types is a text field, a
/// class in [`ANDROID_ROLES`] is its role, a checkable one is a check box, a clickable one is a button (a Compose
/// `View` or a tappable row), a scrollable one a scroll area, anything else a group.
pub fn android_role(n: AndroidNode<'_>) -> &'static str {
    if n.editable {
        return "AXTextField";
    }
    let short = n.class.rsplit(['.', '$']).next().unwrap_or_default();
    let known = ANDROID_ROLES
        .iter()
        .find(|(name, _)| short.ends_with(name))
        .map(|(_, role)| *role);
    match known {
        // A text or a picture someone can tap acts as a button.
        Some("AXStaticText" | "AXImage") if n.clickable => "AXButton",
        Some(role) => role,
        None if n.checkable => "AXCheckBox",
        None if n.clickable => "AXButton",
        None if n.scrollable => "AXScrollArea",
        None => "AXGroup",
    }
}

/// The tree in points of the screen's current orientation. The accessibility frames of a turned screen come in the
/// device's own portrait points (seen 2026-11-22 on an iPad in landscape: Settings' icon at portrait (758, 1182) is
/// drawn at (194, 758)); a tree whose largest frame is portrait-shaped while the screen is landscape is turned the
/// way the picture is. A tree that is landscape already is kept. `screen` is the current orientation's size.
pub fn in_orientation(root: &UiNode, screen: Size, o: Orientation) -> UiNode {
    let quarter = matches!(o, Orientation::Rot90 | Orientation::Rot270);
    if !quarter || screen.w <= screen.h || !portrait_frames(root, screen) {
        return root.clone();
    }
    // The portrait screen: the current one turned back.
    let portrait = screen.turned();
    let mut out = root.clone();
    turn(&mut out, portrait, o);
    out
}

/// Whether the frames are the portrait screen's: the widest frame is taller than wide, or a frame reaches below
/// the landscape screen's foot.
fn portrait_frames(root: &UiNode, screen: Size) -> bool {
    let r = root.frame;
    if !r.is_empty() && r.h > r.w {
        return true;
    }
    fn below(n: &UiNode, foot: f64) -> bool {
        n.frame.y + n.frame.h > foot || n.children.iter().any(|c| below(c, foot))
    }
    below(root, screen.h + 1.0)
}

/// One frame and its children from portrait points to the screen turned `o` (the inverse of
/// [`crate::geom::to_portrait_ratio`]'s turn).
fn turn(n: &mut UiNode, portrait: Size, o: Orientation) {
    let f = n.frame;
    let (w, h) = (portrait.w, portrait.h);
    n.frame = match o {
        // Shown turned clockwise: portrait (px, py) is drawn at (h - py, px).
        Orientation::Rot90 => Rect {
            x: h - (f.y + f.h),
            y: f.x,
            w: f.h,
            h: f.w,
        },
        Orientation::Rot270 => Rect {
            x: f.y,
            y: w - (f.x + f.w),
            w: f.h,
            h: f.w,
        },
        _ => f,
    };
    for c in &mut n.children {
        turn(c, portrait, o);
    }
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

    fn role(class: &str) -> &'static str {
        android_role(AndroidNode {
            class,
            ..AndroidNode::default()
        })
    }

    /// Rule 1360: Android classes map onto the tree model's roles, so the filter treats both kinds the same.
    #[test]
    fn android_classes_map_to_the_tree_roles() {
        for (class, want) in [
            ("android.widget.Button", "AXButton"),
            ("android.widget.ImageButton", "AXButton"),
            (
                "com.google.android.material.button.MaterialButton",
                "AXButton",
            ),
            (
                "com.google.android.material.floatingactionbutton.FloatingActionButton",
                "AXButton",
            ),
            ("android.widget.EditText", "AXTextField"),
            (
                "com.google.android.material.textfield.TextInputEditText",
                "AXTextField",
            ),
            ("android.widget.AutoCompleteTextView", "AXTextField"),
            ("android.widget.Switch", "AXSwitch"),
            ("androidx.appcompat.widget.SwitchCompat", "AXSwitch"),
            ("android.widget.ToggleButton", "AXSwitch"),
            ("android.widget.CheckBox", "AXCheckBox"),
            ("android.widget.RadioButton", "AXCheckBox"),
            ("android.widget.CheckedTextView", "AXCheckBox"),
            ("android.widget.SeekBar", "AXSlider"),
            ("android.widget.Spinner", "AXPopUpButton"),
            (
                "com.google.android.material.tabs.TabLayout$TabView",
                "AXTab",
            ),
            ("android.widget.TextView", "AXStaticText"),
            (
                "androidx.appcompat.widget.AppCompatTextView",
                "AXStaticText",
            ),
            ("android.widget.ImageView", "AXImage"),
            ("androidx.recyclerview.widget.RecyclerView", "AXScrollArea"),
            ("androidx.core.widget.NestedScrollView", "AXScrollArea"),
            ("android.webkit.WebView", "AXWebArea"),
            ("android.widget.FrameLayout", "AXGroup"),
            ("android.view.ViewGroup", "AXGroup"),
            ("android.view.View", "AXGroup"),
            ("", "AXGroup"),
        ] {
            assert_eq!(role(class), want, "{class}");
        }
        let with = |class, f: fn(&mut AndroidNode<'_>)| {
            let mut n = AndroidNode {
                class,
                ..AndroidNode::default()
            };
            f(&mut n);
            android_role(n)
        };
        // Flags decide what the class does not: Compose nodes are all `android.view.View`.
        assert_eq!(
            with("android.view.View", |n| n.clickable = true),
            "AXButton"
        );
        assert_eq!(
            with("android.view.View", |n| {
                n.clickable = true;
                n.checkable = true;
            }),
            "AXCheckBox",
            "a Compose check box is clickable too"
        );
        assert_eq!(
            with("android.view.View", |n| n.editable = true),
            "AXTextField"
        );
        assert_eq!(
            with("android.view.View", |n| n.scrollable = true),
            "AXScrollArea"
        );
        // A tappable row, text or picture is a button; a known control keeps its role when it is clickable.
        assert_eq!(
            with("android.widget.LinearLayout", |n| n.clickable = true),
            "AXButton"
        );
        assert_eq!(
            with("android.widget.TextView", |n| n.clickable = true),
            "AXButton"
        );
        assert_eq!(
            with("android.widget.ImageView", |n| n.clickable = true),
            "AXButton"
        );
        assert_eq!(
            with("android.widget.Switch", |n| n.clickable = true),
            "AXSwitch"
        );
        // Every role a known class gets is one the iOS tree uses; the actionable ones are listed without a label.
        for (_, r) in ANDROID_ROLES {
            assert!(r.starts_with("AX"), "{r}");
        }
        for class in [
            "android.widget.Button",
            "android.widget.EditText",
            "android.widget.Switch",
            "android.widget.CheckBox",
            "android.widget.SeekBar",
            "android.widget.Spinner",
        ] {
            assert!(actionable(role(class)), "{class}");
        }
    }

    /// An Android tree in dp, as the emu host gives it, filters like an iOS one: an icon button with no label is
    /// listed, the full-screen frame and the label-less layouts are not.
    #[test]
    fn an_android_tree_filters_like_an_ios_one() {
        let screen = Size::new(411.0, 914.0);
        let at = |class, clickable, label: &str, f: (f64, f64, f64, f64), kids| {
            node(
                android_role(AndroidNode {
                    class,
                    clickable,
                    ..AndroidNode::default()
                }),
                label,
                f,
                kids,
            )
        };
        let root = at(
            "android.widget.FrameLayout",
            false,
            "",
            (0.0, 0.0, 411.0, 914.0),
            vec![at(
                "android.widget.LinearLayout",
                false,
                "",
                (0.0, 80.0, 411.0, 400.0),
                vec![
                    at(
                        "android.widget.TextView",
                        false,
                        "Settings",
                        (16.0, 90.0, 200.0, 32.0),
                        vec![],
                    ),
                    at(
                        "android.widget.ImageButton",
                        true,
                        "",
                        (355.0, 90.0, 48.0, 48.0),
                        vec![],
                    ),
                    at(
                        "android.widget.LinearLayout",
                        true,
                        "",
                        (0.0, 150.0, 411.0, 72.0),
                        vec![at(
                            "android.widget.TextView",
                            false,
                            "Network & internet",
                            (72.0, 160.0, 300.0, 24.0),
                            vec![],
                        )],
                    ),
                ],
            )],
        );
        let l = filter_tree(&root, screen);
        assert_eq!(
            l.refs
                .iter()
                .map(|r| (r.role.as_str(), r.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("AXStaticText", "Settings"),
                ("AXButton", ""),
                ("AXButton", ""),
                ("AXStaticText", "Network & internet"),
            ]
        );
        assert_eq!(l.refs[0].r, "d1");
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

    /// Max, 2026-11-22: on an iPad in landscape a ref tap was "off the screen": the frames were portrait points.
    #[test]
    fn a_landscape_tree_in_portrait_points_is_turned_with_the_picture() {
        let screen = Size::new(1376.0, 1032.0);
        let root = node(
            "AXApplication",
            "",
            (0.0, 0.0, 1032.0, 1376.0),
            vec![node(
                "AXButton",
                "Settings",
                (710.0, 1146.0, 96.0, 72.0),
                vec![],
            )],
        );
        let t = in_orientation(&root, screen, Orientation::Rot90);
        assert_eq!(
            t.frame,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1376.0,
                h: 1032.0
            }
        );
        // Drawn at (194, 758), where the picture shows it.
        assert_eq!(t.children[0].frame.center(), (194.0, 758.0));
        let l = filter_tree(&t, screen);
        assert_eq!(labels(&l), ["Settings"]);
        // Turned the other way, the same point goes to the opposite side.
        let u = in_orientation(&root, screen, Orientation::Rot270);
        assert_eq!(u.children[0].frame.center(), (1182.0, 1032.0 - 758.0));
        // Each corner stays on the screen.
        for f in [t.frame, u.frame] {
            assert!(f.x >= 0.0 && f.y >= 0.0 && f.x + f.w <= 1376.0 && f.y + f.h <= 1032.0);
        }
    }

    #[test]
    fn a_tree_in_landscape_points_or_a_portrait_screen_is_kept() {
        let wide = node(
            "AXApplication",
            "",
            (0.0, 0.0, 1376.0, 1032.0),
            vec![node("AXButton", "Go", (100.0, 900.0, 80.0, 40.0), vec![])],
        );
        let l = Size::new(1376.0, 1032.0);
        assert_eq!(in_orientation(&wide, l, Orientation::Rot90), wide);
        let tall = node("AXApplication", "", (0.0, 0.0, 1032.0, 1376.0), vec![]);
        assert_eq!(in_orientation(&tall, l.turned(), Orientation::Rot0), tall);
        assert_eq!(in_orientation(&tall, l.turned(), Orientation::Rot180), tall);
    }
}
