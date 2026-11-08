//! The CEF helper of the web host tests' bundle (`flare_cef::macos::helper`). macOS only.

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(flare_cef::macos::helper::main());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("flare-web-cef-helper is the macOS CEF helper; Windows uses WebView2");
    std::process::exit(1);
}
