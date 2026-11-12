//! What the web panel's page asks the person, shown on the Mac window as WebView2 shows its own on Windows: a
//! script dialog (`alert`, `confirm`, `prompt`) and the leave-page question as an `NSAlert` sheet, a file chooser as
//! an `NSOpenPanel` (or `NSSavePanel`) sheet, and the right-click menu as an `NSMenu` where the click was. The CEF
//! view holds each one (`flare_cef::macos::view::ViewEvent::Ask`) until the reply comes back through the host's
//! queue.
//!
//! Only the plain web panel asks: a mini-app's and a plugin's views keep their seals ([`crate::seal`]), so their
//! dialogs are still answered at once and never shown. A host with no window (the tests, parent 0) answers every ask
//! no at once ([`NoWindow`]), and the tests put in an asker of their own ([`super::host::WebHost::set_asker`]):
//! nothing here may come on screen in a test.

use flare_cef::macos::view::{Ask, FileMode, MenuItem, Reply};
use flare_surfaces::slot::Slot;
use flare_surfaces::web::{PhysRect, host_label};

/// Which held ask a sheet answers: the view (its slot and generation) and the view's id for it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AskKey {
    pub slot: Slot,
    pub generation: u64,
    pub id: u64,
}

/// Takes the person's reply, once (it only queues it for the host's next turn).
pub type Done = Box<dyn FnOnce(Reply) + Send>;

/// Puts a page's ask in front of the person.
pub trait Asker {
    /// Shows `ask`. `page` is the page's top-left corner in the window's content view (points from its top left),
    /// where a menu's (x, y) start. `done` gets the reply.
    fn ask(&mut self, key: AskKey, ask: Ask, page: (f64, f64), done: Done);
    /// The asks `gone` picks are over (the page went on, the view closed): their sheets close.
    fn withdraw(&mut self, gone: &dyn Fn(&AskKey) -> bool);
}

/// No window to ask in: every ask is answered no at once, as before the asks were shown.
pub struct NoWindow;

impl Asker for NoWindow {
    fn ask(&mut self, _key: AskKey, _ask: Ask, _page: (f64, f64), done: Done) {
        done(Reply::No);
    }

    fn withdraw(&mut self, _gone: &dyn Fn(&AskKey) -> bool) {}
}

/// The page's top-left corner in points from the content view's top left (its box is in device pixels).
pub fn page_origin(bounds: PhysRect, scale: f64) -> (f64, f64) {
    let k = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (f64::from(bounds.x) / k, f64::from(bounds.y) / k)
}

/// A script dialog's heading, as Chromium words it: "example.com says", or "This page says" without a host.
pub fn script_heading(origin: &str) -> String {
    let host = host_label(origin);
    if host.is_empty() || host == origin {
        "This page says".to_string()
    } else {
        format!("{host} says")
    }
}

/// The leave-page question: heading, text and the yes button (Chromium's words).
pub fn leave_words(reload: bool) -> (&'static str, &'static str, &'static str) {
    if reload {
        (
            "Reload site?",
            "Changes you made may not be saved.",
            "Reload",
        )
    } else {
        ("Leave site?", "Changes you made may not be saved.", "Leave")
    }
}

/// The sheets on the window now, by the ask each answers.
type OpenSheets =
    std::rc::Rc<std::cell::RefCell<Vec<(AskKey, objc2::rc::Retained<objc2_app_kit::NSWindow>)>>>;

/// The AppKit asker: sheets on the window whose content view is `view`, and the menu in that view.
pub struct Sheets {
    view: isize,
    open: OpenSheets,
}

impl Sheets {
    /// `view` is the window's content `NSView` (the host's parent); 0 gives a [`NoWindow`].
    pub fn for_view(view: isize) -> Box<dyn Asker> {
        if view == 0 {
            return Box::new(NoWindow);
        }
        Box::new(Sheets {
            view,
            open: Default::default(),
        })
    }
}

impl Asker for Sheets {
    fn ask(&mut self, key: AskKey, ask: Ask, page: (f64, f64), done: Done) {
        appkit::ask(self.view, &self.open, key, ask, page, done);
    }

    fn withdraw(&mut self, gone: &dyn Fn(&AskKey) -> bool) {
        appkit::withdraw(self.view, &self.open, gone);
    }
}

mod appkit {
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::rc::Rc;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSControlStateValueOff, NSControlStateValueOn, NSMenu,
        NSMenuItem, NSModalResponse, NSModalResponseOK, NSOpenPanel, NSSavePanel, NSTextField,
        NSView, NSWindow,
    };
    use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};

    use super::*;

    type Open = OpenSheets;

    thread_local! {
        /// The tag of the page menu's row the person picked (set by the row's action, read when the menu closes).
        static PICKED: Cell<Option<isize>> = const { Cell::new(None) };
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and `PageMenuTarget` has no `Drop`.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "FlareWebPageMenuTarget"]
        struct PageMenuTarget;

        impl PageMenuTarget {
            #[unsafe(method(flarePageMenuItem:))]
            fn picked(&self, sender: &NSMenuItem) {
                PICKED.with(|p| p.set(Some(sender.tag())));
            }
        }

        unsafe impl NSObjectProtocol for PageMenuTarget {}
    );

    impl PageMenuTarget {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            // SAFETY: NSObject's plain init on a freshly allocated object.
            unsafe { msg_send![super(this), init] }
        }
    }

    fn ns(s: &str) -> Retained<NSString> {
        NSString::from_str(s)
    }

    fn content_view<'a>(view: isize) -> &'a NSView {
        // SAFETY: the host's parent is the live content view of Flare's window (the AppKit handle raw-window-handle
        // gave the window), which lives as long as the window; we are on the main thread.
        unsafe { &*(view as *const NSView) }
    }

    /// A sheet's reply: it forgets the sheet, then queues the reply.
    type Finish = Box<dyn FnOnce(Reply)>;

    /// The reply slot a block takes once (AppKit's completion blocks are `Fn`).
    fn once(done: Finish) -> Rc<Cell<Option<Finish>>> {
        Rc::new(Cell::new(Some(done)))
    }

    pub(super) fn ask(
        view: isize,
        open: &Open,
        key: AskKey,
        ask: Ask,
        page: (f64, f64),
        done: Done,
    ) {
        let Some(mtm) = MainThreadMarker::new() else {
            return done(Reply::No);
        };
        if let Ask::Menu { x, y, items } = ask {
            // After this turn: the menu runs its own tracking loop, which must not run inside the host's pump.
            let at = (page.0 + f64::from(x), page.1 + f64::from(y));
            dispatch2::DispatchQueue::main().exec_async(move || {
                let reply = MainThreadMarker::new()
                    .map(|mtm| run_menu(mtm, view, &items, at))
                    .unwrap_or(Reply::No);
                done(reply);
            });
            return;
        }
        let Some(window) = content_view(view).window() else {
            return done(Reply::No);
        };
        let (o, k) = (Rc::clone(open), key.clone());
        let done: Finish = Box::new(move |r| {
            o.borrow_mut().retain(|(x, _)| *x != k);
            done(r);
        });
        let sheet = match ask {
            Ask::Script {
                kind,
                message,
                default,
                origin,
            } => script(mtm, &window, kind, &message, &default, &origin, done),
            Ask::Leave { reload } => leave(mtm, &window, reload, done),
            Ask::Files {
                mode,
                title,
                default_path,
                extensions,
            } => files(mtm, &window, mode, &title, &default_path, &extensions, done),
            Ask::Menu { .. } => return,
        };
        open.borrow_mut().push((key, sheet));
    }

    pub(super) fn withdraw(view: isize, open: &Open, gone: &dyn Fn(&AskKey) -> bool) {
        let ended: Vec<Retained<NSWindow>> = {
            let mut o = open.borrow_mut();
            let (out, keep): (Vec<_>, Vec<_>) = o.drain(..).partition(|(k, _)| gone(k));
            *o = keep;
            out.into_iter().map(|(_, w)| w).collect()
        };
        let Some(window) = content_view(view).window() else {
            return;
        };
        for sheet in ended {
            window.endSheet(&sheet);
        }
    }

    fn alert_sheet(
        window: &NSWindow,
        alert: &NSAlert,
        on_end: impl Fn(NSModalResponse) + 'static,
    ) -> Retained<NSWindow> {
        let block = RcBlock::new(on_end);
        alert.beginSheetModalForWindow_completionHandler(window, Some(&block));
        alert.window()
    }

    fn script(
        mtm: MainThreadMarker,
        window: &NSWindow,
        kind: &str,
        message: &str,
        default: &str,
        origin: &str,
        done: Finish,
    ) -> Retained<NSWindow> {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&ns(&script_heading(origin)));
        alert.setInformativeText(&ns(message));
        alert.addButtonWithTitle(&ns("OK"));
        if kind != "alert" {
            alert.addButtonWithTitle(&ns("Cancel"));
        }
        let field = (kind == "prompt").then(|| {
            let f = NSTextField::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, 24.0)),
            );
            f.setStringValue(&ns(default));
            alert.setAccessoryView(Some(&f));
            f
        });
        if let Some(f) = &field {
            alert.window().setInitialFirstResponder(Some(f));
        }
        let done = once(done);
        alert_sheet(window, &alert, move |r| {
            let reply = if r == NSAlertFirstButtonReturn {
                Reply::Yes(
                    field
                        .as_ref()
                        .map(|f| f.stringValue().to_string())
                        .unwrap_or_default(),
                )
            } else {
                Reply::No
            };
            if let Some(d) = done.take() {
                d(reply);
            }
        })
    }

    fn leave(
        mtm: MainThreadMarker,
        window: &NSWindow,
        reload: bool,
        done: Finish,
    ) -> Retained<NSWindow> {
        let (heading, text, yes) = leave_words(reload);
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&ns(heading));
        alert.setInformativeText(&ns(text));
        alert.addButtonWithTitle(&ns(yes));
        alert.addButtonWithTitle(&ns("Cancel"));
        let done = once(done);
        alert_sheet(window, &alert, move |r| {
            let reply = if r == NSAlertFirstButtonReturn {
                Reply::Yes(String::new())
            } else {
                Reply::No
            };
            if let Some(d) = done.take() {
                d(reply);
            }
        })
    }

    fn path_of(url: &NSURL) -> Option<PathBuf> {
        url.path().map(|p| PathBuf::from(p.to_string()))
    }

    fn files(
        mtm: MainThreadMarker,
        window: &NSWindow,
        mode: FileMode,
        title: &str,
        default_path: &str,
        extensions: &[String],
        done: Finish,
    ) -> Retained<NSWindow> {
        let open_panel = (mode != FileMode::Save).then(|| {
            let p = NSOpenPanel::openPanel(mtm);
            p.setCanChooseFiles(mode != FileMode::Folder);
            p.setCanChooseDirectories(mode == FileMode::Folder);
            p.setAllowsMultipleSelection(mode == FileMode::OpenMany);
            p
        });
        let panel: Retained<NSSavePanel> = match &open_panel {
            Some(p) => Retained::into_super(p.clone()),
            None => NSSavePanel::savePanel(mtm),
        };
        if !title.is_empty() {
            panel.setMessage(Some(&ns(title)));
        }
        let default = std::path::Path::new(default_path);
        if !default_path.is_empty() {
            let dir = if default.is_dir() {
                Some(default)
            } else {
                default.parent()
            };
            if let Some(dir) = dir.filter(|d| d.is_dir()) {
                let url = NSURL::fileURLWithPath(&ns(&dir.to_string_lossy()));
                panel.setDirectoryURL(Some(&url));
            }
            if mode == FileMode::Save
                && let Some(name) = default.file_name()
            {
                panel.setNameFieldStringValue(&ns(&name.to_string_lossy()));
            }
        }
        if !extensions.is_empty() && mode != FileMode::Folder {
            let types: Vec<Retained<NSString>> = extensions.iter().map(|e| ns(e)).collect();
            let types = NSArray::from_retained_slice(&types);
            #[allow(deprecated)]
            panel.setAllowedFileTypes(Some(&types));
        }
        let done = once(done);
        let read = panel.clone();
        let block = RcBlock::new(move |r: NSModalResponse| {
            let paths: Vec<PathBuf> = if r != NSModalResponseOK {
                Vec::new()
            } else if let Some(p) = &open_panel {
                p.URLs().iter().filter_map(|u| path_of(&u)).collect()
            } else {
                read.URL().and_then(|u| path_of(&u)).into_iter().collect()
            };
            let reply = if paths.is_empty() {
                Reply::No
            } else {
                Reply::Files(paths)
            };
            if let Some(d) = done.take() {
                d(reply);
            }
        });
        panel.beginSheetModalForWindow_completionHandler(window, &block);
        Retained::into_super(Retained::into_super(panel))
    }

    /// The page's menu at `at` (points from the content view's top left); the row picked, or no.
    fn run_menu(mtm: MainThreadMarker, view: isize, items: &[MenuItem], at: (f64, f64)) -> Reply {
        let host = content_view(view);
        if host.window().is_none() {
            return Reply::No;
        }
        let menu = NSMenu::initWithTitle(mtm.alloc(), &ns(""));
        menu.setAutoenablesItems(false);
        let target = PageMenuTarget::new(mtm);
        for i in items {
            if i.is_separator() {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
                continue;
            }
            // SAFETY: the action is the target's own method, which takes the sending item.
            let row = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &ns(&i.label),
                    Some(sel!(flarePageMenuItem:)),
                    &ns(""),
                )
            };
            row.setTag(i.id as isize);
            row.setEnabled(i.enabled);
            row.setState(if i.checked {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            // SAFETY: the target outlives the menu's tracking (it is held until this function returns).
            unsafe { row.setTarget(Some(&target)) };
            menu.addItem(&row);
        }
        let y = if host.isFlipped() {
            at.1
        } else {
            host.bounds().size.height - at.1
        };
        PICKED.with(|p| p.set(None));
        menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(at.0, y), Some(host));
        match PICKED.with(Cell::take).and_then(|t| i32::try_from(t).ok()) {
            Some(id) => Reply::Command(id),
            None => Reply::No,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn the_page_sits_in_points_from_the_content_views_top_left() {
        let b = PhysRect {
            x: 200,
            y: 100,
            w: 800,
            h: 600,
        };
        assert_eq!(page_origin(b, 2.0), (100.0, 50.0));
        assert_eq!(
            page_origin(b, 0.0),
            (200.0, 100.0),
            "a scale of nothing is 1"
        );
    }

    #[test]
    fn a_dialog_is_headed_by_the_host_that_asks_as_chromium_does() {
        assert_eq!(
            script_heading("https://example.com/a?b"),
            "example.com says"
        );
        assert_eq!(script_heading(""), "This page says");
        assert_eq!(script_heading("about:blank"), "This page says");
    }

    #[test]
    fn the_leave_question_says_leave_or_reload() {
        assert_eq!(leave_words(false).2, "Leave");
        assert_eq!(leave_words(true).2, "Reload");
    }

    #[test]
    fn without_a_window_every_ask_is_answered_no_at_once() {
        let got = Arc::new(Mutex::new(None));
        let g = Arc::clone(&got);
        let mut a = Sheets::for_view(0);
        a.ask(
            AskKey {
                slot: Slot::panel("t"),
                generation: 1,
                id: 1,
            },
            Ask::Leave { reload: false },
            (0.0, 0.0),
            Box::new(move |r| *g.lock().unwrap() = Some(r)),
        );
        assert_eq!(*got.lock().unwrap(), Some(Reply::No));
    }
}
