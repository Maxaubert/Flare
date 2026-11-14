//! Rule 1238: a key whose press went to Slint has its release go there too. Slint keeps the modifiers from its own key
//! events and clears them only on a focus loss, so a swallowed Ctrl or Shift release left it held: after Ctrl+A in a
//! Settings field letters were dropped and Backspace deleted words, after Shift an arrow selected.
//!
//! The keys go through `window::handle_key` as winit reports them (a modifier is a key of its own, its flag set on
//! its press and cleared on its release), and what the window lets through reaches Slint as its winit backend sends
//! it: the modifier key itself, the logical key, never a chord folded into one event.

use slint::platform::WindowEvent;
use slint::winit_030::EventResult;

use super::*;
use crate::SettingsBridge;
use crate::e2e_glue::{slint_modifier_of, slint_text_for};
use crate::host::{GridHost, GridPainter};
use crate::ime::CaretFollower;
use crate::window::{KeyPath, handle_focus, handle_key, release_outcome};

/// The key path the window builds, over the fixture's controller and window.
fn key_path(f: &Fx, front: Option<Arc<TermHandle>>) -> KeyPath {
    KeyPath::new(
        Rc::downgrade(&f.ctl),
        Rc::new(RefCell::new(front)),
        Rc::new(RefCell::new(CaretFollower::new())),
        f.h.comp.as_weak(),
        Rc::new(RefCell::new(GridHost::new(
            Box::new(FakeGlyphs::new(8, 16)),
            GridPainter::Cpu,
        ))),
    )
}

/// A modifier key as winit reports it: its own flag on the press, none on the release.
fn modifier(code: &str, key: &str, pressed: bool) -> KeyEvent {
    let mut ev = KeyEvent::new(key, code);
    ev.pressed = pressed;
    match code {
        "ControlLeft" => ev.ctrl = pressed,
        "SuperLeft" => ev.meta = pressed,
        "ShiftLeft" => ev.shift = pressed,
        _ => {}
    }
    ev
}

/// The key that selects all with A: Cmd on a Mac, Ctrl elsewhere, as (code, key name).
fn select_all_mod() -> (&'static str, &'static str) {
    if cfg!(target_vendor = "apple") {
        ("SuperLeft", "Meta")
    } else {
        ("ControlLeft", "Control")
    }
}

/// A character key; with the select-all modifier (`ctrl`) held winit's text is dropped (`key_event_from_parts`).
fn letter(c: &str, pressed: bool, ctrl: bool, shift: bool) -> KeyEvent {
    let code = format!("Key{}", c.to_ascii_uppercase());
    let mut ev = KeyEvent::new(c, &code);
    ev.pressed = pressed;
    if cfg!(target_vendor = "apple") {
        ev.meta = ctrl;
    } else {
        ev.ctrl = ctrl;
    }
    ev.shift = shift;
    if !ctrl {
        ev.text = Some(c.to_string());
    }
    ev
}

/// A named key (Backspace, ArrowLeft) with no modifier.
fn named(key: &str, pressed: bool) -> KeyEvent {
    let mut ev = KeyEvent::new(key, key);
    ev.pressed = pressed;
    ev
}

/// What Slint's winit backend hands Slint for a key the window let through.
fn slint_text(ev: &KeyEvent) -> Option<slint::SharedString> {
    if let Some(m) = slint_modifier_of(&ev.code) {
        return Some(char::from(m).into());
    }
    if ev.key.chars().count() == 1 {
        return Some(ev.key.as_str().into());
    }
    slint_text_for(&KeyEvent {
        text: None,
        ..ev.clone()
    })
}

/// One winit key event through the window: `handle_key`, then Slint when the window lets it through.
fn winit_key(f: &Fx, kp: &KeyPath, ev: &KeyEvent) -> EventResult {
    let result = handle_key(kp, ev);
    if matches!(result, EventResult::Propagate)
        && let Some(text) = slint_text(ev)
    {
        let e = if ev.pressed {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        };
        f.h.comp.window().dispatch_event(e);
    }
    f.h.advance(0);
    result
}

/// A key pressed and let go.
fn tap(f: &Fx, kp: &KeyPath, down: KeyEvent) {
    let mut up = down.clone();
    up.pressed = false;
    winit_key(f, kp, &down);
    winit_key(f, kp, &up);
}

fn type_text(f: &Fx, kp: &KeyPath, s: &str) {
    for c in s.chars() {
        tap(f, kp, letter(&c.to_string(), true, false, false));
    }
}

/// Ctrl down (Cmd on a Mac), A down and up, Ctrl up, as a person selects all.
fn ctrl_a(f: &Fx, kp: &KeyPath) -> EventResult {
    let (code, key) = select_all_mod();
    let down = winit_key(f, kp, &modifier(code, key, true));
    tap(f, kp, letter("a", true, true, false));
    let up = winit_key(f, kp, &modifier(code, key, false));
    assert_eq!(
        matches!(down, EventResult::Propagate),
        matches!(up, EventResult::Propagate),
        "Ctrl's release goes where its press went"
    );
    up
}

/// Shift down, Shift+A down and up, Shift up.
fn shift_a(f: &Fx, kp: &KeyPath) -> EventResult {
    let down = winit_key(f, kp, &modifier("ShiftLeft", "Shift", true));
    tap(f, kp, letter("A", true, false, true));
    let up = winit_key(f, kp, &modifier("ShiftLeft", "Shift", false));
    assert_eq!(
        matches!(down, EventResult::Propagate),
        matches!(up, EventResult::Propagate),
        "Shift's release goes where its press went"
    );
    up
}

/// The Settings tab in front, its Find field holding the keys with `text` in it. The controller's worker answers
/// (the shell list, the Explorer verb) are waited for first: a late one draws the page again and drops the focus.
fn settings_field(text: &str) -> Fx {
    let f = fx();
    let post: Post = {
        let q = f.q.clone();
        Arc::new(move |m| q.lock().unwrap().push_back(m))
    };
    let clock = f.clock.clone();
    let settings = crate::settings_ctl::SettingsCtl::new(
        &f.h.comp,
        crate::settings_scene::deps(&f.prefs, post, Rc::new(move || clock.get())),
    );
    {
        let mut c = f.ctl.borrow_mut();
        c.set_settings(settings);
        c.model_mut().open_settings();
        c.refresh();
    }
    let (mut shells, mut verb) = (false, false);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !(shells && verb) {
        let m = f.q.lock().unwrap().pop_front();
        match m {
            Some(m) => {
                if let UiMsg::Settings(s) = &m {
                    shells |= matches!(s, crate::settings_ctl::SettingsMsg::ShellList(_));
                    verb |= matches!(s, crate::settings_ctl::SettingsMsg::Verb(_));
                }
                f.ctl.borrow_mut().on_msg(m);
            }
            None => {
                assert!(Instant::now() < deadline, "the Settings answers never came");
                std::thread::sleep(Duration::from_millis(5)); // sync-ok: test code only
            }
        }
    }
    f.pump();
    f.h.advance(0);
    assert_eq!(f.h.comp.get_front_kind(), 1, "Settings is in front");
    f.h.comp
        .global::<SettingsBridge>()
        .set_find_text(text.into());
    f.h.activate();
    f.h.comp.invoke_settings_focus_find();
    f.h.advance(0);
    f
}

fn settings_text(f: &Fx) -> String {
    f.h.comp
        .global::<SettingsBridge>()
        .get_find_text()
        .to_string()
}

#[test]
fn after_ctrl_a_in_a_settings_field_backspace_clears_it_and_letters_type() {
    let f = settings_field("xyz");
    let kp = key_path(&f, None);
    assert!(
        matches!(ctrl_a(&f, &kp), EventResult::Propagate),
        "Slint hears Ctrl let go"
    );
    tap(&f, &kp, named("Backspace", true));
    type_text(&f, &kp, "abc");
    assert_eq!(settings_text(&f), "abc");
}

#[test]
fn after_shift_in_a_settings_field_an_arrow_moves_the_caret_without_selecting() {
    let f = settings_field("");
    let kp = key_path(&f, None);
    assert!(matches!(shift_a(&f, &kp), EventResult::Propagate));
    assert_eq!(settings_text(&f), "A");
    tap(&f, &kp, named("ArrowLeft", true));
    type_text(&f, &kp, "b");
    assert_eq!(
        settings_text(&f),
        "bA",
        "the arrow moved the caret; nothing was selected and typed over"
    );
}

/// A live tab with a text file in its panel, the panel holding the keys.
fn panel_focused() -> (Fx, KeyPath) {
    let (f, h, _) = live(Opts::default());
    show_text(&f, "a.txt", "one\ntwo");
    f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
    f.h.activate();
    f.h.comp.invoke_focus_panel();
    f.h.advance(0);
    assert!(f.h.comp.get_panel_has_focus());
    let kp = key_path(&f, Some(h));
    (f, kp)
}

/// The keys move from the panel to the find bar: the controller opens it (its chord, straight to the controller so
/// the key path under test never sees it) and the bar takes the keys.
fn to_find_bar(f: &Fx) {
    assert_eq!(f.key(ctrl("f", "KeyF")), KeyOutcome::Consumed);
    f.pump();
    f.h.advance(0);
    assert!(f.h.comp.get_find_open());
    assert!(f.h.comp.get_find_input_focused());
}

#[test]
fn ctrl_a_in_the_panel_leaves_no_ctrl_held_for_the_next_field() {
    let (f, kp) = panel_focused();
    assert!(
        matches!(ctrl_a(&f, &kp), EventResult::Propagate),
        "Slint hears Ctrl let go"
    );
    to_find_bar(&f);
    type_text(&f, &kp, "abc");
    assert_eq!(f.h.comp.get_find_query().as_str(), "abc");
}

#[test]
fn shift_in_the_panel_leaves_no_shift_held_for_the_next_field() {
    let (f, kp) = panel_focused();
    assert!(matches!(shift_a(&f, &kp), EventResult::Propagate));
    to_find_bar(&f);
    type_text(&f, &kp, "x");
    tap(&f, &kp, named("ArrowLeft", true));
    type_text(&f, &kp, "b");
    assert_eq!(f.h.comp.get_find_query().as_str(), "bx");
}

#[test]
fn a_shift_release_goes_where_its_press_went_and_a_focus_loss_forgets_it() {
    let f = settings_field("");
    let kp = key_path(&f, None);
    assert!(matches!(
        handle_key(&kp, &modifier("ShiftLeft", "Shift", true)),
        EventResult::Propagate
    ));
    assert!(kp.slint_down.borrow().contains("ShiftLeft"));
    assert!(matches!(
        handle_key(&kp, &modifier("ShiftLeft", "Shift", false)),
        EventResult::Propagate
    ));
    assert!(kp.slint_down.borrow().is_empty());
    // Slint lets go of every key on a focus loss: nothing is owed to it after one.
    let _ = handle_key(&kp, &letter("q", true, false, false));
    assert!(kp.slint_down.borrow().contains("KeyQ"));
    handle_focus(&kp, false);
    assert!(kp.slint_down.borrow().is_empty());
}

#[test]
fn a_modifier_release_on_a_terminal_reaches_the_shell_and_slint() {
    let (f, h, _) = live(Opts::default());
    let kp = key_path(&f, Some(h));
    assert!(matches!(
        handle_key(&kp, &modifier("ShiftLeft", "Shift", true)),
        EventResult::PreventDefault
    ));
    assert!(
        matches!(
            handle_key(&kp, &modifier("ShiftLeft", "Shift", false)),
            EventResult::Propagate
        ),
        "a modifier let go is never swallowed"
    );
    // A plain key's release stays the shell's.
    let _ = handle_key(&kp, &letter("q", true, false, false));
    assert!(matches!(
        handle_key(&kp, &letter("q", false, false, false)),
        EventResult::PreventDefault
    ));
}

#[test]
fn the_release_follows_the_press_first() {
    assert_eq!(release_outcome(false, true, false), KeyOutcome::Slint);
    assert_eq!(release_outcome(true, true, false), KeyOutcome::Slint);
    assert_eq!(release_outcome(true, false, true), KeyOutcome::Consumed);
    assert_eq!(release_outcome(false, false, true), KeyOutcome::Slint);
    assert_eq!(release_outcome(false, false, false), KeyOutcome::Pty);
}

#[test]
fn a_lone_modifier_is_a_modifier_code() {
    for c in [
        "ShiftLeft",
        "ControlRight",
        "AltRight",
        "SuperLeft",
        "MetaLeft",
    ] {
        assert!(crate::keymap::is_modifier_code(c), "{c}");
    }
    for c in ["KeyA", "Backspace", "CapsLock"] {
        assert!(!crate::keymap::is_modifier_code(c), "{c}");
    }
}
