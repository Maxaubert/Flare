//! The macOS font list (port phase 2, step 4).
//!
//! macOS has no registry of font files, so the list comes from the font folders themselves: the
//! system's (`/System/Library/Fonts`, which holds `Supplemental`), the downloadable system fonts
//! under `/System/Library/AssetsV2/com_apple_MobileAsset_Font*`, the machine's `/Library/Fonts` and
//! the user's `~/Library/Fonts`. fontdb reads only each file's name table (the files are mapped,
//! not read whole): about 900 faces in 35 ms on a warm Mac. `/Network/Library/Fonts` is left out
//! on purpose: it is an autofs mount point, where even a listing can wait on the network.
//!
//! Flare ships no font of its own, so the terminal falls to SF Mono (when the user installed it)
//! and then Menlo, which every macOS has.

use super::FaceFile;
use cosmic_text::fontdb;
use std::path::{Path, PathBuf};

/// The families loaded at start-up next to the user's stack, in order of preference: the system
/// monospace faces, then every face cosmic-text's macOS fallback names (its per-script faces, its
/// Han faces for each locale and its common fallbacks), then symbols and older monospace faces.
///
/// The script faces are loaded here and not lazily, unlike on Windows: Arial Unicode MS, a common
/// fallback every macOS has, covers most scripts, so with the script faces left out it would draw
/// them instead of the face cosmic-text prefers. They are small (about 11 MB of files together),
/// and only their name tables are read until a glyph is drawn.
pub const FALLBACK_FAMILIES: &[&str] = &[
    "SF Mono",
    "Menlo",
    // cosmic-text's per-script faces, in its order.
    "Noto Sans Adlam",
    "Geeza Pro",
    "Noto Sans Armenian",
    "Bangla Sangam MN",
    "Noto Sans Buhid",
    "Euphemia UCAS",
    "Noto Sans Chakma",
    "Devanagari Sangam MN",
    "Kefa",
    "Noto Sans Gothic",
    "Grantha Sangam MN",
    "Gujarati Sangam MN",
    "Gurmukhi Sangam MN",
    "Noto Sans Hanunoo",
    "Arial",
    "Noto Sans Javanese",
    "Noto Sans Kannada",
    "Khmer Sangam MN",
    "Lao Sangam MN",
    "Malayalam Sangam MN",
    "Noto Sans Mongolian",
    "Noto Sans Myanmar",
    "Noto Sans Oriya",
    "Sinhala Sangam MN",
    "Noto Sans Syriac",
    "Noto Sans Tagalog",
    "Noto Sans Tagbanwa",
    "Noto Sans Tai Le",
    "Noto Sans Tai Tham",
    "Noto Sans Tai Viet",
    "InaiMathi",
    "Telugu Sangam MN",
    "Noto Sans Thaana",
    "Ayuthaya",
    "Kailasa",
    "Noto Sans Tifinagh",
    "Noto Sans Vai",
    "Noto Sans Yi",
    // Its Han faces (PingFang is a private system face on recent macOS, so usually absent).
    "PingFang SC",
    "PingFang TC",
    "PingFang HK",
    "Hiragino Sans",
    "Apple SD Gothic Neo",
    // Its common fallbacks.
    ".SF NS",
    "Apple Color Emoji",
    "Geneva",
    "Arial Unicode MS",
    // Symbols and the older monospace faces. Apple Braille comes before Apple Symbols, which also
    // has the braille cells: no face cosmic-text names covers U+2800..U+28FF, so the all-fonts
    // build takes the first face in load order that does, and in /System/Library/Fonts that is
    // Apple Braille.ttf (the folder lists it before Apple Symbols.ttf). Loaded lazily, it was
    // never asked for, since Apple Symbols already drew the cell (CI run 42520841627, macOS 26).
    // Its five files are about 140 KB.
    "Apple Braille",
    "Apple Symbols",
    "Monaco",
    "Courier New",
];

/// Families loaded one at a time, only when a character nothing else covers asks for them: the
/// faces of scripts cosmic-text names no face for, and maths.
pub const LAZY_FALLBACK_FAMILIES: &[&str] = &[
    "Noto Sans Egyptian Hieroglyphs",
    "Noto Sans Cuneiform",
    "Noto Sans Linear A",
    "Noto Sans Linear B",
    "Noto Sans Old Italic",
    "Noto Sans Phoenician",
    "Noto Sans Glagolitic",
    "Noto Sans Coptic",
    "Noto Sans NKo",
    "Noto Sans Samaritan",
    "Noto Sans Bamum",
    "Noto Sans Cham",
    "Noto Sans Lisu",
    "Noto Sans Ol Chiki",
    "Noto Sans Sundanese",
    "Noto Sans Osage",
    "Noto Sans Meetei Mayek",
    "Noto Sans New Tai Lue",
    "Noto Sans Lepcha",
    "Noto Sans Limbu",
    "Noto Sans Saurashtra",
    "Noto Sans Rejang",
    "Noto Sans Kayah Li",
    "Noto Sans Buginese",
    "Noto Sans Batak",
    "Noto Serif Balinese",
    "Noto Serif Ahom",
    "Plantagenet Cherokee",
    "Kefa III",
    "STIX Two Math",
];

/// PT's `TERM_FONTS` 'cascadia' stack with the macOS system monospace faces in Consolas' place
/// (re-exported as `raster::DEFAULT_FAMILIES`).
pub const DEFAULT_FAMILIES: &[&str] = &["Cascadia Mono", "JetBrainsMono NF", "SF Mono", "Menlo"];

/// The faces the generic `monospace` at the end of every stack stands for, in order.
pub const MONOSPACE_FAMILIES: &[&str] = &["SF Mono", "Menlo", "Monaco"];

/// The font folders, in the order they are read (a later copy of a file is not read again).
pub fn font_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/System/Library/Fonts")];
    // sync-ok: once per process, from the cached `installed_faces`
    if let Ok(assets) = std::fs::read_dir("/System/Library/AssetsV2") {
        let mut found: Vec<PathBuf> = assets
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("com_apple_MobileAsset_Font")
            })
            .map(|e| e.path())
            .collect();
        found.sort();
        dirs.extend(found);
    }
    dirs.push(PathBuf::from("/Library/Fonts"));
    if let Some(home) = home {
        dirs.push(home.join("Library/Fonts"));
    }
    dirs
}

/// Every installed font file, from the font folders.
pub fn installed_faces() -> Vec<FaceFile> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute());
    faces_from_dirs(&font_dirs(home.as_deref()))
}

/// The pure half of `installed_faces`: the faces in these folders (each walked to the bottom). A
/// file that is not a font, or cannot be parsed, is skipped; a folder that does not exist does
/// not matter. A face gets one entry per family name it carries (its English name first, then
/// its localised ones), with its index in a collection.
pub fn faces_from_dirs(dirs: &[PathBuf]) -> Vec<FaceFile> {
    let mut db = fontdb::Database::new();
    for dir in dirs {
        db.load_fonts_dir(dir);
    }
    let mut out = Vec::new();
    for face in db.faces() {
        let path = match &face.source {
            fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => p.clone(),
            fontdb::Source::Binary(_) => continue,
        };
        for (family, _) in &face.families {
            let entry = FaceFile {
                family: family.clone(),
                path: path.clone(),
                index: face.index,
            };
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_catalog::files_for_family;

    #[test]
    fn the_font_folders_are_the_system_machine_and_user_ones_and_never_the_network() {
        let dirs = font_dirs(Some(Path::new("/Users/someone")));
        assert_eq!(dirs[0], Path::new("/System/Library/Fonts"));
        assert!(dirs.contains(&PathBuf::from("/Library/Fonts")));
        assert_eq!(
            dirs.last().map(PathBuf::as_path),
            Some(Path::new("/Users/someone/Library/Fonts"))
        );
        assert!(dirs.iter().all(|d| !d.starts_with("/Network")));
        assert!(!font_dirs(None).iter().any(|d| d.starts_with("/Users")));
    }

    #[test]
    fn faces_come_from_the_files_names_not_their_file_names() {
        let menlo = Path::new("/System/Library/Fonts/Menlo.ttc");
        if !menlo.exists() {
            eprintln!("skipped: no Menlo on this machine");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("deeper");
        std::fs::create_dir(&nested).unwrap();
        std::fs::copy(menlo, nested.join("renamed.ttc")).unwrap();
        std::fs::write(dir.path().join("broken.ttf"), b"not a font").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        let faces = faces_from_dirs(&[dir.path().to_path_buf(), dir.path().join("no-such-folder")]);
        assert!(!faces.is_empty());
        assert!(faces.iter().all(|f| f.path == nested.join("renamed.ttc")));
        assert!(faces.iter().all(|f| f.family == "Menlo"), "{faces:?}");
        // Menlo.ttc holds four styles: one entry per face, each with its own index.
        let mut indices: Vec<u32> = faces.iter().map(|f| f.index).collect();
        indices.sort_unstable();
        assert_eq!(indices, [0, 1, 2, 3]);
        assert_eq!(files_for_family(&faces, "menlo").len(), 1);
        assert!(files_for_family(&faces, "Men").is_empty());
    }

    #[test]
    fn the_real_font_folders_hold_the_default_and_the_fallbacks() {
        let faces = installed_faces();
        assert!(faces.len() > 100, "{} faces", faces.len());
        // Menlo and Apple Color Emoji ship with every macOS.
        assert!(!files_for_family(&faces, "Menlo").is_empty());
        assert!(!files_for_family(&faces, "Apple Color Emoji").is_empty());
        let cjk = ["PingFang SC", "Hiragino Sans", "Apple SD Gothic Neo"];
        assert!(
            cjk.iter().any(|f| !files_for_family(&faces, f).is_empty()),
            "no CJK fallback is installed"
        );
        assert!(
            FALLBACK_FAMILIES
                .iter()
                .filter(|f| !files_for_family(&faces, f).is_empty())
                .count()
                >= 5
        );
    }
}
