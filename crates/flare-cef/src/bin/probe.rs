//! The engine tests' browser process (see `flare_cef::macos::probe`). macOS only, and only inside the dev bundle.

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(flare_cef::macos::probe::main());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("flare-cef-probe is macOS only; Windows uses WebView2");
    std::process::exit(1);
}
