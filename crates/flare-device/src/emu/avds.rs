//! The virtual devices (AVDs) this machine has: the person's (their AVD folder) and Flare's own (Flare's `avd`
//! folder, A2b). Read straight from the folders: each AVD is `<name>.ini` (with `path=` to its folder) beside
//! `<name>.avd/config.ini` (screen size, density, image). No `avdmanager`, no Java (rule 1361).

use std::path::{Path, PathBuf};

use super::Places;
use crate::geom::Size;
use crate::target::Target;

/// The person's AVD folder: `ANDROID_AVD_HOME`, else `ANDROID_USER_HOME/avd`, else `ANDROID_EMULATOR_HOME/avd`, else
/// `~/.android/avd` (the emulator's own order).
pub fn persons_home(p: &Places) -> Option<PathBuf> {
    if let Some(h) = &p.android_avd_home {
        return Some(h.clone());
    }
    if let Some(h) = p.android_user_home.as_ref().or(p.android_emulator_home.as_ref()) {
        return Some(h.join("avd"));
    }
    Some(p.home.as_ref()?.join(".android").join("avd"))
}

/// The folders to list: the person's, then Flare's own (each once).
pub fn homes(p: &Places) -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    if let Some(h) = persons_home(p) {
        out.push((h, false));
    }
    if let Some(h) = p.flare_avds()
        && !out.iter().any(|(o, _)| *o == h)
    {
        out.push((h, true));
    }
    out
}

/// One virtual device.
#[derive(Clone, Debug, PartialEq)]
pub struct Avd {
    /// Its name, as `emulator -avd` takes it.
    pub name: String,
    /// The folder holding `<name>.ini` (`ANDROID_AVD_HOME` when it boots).
    pub home: PathBuf,
    /// `<name>.avd`.
    pub dir: PathBuf,
    /// For the person: `avd.ini.displayname`, else the name with its underscores as spaces.
    pub display_name: String,
    /// The portrait screen in pixels and its density, from `config.ini`.
    pub size_px: Option<Size>,
    pub dpi: Option<f64>,
    /// `target` (`android-37.0`).
    pub api: Option<String>,
    pub abi: Option<String>,
    /// In Flare's own folder.
    pub flare_own: bool,
}

impl Avd {
    pub fn target(&self) -> Option<Target> {
        Target::emu(&self.name)
    }

    /// "Android 37" from `android-37.0` (the device list's OS line), or "Android" when it does not say.
    pub fn os(&self) -> String {
        let n = self
            .api
            .as_deref()
            .and_then(|t| t.strip_prefix("android-"))
            .map(|v| v.trim_end_matches(".0"));
        match n {
            Some(v) if !v.is_empty() => format!("Android {v}"),
            _ => "Android".to_string(),
        }
    }
}

/// The folder a `<name>.ini` points at (`path=`), resolved against `home` when it is relative (`path.rel=`).
pub fn pointed_dir(home: &Path, ini: &str) -> Option<PathBuf> {
    let m = super::parse_ini(ini);
    if let Some(p) = m.get("path").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    // `path.rel` is relative to the folder above the AVD folder (`avd/<name>.avd`).
    let rel = m.get("path.rel").filter(|p| !p.is_empty())?;
    Some(home.parent().unwrap_or(home).join(rel))
}

/// An AVD from its `config.ini`.
pub fn parse_config(name: &str, home: &Path, dir: &Path, config: &str, flare_own: bool) -> Avd {
    let m = super::parse_ini(config);
    let num = |k: &str| m.get(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0);
    let size_px = match (num("hw.lcd.width"), num("hw.lcd.height")) {
        (Some(w), Some(h)) => Some(Size::new(w, h)),
        _ => None,
    };
    let display_name = m
        .get("avd.ini.displayname")
        .filter(|d| !d.is_empty())
        .cloned()
        .unwrap_or_else(|| name.replace('_', " "));
    Avd {
        name: name.to_string(),
        home: home.to_path_buf(),
        dir: dir.to_path_buf(),
        display_name,
        size_px,
        dpi: num("hw.lcd.density"),
        api: m.get("target").or(m.get("image.sysdir.1")).cloned().map(|t| {
            // `image.sysdir.1=system-images\android-37.0\google_apis\x86_64\` names the API too.
            t.replace('\\', "/")
                .split('/')
                .find(|p| p.starts_with("android-"))
                .unwrap_or(&t)
                .to_string()
        }),
        abi: m.get("abi.type").cloned(),
        flare_own,
    }
}

/// BLOCKING (reads the folders): every AVD whose name Flare takes, sorted by name; a name in both folders is the
/// person's.
pub fn list(homes: &[(PathBuf, bool)]) -> Vec<Avd> {
    let mut out: Vec<Avd> = Vec::new();
    for (home, own) in homes {
        let Ok(rd) = std::fs::read_dir(home) else {
            // sync-ok: the emu host and Flare's workers
            continue;
        };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            let Some(name) = file.strip_suffix(".ini") else {
                continue;
            };
            if Target::emu(name).is_none() || out.iter().any(|a| a.name == name) {
                continue;
            }
            let Ok(ini) = std::fs::read_to_string(e.path()) else {
                // sync-ok: as above
                continue;
            };
            let dir = pointed_dir(home, &ini).unwrap_or_else(|| home.join(format!("{name}.avd")));
            let config = std::fs::read_to_string(dir.join("config.ini")).unwrap_or_default(); // sync-ok: as above
            out.push(parse_config(name, home, &dir, &config, *own));
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\
AvdId=Flare_Pixel
avd.ini.displayname=Flare Pixel
abi.type=x86_64
hw.lcd.width=1080
hw.lcd.height=2400
hw.lcd.density=420
image.sysdir.1=system-images\\android-37.0\\google_apis\\x86_64\\
";

    #[test]
    fn a_config_gives_the_screen_the_density_and_the_os() {
        let a = parse_config("Flare_Pixel", Path::new("/h/avd"), Path::new("/h/avd/Flare_Pixel.avd"), CONFIG, true);
        assert_eq!(a.display_name, "Flare Pixel");
        assert_eq!(a.size_px, Some(Size::new(1080.0, 2400.0)));
        assert_eq!(a.dpi, Some(420.0));
        assert_eq!(a.api.as_deref(), Some("android-37.0"));
        assert_eq!(a.os(), "Android 37");
        assert_eq!(a.abi.as_deref(), Some("x86_64"));
        assert_eq!(a.target(), Target::emu("Flare_Pixel"));
        // A bare config: the name with spaces, no screen.
        let b = parse_config("Pixel_8_API_36", Path::new("/h"), Path::new("/h/x"), "target=android-36\n", false);
        assert_eq!(b.display_name, "Pixel 8 API 36");
        assert_eq!((b.size_px, b.dpi), (None, None));
        assert_eq!(b.os(), "Android 36");
        let c = parse_config("X", Path::new("/h"), Path::new("/h/x"), "hw.lcd.width=-1\nhw.lcd.height=9\n", false);
        assert_eq!((c.size_px, c.os().as_str()), (None, "Android"));
    }

    #[test]
    fn the_persons_avd_folder_follows_the_emulators_order() {
        let mut p = Places {
            home: Some("/u".into()),
            local_app_data: Some("/l".into()),
            ..Places::default()
        };
        assert_eq!(persons_home(&p).unwrap(), Path::new("/u/.android/avd"));
        p.android_emulator_home = Some("/e".into());
        assert_eq!(persons_home(&p).unwrap(), Path::new("/e/avd"));
        p.android_user_home = Some("/uh".into());
        assert_eq!(persons_home(&p).unwrap(), Path::new("/uh/avd"));
        p.android_avd_home = Some("/avds".into());
        assert_eq!(persons_home(&p).unwrap(), Path::new("/avds"));
        let h = homes(&p);
        assert_eq!(h.len(), 2);
        assert!(!h[0].1 && h[1].1, "the person's first, then Flare's");
    }

    #[test]
    fn the_folders_list_every_avd_once() {
        let mine = tempfile::tempdir().unwrap();
        let flares = tempfile::tempdir().unwrap();
        let put = |home: &Path, name: &str, config: &str, absolute: bool| {
            let dir = home.join(format!("{name}.avd"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("config.ini"), config).unwrap();
            let ini = if absolute {
                format!("avd.ini.encoding=UTF-8\npath={}\n", dir.display())
            } else {
                "avd.ini.encoding=UTF-8\n".to_string()
            };
            std::fs::write(home.join(format!("{name}.ini")), ini).unwrap();
        };
        put(mine.path(), "Pixel_8", "hw.lcd.width=1080\nhw.lcd.height=2400\nhw.lcd.density=420\n", true);
        put(mine.path(), "Same", "avd.ini.displayname=Mine\n", false);
        put(flares.path(), "Flare_Pixel", CONFIG, true);
        put(flares.path(), "Same", "avd.ini.displayname=Flare's\n", true);
        // A name the emulator would read as a flag is not listed.
        std::fs::write(mine.path().join("-bad.ini"), "path=/x\n").unwrap();
        let l = list(&[(mine.path().to_path_buf(), false), (flares.path().to_path_buf(), true)]);
        let names: Vec<_> = l.iter().map(|a| (a.name.as_str(), a.flare_own)).collect();
        assert_eq!(names, [("Flare_Pixel", true), ("Pixel_8", false), ("Same", false)]);
        assert_eq!(l[2].display_name, "Mine", "the person's copy wins");
        assert_eq!(l[1].dpi, Some(420.0));
        assert_eq!(l[0].home, flares.path());
        assert!(list(&[(mine.path().join("none"), false)]).is_empty());
    }
}
