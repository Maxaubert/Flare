//! A Start menu shortcut's icon (rule 1272): read a `.lnk`'s taskbar id and icon, point its icon at a file, and tell
//! the shell the shortcut changed.
//!
//! Each call opens COM in the apartment model on the calling thread (a worker, never the UI thread) and closes it
//! again when the call opened it. Every interface is released before COM is closed. A `PROPVARIANT` here is only read:
//! the one `GetValue` gives back is COM's, and its own drop (`PropVariantClear`) frees it once (rule 1270).

/// What a shortcut says: its `PKEY_AppUserModel_ID` (None when it carries none) and its icon file and index (an empty
/// file when it names none, so the shell uses the target's own icon).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkFacts {
    pub app_id: Option<String>,
    pub icon: String,
    pub index: i32,
}

/// The current user's Start menu Programs folder (`FOLDERID_Programs`), where the installer puts Flare's shortcut.
#[cfg(windows)]
pub fn programs_dir() -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Programs, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: the shell gives out a string from COM's task memory; it is copied, then freed once here.
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None).ok()?;
        let text = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        text.filter(|t| !t.is_empty()).map(std::path::PathBuf::from)
    }
}

/// COM opened for one call on this thread, closed again on drop when this call opened it.
#[cfg(windows)]
struct Com(bool);

#[cfg(windows)]
impl Com {
    fn open() -> Result<Com, String> {
        use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: COM init on the calling thread; S_FALSE and a changed mode are fine.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(format!("CoInitializeEx {:#010x}", hr.0));
        }
        Ok(Com(hr.is_ok()))
    }
}

#[cfg(windows)]
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx of `open` on the same thread.
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// A NUL-ended UTF-16 copy of a path, for a `PCWSTR` the callee only reads during the call.
#[cfg(windows)]
fn wide(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// The shortcut at `lnk`, loaded in `mode`. The returned interfaces are released by the caller before COM closes.
#[cfg(windows)]
fn load(
    lnk: &std::path::Path,
    mode: windows::Win32::System::Com::STGM,
) -> Result<
    (
        windows::Win32::UI::Shell::IShellLinkW,
        windows::Win32::System::Com::IPersistFile,
    ),
    String,
> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};
    let path = wide(lnk);
    // SAFETY: plain COM calls on an apartment this thread opened; `path` outlives `Load`, which copies it.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("CoCreateInstance {:#010x}", e.code().0))?;
        let file: IPersistFile = link
            .cast()
            .map_err(|e| format!("IPersistFile {:#010x}", e.code().0))?;
        file.Load(PCWSTR::from_raw(path.as_ptr()), mode)
            .map_err(|e| format!("Load {:#010x}", e.code().0))?;
        Ok((link, file))
    }
}

/// Reads the shortcut at `lnk`: its taskbar id and its icon.
#[cfg(windows)]
pub fn read(lnk: &std::path::Path) -> Result<LinkFacts, String> {
    let com = Com::open()?;
    let facts = read_in(lnk);
    drop(com);
    facts
}

#[cfg(windows)]
fn read_in(lnk: &std::path::Path) -> Result<LinkFacts, String> {
    use crate::window_app_id::{APP_ID_FMTID, APP_ID_PID};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::STGM_READ;
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::core::{GUID, Interface};

    let (link, _file) = load(lnk, STGM_READ)?;
    let mut buf = vec![0u16; 1024];
    let mut index = 0i32;
    // SAFETY: `buf` is a writable buffer whose length the binding passes; `index` is a plain out value.
    unsafe { link.GetIconLocation(&mut buf, &mut index) }
        .map_err(|e| format!("GetIconLocation {:#010x}", e.code().0))?;
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let icon = String::from_utf16_lossy(&buf[..end]);
    let key = PROPERTYKEY {
        fmtid: GUID::from_u128(APP_ID_FMTID),
        pid: APP_ID_PID,
    };
    let store: IPropertyStore = link
        .cast()
        .map_err(|e| format!("IPropertyStore {:#010x}", e.code().0))?;
    // SAFETY: `key` lives across the call. The value is COM's: it is only read here, and its drop at the end of this
    // block frees it once (`PropVariantClear`).
    let app_id = unsafe {
        let value = store
            .GetValue(&key)
            .map_err(|e| format!("GetValue {:#010x}", e.code().0))?;
        let inner = &value.Anonymous.Anonymous;
        if inner.vt == VT_LPWSTR && !inner.Anonymous.pwszVal.is_null() {
            inner.Anonymous.pwszVal.to_string().ok()
        } else {
            None
        }
    };
    Ok(LinkFacts {
        app_id: app_id.filter(|s| !s.is_empty()),
        icon,
        index,
    })
}

/// Points the shortcut at `lnk` to icon `index` of `icon` and saves it in place. Nothing else of it changes.
#[cfg(windows)]
pub fn set_icon(lnk: &std::path::Path, icon: &std::path::Path, index: i32) -> Result<(), String> {
    let com = Com::open()?;
    let done = set_icon_in(lnk, icon, index);
    drop(com);
    done
}

#[cfg(windows)]
fn set_icon_in(lnk: &std::path::Path, icon: &std::path::Path, index: i32) -> Result<(), String> {
    use windows::Win32::System::Com::STGM_READWRITE;
    use windows::core::PCWSTR;
    let (link, file) = load(lnk, STGM_READWRITE)?;
    let icon = wide(icon);
    // SAFETY: `icon` outlives the call, which copies it; a null name saves to the file `Load` opened.
    unsafe {
        link.SetIconLocation(PCWSTR::from_raw(icon.as_ptr()), index)
            .map_err(|e| format!("SetIconLocation {:#010x}", e.code().0))?;
        file.Save(PCWSTR::null(), true)
            .map_err(|e| format!("Save {:#010x}", e.code().0))
    }
}

/// Tells the shell the shortcut at `lnk` changed (`SHCNE_UPDATEITEM`) and that icons changed
/// (`SHCNE_ASSOCCHANGED`), so the Start menu draws the new icon without an Explorer restart.
#[cfg(windows)]
pub fn notify_changed(lnk: &std::path::Path) {
    use windows::Win32::UI::Shell::{
        SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_FLUSHNOWAIT, SHCNF_IDLIST, SHCNF_PATHW,
        SHChangeNotify,
    };
    let path = wide(lnk);
    // SAFETY: `path` is a NUL-ended UTF-16 path that outlives both calls; the shell copies what it keeps.
    unsafe {
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW | SHCNF_FLUSHNOWAIT,
            Some(path.as_ptr().cast()),
            None,
        );
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST | SHCNF_FLUSHNOWAIT,
            None,
            None,
        );
    }
}

/// Shortcuts are a Windows notion; elsewhere there is no Programs folder.
#[cfg(not(windows))]
pub fn programs_dir() -> Option<std::path::PathBuf> {
    None
}

/// Elsewhere no shortcut is read.
#[cfg(not(windows))]
pub fn read(_lnk: &std::path::Path) -> Result<LinkFacts, String> {
    Err("no shortcuts here".to_string())
}

/// Elsewhere no shortcut is written.
#[cfg(not(windows))]
pub fn set_icon(
    _lnk: &std::path::Path,
    _icon: &std::path::Path,
    _index: i32,
) -> Result<(), String> {
    Err("no shortcuts here".to_string())
}

/// Elsewhere there is no shell to tell.
#[cfg(not(windows))]
pub fn notify_changed(_lnk: &std::path::Path) {}

#[cfg(test)]
mod tests {
    /// Rule 1270 and 1272: this file only reads a PROPVARIANT that COM gave out; it never builds one, so none can point
    /// at Rust memory.
    #[test]
    fn no_propvariant_is_built_here() {
        let src = include_str!("shell_link.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(!body.contains(concat!("PROPVARIANT", " {")));
        assert!(!body.contains(concat!("SetValue", "(")));
    }

    /// Makes a shortcut in a temp folder (tests only; Flare itself never makes one), carrying `app_id`.
    #[cfg(windows)]
    fn make_link(lnk: &std::path::Path, app_id: &str) {
        use crate::window_app_id::{APP_ID_FMTID, APP_ID_PID, lpwstr_value};
        use windows::Win32::Foundation::PROPERTYKEY;
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
        use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::core::{GUID, Interface, PCWSTR};
        let com = super::Com::open().expect("com");
        {
            let target = super::wide(std::path::Path::new(r"C:\Windows\System32\notepad.exe"));
            let path = super::wide(lnk);
            let key = PROPERTYKEY {
                fmtid: GUID::from_u128(APP_ID_FMTID),
                pid: APP_ID_PID,
            };
            // SAFETY: COM calls on this thread's apartment; every buffer outlives its call; the value owns a COM
            // string and its drop frees it once (rule 1270).
            unsafe {
                let link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).expect("link");
                link.SetPath(PCWSTR::from_raw(target.as_ptr()))
                    .expect("path");
                let store: IPropertyStore = link.cast().expect("store");
                let value = lpwstr_value(app_id).expect("value");
                store.SetValue(&key, &value).expect("set");
                store.Commit().expect("commit");
                drop(value);
                let file: IPersistFile = link.cast().expect("file");
                file.Save(PCWSTR::from_raw(path.as_ptr()), true)
                    .expect("save");
            }
        }
        drop(com);
    }

    /// A shortcut in a temp folder reads back its id; its icon is pointed at a file and back at index 0, and nothing
    /// else of it is lost (the id stays). Real COM, so the heap and the apartment are checked too.
    #[cfg(windows)]
    #[test]
    fn a_temp_shortcut_reads_its_id_and_takes_a_new_icon() {
        let dir = tempfile::tempdir().expect("a temp folder");
        let lnk = dir.path().join("Flare Test.lnk");
        make_link(&lnk, "com.flare.terminal.ptest");
        let facts = super::read(&lnk).expect("read");
        assert_eq!(facts.app_id.as_deref(), Some("com.flare.terminal.ptest"));
        let ico = dir.path().join("app-icon-0123.ico");
        std::fs::write(&ico, b"not drawn").expect("write");
        super::set_icon(&lnk, &ico, 0).expect("set icon");
        let after = super::read(&lnk).expect("read again");
        assert_eq!(std::path::PathBuf::from(&after.icon), ico);
        assert_eq!(after.index, 0);
        assert_eq!(after.app_id.as_deref(), Some("com.flare.terminal.ptest"));
        let exe = std::path::Path::new(r"C:\Windows\System32\notepad.exe");
        super::set_icon(&lnk, exe, 0).expect("back");
        let back = super::read(&lnk).expect("read back");
        assert_eq!(std::path::PathBuf::from(&back.icon), exe);
    }

    /// A missing shortcut is an error, not a crash, and COM is balanced (the call can repeat).
    #[cfg(windows)]
    #[test]
    fn a_missing_shortcut_reads_as_an_error() {
        let dir = tempfile::tempdir().expect("a temp folder");
        let lnk = dir.path().join("none.lnk");
        for _ in 0..3 {
            assert!(super::read(&lnk).is_err());
            assert!(super::set_icon(&lnk, &dir.path().join("x.ico"), 0).is_err());
        }
    }
}
