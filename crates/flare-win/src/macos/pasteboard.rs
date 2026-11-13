//! The clipboard on macOS: `NSPasteboard`, the Mac counterpart of the Win32 clipboard calls in
//! `clipboard.rs` and `clipboard_port.rs`.
//!
//! - Text is `public.utf8-plain-text` (`NSPasteboardTypeString`). An `NSString` always comes back
//!   to Rust as valid UTF-8, whatever the locale (no `LANG` is needed, unlike `pbcopy`).
//! - Copied files are the file URLs of the pasteboard's items (`public.file-url`), as absolute
//!   POSIX paths, in the order Finder put them there.
//! - An image is only seen (PNG or TIFF on the pasteboard), as on Windows: a paste then sends the
//!   key so the program reads the image itself. A paste on a remote tab reads it as PNG bytes
//!   (`read_png`) to copy them to the host (remote sessions PR 6): the PNG when there is one, else
//!   the TIFF made a PNG by AppKit (`NSBitmapImageRep`, rule 1198).
//! - The sequence is the pasteboard's `changeCount`, which moves with every change by anyone.
//!
//! Every function takes the pasteboard it works on, so the tests use a private one
//! (`pasteboardWithUniqueName`) and never touch what the user copied. `NSPasteboard` may be used
//! from any thread; each call runs in its own autorelease pool (the callers are worker threads).

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSPasteboard,
    NSPasteboardItem, NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};

/// The system clipboard.
pub fn general() -> Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

fn string_type() -> &'static NSPasteboardType {
    // SAFETY: an AppKit constant, valid for the life of the process.
    unsafe { NSPasteboardTypeString }
}

fn file_url_type() -> &'static NSPasteboardType {
    // SAFETY: an AppKit constant, valid for the life of the process.
    unsafe { NSPasteboardTypeFileURL }
}

/// Replaces the pasteboard's contents with `text`. Err when the pasteboard refuses it.
pub fn write_text(pb: &NSPasteboard, text: &str) -> Result<(), String> {
    autoreleasepool(|_| {
        pb.clearContents();
        if pb.setString_forType(&NSString::from_str(text), string_type()) {
            Ok(())
        } else {
            Err("the pasteboard refused the text".to_string())
        }
    })
}

/// The pasteboard's text, None when it holds none (or only an empty string).
pub fn read_text(pb: &NSPasteboard) -> Option<String> {
    autoreleasepool(|_| {
        let s = pb.stringForType(string_type())?.to_string();
        (!s.is_empty()).then_some(s)
    })
}

/// The copied files: every item's file URL as an absolute POSIX path. A URL that is not a file
/// URL, or has no path, is left out.
pub fn read_files(pb: &NSPasteboard) -> Vec<String> {
    autoreleasepool(|_| {
        let Some(items) = pb.pasteboardItems() else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| item.stringForType(file_url_type()))
            .filter_map(|s| file_url_path(&s.to_string()))
            .collect()
    })
}

/// Pure-ish: the POSIX path of a `file://` URL string (percent escapes decoded by `NSURL`), None
/// for any other URL or one without an absolute path.
pub fn file_url_path(url: &str) -> Option<String> {
    let u = NSURL::URLWithString(&NSString::from_str(url))?;
    if !u.isFileURL() {
        return None;
    }
    let path = u.path()?.to_string();
    path.starts_with('/').then_some(path)
}

/// True when an image (PNG or TIFF, what screenshots and browsers put there) is on the pasteboard.
pub fn has_image(pb: &NSPasteboard) -> bool {
    autoreleasepool(|_| {
        // SAFETY: AppKit constants, valid for the life of the process.
        let kinds = unsafe { [NSPasteboardTypePNG, NSPasteboardTypeTIFF] };
        let types = NSArray::from_slice(&kinds);
        pb.availableTypeFromArray(&types).is_some()
    })
}

/// The pasteboard's image as PNG file bytes, at most `max` (remote sessions PR 6: a paste on a remote tab copies it to
/// the host). The PNG when there is one; else a TIFF (what Preview, Pages and many apps copy, often alone) made a PNG
/// (rule 1198), `max` holding for the TIFF read and for the PNG made. None when there is no picture or it cannot be
/// read.
pub fn read_png(pb: &NSPasteboard, max: usize) -> Option<Vec<u8>> {
    autoreleasepool(|_| {
        // SAFETY: AppKit constants, valid for the life of the process.
        let (png, tiff) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };
        if let Some(data) = pb.dataForType(png) {
            return (data.length() > 0 && data.length() <= max).then(|| data.to_vec());
        }
        let data = pb.dataForType(tiff)?;
        if data.length() == 0 || data.length() > max {
            return None;
        }
        png_of_tiff(&data.to_vec()).filter(|p| p.len() <= max)
    })
}

/// A TIFF file's first picture as a PNG file, by AppKit (`NSBitmapImageRep`): None when AppKit cannot read it. Any
/// thread (no window, no screen).
pub fn png_of_tiff(tiff: &[u8]) -> Option<Vec<u8>> {
    autoreleasepool(|_| {
        let data = NSData::with_bytes(tiff);
        let rep = NSBitmapImageRep::imageRepWithData(&data)?;
        let props = NSDictionary::<NSBitmapImageRepPropertyKey, AnyObject>::new();
        // SAFETY: an empty dictionary of the key and value types the method takes.
        let png =
            unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
        (png.length() > 0).then(|| png.to_vec())
    })
}

/// The change count, folded into the `u32` the port reports (it only has to change).
pub fn change_count(pb: &NSPasteboard) -> u32 {
    pb.changeCount() as u32
}

/// Puts `paths` on the pasteboard as file URLs, one item each (the tests stand in Finder's copy).
pub fn write_files(pb: &NSPasteboard, paths: &[&str]) -> Result<(), String> {
    autoreleasepool(|_| {
        let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
        for p in paths {
            let url = NSURL::fileURLWithPath(&NSString::from_str(p));
            let text = url
                .absoluteString()
                .ok_or_else(|| "a path has no URL".to_string())?;
            let item = NSPasteboardItem::new();
            if !item.setString_forType(&text, file_url_type()) {
                return Err("the item refused the URL".to_string());
            }
            items.push(ProtocolObject::from_retained(item));
        }
        pb.clearContents();
        let array = NSArray::from_retained_slice(&items);
        if pb.writeObjects(&array) {
            Ok(())
        } else {
            Err("the pasteboard refused the files".to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private pasteboard, released when the test ends.
    struct Private(Retained<NSPasteboard>);

    impl Private {
        fn new() -> Self {
            Self(NSPasteboard::pasteboardWithUniqueName())
        }
    }

    impl Drop for Private {
        fn drop(&mut self) {
            // SAFETY: `releaseGlobally` takes no arguments and returns nothing; it frees the private
            // pasteboard's server side (objc2 has no binding for it).
            unsafe {
                let () = objc2::msg_send![&*self.0, releaseGlobally];
            }
        }
    }

    #[test]
    fn text_round_trips_exactly_with_non_ascii_and_emoji() {
        let pb = Private::new();
        let text = "Bl\u{e5}b\u{e6}r \u{4e2d}\u{6587} \u{1F600}\ttab\nline";
        write_text(&pb.0, text).unwrap();
        assert_eq!(read_text(&pb.0).as_deref(), Some(text));
    }

    #[test]
    fn an_empty_pasteboard_reads_no_text_no_files_and_no_image() {
        let pb = Private::new();
        pb.0.clearContents();
        assert_eq!(read_text(&pb.0), None);
        assert!(read_files(&pb.0).is_empty());
        assert!(!has_image(&pb.0));
    }

    #[test]
    fn the_change_count_moves_with_every_write() {
        let pb = Private::new();
        let a = change_count(&pb.0);
        write_text(&pb.0, "a").unwrap();
        let b = change_count(&pb.0);
        write_text(&pb.0, "b").unwrap();
        let c = change_count(&pb.0);
        assert!(a != b && b != c, "{a} {b} {c}");
    }

    #[test]
    fn copied_files_come_back_as_posix_paths_in_order() {
        let pb = Private::new();
        // (A composed letter such as U+00E5 comes back decomposed: `fileURLWithPath` writes the file
        // system's form. Either form names the same file on APFS.)
        write_files(
            &pb.0,
            &["/tmp/a b.txt", "/Users/me/it's \u{4e2d}\u{1F600}.md"],
        )
        .unwrap();
        assert_eq!(
            read_files(&pb.0),
            vec![
                "/tmp/a b.txt".to_string(),
                "/Users/me/it's \u{4e2d}\u{1F600}.md".to_string()
            ]
        );
        // Files replace the text.
        assert_eq!(read_text(&pb.0), None);
    }

    #[test]
    fn an_image_on_the_pasteboard_is_seen() {
        let pb = Private::new();
        pb.0.clearContents();
        // SAFETY: an AppKit constant.
        let png = unsafe { NSPasteboardTypePNG };
        let data = objc2_foundation::NSData::with_bytes(b"\x89PNG\r\n\x1a\n");
        assert!(pb.0.setData_forType(Some(&data), png));
        assert!(has_image(&pb.0));
        // Remote sessions PR 6: its bytes are read back whole, and not past a cap.
        assert_eq!(
            read_png(&pb.0, 1024).as_deref(),
            Some(&b"\x89PNG\r\n\x1a\n"[..])
        );
        assert_eq!(read_png(&pb.0, 4), None);
    }

    /// A 2 x 1 baseline TIFF, written out by hand: little-endian, 8-bit grey, uncompressed, one strip of two pixels
    /// (black, white).
    fn tiny_tiff() -> Vec<u8> {
        let mut t = b"II*\0".to_vec();
        t.extend_from_slice(&8u32.to_le_bytes()); // the first directory
        // (tag, type 3 = SHORT or 4 = LONG, value)
        let entries: [(u16, u16, u32); 9] = [
            (256, 3, 2), // ImageWidth
            (257, 3, 1), // ImageLength
            (258, 3, 8), // BitsPerSample
            (259, 3, 1), // Compression: none
            (262, 3, 1), // PhotometricInterpretation: black is zero
            (273, 4, 0), // StripOffsets, filled in below
            (277, 3, 1), // SamplesPerPixel
            (278, 3, 1), // RowsPerStrip
            (279, 4, 2), // StripByteCounts
        ];
        let pixels_at = 8 + 2 + entries.len() as u32 * 12 + 4;
        t.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, value) in entries {
            let value = if tag == 273 { pixels_at } else { value };
            t.extend_from_slice(&tag.to_le_bytes());
            t.extend_from_slice(&kind.to_le_bytes());
            t.extend_from_slice(&1u32.to_le_bytes());
            if kind == 3 {
                t.extend_from_slice(&(value as u16).to_le_bytes());
                t.extend_from_slice(&[0, 0]);
            } else {
                t.extend_from_slice(&value.to_le_bytes());
            }
        }
        t.extend_from_slice(&0u32.to_le_bytes()); // no next directory
        t.extend_from_slice(&[0x00, 0xff]);
        t
    }

    /// The width and height a PNG's header says.
    fn png_size(png: &[u8]) -> (u32, u32) {
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
        assert_eq!(&png[12..16], b"IHDR");
        let be = |i: usize| u32::from_be_bytes(png[i..i + 4].try_into().unwrap());
        (be(16), be(20))
    }

    /// Rule 1198: a picture copied as a TIFF alone is pasted on a remote tab as a PNG of the same size, made here; a
    /// PNG beside it wins; neither is read past the cap; a TIFF AppKit cannot read gives nothing.
    #[test]
    fn a_tiff_only_picture_is_read_as_a_png_of_the_same_size() {
        let tiff = tiny_tiff();
        let png = png_of_tiff(&tiff).expect("AppKit reads the TIFF");
        assert_eq!(png_size(&png), (2, 1));
        assert_eq!(png_of_tiff(b"II*\0 not a tiff"), None);

        let pb = Private::new();
        pb.0.clearContents();
        // SAFETY: AppKit constants.
        let (png_t, tiff_t) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };
        assert!(pb.0.setData_forType(Some(&NSData::with_bytes(&tiff)), tiff_t));
        assert!(has_image(&pb.0));
        let read = read_png(&pb.0, 1024 * 1024).expect("the TIFF is read as a PNG");
        assert_eq!(png_size(&read), (2, 1));
        assert_eq!(
            read_png(&pb.0, tiff.len() - 1),
            None,
            "the cap holds for the TIFF"
        );

        pb.0.clearContents();
        assert!(pb.0.setData_forType(Some(&NSData::with_bytes(&tiff)), tiff_t));
        assert!(pb.0.setData_forType(Some(&NSData::with_bytes(&png)), png_t));
        assert_eq!(
            read_png(&pb.0, 1024 * 1024),
            Some(png.clone()),
            "the PNG wins"
        );
    }

    #[test]
    fn only_file_urls_give_a_path() {
        assert_eq!(
            file_url_path("file:///tmp/a%20b").as_deref(),
            Some("/tmp/a b")
        );
        assert_eq!(file_url_path("https://example.com/x"), None);
        assert_eq!(file_url_path("not a url"), None);
    }
}
