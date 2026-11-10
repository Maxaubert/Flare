//! The CEF helper's `main` (GPU, renderer, plugin, utility and alerts processes), shared by every helper executable
//! a bundle runs (`flare-cef-helper`, and the web host tests' own): enters the sandbox, loads the framework from the
//! helper's place in the bundle (`Contents/Frameworks/<App> Helper (X).app/Contents/MacOS/`), fixes the API version
//! and runs the child process CEF asked for.

use cef::*;

/// The helper's exit code.
pub fn main() -> i32 {
    let args = args::Args::new();
    // The sandbox first, before the framework loads (CEF's order).
    let mut sandbox = sandbox::Sandbox::new();
    sandbox.initialize(args.as_main_args());
    let Ok(exe) = std::env::current_exe() else {
        return 70;
    };
    let loader = library_loader::LibraryLoader::new(&exe, true);
    if !loader.load() {
        return 70;
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let code = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    drop(sandbox);
    drop(loader);
    code.max(0)
}
