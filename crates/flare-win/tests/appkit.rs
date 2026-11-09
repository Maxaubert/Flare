//! AppKit checks that need the main thread (port phase 3): the glass, the traffic lights, the
//! screens and the e2e park on a real `NSWindow` that is NEVER ordered front, in an app whose
//! activation policy is "prohibited" (no Dock icon, never active). Nothing shows on the screen.
//!
//! libtest runs every test on a worker thread, where AppKit refuses windows, so this binary has
//! its own small harness (`harness = false`): it speaks the part of the libtest command line
//! nextest uses (`--list --format terse`, `--ignored`, `<name> --exact`) and runs the chosen tests
//! on the main thread. Off macOS it lists nothing.

#[cfg(target_os = "macos")]
mod mac {
    use flare_win::chrome::{self, BAR_HEIGHT, LIGHTS_LEFT, LIGHTS_STEP};
    use flare_win::dwm::Backdrop;
    use flare_win::glass;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSApplication,
        NSApplicationActivationPolicy, NSBackingStoreType, NSUserInterfaceItemIdentification,
        NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSWindow, NSWindowButton,
        NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    /// A Flare-shaped window (titled, full-size content, transparent titlebar), never shown.
    fn window(mtm: MainThreadMarker) -> (Retained<NSWindow>, isize) {
        NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        let mask = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        let rect = NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(1000.0, 640.0));
        // SAFETY: a plain window made on the main thread; deferred, so no backing store is made
        // until it would be shown, and it never is.
        let w = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect,
                mask,
                NSBackingStoreType::Buffered,
                true,
            )
        };
        // SAFETY: the window is owned by the Retained here, not released by a close.
        unsafe { w.setReleasedWhenClosed(false) };
        w.setTitlebarAppearsTransparent(true);
        let view = NSView::initWithFrame(mtm.alloc(), rect);
        w.setContentView(Some(&view));
        assert!(!w.isVisible(), "the test window must never show");
        let handle = Retained::as_ptr(&view) as isize;
        (w, handle)
    }

    fn mtm() -> MainThreadMarker {
        MainThreadMarker::new().expect("the AppKit tests run on the main thread")
    }

    fn the_glass_sits_behind_the_content_and_changes_in_place() {
        let mtm = mtm();
        let (w, view) = window(mtm);
        assert!(glass::set_backdrop(view, Backdrop::Acrylic));
        let g = glass::glass_view(&w).expect("a glass view");
        assert_eq!(g.material(), NSVisualEffectMaterial::UnderWindowBackground);
        assert_eq!(g.blendingMode(), NSVisualEffectBlendingMode::BehindWindow);
        assert!(!w.isOpaque());
        let content = w.contentView().expect("content");
        // SAFETY: the window's own content view, read on the main thread.
        let frame = unsafe { content.superview() }.expect("frame view");
        let subs = frame.subviews();
        let at = |v: &NSView| subs.iter().position(|s| std::ptr::eq(&*s, v));
        assert!(at(&g) < at(&content), "the glass is below the content");
        assert_eq!(g.frame().size, frame.bounds().size);
        // A second call changes the material of the same view.
        assert!(glass::set_backdrop(view, Backdrop::Mica));
        let again = glass::glass_view(&w).expect("still there");
        assert!(std::ptr::eq(&*again, &*g));
        assert_eq!(again.material(), NSVisualEffectMaterial::WindowBackground);
        assert_eq!(
            frame
                .subviews()
                .iter()
                .filter(|s| s
                    .identifier()
                    .is_some_and(|i| i.to_string() == glass::GLASS_ID))
                .count(),
            1
        );
        assert!(glass::set_backdrop(view, Backdrop::None));
        assert!(glass::glass_view(&w).is_none());
    }

    fn the_frost_follows_the_grounds_light_or_dark() {
        let mtm = mtm();
        let (w, view) = window(mtm);
        assert!(glass::set_dark(view, true));
        // SAFETY: AppKit's own constant string.
        let dark = unsafe { NSAppearanceNameDarkAqua };
        let name = w.appearance().expect("an appearance").name();
        assert_eq!(*name, *dark);
        assert!(glass::set_dark(view, false));
        assert_ne!(*w.appearance().expect("an appearance").name(), *dark);
    }

    fn origin(w: &NSWindow, b: NSWindowButton) -> NSPoint {
        w.standardWindowButton(b).expect("a light").frame().origin
    }

    fn assert_placed(w: &NSWindow, when: &str) {
        let close = w
            .standardWindowButton(NSWindowButton::CloseButton)
            .expect("close");
        let h = close.frame().size.height;
        let o = chrome::light_origins(BAR_HEIGHT, h);
        for (i, b) in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ]
        .into_iter()
        .enumerate()
        {
            let p = origin(w, b);
            assert_eq!((p.x, p.y), o[i], "light {i} {when}");
        }
        assert_eq!(o[0].0, LIGHTS_LEFT);
        assert_eq!(o[1].0 - o[0].0, LIGHTS_STEP);
        // SAFETY: the close button's own superviews, read on the main thread.
        let container =
            unsafe { close.superview().and_then(|t| t.superview()) }.expect("container");
        assert_eq!(container.frame().size.height, BAR_HEIGHT, "{when}");
        assert_eq!(
            container.frame().size.width,
            f64::from(chrome::CAPTION_GAP),
            "{when}"
        );
        let top = container.frame().origin.y + container.frame().size.height;
        assert_eq!(top, w.frame().size.height, "the row sits at the top {when}");
    }

    fn the_lights_fit_the_top_row_and_stay_there_after_a_resize() {
        let mtm = mtm();
        let (w, view) = window(mtm);
        assert!(chrome::install(view));
        assert!(
            w.collectionBehavior()
                .contains(NSWindowCollectionBehavior::FullScreenPrimary)
        );
        assert_placed(&w, "after install");
        w.setContentSize(NSSize::new(1200.0, 700.0));
        assert_placed(&w, "after a resize");
        w.setTitle(&NSString::from_str("Flare 2"));
        // SAFETY: the window's own frame view, laid out on the main thread.
        if let Some(frame) = w.contentView().and_then(|c| unsafe { c.superview() }) {
            frame.layoutSubtreeIfNeeded();
        }
        w.setContentSize(NSSize::new(1100.0, 700.0));
        assert_placed(&w, "after a new title and a resize");
    }

    fn the_screens_are_device_pixel_areas() {
        let _ = mtm();
        let areas = flare_win::monitors::work_areas();
        for a in &areas {
            assert!(a.width > 0 && a.height > 0, "{a:?}");
        }
        if let Some(a) = areas.first() {
            let s = flare_win::monitors::scale_at(a.x + 1, a.y + 1);
            assert!(s >= 1.0, "scale {s}");
        }
    }

    fn the_e2e_window_parks_clear_and_click_through_as_an_accessory() {
        let mtm = mtm();
        // What an --e2e run does before its first window (rules 334 and 336).
        flare_win::window_park::keep_windows_off_screen();
        let (w, view) = window(mtm);
        flare_win::window_park::park(view);
        // Slint reveals a GPU window with alpha 1 after its first frame; the run keeps it clear.
        w.setAlphaValue(1.0);
        w.setIgnoresMouseEvents(false);
        assert_eq!(w.alphaValue(), 0.0, "the window stays clear");
        assert!(w.ignoresMouseEvents(), "and lets every click through");
        let r = flare_win::window_park::report(view);
        // Accessory, or still prohibited: AppKit does not move a prohibited app to accessory, and
        // this test process starts prohibited. Either way it is never in the Dock.
        assert!(
            [
                flare_win::window_park::POLICY_ACCESSORY,
                flare_win::window_park::POLICY_PROHIBITED
            ]
            .contains(&r.ex_style),
            "policy {}",
            r.ex_style
        );
        // Rule 336: clear and click-through (a window on no screen never paints on a Mac).
        assert!(r.clear, "{r:?}");
        assert!(!r.foreground);
        if r.screen != (0, 0, 0, 0) {
            assert!(r.problems().is_empty(), "{:?}: {:?}", r, r.problems());
        }
    }

    fn the_top_row_reaches_flare_everywhere_but_on_the_lights() {
        let mtm = mtm();
        let (w, view) = window(mtm);
        w.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        assert!(chrome::install(view));
        let content = w.contentView().expect("content");
        // SAFETY: the window's own content view, read on the main thread.
        let frame = unsafe { content.superview() }.expect("frame view");
        frame.layoutSubtreeIfNeeded();
        let h = w.frame().size.height;
        let hits_content = |x: f64, y: f64| {
            frame
                .hitTest(NSPoint::new(x, y))
                .is_some_and(|v| std::ptr::eq(&*v, &*content))
        };
        // The tabs' part of the row, the gap after the lights and the very top edge: Flare's.
        assert!(hits_content(300.0, h - 18.0));
        assert!(hits_content(75.0, h - 18.0));
        assert!(hits_content(5.0, h - 2.0));
        // The middle of the close light: AppKit's.
        let o = chrome::light_origins(BAR_HEIGHT, 16.0);
        assert!(!hits_content(o[0].0 + 7.0, h - BAR_HEIGHT / 2.0));
    }

    fn spec() -> Vec<flare_win::menu_spec::MenuSpec> {
        use flare_win::menu_spec::{
            Entry, ItemAction, ItemSpec, MenuRole, MenuSpec, Mods, item, native,
        };
        vec![
            MenuSpec {
                title: "Flare".into(),
                role: MenuRole::App,
                entries: vec![
                    item("Settings", ",", Mods::CMD, 1),
                    Entry::Separator,
                    Entry::Item(ItemSpec {
                        title: "Services".into(),
                        key: String::new(),
                        mods: Mods::NONE,
                        action: ItemAction::Services,
                    }),
                    native("Hide Flare", "h", Mods::CMD, "hide:"),
                    native(
                        "Hide Others",
                        "h",
                        Mods::CMD_OPTION,
                        "hideOtherApplications:",
                    ),
                    item("Quit Flare", "q", Mods::CMD, 2),
                ],
            },
            MenuSpec {
                title: "File".into(),
                role: MenuRole::Plain,
                entries: vec![item("New Tab", "t", Mods::CMD, 3)],
            },
            MenuSpec {
                title: "View".into(),
                role: MenuRole::Plain,
                entries: vec![
                    item("Enter Full Screen", "f", Mods::CTRL_CMD, 4),
                    item("Paste as Plain Text", "v", Mods::CMD_SHIFT, 5),
                ],
            },
            MenuSpec {
                title: "Window".into(),
                role: MenuRole::Window,
                entries: vec![native("Minimize", "m", Mods::CMD, "performMiniaturize:")],
            },
            MenuSpec {
                title: "Help".into(),
                role: MenuRole::Help,
                entries: vec![item("Command Help", "", Mods::NONE, 6)],
            },
        ]
    }

    fn the_menu_bar_is_built_from_the_spec_with_its_keys_and_actions() {
        use flare_win::menu_bar;
        use objc2_app_kit::NSEventModifierFlags as F;
        let mtm = mtm();
        let (bar, roles) = menu_bar::build(mtm, &spec());
        let titles: Vec<String> = bar
            .itemArray()
            .iter()
            .map(|i| i.title().to_string())
            .collect();
        assert_eq!(titles, ["Flare", "File", "View", "Window", "Help"]);
        let flare = bar
            .itemAtIndex(0)
            .and_then(|i| i.submenu())
            .expect("the app menu");
        assert_eq!(flare.numberOfItems(), 6);
        let settings = flare.itemAtIndex(0).expect("settings");
        assert_eq!(settings.keyEquivalent().to_string(), ",");
        assert_eq!(settings.keyEquivalentModifierMask(), F::Command);
        assert_eq!(settings.tag(), 1);
        assert_eq!(settings.action(), Some(menu_bar::app_item_selector()));
        assert!(
            settings.target().is_some(),
            "an app item has the menu target"
        );
        assert!(flare.itemAtIndex(1).expect("separator").isSeparatorItem());
        let services = flare.itemAtIndex(2).expect("services");
        assert!(services.submenu().is_some());
        let hide = flare.itemAtIndex(3).expect("hide");
        assert_eq!(hide.action(), Some(objc2::sel!(hide:)));
        assert!(
            hide.target().is_none(),
            "a standard item goes up the responder chain"
        );
        let others = flare.itemAtIndex(4).expect("hide others");
        assert_eq!(others.keyEquivalentModifierMask(), F::Command | F::Option);
        let view = bar.itemAtIndex(2).and_then(|i| i.submenu()).expect("view");
        let full = view.itemAtIndex(0).expect("full screen");
        assert_eq!(full.keyEquivalentModifierMask(), F::Command | F::Control);
        let plain = view.itemAtIndex(1).expect("paste plain");
        assert_eq!(plain.keyEquivalentModifierMask(), F::Command | F::Shift);
        let help = bar.itemAtIndex(4).and_then(|i| i.submenu()).expect("help");
        assert_eq!(
            help.itemAtIndex(0)
                .expect("help item")
                .keyEquivalent()
                .to_string(),
            ""
        );
        let kinds: Vec<flare_win::menu_spec::MenuRole> = roles.iter().map(|(r, _)| *r).collect();
        use flare_win::menu_spec::MenuRole;
        assert_eq!(
            kinds,
            [
                MenuRole::Plain,
                MenuRole::App,
                MenuRole::Window,
                MenuRole::Help
            ]
        );
    }

    fn a_menu_item_and_its_key_equivalent_reach_the_handler_with_the_tag() {
        use flare_win::menu_bar;
        use objc2_app_kit::{NSEvent, NSEventModifierFlags as F, NSEventType};
        use std::cell::RefCell;
        use std::rc::Rc;
        let mtm = mtm();
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        let got = Rc::new(RefCell::new(Vec::new()));
        let sink = got.clone();
        assert!(menu_bar::install(
            &spec(),
            Rc::new(move |t| sink.borrow_mut().push(t))
        ));
        let main = app.mainMenu().expect("the main menu is ours");
        assert_eq!(main.numberOfItems(), 5);
        assert!(app.windowsMenu().is_some());
        assert!(app.helpMenu().is_some());
        assert!(app.servicesMenu().is_some());
        // Choosing New Tab.
        let file = main.itemAtIndex(1).and_then(|i| i.submenu()).expect("file");
        file.performActionForItemAtIndex(0);
        assert_eq!(*got.borrow(), vec![3]);
        // Cmd+Q as a key event: the menu takes it and the handler hears Quit.
        let ev = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::new(0.0, 0.0),
            F::Command,
            0.0,
            0,
            None,
            &NSString::from_str("q"),
            &NSString::from_str("q"),
            false,
            12,
        )
        .expect("a key event");
        assert!(main.performKeyEquivalent(&ev), "the menu takes Cmd+Q");
        assert_eq!(*got.borrow(), vec![3, 2]);
        // A key no item has is left to the window.
        let other = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::new(0.0, 0.0),
            F::Command,
            0.0,
            0,
            None,
            &NSString::from_str("j"),
            &NSString::from_str("j"),
            false,
            38,
        )
        .expect("a key event");
        assert!(!main.performKeyEquivalent(&other));
        assert_eq!(got.borrow().len(), 2);
    }

    /// The Dock calls on the main thread, in an app whose policy is "prohibited": it has no Dock tile to
    /// draw on and is never active, so the badge shows nothing anywhere.
    fn the_dock_badge_reads_back_and_attention_is_asked_only_while_flare_is_not_active() {
        use flare_win::{dock, taskbar, taskbar_flash};
        let mtm = mtm();
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        assert!(!dock::is_active(), "a prohibited app is never active");
        assert!(!taskbar_flash::is_foreground(0));
        assert!(dock::set_badge_label(Some("3")));
        assert_eq!(dock::badge_label().as_deref(), Some("3"));
        assert!(dock::set_badge_label(None));
        assert_eq!(dock::badge_label(), None);
        // The taskbar badge on macOS is the Dock tile's label.
        let mut badge = taskbar::TaskbarBadge::new(0);
        assert!(badge.set(12, 2.0));
        assert_eq!(dock::badge_label().as_deref(), Some("9+"));
        assert!(badge.clear());
        assert_eq!(dock::badge_label(), None);
        // Not active, so a bell would ask for a critical request. The request itself is never made
        // from a test (the planner and its AppKit type are checked in `dock::tests`).
        assert_eq!(
            dock::attention_for(dock::is_active()),
            Some(dock::Attention::Critical)
        );
    }

    pub const TESTS: &[(&str, fn())] = &[
        (
            "dock_badge_reads_back_and_attention_is_asked_only_while_flare_is_not_active",
            the_dock_badge_reads_back_and_attention_is_asked_only_while_flare_is_not_active,
        ),
        (
            "menu_bar_is_built_from_the_spec_with_its_keys_and_actions",
            the_menu_bar_is_built_from_the_spec_with_its_keys_and_actions,
        ),
        (
            "a_menu_item_and_its_key_equivalent_reach_the_handler_with_the_tag",
            a_menu_item_and_its_key_equivalent_reach_the_handler_with_the_tag,
        ),
        (
            "top_row_reaches_flare_everywhere_but_on_the_lights",
            the_top_row_reaches_flare_everywhere_but_on_the_lights,
        ),
        (
            "glass_sits_behind_the_content_and_changes_in_place",
            the_glass_sits_behind_the_content_and_changes_in_place,
        ),
        (
            "frost_follows_the_grounds_light_or_dark",
            the_frost_follows_the_grounds_light_or_dark,
        ),
        (
            "lights_fit_the_top_row_and_stay_there_after_a_resize",
            the_lights_fit_the_top_row_and_stay_there_after_a_resize,
        ),
        (
            "screens_are_device_pixel_areas",
            the_screens_are_device_pixel_areas,
        ),
        (
            "e2e_window_parks_clear_and_click_through_as_an_accessory",
            the_e2e_window_parks_clear_and_click_through_as_an_accessory,
        ),
    ];
}

#[cfg(target_os = "macos")]
use mac::TESTS;
#[cfg(not(target_os = "macos"))]
const TESTS: &[(&str, fn())] = &[];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let ignored = flag("--ignored");
    if flag("--list") {
        if !ignored {
            for (name, _) in TESTS {
                println!("{name}: test");
            }
        }
        return;
    }
    if ignored {
        return;
    }
    let exact = flag("--exact");
    let skip_value = ["--format", "--test-threads", "--color", "-Z"];
    let mut filters = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if skip_value.contains(&a.as_str()) {
            it.next();
        } else if !a.starts_with('-') {
            filters.push(a.as_str());
        }
    }
    let chosen = |name: &str| {
        filters.is_empty()
            || filters
                .iter()
                .any(|f| if exact { name == *f } else { name.contains(f) })
    };
    let mut failed = 0;
    for (name, test) in TESTS.iter().filter(|(n, _)| chosen(n)) {
        match std::panic::catch_unwind(test) {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        std::process::exit(101);
    }
}
