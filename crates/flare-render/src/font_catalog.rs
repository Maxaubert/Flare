//! The fonts the terminal needs, found once (Task 62).
//!
//! `FontSystem::new()` makes cosmic-text open every installed font, which costs seconds of disk and
//! tens of MB of mapped pages. The terminal needs the font stack the user chose plus a short list
//! of fallbacks, so this module reads the registry's font list (no directory walk), picks the files
//! for the named families and builds a `fontdb::Database` from just those.

use cosmic_text::fontdb;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One installed font file, as the registry lists it. `family` is the registry display name with
/// its type suffix removed (for a collection, one entry per family it holds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceFile {
    pub family: String,
    pub path: PathBuf,
    pub index: u32,
}

/// The families loaded at start-up next to the user's stack, in order of preference.
pub const FALLBACK_FAMILIES: &[&str] = &[
    "Cascadia Mono",
    "Consolas",
    "Segoe UI Emoji",
    "Segoe UI Symbol",
    "Segoe UI",
    "Segoe UI Historic",
    "MS Gothic",
    "Microsoft YaHei UI",
    "Malgun Gothic",
    "Yu Gothic UI",
    "Nirmala UI",
    "Arial Unicode MS",
    "Lucida Console",
    "Courier New",
];

/// Families loaded one at a time, only when a character nothing else covers asks for them (the
/// per-script lists cosmic-text's Windows fallback names).
pub const LAZY_FALLBACK_FAMILIES: &[&str] = &[
    "Ebrima",
    "Gadugi",
    "Javanese Text",
    "Leelawadee UI",
    "Mongolian Baiti",
    "Myanmar Text",
    "MV Boli",
    "Microsoft Himalaya",
    "Microsoft Yi Baiti",
    "Microsoft JhengHei UI",
    "MingLiU_HKSCS",
    "Yu Gothic",
    "Cambria Math",
    "Segoe UI Variable",
];

/// Words that follow a family name in a registry display name.
const STYLE_WORDS: &[&str] = &[
    "regular",
    "bold",
    "italic",
    "oblique",
    "light",
    "semilight",
    "semibold",
    "extrabold",
    "extralight",
    "black",
    "thin",
    "medium",
    "demi",
    "demibold",
    "heavy",
    "book",
    "condensed",
    "narrow",
    "semicondensed",
    "extracondensed",
    "ultralight",
    "ultrabold",
    "retina",
];

/// Every installed font file, from the registry's two font lists (system and user). Cached.
pub fn installed_faces() -> Vec<FaceFile> {
    static FACES: OnceLock<Vec<FaceFile>> = OnceLock::new();
    FACES
        .get_or_init(|| {
            let windir = std::env::var_os("WINDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            let system_dir = windir.join("Fonts");
            let mut entries = registry::font_values(registry::Hive::Machine);
            entries.extend(registry::font_values(registry::Hive::User));
            faces_from_entries(&entries, &system_dir)
        })
        .clone()
}

/// The pure half of `installed_faces`: registry (name, file) pairs to font files. A relative file
/// is under `system_dir`. Nothing is read from the disk, the font folder is never listed.
pub fn faces_from_entries(entries: &[(String, String)], system_dir: &Path) -> Vec<FaceFile> {
    let mut out = Vec::new();
    for (name, file) in entries {
        let path = if Path::new(file).is_absolute() {
            PathBuf::from(file)
        } else {
            system_dir.join(file)
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !matches!(ext.as_str(), "ttf" | "ttc" | "otf" | "otc") {
            continue;
        }
        let display = strip_type_suffix(name);
        for (index, part) in display.split(" & ").enumerate() {
            out.push(FaceFile {
                family: part.trim().to_string(),
                path: path.clone(),
                index: index as u32,
            });
        }
    }
    out
}

fn strip_type_suffix(name: &str) -> &str {
    match name.rfind(" (") {
        Some(at) if name.ends_with(')') => &name[..at],
        _ => name,
    }
}

/// Whether a registry display name (`Cascadia Mono SemiBold Italic`) is `family` in some style.
fn is_family(display: &str, family: &str) -> bool {
    let display = display.trim();
    if display.len() < family.len() || !display.is_char_boundary(family.len()) {
        return false;
    }
    let (head, rest) = display.split_at(family.len());
    head.eq_ignore_ascii_case(family)
        && rest.split_whitespace().all(|word| {
            let word = word.to_ascii_lowercase();
            STYLE_WORDS.contains(&word.as_str())
        })
}

/// The font files that hold `family` (any style).
pub fn files_for_family(faces: &[FaceFile], family: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for face in faces {
        if is_family(&face.family, family) && !files.contains(&face.path) {
            files.push(face.path.clone());
        }
    }
    files
}

/// A database holding only the files of the named families (case-insensitive), `stack` first and
/// then `fallbacks`. A family that is not installed is skipped.
pub fn database_for(stack: &[&str], fallbacks: &[&str]) -> fontdb::Database {
    database_from(&installed_faces(), stack, fallbacks)
}

/// `database_for` over a given face list.
pub fn database_from(faces: &[FaceFile], stack: &[&str], fallbacks: &[&str]) -> fontdb::Database {
    let mut db = fontdb::Database::new();
    let mut loaded: Vec<PathBuf> = Vec::new();
    for family in stack.iter().chain(fallbacks.iter()) {
        for file in files_for_family(faces, family) {
            if loaded.contains(&file) {
                continue;
            }
            // A file that cannot be read is skipped: the family falls back to the next one.
            if db.load_font_file(&file).is_ok() {
                loaded.push(file);
            }
        }
    }
    db
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegEnumValueW,
        RegOpenKeyExW,
    };

    pub enum Hive {
        Machine,
        User,
    }

    /// The (name, data) string values of `...\Windows NT\CurrentVersion\Fonts`.
    pub fn font_values(hive: Hive) -> Vec<(String, String)> {
        let root = match hive {
            Hive::Machine => HKEY_LOCAL_MACHINE,
            Hive::User => HKEY_CURRENT_USER,
        };
        let path: Vec<u16> = r"Software\Microsoft\Windows NT\CurrentVersion\Fonts"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `path` is a NUL-terminated UTF-16 string and `key` a valid out pointer.
        let status = unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ, &mut key) };
        if status != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut index = 0u32;
        loop {
            let mut name = vec![0u16; 512];
            let mut name_len = name.len() as u32;
            let mut data = vec![0u8; 2048];
            let mut data_len = data.len() as u32;
            let mut kind = 0u32;
            // SAFETY: every buffer is as long as the length passed beside it, `kind` is a valid
            // out pointer, and `key` is the key opened above.
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            index += 1;
            if status == 259 {
                // ERROR_NO_MORE_ITEMS
                break;
            }
            if status != ERROR_SUCCESS || kind != REG_SZ {
                continue;
            }
            let wide: Vec<u16> = data[..data_len as usize]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&c| c != 0)
                .collect();
            out.push((
                String::from_utf16_lossy(&name[..name_len as usize]),
                String::from_utf16_lossy(&wide),
            ));
        }
        // SAFETY: `key` was opened above and is closed once.
        unsafe { RegCloseKey(key) };
        out
    }
}

#[cfg(not(windows))]
mod registry {
    pub enum Hive {
        Machine,
        User,
    }
    pub fn font_values(_hive: Hive) -> Vec<(String, String)> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, file: &str) -> (String, String) {
        (name.to_string(), file.to_string())
    }

    fn windows_fonts() -> PathBuf {
        PathBuf::from(r"C:\Windows\Fonts")
    }

    #[test]
    fn installed_faces_reads_the_registry_without_walking_the_font_folder() {
        // A folder holding a font file the fake registry does not list: it must not appear, and a
        // folder that does not exist must not matter.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("unlisted.ttf"), b"x").unwrap();
        let entries = vec![
            entry("Fake Mono Regular (TrueType)", "fakemono.ttf"),
            entry("Fake Mono Bold (TrueType)", "fakemonob.ttf"),
            entry("Some Setting", "notafont.dat"),
            entry("User Font (TrueType)", r"D:\fonts\user.otf"),
            entry("Pair One & Pair Two (TrueType)", "pair.ttc"),
        ];
        let faces = faces_from_entries(&entries, dir.path());
        assert!(faces.iter().all(|f| !f.path.ends_with("unlisted.ttf")));
        assert!(faces.iter().all(|f| !f.path.ends_with("notafont.dat")));
        assert!(
            faces
                .iter()
                .any(|f| f.family == "Fake Mono Regular"
                    && f.path == dir.path().join("fakemono.ttf"))
        );
        assert!(
            faces
                .iter()
                .any(|f| f.path == Path::new(r"D:\fonts\user.otf"))
        );
        let pair: Vec<&FaceFile> = faces
            .iter()
            .filter(|f| f.path.ends_with("pair.ttc"))
            .collect();
        assert_eq!(pair.len(), 2);
        assert_eq!(pair[1].family, "Pair Two");
        assert_eq!(pair[1].index, 1);
        assert_eq!(
            files_for_family(&faces, "fake mono").len(),
            2,
            "the family match ignores case and takes every style"
        );
        assert!(files_for_family(&faces, "Fake").is_empty());
        let missing = faces_from_entries(&entries, Path::new(r"Z:\no\such\folder"));
        assert_eq!(missing.len(), faces.len());
    }

    #[test]
    fn a_longer_family_name_is_not_taken_for_its_prefix() {
        let entries = vec![
            entry("Segoe UI (TrueType)", "segoeui.ttf"),
            entry("Segoe UI Semibold (TrueType)", "seguisb.ttf"),
            entry("Segoe UI Black Italic (TrueType)", "seguibli.ttf"),
            entry("Segoe UI Emoji (TrueType)", "seguiemj.ttf"),
            entry("Segoe UI Symbol (TrueType)", "seguisym.ttf"),
        ];
        let faces = faces_from_entries(&entries, &windows_fonts());
        let files = files_for_family(&faces, "Segoe UI");
        assert_eq!(files.len(), 3, "{files:?}");
        assert_eq!(files_for_family(&faces, "Segoe UI Emoji").len(), 1);
    }

    #[test]
    fn the_real_registry_lists_the_terminal_fonts() {
        let faces = installed_faces();
        if faces.is_empty() {
            eprintln!("skipped: no registry font list on this machine");
            return;
        }
        assert!(faces.len() > 20);
        assert!(!files_for_family(&faces, "Consolas").is_empty());
    }

    #[test]
    fn the_database_holds_only_the_named_faces_plus_the_fallbacks() {
        let faces = installed_faces();
        if files_for_family(&faces, "Arial").is_empty() {
            eprintln!("skipped: Arial is not installed");
            return;
        }
        let db = database_for(&["Arial"], FALLBACK_FAMILIES);
        // Every family in the database is the stack or one of the fallbacks (a collection can
        // bring its siblings, so the check is by file).
        let allowed: Vec<PathBuf> = ["Arial"]
            .iter()
            .chain(FALLBACK_FAMILIES.iter())
            .flat_map(|f| files_for_family(&faces, f))
            .collect();
        for face in db.faces() {
            let fontdb::Source::File(path) = &face.source else {
                panic!("a non-file source");
            };
            assert!(allowed.contains(path), "{path:?} was not asked for");
        }
        assert!(!db.is_empty());
        let stack_and_fallbacks = 1 + FALLBACK_FAMILIES.len();
        assert!(
            db.faces()
                .map(|f| f.families[0].0.to_ascii_lowercase())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                <= stack_and_fallbacks * 4,
            "far more families than named"
        );
        let all = installed_faces().len();
        assert!(db.len() < all, "{} faces of {all}", db.len());
    }

    #[test]
    fn a_family_that_is_not_installed_is_skipped() {
        let db = database_for(&["No Such Font Anywhere"], &[]);
        assert_eq!(db.len(), 0);
    }
}
