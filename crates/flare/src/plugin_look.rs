//! A plugin's own look and its details sheet (2026-11-08, `docs/surfaces/plugins.md` "Brand" and "The details
//! sheet"). The same rules for every plugin: the brand's colour and logo from its manifest go only on the surfaces it
//! owns (its sheet, its questions, its menu item), on the fixed dark brand surface (`ui/plugin_look.slint`); a plugin
//! with no brand wears Flare's theme and the chip. The sheet's tiles come from the manifest and the permission
//! catalog (`flare_mods::grants`), so a plugin gets them by asking for permissions, with nothing of its own to draw.

use flare_mods::grants::{Choices, Tile, TileKind, Tone};
use flare_mods::hub::Shown;
use flare_mods::manifest::{Brand, Manifest, hex_rgb};
use slint::{Color, ModelRc, VecModel};

use crate::{PluginLook, SheetOption, SheetTile};

/// How far toward white the lighter tint of a brand colour goes (links, the focus ring, the usual level).
const HI_MIX: f32 = 0.35;

/// Flare's chip glyph: the logo of a plugin with no brand.
fn chip() -> &'static str {
    flare_settings::icons::icon_path("chip").unwrap_or("")
}

/// The brand colour and its lighter tint.
pub fn brand_colours(color: &str) -> (Color, Color) {
    let [r, g, b] = hex_rgb(color).unwrap_or([0, 0, 0]);
    let hi = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * HI_MIX).round() as u8;
    (
        Color::from_rgb_u8(r, g, b),
        Color::from_rgb_u8(hi(r), hi(g), hi(b)),
    )
}

/// The look of a plugin's surfaces: its brand's, or Flare's own (with the chip) when it has none.
pub fn look_of(brand: Option<&Brand>) -> PluginLook {
    match brand {
        Some(b) => {
            let (accent, accent_hi) = brand_colours(&b.color);
            let logo = b.icon.as_deref();
            PluginLook {
                branded: true,
                accent,
                accent_hi,
                logo: logo.unwrap_or(chip()).into(),
                plate: logo.is_some(),
            }
        }
        None => PluginLook {
            branded: false,
            accent: Color::default(),
            accent_hi: Color::default(),
            logo: chip().into(),
            plate: false,
        },
    }
}

/// What the details sheet shows of one plugin.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub id: String,
    pub name: String,
    /// `version · author` (the version alone without an author).
    pub sub: String,
    pub desc: String,
    pub tiles: Vec<Tile>,
    pub brand: Option<Brand>,
    /// Whether it is on. Off, the sheet offers "Turn on", which approves what the sheet lists (`shown`).
    pub on: bool,
    /// What the sheet lists, as the hub compares it when the person turns the plugin on here: a tile per permission
    /// the manifest asks for (its words), the maker in the sub line and the homepage tile.
    pub shown: Shown,
}

/// The sheet of the plugin `m` with the person's choices `c`; `on` says whether it is on now.
pub fn sheet_of(m: &Manifest, c: &Choices, on: bool) -> Sheet {
    let sub = match &m.author {
        Some(a) => format!("{} \u{b7} {a}", m.version),
        None => m.version.clone(),
    };
    Sheet {
        id: m.id.clone(),
        name: m.name.clone(),
        sub,
        desc: m.description.clone(),
        tiles: flare_mods::grants::tiles(m, c),
        brand: m.brand.clone(),
        on,
        shown: Shown {
            permissions: m
                .permissions
                .iter()
                .map(|p| flare_mods::hub::permission_words(*p).to_string())
                .collect(),
            author: m.author.clone().unwrap_or_default(),
            homepage: m.homepage.clone().unwrap_or_default(),
        },
    }
}

fn tone(t: Tone) -> i32 {
    match t {
        Tone::Plain => 0,
        Tone::Brand => 1,
        Tone::Warn => 2,
    }
}

/// One tile as the sheet draws it.
pub fn slint_tile(t: &Tile) -> SheetTile {
    let mut out = SheetTile {
        key: t.key.as_str().into(),
        label: t.label.as_str().into(),
        caps: t.label.to_uppercase().into(),
        detail: t.detail.as_str().into(),
        words: t.words.as_str().into(),
        tip: t.tip.as_str().into(),
        tone: tone(t.tone()),
        ..SheetTile::default()
    };
    match &t.kind {
        TileKind::Toggle { on } => {
            out.kind = 0;
            out.on = *on;
        }
        TileKind::Choice { value, levels } => {
            out.kind = 1;
            out.value = (*value).into();
            let i = levels.iter().position(|l| l.id == *value).unwrap_or(0);
            out.chosen = i32::try_from(i).unwrap_or(0);
            out.value_name = levels.get(i).map_or("", |l| l.name).into();
            let opts: Vec<SheetOption> = levels
                .iter()
                .map(|l| SheetOption {
                    id: l.id.into(),
                    name: l.name.into(),
                    tone: tone(l.tone),
                })
                .collect();
            out.options = ModelRc::new(VecModel::from(opts));
        }
        TileKind::Fact { value } => {
            out.kind = 2;
            out.value = value.as_str().into();
            out.value_name = value.as_str().into();
        }
    }
    out
}

/// Puts the sheet on the Settings bridge (open), or takes it down (`None`).
pub fn show_sheet(b: &crate::SettingsBridge<'_>, sheet: Option<&Sheet>) {
    let Some(s) = sheet else {
        b.set_sheet_open(false);
        b.set_sheet_tiles(ModelRc::new(VecModel::<SheetTile>::default()));
        return;
    };
    let tiles: Vec<SheetTile> = s.tiles.iter().map(slint_tile).collect();
    b.set_sheet_name(s.name.as_str().into());
    b.set_sheet_sub(s.sub.as_str().into());
    b.set_sheet_desc(s.desc.as_str().into());
    b.set_sheet_tall(s.tiles.iter().any(|t| !t.detail.is_empty()));
    b.set_sheet_look(look_of(s.brand.as_ref()));
    b.set_sheet_tiles(ModelRc::new(VecModel::from(tiles)));
    b.set_sheet_can_turn_on(!s.on);
    b.set_sheet_open(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(brand: &str) -> Manifest {
        flare_mods::manifest::parse_plugin(&format!(
            r#"{{"id":"vault","name":"Vault","version":"0.2.0","protocol":2,"description":"Fills logins.","author":"someone",
            "permissions":["tools","process","fill"],"helper":{{"command":"bin/vault.exe"}},
            "helpers":{{"macos":{{"command":"bin/vault"}}}},
            "tools":[{{"name":"fill_login","description":"d","input_schema":{{"type":"object"}}}}]{brand}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_brand_sets_its_colour_a_lighter_tint_and_its_logo_on_a_plate() {
        let m = vault(r##","brand":{"color":"#175ddc","icon":"M3 3h18v18H3z"}"##);
        let l = look_of(m.brand.as_ref());
        assert!(l.branded && l.plate);
        assert_eq!(l.accent, Color::from_rgb_u8(0x17, 0x5d, 0xdc));
        // About the mockup's #6a99f0.
        assert_eq!(l.accent_hi, Color::from_rgb_u8(104, 150, 232));
        assert_eq!(l.logo, "M3 3h18v18H3z");
        // A brand with no logo keeps the colour and draws the chip; no brand is Flare's own look.
        let l = look_of(vault(r##","brand":{"color":"#175ddc"}"##).brand.as_ref());
        assert!(l.branded && !l.plate && l.logo == chip());
        let l = look_of(None);
        assert!(!l.branded && !l.plate && l.logo == chip());
    }

    #[test]
    fn the_sheet_has_its_header_and_tiles_from_the_manifest() {
        let s = sheet_of(&vault(""), &Choices::default(), false);
        assert_eq!(s.name, "Vault");
        assert_eq!(s.sub, "0.2.0 \u{b7} someone");
        let t: Vec<SheetTile> = s.tiles.iter().map(slint_tile).collect();
        assert_eq!(t[0].caps, "ACCESS");
        assert_eq!(
            (t[0].kind, t[0].value_name.as_str(), t[0].tone),
            (1, "Full control", 2)
        );
        assert_eq!(
            (t[1].kind, t[1].value_name.as_str(), t[1].tone),
            (1, "Ask each time", 1)
        );
        assert_eq!(t[1].chosen, 0);
        assert_eq!(
            (t[2].kind, t[2].on, t[2].detail.as_str()),
            (0, true, "fill_login")
        );
        assert_eq!((t[3].kind, t[3].value_name.as_str()), (2, "vault"));
        // Off, it can be turned on here, and what the sheet lists is what that approves.
        assert!(!s.on);
        assert_eq!(s.shown.author, "someone");
        assert_eq!(
            s.shown.permissions,
            ["tools", "process", "fill"]
                .iter()
                .map(|p| flare_mods::hub::permission_words(
                    flare_ctl::Permission::parse(p).unwrap()
                ))
                .collect::<Vec<_>>()
        );
    }
}
