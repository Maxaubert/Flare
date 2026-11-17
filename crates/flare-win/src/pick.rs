//! pick_folder: the parented folder picker, off the UI thread (Task 17).

use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::thread;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle,
};

/// The window a picker is parented to (an HWND as an integer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParentWindow {
    pub hwnd: isize,
}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.hwnd).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY: the caller keeps the window alive while the picker it parents is open; the
        // picker is modal to that window and is gone before the window closes.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for ParentWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

#[derive(Clone, Debug, Default)]
pub struct PickOptions {
    pub parent: Option<ParentWindow>,
    pub default_dir: Option<PathBuf>,
    /// Where the picker opens when `default_dir` is unset or no longer a folder (a plugin zip: Downloads). Without
    /// either, Windows opens the folder it used last, which may be any folder of any app.
    pub fallback_dir: Option<PathBuf>,
    /// Set only under --e2e: the answer, with no dialog.
    pub seam: Option<String>,
}

/// The e2e seam: None when there is no seam, Some(None) for a cancel (an empty value), and
/// Some(Some(path)) for the path the seam names.
pub fn pick_seam(env: Option<&str>) -> Option<Option<PathBuf>> {
    env.map(|s| (!s.is_empty()).then(|| PathBuf::from(s)))
}

/// The folder a picker opens in: `default_dir` when it is a folder, else `fallback_dir` when it is, else None. It
/// looks at the disk through `is_dir`, so the pickers call it on their worker, never on the UI thread.
pub fn start_dir(opts: &PickOptions, is_dir: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    [opts.default_dir.as_ref(), opts.fallback_dir.as_ref()]
        .into_iter()
        .flatten()
        .find(|d| is_dir(d))
        .cloned()
}

/// Shows the folder picker without blocking the caller: a worker thread runs the dialog and calls
/// `done` with the folder, or None on a cancel. A seam answers at once with no dialog.
pub fn pick_folder(opts: PickOptions, done: impl FnOnce(Option<PathBuf>) + Send + 'static) {
    if let Some(answer) = pick_seam(opts.seam.as_deref()) {
        done(answer);
        return;
    }
    thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(parent) = &opts.parent {
            dialog = dialog.set_parent(parent);
        }
        // sync-ok: the existence check runs on this worker thread, never on the UI thread
        if let Some(dir) = start_dir(&opts, |d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        done(dialog.pick_folder());
    });
}

/// Shows the file picker for one file of the kind `filter` names (a label and its extensions, no dot), as
/// [`pick_folder`] does: on a worker, `done` with the file or None on a cancel, a seam answering at once.
pub fn pick_file(
    opts: PickOptions,
    filter: (&'static str, &'static [&'static str]),
    done: impl FnOnce(Option<PathBuf>) + Send + 'static,
) {
    if let Some(answer) = pick_seam(opts.seam.as_deref()) {
        done(answer);
        return;
    }
    thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new().add_filter(filter.0, filter.1);
        if let Some(parent) = &opts.parent {
            dialog = dialog.set_parent(parent);
        }
        // sync-ok: the existence check runs on this worker thread, never on the UI thread
        if let Some(dir) = start_dir(&opts, |d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        done(dialog.pick_file());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn the_pick_seam_answers_without_a_dialog() {
        assert_eq!(pick_seam(None), None);
        assert_eq!(pick_seam(Some("")), Some(None));
        assert_eq!(pick_seam(Some("C:\\x")), Some(Some(PathBuf::from("C:\\x"))));
    }

    /// Rule 1271: a plugin zip's picker opens where the zip was last installed from while that folder is there, else
    /// in the fallback (Downloads), never in the folder Windows used last.
    #[test]
    fn the_picker_starts_in_the_remembered_folder_then_the_fallback() {
        let remembered = PathBuf::from("C:\\from");
        let downloads = PathBuf::from("C:\\Users\\me\\Downloads");
        let opts = PickOptions {
            default_dir: Some(remembered.clone()),
            fallback_dir: Some(downloads.clone()),
            ..PickOptions::default()
        };
        assert_eq!(start_dir(&opts, |_| true), Some(remembered.clone()));
        assert_eq!(
            start_dir(&opts, |d| d != remembered.as_path()),
            Some(downloads.clone()),
            "a remembered folder that is gone falls back to Downloads"
        );
        assert_eq!(start_dir(&opts, |_| false), None, "neither is there");
        let install = PickOptions {
            fallback_dir: Some(downloads.clone()),
            ..PickOptions::default()
        };
        assert_eq!(
            start_dir(&install, |_| true),
            Some(downloads),
            "Install has no remembered folder: Downloads"
        );
        assert_eq!(start_dir(&PickOptions::default(), |_| true), None);
    }

    #[test]
    fn the_start_folder_is_checked_on_the_disk() {
        let dir = std::env::temp_dir();
        let opts = PickOptions {
            default_dir: Some(dir.join("flare-no-such-folder-1271")),
            fallback_dir: Some(dir.clone()),
            ..PickOptions::default()
        };
        // sync-ok: test code only, never the app
        assert_eq!(start_dir(&opts, |d| d.is_dir()), Some(dir));
    }

    #[test]
    fn pick_folder_with_a_seam_calls_done_at_once() {
        let (tx, rx) = mpsc::channel();
        let tx2 = tx.clone();
        pick_folder(
            PickOptions {
                seam: Some("C:\\x".to_string()),
                ..PickOptions::default()
            },
            move |p| {
                let _ = tx.send(p);
            },
        );
        let got = rx.recv_timeout(Duration::from_secs(1)).expect("answered");
        assert_eq!(got, Some(PathBuf::from("C:\\x")));

        pick_folder(
            PickOptions {
                seam: Some(String::new()),
                ..PickOptions::default()
            },
            move |p| {
                let _ = tx2.send(p);
            },
        );
        let got = rx.recv_timeout(Duration::from_secs(1)).expect("answered");
        assert_eq!(got, None);
    }
}
