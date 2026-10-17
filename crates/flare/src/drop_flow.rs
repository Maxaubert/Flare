//! The file drop flow of the terminal: dropped paths are typed, quoted for the shell that runs,
//! never followed by Enter, and only over a live shell (regression rule 143, rule 135).
#![allow(dead_code)] // wired by TermUi (Task 23); remove this line then

use std::path::Path;

use flare_shells::paste::{PathShell, path_shell_of, quote_paths, sanitize_paste};
use flare_term::handle::TermHandle;

/// Quiet time after the last dropped file before the batch is typed.
pub const DROP_QUIET_MS: u64 = 40;

/// Collects the files of one drop (winit sends one event per file) into one batch.
#[derive(Debug, Default)]
pub struct DropBatcher {
    paths: Vec<String>,
    last_ms: u64,
}

impl DropBatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// winit delivers one DroppedFile per file; a path that is not valid UTF-16-to-UTF-8 is skipped (never lossy).
    pub fn push(&mut self, path: &Path, now_ms: u64) {
        let Some(text) = path.to_str() else {
            log::warn!("a dropped path is not valid text; skipped");
            return;
        };
        self.paths.push(text.to_string());
        self.last_ms = now_ms;
    }

    /// The batch, once `DROP_QUIET_MS` passed since the last push; None before, or when empty.
    pub fn take_ready(&mut self, now_ms: u64) -> Option<Vec<String>> {
        if self.paths.is_empty() || now_ms.saturating_sub(self.last_ms) < DROP_QUIET_MS {
            return None;
        }
        Some(std::mem::take(&mut self.paths))
    }

    pub fn is_pending(&self) -> bool {
        !self.paths.is_empty()
    }
}

/// `quote_paths(paths, Some(shell))`, then `None` unless `sanitize_paste(&t) == t` (the control filter); None
/// when empty or refused. No trailing space, no newline, no Enter.
pub fn drop_text(paths: &[String], shell: PathShell) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let text = quote_paths(paths, Some(shell))?;
    (sanitize_paste(&text) == text).then_some(text)
}

/// Types the text with `TermHandle::send_text` (raw, like Prism Terminal's `termInput`: a drop is not a paste).
/// The shell is `path_shell_of(handle.shell_id())`, read HERE; `handle.shell_id() == None` types nothing and
/// returns false.
pub fn type_dropped(handle: &TermHandle, paths: &[String]) -> bool {
    let Some(id) = handle.shell_id() else {
        return false;
    };
    let Some(text) = drop_text(paths, path_shell_of(Some(&id))) else {
        return false;
    };
    handle.send_text(&text);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_shells::list::ShellDef;
    use flare_term::fake::{FakeShells, FakeSpawner, FakeState};
    use flare_term::handle::StartOptions;
    use flare_term::session::{SessionManager, SessionOptions};
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn p(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn a_burst_is_one_batch() {
        let mut b = DropBatcher::new();
        b.push(Path::new(r"C:\a.txt"), 0);
        b.push(Path::new(r"C:\b.txt"), 1);
        b.push(Path::new(r"C:\c.txt"), 2);
        assert_eq!(b.take_ready(30), None);
        assert_eq!(
            b.take_ready(42),
            Some(vec![p(r"C:\a.txt"), p(r"C:\b.txt"), p(r"C:\c.txt")])
        );
        assert_eq!(b.take_ready(100), None);
        assert!(!b.is_pending());
    }

    #[test]
    fn a_later_file_extends_the_wait() {
        let mut b = DropBatcher::new();
        b.push(Path::new(r"C:\a.txt"), 0);
        b.push(Path::new(r"C:\b.txt"), 30);
        assert_eq!(b.take_ready(50), None);
        assert_eq!(b.take_ready(70), Some(vec![p(r"C:\a.txt"), p(r"C:\b.txt")]));
    }

    #[test]
    fn two_bursts_are_two_batches() {
        let mut b = DropBatcher::new();
        b.push(Path::new(r"C:\a.txt"), 0);
        assert_eq!(b.take_ready(40), Some(vec![p(r"C:\a.txt")]));
        b.push(Path::new(r"C:\b.txt"), 100);
        assert_eq!(b.take_ready(140), Some(vec![p(r"C:\b.txt")]));
    }

    #[test]
    fn a_path_that_is_not_utf8_is_skipped() {
        let bad = OsString::from_wide(&[0x43, 0x3a, 0x5c, 0xD800]);
        let mut b = DropBatcher::new();
        b.push(Path::new(&bad), 0);
        b.push(Path::new(r"C:\ok.txt"), 1);
        assert_eq!(b.take_ready(100), Some(vec![p(r"C:\ok.txt")]));
    }

    #[test]
    fn a_path_with_spaces_is_quoted_for_powershell() {
        assert_eq!(
            drop_text(&[p(r"C:\My Dir\a b.txt")], PathShell::PowerShell).as_deref(),
            Some(r"'C:\My Dir\a b.txt'")
        );
    }

    #[test]
    fn a_dollar_expression_stays_inert() {
        assert_eq!(
            drop_text(
                &[p(r"C:\d\report$(Start-Process calc).pdf")],
                PathShell::PowerShell
            )
            .as_deref(),
            Some(r"'C:\d\report$(Start-Process calc).pdf'")
        );
    }

    #[test]
    fn quotes_inside_are_doubled_including_curly_ones() {
        assert_eq!(
            drop_text(&[p(r"C:\Max's")], PathShell::PowerShell).as_deref(),
            Some(r"'C:\Max''s'")
        );
        assert_eq!(
            drop_text(&[p("C:\\Max\u{2019}s")], PathShell::PowerShell).as_deref(),
            Some("'C:\\Max\u{2019}\u{2019}s'")
        );
    }

    #[test]
    fn cmd_quotes_with_double_quotes_and_refuses_percent_bang_and_quote() {
        assert_eq!(
            drop_text(&[p(r"C:\a b")], PathShell::Cmd).as_deref(),
            Some(r#""C:\a b""#)
        );
        assert_eq!(drop_text(&[p(r"C:\100%")], PathShell::Cmd), None);
        assert_eq!(drop_text(&[p(r"C:\a!b")], PathShell::Cmd), None);
        assert_eq!(drop_text(&[p("C:\\a\"b")], PathShell::Cmd), None);
    }

    #[test]
    fn bash_uses_single_quotes_with_the_escape() {
        assert_eq!(
            drop_text(&[p("/mnt/c/it's")], PathShell::Bash).as_deref(),
            Some(r"'/mnt/c/it'\''s'")
        );
    }

    #[test]
    fn a_control_character_in_a_name_types_nothing() {
        for shell in [PathShell::PowerShell, PathShell::Cmd, PathShell::Bash] {
            for bad in [
                "C:\\a\u{1b}b",
                "C:\\a\u{7}b",
                "C:\\a\u{7f}b",
                "C:\\a\u{9b}b",
            ] {
                assert_eq!(drop_text(&[p(bad)], shell), None, "{bad:?} {shell:?}");
            }
        }
    }

    #[test]
    fn several_files_are_joined_by_one_space_and_never_end_in_enter() {
        let t = drop_text(&[p(r"C:\a"), p(r"C:\b")], PathShell::PowerShell).unwrap();
        assert_eq!(t, r"'C:\a' 'C:\b'");
        assert!(!t.contains('\r') && !t.contains('\n'));
        assert!(!t.ends_with(' '));
    }

    #[test]
    fn an_empty_drop_is_none() {
        assert_eq!(drop_text(&[], PathShell::PowerShell), None);
    }

    fn wait_for_shell(handle: &TermHandle) {
        let t = Instant::now();
        while handle.shell_id().is_none() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The fake pty's bytes from `from` on, once `want` bytes arrived (the session's writer thread
    /// writes asynchronously) or five seconds passed.
    fn bytes_after(fake: &FakeState, from: usize, want: usize) -> Vec<u8> {
        let t = Instant::now();
        loop {
            let w = fake.written();
            if w.len() >= from + want || t.elapsed() > Duration::from_secs(5) {
                return w[from.min(w.len())..].to_vec();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn handle_with(
        defs: Vec<ShellDef>,
    ) -> (Arc<SessionManager>, Arc<TermHandle>, Arc<FakeSpawner>) {
        let sp = FakeSpawner::new();
        let mgr = SessionManager::new(
            sp.clone(),
            FakeShells::with(defs),
            SessionOptions::default(),
        );
        let handle = TermHandle::new("d1", Arc::clone(&mgr), 80, 24);
        (mgr, handle, sp)
    }

    fn shell_def(id: &str, name: &str, exe: &str) -> ShellDef {
        ShellDef {
            id: id.to_string(),
            name: name.to_string(),
            exe: exe.to_string(),
            args: Vec::new(),
        }
    }

    #[test]
    fn type_dropped_writes_the_exact_text_and_marks_typing() {
        let (_mgr, handle, sp) = handle_with(vec![shell_def("pwsh", "PowerShell 7", "pwsh.exe")]);
        handle.start(StartOptions {
            root: "C:\\x".to_string(),
            shell_id: Some("pwsh".to_string()),
            ..StartOptions::default()
        });
        wait_for_shell(&handle);
        let fake = Arc::clone(&sp.made()[0]);
        let before = fake.written().len();
        assert!(type_dropped(&handle, &[p(r"C:\a b.txt")]));
        let text = b"'C:\\a b.txt'";
        let written = bytes_after(&fake, before, text.len());
        assert_eq!(written, text);
        assert!(!written.contains(&0x0d));
        assert!(!written.contains(&0x0a));
        assert!(handle.last_typed().is_some());
    }

    #[test]
    fn type_dropped_quotes_for_the_shell_that_runs() {
        let (_mgr, handle, sp) = handle_with(vec![shell_def("cmd", "Command Prompt", "cmd.exe")]);
        handle.start(StartOptions {
            root: "C:\\x".to_string(),
            shell_id: None,
            ..StartOptions::default()
        });
        wait_for_shell(&handle);
        assert_eq!(handle.shell_id().as_deref(), Some("cmd"));
        let fake = Arc::clone(&sp.made()[0]);
        let before = fake.written().len();
        assert!(type_dropped(&handle, &[p(r"C:\a b.txt")]));
        let text = br#""C:\a b.txt""#;
        assert_eq!(bytes_after(&fake, before, text.len()), text);
    }

    #[test]
    fn type_dropped_before_the_shell_started_types_nothing() {
        let (_mgr, handle, sp) = handle_with(vec![shell_def("pwsh", "PowerShell 7", "pwsh.exe")]);
        assert!(!type_dropped(&handle, &[p(r"C:\a.txt")]));
        assert!(sp.made().is_empty());
        assert!(handle.last_typed().is_none());
    }
}
