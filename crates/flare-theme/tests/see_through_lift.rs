//! Task 57 step 4: a light theme at see-through 70 without acrylic lifts its red and yellow against the
//! ground the eye really sees (the ground blended with an assumed mid-grey desktop).

use flare_theme::ansi::{contrast_ratio, luminance};
use flare_theme::colour::{parse_colour, to_stored, Rgba};
use flare_theme::colour_contrast::{
    composite, effective_ground, lift_ansi_for_see_through, ASSUMED_DESKTOP,
};
use flare_theme::ground::LIGHT_GROUND;
use flare_theme::preset::presets;
use flare_theme::resolve::resolve_preset;

#[test]
fn a_solid_window_changes_nothing() {
    for p in presets() {
        let t = resolve_preset(p);
        let lifted = lift_ansi_for_see_through(&t.ansi, p.bg, 1.0);
        assert_eq!(lifted, t.ansi, "{} changed at alpha 1", p.id);
    }
}

#[test]
fn the_effective_ground_is_the_ground_over_the_desktop() {
    let g = Rgba {
        r: 255.0,
        g: 255.0,
        b: 255.0,
        a: 1.0,
    };
    let e = effective_ground(&g, 0.3, &ASSUMED_DESKTOP);
    assert_eq!(to_stored(&e), "#a6a6a6");
    assert_eq!(
        to_stored(&effective_ground(&g, 1.0, &ASSUMED_DESKTOP)),
        "#ffffff"
    );
}

#[test]
fn a_light_theme_at_70_lifts_red_and_yellow_to_3_to_1_on_grey() {
    let mut checked = 0;
    for p in presets().iter().filter(|p| luminance(p.bg) > LIGHT_GROUND) {
        let t = resolve_preset(p);
        let lifted = lift_ansi_for_see_through(&t.ansi, p.bg, 0.3);
        let g = parse_colour(p.bg).unwrap();
        let eff = to_stored(&effective_ground(&g, 0.3, &ASSUMED_DESKTOP));
        for i in [1, 3, 9, 11] {
            let c = lifted[i].as_deref().unwrap();
            let seen = composite(c, &eff);
            assert!(
                contrast_ratio(&seen, &eff) >= 3.0,
                "{} ansi {i}: {c} on {eff}",
                p.id
            );
        }
        checked += 1;
    }
    assert!(checked >= 3);
}

#[test]
fn dark_themes_are_untouched() {
    for p in presets().iter().filter(|p| luminance(p.bg) <= LIGHT_GROUND) {
        let t = resolve_preset(p);
        assert_eq!(
            lift_ansi_for_see_through(&t.ansi, p.bg, 0.3),
            t.ansi,
            "{}",
            p.id
        );
    }
}
