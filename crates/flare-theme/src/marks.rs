//! Mode, rainbow, mark colours and name ink (Task 11). Source: PT `core/renderer/lib/{nameInk,markColours,
//! markPalette}.ts` and their tests, and the mode part of `termReplies.ts`.

use crate::ansi::{contrast_ratio, luminance, mix_hex, toward_room};
use crate::colour::opaque;
use crate::colour_contrast::composite;
use crate::ground::{DEFAULT_GROUND, LIGHT_GROUND, ON_DARK_INK};

/// Light or dark, decided from the measured ground (regression rule 15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

/// The ground's mode: light when its WCAG luminance, with alpha dropped onto the default ground, is above
/// `LIGHT_GROUND` (PT `groundMode`). Never read off a name.
pub fn ground_mode(bg: &str) -> Mode {
    if luminance(&opaque(bg, DEFAULT_GROUND)) > LIGHT_GROUND {
        Mode::Light
    } else {
        Mode::Dark
    }
}

/// Dark when the ground's luminance is at or under `LIGHT_GROUND` (PT `groundIsDark`).
pub fn ground_is_dark(ground: &str) -> bool {
    luminance(ground) <= LIGHT_GROUND
}

/// Under this, the theme's text on a fill is gone and the name flips.
pub const NAME_FLIP: f64 = 2.0;

/// The icon's seven ribbon colours, in the order the ribbon runs (PT `ICON_RAINBOW`).
pub const ICON_RAINBOW: [&str; 7] = [
    "#12cee5", "#2179fa", "#945af5", "#e84fd2", "#fb7f6c", "#f0a934", "#e8d021",
];

/// The axis a rainbow gradient runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

/// The four state colours a mark can take, as the theme gives them.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkColours {
    pub working: String,
    pub finished: String,
    pub question: String,
    pub failed: String,
}

/// One colour per mark state (`done` is the finished state).
#[derive(Clone, Debug, PartialEq)]
pub struct MarkSet {
    pub working: String,
    pub done: String,
    pub question: String,
    pub failed: String,
}

/// Every colour a strip's marks wear (PT `markPalette`).
#[derive(Clone, Debug, PartialEq)]
pub struct MarkPalette {
    pub line: MarkSet,
    pub rainbow_x: String,
    pub rainbow_y: String,
    pub rainbow_stops: Vec<String>,
    pub fill: MarkSet,
    pub ink: MarkSet,
    pub overlay_run: String,
}

/// The name's ink on a single fill: the theme's text when it reads at 2:1 or better, else the opposite
/// (ON_DARK_INK on a dark ground, white on a light one). PT `nameInk`.
pub fn name_ink(fill: &str, text: &str, dark_ground: bool) -> String {
    if contrast_ratio(text, fill) >= NAME_FLIP {
        return text.to_string();
    }
    if dark_ground {
        ON_DARK_INK.to_string()
    } else {
        "#ffffff".to_string()
    }
}

/// The 28 colours a rainbow name can sit on: each stop and its blends at 0, 25, 50 and 75 per cent toward
/// the next, wrapping (PT `RAINBOW_SAMPLES`).
fn rainbow_samples() -> Vec<String> {
    let n = ICON_RAINBOW.len();
    ICON_RAINBOW
        .iter()
        .enumerate()
        .flat_map(|(i, c)| {
            let next = ICON_RAINBOW[(i + 1) % n];
            [0.0, 0.25, 0.5, 0.75].map(|t| mix_hex(c, next, t))
        })
        .collect()
}

fn worst_on_rainbow(ink: &str) -> f64 {
    rainbow_samples()
        .iter()
        .map(|c| contrast_ratio(ink, c))
        .fold(f64::INFINITY, f64::min)
}

/// The name's ink on the flowing rainbow, judged once from the WORST colour under it (PT `rainbowInk`).
pub fn rainbow_ink(text: &str, dark_ground: bool) -> String {
    let keep = worst_on_rainbow(text);
    if keep >= NAME_FLIP {
        return text.to_string();
    }
    let opposite = if dark_ground { ON_DARK_INK } else { "#ffffff" };
    if worst_on_rainbow(opposite) > keep {
        opposite.to_string()
    } else {
        text.to_string()
    }
}

/// The lowest contrast of `c` over every ground it can sit on.
fn worst_on(c: &str, grounds: &[&str]) -> f64 {
    grounds
        .iter()
        .map(|g| contrast_ratio(c, g))
        .fold(f64::INFINITY, f64::min)
}

/// `c` moved toward white or black, the least 2% steps that clear `floor` on every ground; the nearest step
/// when none does (PT `floorMark`).
pub fn floor_mark(c: &str, grounds: &[&str], floor: f64) -> String {
    if grounds.is_empty() || worst_on(c, grounds) >= floor {
        return c.to_string();
    }
    let dark = grounds.iter().fold(grounds[0], |a, g| {
        if contrast_ratio("#ffffff", g) < contrast_ratio("#ffffff", a) {
            *g
        } else {
            a
        }
    });
    let to = toward_room(dark);
    let mut best = c.to_string();
    let mut best_score = worst_on(c, grounds);
    for i in 1..=50 {
        let m = mix_hex(c, to, f64::from(i) * 0.02);
        let score = worst_on(&m, grounds);
        if score >= floor {
            return m;
        }
        if score > best_score {
            best = m;
            best_score = score;
        }
    }
    best
}

/// The seven ribbon colours, each floored on these grounds (PT `rainbowOn`).
pub fn rainbow_on(grounds: &[&str], floor: f64) -> Vec<String> {
    ICON_RAINBOW
        .iter()
        .map(|c| floor_mark(c, grounds, floor))
        .collect()
}

/// A gradient that loops: its first colour is repeated at the end (PT `rainbowGradient`).
pub fn rainbow_gradient<S: AsRef<str>>(cols: &[S], axis: Axis) -> String {
    let dir = match axis {
        Axis::X => "90deg",
        Axis::Y => "180deg",
    };
    let mut stops: Vec<&str> = cols.iter().map(AsRef::as_ref).collect();
    if let Some(first) = stops.first().copied() {
        stops.push(first);
    }
    format!("linear-gradient({dir}, {})", stops.join(", "))
}

/// The icon's seven, raw, looping across a whole finished fill (PT `rainbowFillX`).
pub fn rainbow_fill_x() -> String {
    rainbow_gradient(&ICON_RAINBOW[..], Axis::X)
}

/// A see-through colour as the opaque colour it shows over the solid ground (PT `opaqueOver`).
pub fn opaque_over(c: &str, ground: &str) -> String {
    composite(c, ground)
}

/// Every colour a strip's marks wear (PT `markPalette`). The line colours are the state colours made
/// opaque and floored to 3:1 on every ground; the fills are solid; the inks follow the name rule.
pub fn mark_palette(
    colours: &MarkColours,
    grounds: &[&str],
    solid_ground: &str,
    text: &str,
    rainbow: bool,
) -> MarkPalette {
    let solid = MarkSet {
        working: opaque_over(&colours.working, solid_ground),
        done: opaque_over(&colours.finished, solid_ground),
        question: opaque_over(&colours.question, solid_ground),
        failed: opaque_over(&colours.failed, solid_ground),
    };
    let line = MarkSet {
        working: floor_mark(&solid.working, grounds, 3.0),
        done: floor_mark(&solid.done, grounds, 3.0),
        question: floor_mark(&solid.question, grounds, 3.0),
        failed: floor_mark(&solid.failed, grounds, 3.0),
    };
    let seven = rainbow_on(grounds, 3.0);
    let fill = MarkSet {
        working: solid.working.clone(),
        done: if rainbow {
            rainbow_fill_x()
        } else {
            solid.done.clone()
        },
        question: solid.question.clone(),
        failed: solid.failed.clone(),
    };
    let dark = ground_is_dark(solid_ground);
    let ink = MarkSet {
        working: name_ink(&solid.working, text, dark),
        done: if rainbow {
            rainbow_ink(text, dark)
        } else {
            name_ink(&solid.done, text, dark)
        },
        question: name_ink(&solid.question, text, dark),
        failed: name_ink(&solid.failed, text, dark),
    };
    MarkPalette {
        line,
        rainbow_x: rainbow_gradient(&seven[..], Axis::X),
        rainbow_y: rainbow_gradient(&seven[..], Axis::Y),
        rainbow_stops: seven,
        overlay_run: ink.working.clone(),
        fill,
        ink,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::{contrast_ratio, mix_hex};
    use crate::colour::{parse_colour, rgb_to_hsl};
    use crate::ground::ON_DARK_INK;
    use crate::preset::find_preset;
    use crate::testutil::fixture;

    const GROUND: &str = "#050706";
    const TEXT: &str = "#eef2e6";
    const PAPER_GROUND: &str = "#f6f4ee";
    const PAPER_TEXT: &str = "#2a2620";

    fn volt_colours() -> MarkColours {
        MarkColours {
            working: "#d8ff26".into(),
            finished: "#22c55e".into(),
            question: "#3b82f6".into(),
            failed: "#ff3b5c".into(),
        }
    }

    fn paper_colours() -> MarkColours {
        MarkColours {
            working: "#3f8ccb".into(),
            finished: "#1b9a49".into(),
            question: "#3b82f6".into(),
            failed: "#e05561".into(),
        }
    }

    fn volt(rainbow: bool) -> MarkPalette {
        mark_palette(&volt_colours(), &[GROUND], GROUND, TEXT, rainbow)
    }

    fn paper(rainbow: bool) -> MarkPalette {
        mark_palette(
            &paper_colours(),
            &[PAPER_GROUND],
            PAPER_GROUND,
            PAPER_TEXT,
            rainbow,
        )
    }

    fn volt_grounds() -> [String; 3] {
        [
            GROUND.to_string(),
            mix_hex(GROUND, TEXT, 0.04),
            mix_hex(GROUND, TEXT, 0.11),
        ]
    }

    fn paper_grounds() -> [String; 3] {
        [
            PAPER_GROUND.to_string(),
            mix_hex(PAPER_GROUND, PAPER_TEXT, 0.04),
            mix_hex(PAPER_GROUND, PAPER_TEXT, 0.11),
        ]
    }

    fn refs(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    fn worst(c: &str, grounds: &[&str]) -> f64 {
        grounds
            .iter()
            .map(|g| contrast_ratio(c, g))
            .fold(f64::INFINITY, f64::min)
    }

    fn is_hex6(s: &str) -> bool {
        s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
    }

    fn grey(n: i32) -> String {
        format!("#{n:02x}{n:02x}{n:02x}")
    }

    fn hue_gap(a: f64, b: f64) -> f64 {
        let d = (a - b).rem_euclid(360.0);
        d.min(360.0 - d)
    }

    #[test]
    fn the_rainbow_is_the_icons_seven_in_order() {
        assert_eq!(
            ICON_RAINBOW,
            [
                "#12cee5", "#2179fa", "#945af5", "#e84fd2", "#fb7f6c", "#f0a934", "#e8d021"
            ]
        );
    }

    #[test]
    fn volt_keeps_every_rainbow_colour_and_clears_3_8() {
        let g = volt_grounds();
        let grounds = refs(&g);
        for (i, c) in ICON_RAINBOW.iter().enumerate() {
            assert_eq!(floor_mark(c, &grounds, 3.0), *c, "stop {i}");
        }
        let seven = rainbow_on(&grounds, 3.0);
        let lowest = seven
            .iter()
            .map(|c| worst(c, &grounds))
            .fold(f64::INFINITY, f64::min);
        assert!(lowest > 3.8, "lowest {lowest}");
    }

    #[test]
    fn paper_darkens_what_falls_short_keeping_the_hue() {
        let g = paper_grounds();
        let grounds = refs(&g);
        let floored = rainbow_on(&grounds, 3.0);
        for (i, c) in floored.iter().enumerate() {
            assert!(worst(c, &grounds) >= 3.0, "stop {i} {c}");
            let before = rgb_to_hsl(&parse_colour(ICON_RAINBOW[i]).unwrap()).h;
            let after = rgb_to_hsl(&parse_colour(c).unwrap()).h;
            assert!(
                hue_gap(before, after) <= 2.0,
                "stop {i}: {before} vs {after}"
            );
        }
        let closest = floored
            .iter()
            .map(|c| worst(c, &grounds))
            .fold(f64::INFINITY, f64::min);
        assert!(closest < 3.2, "closest {closest}");
    }

    #[test]
    fn a_short_colour_on_a_dark_mid_grey_is_lightened() {
        let c = floor_mark("#2179fa", &["#383c44"], 3.0);
        assert!(contrast_ratio(&c, "#383c44") >= 3.0);
        let r = parse_colour(&c).unwrap().r;
        assert!(r > f64::from(0x21), "red channel {r}");
    }

    #[test]
    fn an_unreachable_floor_returns_the_nearest_and_never_throws() {
        let input = "#7a7a7a";
        let out = floor_mark(input, &["#777777"], 21.0);
        assert!(contrast_ratio(&out, "#777777") > contrast_ratio(input, "#777777"));
    }

    #[test]
    fn gradients_loop_along_x_or_down_y() {
        assert_eq!(
            rainbow_gradient(&["#111111", "#222222"], Axis::X),
            "linear-gradient(90deg, #111111, #222222, #111111)"
        );
        assert!(
            rainbow_gradient(&["#111111", "#222222"], Axis::Y)
                .starts_with("linear-gradient(180deg, ")
        );
    }

    #[test]
    fn the_icons_seven_raw_loop_across_a_whole_fill() {
        assert_eq!(
            rainbow_fill_x(),
            "linear-gradient(90deg, #12cee5, #2179fa, #945af5, #e84fd2, #fb7f6c, #f0a934, #e8d021, #12cee5)"
        );
    }

    #[test]
    fn a_see_through_colour_is_made_opaque_over_the_solid_ground() {
        assert_eq!(
            opaque_over("#d8ff2680", GROUND),
            mix_hex(GROUND, "#d8ff26", f64::from(0x80_u8) / 255.0)
        );
        assert_eq!(opaque_over("#d8ff26", GROUND), "#d8ff26");
    }

    #[test]
    fn volt_bright_marks_stay_and_lines_floor_to_3() {
        let p = volt(true);
        assert_eq!(p.line.working, "#d8ff26");
        for c in [
            &p.line.working,
            &p.line.done,
            &p.line.question,
            &p.line.failed,
        ] {
            assert!(contrast_ratio(c, GROUND) >= 3.0, "{c}");
        }
    }

    #[test]
    fn volt_full_inks_working_and_rainbow_flip() {
        let p = volt(true);
        assert_eq!(
            p.ink,
            MarkSet {
                working: ON_DARK_INK.into(),
                done: ON_DARK_INK.into(),
                question: TEXT.into(),
                failed: TEXT.into(),
            }
        );
        assert_eq!(volt(false).ink.done, TEXT);
    }

    #[test]
    fn paper_keeps_its_text_on_every_fill() {
        let p = paper(true);
        assert_eq!(p.ink.working, PAPER_TEXT);
        assert_eq!(p.ink.done, PAPER_TEXT);
        assert_eq!(p.ink.question, PAPER_TEXT);
        assert_eq!(p.ink.failed, PAPER_TEXT);
    }

    #[test]
    fn finished_is_the_icons_seven_while_the_rainbow_is_on() {
        let on = volt(true).fill.done;
        assert!(on.starts_with("linear-gradient(90deg, "));
        for c in ICON_RAINBOW {
            assert!(on.contains(c), "{c}");
        }
        assert_eq!(volt(false).fill.done, "#22c55e");
    }

    #[test]
    fn fills_are_solid_a_half_alpha_working_is_laid_on_the_ground() {
        let mut colours = volt_colours();
        colours.working = "#d8ff2680".into();
        let p = mark_palette(&colours, &[GROUND], GROUND, TEXT, true);
        assert!(is_hex6(&p.fill.working), "{}", p.fill.working);
        assert_eq!(
            p.fill.working,
            mix_hex(GROUND, "#d8ff26", f64::from(0x80_u8) / 255.0)
        );
    }

    #[test]
    fn classic_run_on_a_working_fill_is_the_names_ink() {
        assert_eq!(volt(true).overlay_run, ON_DARK_INK);
        assert_eq!(volt(true).overlay_run, volt(true).ink.working);
        assert_eq!(paper(true).overlay_run, PAPER_TEXT);
    }

    #[test]
    fn both_rainbow_gradients_loop() {
        let p = volt(true);
        assert!(p.rainbow_x.starts_with("linear-gradient(90deg, #12cee5"));
        assert!(p.rainbow_y.starts_with("linear-gradient(180deg, #12cee5"));
        assert!(p.rainbow_x.ends_with("#12cee5)"));
    }

    #[test]
    fn on_volt_only_the_yellow_green_working_fill_flips() {
        assert_eq!(name_ink("#d8ff26", TEXT, true), ON_DARK_INK);
        assert_eq!(name_ink("#3b82f6", TEXT, true), TEXT);
        assert_eq!(name_ink("#ff3b5c", TEXT, true), TEXT);
        assert_eq!(name_ink("#383c44", TEXT, true), TEXT);
    }

    #[test]
    fn flips_on_a_very_pale_question_fill_and_white_on_a_light_themes_dark_working() {
        assert_eq!(name_ink("#cfe4ff", TEXT, true), ON_DARK_INK);
        assert_eq!(name_ink("#2a2c30", "#1d1f1a", false), "#ffffff");
        assert_eq!(name_ink("#2563eb", "#1d1f1a", false), "#1d1f1a");
    }

    #[test]
    fn the_rule_is_under_2_to_1() {
        let mut g = 0xc0_i32;
        while contrast_ratio("#ffffff", &grey(g)) < 2.0 {
            g -= 1;
        }
        let fill = grey(g);
        assert!(contrast_ratio("#ffffff", &fill) >= 2.0);
        assert_eq!(name_ink(&fill, "#ffffff", true), "#ffffff");
    }

    #[test]
    fn the_rainbow_is_judged_on_its_worst_colour() {
        assert_eq!(rainbow_ink(TEXT, true), ON_DARK_INK);
        assert_eq!(rainbow_ink(PAPER_TEXT, false), PAPER_TEXT);
    }

    #[test]
    fn the_rainbow_keeps_the_text_when_the_opposite_is_no_better() {
        let ink = rainbow_ink("#808080", true);
        assert!(ink == "#808080" || ink == ON_DARK_INK, "{ink}");
    }

    #[test]
    fn dark_or_light_comes_from_the_ground_never_from_the_text() {
        assert_eq!(name_ink("#f0f0f0", "#ffffff", true), ON_DARK_INK);
        assert_eq!(name_ink("#f0f0f0", "#ffffff", false), "#ffffff");
        assert!(ground_is_dark(GROUND));
        assert!(!ground_is_dark(PAPER_GROUND));
    }

    #[test]
    fn ground_mode_is_measured_never_named() {
        let mode_of = |id: &str| ground_mode(find_preset(id).expect(id).bg);
        assert_eq!(mode_of("fawn"), Mode::Light);
        assert_eq!(mode_of("paper"), Mode::Light);
        assert_eq!(mode_of("pt-default"), Mode::Dark);
        assert_eq!(mode_of("prism"), Mode::Dark);
        assert_eq!(ground_mode("#e6d8c040"), Mode::Light);
    }

    #[test]
    fn oracle_mark_vectors() {
        let v = fixture("mark_vectors.json");
        let stops: Vec<String> = v["rainbow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        assert_eq!(stops, ICON_RAINBOW.map(String::from));

        let grounds_of = |k: &str| -> Vec<String> {
            match k {
                "volt" => volt_grounds().to_vec(),
                "paper" => paper_grounds().to_vec(),
                _ => vec!["#383c44".to_string()],
            }
        };
        for k in ["volt", "paper", "cinder"] {
            let g = grounds_of(k);
            let gr = refs(&g);
            let f = &v["floored"][k];
            let got: Vec<String> = rainbow_on(&gr, 3.0);
            assert_eq!(json_strings(&f["rainbowOn"]), got, "{k} rainbowOn");
            let each: Vec<String> = ICON_RAINBOW
                .iter()
                .map(|c| floor_mark(c, &gr, 3.0))
                .collect();
            assert_eq!(json_strings(&f["each"]), each, "{k} each");
        }

        for (k, colours, ground, text) in [
            ("volt", volt_colours(), GROUND, TEXT),
            ("paper", paper_colours(), PAPER_GROUND, PAPER_TEXT),
        ] {
            let rows = v["palettes"][k].as_array().unwrap();
            for (i, rainbow) in [true, false].into_iter().enumerate() {
                let p = mark_palette(&colours, &[ground], ground, text, rainbow);
                let want = &rows[i];
                assert_eq!(p.line, set_of(&want["line"]), "{k}[{i}] line");
                assert_eq!(p.fill, set_of(&want["fill"]), "{k}[{i}] fill");
                assert_eq!(p.ink, set_of(&want["ink"]), "{k}[{i}] ink");
                assert_eq!(
                    p.rainbow_x,
                    want["rainbowX"].as_str().unwrap(),
                    "{k}[{i}] rainbowX"
                );
                assert_eq!(
                    p.rainbow_y,
                    want["rainbowY"].as_str().unwrap(),
                    "{k}[{i}] rainbowY"
                );
                assert_eq!(
                    p.overlay_run,
                    want["overlay"]["run"].as_str().unwrap(),
                    "{k}[{i}] run"
                );
            }
        }

        for row in v["nameInk"].as_array().unwrap() {
            let got = name_ink(
                row["fill"].as_str().unwrap(),
                row["text"].as_str().unwrap(),
                row["dark"].as_bool().unwrap(),
            );
            assert_eq!(got, row["ink"].as_str().unwrap(), "nameInk {row}");
        }
        for row in v["rainbowInk"].as_array().unwrap() {
            let got = rainbow_ink(
                row["text"].as_str().unwrap(),
                row["dark"].as_bool().unwrap(),
            );
            assert_eq!(got, row["ink"].as_str().unwrap(), "rainbowInk {row}");
        }
    }

    fn json_strings(v: &serde_json::Value) -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    }

    fn set_of(v: &serde_json::Value) -> MarkSet {
        let s = |k: &str| v[k].as_str().unwrap().to_string();
        MarkSet {
            working: s("working"),
            done: s("done"),
            question: s("question"),
            failed: s("failed"),
        }
    }
}
