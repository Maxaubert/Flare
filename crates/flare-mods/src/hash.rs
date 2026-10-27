//! The content hash of a plugin folder, kept with the approval (design section 13): `sha256:` and 64 hex digits
//! over every file's path and bytes, in path order. Two folders with the same files have the same hash, whatever
//! order the disk lists them in.
//!
//! Worker-only: it reads every file. It is bounded (files, depth and bytes, the limits of a plugin zip), so a huge
//! folder answers an error instead of a long read, and a link is hashed by its name only, never followed.

use sha2::{Digest, Sha256};
use std::path::Path;

/// Files hashed at most.
pub const MAX_FILES: usize = 2000;
/// Folder depth below the plugin folder.
pub const MAX_DEPTH: usize = 8;
/// Bytes read at most, over all files.
pub const MAX_BYTES: u64 = 100 * 1024 * 1024;

const PREFIX: &str = "sha256:";

/// Whether `s` is a hash this module writes.
pub fn is_hash(s: &str) -> bool {
    s.strip_prefix(PREFIX).is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The hash of the files under `folder`, or why there is none (too many files, too deep, too big, unreadable).
pub fn folder_hash(folder: &Path) -> Result<String, String> {
    let mut items: Vec<(String, Item)> = Vec::new();
    walk(folder, "", 0, &mut items)?;
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    let mut total: u64 = 0;
    for (rel, item) in &items {
        match item {
            Item::Link => {
                h.update(b"l\0");
                h.update(rel.as_bytes());
                h.update(b"\0");
            }
            Item::File(len) => {
                total += len;
                if total > MAX_BYTES {
                    return Err(format!(
                        "the folder holds more than {} MB",
                        MAX_BYTES / (1024 * 1024)
                    ));
                }
                h.update(b"f\0");
                h.update(rel.as_bytes());
                h.update(b"\0");
                h.update(len.to_le_bytes());
                let mut f = std::fs::File::open(folder.join(rel)) // sync-ok: runs on a worker
                    .map_err(|e| format!("{rel} cannot be read: {e}"))?;
                let copied = std::io::copy(&mut f, &mut h)
                    .map_err(|e| format!("{rel} cannot be read: {e}"))?;
                if copied != *len {
                    return Err(format!("{rel} changed while it was read"));
                }
            }
        }
    }
    let digest = h.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{PREFIX}{hex}"))
}

enum Item {
    File(u64),
    Link,
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<(String, Item)>) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("the folder is deeper than {MAX_DEPTH} levels"));
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("a folder cannot be listed: {e}"))?; // sync-ok: runs on a worker
    for entry in entries {
        let entry = entry.map_err(|e| format!("a folder cannot be listed: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let meta = std::fs::symlink_metadata(entry.path()) // sync-ok: runs on a worker
            .map_err(|e| format!("{path} cannot be read: {e}"))?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            out.push((path, Item::Link));
        } else if ft.is_dir() {
            walk(&entry.path(), &path, depth + 1, out)?;
        } else {
            out.push((path, Item::File(meta.len())));
        }
        if out.len() > MAX_FILES {
            return Err(format!("the folder holds more than {MAX_FILES} files"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn folder(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let p = dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap(); // sync-ok: test code only
            fs::write(p, text).unwrap(); // sync-ok: test code only
        }
        dir
    }

    #[test]
    fn the_same_files_give_the_same_hash_and_any_change_another() {
        let a = folder(&[("plugin.json", "{}"), ("ui/panel.html", "<p>x</p>")]);
        let b = folder(&[("ui/panel.html", "<p>x</p>"), ("plugin.json", "{}")]);
        let ha = folder_hash(a.path()).unwrap();
        assert!(is_hash(&ha), "{ha}");
        assert_eq!(ha, folder_hash(b.path()).unwrap());
        let c = folder(&[("plugin.json", "{}"), ("ui/panel.html", "<p>y</p>")]);
        assert_ne!(ha, folder_hash(c.path()).unwrap(), "a changed byte");
        let d = folder(&[("plugin.json", "{}"), ("ui/other.html", "<p>x</p>")]);
        assert_ne!(ha, folder_hash(d.path()).unwrap(), "a renamed file");
        let e = folder(&[("plugin.json", "{}")]);
        assert_ne!(ha, folder_hash(e.path()).unwrap(), "a missing file");
    }

    #[test]
    fn a_folder_too_deep_is_an_error() {
        let deep = (0..=MAX_DEPTH)
            .map(|i| format!("d{i}"))
            .collect::<Vec<_>>()
            .join("/");
        let dir = folder(&[(&format!("{deep}/x.txt"), "x")]);
        assert!(folder_hash(dir.path()).unwrap_err().contains("deeper"));
    }

    #[test]
    fn only_written_hashes_pass() {
        assert!(is_hash(&format!("sha256:{}", "0f".repeat(32))));
        for bad in [
            "",
            "sha256:",
            "md5:00",
            &format!("sha256:{}", "0F".repeat(32)),
            &format!("sha256:{}", "0".repeat(63)),
        ] {
            assert!(!is_hash(bad), "{bad:?}");
        }
    }
}
