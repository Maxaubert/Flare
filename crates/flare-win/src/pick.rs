//! pick_folder: the parented folder picker, off the UI thread (Task 17).

use std::num::NonZeroIsize;
use std::path::PathBuf;
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
    /// Set only under --e2e: the answer, with no dialog.
    pub seam: Option<String>,
}

/// The e2e seam: None when there is no seam, Some(None) for a cancel (an empty value), and
/// Some(Some(path)) for the path the seam names.
pub fn pick_seam(env: Option<&str>) -> Option<Option<PathBuf>> {
    env.map(|s| (!s.is_empty()).then(|| PathBuf::from(s)))
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
        if let Some(dir) = opts.default_dir.as_ref().filter(|d| d.is_dir()) {
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
        if let Some(dir) = opts.default_dir.as_ref().filter(|d| d.is_dir()) {
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
