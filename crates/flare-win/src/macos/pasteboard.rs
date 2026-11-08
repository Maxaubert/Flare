//! The clipboard on macOS: `NSPasteboard`, the Mac counterpart of the Win32 clipboard calls in
//! `clipboard.rs` and `clipboard_port.rs`.
//!
//! - Text is `public.utf8-plain-text` (`NSPasteboardTypeString`). An `NSString` always comes back
//!   to Rust as valid UTF-8, whatever the locale (no `LANG` is needed, unlike `pbcopy`).
//! - Copied files are the file URLs of the pasteboard's items (`public.file-url`), as absolute
//!   POSIX paths, in the order Finder put them there.
//! - An image is only seen (PNG or TIFF on the pasteboard), as on Windows: a paste then sends the
//!   key so the program reads the image itself.
//! - The sequence is the pasteboard's `changeCount`, which moves with every change by anyone.
//!
//! Every function takes the pasteboard it works on, so the tests use a private one
//! (`pasteboardWithUniqueName`) and never touch what the user copied. `NSPasteboard` may be used
//! from any thread; each call runs in its own autorelease pool (the callers are worker threads).

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSString, NSURL};

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
