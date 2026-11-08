//! M9 Task 29: the real `VelopackFeed` over real `vpk` output, read through a file source.
//!
//! Run by hand and in the update e2e job:
//! `FLARE_PACKED_DIR=<Releases folder with 0.0.1 and 0.0.2> cargo test -p flare-update --test packed_feed -- --ignored`.
//! Nothing here installs an app: the "installed" layout is a temp folder with a dummy `Update.exe`
//! and a `sq.version` that says 0.0.1. `apply` is never called.
//! Windows-only: the installed layout (`Update.exe`, `create_config_from_root_dir`) is Velopack's Windows one.
#![cfg(windows)]

use std::path::{Path, PathBuf};

use flare_update::feed::{Feed, FeedError};
use flare_update::gate::CancelToken;
use flare_update::velopack_feed::{VelopackFeed, package_is_ours};
use velopack::locator::create_config_from_root_dir;
use velopack::sources::FileSource;

/// The nuspec `vpk` writes as `current\sq.version`, for the layout's own version.
fn sq_version(version: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<package xmlns=\"http://schemas.microsoft.com/packaging/2010/07/nuspec.xsd\">\n<metadata>\n\
<id>Flare</id>\n<title>Flare</title>\n<description>Flare</description>\n<authors>Max</authors>\n\
<version>{version}</version>\n<channel>win</channel>\n<mainExe>Flare.exe</mainExe>\n<os>win</os>\n\
<rid>win-x64</rid>\n<machineArchitecture>x64</machineArchitecture>\n</metadata>\n</package>\n"
    )
}

fn layout(root: &Path, version: &str) {
    std::fs::create_dir_all(root.join("current")).unwrap();
    std::fs::write(root.join("Update.exe"), b"stub").unwrap();
    std::fs::write(root.join("current").join("sq.version"), sq_version(version)).unwrap();
}

fn feed_over(releases: &Path, root: &Path) -> VelopackFeed {
    let cfg = create_config_from_root_dir(root);
    VelopackFeed::with_source(Box::new(FileSource::new(releases)), Some(cfg)).expect("feed")
}

fn packed_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("FLARE_PACKED_DIR").expect("FLARE_PACKED_DIR names the vpk Releases folder"),
    )
}

#[test]
#[ignore = "needs real packages: set FLARE_PACKED_DIR to a Releases folder holding 0.0.1 and 0.0.2"]
fn the_feed_over_real_packages_checks_downloads_discards_and_refuses_the_unoffered() {
    let releases = packed_dir();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Flare");
    layout(&root, "0.0.1");
    let feed = feed_over(&releases, &root);

    // check() finds 0.0.2 from a layout that says it is 0.0.1, and the notes equal the file.
    let found = feed.check().expect("check").expect("an offer");
    assert_eq!(found.version, "0.0.2");
    let json = std::fs::read_to_string(releases.join("releases.win.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
    let full = doc["Assets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["Version"] == "0.0.2" && a["Type"] == "Full")
        .expect("a 0.0.2 Full asset");
    assert_eq!(found.notes, full["NotesMarkdown"].as_str().unwrap());
    assert!(package_is_ours(full["PackageId"].as_str().unwrap()));

    // A version that was not offered is refused before anything is written.
    let cancel = CancelToken::default();
    let err = feed.download("0.0.3", &mut |_| {}, &cancel).unwrap_err();
    assert!(
        matches!(err, FeedError::Failed(ref m) if m == "not offered"),
        "{err:?}"
    );

    // download reports progress that ends at 100 (or fewer steps) and lands in <root>\packages.
    let mut seen: Vec<u8> = Vec::new();
    feed.download("0.0.2", &mut |p| seen.push(p), &cancel)
        .expect("download");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "monotonic: {seen:?}");
    if let Some(last) = seen.last() {
        assert_eq!(*last, 100, "{seen:?}");
    }
    let packages = root.join("packages");
    let landed: Vec<String> = std::fs::read_dir(&packages)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        landed.iter().any(|n| n == "Flare-0.0.2-full.nupkg"),
        "packages folder holds {landed:?}"
    );

    // discard removes what the download wrote, and is idempotent.
    feed.discard();
    feed.discard();
    assert!(!packages.join("Flare-0.0.2-full.nupkg").exists());
    // `apply` is never called here: it would start the updater.
}
