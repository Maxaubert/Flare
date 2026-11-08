//! Hands the CEF folder cef-dll-sys resolved (its `links` metadata) to the crate, so the dev bundle builder
//! copies the framework of the exact build the bindings were made for. Nothing off macOS.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let dir = std::env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR")
        .expect("cef-dll-sys names the CEF folder it built against");
    println!("cargo::rustc-env=FLARE_CEF_DIR={dir}");
}
