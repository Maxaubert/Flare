//! Previews: what a path may open as in a panel, and how an image or a text file is read.
//!
//! Every function that touches the disk here is for a worker thread (rule 34). The caller has
//! already re-resolved the path in the backend (rule 13); this module only decides by extension
//! and reads. A file with a runnable extension is never previewable: it is not in either list
//! (rule 14), and the caller also requires `flare_term::path_open::may_open` of the path and of
//! its canonical form.

use crate::kind::PanelKind;
use std::fmt;
use std::io::Read;
use std::path::Path;

/// The image types the viewer draws.
pub const IMAGE_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];

/// The text types the viewer shows. Each is also on the click allowlist (`may_open`); none is a
/// type whose default verb runs it.
pub const TEXT_EXTS: [&str; 38] = [
    "txt", "md", "markdown", "rst", "log", "json", "jsonc", "yaml", "yml", "toml", "csv", "tsv",
    "ini", "cfg", "conf", "xml", "lock", "diff", "patch", "rs", "ts", "tsx", "c", "h", "cpp",
    "hpp", "cc", "cs", "go", "java", "kt", "swift", "css", "scss", "sql", "slint", "vue", "svelte",
];

/// The most bytes of a text file shown (4 MB).
pub const TEXT_CAP: usize = 4 * 1024 * 1024;
/// The most lines kept of those bytes.
pub const LINES_MAX: usize = 100_000;
/// A line longer than this is cut (a minified bundle on one line would stall the layout).
pub const LINE_CHARS_MAX: usize = 2_000;
/// The largest image file read (64 MB).
pub const IMAGE_FILE_MAX: u64 = 64 * 1024 * 1024;
/// The most pixels an image may have (100 megapixels).
pub const IMAGE_PIXELS_MAX: u64 = 100_000_000;
/// A decoded image longer than this on a side is scaled down to it (a GPU texture limit).
pub const IMAGE_SIDE_MAX: u32 = 8192;
/// The longest side of a hover thumbnail.
pub const THUMB_SIDE: u32 = 256;
/// The largest image file a hover reads (16 MB).
pub const THUMB_FILE_MAX: u64 = 16 * 1024 * 1024;
/// The most pixels a picture may have for a hover thumbnail (25 megapixels).
pub const THUMB_PIXELS_MAX: u64 = 25_000_000;

/// The extension in lower case, when the name has one.
fn ext_of(path: &str) -> Option<String> {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let dot = name.rfind('.')?;
    let ext = &name[dot + 1..];
    (!ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// What a path of this name may be shown as: an image, a text file, or nothing.
pub fn classify(path: &str) -> Option<PanelKind> {
    let ext = ext_of(path)?;
    if IMAGE_EXTS.contains(&ext.as_str()) {
        Some(PanelKind::Image)
    } else if TEXT_EXTS.contains(&ext.as_str()) {
        Some(PanelKind::Text)
    } else {
        None
    }
}

/// The kind for a resolved path AND its canonical form: both must name the same previewable kind,
/// so a link called `a.png` that points at `run.exe` shows nothing.
pub fn classify_resolved(abs: &str, canonical: &str) -> Option<PanelKind> {
    let a = classify(abs)?;
    (classify(canonical)? == a).then_some(a)
}

/// True when a panel may load this path as `kind`: a path on a local drive (`C:\\...`, never a
/// network path, which would make Windows reach out), with no NUL, whose name is of that kind. The
/// controller asks this before it starts a read, for a target that came from a click and for one
/// that came back from `panels.json`, a file anyone can edit.
pub fn loadable(path: &str, kind: &PanelKind) -> bool {
    on_local_drive(path) && classify(path).as_ref() == Some(kind)
}

/// True for a path on a local drive (`C:\\...`, never a network path, which would make Windows reach out), with no NUL.
pub fn on_local_drive(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/')
        && !path.contains('\0')
}

/// Why a file could not be shown: plain words for the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    Missing,
    Denied,
    TooBig,
    NotText,
    NotAnImage,
    TooManyPixels,
    /// The user moved on while it was being read; never shown.
    Superseded,
    Other(String),
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::Missing => f.write_str("The file is gone."),
            Reason::Denied => f.write_str("Windows will not let Flare read this file."),
            Reason::TooBig => f.write_str("The file is too big to show."),
            Reason::NotText => f.write_str("This file is not text."),
            Reason::NotAnImage => f.write_str("Flare cannot read this picture."),
            Reason::TooManyPixels => f.write_str("The picture is too large to show."),
            Reason::Superseded => f.write_str("Opening…"),
            Reason::Other(m) => write!(f, "The file could not be read ({m})."),
        }
    }
}

fn io_reason(e: &std::io::Error) -> Reason {
    match e.kind() {
        std::io::ErrorKind::NotFound => Reason::Missing,
        std::io::ErrorKind::PermissionDenied => Reason::Denied,
        _ => Reason::Other(e.kind().to_string()),
    }
}

/// A text file ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextDoc {
    pub lines: Vec<String>,
    /// More of the file exists than is shown (the byte cap or the line cap).
    pub truncated: bool,
    /// Which cap cut it, when one did.
    pub cut: Option<Cut>,
    /// The file's real length (the bytes read are capped).
    pub bytes: u64,
}

/// The cap that cut a text file short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    Bytes,
    Lines,
}

/// Text from bytes: a UTF-16 file (a BOM, as Windows PowerShell 5.1 writes for `>`) is decoded as
/// such, a UTF-8 BOM is dropped, and any other byte shows as a replacement mark.
fn decode_text(buf: &[u8]) -> std::borrow::Cow<'_, str> {
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        std::borrow::Cow::Owned(String::from_utf16_lossy(&units))
    };
    match buf {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest),
        _ => String::from_utf8_lossy(buf),
    }
}

/// Reads up to `cap` bytes of a text file. Bytes that are not UTF-8 show as replacement marks; a
/// file with a NUL in its first 8 KB is not text.
pub fn read_text(path: &Path, cap: usize) -> Result<TextDoc, Reason> {
    // sync-ok: runs on a worker thread only
    let file = std::fs::File::open(path).map_err(|e| io_reason(&e))?;
    let meta = file.metadata().map_err(|e| io_reason(&e))?;
    if !meta.is_file() {
        return Err(Reason::Missing);
    }
    let mut buf = Vec::new();
    // One byte more than the cap says whether the file goes on.
    file.take(cap as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| io_reason(&e))?;
    let more = buf.len() > cap;
    if more {
        buf.truncate(cap);
    }
    let bom16 = matches!(buf.as_slice(), [0xFF, 0xFE, ..] | [0xFE, 0xFF, ..]);
    if !bom16 && buf.iter().take(8192).any(|b| *b == 0) {
        return Err(Reason::NotText);
    }
    Ok(doc_of(&decode_text(&buf), more, meta.len()))
}

/// The rows of `text`: at most `LINES_MAX`, each shaped for drawing. `more` says a byte cap already
/// cut the text short; `bytes` is the real length to show in the header.
fn doc_of(text: &str, more: bool, bytes: u64) -> TextDoc {
    let mut lines = Vec::new();
    let mut cut = more.then_some(Cut::Bytes);
    for raw in text.lines() {
        if lines.len() >= LINES_MAX {
            cut = Some(Cut::Lines);
            break;
        }
        lines.push(shape_line(raw));
    }
    TextDoc {
        lines,
        truncated: cut.is_some(),
        cut,
        bytes,
    }
}

/// A piece of text the app was handed (not a file): the first `cap` bytes of it, cut on a character
/// boundary, shaped like a file's. Nothing touches the disk, so it may run anywhere.
pub fn text_doc_from_str(text: &str, cap: usize) -> TextDoc {
    let more = text.len() > cap;
    let mut end = text.len().min(cap);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    doc_of(&text[..end], more, text.len() as u64)
}

/// A line as drawn: tabs become four spaces and a very long line is cut with a mark.
fn shape_line(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(LINE_CHARS_MAX + 4));
    let mut n = 0;
    for c in raw.chars() {
        if n >= LINE_CHARS_MAX {
            out.push('\u{2026}');
            break;
        }
        if c == '\t' {
            out.push_str("    ");
            n += 4;
        } else if c.is_control() {
            out.push('\u{fffd}');
            n += 1;
        } else {
            out.push(c);
            n += 1;
        }
    }
    out
}

/// A decoded picture: straight (not premultiplied) RGBA, 8 bits a channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Some pixel is not fully opaque: the viewer draws a checkerboard under it.
    pub has_alpha: bool,
    /// The picture was larger than `IMAGE_SIDE_MAX` and was scaled down.
    pub scaled: bool,
}

/// A hover thumbnail: straight RGBA, longest side at most `THUMB_SIDE`, and the size of the picture it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedThumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// The picture's own width, as its camera held it.
    pub source_width: u32,
    /// The picture's own height, as its camera held it.
    pub source_height: u32,
}

/// Reads an image file for a hover thumbnail, with limits below the panel's: pointing at a path is not asking for
/// the file. Same stages and same `stale` as [`decode_image`]; an animation gives its first frame.
pub fn decode_thumb(path: &Path, stale: &dyn Fn() -> bool) -> Result<DecodedThumb, Reason> {
    let img = read_oriented(path, THUMB_FILE_MAX, THUMB_PIXELS_MAX, stale)?;
    let (source_width, source_height) = (img.width(), img.height());
    if stale() {
        return Err(Reason::Superseded);
    }
    let img = if source_width > THUMB_SIDE || source_height > THUMB_SIDE {
        img.thumbnail(THUMB_SIDE, THUMB_SIDE)
    } else {
        img
    };
    let rgba = img.into_rgba8();
    Ok(DecodedThumb {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        source_width,
        source_height,
    })
}

/// Reads and decodes an image file. `stale` is asked between the stages (size, decode, resize): a
/// true answer abandons the work, so a picture the user has left costs no more memory.
pub fn decode_image(path: &Path, stale: &dyn Fn() -> bool) -> Result<Decoded, Reason> {
    let img = read_oriented(path, IMAGE_FILE_MAX, IMAGE_PIXELS_MAX, stale)?;
    let (w, h) = (img.width(), img.height());
    if stale() {
        return Err(Reason::Superseded);
    }
    let mut scaled = false;
    let img = if w > IMAGE_SIDE_MAX || h > IMAGE_SIDE_MAX {
        scaled = true;
        img.resize(
            IMAGE_SIDE_MAX,
            IMAGE_SIDE_MAX,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.into_rgba8();
    let has_alpha = rgba.pixels().any(|p| p.0[3] < 255);
    Ok(Decoded {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        has_alpha,
        scaled,
    })
}

/// The checks and the decode both readers share: the file's size, the picture's pixels from its header (nothing is
/// decoded over the limit), then the pixels the way the camera held them.
fn read_oriented(
    path: &Path,
    file_max: u64,
    pixels_max: u64,
    stale: &dyn Fn() -> bool,
) -> Result<image::DynamicImage, Reason> {
    // sync-ok: runs on a worker thread only
    let meta = std::fs::metadata(path).map_err(|e| io_reason(&e))?;
    if !meta.is_file() {
        return Err(Reason::Missing);
    }
    if meta.len() > file_max {
        return Err(Reason::TooBig);
    }
    let open = || {
        image::ImageReader::open(path)
            .and_then(image::ImageReader::with_guessed_format)
            .map_err(|e| io_reason(&e))
    };
    let (w, h) = open()?.into_dimensions().map_err(|_| Reason::NotAnImage)?;
    if u64::from(w) * u64::from(h) > pixels_max {
        return Err(Reason::TooManyPixels);
    }
    if stale() {
        return Err(Reason::Superseded);
    }
    // A picture too big for the decoder's memory limit (a large 16-bit one) is too large, not unreadable.
    let not_read = |e: image::ImageError| match e {
        image::ImageError::Limits(_) => Reason::TooManyPixels,
        _ => Reason::NotAnImage,
    };
    // A photo is drawn the way its camera held it (its EXIF orientation), not as the sensor stored it.
    let mut decoder = open()?.into_decoder().map_err(not_read)?;
    let orientation = image::ImageDecoder::orientation(&mut decoder)
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(not_read)?;
    img.apply_orientation(orientation);
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_decide_the_kind_in_any_case() {
        assert_eq!(classify(r"C:\a\Photo.PNG"), Some(PanelKind::Image));
        assert_eq!(classify("x/y.jpeg"), Some(PanelKind::Image));
        assert_eq!(classify("notes.md"), Some(PanelKind::Text));
        assert_eq!(classify("main.RS"), Some(PanelKind::Text));
        for ext in IMAGE_EXTS {
            assert_eq!(
                classify(&format!("a.{ext}")),
                Some(PanelKind::Image),
                "{ext}"
            );
        }
        for ext in TEXT_EXTS {
            assert_eq!(
                classify(&format!("a.{ext}")),
                Some(PanelKind::Text),
                "{ext}"
            );
        }
    }

    #[test]
    fn a_runnable_or_unknown_file_has_no_kind() {
        // Rule 14: nothing that a default verb runs, and nothing unknown.
        for name in [
            "run.bat", "run.cmd", "a.exe", "a.ps1", "a.py", "a.js", "a.sh", "a.vbs", "a.lnk",
            "a.msi", "a.com", "a.scr", "a.dll", "a.jar", "a.hta", "a.reg", "a.url", "a.svg",
            "a.pdf", "a.zip", "noext", "dot.", ".hidden",
        ] {
            assert_eq!(classify(name), None, "{name}");
        }
    }

    #[test]
    fn a_piece_of_text_is_shaped_like_a_file_and_cut_on_a_character_boundary() {
        let d = text_doc_from_str("a\tb\nsecond\n", 1000);
        assert_eq!(d.lines, ["a    b", "second"]);
        assert!(!d.truncated && d.cut.is_none());
        assert_eq!(d.bytes, 11);
        // The cap falls inside a two-byte character: the cut backs up to its start.
        let d = text_doc_from_str("ab\u{e9}cd", 3);
        assert_eq!(d.lines, ["ab"]);
        assert_eq!(d.cut, Some(Cut::Bytes));
        assert_eq!(d.bytes, 6);
        let many = "x\n".repeat(LINES_MAX + 5);
        let d = text_doc_from_str(&many, usize::MAX);
        assert_eq!(d.lines.len(), LINES_MAX);
        assert_eq!(d.cut, Some(Cut::Lines));
        assert!(text_doc_from_str("", 10).lines.is_empty());
    }

    #[test]
    fn a_link_that_changes_kind_shows_nothing() {
        assert_eq!(classify_resolved("a.png", "b.png"), Some(PanelKind::Image));
        assert_eq!(classify_resolved("a.png", "run.exe"), None);
        assert_eq!(classify_resolved("a.png", "b.txt"), None);
        assert_eq!(classify_resolved("a.txt", "b.md"), Some(PanelKind::Text));
        assert_eq!(classify_resolved("a.exe", "b.png"), None);
    }

    #[test]
    fn only_a_local_file_of_the_right_kind_is_loadable() {
        let img = PanelKind::Image;
        let txt = PanelKind::Text;
        assert!(loadable(r"C:\a\p.png", &img));
        assert!(loadable("d:/a/p.JPG", &img));
        assert!(loadable(r"C:\a\n.md", &txt));
        assert!(!loadable(r"C:\a\n.md", &img), "the wrong kind");
        assert!(!loadable(r"C:\a\run.exe", &img));
        for bad in [
            r"\\server\share\p.png",
            "//server/share/p.png",
            r"\\?\C:\p.png",
            r"\\.\C:\p.png",
            "p.png",
            r"a\p.png",
            "C:p.png",
            "C:\\p\0.png",
            "",
        ] {
            assert!(!loadable(bad, &img), "{bad:?}");
        }
    }

    #[test]
    fn the_lists_have_no_duplicates() {
        for list in [&IMAGE_EXTS[..], &TEXT_EXTS[..]] {
            let mut v = list.to_vec();
            v.sort_unstable();
            v.dedup();
            assert_eq!(v.len(), list.len());
        }
        assert!(IMAGE_EXTS.iter().all(|e| !TEXT_EXTS.contains(e)));
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap(); // sync-ok: test code only
        p
    }

    #[test]
    fn text_is_read_into_lines_with_tabs_expanded() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "a.txt", b"one\r\n\ttwo\nthree");
        let doc = read_text(&p, TEXT_CAP).unwrap();
        assert_eq!(doc.lines, ["one", "    two", "three"]);
        assert!(!doc.truncated);
        assert_eq!(doc.bytes, 15);
    }

    #[test]
    fn an_empty_file_is_no_lines() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "e.txt", b"");
        let doc = read_text(&p, TEXT_CAP).unwrap();
        assert!(doc.lines.is_empty() && !doc.truncated);
    }

    #[test]
    fn text_over_the_cap_is_cut_and_says_so() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "big.txt", &b"abcdefghij\n".repeat(100));
        let doc = read_text(&p, 55).unwrap();
        assert!(doc.truncated);
        assert_eq!(doc.bytes, 1100, "the file's own length");
        assert_eq!(doc.cut, Some(Cut::Bytes));
        assert_eq!(doc.lines.len(), 5);
        let exact = read_text(&p, 1100).unwrap();
        assert!(!exact.truncated, "a file exactly at the cap is whole");
    }

    #[test]
    fn a_file_with_many_lines_is_cut_by_lines_not_bytes() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "many.txt", &b"x\n".repeat(LINES_MAX + 50));
        let doc = read_text(&p, TEXT_CAP).unwrap();
        assert_eq!(doc.lines.len(), LINES_MAX);
        assert_eq!(doc.cut, Some(Cut::Lines));
    }

    #[test]
    fn a_utf16_file_with_a_bom_is_text() {
        // Windows PowerShell 5.1 writes this for `>` and Out-File.
        let d = tempfile::tempdir().unwrap();
        let mut le = vec![0xFF, 0xFE];
        for u in "one\r\ntwo \u{e6}".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        let p = write(d.path(), "le.txt", &le);
        assert_eq!(
            read_text(&p, TEXT_CAP).unwrap().lines,
            ["one", "two \u{e6}"]
        );
        let mut be = vec![0xFE, 0xFF];
        for u in "hi".encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        let p = write(d.path(), "be.txt", &be);
        assert_eq!(read_text(&p, TEXT_CAP).unwrap().lines, ["hi"]);
        let p = write(d.path(), "bom8.txt", b"\xEF\xBB\xBFhello");
        assert_eq!(read_text(&p, TEXT_CAP).unwrap().lines, ["hello"]);
        let p = write(d.path(), "bin.txt", b"ab\0cd");
        assert_eq!(read_text(&p, TEXT_CAP), Err(Reason::NotText));
    }

    #[test]
    fn a_stale_decode_is_abandoned() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save(&p)
            .unwrap();
        assert_eq!(decode_image(&p, &|| true), Err(Reason::Superseded));
        assert!(decode_image(&p, &|| false).is_ok());
    }

    #[test]
    fn the_real_cap_is_four_megabytes() {
        assert_eq!(TEXT_CAP, 4_194_304);
    }

    #[test]
    fn text_with_a_nul_is_not_text() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "b.txt", b"ab\0cd");
        assert_eq!(read_text(&p, TEXT_CAP), Err(Reason::NotText));
    }

    #[test]
    fn bad_utf8_shows_replacement_marks_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "l.txt", b"caf\xe9 ok");
        let doc = read_text(&p, TEXT_CAP).unwrap();
        assert_eq!(doc.lines, ["caf\u{fffd} ok"]);
    }

    #[test]
    fn a_long_line_is_cut_and_a_control_character_is_marked() {
        let d = tempfile::tempdir().unwrap();
        let long = "x".repeat(LINE_CHARS_MAX + 500);
        let p = write(d.path(), "m.txt", format!("{long}\na\u{1b}b").as_bytes());
        let doc = read_text(&p, TEXT_CAP).unwrap();
        assert_eq!(doc.lines[0].chars().count(), LINE_CHARS_MAX + 1);
        assert!(doc.lines[0].ends_with('\u{2026}'));
        assert_eq!(doc.lines[1], "a\u{fffd}b");
    }

    #[test]
    fn too_many_lines_are_cut() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "n.txt", "\n".repeat(LINES_MAX + 10).as_bytes());
        let doc = read_text(&p, TEXT_CAP).unwrap();
        assert_eq!(doc.lines.len(), LINES_MAX);
        assert!(doc.truncated);
    }

    #[test]
    fn a_missing_file_and_a_folder_answer_with_reasons() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            read_text(&d.path().join("nope.txt"), TEXT_CAP),
            Err(Reason::Missing)
        );
        // Opening a folder as a file is refused by Windows; either way it is a reason, not a crash.
        assert!(read_text(d.path(), TEXT_CAP).is_err());
        assert!(decode_image(&d.path().join("nope.png"), &|| false).is_err());
    }

    #[test]
    fn every_reason_has_words_and_no_em_dash() {
        for r in [
            Reason::Missing,
            Reason::Denied,
            Reason::TooBig,
            Reason::NotText,
            Reason::NotAnImage,
            Reason::TooManyPixels,
            Reason::Other("x".into()),
        ] {
            let s = r.to_string();
            assert!(!s.is_empty() && !s.contains('\u{2014}'), "{s}");
        }
    }

    fn png_bytes(w: u32, h: u32, alpha: u8) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for p in img.pixels_mut() {
            *p = image::Rgba([200, 30, 40, alpha]);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_png_decodes_to_rgba() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "a.png", &png_bytes(3, 2, 255));
        let got = decode_image(&p, &|| false).unwrap();
        assert_eq!((got.width, got.height), (3, 2));
        assert_eq!(got.rgba.len(), 3 * 2 * 4);
        assert_eq!(&got.rgba[..4], &[200, 30, 40, 255]);
        assert!(!got.has_alpha && !got.scaled);
    }

    /// A JPEG of `w` by `h` whose EXIF says the camera was turned (orientation 6: rotate 90 degrees clockwise).
    fn turned_jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 40]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg = out.into_inner();
        // APP1 "Exif": a little-endian TIFF header and one IFD entry, 0x0112 (orientation) = 6.
        let tiff: [u8; 26] = [
            b'I', b'I', 42, 0, 8, 0, 0, 0, // header, IFD at 8
            1, 0, // one entry
            0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, // orientation, SHORT, 1, 6
            0, 0, 0, 0, // no next IFD
        ];
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let len = u16::try_from(app1.len() + 2).unwrap().to_be_bytes();
        let mut with = jpeg[..2].to_vec();
        with.extend_from_slice(&[0xFF, 0xE1, len[0], len[1]]);
        with.extend_from_slice(&app1);
        with.extend_from_slice(&jpeg[2..]);
        with
    }

    /// The review (2026-10-24): phone photos showed sideways, the EXIF orientation ignored.
    #[test]
    fn a_photo_is_drawn_the_way_its_camera_held_it() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "turned.jpg", &turned_jpeg(8, 4));
        let got = decode_image(&p, &|| false).unwrap();
        assert_eq!((got.width, got.height), (4, 8), "turned upright");
    }

    #[test]
    fn a_transparent_png_says_it_has_alpha() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "t.png", &png_bytes(2, 2, 128));
        assert!(decode_image(&p, &|| false).unwrap().has_alpha);
    }

    #[test]
    fn every_format_the_viewer_lists_decodes() {
        let d = tempfile::tempdir().unwrap();
        let mut img = image::RgbaImage::new(4, 4);
        for p in img.pixels_mut() {
            *p = image::Rgba([10, 20, 30, 255]);
        }
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let cases: [(&str, image::ImageFormat, bool); 5] = [
            ("a.jpg", image::ImageFormat::Jpeg, true),
            ("a.gif", image::ImageFormat::Gif, false),
            ("a.bmp", image::ImageFormat::Bmp, false),
            ("a.ico", image::ImageFormat::Ico, false),
            ("a.webp", image::ImageFormat::WebP, false),
        ];
        for (name, fmt, use_rgb) in cases {
            let mut out = std::io::Cursor::new(Vec::new());
            let r = if use_rgb {
                rgb.write_to(&mut out, fmt)
            } else {
                img.write_to(&mut out, fmt)
            };
            r.unwrap_or_else(|e| panic!("{name}: {e}"));
            let p = write(d.path(), name, &out.into_inner());
            let got = decode_image(&p, &|| false).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((got.width, got.height), (4, 4), "{name}");
        }
    }

    #[test]
    fn a_file_that_is_not_a_picture_is_refused_with_a_reason() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "fake.png", b"this is not a picture at all");
        assert_eq!(decode_image(&p, &|| false), Err(Reason::NotAnImage));
    }

    #[test]
    fn a_picture_over_the_pixel_cap_is_refused_without_decoding_it() {
        // A BMP header that claims 20000 x 20000 (400 megapixels) with no pixel data after it.
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&20_000i32.to_le_bytes());
        bmp.extend_from_slice(&20_000i32.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0u8; 24]);
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "huge.bmp", &bmp);
        assert_eq!(decode_image(&p, &|| false), Err(Reason::TooManyPixels));
    }

    #[test]
    fn a_thumbnail_keeps_the_shape_and_says_the_pictures_own_size() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "w.png", &png_bytes(1000, 500, 255));
        let got = decode_thumb(&p, &|| false).unwrap();
        assert_eq!((got.width, got.height), (256, 128));
        assert_eq!((got.source_width, got.source_height), (1000, 500));
        assert_eq!(got.rgba.len(), 256 * 128 * 4);
    }

    #[test]
    fn a_small_picture_is_not_blown_up_for_its_thumbnail() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "s.png", &png_bytes(3, 2, 128));
        let got = decode_thumb(&p, &|| false).unwrap();
        assert_eq!((got.width, got.height), (3, 2));
        assert_eq!(&got.rgba[..4], &[200, 30, 40, 128], "straight alpha");
    }

    #[test]
    fn a_thumbnail_is_drawn_the_way_the_camera_held_it() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "turned.jpg", &turned_jpeg(8, 4));
        let got = decode_thumb(&p, &|| false).unwrap();
        assert_eq!((got.width, got.height), (4, 8));
        assert_eq!((got.source_width, got.source_height), (4, 8));
    }

    /// The panel would open this one (36 MP, under its 100 MP), but a hover does not.
    #[test]
    fn a_thumbnail_has_its_own_lower_pixel_limit() {
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&6_000i32.to_le_bytes());
        bmp.extend_from_slice(&6_000i32.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0u8; 24]);
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "big.bmp", &bmp);
        assert_eq!(decode_thumb(&p, &|| false), Err(Reason::TooManyPixels));
    }

    #[test]
    fn a_thumbnail_refuses_a_file_over_its_own_size_limit_without_reading_it() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big.png");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(THUMB_FILE_MAX + 1).unwrap();
        drop(f);
        assert_eq!(decode_thumb(&p, &|| false), Err(Reason::TooBig));
    }

    #[test]
    fn a_thumbnail_nobody_wants_any_more_is_abandoned() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "a.png", &png_bytes(3, 2, 255));
        assert_eq!(decode_thumb(&p, &|| true), Err(Reason::Superseded));
    }

    #[test]
    fn a_wide_picture_is_scaled_to_the_side_limit() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "w.png", &png_bytes(IMAGE_SIDE_MAX + 100, 2, 255));
        let got = decode_image(&p, &|| false).unwrap();
        assert!(got.scaled);
        assert_eq!(got.width, IMAGE_SIDE_MAX);
        assert!(got.height >= 1);
    }
}
