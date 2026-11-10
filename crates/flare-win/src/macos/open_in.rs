//! "Open in Flare" on macOS (port phase 6, step 3): the Finder service and the app's open-documents route, the Mac
//! counterparts of the Explorer verb (`verb.rs`) and of a second launch's hand-off (`single_instance.rs`).
//!
//! - **The service.** `Info.plist` declares it ([`plist_keys`], merged into the app's keys by the bundler) for
//!   folders and files (`NSSendFileTypes` `public.folder` and `public.item`, read as `public.file-url` or the older
//!   `NSFilenamesPboardType`). When the person picks "Open in Flare" in Finder's Services menu, AppKit calls
//!   `openInFlare:userData:error:` on the object [`install`] made the app's services provider.
//! - **Open documents.** `open -a Flare <folder>`, an AppleScript `open` or another app's "open with" send the app an
//!   open-documents event, which AppKit hands to the app delegate's `application:openURLs:`. winit owns the delegate
//!   (`WinitApplicationDelegate`) and has no such method, so [`install`] adds one to its class, never replacing a
//!   method the class already has. `NSTreatUnknownArgumentsAsOpen` is registered NO (in memory, nothing is written),
//!   so AppKit does not also treat the command line's paths as documents: `main` opens those (rule 32).
//!
//! Both end in the same request a second launch makes ([`handoff_for`]): a [`Handoff`] whose arguments are the
//! paths, which the controller turns into new tabs through `folders_from_argv` (a folder opens as itself, a file as
//! the folder that holds it, every path checked on the hand-off worker). Nothing here registers with the system:
//! `pbs` reads the service from the installed app's `Info.plist`, and the tests call the handlers as functions.
//!
//! The objects are main-thread only (AppKit calls them there); the path functions are pure.

use crate::argv::ARGV_MAX;
use crate::single_instance::Handoff;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSPasteboard};
use objc2_foundation::{NSArray, NSDictionary, NSObjectProtocol, NSString, NSURL, NSUserDefaults};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// The service's `NSMessage`: AppKit calls `<message>:userData:error:` on the services provider.
pub const SERVICE_MESSAGE: &str = "openInFlare";
/// The item's title in the Services menu (the Explorer verb's label, decision O4).
pub const SERVICE_LABEL: &str = "Open in Flare";
/// The pasteboard types the service reads: file URLs, and the older list of file names.
pub const FILE_URL_TYPE: &str = "public.file-url";
pub const FILENAMES_TYPE: &str = "NSFilenamesPboardType";
/// The kinds of item the service is offered for in Finder: folders, and any file (it opens the folder holding it).
pub const SEND_FILE_TYPES: [&str; 2] = ["public.folder", "public.item"];
/// The user default that stops AppKit treating command-line paths as documents to open.
pub const TREAT_ARGS_AS_OPEN: &str = "NSTreatUnknownArgumentsAsOpen";
/// The first argument of the request, which `paths_from_argv` skips as a launch's own program.
const PROGRAM: &str = "Flare";

/// What the service and the open route call, on the main thread, with the request.
pub type OpenHandler = Rc<dyn Fn(Handoff)>;

thread_local! {
    static HANDLER: RefCell<Option<OpenHandler>> = const { RefCell::new(None) };
    static PROVIDER: RefCell<Option<Retained<ServiceProvider>>> = const { RefCell::new(None) };
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn strings(v: &[&str]) -> String {
    let items: String = v
        .iter()
        .map(|s| format!("<string>{}</string>", xml(s)))
        .collect();
    format!("<array>{items}</array>")
}

/// The `Info.plist` keys of the service, as the bundler's (key, XML value) pairs: `NSServices` with one entry for
/// the app named `app_name` (its `NSPortName`, the name AppKit listens under). `NSRequiredContext` makes it on by
/// default. No document or URL types (a folder reaches Flare through this service; phase 8's `app_keys`).
pub fn plist_keys(app_name: &str) -> Vec<(&'static str, String)> {
    let entry = format!(
        "<dict>\
         <key>NSMenuItem</key><dict><key>default</key><string>{label}</string></dict>\
         <key>NSMessage</key><string>{message}</string>\
         <key>NSPortName</key><string>{port}</string>\
         <key>NSRequiredContext</key><dict/>\
         <key>NSSendFileTypes</key>{file_types}\
         <key>NSSendTypes</key>{send_types}\
         </dict>",
        label = xml(SERVICE_LABEL),
        message = xml(SERVICE_MESSAGE),
        port = xml(app_name),
        file_types = strings(&SEND_FILE_TYPES),
        send_types = strings(&[FILENAMES_TYPE, FILE_URL_TYPE]),
    );
    vec![("NSServices", format!("<array>{entry}</array>"))]
}

/// The paths of what the pasteboard holds: its file URLs (`public.file-url` items, as strings), or when it has none
/// the older `NSFilenamesPboardType` list. Only absolute paths; a URL that is not a file URL is left out.
pub fn paths_of(file_urls: &[String], filenames: &[String]) -> Vec<String> {
    let from_urls: Vec<String> = file_urls
        .iter()
        .filter_map(|u| crate::pasteboard::file_url_path(u))
        .collect();
    if !from_urls.is_empty() {
        return from_urls;
    }
    filenames
        .iter()
        .filter(|p| p.starts_with('/'))
        .cloned()
        .collect()
}

/// The tab request for `paths`: a hand-off as a second launch would send it, its arguments the absolute paths in
/// order, each once, at most `ARGV_MAX`. None when nothing is left to open.
pub fn handoff_for(paths: &[String]) -> Option<Handoff> {
    let mut seen = HashSet::new();
    let mut args = vec![PROGRAM.to_string()];
    for p in paths {
        if args.len() > ARGV_MAX {
            break;
        }
        if p.starts_with('/') && seen.insert(p.as_str()) {
            args.push(p.clone());
        }
    }
    (args.len() > 1).then(|| Handoff {
        cwd: "/".to_string(),
        args,
    })
}

/// Pasteboard contents to the tab request: [`paths_of`] then [`handoff_for`].
pub fn handoff_from_pasteboard(file_urls: &[String], filenames: &[String]) -> Option<Handoff> {
    handoff_for(&paths_of(file_urls, filenames))
}

/// What a pasteboard holds, as [`handoff_from_pasteboard`] reads it: the items' file URLs and the file name list.
pub fn read_pasteboard(pb: &NSPasteboard) -> (Vec<String>, Vec<String>) {
    autoreleasepool(|_| {
        let urls = pb
            .pasteboardItems()
            .map(|items| {
                let t = NSString::from_str(FILE_URL_TYPE);
                items
                    .iter()
                    .filter_map(|i| i.stringForType(&t))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let names = pb
            .propertyListForType(&NSString::from_str(FILENAMES_TYPE))
            .and_then(|v| v.downcast::<NSArray>().ok())
            .map(|a| {
                a.iter()
                    .filter_map(|o| o.downcast::<NSString>().ok())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        (urls, names)
    })
}

fn deliver(h: Handoff) {
    // Cloned out first: the handler may install another one.
    let handler = HANDLER.with(|c| c.borrow().clone());
    match handler {
        Some(f) => f(h),
        None => log::warn!("an Open in Flare request came before the window was ready; dropped"),
    }
}

/// The service's body: what `pb` holds to a request, delivered. Err with the sentence the Services menu shows.
pub fn serve_pasteboard(pb: &NSPasteboard) -> Result<(), String> {
    let (urls, names) = read_pasteboard(pb);
    match handoff_from_pasteboard(&urls, &names) {
        Some(h) => {
            deliver(h);
            Ok(())
        }
        None => Err("Flare found no folder or file to open.".to_string()),
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `ServiceProvider` has no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "FlareServiceProvider"]
    pub struct ServiceProvider;

    impl ServiceProvider {
        /// `openInFlare:userData:error:`, the selector of [`SERVICE_MESSAGE`].
        #[unsafe(method(openInFlare:userData:error:))]
        fn open_in_flare(
            &self,
            pb: &NSPasteboard,
            _user_data: Option<&NSString>,
            error: *mut *mut NSString,
        ) {
            if let Err(e) = serve_pasteboard(pb)
                && !error.is_null()
            {
                // SAFETY: AppKit passes a valid out pointer (checked for null); the string is autoreleased, as
                // the service protocol expects.
                unsafe { *error = Retained::autorelease_ptr(NSString::from_str(&e)) };
            }
        }
    }

    unsafe impl NSObjectProtocol for ServiceProvider {}
);

impl ServiceProvider {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's plain init on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// The selector the service calls.
pub fn service_selector() -> Sel {
    sel!(openInFlare:userData:error:)
}

/// `application:openURLs:` as added to the delegate's class: each file URL's path, as one request.
unsafe extern "C-unwind" fn open_urls(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    urls: *mut NSArray<NSURL>,
) {
    if urls.is_null() {
        return;
    }
    // SAFETY: AppKit passes the array of URLs it was asked to open, alive for the call.
    let urls = unsafe { &*urls };
    let paths: Vec<String> = autoreleasepool(|_| {
        urls.iter()
            .filter(|u| u.isFileURL())
            .filter_map(|u| u.path().map(|p| p.to_string()))
            .collect()
    });
    if let Some(h) = handoff_for(&paths) {
        deliver(h);
    }
}

/// How [`add_open_urls`] left a class.
#[derive(Debug, PartialEq, Eq)]
pub enum Added {
    /// The method is Flare's now.
    Added,
    /// The class answered it already; left as it was.
    HadOne,
    /// The runtime refused it.
    Refused,
}

/// Adds `application:openURLs:` to `cls` (the app delegate's class) unless it, or a class above it, has one.
pub fn add_open_urls(cls: &AnyClass) -> Added {
    let s = sel!(application:openURLs:);
    if cls.instance_method(s).is_some() {
        return Added::HadOne;
    }
    type OpenUrls =
        unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>);
    // SAFETY: an IMP is any C function pointer called with the method's own arguments; the type string below says
    // which (void return, self, _cmd, two objects), and the function takes exactly those.
    let imp: Imp = unsafe { std::mem::transmute::<OpenUrls, Imp>(open_urls) };
    let ptr: *const AnyClass = cls;
    // SAFETY: a registered class, on the main thread, before AppKit sends the method.
    let ok = unsafe { objc2::ffi::class_addMethod(ptr.cast_mut(), s, imp, c"v@:@@".as_ptr()) };
    if ok.as_bool() {
        Added::Added
    } else {
        Added::Refused
    }
}

/// Registers `NSTreatUnknownArgumentsAsOpen` NO in the registration domain (memory only, never written to disk).
pub fn keep_arguments_out_of_open() {
    let key = NSString::from_str(TREAT_ARGS_AS_OPEN);
    let no = NSString::from_str("NO");
    let no: &AnyObject = &no;
    let dict = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[no]);
    // SAFETY: a dictionary of property-list values (a string), as registerDefaults: takes.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&dict) };
}

/// Sets the handler both routes call. The service and the delegate method find it on the main thread.
pub fn set_handler(handler: OpenHandler) {
    HANDLER.with(|c| *c.borrow_mut() = Some(handler));
}

/// Installs both routes on the shared application: the handler, the services provider, the delegate's
/// `application:openURLs:`, and the command line kept out of open documents. Runs on the main thread after winit
/// made the application and its delegate and before the event loop runs (the first launch's open-documents event
/// comes while AppKit finishes launching). False off the main thread; a delegate that cannot take the method is
/// logged and the service still works.
pub fn install(handler: OpenHandler) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    set_handler(handler);
    keep_arguments_out_of_open();
    let app = NSApplication::sharedApplication(mtm);
    let provider = ServiceProvider::new(mtm);
    // SAFETY: the provider answers the service's selector (above); it is kept alive in PROVIDER for the run.
    unsafe { app.setServicesProvider(Some(&provider)) };
    PROVIDER.with(|p| *p.borrow_mut() = Some(provider));
    match app.delegate() {
        Some(d) => {
            let obj: &AnyObject = (*d).as_ref();
            let added = add_open_urls(obj.class());
            if added == Added::Refused {
                log::warn!(
                    "the app delegate refused application:openURLs:; open -a Flare will not open tabs"
                );
            }
        }
        None => log::warn!("no app delegate yet; open -a Flare will not open tabs"),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::ClassType;
    use std::process::Command;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn file_urls_become_paths_and_other_urls_are_left_out() {
        let urls = v(&[
            "file:///Users/me/My%20Project/",
            "https://example.com/x",
            "file:///Users/me/notes.txt",
        ]);
        assert_eq!(
            paths_of(&urls, &v(&["/ignored/because/urls/came"])),
            v(&["/Users/me/My Project", "/Users/me/notes.txt"])
        );
    }

    #[test]
    fn the_older_file_name_list_is_read_when_there_are_no_file_urls() {
        assert_eq!(
            paths_of(&[], &v(&["/Users/me/src", "relative/no", ""])),
            v(&["/Users/me/src"])
        );
        assert_eq!(paths_of(&v(&["mailto:x@y.z"]), &[]), Vec::<String>::new());
    }

    #[test]
    fn the_request_is_a_second_launchs_hand_off_with_each_path_once() {
        let h = handoff_for(&v(&["/a", "/b", "/a", "rel"])).expect("a request");
        assert_eq!(h.cwd, "/");
        assert_eq!(h.args, v(&[PROGRAM, "/a", "/b"]));
        assert_eq!(handoff_for(&v(&["rel", ""])), None);
        assert_eq!(handoff_for(&[]), None);
        let many: Vec<String> = (0..100).map(|i| format!("/p{i}")).collect();
        assert_eq!(handoff_for(&many).expect("some").args.len(), ARGV_MAX + 1);
    }

    #[test]
    fn a_folder_opens_as_itself_and_a_file_as_the_folder_that_holds_it() {
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path().canonicalize().expect("real path");
        let sub = root.join("with space");
        std::fs::create_dir(&sub).expect("sub");
        let file = sub.join("a.txt");
        std::fs::write(&file, "x").expect("file");
        let url = |p: &std::path::Path| {
            NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()))
                .absoluteString()
                .expect("url")
                .to_string()
        };
        let h = handoff_from_pasteboard(&[url(&root), url(&file)], &[]).expect("a request");
        // The controller's own path from a hand-off to tabs.
        let folders = crate::argv::folders_from_argv(&h.args, &[], std::path::Path::new(&h.cwd));
        assert_eq!(
            folders,
            vec![
                root.to_string_lossy().into_owned(),
                sub.to_string_lossy().into_owned()
            ]
        );
    }

    #[test]
    fn the_service_selector_is_the_plists_message() {
        assert_eq!(
            service_selector().name().to_str().expect("utf8"),
            format!("{SERVICE_MESSAGE}:userData:error:")
        );
        let cls = ServiceProvider::class();
        assert!(cls.instance_method(service_selector()).is_some());
    }

    #[test]
    fn the_service_plist_is_valid_and_offers_folders_and_files() {
        let body: String = plist_keys("Flare & Co")
            .iter()
            .map(|(k, val)| format!("  <key>{k}</key>\n  {val}\n"))
            .collect();
        let doc = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{body}</dict>\n\
             </plist>\n"
        );
        let dir = tempfile::tempdir().expect("dir");
        let f = dir.path().join("Info.plist");
        std::fs::write(&f, &doc).expect("write");
        let lint = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&f)
            .output()
            .expect("plutil runs");
        assert!(
            lint.status.success(),
            "{}",
            String::from_utf8_lossy(&lint.stdout)
        );
        let get = |path: &str| {
            let o = Command::new("/usr/bin/plutil")
                .args(["-extract", path, "raw", "-o", "-"])
                .arg(&f)
                .output()
                .expect("plutil runs");
            assert!(
                o.status.success(),
                "{path}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        assert_eq!(get("NSServices.0.NSMessage"), SERVICE_MESSAGE);
        assert_eq!(get("NSServices.0.NSMenuItem.default"), SERVICE_LABEL);
        assert_eq!(get("NSServices.0.NSPortName"), "Flare & Co");
        assert_eq!(get("NSServices.0.NSSendFileTypes.0"), "public.folder");
        assert_eq!(get("NSServices.0.NSSendFileTypes.1"), "public.item");
        assert_eq!(get("NSServices.0.NSSendTypes.0"), FILENAMES_TYPE);
        assert_eq!(get("NSServices.0.NSSendTypes.1"), FILE_URL_TYPE);
        // On by default, and no document type that would make Flare a folder's or a file's handler.
        assert!(doc.contains("<key>NSRequiredContext</key><dict/>"));
        assert!(!doc.contains("CFBundleDocumentTypes"));
    }
}
