//! The taskbar identity of one window (rule 1265): its window-level AppUserModelID.
//!
//! The process-level id (`SetCurrentProcessExplicitAppUserModelID`) matches the Start menu shortcut, so the
//! Windows 11 taskbar draws the shortcut's icon and ignores the window's `WM_SETICON`. A window whose own id has no
//! shortcut shows its `WM_SETICON` icon instead. This writes that id through the window's property store
//! (`SHGetPropertyStoreForWindow`, `PKEY_AppUserModel_ID`). Which id to write is the caller's choice.

/// `PKEY_AppUserModel_ID`: format id {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, property id 5 (propkey.h).
pub const APP_ID_FMTID: u128 = 0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3;
/// The property id of `PKEY_AppUserModel_ID` in its format.
pub const APP_ID_PID: u32 = 5;

/// Sets the window-level AppUserModelID of `hwnd` to `id`, on the calling thread (the UI thread, which owns the
/// window). A null window or an empty id is refused without touching the shell. COM is started for the call in
/// the apartment model and closed again when this call opened it; a thread already in another model keeps it.
#[cfg(windows)]
pub fn set(hwnd: isize, id: &str) -> Result<(), String> {
    use std::mem::ManuallyDrop;
    use windows::Win32::Foundation::{HWND, PROPERTYKEY, RPC_E_CHANGED_MODE};
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
    };
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::{
        IPropertyStore, SHGetPropertyStoreForWindow,
    };
    use windows::core::{GUID, PWSTR};

    if hwnd == 0 || id.is_empty() {
        return Err("no window or no id".to_string());
    }
    // SAFETY: COM init on the calling thread; S_FALSE and a changed mode are fine.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        return Err(format!("CoInitializeEx {:#010x}", hr.0));
    }
    let key = PROPERTYKEY {
        fmtid: GUID::from_u128(APP_ID_FMTID),
        pid: APP_ID_PID,
    };
    // The string stays owned here: SetValue copies the value, so nothing is allocated for COM to free.
    let mut text: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    let value = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    pwszVal: PWSTR(text.as_mut_ptr()),
                },
            }),
        },
    };
    let result = {
        // SAFETY: `hwnd` is a window of this process; the store is a COM interface released when it drops,
        // at the end of this block, before COM is closed.
        let store: windows::core::Result<IPropertyStore> =
            unsafe { SHGetPropertyStoreForWindow(HWND(hwnd as *mut _)) };
        match store {
            // SAFETY: `key` and `value` live across both calls, and `text` outlives `value`.
            Ok(store) => unsafe { store.SetValue(&key, &value).and_then(|()| store.Commit()) }
                .map_err(|e| format!("SetValue or Commit {:#010x}", e.code().0)),
            Err(e) => Err(format!("SHGetPropertyStoreForWindow {:#010x}", e.code().0)),
        }
    };
    drop(text);
    if hr.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above on the same thread.
        unsafe { CoUninitialize() };
    }
    result
}

/// The window id is a Windows notion; elsewhere nothing is written.
#[cfg(not(windows))]
pub fn set(_hwnd: isize, _id: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_pkey_app_user_model_id() {
        assert_eq!(
            format!("{APP_ID_FMTID:032X}"),
            "9F4C28559F794B39A8D0E1D42DE1D5F3"
        );
        assert_eq!(APP_ID_PID, 5);
    }

    #[cfg(windows)]
    #[test]
    fn a_null_window_or_an_empty_id_is_refused_without_the_shell() {
        assert!(set(0, "com.flare.terminal.icon").is_err());
        assert!(set(1, "").is_err());
    }
}
