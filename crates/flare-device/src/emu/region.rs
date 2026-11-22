//! The frame region (rule 1363): a file Flare owns, which the emulator maps and writes each new frame into
//! (`streamScreenshot` with `ImageTransport{MMAP, handle}`); only the frame's facts travel on the emu host's stdout.
//!
//! - **Where**: a `flare` folder inside the emulator's discovery folder. Emulator 37.3.10 on macOS accepts a region
//!   file only there, in `/tmp` or in `/tmp/android-<user>`, and answers PERMISSION_DENIED for anything else (spike S7,
//!   2026-11-22). Windows 37.3.10 does not check; Flare keeps one place on both.
//! - **Who**: only this user may read it (the folder 0700 and the file 0600 on macOS; on Windows the folder is under
//!   the user's own `%LOCALAPPDATA%`, which only the user and the system can open).
//! - **When**: made by Flare for an attached device, deleted on detach, and files a Flare that is gone left behind are
//!   deleted when Flare starts ([`clean_stale`]).
//! - **Size**: big enough for every rotation of the screen, `max(w, h)^2 * 4` bytes (RGBA), so turning the device
//!   never needs a new file.
//!
//! The emulator writes rows top-down (spike S1: no Y flip), and its screenshot masks the display's rounded corners.

use std::path::{Path, PathBuf};

/// The folder inside the discovery folder.
pub const FOLDER: &str = "flare";

/// The region folder for a discovery folder.
pub fn dir(discovery: &Path) -> PathBuf {
    discovery.join(FOLDER)
}

/// A region file's name: `<Flare's pid>-<n>.rgba`.
pub fn name(pid: u32, n: u64) -> String {
    format!("{pid}-{n}.rgba")
}

/// The pid in a region file's name, None for any other name.
pub fn pid_of(file_name: &str) -> Option<u32> {
    let (pid, rest) = file_name.strip_suffix(".rgba")?.split_once('-')?;
    (rest.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty())
        .then(|| pid.parse().ok())
        .flatten()
}

/// The `handle` the emulator takes: `file:///` and the path, the exact form spikes S1 and S7 used on both OSes
/// (`file:///C:/...` on Windows, `file:////Users/...` on macOS; the proto asks for a URL that starts with `file:///`).
/// Backslashes become slashes; nothing is escaped, as the emulator reads the path after the prefix as it is.
pub fn url(path: &Path) -> String {
    format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
}

/// The bytes a region needs for a screen of `w` by `h` pixels in any rotation.
pub fn bytes_for(w: u32, h: u32) -> u64 {
    let side = u64::from(w.max(h));
    side * side * 4
}

/// A region file. Dropping it deletes the file (detach).
#[derive(Debug)]
pub struct Region {
    path: PathBuf,
    bytes: u64,
}

impl Region {
    /// BLOCKING (makes a folder and a file): a new region of `bytes` in `dir`, only this user may read it.
    pub fn create(dir: &Path, pid: u32, n: u64, bytes: u64) -> std::io::Result<Region> {
        make_dir(dir)?;
        let path = dir.join(name(pid, n));
        let mut o = std::fs::OpenOptions::new(); // sync-ok: called on a worker
        o.read(true).write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let f = o.open(&path)?;
        if let Err(e) = f.set_len(bytes) {
            let _ = std::fs::remove_file(&path); // sync-ok: the same worker
            return Err(e);
        }
        Ok(Region { path, bytes })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn url(&self) -> String {
        url(&self.path)
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path); // sync-ok: one small unlink on detach
    }
}

fn make_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut b = std::fs::DirBuilder::new(); // sync-ok: called on a worker
        b.recursive(true).mode(0o700);
        b.create(dir)?;
        // A folder made earlier by someone else's umask is made user-only too.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?; // sync-ok: as above
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir) // sync-ok: called on a worker
    }
}

/// BLOCKING: deletes every region file in `dir` whose Flare is gone (`alive(pid)` false) or is this one (`me`, a
/// region of an earlier run of this pid cannot be live). Returns how many went.
pub fn clean_stale(dir: &Path, me: u32, alive: impl Fn(u32) -> bool) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        // sync-ok: Flare's start worker
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let Some(pid) = pid_of(&e.file_name().to_string_lossy()) else {
            continue;
        };
        if (pid == me || !alive(pid)) && std::fs::remove_file(e.path()).is_ok() {
            // sync-ok: as above
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_has_a_file_url_and_room_for_every_rotation() {
        assert_eq!(
            url(Path::new(
                "C:\\Users\\a\\AppData\\Local\\Temp\\avd\\running\\flare\\12-1.rgba"
            )),
            "file:///C:/Users/a/AppData/Local/Temp/avd/running/flare/12-1.rgba"
        );
        assert_eq!(
            url(Path::new(
                "/Users/a/Library/Caches/TemporaryItems/avd/running/flare/12-1.rgba"
            )),
            "file:////Users/a/Library/Caches/TemporaryItems/avd/running/flare/12-1.rgba"
        );
        assert_eq!(bytes_for(1080, 2400), 2400 * 2400 * 4);
        assert_eq!(bytes_for(2400, 1080), bytes_for(1080, 2400));
        assert_eq!(name(12, 3), "12-3.rgba");
        assert_eq!(pid_of("12-3.rgba"), Some(12));
        for bad in ["12.rgba", "x-1.rgba", "12-.rgba", "12-1.png", "12-a.rgba"] {
            assert_eq!(pid_of(bad), None, "{bad}");
        }
        assert!(dir(Path::new("/d")).ends_with("flare"));
    }

    /// Rule 1363: the region is a user-only file in the `flare` folder, sized, and gone on drop and at start.
    #[test]
    fn a_region_is_made_user_only_and_deleted_on_detach_and_at_start() {
        let d = tempfile::tempdir().unwrap();
        let rd = dir(d.path());
        let r = Region::create(&rd, 77, 1, 4096).unwrap();
        assert_eq!(std::fs::metadata(r.path()).unwrap().len(), 4096);
        assert!(r.url().ends_with("/flare/77-1.rgba"), "{}", r.url());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(r.path()), 0o600);
            assert_eq!(mode(&rd), 0o700);
        }
        let p = r.path().to_path_buf();
        drop(r);
        assert!(!p.exists(), "deleted on detach");
        // At start: a gone Flare's and this pid's old files go; a live one's and other files stay.
        for f in ["5-1.rgba", "6-1.rgba", "77-9.rgba", "notes.txt"] {
            std::fs::write(rd.join(f), b"x").unwrap();
        }
        assert_eq!(clean_stale(&rd, 77, |pid| pid == 6), 2);
        let mut left: Vec<_> = std::fs::read_dir(&rd)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, ["6-1.rgba", "notes.txt"]);
        assert_eq!(clean_stale(&d.path().join("none"), 1, |_| true), 0);
    }
}
