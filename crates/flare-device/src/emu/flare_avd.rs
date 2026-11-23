//! Flare's own virtual device, "Flare Pixel" (Android plan PR A2b, spec "Setup"): written by hand in Flare's `avd`
//! folder, with no `avdmanager` (rule 1361). The Pixel 8 screen (1080x2400 at 420 dpi), 4 GB of memory, 4 cores, the
//! `google_apis` image setup chose. Flare's emulators run with `ANDROID_AVD_HOME` set to that folder and
//! `ANDROID_SDK_ROOT` to Flare's own SDK (`boot::command`), so the image path in `config.ini` is read against it.
//!
//! A device that exists is never written again (the person may have changed it). Its first boot ends with
//! "Preparing": the emu host runs `pm bg-dexopt-job` once (the guest compiles its apps in the background for a minute
//! or two after a first boot and draws about 36 fps meanwhile, Step 0 S7), then leaves [`PREPARED`] in its folder.

use std::path::{Path, PathBuf};

/// The AVD's name (`emulator -avd Flare_Pixel`, target `emu/Flare_Pixel`).
pub const NAME: &str = "Flare_Pixel";
/// What the person sees.
pub const DISPLAY_NAME: &str = "Flare Pixel";
/// The file in the AVD's folder that says its first-boot compile ran.
pub const PREPARED: &str = "flare-prepared";

/// The image a device runs: its folder under the SDK root and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// `system-images/android-37.0/google_apis/x86_64`.
    pub dest: String,
    /// `x86_64` or `arm64-v8a`.
    pub abi: String,
    /// `android-37.0`.
    pub platform: String,
}

impl Image {
    /// From an image package path (`system-images;android-37.0;google_apis;x86_64`).
    pub fn from_path(path: &str) -> Option<Image> {
        let parts: Vec<&str> = path.split(';').collect();
        let [_, platform, _, abi] = parts.as_slice() else {
            return None;
        };
        Some(Image {
            dest: parts.join("/"),
            abi: (*abi).to_string(),
            platform: (*platform).to_string(),
        })
    }
}

/// `config.ini`: the device itself.
pub fn config_ini(img: &Image) -> String {
    let arch = if img.abi == "arm64-v8a" {
        "arm64"
    } else {
        "x86_64"
    };
    let lines: [(&str, &str); 26] = [
        ("AvdId", NAME),
        ("avd.ini.displayname", DISPLAY_NAME),
        ("avd.ini.encoding", "UTF-8"),
        ("PlayStore.enabled", "no"),
        ("abi.type", &img.abi),
        ("hw.cpu.arch", arch),
        ("hw.cpu.ncore", "4"),
        ("hw.ramSize", "4096"),
        ("hw.lcd.width", "1080"),
        ("hw.lcd.height", "2400"),
        ("hw.lcd.density", "420"),
        ("hw.lcd.vsync", "60"),
        ("hw.gpu.enabled", "yes"),
        ("hw.gpu.mode", "host"),
        ("hw.keyboard", "yes"),
        ("hw.mainKeys", "no"),
        ("hw.audioInput", "no"),
        ("hw.camera.back", "none"),
        ("hw.camera.front", "none"),
        ("disk.dataPartition.size", "6G"),
        ("tag.id", super::setup::IMAGE_TAG),
        ("tag.display", "Google APIs"),
        ("target", &img.platform),
        ("showDeviceFrame", "no"),
        ("fastboot.forceColdBoot", "no"),
        ("firstboot.saveToLocalSnapshot", "yes"),
    ];
    let mut s: String = lines.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    s.push_str(&format!("image.sysdir.1={}/\n", img.dest));
    s
}

/// `<name>.ini` beside the device's folder: where it is (absolute, and relative to the folder above `home`).
pub fn pointer_ini(home: &Path, img: &Image) -> String {
    let dir = home.join(format!("{NAME}.avd"));
    let rel_home = home
        .file_name()
        .map_or_else(|| "avd".to_string(), |n| n.to_string_lossy().to_string());
    format!(
        "avd.ini.encoding=UTF-8\npath={}\npath.rel={rel_home}/{NAME}.avd\ntarget={}\n",
        dir.display(),
        img.platform
    )
}

/// BLOCKING (writes two small files): Flare Pixel in `home`. Ok(None) when it exists already (never written again),
/// Ok(Some(folder)) when written. A failed write leaves nothing behind.
pub fn write(home: &Path, img: &Image) -> std::io::Result<Option<PathBuf>> {
    let dir = home.join(format!("{NAME}.avd"));
    let ini = home.join(format!("{NAME}.ini"));
    if dir.exists() || ini.exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(home)?; // sync-ok: the emu host's setup worker
    let r = (|| {
        std::fs::create_dir(&dir)?; // sync-ok: as above
        std::fs::write(dir.join("config.ini"), config_ini(img))?; // sync-ok: as above
        std::fs::write(&ini, pointer_ini(home, img)) // sync-ok: as above
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_dir_all(&dir); // sync-ok: as above
        let _ = std::fs::remove_file(&ini); // sync-ok: as above
        return Err(e);
    }
    Ok(Some(dir))
}

/// Whether `avd` still needs its first-boot compile: only Flare's own devices (Flare never writes into the person's
/// AVD folder), and only once.
pub fn needs_preparing(avd: &super::avds::Avd) -> bool {
    avd.flare_own && !avd.dir.join(PREPARED).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emu::avds;

    fn img() -> Image {
        Image::from_path("system-images;android-37.0;google_apis;x86_64").unwrap()
    }

    /// Rule 1361: Flare's device is written by hand and reads back as the Pixel-sized device it says.
    #[test]
    fn flares_own_device_is_written_once_and_reads_back() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("avd");
        let dir = write(&home, &img()).unwrap().expect("written");
        assert_eq!(dir, home.join("Flare_Pixel.avd"));
        let cfg = std::fs::read_to_string(dir.join("config.ini")).unwrap();
        assert!(cfg.contains("image.sysdir.1=system-images/android-37.0/google_apis/x86_64/\n"));
        assert!(cfg.contains("hw.cpu.arch=x86_64\n"));
        assert!(cfg.contains("PlayStore.enabled=no\n"));
        let l = avds::list(&[(home.clone(), true)]);
        assert_eq!(l.len(), 1);
        let a = &l[0];
        assert_eq!(a.name, NAME);
        assert_eq!(a.display_name, "Flare Pixel");
        assert_eq!(a.size_px, Some(crate::geom::Size::new(1080.0, 2400.0)));
        assert_eq!(a.dpi, Some(420.0));
        assert_eq!(a.os(), "Android 37");
        assert_eq!(a.dir, dir);
        assert!(a.flare_own);
        // Never written again.
        std::fs::write(dir.join("config.ini"), "hw.lcd.width=1\n").unwrap();
        assert_eq!(write(&home, &img()).unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(dir.join("config.ini")).unwrap(),
            "hw.lcd.width=1\n"
        );
        // The pointer file names the folder both ways.
        let ini = std::fs::read_to_string(home.join("Flare_Pixel.ini")).unwrap();
        assert!(ini.contains("path.rel=avd/Flare_Pixel.avd\n"), "{ini}");
        assert!(ini.contains("target=android-37.0\n"));
    }

    #[test]
    fn an_arm_image_gives_an_arm_device() {
        let i = Image::from_path("system-images;android-37.0;google_apis;arm64-v8a").unwrap();
        let c = config_ini(&i);
        assert!(c.contains("abi.type=arm64-v8a\n") && c.contains("hw.cpu.arch=arm64\n"));
        assert_eq!(Image::from_path("emulator"), None);
    }

    /// Only Flare's own devices are prepared, and only once.
    #[test]
    fn only_a_new_flare_device_is_prepared() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("avd");
        write(&home, &img()).unwrap();
        let mut a = avds::list(&[(home.clone(), true)]).remove(0);
        assert!(needs_preparing(&a));
        std::fs::write(a.dir.join(PREPARED), b"").unwrap();
        assert!(!needs_preparing(&a));
        std::fs::remove_file(a.dir.join(PREPARED)).unwrap();
        a.flare_own = false;
        assert!(!needs_preparing(&a), "the person's device is never touched");
    }
}
