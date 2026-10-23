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
    if !e.pressed {
        return TermKeyAction::Pty;
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
}
