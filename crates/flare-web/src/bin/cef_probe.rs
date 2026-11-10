//! The web host tests' browser process (see `flare_web::cef_probe`). macOS only, and only inside the dev bundle.

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(flare_web::cef_probe::main());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("flare-web-cef-probe is macOS only; Windows uses WebView2");
    std::process::exit(1);
}
