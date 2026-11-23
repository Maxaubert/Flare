//! Android setup's pure parts (Android plan PR A2b, rule 1361): Google's package lists read back, the packages Flare
//! needs picked from them, the licences, the `package.xml` each package gets, and the disk space a setup takes. The
//! emu host does the network, the checksums and the files (`flare-emu-host/src/setup.rs`).
//!
//! What Flare downloads, only after the person accepts Google's licence in Flare, always from Google's own server
//! (`https://dl.google.com/android/repository/`), never from Flare's:
//! - `emulator`: the newest **stable** one for this computer (the list also has preview ones on other channels).
//! - `platform-tools` (adb): the newest stable one.
//! - one system image: the newest stable `google_apis` image of a plain API level (`android-37.0`, not a beta, a
//!   canary, an extension level or a 16 KB page image) for this CPU (`x86_64` on Windows, `arm64-v8a` on Apple
//!   silicon) that the chosen emulator can run (the image's own `min-revision` for `emulator`).
//!
//! The lists are parsed with roxmltree's default options, which refuse a DOCTYPE: no entity is ever declared, so none
//! is expanded (a list with one is refused). Each archive URL must be a bare file name; Flare joins it to the list's
//! own folder on `dl.google.com`, so a list can never send Flare anywhere else.

use std::collections::BTreeMap;

use crate::versions::Version;

/// Google's repository folder. Every download is under it (rule 1361).
pub const REPO: &str = "https://dl.google.com/android/repository/";
/// The list with `emulator` and `platform-tools` (the same one `sdkmanager` reads).
pub const REPO_LIST: &str = "https://dl.google.com/android/repository/repository2-3.xml";
/// The `google_apis` system images and their folder.
pub const IMAGES: &str = "https://dl.google.com/android/repository/sys-img/google_apis/";
pub const IMAGES_LIST: &str =
    "https://dl.google.com/android/repository/sys-img/google_apis/sys-img2-3.xml";
/// A list bigger than this is refused (the real ones were 433 KB and 241 KB on 2026-11-22).
pub const LIST_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// The system image's tag: Google APIs, not the Play Store (spec "Setup": no Google sign-in in the device).
pub const IMAGE_TAG: &str = "google_apis";

/// This computer as the lists name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostCpu {
    /// `windows`, `macosx` or `linux`.
    pub os: &'static str,
    /// `x64` or `aarch64`.
    pub arch: &'static str,
}

impl HostCpu {
    /// This build's OS and CPU, None for one the lists have no emulator for.
    pub fn this() -> Option<HostCpu> {
        let os = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macosx"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            return None;
        };
        let arch = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            return None;
        };
        Some(HostCpu { os, arch })
    }

    /// The system image's ABI for this CPU.
    pub fn abi(&self) -> &'static str {
        if self.arch == "aarch64" {
            "arm64-v8a"
        } else {
            "x86_64"
        }
    }
}

/// One downloadable file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive {
    /// `https://dl.google.com/android/repository/...`, made by [`archive_url`].
    pub url: String,
    /// 40 lower-case hex digits.
    pub sha1: String,
    pub size: u64,
}

/// One `remotePackage` of a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// `emulator`, `platform-tools`, `system-images;android-37.0;google_apis;x86_64`.
    pub path: String,
    /// `37.3.10` (the revision's major, minor and micro that are given).
    pub version: String,
    /// The channel's name: `stable`, `beta`, `dev`, `canary` (empty when the list does not say).
    pub channel: String,
    /// `uses-license`: the licence's id.
    pub licence: Option<String>,
    pub display_name: String,
    /// The emulator version this package needs (an image's `dependency path="emulator"`).
    pub needs_emulator: Option<String>,
    /// Its archive for the host given to [`parse_list`], None when it has none.
    pub archive: Option<Archive>,
    /// The package's description as Google wrote it (`type-details`, `revision`, `display-name`, `uses-license`,
    /// `dependencies`), copied into its `package.xml`.
    pub meta_xml: String,
}

impl Remote {
    pub fn stable(&self) -> bool {
        self.channel == "stable"
    }

    /// The folder the package goes in, under the SDK root: its path with `;` as `/`.
    pub fn dest(&self) -> String {
        self.path.replace(';', "/")
    }
}

/// One package list read back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub packages: Vec<Remote>,
    /// Licence texts by id, as written in the list.
    pub licences: BTreeMap<String, String>,
    /// The list's namespace declarations (prefix, URI), so the copied descriptions keep their meaning.
    pub namespaces: Vec<(String, String)>,
}

/// The full URL of an archive named `name` in a list at `base` (a folder on dl.google.com, ending in `/`). Only a bare
/// file name is taken: no folder, no scheme, no `..`.
pub fn archive_url(base: &str, name: &str) -> Option<String> {
    let name = name.trim();
    let ok = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    let base_ok = base.starts_with(REPO) && base.ends_with('/') && !base.contains("..");
    (ok && base_ok).then(|| format!("{base}{name}"))
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn child_text(n: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    Some(child(n, name)?.text()?.trim().to_string())
}

/// `<major>`, `<minor>`, `<micro>` as `37.3.10` (the parts given).
fn revision_of(r: roxmltree::Node<'_, '_>) -> Option<String> {
    let mut parts = Vec::new();
    for k in ["major", "minor", "micro"] {
        match child_text(r, k) {
            Some(v) if v.bytes().all(|b| b.is_ascii_digit()) && !v.is_empty() => parts.push(v),
            Some(_) => return None,
            None => break,
        }
    }
    (!parts.is_empty()).then(|| parts.join("."))
}

fn archive_for(
    arcs: roxmltree::Node<'_, '_>,
    host: HostCpu,
    base: &str,
) -> Result<Option<Archive>, String> {
    for a in arcs.children().filter(|c| c.has_tag_name("archive")) {
        if child_text(a, "host-os").is_some_and(|o| o != host.os)
            || child_text(a, "host-arch").is_some_and(|o| o != host.arch)
        {
            continue;
        }
        let Some(c) = child(a, "complete") else {
            continue;
        };
        let name = child_text(c, "url").unwrap_or_default();
        let url = archive_url(base, &name)
            .ok_or_else(|| format!("an archive name Flare does not take: {name:?}"))?;
        let sum = child(c, "checksum").ok_or("an archive without a checksum")?;
        if sum.attribute("type").is_some_and(|t| t != "sha1") {
            return Err("an archive whose checksum is not SHA-1".into());
        }
        let sha1 = sum.text().unwrap_or_default().trim().to_ascii_lowercase();
        if sha1.len() != 40 || !sha1.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("{name}: a checksum that is not SHA-1"));
        }
        let size = child_text(c, "size")
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|s| *s > 0)
            .ok_or_else(|| format!("{name}: no size"))?;
        return Ok(Some(Archive { url, sha1, size }));
    }
    Ok(None)
}

/// A package list (`repository2-3.xml` or `sys-img2-3.xml`) read for `host`. `base` is the list's folder (its archives
/// are relative to it). Err for a list that is not XML, has a DOCTYPE, or names an archive outside its folder.
pub fn parse_list(xml: &str, base: &str, host: HostCpu) -> Result<Listing, String> {
    let doc = roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions::default())
        .map_err(|e| format!("the package list does not read: {e}"))?;
    let root = doc.root_element();
    let mut out = Listing {
        namespaces: root
            .namespaces()
            .filter_map(|n| Some((n.name()?.to_string(), n.uri().to_string())))
            .collect(),
        ..Listing::default()
    };
    let mut channels: BTreeMap<String, String> = BTreeMap::new();
    for n in root.children().filter(roxmltree::Node::is_element) {
        match n.tag_name().name() {
            "license" => {
                if let Some(id) = n.attribute("id") {
                    out.licences
                        .insert(id.to_string(), n.text().unwrap_or_default().to_string());
                }
            }
            "channel" => {
                if let Some(id) = n.attribute("id") {
                    channels.insert(
                        id.to_string(),
                        n.text().unwrap_or_default().trim().to_string(),
                    );
                }
            }
            _ => {}
        }
    }
    for p in root.children().filter(|c| c.has_tag_name("remotePackage")) {
        let Some(path) = p.attribute("path") else {
            continue;
        };
        let Some(version) = child(p, "revision").and_then(revision_of) else {
            continue;
        };
        let channel = child(p, "channelRef")
            .and_then(|c| c.attribute("ref"))
            .and_then(|r| channels.get(r).cloned())
            .unwrap_or_default();
        let needs_emulator = child(p, "dependencies").and_then(|d| {
            d.children()
                .filter(|c| c.has_tag_name("dependency"))
                .find(|c| c.attribute("path") == Some("emulator"))
                .and_then(|c| child(c, "min-revision"))
                .and_then(revision_of)
        });
        let archive = match child(p, "archives") {
            Some(a) => archive_for(a, host, base)?,
            None => None,
        };
        let meta_xml = p
            .children()
            .filter(|c| {
                c.is_element()
                    && [
                        "type-details",
                        "revision",
                        "display-name",
                        "uses-license",
                        "dependencies",
                    ]
                    .contains(&c.tag_name().name())
            })
            .map(|c| format!("    {}\n", &xml[c.range()]))
            .collect();
        out.packages.push(Remote {
            path: path.to_string(),
            version,
            channel,
            licence: child(p, "uses-license")
                .and_then(|l| l.attribute("ref"))
                .map(str::to_string),
            display_name: child_text(p, "display-name").unwrap_or_default(),
            needs_emulator,
            archive,
            meta_xml,
        });
    }
    Ok(out)
}

fn newer(a: &Remote, b: &Remote) -> bool {
    Version::parse(&a.version) > Version::parse(&b.version)
}

/// The newest stable package at `path` that has an archive for this host.
pub fn newest_stable<'a>(l: &'a Listing, path: &str) -> Option<&'a Remote> {
    l.packages
        .iter()
        .filter(|p| p.path == path && p.stable() && p.archive.is_some())
        .fold(None, |best: Option<&Remote>, p| match best {
            Some(b) if !newer(p, b) => Some(b),
            _ => Some(p),
        })
}

/// The API level of a plain image path (`system-images;android-37.0;google_apis;x86_64` gives 37.0); None for a
/// beta, a canary, an extension level or another tag or ABI.
pub fn image_api(path: &str, abi: &str) -> Option<Version> {
    let parts: Vec<&str> = path.split(';').collect();
    let [kind, platform, tag, a] = parts.as_slice() else {
        return None;
    };
    if *kind != "system-images" || *tag != IMAGE_TAG || *a != abi {
        return None;
    }
    Version::parse(platform.strip_prefix("android-")?)
}

/// The newest stable plain `google_apis` image for `abi` that an emulator of `emulator` can run.
pub fn pick_image<'a>(l: &'a Listing, abi: &str, emulator: &str) -> Option<&'a Remote> {
    let emu = Version::parse(emulator);
    l.packages
        .iter()
        .filter(|p| p.stable() && p.archive.is_some())
        .filter(|p| {
            p.needs_emulator
                .as_deref()
                .is_none_or(|n| Version::parse(n) <= emu)
        })
        .filter_map(|p| Some((image_api(&p.path, abi)?, p)))
        .fold(
            None,
            |best: Option<(Version, &Remote)>, (api, p)| match best {
                Some((ba, bp))
                    if (ba.clone(), Version::parse(&bp.version))
                        >= (api.clone(), Version::parse(&p.version)) =>
                {
                    Some((ba, bp))
                }
                _ => Some((api, p)),
            },
        )
        .map(|(_, p)| p)
}

/// The licence text the way the SDK reads it before hashing it (the SDK's `TrimStringAdapter`): trimmed, each space or
/// tab after any whitespace dropped, a single line break made a space, runs of spaces made one. Its SHA-1 is what
/// `licenses/<id>` holds (checked 2026-11-22: this gives `24333f8a63b6825ea9c5514f83c2829b004d1fee`, the hash in an
/// SDK `sdkmanager` set up, for today's `android-sdk-license`; the raw text does not).
pub fn licence_text(raw: &str) -> String {
    let java_ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r');
    let t = raw.trim_matches(|c: char| c <= ' ');
    // A space or tab right after any whitespace goes.
    let mut a = String::with_capacity(t.len());
    let mut prev_ws = false;
    for c in t.chars() {
        if !(prev_ws && (c == ' ' || c == '\t')) {
            a.push(c);
        }
        prev_ws = java_ws(c);
    }
    // A line break with no line break on either side becomes a space.
    let chars: Vec<char> = a.chars().collect();
    let mut b = String::with_capacity(a.len());
    for (i, c) in chars.iter().enumerate() {
        let lone = *c == '\n'
            && (i == 0 || chars[i - 1] != '\n')
            && chars.get(i + 1).is_none_or(|n| *n != '\n');
        b.push(if lone { ' ' } else { *c });
    }
    // Runs of spaces become one.
    let mut out = String::with_capacity(b.len());
    for c in b.chars() {
        if !(c == ' ' && out.ends_with(' ')) {
            out.push(c);
        }
    }
    out
}

/// What Flare's own SDK folder already holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Have {
    /// The emulator's version, None without one.
    pub emulator: Option<String>,
    pub platform_tools: bool,
    /// Flare's own AVD already exists (its image is there or was chosen before): no image is downloaded again.
    pub avd: bool,
}

/// One package to fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Get {
    pub remote: Remote,
    pub archive: Archive,
    /// The list's namespaces (for its `package.xml`).
    pub namespaces: Vec<(String, String)>,
}

/// What a setup (or an update) downloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub get: Vec<Get>,
    /// The licences the packages use: id and the text as the list has it.
    pub licences: Vec<(String, String)>,
    /// The image Flare's AVD uses (None when the AVD already exists).
    pub image: Option<Remote>,
    /// The emulator in Flare's folder is older than the newest stable one (or under the floor): an update.
    pub update: bool,
    /// The newest stable emulator Google lists.
    pub newest_emulator: String,
}

impl Plan {
    pub fn download_bytes(&self) -> u64 {
        self.get.iter().map(|g| g.archive.size).sum()
    }

    /// About what the setup takes on disk once unpacked, with one device: three times the download (S7 on the Mac:
    /// 2.5 GB downloaded, 8.0 GB on disk after the first boot).
    pub fn disk_bytes(&self) -> u64 {
        self.download_bytes().saturating_mul(3)
    }

    /// The free space a setup asks for before it starts: the disk size and 1 GiB to spare.
    pub fn free_bytes_needed(&self) -> u64 {
        self.disk_bytes().saturating_add(1 << 30)
    }

    pub fn is_empty(&self) -> bool {
        self.get.is_empty()
    }
}

/// What Flare's folder needs, from both lists. `floor` is this OS's emulator floor: the newest stable emulator must
/// be at it (else Err: Flare cannot set up yet).
pub fn plan(
    repo: &Listing,
    images: &Listing,
    host: HostCpu,
    have: &Have,
    floor: &str,
) -> Result<Plan, String> {
    let emu = newest_stable(repo, "emulator")
        .ok_or("Google's list has no stable Android emulator for this computer")?;
    if !super::sdk::at_floor(&emu.version, floor) {
        return Err(format!(
            "Google's newest stable Android emulator is {}; Flare needs {floor}",
            emu.version
        ));
    }
    let tools = newest_stable(repo, "platform-tools")
        .ok_or("Google's list has no stable platform-tools for this computer")?;
    let update = have
        .emulator
        .as_deref()
        .is_some_and(|v| Version::parse(v) < Version::parse(&emu.version));
    let mut picks: Vec<(&Remote, &Listing)> = Vec::new();
    if have.emulator.is_none() || update {
        picks.push((emu, repo));
    }
    if !have.platform_tools {
        picks.push((tools, repo));
    }
    let image = if have.avd {
        None
    } else {
        let img = pick_image(images, host.abi(), &emu.version).ok_or_else(|| {
            format!(
                "Google's list has no stable {IMAGE_TAG} image for {}",
                host.abi()
            )
        })?;
        picks.push((img, images));
        Some(img.clone())
    };
    let mut licences: Vec<(String, String)> = Vec::new();
    let mut get = Vec::new();
    for (r, l) in picks {
        let id = r
            .licence
            .clone()
            .ok_or_else(|| format!("{} names no licence", r.path))?;
        let text = l
            .licences
            .get(&id)
            .ok_or_else(|| format!("{} names a licence the list does not hold: {id}", r.path))?;
        if !licences.iter().any(|(i, _)| *i == id) {
            licences.push((id, text.clone()));
        }
        get.push(Get {
            remote: r.clone(),
            archive: r.archive.clone().ok_or("no archive")?,
            namespaces: l.namespaces.clone(),
        });
    }
    Ok(Plan {
        get,
        licences,
        image,
        update,
        newest_emulator: emu.version.clone(),
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The `package.xml` the SDK keeps in each package's folder (as `sdkmanager` writes it): the licence, then the
/// package as a `localPackage` with Google's own description of it. Studio and `sdkmanager` read it to know what is
/// installed.
pub fn package_xml(g: &Get, licence_text: &str) -> String {
    const COMMON: &str = "http://schemas.android.com/repository/android/common/02";
    let mut decls = String::new();
    let mut prefix = None;
    for (p, uri) in &g.namespaces {
        decls.push_str(&format!(" xmlns:{p}=\"{}\"", escape(uri)));
        if uri == COMMON {
            prefix = Some(p.clone());
        }
    }
    let p = prefix.unwrap_or_else(|| {
        decls.push_str(&format!(" xmlns:common02=\"{COMMON}\""));
        "common02".to_string()
    });
    if !g.namespaces.iter().any(|(p, _)| p == "xsi") {
        decls.push_str(" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"");
    }
    let r = &g.remote;
    let licence = r.licence.as_deref().unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<{p}:repository{decls}>\n  <license id=\"{}\" type=\"text\">{}</license>\n  <localPackage path=\"{}\" obsolete=\"false\">\n{}  </localPackage>\n</{p}:repository>\n",
        escape(licence),
        escape(licence_text),
        escape(&r.path),
        r.meta_xml,
    )
}

/// A `package.xml` read back: its package's path and version.
pub fn read_package_xml(xml: &str) -> Option<(String, String)> {
    let doc =
        roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions::default()).ok()?;
    let p = doc
        .root_element()
        .children()
        .find(|c| c.has_tag_name("localPackage"))?;
    Some((
        p.attribute("path")?.to_string(),
        child(p, "revision").and_then(revision_of)?,
    ))
}

/// `source.properties` for a package whose zip has none (the emulator reads an SDK's versions from it; Flare's own
/// `sdk::read_version` too).
pub fn source_properties(r: &Remote) -> String {
    format!(
        "Pkg.Desc={}\nPkg.Revision={}\nPkg.Path={}\n",
        r.display_name, r.version, r.path
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const REPO_XML: &str = include_str!("fixtures/repository2-3.xml");
    pub(crate) const IMAGES_XML: &str = include_str!("fixtures/sys-img2-3.xml");
    const WIN: HostCpu = HostCpu {
        os: "windows",
        arch: "x64",
    };
    const MAC: HostCpu = HostCpu {
        os: "macosx",
        arch: "aarch64",
    };

    fn lists(host: HostCpu) -> (Listing, Listing) {
        (
            parse_list(REPO_XML, REPO, host).unwrap(),
            parse_list(IMAGES_XML, IMAGES, host).unwrap(),
        )
    }

    /// Rule 1361: from the saved lists (2026-11-22), the newest stable emulator and platform-tools, and the newest plain
    /// stable `google_apis` image for each CPU; beta, canary, extension and 16 KB images are left out.
    #[test]
    fn the_lists_give_the_stable_emulator_tools_and_image_for_each_computer() {
        for (host, emu_zip, abi, img_zip) in [
            (
                WIN,
                "emulator-windows_x64-",
                "x86_64",
                "x86_64-37.0_r08.zip",
            ),
            (
                MAC,
                "emulator-darwin_aarch64-",
                "arm64-v8a",
                "arm64-v8a-37.0_r08.zip",
            ),
        ] {
            let (repo, images) = lists(host);
            let emu = newest_stable(&repo, "emulator").unwrap();
            assert_eq!(emu.version, "37.3.10", "37.4.1 is on the dev channel");
            assert_eq!(emu.licence.as_deref(), Some("android-sdk-license"));
            let a = emu.archive.as_ref().unwrap();
            assert!(a.url.starts_with(&format!("{REPO}{emu_zip}")), "{}", a.url);
            assert_eq!(a.sha1.len(), 40);
            let tools = newest_stable(&repo, "platform-tools").unwrap();
            assert_eq!(tools.version, "37.0.3");
            let img = pick_image(&images, abi, &emu.version).unwrap();
            assert_eq!(
                img.path,
                format!("system-images;android-37.0;google_apis;{abi}")
            );
            assert_eq!(
                img.dest(),
                format!("system-images/android-37.0/google_apis/{abi}")
            );
            assert_eq!(
                img.archive.as_ref().unwrap().url,
                format!("{IMAGES}{img_zip}")
            );
            assert_eq!(img.needs_emulator.as_deref(), Some("37.2.11"));
            // An emulator too old for 37.0 gets the newest image it can run.
            let old = pick_image(&images, abi, "36.1.0").unwrap();
            assert!(old.path.contains("android-36"), "{}", old.path);
        }
        assert_eq!(
            image_api(
                "system-images;android-36-ext19;google_apis;x86_64",
                "x86_64"
            ),
            None
        );
        assert_eq!(
            image_api(
                "system-images;android-37.2;google_apis_ps16k;x86_64",
                "x86_64"
            ),
            None
        );
        assert_eq!(
            image_api("system-images;android-CANARY;google_apis;x86_64", "x86_64"),
            None
        );
        assert_eq!(
            image_api("system-images;android-37.0;google_apis;x86_64", "arm64-v8a"),
            None
        );
        assert_eq!(
            image_api("system-images;android-36.1;google_apis;x86_64", "x86_64"),
            Version::parse("36.1")
        );
    }

    /// The arm64 images use their own licence (`android-sdk-arm-dbt-license`), so a Mac setup shows two.
    #[test]
    fn a_plan_lists_each_licence_its_packages_use_once() {
        let (repo, images) = lists(WIN);
        let p = plan(&repo, &images, WIN, &Have::default(), "37.2.13").unwrap();
        let paths: Vec<&str> = p.get.iter().map(|g| g.remote.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "emulator",
                "platform-tools",
                "system-images;android-37.0;google_apis;x86_64"
            ]
        );
        let ids: Vec<&str> = p.licences.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(ids, ["android-sdk-license"]);
        assert!(!p.update);
        assert_eq!(
            p.download_bytes(),
            p.get.iter().map(|g| g.archive.size).sum::<u64>()
        );
        assert!(p.download_bytes() > 2_000_000_000, "about 2.5 GB");
        assert_eq!(p.disk_bytes(), p.download_bytes() * 3);
        assert!(p.free_bytes_needed() > p.disk_bytes());
        let (repo, images) = lists(MAC);
        let p = plan(&repo, &images, MAC, &Have::default(), "37.3.10").unwrap();
        let ids: Vec<&str> = p.licences.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(ids, ["android-sdk-license", "android-sdk-arm-dbt-license"]);
    }

    /// The update check: a newer stable emulator than Flare's own is an update of the emulator alone; a folder that
    /// has everything needs nothing; a list whose newest emulator is under the floor is refused.
    #[test]
    fn a_plan_fetches_only_what_flares_folder_lacks() {
        let (repo, images) = lists(WIN);
        let have = Have {
            emulator: Some("37.2.13".into()),
            platform_tools: true,
            avd: true,
        };
        let p = plan(&repo, &images, WIN, &have, "37.2.13").unwrap();
        assert!(p.update);
        assert_eq!(p.get.len(), 1);
        assert_eq!(p.get[0].remote.path, "emulator");
        assert_eq!(p.image, None);
        let have = Have {
            emulator: Some("37.3.10".into()),
            ..have
        };
        let p = plan(&repo, &images, WIN, &have, "37.2.13").unwrap();
        assert!(p.is_empty() && !p.update);
        assert_eq!(p.newest_emulator, "37.3.10");
        let e = plan(&repo, &images, WIN, &Have::default(), "38.0").unwrap_err();
        assert!(e.contains("Flare needs 38.0"), "{e}");
    }

    /// The SDK's licence hash is the SHA-1 of the normalised text; the normalising is held here by its exact output on
    /// small cases (the hash itself is checked in the emu host, which has SHA-1).
    #[test]
    fn the_licence_text_is_normalised_as_the_sdk_does() {
        assert_eq!(licence_text("  a\n b  c\n\n\n  d \t e\n"), "a b c\n\n\nd e");
        assert_eq!(licence_text("x\r\ny"), "x\r y");
        assert_eq!(licence_text("1.\n\n2."), "1.\n\n2.");
        let (repo, _) = lists(WIN);
        let t = licence_text(&repo.licences["android-sdk-license"]);
        assert!(t.starts_with("Terms and Conditions\n\nThis is the Android Software"));
        assert!(t.ends_with("January 16, 2019"));
    }

    /// Rule 1361: an archive is only ever a file in the list's own folder on dl.google.com.
    #[test]
    fn an_archive_url_stays_in_googles_folder() {
        assert_eq!(
            archive_url(REPO, "emulator-windows_x64-16916650.zip").as_deref(),
            Some("https://dl.google.com/android/repository/emulator-windows_x64-16916650.zip")
        );
        for bad in [
            "https://evil.example/x.zip",
            "../x.zip",
            "a/b.zip",
            "a\\b.zip",
            "",
            ".hidden",
            "x..zip",
            "a b.zip",
        ] {
            assert_eq!(archive_url(REPO, bad), None, "{bad}");
        }
        assert_eq!(archive_url("https://evil.example/", "x.zip"), None);
        assert_eq!(
            archive_url("http://dl.google.com/android/repository/", "x.zip"),
            None
        );
        let xml = REPO_XML.replacen(
            "<url>emulator-windows_x64-",
            "<url>https://evil.example/emulator-windows_x64-",
            1,
        );
        assert!(parse_list(&xml, REPO, WIN).is_err());
        // The same name for another computer is skipped, not refused.
        let xml = REPO_XML.replacen(
            "<url>emulator-linux_x64-",
            "<url>https://evil.example/emulator-linux_x64-",
            1,
        );
        assert!(parse_list(&xml, REPO, WIN).is_ok());
        assert!(
            parse_list(
                &xml,
                REPO,
                HostCpu {
                    os: "linux",
                    arch: "x64"
                }
            )
            .is_err()
        );
    }

    /// No entity is ever expanded: a list with a DOCTYPE (a "billion laughs" here) is refused.
    #[test]
    fn a_list_with_a_doctype_is_refused() {
        let bomb = "<?xml version=\"1.0\"?>\n<!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">]>\n<r><license id=\"a\">&lol2;</license></r>";
        let e = parse_list(bomb, REPO, WIN).unwrap_err();
        assert!(e.contains("does not read"), "{e}");
        let ext = "<?xml version=\"1.0\"?>\n<!DOCTYPE r [<!ENTITY x SYSTEM \"file:///etc/passwd\">]>\n<r>&x;</r>";
        assert!(parse_list(ext, REPO, WIN).is_err());
        assert!(parse_list("not xml", REPO, WIN).is_err());
        assert_eq!(parse_list("<r/>", REPO, WIN).unwrap(), Listing::default());
    }

    /// The `package.xml` reads back as the package it describes, with Google's description copied in.
    #[test]
    fn a_package_xml_reads_back() {
        let (repo, images) = lists(MAC);
        let p = plan(&repo, &images, MAC, &Have::default(), "37.3.10").unwrap();
        for g in &p.get {
            let text = &p
                .licences
                .iter()
                .find(|(i, _)| Some(i) == g.remote.licence.as_ref())
                .unwrap()
                .1;
            let x = package_xml(g, text);
            assert_eq!(
                read_package_xml(&x),
                Some((g.remote.path.clone(), g.remote.version.clone())),
                "{x}"
            );
            assert!(x.contains("<localPackage path="));
            assert!(x.contains("type-details"));
            assert!(!x.contains("archives"), "no download addresses");
            assert!(!x.contains("channelRef"));
            let doc = roxmltree::Document::parse(&x).unwrap();
            let lic = doc
                .root_element()
                .children()
                .find(|c| c.has_tag_name("license"))
                .unwrap();
            assert_eq!(
                lic.text(),
                Some(text.as_str()),
                "the licence survives the escaping"
            );
        }
        let emu = &p.get[0].remote;
        assert_eq!(
            source_properties(emu),
            "Pkg.Desc=Android Emulator\nPkg.Revision=37.3.10\nPkg.Path=emulator\n"
        );
    }

    #[test]
    fn this_computer_has_a_cpu_the_lists_know() {
        let h = HostCpu::this().unwrap();
        if cfg!(windows) {
            assert_eq!((h.os, h.abi()), ("windows", "x86_64"));
        }
        if cfg!(target_os = "macos") {
            assert_eq!(h.os, "macosx");
        }
        assert_eq!(MAC.abi(), "arm64-v8a");
    }
}
