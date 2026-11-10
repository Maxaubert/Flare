//! The Mac menu bar (port phase 3, step 2): builds the `NSMenu` main menu from a `menu_spec` list
//! and hands the app's own items to one callback by their tag.
//!
//! Every app item targets one small `FlareMenuTarget` object, whose `flareMenuItem:` reads the
//! item's tag and calls the handler `install` was given. Standard items (Hide, Minimize, Bring All
//! to Front) send their AppKit action up the responder chain with no target, as Cocoa apps do.
//! The App menu gets the Services submenu, the Window and Help menus are registered with
//! `NSApplication`, so the system adds the window list and its help search there.
//!
//! The menu replaces winit's default one: `install` runs once the window exists, after winit has
//! set its own at launch. A key equivalent is handled by the menu before the window sees the key,
//! so each app item's key is the same Cmd chord the keymap answers to (`flare::mac_menu`), and the
//! handler acts as that key press would.
//!
//! Calls refuse (false, none) off the main thread.

use crate::menu_spec::{Entry, ItemAction, MenuRole, MenuSpec, Mods};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{NSObjectProtocol, NSString};
use std::cell::RefCell;
use std::rc::Rc;

/// What an app item calls, with its tag.
pub type MenuHandler = Rc<dyn Fn(isize)>;

thread_local! {
    static HANDLER: RefCell<Option<MenuHandler>> = const { RefCell::new(None) };
    static TARGET: RefCell<Option<Retained<MenuTarget>>> = const { RefCell::new(None) };
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `MenuTarget` has no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "FlareMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(flareMenuItem:))]
        fn flare_menu_item(&self, sender: &NSMenuItem) {
            let tag = sender.tag();
            // The handler is cloned out first: it may install a new menu, which sets the handler.
            let h = HANDLER.with(|h| h.borrow().clone());
            if let Some(h) = h {
                h(tag);
            }
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's plain init on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// The selector every app item sends.
pub fn app_item_selector() -> Sel {
    sel!(flareMenuItem:)
}

/// AppKit's modifier mask for a key equivalent.
pub fn modifier_mask(m: Mods) -> NSEventModifierFlags {
    let mut f = NSEventModifierFlags::empty();
    if m.cmd {
        f |= NSEventModifierFlags::Command;
    }
    if m.shift {
        f |= NSEventModifierFlags::Shift;
    }
    if m.option {
        f |= NSEventModifierFlags::Option;
    }
    if m.ctrl {
        f |= NSEventModifierFlags::Control;
    }
    f
}

/// The shared target of the app items, made once per thread.
fn target(mtm: MainThreadMarker) -> Retained<MenuTarget> {
    TARGET.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(|| MenuTarget::new(mtm))
            .clone()
    })
}

/// One menu of the bar, its items made from the spec. The Services submenu, when the spec has
/// one, is returned too so the app can register it.
fn build_menu(
    mtm: MainThreadMarker,
    spec: &MenuSpec,
) -> (Retained<NSMenu>, Option<Retained<NSMenu>>) {
    let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(&spec.title));
    let mut services = None;
    let target = target(mtm);
    for entry in &spec.entries {
        let item = match entry {
            Entry::Separator => NSMenuItem::separatorItem(mtm),
            Entry::Item(i) => {
                let action = match &i.action {
                    ItemAction::Tag(_) => Some(app_item_selector()),
                    ItemAction::Selector(s) => Some(Sel::register(
                        &std::ffi::CString::new(*s).unwrap_or_default(),
                    )),
                    ItemAction::Services => None,
                };
                // SAFETY: a menu item made on the main thread; the action is a selector the target
                // (or the responder chain) answers to with one object argument.
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        mtm.alloc(),
                        &NSString::from_str(&i.title),
                        action,
                        &NSString::from_str(&i.key),
                    )
                };
                if !i.key.is_empty() {
                    item.setKeyEquivalentModifierMask(modifier_mask(i.mods));
                }
                match &i.action {
                    ItemAction::Tag(tag) => {
                        item.setTag(*tag);
                        let obj: &AnyObject = &target;
                        // SAFETY: the target lives for the thread (`TARGET`), so the weak
                        // reference AppKit keeps never dangles.
                        unsafe { item.setTarget(Some(obj)) };
                    }
                    ItemAction::Services => {
                        let sub = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(&i.title));
                        item.setSubmenu(Some(&sub));
                        services = Some(sub);
                    }
                    ItemAction::Selector(_) => {}
                }
                item
            }
        };
        menu.addItem(&item);
    }
    (menu, services)
}

/// The whole bar: one top item per menu, each holding its submenu. The menus with a role, in spec
/// order, come back with it so the app can register them.
pub fn build(
    mtm: MainThreadMarker,
    specs: &[MenuSpec],
) -> (Retained<NSMenu>, Vec<(MenuRole, Retained<NSMenu>)>) {
    let bar = NSMenu::new(mtm);
    let mut roles = Vec::new();
    for spec in specs {
        let (menu, services) = build_menu(mtm, spec);
        let top = NSMenuItem::new(mtm);
        top.setTitle(&NSString::from_str(&spec.title));
        top.setSubmenu(Some(&menu));
        bar.addItem(&top);
        if let Some(s) = services {
            roles.push((MenuRole::Plain, s));
        }
        if spec.role != MenuRole::Plain {
            roles.push((spec.role, menu));
        }
    }
    (bar, roles)
}

/// Makes the spec the app's main menu and `handler` the receiver of its app items. False off the
/// main thread. A second call replaces both.
pub fn install(specs: &[MenuSpec], handler: MenuHandler) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
    let (bar, roles) = build(mtm, specs);
    let app = NSApplication::sharedApplication(mtm);
    app.setMainMenu(Some(&bar));
    for (role, menu) in &roles {
        match role {
            // The Services submenu is the one Plain entry `build` hands back.
            MenuRole::Plain => app.setServicesMenu(Some(menu)),
            MenuRole::Window => app.setWindowsMenu(Some(menu)),
            MenuRole::Help => app.setHelpMenu(Some(menu)),
            MenuRole::App => {}
        }
    }
    true
}
