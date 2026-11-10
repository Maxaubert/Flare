//! The CEF helper (GPU, renderer, plugin, utility and alerts processes): see `flare_cef::macos::helper`. macOS only.

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(flare_cef::macos::helper::main());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("flare-cef-helper is the macOS CEF helper; Windows uses WebView2");
    std::process::exit(1);
}
