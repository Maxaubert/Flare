//! The colour format preference of the colour picker (M7 Task 12).
//!
//! Key `flare.term.colourFormat`: `hex`, `rgba` or `hsla`. Anything else (or nothing) reads as hex.
//! Display only: it changes nothing that is stored by the colour fields themselves.

use crate::kv::PrefStore;
use flare_theme::colour::ColourFormat;

pub const COLOUR_FORMAT_KEY: &str = "flare.term.colourFormat";

fn word(f: ColourFormat) -> &'static str {
    match f {
        ColourFormat::Hex => "hex",
        ColourFormat::Rgba => "rgba",
        ColourFormat::Hsla => "hsla",
    }
}

pub fn colour_format(s: &PrefStore) -> ColourFormat {
    match s.get(COLOUR_FORMAT_KEY).as_deref() {
        Some("rgba") => ColourFormat::Rgba,
        Some("hsla") => ColourFormat::Hsla,
        _ => ColourFormat::Hex,
    }
}

pub fn set_colour_format(s: &PrefStore, f: ColourFormat) {
    s.set(COLOUR_FORMAT_KEY, word(f));
}

/// Hex, then RGBA, then HSLA, then hex again. Returns the format now stored.
pub fn cycle_colour_format(s: &PrefStore) -> ColourFormat {
    let next = match colour_format(s) {
        ColourFormat::Hex => ColourFormat::Rgba,
        ColourFormat::Rgba => ColourFormat::Hsla,
        ColourFormat::Hsla => ColourFormat::Hex,
    };
    set_colour_format(s, next);
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::EnumPref;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn colour_format_opens_in_hex() {
        assert_eq!(colour_format(&PrefStore::memory()), ColourFormat::Hex);
    }

    #[test]
    fn junk_reads_hex() {
        let s = PrefStore::memory();
        s.set(COLOUR_FORMAT_KEY, "cmyk");
        assert_eq!(colour_format(&s), ColourFormat::Hex);
    }

    #[test]
    fn a_choice_is_remembered() {
        let s = PrefStore::memory();
        set_colour_format(&s, ColourFormat::Hsla);
        assert_eq!(colour_format(&s), ColourFormat::Hsla);
        assert_eq!(s.get(COLOUR_FORMAT_KEY), Some("hsla".to_string()));
    }

    #[test]
    fn the_format_cycles_hex_rgba_hsla_and_round() {
        let s = PrefStore::memory();
        assert_eq!(cycle_colour_format(&s), ColourFormat::Rgba);
        assert_eq!(cycle_colour_format(&s), ColourFormat::Hsla);
        assert_eq!(cycle_colour_format(&s), ColourFormat::Hex);
        assert_eq!(s.get(COLOUR_FORMAT_KEY), Some("hex".to_string()));
    }

    #[test]
    fn an_enum_pref_tells_its_listeners_and_lets_go() {
        let s = PrefStore::memory();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let sub = s.on_change(COLOUR_FORMAT_KEY, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        let p = EnumPref {
            key: COLOUR_FORMAT_KEY,
            allowed: &["hex", "rgba", "hsla"],
            fallback: "hex",
        };
        p.set(&s, "rgba");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        sub.cancel();
        p.set(&s, "hsla");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
