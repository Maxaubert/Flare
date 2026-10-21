//! Acting on chords (Task 28).
//!
//! Source: Prism Terminal's `App.tsx` (`onKey`, the keyup listener) and `keymap.ts`. The window's key
//! filter calls `dispatch` for every key press: the app acts on its own chords here and tells the
//! caller whether the shell may hear the key as well.
#![allow(dead_code)] // the controller (Task 30) is the caller

use flare_tabs::keymap::{ChordId, KeyContext, UnderModal, chord, chord_of};
use flare_term::keys::KeyEvent;

/// What the window does with the key after `dispatch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch {
    /// Not the app's key: the shell gets it.
    NotAChord,
    /// Consumed: it never reaches the shell.
    Taken,
    /// F11 acted and the shell hears it as well.
    ActedShellToo,
}

/// What a chord does, implemented by the window controller.
pub trait ChordTarget {
    /// A close question or the update window is up.
    fn blocked(&self) -> bool;
    fn toggle_fullscreen(&mut self);
    fn toggle_help(&mut self);
    fn commit_walk(&mut self);
    fn next_tab(&mut self, dir: i32);
    /// Jump to the nth tab; false when there is no such tab (the key is then not taken).
    fn jump_tab(&mut self, n: usize) -> bool;
    fn toggle_settings(&mut self);
    fn find_tab(&mut self);
    fn toggle_sidebar(&mut self);
    fn new_tab(&mut self);
    fn close_tab(&mut self);
    fn find(&mut self);
    /// Alt+Space: the system menu at the window's top-left.
    fn open_system_menu(&mut self);
    /// Ctrl+Shift+P: shows or hides the tab's panel; does nothing when the tab has none to bring back.
    /// The key is the app's either way: the shell never hears it.
    fn toggle_panel(&mut self);
    /// Ctrl+Shift+O: moves the keys between the terminal and the open panel; does nothing when no
    /// panel is open. The key is the app's either way.
    fn focus_panel(&mut self);
}

/// App.tsx `onKey`, in order: a Ctrl chord that is not Tab, Control or Shift ends a Most recent walk;
/// then the chord is looked up; a swallowed chord does nothing while a question is up; then it acts.
pub fn dispatch(ev: &KeyEvent, ctx: &KeyContext, t: &mut dyn ChordTarget) -> Dispatch {
    if !ev.pressed {
        return Dispatch::NotAChord;
    }
    if ev.ctrl && !ev.alt && !matches!(ev.key.as_str(), "Tab" | "Control" | "Shift") {
        t.commit_walk();
    }
    let Some(id) = chord_of(ev, ctx) else {
        return Dispatch::NotAChord;
    };
    if t.blocked() && chord(id).under_modal == UnderModal::Swallow {
        return Dispatch::Taken;
    }
    match id {
        ChordId::AgentGuard => {}
        ChordId::Fullscreen => {
            t.toggle_fullscreen();
            return Dispatch::ActedShellToo;
        }
        ChordId::Help => t.toggle_help(),
        ChordId::NextTab => {
            let dir = if ev.shift || ev.key == "PageUp" {
                -1
            } else {
                1
            };
            t.next_tab(dir);
        }
        ChordId::JumpTab => {
            let n = ev.key.parse::<usize>().unwrap_or(0);
            if !t.jump_tab(n) {
                return Dispatch::NotAChord;
            }
        }
        ChordId::Settings => t.toggle_settings(),
        ChordId::FindTab => t.find_tab(),
        ChordId::Sidebar => t.toggle_sidebar(),
        ChordId::NewTab => t.new_tab(),
        ChordId::CloseTab => t.close_tab(),
        ChordId::Find => t.find(),
        ChordId::SystemMenu => t.open_system_menu(),
        ChordId::Panel => t.toggle_panel(),
        ChordId::PanelFocus => t.focus_panel(),
    }
    Dispatch::Taken
}

/// Control released: a Most recent walk ends.
pub fn on_key_up(ev: &KeyEvent, t: &mut dyn ChordTarget) {
    if ev.key == "Control" {
        t.commit_walk();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeTarget {
        calls: Vec<String>,
        blocked: bool,
        has_tabs: usize,
    }

    impl ChordTarget for FakeTarget {
        fn blocked(&self) -> bool {
            self.blocked
        }
        fn toggle_fullscreen(&mut self) {
            self.calls.push("fullscreen".into());
        }
        fn toggle_help(&mut self) {
            self.calls.push("help".into());
        }
        fn commit_walk(&mut self) {
            self.calls.push("commit_walk".into());
        }
        fn next_tab(&mut self, dir: i32) {
            self.calls.push(format!("next_tab({dir})"));
        }
        fn jump_tab(&mut self, n: usize) -> bool {
            self.calls.push(format!("jump_tab({n})"));
            n <= self.has_tabs
        }
        fn toggle_settings(&mut self) {
            self.calls.push("settings".into());
        }
        fn find_tab(&mut self) {
            self.calls.push("find_tab".into());
        }
        fn toggle_sidebar(&mut self) {
            self.calls.push("sidebar".into());
        }
        fn new_tab(&mut self) {
            self.calls.push("new_tab".into());
        }
        fn close_tab(&mut self) {
            self.calls.push("close_tab".into());
        }
        fn find(&mut self) {
            self.calls.push("find".into());
        }
        fn open_system_menu(&mut self) {
            self.calls.push("system_menu".into());
        }
        fn toggle_panel(&mut self) {
            self.calls.push("panel".into());
        }
        fn focus_panel(&mut self) {
            self.calls.push("panel_focus".into());
        }
    }

    fn on() -> KeyContext {
        KeyContext {
            help: true,
            vertical: true,
            full_screen: false,
            agent_keys: false,
            agent_guard: true,
            agent_here: true,
            agent_working: false,
            agent_selecting: false,
        }
    }

    fn k(key: &str, code: &str) -> KeyEvent {
        KeyEvent::new(key, code)
    }

    #[test]
    fn alt_space_opens_the_system_menu_and_is_taken_even_under_a_question() {
        for blocked in [false, true] {
            let mut t = FakeTarget {
                blocked,
                ..FakeTarget::default()
            };
            let d = dispatch(&k(" ", "Space").alt(), &on(), &mut t);
            assert_eq!(d, Dispatch::Taken);
            assert_eq!(t.calls, vec!["system_menu".to_string()]);
        }
    }

    fn run(ev: &KeyEvent, ctx: &KeyContext) -> (Dispatch, Vec<String>) {
        let mut t = FakeTarget {
            has_tabs: 3,
            ..Default::default()
        };
        let d = dispatch(ev, ctx, &mut t);
        (d, t.calls)
    }

    #[test]
    fn every_chord_calls_its_action() {
        let c = on();
        let cases = [
            (k("t", "KeyT").ctrl(), "new_tab"),
            (k("w", "KeyW").ctrl(), "close_tab"),
            (k(",", "Comma").ctrl(), "settings"),
            (k("K", "KeyK").ctrl().shift(), "find_tab"),
            (k("B", "KeyB").ctrl().shift(), "sidebar"),
            (k("f", "KeyF").ctrl(), "find"),
            (k("F1", "F1"), "help"),
            (k("P", "KeyP").ctrl().shift(), "panel"),
            (k("O", "KeyO").ctrl().shift(), "panel_focus"),
        ];
        for (ev, call) in cases {
            let (d, calls) = run(&ev, &c);
            assert_eq!(d, Dispatch::Taken, "{call}");
            assert!(calls.contains(&call.to_string()), "{call}: {calls:?}");
        }
    }

    #[test]
    fn ctrl_tab_steps_forward_and_shift_back_and_page_keys_with_agent_keys() {
        let c = on();
        assert_eq!(
            run(&k("Tab", "Tab").ctrl(), &c).1,
            vec!["next_tab(1)".to_string()]
        );
        assert_eq!(
            run(&k("Tab", "Tab").ctrl().shift(), &c).1,
            vec!["next_tab(-1)".to_string()]
        );
        let a = KeyContext {
            agent_keys: true,
            ..on()
        };
        assert!(
            run(&k("PageDown", "PageDown").ctrl(), &a)
                .1
                .contains(&"next_tab(1)".to_string())
        );
        assert!(
            run(&k("PageUp", "PageUp").ctrl(), &a)
                .1
                .contains(&"next_tab(-1)".to_string())
        );
    }

    #[test]
    fn the_panel_chords_are_always_taken_so_the_shell_never_hears_them() {
        let mut t = FakeTarget::default();
        assert_eq!(
            dispatch(&k("P", "KeyP").ctrl().shift(), &on(), &mut t),
            Dispatch::Taken
        );
        assert_eq!(
            dispatch(&k("O", "KeyO").ctrl().shift(), &on(), &mut t),
            Dispatch::Taken
        );
        assert!(t.calls.contains(&"panel".to_string()));
        assert!(t.calls.contains(&"panel_focus".to_string()));
        // Plain Ctrl+P and Ctrl+O stay the shell's.
        let (d, calls) = run(&k("p", "KeyP").ctrl(), &on());
        assert_eq!(d, Dispatch::NotAChord);
        assert!(!calls.contains(&"panel".to_string()));
    }

    #[test]
    fn jump_takes_the_key_only_when_the_tab_exists() {
        let c = on();
        assert_eq!(run(&k("2", "Digit2").ctrl(), &c).0, Dispatch::Taken);
        assert_eq!(run(&k("9", "Digit9").ctrl(), &c).0, Dispatch::NotAChord);
    }

    #[test]
    fn f11_acts_and_the_shell_hears_it_too() {
        let (d, calls) = run(&k("F11", "F11"), &on());
        assert_eq!(d, Dispatch::ActedShellToo);
        assert_eq!(calls, vec!["fullscreen".to_string()]);
    }

    #[test]
    fn under_a_question_swallowed_chords_do_nothing_and_close_tab_acts() {
        let c = on();
        let mut t = FakeTarget {
            blocked: true,
            ..Default::default()
        };
        assert_eq!(
            dispatch(&k("t", "KeyT").ctrl(), &c, &mut t),
            Dispatch::Taken
        );
        assert_eq!(
            dispatch(&k("Tab", "Tab").ctrl(), &c, &mut t),
            Dispatch::Taken
        );
        assert!(
            t.calls.iter().all(|s| s == "commit_walk"),
            "no action ran: {:?}",
            t.calls
        );
        t.calls.clear();
        assert_eq!(
            dispatch(&k("w", "KeyW").ctrl(), &c, &mut t),
            Dispatch::Taken
        );
        assert!(t.calls.contains(&"close_tab".to_string()));
    }

    #[test]
    fn agent_guard_is_taken_and_dropped() {
        let (d, calls) = run(&k("d", "KeyD").ctrl(), &on());
        assert_eq!(d, Dispatch::Taken);
        assert!(calls.iter().all(|s| s == "commit_walk"), "{calls:?}");
    }

    #[test]
    fn any_other_ctrl_chord_ends_a_walk_first() {
        let c = on();
        assert_eq!(
            run(&k("t", "KeyT").ctrl(), &c).1,
            vec!["commit_walk".to_string(), "new_tab".to_string()]
        );
        assert!(
            !run(&k("Tab", "Tab").ctrl(), &c)
                .1
                .contains(&"commit_walk".to_string())
        );
        assert!(
            run(&k("Shift", "ShiftLeft").ctrl().shift(), &c)
                .1
                .is_empty()
        );
        assert!(run(&k("Control", "ControlLeft").ctrl(), &c).1.is_empty());
    }

    #[test]
    fn releasing_control_commits_the_walk() {
        let mut t = FakeTarget::default();
        on_key_up(&k("Control", "ControlLeft").released(), &mut t);
        assert_eq!(t.calls, vec!["commit_walk".to_string()]);
        let mut t = FakeTarget::default();
        on_key_up(&k("a", "KeyA").released(), &mut t);
        assert!(t.calls.is_empty());
    }

    #[test]
    fn a_key_up_event_is_not_dispatched() {
        let (d, calls) = run(&k("t", "KeyT").ctrl().released(), &on());
        assert_eq!(d, Dispatch::NotAChord);
        assert!(calls.is_empty());
    }

    #[test]
    fn plain_keys_and_shell_chords_are_not_chords() {
        let c = KeyContext {
            agent_here: false,
            ..on()
        };
        for ev in [k("a", "KeyA"), k("c", "KeyC").ctrl(), k("b", "KeyB").ctrl()] {
            assert_eq!(run(&ev, &c).0, Dispatch::NotAChord, "{ev:?}");
        }
    }
}
