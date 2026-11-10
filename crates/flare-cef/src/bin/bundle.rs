//! Builds a dev bundle around a browser process under the target dir:
//! `flare-cef-bundle <name> <identifier> <main exe> <helper exe> <out dir>`, for example
//! `cargo run -p flare-cef --bin flare-cef-bundle -- "Flare CEF Probe" dev.flare.cef-probe
//! target/debug/flare-cef-probe target/debug/flare-cef-helper target/flare-cef-dev`.
//! The framework is the CEF build cef-dll-sys was built against. macOS only.

#[cfg(target_os = "macos")]
fn main() {
    use std::path::Path;
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 5 {
        eprintln!("usage: flare-cef-bundle <name> <identifier> <main exe> <helper exe> <out dir>");
        std::process::exit(2);
    }
    let spec = flare_cef::macos::bundle::BundleSpec {
        name: &a[0],
        identifier: &a[1],
        main_exe: Path::new(&a[2]),
        helper_exe: Path::new(&a[3]),
        cef_dir: Path::new(flare_cef::macos::CEF_DIR),
        out_dir: Path::new(&a[4]),
        background_only: true,
        version: env!("CARGO_PKG_VERSION"),
        signing: flare_cef::macos::sign::Signing::for_tests(),
        extras: Default::default(),
    };
    match flare_cef::macos::bundle::build(&spec) {
        Ok(app) => println!("{}", app.display()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("flare-cef-bundle is macOS only");
    std::process::exit(1);
}
