//! Android setup (Android plan PR A2b, rule 1361): Flare installs the emulator, platform-tools and one system image
//! into its own SDK folder, from Google's own server, after the person accepts Google's licence in Flare. No Java, no
//! `sdkmanager`, no `avdmanager`. The pure parts (the lists, the pick, the licence text, `package.xml`, the device's
//! files) are in `flare_device::emu::{setup, flare_avd}`.
//!
//! Every step runs on a blocking worker, never on the control loop:
//! 1. The plan: both package lists over HTTPS (at most [`setup::LIST_MAX_BYTES`] each), what Flare's folder lacks.
//! 2. The download: each archive streamed into a temporary folder inside Flare's SDK folder (so the last step is a
//!    rename on one volume), its size and SHA-1 checked before anything is unpacked. A cancel is seen within
//!    [`POLL`]; a dead line gives up after [`IDLE_LIMIT`] without a byte (no total limit: 2.5 GB takes long on a slow
//!    line).
//! 3. The unpack: into the temporary folder (only names that stay inside it, one top folder per zip, unix modes
//!    kept), then `package.xml` written, then renamed into place. A cancelled or broken setup removes the temporary
//!    folder, so nothing half-unpacked is ever left (a package already renamed into place is whole).
//! 4. The licences (`licenses/<id>`, the SHA-1 of the text as the SDK hashes it) and Flare's own device.
//!
//! The person's own SDK is never written: everything goes under `Places::flare_sdk` and `Places::flare_avds`.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant};

use flare_device::emu::setup::{self, Get, Have, HostCpu, Listing, Plan};
use flare_device::emu::{Places, flare_avd, sdk};
use flare_device::wire::SetupEvent;

/// No byte for this long is a dead line.
pub const IDLE_LIMIT: Duration = Duration::from_secs(60);
/// How often a waiting download looks at the cancel flag.
pub const POLL: Duration = Duration::from_millis(100);
/// Only so an abandoned reader cannot live for ever.
const CEILING: Duration = Duration::from_secs(12 * 60 * 60);
/// At most this often a progress event.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// The temporary folders' names start with this, inside Flare's SDK folder.
pub const TMP_PREFIX: &str = ".flare-setup-";

/// Why a setup stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    Failed(String),
}

impl From<String> for Stop {
    fn from(s: String) -> Stop {
        Stop::Failed(s)
    }
}

/// Where the bytes come from: the network in Flare, memory in the tests.
pub trait Fetch: Sync {
    /// Streams `url` into `sink`; `sink` answers false to stop (a cancel). An empty slice is a ping while waiting.
    fn get(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Stop>;
}

/// HTTPS with ureq (the dictation downloader's pattern: the blocking reads on a helper thread, this side waits in
/// short slices so a cancel is seen at once).
pub struct Https {
    agent: ureq::Agent,
}

impl Https {
    pub fn new() -> Https {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_global(Some(CEILING))
            .build()
            .into();
        Https { agent }
    }
}

impl Fetch for Https {
    fn get(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Stop> {
        if !url.starts_with(setup::REPO) {
            return Err(Stop::Failed(format!("{url} is not on Google's server")));
        }
        let (tx, rx) = sync_channel::<Result<Vec<u8>, String>>(4);
        let agent = self.agent.clone();
        let u = url.to_string();
        std::thread::spawn(move || {
            let r = match agent.get(&u).call() {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(format!("{u}: {e}")));
                    return;
                }
            };
            let mut reader = r.into_body().into_reader();
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => {
                        if tx.send(Ok(buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(format!("{u}: {e}")));
                        return;
                    }
                }
            }
        });
        let mut last = Instant::now();
        loop {
            match rx.recv_timeout(POLL) {
                Ok(Ok(chunk)) => {
                    last = Instant::now();
                    if !sink(&chunk) {
                        return Err(Stop::Cancelled);
                    }
                }
                Ok(Err(e)) => return Err(Stop::Failed(e)),
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {
                    if !sink(&[]) {
                        return Err(Stop::Cancelled);
                    }
                    if last.elapsed() >= IDLE_LIMIT {
                        return Err(Stop::Failed(format!(
                            "no data from Google for {} s",
                            IDLE_LIMIT.as_secs()
                        )));
                    }
                }
            }
        }
    }
}

/// The SHA-1 of a licence as the SDK keeps it in `licenses/<id>`.
pub fn licence_hash(raw: &str) -> String {
    sha1_smol::Sha1::from(setup::licence_text(raw))
        .digest()
        .to_string()
}

/// BLOCKING: a list, at most [`setup::LIST_MAX_BYTES`].
pub fn fetch_list(f: &dyn Fetch, url: &str, cancel: &AtomicBool) -> Result<String, Stop> {
    let mut buf: Vec<u8> = Vec::new();
    let mut too_big = false;
    let r = f.get(url, &mut |c| {
        if buf.len() as u64 + c.len() as u64 > setup::LIST_MAX_BYTES {
            too_big = true;
            return false;
        }
        buf.extend_from_slice(c);
        !cancel.load(Ordering::Relaxed)
    });
    if too_big {
        return Err(Stop::Failed(format!("{url} is too big for a package list")));
    }
    r?;
    String::from_utf8(buf).map_err(|_| Stop::Failed(format!("{url} is not text")))
}

/// BLOCKING: both lists.
pub fn read_lists(
    f: &dyn Fetch,
    host: HostCpu,
    cancel: &AtomicBool,
) -> Result<(Listing, Listing), Stop> {
    let repo = setup::parse_list(&fetch_list(f, setup::REPO_LIST, cancel)?, setup::REPO, host)?;
    let images = setup::parse_list(
        &fetch_list(f, setup::IMAGES_LIST, cancel)?,
        setup::IMAGES,
        host,
    )?;
    Ok((repo, images))
}

/// BLOCKING (reads a few small files): what Flare's own folders hold.
pub fn have(sdk_root: &Path, avd_home: &Path) -> Have {
    Have {
        emulator: sdk::read_version(sdk_root),
        platform_tools: sdk::adb_exe(sdk_root).is_file(),
        avd: avd_home.join(format!("{}.ini", flare_avd::NAME)).exists(),
    }
}

/// Flare's own SDK and AVD folders.
pub fn folders(p: &Places) -> Result<(PathBuf, PathBuf), Stop> {
    match (p.flare_sdk(), p.flare_avds()) {
        (Some(s), Some(a)) => Ok((s, a)),
        _ => Err(Stop::Failed("no folder for Flare's own files".to_string())),
    }
}

/// BLOCKING: free bytes on the volume of `path` (or of its nearest folder that exists).
pub fn free_bytes(path: &Path) -> Option<u64> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    free_on(&p)
}

#[cfg(windows)]
fn free_on(p: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let mut free: u64 = 0;
    // SAFETY: `wide` is a NUL-ended path that outlives the call; the out pointer is a live u64, the others may be null.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(unix)]
fn free_on(p: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a NUL-ended path; `s` is a zeroed statvfs the call fills.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: as above; both pointers are live for the call.
    let r = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
    #[allow(clippy::unnecessary_cast)] // the fields' widths differ per OS
    (r == 0).then(|| (s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
}

/// Removes temporary folders a setup left (the guard ends the emu host with Flare, maybe mid-download).
pub fn sweep(sdk_root: &Path) {
    let Ok(rd) = std::fs::read_dir(sdk_root) else {
        return;
    };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with(TMP_PREFIX) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// What a finished setup did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub installed: Vec<String>,
    /// Flare's own device was written now.
    pub avd: bool,
}

/// BLOCKING: does `plan` into `sdk_root` and `avd_home`. `accepted`: the licence hashes the person accepted (each of
/// the plan's licences must be among them). `free`: the free bytes on the SDK's volume. Nothing half-unpacked is left
/// on any return.
#[allow(clippy::too_many_arguments)] // each is its own input; a struct would only name them again
pub fn install(
    f: &dyn Fetch,
    plan: &Plan,
    sdk_root: &Path,
    avd_home: &Path,
    accepted: &[String],
    free: Option<u64>,
    cancel: &AtomicBool,
    event: &mut dyn FnMut(SetupEvent),
) -> Result<Done, Stop> {
    for (id, text) in &plan.licences {
        if !accepted.contains(&licence_hash(text)) {
            return Err(Stop::Failed(format!(
                "Google's licence {id} was not accepted (or it changed); read it again"
            )));
        }
    }
    if let Some(free) = free
        && free < plan.free_bytes_needed()
    {
        return Err(Stop::Failed(format!(
            "Not enough free space: setting up needs about {} GB, {} GB is free.",
            gb(plan.free_bytes_needed()),
            gb(free)
        )));
    }
    std::fs::create_dir_all(sdk_root).map_err(|e| format!("{}: {e}", sdk_root.display()))?;
    sweep(sdk_root);
    let tmp = sdk_root.join(format!("{TMP_PREFIX}{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let r = install_in(f, plan, sdk_root, avd_home, &tmp, cancel, event);
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

fn gb(b: u64) -> String {
    format!("{:.1}", b as f64 / 1e9)
}

fn install_in(
    f: &dyn Fetch,
    plan: &Plan,
    sdk_root: &Path,
    avd_home: &Path,
    tmp: &Path,
    cancel: &AtomicBool,
    event: &mut dyn FnMut(SetupEvent),
) -> Result<Done, Stop> {
    let total = plan.download_bytes();
    let mut before = 0u64;
    let mut done = Done::default();
    for (n, g) in plan.get.iter().enumerate() {
        let zip = tmp.join(format!("{n}.zip"));
        download(f, g, &zip, before, total, cancel, event)?;
        before += g.archive.size;
        event(SetupEvent::Unpack {
            package: g.remote.path.clone(),
        });
        let out = tmp.join(n.to_string());
        let top = unzip(&zip, &out, cancel)?;
        let _ = std::fs::remove_file(&zip);
        let pkg = out.join(&top);
        let text = plan
            .licences
            .iter()
            .find(|(i, _)| Some(i) == g.remote.licence.as_ref())
            .map(|(_, t)| t.as_str())
            .unwrap_or_default();
        write_file(&pkg.join("package.xml"), &setup::package_xml(g, text))?;
        if !pkg.join("source.properties").exists() {
            write_file(
                &pkg.join("source.properties"),
                &setup::source_properties(&g.remote),
            )?;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        put_in_place(
            &pkg,
            &sdk_root.join(g.remote.dest()),
            &tmp.join(format!("{n}-old")),
        )?;
        done.installed
            .push(format!("{} {}", g.remote.path, g.remote.version));
    }
    for (id, text) in &plan.licences {
        record_licence(sdk_root, id, &licence_hash(text))?;
    }
    if let Some(img) = plan
        .image
        .as_ref()
        .and_then(|i| flare_avd::Image::from_path(&i.path))
    {
        done.avd = flare_avd::write(avd_home, &img)
            .map_err(|e| format!("Flare's device: {e}"))?
            .is_some();
    }
    Ok(done)
}

fn write_file(p: &Path, text: &str) -> Result<(), Stop> {
    std::fs::write(p, text).map_err(|e| Stop::Failed(format!("{}: {e}", p.display())))
}

/// Streams one archive into `zip`, checking its size and SHA-1 as it goes.
fn download(
    f: &dyn Fetch,
    g: &Get,
    zip: &Path,
    before: u64,
    total: u64,
    cancel: &AtomicBool,
    event: &mut dyn FnMut(SetupEvent),
) -> Result<(), Stop> {
    let a = &g.archive;
    let mut file = std::fs::File::create(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut sha = sha1_smol::Sha1::new();
    let mut got = 0u64;
    let mut fail: Option<String> = None;
    let mut said = Instant::now() - PROGRESS_EVERY;
    let mb = |b: u64| b / 1_000_000;
    let r = f.get(&a.url, &mut |c| {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        if c.is_empty() {
            return true;
        }
        got += c.len() as u64;
        if got > a.size {
            fail = Some(format!("{} is bigger than Google's list says", a.url));
            return false;
        }
        sha.update(c);
        if let Err(e) = file.write_all(c) {
            fail = Some(format!("{}: {e}", zip.display()));
            return false;
        }
        if said.elapsed() >= PROGRESS_EVERY || got == a.size {
            said = Instant::now();
            event(SetupEvent::Download {
                package: g.remote.path.clone(),
                done_mb: mb(before + got),
                total_mb: mb(total),
            });
        }
        true
    });
    if let Some(e) = fail {
        return Err(Stop::Failed(e));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Stop::Cancelled);
    }
    r?;
    file.flush()
        .map_err(|e| format!("{}: {e}", zip.display()))?;
    if got != a.size {
        return Err(Stop::Failed(format!(
            "The download failed: {} ended after {got} of {} bytes.",
            a.url, a.size
        )));
    }
    let sum = sha.digest().to_string();
    if sum != a.sha1 {
        return Err(Stop::Failed(format!(
            "The download failed: {} does not match its checksum.",
            a.url
        )));
    }
    Ok(())
}

/// Unzips `zip` into `out` and answers its one top folder. Only names that stay inside `out` are taken
/// (`enclosed_name`), every entry must be under one top folder, and unix modes are kept.
///
/// No link exists while files are written: folders and regular files come first, links last. A link's target must be
/// relative with no `..` (so it can only point down from its own folder), and a link is made only on unix. Before
/// each write the parent folder is canonicalized and must still be inside `out`, so nothing is ever written through a
/// link.
pub fn unzip(zip: &Path, out: &Path, cancel: &AtomicBool) -> Result<String, Stop> {
    let file = std::fs::File::open(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut a =
        zip::ZipArchive::new(file).map_err(|e| format!("the download is not a zip: {e}"))?;
    let mut top: Option<String> = None;
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let root = out
        .canonicalize()
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let mut links: Vec<(usize, PathBuf)> = Vec::new();
    for i in 0..a.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let mut e = a.by_index(i).map_err(|e| format!("the zip: {e}"))?;
        let rel = e
            .enclosed_name()
            .ok_or_else(|| format!("the zip names a file outside its folder: {:?}", e.name()))?;
        let first = match rel.components().next() {
            Some(Component::Normal(c)) => c.to_string_lossy().to_string(),
            _ => {
                return Err(Stop::Failed(format!(
                    "an odd name in the zip: {:?}",
                    e.name()
                )));
            }
        };
        match &top {
            None => top = Some(first.clone()),
            Some(t) if *t != first => {
                return Err(Stop::Failed(
                    "the zip holds more than one folder".to_string(),
                ));
            }
            Some(_) => {}
        }
        let path = out.join(&rel);
        if e.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            inside(&root, &path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            inside(&root, parent)?;
        }
        if e.is_symlink() {
            links.push((i, path));
            continue;
        }
        let mut w = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::io::copy(&mut e, &mut w).map_err(|e| format!("{}: {e}", path.display()))?;
        #[cfg(unix)]
        if let Some(mode) = e.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode & 0o755));
        }
    }
    for (i, path) in links {
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let mut e = a.by_index(i).map_err(|e| format!("the zip: {e}"))?;
        let mut target = String::new();
        e.read_to_string(&mut target)
            .map_err(|e| format!("the zip: {e}"))?;
        if !link_target_ok(&target) {
            return Err(Stop::Failed(format!(
                "the zip has a link outside its folder: {target:?}"
            )));
        }
        if let Some(parent) = path.parent() {
            inside(&root, parent)?;
        }
        link(&path, &target)?;
    }
    top.ok_or_else(|| Stop::Failed("the zip is empty".to_string()))
}

/// Refuses a write whose folder `dir`, with every link in it followed, is not inside `root` (already canonical).
fn inside(root: &Path, dir: &Path) -> Result<(), Stop> {
    let real = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    if real.starts_with(root) {
        Ok(())
    } else {
        Err(Stop::Failed(format!(
            "the zip writes outside its folder: {}",
            dir.display()
        )))
    }
}

/// Whether a link in a package may point at `target`: relative, not empty, no `..`, no backslash, no colon (a drive),
/// no root. Such a target only ever points down from the link's own folder.
pub fn link_target_ok(target: &str) -> bool {
    let t = Path::new(target);
    if target.is_empty() || target.contains(['\\', ':']) || t.is_absolute() || t.has_root() {
        return false;
    }
    t.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(unix)]
fn link(at: &Path, target: &str) -> Result<(), Stop> {
    std::os::unix::fs::symlink(target, at)
        .map_err(|e| Stop::Failed(format!("{}: {e}", at.display())))
}

#[cfg(not(unix))]
fn link(at: &Path, _target: &str) -> Result<(), Stop> {
    // Windows zips from Google hold no links; a safe one is skipped.
    log::warn!(
        "setup: a link in the zip is skipped on Windows: {}",
        at.display()
    );
    Ok(())
}

/// Renames the unpacked package to `dest`; an older copy there is moved to `old` first and deleted after (put back if
/// the rename fails).
fn put_in_place(pkg: &Path, dest: &Path, old: &Path) -> Result<(), Stop> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let had = dest.exists();
    if had {
        std::fs::rename(dest, old).map_err(|e| {
            format!(
                "{} is in use (shut down the Android devices first): {e}",
                dest.display()
            )
        })?;
    }
    if let Err(e) = std::fs::rename(pkg, dest) {
        if had {
            let _ = std::fs::rename(old, dest);
        }
        return Err(Stop::Failed(format!("{}: {e}", dest.display())));
    }
    if had {
        let _ = std::fs::remove_dir_all(old);
    }
    Ok(())
}

/// Adds `hash` to `licenses/<id>` the way the SDK keeps it (a line break, then the hash; older hashes stay).
pub fn record_licence(sdk_root: &Path, id: &str, hash: &str) -> Result<(), Stop> {
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || id.is_empty()
    {
        return Err(Stop::Failed(format!("an odd licence id: {id:?}")));
    }
    let dir = sdk_root.join("licenses");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let p = dir.join(id);
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    if old.lines().any(|l| l.trim() == hash) {
        return Ok(());
    }
    write_file(&p, &format!("{old}\n{hash}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::sync::Mutex;

    const REPO_XML: &str = include_str!("../../flare-device/src/emu/fixtures/repository2-3.xml");
    const IMAGES_XML: &str = include_str!("../../flare-device/src/emu/fixtures/sys-img2-3.xml");

    /// Rule 1361: the licence hash is the SDK's own (checked against an SDK `sdkmanager` set up on 2026-11-22).
    #[test]
    fn the_licence_hash_is_the_one_the_sdk_writes() {
        let host = HostCpu {
            os: "windows",
            arch: "x64",
        };
        let l = setup::parse_list(REPO_XML, setup::REPO, host).unwrap();
        assert_eq!(
            licence_hash(&l.licences["android-sdk-license"]),
            "24333f8a63b6825ea9c5514f83c2829b004d1fee"
        );
        assert_eq!(
            licence_hash(&l.licences["android-sdk-preview-license"]),
            "84831b9409646a918e30573bab4c9c91346d8abd"
        );
    }

    /// Files served from memory, in chunks; `stall_after`: stop sending after this many bytes and wait for a cancel.
    struct Mem {
        files: HashMap<String, Vec<u8>>,
        stall_after: Option<usize>,
        asked: Mutex<Vec<String>>,
    }

    impl Fetch for Mem {
        fn get(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Stop> {
            self.asked.lock().unwrap().push(url.to_string());
            let body = self
                .files
                .get(url)
                .ok_or_else(|| Stop::Failed(format!("404 {url}")))?;
            let mut sent = 0;
            for c in body.chunks(100) {
                if self.stall_after.is_some_and(|s| sent >= s) {
                    loop {
                        std::thread::sleep(Duration::from_millis(5));
                        if !sink(&[]) {
                            return Err(Stop::Cancelled);
                        }
                    }
                }
                if !sink(c) {
                    return Err(Stop::Cancelled);
                }
                sent += c.len();
            }
            Ok(())
        }
    }

    /// A zip with one top folder.
    fn zip_of(top: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        w.add_directory(format!("{top}/"), o).unwrap();
        for (name, text) in files {
            w.start_file(format!("{top}/{name}"), o).unwrap();
            w.write_all(text.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn sha(b: &[u8]) -> String {
        sha1_smol::Sha1::from(b).digest().to_string()
    }

    /// The saved lists with their archives swapped for small zips served from memory.
    fn world(host: HostCpu) -> (Mem, Plan) {
        let mut repo = setup::parse_list(REPO_XML, setup::REPO, host).unwrap();
        let mut images = setup::parse_list(IMAGES_XML, setup::IMAGES, host).unwrap();
        let mut files = HashMap::new();
        let emu_exe = if cfg!(windows) {
            "emulator.exe"
        } else {
            "emulator"
        };
        let adb_exe = if cfg!(windows) { "adb.exe" } else { "adb" };
        let bodies = [
            (
                "emulator",
                zip_of(
                    "emulator",
                    &[
                        (emu_exe, "x"),
                        ("source.properties", "Pkg.Revision=37.3.10\n"),
                    ],
                ),
            ),
            (
                "platform-tools",
                zip_of("platform-tools", &[(adb_exe, "x")]),
            ),
            (
                "system-images",
                zip_of(host_abi(host), &[("system.img", "img")]),
            ),
        ];
        for l in [&mut repo, &mut images] {
            for p in &mut l.packages {
                if let Some(a) = p.archive.as_mut() {
                    let key = p.path.split(';').next().unwrap();
                    if let Some((_, b)) = bodies.iter().find(|(k, _)| *k == key) {
                        a.size = b.len() as u64;
                        a.sha1 = sha(b);
                        files.insert(a.url.clone(), b.clone());
                    }
                }
            }
        }
        let plan = setup::plan(&repo, &images, host, &Have::default(), "37.2.13").unwrap();
        (
            Mem {
                files,
                stall_after: None,
                asked: Mutex::new(Vec::new()),
            },
            plan,
        )
    }

    fn host_abi(h: HostCpu) -> &'static str {
        h.abi()
    }

    fn accepted(p: &Plan) -> Vec<String> {
        p.licences.iter().map(|(_, t)| licence_hash(t)).collect()
    }

    fn leftovers(sdk: &Path) -> Vec<String> {
        std::fs::read_dir(sdk)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.starts_with(TMP_PREFIX))
                    .collect()
            })
            .unwrap_or_default()
    }

    const PC: HostCpu = HostCpu {
        os: "windows",
        arch: "x64",
    };

    /// Rule 1361: a full setup puts each package in place with its `package.xml`, the licence hash and Flare's own
    /// device; the emulator then reads as Flare's SDK at the floor.
    #[test]
    fn a_setup_installs_the_packages_the_licence_and_the_device() {
        let d = tempfile::tempdir().unwrap();
        let (sdk_root, avd_home) = (d.path().join("android-sdk"), d.path().join("avd"));
        let (f, plan) = world(PC);
        let mut events = Vec::new();
        let done = install(
            &f,
            &plan,
            &sdk_root,
            &avd_home,
            &accepted(&plan),
            Some(u64::MAX),
            &AtomicBool::new(false),
            &mut |e| events.push(e),
        )
        .unwrap();
        assert_eq!(done.installed.len(), 3);
        assert!(done.avd);
        assert_eq!(sdk::read_version(&sdk_root).as_deref(), Some("37.3.10"));
        assert!(sdk::adb_exe(&sdk_root).is_file());
        let img = sdk_root.join("system-images/android-37.0/google_apis/x86_64");
        assert!(img.join("system.img").is_file());
        let px = std::fs::read_to_string(img.join("package.xml")).unwrap();
        assert_eq!(
            setup::read_package_xml(&px),
            Some((
                "system-images;android-37.0;google_apis;x86_64".to_string(),
                "8".to_string()
            ))
        );
        assert!(
            img.join("source.properties").is_file(),
            "written when the zip has none"
        );
        assert_eq!(
            std::fs::read_to_string(sdk_root.join("licenses/android-sdk-license")).unwrap(),
            "\n24333f8a63b6825ea9c5514f83c2829b004d1fee"
        );
        assert!(avd_home.join("Flare_Pixel.avd/config.ini").is_file());
        assert!(leftovers(&sdk_root).is_empty(), "no temporary folder left");
        assert!(matches!(events.last(), Some(SetupEvent::Unpack { .. })));
        let last_mb = events
            .iter()
            .filter_map(|e| match e {
                SetupEvent::Download {
                    done_mb, total_mb, ..
                } => Some((*done_mb, *total_mb)),
                _ => None,
            })
            .next_back()
            .unwrap();
        assert_eq!(last_mb.0, last_mb.1, "the progress ends at the total");
        assert!(
            f.asked
                .lock()
                .unwrap()
                .iter()
                .all(|u| u.starts_with(setup::REPO))
        );
        // The same licence again is not written twice; another hash is added.
        record_licence(
            &sdk_root,
            "android-sdk-license",
            "24333f8a63b6825ea9c5514f83c2829b004d1fee",
        )
        .unwrap();
        record_licence(&sdk_root, "android-sdk-license", "abc").unwrap();
        assert_eq!(
            std::fs::read_to_string(sdk_root.join("licenses/android-sdk-license")).unwrap(),
            "\n24333f8a63b6825ea9c5514f83c2829b004d1fee\nabc"
        );
        assert!(record_licence(&sdk_root, "../x", "abc").is_err());
        // Now Flare's folder has everything: nothing more to do.
        let h = have(&sdk_root, &avd_home);
        assert_eq!(h.emulator.as_deref(), Some("37.3.10"));
        assert!(h.platform_tools && h.avd);
    }

    /// A checksum that does not match unpacks nothing and leaves nothing.
    #[test]
    fn a_bad_checksum_unpacks_nothing() {
        let d = tempfile::tempdir().unwrap();
        let sdk_root = d.path().join("android-sdk");
        let (f, mut plan) = world(PC);
        plan.get[0].archive.sha1 = "0".repeat(40);
        let e = install(
            &f,
            &plan,
            &sdk_root,
            &d.path().join("avd"),
            &accepted(&plan),
            None,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(&e, Stop::Failed(m) if m.contains("checksum")),
            "{e:?}"
        );
        assert!(!sdk_root.join("emulator").exists());
        assert!(leftovers(&sdk_root).is_empty());
        // A short download likewise.
        let (f, mut plan) = world(PC);
        plan.get[0].archive.size += 10;
        let e = install(
            &f,
            &plan,
            &sdk_root,
            &d.path().join("avd"),
            &accepted(&plan),
            None,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(&e, Stop::Failed(m) if m.contains("ended after")),
            "{e:?}"
        );
        assert!(leftovers(&sdk_root).is_empty());
    }

    /// A cancel mid-download stops at once and leaves nothing behind.
    #[test]
    fn a_cancel_mid_download_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let sdk_root = d.path().join("android-sdk");
        let (mut f, plan) = world(PC);
        f.stall_after = Some(100);
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let c = std::sync::Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            c.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let mut saw_download = false;
        let e = install(
            &f,
            &plan,
            &sdk_root,
            &d.path().join("avd"),
            &accepted(&plan),
            None,
            &cancel,
            &mut |e| saw_download |= matches!(e, SetupEvent::Download { .. }),
        )
        .unwrap_err();
        assert_eq!(e, Stop::Cancelled);
        assert!(saw_download);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(leftovers(&sdk_root).is_empty());
        assert!(!sdk_root.join("emulator").exists());
        assert!(!d.path().join("avd").exists());
    }

    /// Nothing is downloaded without the licence's hash, or without the room.
    #[test]
    fn no_download_before_the_licence_and_the_space() {
        let d = tempfile::tempdir().unwrap();
        let (f, plan) = world(PC);
        let e = install(
            &f,
            &plan,
            d.path(),
            d.path(),
            &["x".into()],
            None,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(&e, Stop::Failed(m) if m.contains("licence")),
            "{e:?}"
        );
        let e = install(
            &f,
            &plan,
            d.path(),
            d.path(),
            &accepted(&plan),
            Some(10),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(&e, Stop::Failed(m) if m.contains("free space")),
            "{e:?}"
        );
        assert!(f.asked.lock().unwrap().is_empty(), "nothing was fetched");
    }

    /// A list is read whole up to its limit; a bigger one is refused.
    #[test]
    fn a_list_has_a_size_limit() {
        let mut files = HashMap::new();
        files.insert(setup::REPO_LIST.to_string(), REPO_XML.as_bytes().to_vec());
        files.insert(
            setup::IMAGES_LIST.to_string(),
            vec![b' '; setup::LIST_MAX_BYTES as usize + 1],
        );
        let f = Mem {
            files,
            stall_after: None,
            asked: Mutex::new(Vec::new()),
        };
        let no = AtomicBool::new(false);
        assert_eq!(fetch_list(&f, setup::REPO_LIST, &no).unwrap(), REPO_XML);
        let e = read_lists(&f, PC, &no).unwrap_err();
        assert!(
            matches!(&e, Stop::Failed(m) if m.contains("too big")),
            "{e:?}"
        );
    }

    /// Unzip: names outside the folder, two top folders and links that leave the package are refused.
    #[test]
    fn an_unsafe_zip_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);
        let put = |name: &str, b: Vec<u8>| {
            let p = d.path().join(name);
            std::fs::write(&p, b).unwrap();
            p
        };
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("a/x", o).unwrap();
        w.start_file("b/y", o).unwrap();
        let two = put("two.zip", w.finish().unwrap().into_inner());
        assert!(
            matches!(unzip(&two, &d.path().join("o1"), &no), Err(Stop::Failed(m)) if m.contains("more than one"))
        );
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("../evil", o).unwrap();
        let evil = put("evil.zip", w.finish().unwrap().into_inner());
        assert!(unzip(&evil, &d.path().join("o2"), &no).is_err());
        assert!(!d.path().join("evil").exists());
        let ok = put("ok.zip", zip_of("emulator", &[("lib/a.txt", "a")]));
        assert_eq!(unzip(&ok, &d.path().join("o3"), &no).unwrap(), "emulator");
        assert!(d.path().join("o3/emulator/lib/a.txt").is_file());
        assert!(link_target_ok("a.so.1"));
        assert!(link_target_ok("x/./a.so"));
        assert!(!link_target_ok("../b"));
        assert!(!link_target_ok("x/../../etc/passwd"));
        assert!(!link_target_ok("/etc/passwd"));
        assert!(!link_target_ok("C:\\Windows"));
        assert!(!link_target_ok("C:/Windows"));
        assert!(!link_target_ok(""));
    }

    /// A hostile zip: a link that would let a later file land outside the folder, a link out by `..`, an absolute
    /// link, and a link named like a folder that files go into. Nothing lands outside `out`, no link is followed
    /// while files are written, and every one is refused.
    #[test]
    fn a_hostile_zip_writes_nothing_outside_its_folder() {
        let d = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);
        let o = zip::write::SimpleFileOptions::default();
        let outside = d.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("dotdot", {
                let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
                w.add_symlink("pkg/up", "../../outside", o).unwrap();
                w.start_file("pkg/a.txt", o).unwrap();
                w.write_all(b"a").unwrap();
                w.finish().unwrap().into_inner()
            }),
            ("absolute", {
                let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
                w.add_symlink("pkg/abs", outside.to_string_lossy(), o)
                    .unwrap();
                w.finish().unwrap().into_inner()
            }),
            // The link comes first in the zip; were it made at once, `pkg/l/x` would be written through it.
            ("link_then_file", {
                let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
                w.add_symlink("pkg/l", "../../outside", o).unwrap();
                w.start_file("pkg/l/x", o).unwrap();
                w.write_all(b"x").unwrap();
                w.finish().unwrap().into_inner()
            }),
            // A safe-looking link named like a folder files go into: the files are written first, as a real folder,
            // so the link cannot take its place.
            ("link_over_folder", {
                let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
                w.add_symlink("pkg/l", "sub", o).unwrap();
                w.start_file("pkg/l/x", o).unwrap();
                w.write_all(b"x").unwrap();
                w.finish().unwrap().into_inner()
            }),
        ];
        for (name, bytes) in cases {
            let z = d.path().join(format!("{name}.zip"));
            std::fs::write(&z, bytes).unwrap();
            let out = d.path().join(format!("o-{name}"));
            let r = unzip(&z, &out, &no);
            if cfg!(unix) || name != "link_over_folder" {
                assert!(r.is_err(), "{name}: {r:?}");
            }
            assert_eq!(
                std::fs::read_dir(&outside).unwrap().count(),
                0,
                "{name} wrote outside"
            );
            assert!(
                !out.join("pkg/l").is_symlink(),
                "{name}: a link replaced a folder"
            );
        }
        // A link that points down inside the package is made on unix (skipped on Windows), after the files.
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("pkg/lib/a.so.1", o).unwrap();
        w.write_all(b"so").unwrap();
        w.add_symlink("pkg/lib/a.so", "a.so.1", o).unwrap();
        let z = d.path().join("good.zip");
        std::fs::write(&z, w.finish().unwrap().into_inner()).unwrap();
        let out = d.path().join("o-good");
        assert_eq!(unzip(&z, &out, &no).unwrap(), "pkg");
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_to_string(out.join("pkg/lib/a.so")).unwrap(),
            "so"
        );
        #[cfg(not(unix))]
        assert!(!out.join("pkg/lib/a.so").exists());
    }

    /// The canonical-parent check refuses a folder that is a link out of the package (made here by hand, as a
    /// stand-in for one left by anything else).
    #[cfg(unix)]
    #[test]
    fn a_write_through_a_planted_link_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let out = d.path().join("o");
        std::fs::create_dir_all(out.join("pkg")).unwrap();
        std::os::unix::fs::symlink(&outside, out.join("pkg/l")).unwrap();
        let root = out.canonicalize().unwrap();
        assert!(inside(&root, &out.join("pkg")).is_ok());
        assert!(inside(&root, &out.join("pkg/l")).is_err());
        let o = zip::write::SimpleFileOptions::default();
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("pkg/l/x", o).unwrap();
        w.write_all(b"x").unwrap();
        let z = d.path().join("p.zip");
        std::fs::write(&z, w.finish().unwrap().into_inner()).unwrap();
        assert!(unzip(&z, &out, &AtomicBool::new(false)).is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[test]
    fn the_free_space_is_read() {
        let d = tempfile::tempdir().unwrap();
        assert!(free_bytes(&d.path().join("not/yet/made")).is_some_and(|b| b > 0));
    }

    /// The real setup from Google, by hand only (rule 1361's "a real download only in an opt-in test"): set
    /// `FLARE_EMU_REAL_SETUP` to an empty folder. It downloads about 2.5 GB into `<folder>/android-sdk` and writes
    /// `<folder>/avd`; boot it headless by hand, then delete the folder.
    #[test]
    #[ignore = "downloads about 2.5 GB from Google; by hand only"]
    fn a_real_setup_from_google() {
        let Some(root) = std::env::var_os("FLARE_EMU_REAL_SETUP").map(PathBuf::from) else {
            return;
        };
        let host = HostCpu::this().unwrap();
        let (sdk_root, avd_home) = (root.join("android-sdk"), root.join("avd"));
        let f = Https::new();
        let no = AtomicBool::new(false);
        let (repo, images) = read_lists(&f, host, &no).unwrap();
        let plan = setup::plan(
            &repo,
            &images,
            host,
            &have(&sdk_root, &avd_home),
            sdk::floor(),
        )
        .unwrap();
        eprintln!(
            "plan: {:?}, {} MB",
            plan.get.iter().map(|g| &g.remote.path).collect::<Vec<_>>(),
            plan.download_bytes() / 1_000_000
        );
        let started = Instant::now();
        let done = install(
            &f,
            &plan,
            &sdk_root,
            &avd_home,
            &accepted(&plan),
            free_bytes(&sdk_root),
            &no,
            &mut |e| {
                if let SetupEvent::Unpack { package } = e {
                    eprintln!("unpack {package} at {} s", started.elapsed().as_secs());
                }
            },
        )
        .unwrap();
        eprintln!("done in {} s: {done:?}", started.elapsed().as_secs());
        assert!(sdk::read_version(&sdk_root).is_some_and(|v| sdk::at_floor(&v, sdk::floor())));
    }
}
