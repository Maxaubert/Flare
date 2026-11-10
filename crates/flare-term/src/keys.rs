//! KeyEvent, TermKeyAction, term_key_action.
//! Port of Prism Terminal's termKeys.ts: the decision order for keys the host may take
//! before the pty sees them. Keys are stringly like PT (`key`, `code` W3C names), so the
//! physical `code` decides the chords (rule 11, O2).

/// One key press or release, as the window adapter reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub pressed: bool,
    pub key: String,
    pub code: String,
    pub text: Option<String>,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl KeyEvent {
    /// A key press with no text and no modifiers.
    pub fn new(key: &str, code: &str) -> Self {
        Self {
            pressed: true,
            key: key.to_string(),
            code: code.to_string(),
            text: None,
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
        }
    }
    pub fn ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }
    pub fn shift(mut self) -> Self {
        self.shift = true;
        self
    }
    pub fn alt(mut self) -> Self {
        self.alt = true;
        self
    }
    pub fn meta(mut self) -> Self {
        self.meta = true;
        self
    }
    pub fn text(mut self, t: &str) -> Self {
        self.text = Some(t.to_string());
        self
    }
    pub fn released(mut self) -> Self {
        self.pressed = false;
        self
    }
}

/// What the host does with a key before (or instead of) the pty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermKeyAction {
    Copy,
    Deleted,
    Host,
    Newline,
    Paste,
    PastePlain,
    Pty,
    /// macOS Cmd+A: the whole buffer is selected (Windows has no such key: Ctrl+A is the shell's).
    SelectAll,
}

/// Whose key conventions apply: Windows (Ctrl is the app's key) or macOS (Cmd is the app's key, and
/// Ctrl+letter always reaches the terminal as a control character). The default is Windows, so
/// every caller and test that names no system keeps the Windows behaviour; the app passes `HOST`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyOs {
    #[default]
    Windows,
    Mac,
}

impl KeyOs {
    /// The system this build runs on.
    pub const HOST: KeyOs = if cfg!(target_os = "macos") {
        KeyOs::Mac
    } else {
        KeyOs::Windows
    };

    pub fn is_mac(self) -> bool {
        self == KeyOs::Mac
    }

    /// The app's primary modifier alone (Ctrl on Windows, Cmd on macOS), Shift allowed.
    pub fn primary(self, e: &KeyEvent) -> bool {
        match self {
            KeyOs::Windows => e.ctrl && !e.alt && !e.meta,
            KeyOs::Mac => e.meta && !e.ctrl && !e.alt,
        }
    }
}

/// True when the key is `letter`, by its ASCII key name, else by its physical code (regression rule 11, O2).
pub fn is_key(e: &KeyEvent, letter: char) -> bool {
    let mut chars = e.key.chars();
    // An ASCII key name is the layout the user types on, and it decides alone (Dvorak Ctrl+J
    // is not a copy). The physical code answers only for a key named outside ASCII (Russian,
    // Greek), where the letter cannot be read from the name.
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii()
    {
        return c.to_ascii_lowercase() == letter;
    }
    e.code == format!("Key{}", letter.to_ascii_uppercase())
}

/// What the decision needs to know about the terminal and the host.
pub struct KeyState<'a> {
    pub has_selection: bool,
    pub delete_selection: &'a mut dyn FnMut() -> bool,
    pub host_owns: &'a dyn Fn(&KeyEvent) -> bool,
    pub agent_here: bool,
}

/// Decision order (PT termKeyAction): !pressed -> Pty; Ctrl+C with a selection and no other
/// modifier -> Copy; Backspace or Delete with no modifier and a selection the line takes ->
/// Deleted (the closure runs only then); host_owns -> Host; Shift+Enter where an agent runs ->
/// Newline; Ctrl+V (no Alt) -> Paste, or PastePlain with Shift; else Pty.
pub fn term_key_action(e: &KeyEvent, s: &mut KeyState) -> TermKeyAction {
    term_key_action_on(KeyOs::Windows, e, s)
}

/// `term_key_action` for one system. macOS: Cmd+C copies a selection, Cmd+V pastes (Cmd+Shift+V
/// plain), Cmd+A selects all; Ctrl+C and Ctrl+V are the shell's control characters there.
pub fn term_key_action_on(os: KeyOs, e: &KeyEvent, s: &mut KeyState) -> TermKeyAction {
    if !e.pressed {
        return TermKeyAction::Pty;
    }
    if os.is_mac() {
        return mac_key_action(e, s);
    }
    if is_key(e, 'c') && e.ctrl && !e.shift && !e.alt && s.has_selection {
        return TermKeyAction::Copy;
    }
    let plain = !e.ctrl && !e.shift && !e.alt && !e.meta;
    if (e.key == "Backspace" || e.key == "Delete")
        && plain
        && s.has_selection
        && (s.delete_selection)()
    {
        return TermKeyAction::Deleted;
    }
    if (s.host_owns)(e) {
        return TermKeyAction::Host;
    }
    if e.key == "Enter" && e.shift && s.agent_here {
        return TermKeyAction::Newline;
    }
    if is_key(e, 'v') && e.ctrl && !e.alt {
        return if e.shift {
            TermKeyAction::PastePlain
        } else {
            TermKeyAction::Paste
        };
    }
    TermKeyAction::Pty
}

/// The macOS order: the same as Windows with Cmd in Ctrl's place for copy and paste.
fn mac_key_action(e: &KeyEvent, s: &mut KeyState) -> TermKeyAction {
    let cmd_only = e.meta && !e.ctrl && !e.alt;
    if cmd_only && !e.shift && is_key(e, 'c') && s.has_selection {
        return TermKeyAction::Copy;
    }
    let plain = !e.ctrl && !e.shift && !e.alt && !e.meta;
    if (e.key == "Backspace" || e.key == "Delete")
        && plain
        && s.has_selection
        && (s.delete_selection)()
    {
        return TermKeyAction::Deleted;
    }
    if (s.host_owns)(e) {
        return TermKeyAction::Host;
    }
    if e.key == "Enter" && e.shift && s.agent_here {
        return TermKeyAction::Newline;
    }
    if cmd_only && is_key(e, 'v') {
        return if e.shift {
            TermKeyAction::PastePlain
        } else {
            TermKeyAction::Paste
        };
    }
    if cmd_only && !e.shift && is_key(e, 'a') {
        return TermKeyAction::SelectAll;
    }
    TermKeyAction::Pty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str) -> KeyEvent {
        let code = if k.chars().count() == 1 {
            format!("Key{}", k.to_uppercase())
        } else {
            k.to_string()
        };
        KeyEvent::new(k, &code)
    }

    /// Runs the decision with the given selection, delete answer, host answer and agent flag.
    fn run(e: &KeyEvent, sel: bool, del: bool, host: bool, agent: bool) -> TermKeyAction {
        let mut d = move || del;
        let h = move |_: &KeyEvent| host;
        let mut s = KeyState {
            has_selection: sel,
            delete_selection: &mut d,
            host_owns: &h,
            agent_here: agent,
        };
        term_key_action(e, &mut s)
    }

    #[test]
    fn ctrl_c_copies_a_selection_and_interrupts_without_one() {
        assert_eq!(
            run(&key("c").ctrl(), true, false, false, false),
            TermKeyAction::Copy
        );
        assert_eq!(
            run(&key("c").ctrl(), false, false, false, false),
            TermKeyAction::Pty
        );
    }

    #[test]
    fn ctrl_c_reads_the_physical_key() {
        let cyr_c = KeyEvent::new("\u{441}", "KeyC").ctrl();
        assert_eq!(run(&cyr_c, true, false, false, false), TermKeyAction::Copy);
        let cyr_v = KeyEvent::new("\u{43c}", "KeyV").ctrl();
        assert_eq!(
            run(&cyr_v, false, false, false, false),
            TermKeyAction::Paste
        );
    }

    #[test]
    fn backspace_deletes_only_when_the_line_takes_it() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let mut yes = || {
            calls.set(calls.get() + 1);
            true
        };
        let h = |_: &KeyEvent| false;
        let bs = key("Backspace");
        let mut s = KeyState {
            has_selection: true,
            delete_selection: &mut yes,
            host_owns: &h,
            agent_here: false,
        };
        assert_eq!(term_key_action(&bs, &mut s), TermKeyAction::Deleted);

        let mut no = || false;
        let mut s = KeyState {
            has_selection: true,
            delete_selection: &mut no,
            host_owns: &h,
            agent_here: false,
        };
        assert_eq!(term_key_action(&bs, &mut s), TermKeyAction::Pty);

        let mut yes2 = || {
            calls.set(calls.get() + 1);
            true
        };
        let mut s = KeyState {
            has_selection: false,
            delete_selection: &mut yes2,
            host_owns: &h,
            agent_here: false,
        };
        assert_eq!(term_key_action(&bs, &mut s), TermKeyAction::Pty);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn host_chords_never_reach_the_pty() {
        assert_eq!(
            run(&key("t").ctrl(), false, false, true, false),
            TermKeyAction::Host
        );
    }

    #[test]
    fn shift_enter_is_a_newline_only_where_an_agent_runs() {
        assert_eq!(
            run(&key("Enter").shift(), false, false, false, true),
            TermKeyAction::Newline
        );
        assert_eq!(
            run(&key("Enter").shift(), false, false, false, false),
            TermKeyAction::Pty
        );
    }

    #[test]
    fn ctrl_v_paste_ctrl_shift_v_plain_keyup_pty() {
        assert_eq!(
            run(&key("v").ctrl(), false, false, false, false),
            TermKeyAction::Paste
        );
        assert_eq!(
            run(&key("V").ctrl().shift(), false, false, false, false),
            TermKeyAction::PastePlain
        );
        assert_eq!(
            run(&key("v").ctrl().released(), false, false, false, false),
            TermKeyAction::Pty
        );
    }

    #[test]
    fn ctrl_v_with_alt_is_not_a_paste() {
        assert_eq!(
            run(&key("v").ctrl().alt(), false, false, false, false),
            TermKeyAction::Pty
        );
    }

    #[test]
    fn ctrl_shift_c_or_ctrl_alt_c_is_not_a_copy() {
        assert_eq!(
            run(&key("c").ctrl().shift(), true, false, false, false),
            TermKeyAction::Pty
        );
        assert_eq!(
            run(&key("c").ctrl().alt(), true, false, false, false),
            TermKeyAction::Pty
        );
    }

    #[test]
    fn chords_follow_the_layout_letter() {
        // Dvorak: Ctrl+J is physical KeyC, and must not copy.
        let e = KeyEvent::new("j", "KeyC").ctrl();
        assert!(!is_key(&e, 'c'));
        assert!(is_key(&e, 'j'));
        // Dvorak: the key labelled C is physical KeyI, and does copy.
        assert!(is_key(&KeyEvent::new("c", "KeyI"), 'c'));
        // Russian: a Cyrillic name falls back to the physical key.
        assert!(is_key(&KeyEvent::new("\u{441}", "KeyC"), 'c'));
    }

    fn run_mac(e: &KeyEvent, sel: bool) -> TermKeyAction {
        let mut d = || false;
        let h = |_: &KeyEvent| false;
        let mut s = KeyState {
            has_selection: sel,
            delete_selection: &mut d,
            host_owns: &h,
            agent_here: false,
        };
        term_key_action_on(KeyOs::Mac, e, &mut s)
    }

    #[test]
    fn mac_cmd_c_copies_and_ctrl_c_stays_the_shells_interrupt() {
        assert_eq!(run_mac(&key("c").meta(), true), TermKeyAction::Copy);
        // No selection: not a copy; the Mac key translation then sends nothing for a Cmd key.
        assert_eq!(run_mac(&key("c").meta(), false), TermKeyAction::Pty);
        assert_eq!(run_mac(&key("c").ctrl(), true), TermKeyAction::Pty);
        assert_eq!(run_mac(&key("c").ctrl(), false), TermKeyAction::Pty);
        assert_eq!(run_mac(&key("c").meta().shift(), true), TermKeyAction::Pty);
    }

    #[test]
    fn mac_cmd_v_pastes_and_ctrl_v_reaches_the_shell() {
        assert_eq!(run_mac(&key("v").meta(), false), TermKeyAction::Paste);
        assert_eq!(
            run_mac(&key("V").meta().shift(), false),
            TermKeyAction::PastePlain
        );
        assert_eq!(run_mac(&key("v").ctrl(), false), TermKeyAction::Pty);
        assert_eq!(run_mac(&key("v").meta().alt(), false), TermKeyAction::Pty);
    }

    #[test]
    fn mac_cmd_a_selects_all_and_ctrl_a_is_line_start() {
        assert_eq!(run_mac(&key("a").meta(), false), TermKeyAction::SelectAll);
        assert_eq!(run_mac(&key("a").ctrl(), false), TermKeyAction::Pty);
        // Windows: Ctrl+A is the shell's, and Cmd+A means nothing to the decision.
        assert_eq!(
            run(&key("a").ctrl(), false, false, false, false),
            TermKeyAction::Pty
        );
        assert_eq!(
            run(&key("a").meta(), false, false, false, false),
            TermKeyAction::Pty
        );
    }

    #[test]
    fn mac_cmd_reads_the_physical_key_like_ctrl_does() {
        let cyr_c = KeyEvent::new("\u{441}", "KeyC").meta();
        assert_eq!(run_mac(&cyr_c, true), TermKeyAction::Copy);
    }

    #[test]
    fn the_primary_modifier_is_ctrl_on_windows_and_cmd_on_the_mac() {
        assert!(KeyOs::Windows.primary(&key("t").ctrl()));
        assert!(!KeyOs::Windows.primary(&key("t").meta()));
        assert!(KeyOs::Mac.primary(&key("t").meta()));
        assert!(KeyOs::Mac.primary(&key("T").meta().shift()));
        assert!(!KeyOs::Mac.primary(&key("t").ctrl()));
        assert!(!KeyOs::Mac.primary(&key("t").meta().ctrl()));
        assert_eq!(KeyOs::default(), KeyOs::Windows);
        assert_eq!(KeyOs::HOST.is_mac(), cfg!(target_os = "macos"));
    }
}
