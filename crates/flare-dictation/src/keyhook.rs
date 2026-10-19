//! The low-level keyboard hook thread (Task 19). Ported from Prism Terminal's `src/main/hook/appHelper.ps1`
//! at e5848cb (D6: native, on its own thread, not a PowerShell helper).
//!
//! PRIVACY: the hook reads only the keys the owner names (the dictation binding), says only "another key"
//! (never which) while one of them is down, and, only while the session picker is open, the digits 1 to 9
//! and Esc. It swallows nothing else, ever. It is never installed under `--e2e` (the app does not call
//! `KeyHook::start` there; N16 feeds helper lines through the test seam), and no test here installs it.
//!
//! ONE HOOK FOR EVERY COPY OF THE APP: a hook installs only while this process holds the lock file
//! `%TEMP%\Flare-dictation-key.lock` (opened with no sharing; Windows closes it when the process ends).
//! The first copy started owns the key, a later one reports `Waiting` and takes over within the 500 ms poll
//! when the first lets go.

use crate::keyfeed::{code_of_vk, HookEvent};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{GetLastError, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

/// The lock file's name, in the folder `KeyHook::start` is given (`%TEMP%` in the app).
pub const LOCK_NAME: &str = "Flare-dictation-key.lock";

/// An unassigned virtual key: what AutoHotkey sends to keep the Start menu shut.
const MASK_VK: u16 = 0xe8;
const VK_LCTRL: u16 = 0xa2;
const VK_RCTRL: u16 = 0xa3;
const VK_CTRL: u16 = 0x11;
const VK_LWIN: u16 = 0x5b;
const VK_RWIN: u16 = 0x5c;
const VK_ESC: u16 = 0x1b;

/// What the hook remembers between keys: the binding keys held, and the Win and Ctrl rule's flags.
#[derive(Debug, Default)]
pub struct HookState {
    down: HashSet<u16>,
    win_down: bool,
    ctrl_with_win: bool,
}

impl HookState {
    /// Forget everything held (the hook was removed).
    pub fn reset(&mut self) {
        self.down.clear();
        self.win_down = false;
        self.ctrl_with_win = false;
    }
}

fn is_ctrl(vk: u16) -> bool {
    matches!(vk, VK_LCTRL | VK_RCTRL | VK_CTRL)
}

fn is_win(vk: u16) -> bool {
    matches!(vk, VK_LWIN | VK_RWIN)
}

fn is_digit_key(vk: u16) -> bool {
    (0x31..=0x39).contains(&vk) || (0x61..=0x69).contains(&vk)
}

fn ctrl_held(s: &HookState) -> bool {
    s.down.contains(&VK_LCTRL) || s.down.contains(&VK_RCTRL)
}

/// The pure body of Prism Terminal's `Cb`. Returns the event to send (a binding down carries `front: None`;
/// the callback fills in the front window's process), whether to swallow the key, and whether to send the
/// Start-menu mask (a Win up after Ctrl+Win, with masking on).
pub fn decide(
    vk: u16,
    down: bool,
    s: &mut HookState,
    keys: &HashSet<u16>,
    pick: bool,
    mask: bool,
) -> (Option<HookEvent>, bool, bool) {
    if vk == MASK_VK {
        return (None, false, false);
    }
    let mut send_mask = false;
    if is_win(vk) {
        if down && !s.win_down {
            s.win_down = true;
            s.ctrl_with_win = ctrl_held(s);
        } else if !down {
            let used = s.ctrl_with_win || ctrl_held(s);
            s.win_down = false;
            if mask && used {
                send_mask = true;
            }
        }
    } else if is_ctrl(vk) && down && s.win_down {
        s.ctrl_with_win = true;
    }
    if keys.contains(&vk) {
        if down {
            if s.down.insert(vk) {
                if let Some(code) = code_of_vk(vk) {
                    return (
                        Some(HookEvent::Key {
                            down: true,
                            code,
                            front: None,
                        }),
                        false,
                        send_mask,
                    );
                }
            }
        } else {
            s.down.remove(&vk);
            if let Some(code) = code_of_vk(vk) {
                return (
                    Some(HookEvent::Key {
                        down: false,
                        code,
                        front: None,
                    }),
                    false,
                    send_mask,
                );
            }
        }
        return (None, false, send_mask);
    }
    if down {
        if pick && is_digit_key(vk) {
            let n = if vk >= 0x61 { vk - 0x60 } else { vk - 0x30 };
            return (Some(HookEvent::Digit(n as u8)), true, send_mask);
        }
        if pick && vk == VK_ESC {
            return (Some(HookEvent::Esc), true, send_mask);
        }
        if !s.down.is_empty() {
            return (Some(HookEvent::Other), false, send_mask);
        }
    } else if pick && (is_digit_key(vk) || vk == VK_ESC) {
        return (None, true, send_mask);
    }
    (None, false, send_mask)
}

/// Open the lock file with no sharing. Held until the returned file drops (or the process ends).
pub fn take_lock(dir: &Path) -> io::Result<File> {
    let _ = std::fs::create_dir_all(dir);
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(dir.join(LOCK_NAME))
}

/// True when the error means another holder has the lock (a sharing or lock violation).
fn is_busy(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32) | Some(33))
}

/// What the owner sets from outside the hook thread.
#[derive(Default)]
struct Shared {
    keys: Mutex<HashSet<u16>>,
    mask: AtomicBool,
    pick: AtomicBool,
}

/// The state the hook callback reads: one hook per process (the lock file guarantees it).
struct Active {
    shared: Arc<Shared>,
    state: HookState,
    tx: Sender<HookEvent>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

fn lock_active() -> MutexGuard<'static, Option<Active>> {
    ACTIVE.lock().unwrap_or_else(|p| p.into_inner())
}

fn front_pid() -> Option<u32> {
    // SAFETY: both calls take plain values and an out pointer to a local; no handle is kept.
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
        (pid != 0).then_some(pid)
    }
}

fn send_mask_keys() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: MASK_VK,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    // SAFETY: `inputs` is two fully initialised INPUT structs and the size passed is their type's.
    unsafe {
        SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn hook_proc(n: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if n >= 0 && l != 0 {
        // SAFETY: for WH_KEYBOARD_LL with n >= 0, `l` points to a KBDLLHOOKSTRUCT valid for this call.
        let vk = unsafe { (*(l as *const KBDLLHOOKSTRUCT)).vkCode } as u16;
        let msg = w as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let verdict = std::panic::catch_unwind(|| {
            let mut guard = lock_active();
            let Some(a) = guard.as_mut() else {
                return (false, false);
            };
            let keys = a.shared.keys.lock().unwrap_or_else(|p| p.into_inner());
            let (ev, swallow, send_mask) = decide(
                vk,
                down,
                &mut a.state,
                &keys,
                a.shared.pick.load(Ordering::Relaxed),
                a.shared.mask.load(Ordering::Relaxed),
            );
            drop(keys);
            if let Some(mut ev) = ev {
                if let HookEvent::Key {
                    down: true, front, ..
                } = &mut ev
                {
                    *front = front_pid();
                }
                let _ = a.tx.send(ev);
            }
            (swallow, send_mask)
        });
        if let Ok((swallow, send_mask)) = verdict {
            if send_mask {
                send_mask_keys();
            }
            if swallow {
                return 1;
            }
        }
    }
    // SAFETY: the hook handle argument is ignored by the system; the rest are this callback's own arguments.
    unsafe { CallNextHookEx(std::ptr::null_mut(), n, w, l) }
}

/// The running hook: a thread that takes the lock, installs the hook and pumps messages.
pub struct KeyHook {
    thread: Option<JoinHandle<()>>,
    tid: Arc<AtomicU32>,
    wanted: Arc<AtomicBool>,
    shared: Arc<Shared>,
}

impl KeyHook {
    /// Start the thread: take the lock (a 500 ms poll, `Waiting` while another copy holds it), install
    /// `WH_KEYBOARD_LL`, say `Ready` or `Failed`, then pump messages until stopped.
    pub fn start(lock_dir: PathBuf, tx: Sender<HookEvent>) -> Self {
        Self::spawn(lock_dir, tx, true)
    }

    /// Test seam: take the lock like `start` but never install the hook (so a test can watch the wait).
    #[cfg(test)]
    fn start_waiting_only(lock_dir: PathBuf, tx: Sender<HookEvent>) -> Self {
        Self::spawn(lock_dir, tx, false)
    }

    fn spawn(lock_dir: PathBuf, tx: Sender<HookEvent>, install: bool) -> Self {
        let tid = Arc::new(AtomicU32::new(0));
        let wanted = Arc::new(AtomicBool::new(true));
        let shared = Arc::new(Shared::default());
        let t = {
            let (tid, wanted, shared) = (tid.clone(), wanted.clone(), shared.clone());
            std::thread::Builder::new()
                .name("flare-keyhook".into())
                .spawn(move || run(lock_dir, tx, install, &tid, &wanted, shared))
                .ok()
        };
        KeyHook {
            thread: t,
            tid,
            wanted,
            shared,
        }
    }

    /// The binding's virtual keys (either side of a modifier).
    pub fn keys(&self, vks: Vec<u16>) {
        *self.shared.keys.lock().unwrap_or_else(|p| p.into_inner()) = vks.into_iter().collect();
    }

    /// Mask the Start menu on a Win up after Ctrl+Win.
    pub fn mask(&self, on: bool) {
        self.shared.mask.store(on, Ordering::Relaxed);
    }

    /// Report and swallow digits 1 to 9 and Esc (the session picker is open).
    pub fn pick(&self, on: bool) {
        self.shared.pick.store(on, Ordering::Relaxed);
    }

    /// True while the thread runs. A thread that died is the owner's cue to report `Failed` and re-arm.
    pub fn alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Remove the hook, let the lock go and end the thread: `WM_QUIT` is posted until it ends (40 x 50 ms),
    /// because its queue may not exist the first moment.
    pub fn stop(mut self) {
        self.wanted.store(false, Ordering::SeqCst);
        let Some(t) = self.thread.take() else { return };
        for _ in 0..40 {
            if t.is_finished() {
                break;
            }
            let tid = self.tid.load(Ordering::SeqCst);
            if tid != 0 {
                // SAFETY: posting a message to a thread id by value; a stale id only fails.
                unsafe {
                    PostThreadMessageW(tid, WM_QUIT, 0, 0);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if t.is_finished() {
            let _ = t.join();
        }
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        // A hook dropped without `stop` still ends its thread (the thread checks `wanted` while waiting
        // and the quit below ends the pump); it never blocks the dropper.
        self.wanted.store(false, Ordering::SeqCst);
        let tid = self.tid.load(Ordering::SeqCst);
        if tid != 0 {
            // SAFETY: as in `stop`.
            unsafe {
                PostThreadMessageW(tid, WM_QUIT, 0, 0);
            }
        }
    }
}

fn run(
    lock_dir: PathBuf,
    tx: Sender<HookEvent>,
    install: bool,
    tid: &AtomicU32,
    wanted: &AtomicBool,
    shared: Arc<Shared>,
) {
    // A message queue must exist before anyone can post to this thread.
    // SAFETY: PeekMessageW with a zeroed MSG out parameter creates the queue; nothing is removed.
    unsafe {
        let mut m: MSG = std::mem::zeroed();
        PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
        tid.store(GetCurrentThreadId(), Ordering::SeqCst);
    }
    let mut told = false;
    let lock = loop {
        if !wanted.load(Ordering::SeqCst) {
            tid.store(0, Ordering::SeqCst);
            return;
        }
        match take_lock(&lock_dir) {
            Ok(f) => break f,
            Err(e) if is_busy(&e) => {
                if !told {
                    told = true;
                    let _ = tx.send(HookEvent::Waiting);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => {
                let _ = tx.send(HookEvent::Failed);
                tid.store(0, Ordering::SeqCst);
                return;
            }
        }
    };
    if !wanted.load(Ordering::SeqCst) || !install {
        // Stopped while waiting for the lock, or the test seam: nothing to install.
        if install {
            drop(lock);
            tid.store(0, Ordering::SeqCst);
            return;
        }
        while wanted.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(lock);
        tid.store(0, Ordering::SeqCst);
        return;
    }
    *lock_active() = Some(Active {
        shared,
        state: HookState::default(),
        tx: tx.clone(),
    });
    // SAFETY: `hook_proc` has the HOOKPROC signature; the module handle is this process's own.
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(hook_proc),
            GetModuleHandleW(std::ptr::null()),
            0,
        )
    };
    if hook.is_null() {
        // SAFETY: reads the calling thread's last error.
        log::warn!("keyboard hook failed: error {}", unsafe { GetLastError() });
        let _ = tx.send(HookEvent::Failed);
    } else {
        let _ = tx.send(HookEvent::Ready);
        // SAFETY: a plain message pump on this thread; a WM_QUIT (0) or an error (-1) ends it.
        unsafe {
            let mut m: MSG = std::mem::zeroed();
            while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {}
        }
        // SAFETY: `hook` is the live handle installed above.
        unsafe {
            UnhookWindowsHookEx(hook);
        }
    }
    *lock_active() = None;
    drop(lock);
    tid.store(0, Ordering::SeqCst);
    if wanted.load(Ordering::SeqCst) {
        // The pump ended while the owner still wants the hook: say so (it re-arms after 1000 ms).
        let _ = tx.send(HookEvent::Failed);
    }
}

#[cfg(test)]
fn send_key(vk: u16, up: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: one fully initialised INPUT and its type's size.
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const LCTRL: u16 = 0xa2;
    const RALT: u16 = 0xa5;
    const LWIN: u16 = 0x5b;

    fn keys(v: &[u16]) -> HashSet<u16> {
        v.iter().copied().collect()
    }

    fn key_ev(down: bool, code: &str) -> HookEvent {
        HookEvent::Key {
            down,
            code: code.to_string(),
            front: None,
        }
    }

    #[test]
    fn a_binding_down_is_reported_once_and_passes_through() {
        let mut s = HookState::default();
        let k = keys(&[RALT]);
        let (e, swallow, mask) = decide(RALT, true, &mut s, &k, false, false);
        assert_eq!(e, Some(key_ev(true, "AltRight")));
        assert!(!swallow && !mask);
    }

    #[test]
    fn a_held_binding_key_repeating_is_silent() {
        let mut s = HookState::default();
        let k = keys(&[RALT]);
        decide(RALT, true, &mut s, &k, false, false);
        let (e, swallow, _) = decide(RALT, true, &mut s, &k, false, false);
        assert_eq!(e, None);
        assert!(!swallow);
    }

    #[test]
    fn a_binding_up_is_reported_and_forgets_the_key() {
        let mut s = HookState::default();
        let k = keys(&[RALT]);
        decide(RALT, true, &mut s, &k, false, false);
        let (e, swallow, _) = decide(RALT, false, &mut s, &k, false, false);
        assert_eq!(e, Some(key_ev(false, "AltRight")));
        assert!(!swallow);
        // down again is a fresh down
        let (e, _, _) = decide(RALT, true, &mut s, &k, false, false);
        assert_eq!(e, Some(key_ev(true, "AltRight")));
    }

    #[test]
    fn another_key_while_a_binding_key_is_held_says_other_only() {
        let mut s = HookState::default();
        let k = keys(&[RALT]);
        decide(RALT, true, &mut s, &k, false, false);
        let (e, swallow, _) = decide(0x41, true, &mut s, &k, false, false);
        assert_eq!(e, Some(HookEvent::Other));
        assert!(!swallow);
    }

    #[test]
    fn another_key_with_no_binding_key_held_is_not_read() {
        let mut s = HookState::default();
        let k = keys(&[RALT]);
        for vk in [0x41u16, 0x20, 0x0d, 0x1b] {
            let (e, swallow, mask) = decide(vk, true, &mut s, &k, false, false);
            assert_eq!((e, swallow, mask), (None, false, false), "vk {vk:#x}");
        }
    }

    #[test]
    fn an_up_of_another_key_says_nothing() {
        let mut s = HookState::default();
        let k = keys(&[RALT]);
        decide(RALT, true, &mut s, &k, false, false);
        let (e, swallow, _) = decide(0x41, false, &mut s, &k, false, false);
        assert_eq!((e, swallow), (None, false));
    }

    #[test]
    fn digits_one_to_nine_are_swallowed_and_reported_only_while_picking() {
        let k = keys(&[RALT]);
        let mut s = HookState::default();
        let (e, swallow, _) = decide(0x31, true, &mut s, &k, true, false);
        assert_eq!((e, swallow), (Some(HookEvent::Digit(1)), true));
        let (e, swallow, _) = decide(0x39, true, &mut s, &k, true, false);
        assert_eq!((e, swallow), (Some(HookEvent::Digit(9)), true));
        // not picking: untouched
        let (e, swallow, _) = decide(0x31, true, &mut s, &k, false, false);
        assert_eq!((e, swallow), (None, false));
    }

    #[test]
    fn the_numpad_digits_count_as_digits() {
        let k = keys(&[RALT]);
        let mut s = HookState::default();
        let (e, swallow, _) = decide(0x61, true, &mut s, &k, true, false);
        assert_eq!((e, swallow), (Some(HookEvent::Digit(1)), true));
        let (e, swallow, _) = decide(0x69, true, &mut s, &k, true, false);
        assert_eq!((e, swallow), (Some(HookEvent::Digit(9)), true));
    }

    #[test]
    fn zero_and_the_keys_beside_the_digits_are_never_taken() {
        let k = keys(&[RALT]);
        let mut s = HookState::default();
        for vk in [0x30u16, 0x60, 0x6a, 0x40] {
            let (e, swallow, _) = decide(vk, true, &mut s, &k, true, false);
            assert_eq!((e, swallow), (None, false), "vk {vk:#x}");
        }
    }

    #[test]
    fn escape_is_swallowed_and_reported_only_while_picking() {
        let k = keys(&[RALT]);
        let mut s = HookState::default();
        let (e, swallow, _) = decide(0x1b, true, &mut s, &k, true, false);
        assert_eq!((e, swallow), (Some(HookEvent::Esc), true));
        let (e, swallow, _) = decide(0x1b, true, &mut s, &k, false, false);
        assert_eq!((e, swallow), (None, false));
    }

    #[test]
    fn while_picking_the_up_of_a_digit_or_escape_is_swallowed_silently() {
        let k = keys(&[RALT]);
        let mut s = HookState::default();
        for vk in [0x31u16, 0x39, 0x61, 0x69, 0x1b] {
            let (e, swallow, _) = decide(vk, false, &mut s, &k, true, false);
            assert_eq!((e, swallow), (None, true), "vk {vk:#x}");
            let (e, swallow, _) = decide(vk, false, &mut s, &k, false, false);
            assert_eq!((e, swallow), (None, false), "vk {vk:#x}");
        }
    }

    #[test]
    fn a_binding_key_that_is_also_a_picker_key_is_a_binding_key_first() {
        let k = keys(&[0x31]);
        let mut s = HookState::default();
        let (e, swallow, _) = decide(0x31, true, &mut s, &k, true, false);
        assert_eq!((e, swallow), (Some(key_ev(true, "Digit1")), false));
    }

    #[test]
    fn the_mask_key_is_ignored_whatever_state_the_hook_is_in() {
        let k = keys(&[RALT, 0xe8]);
        let mut s = HookState::default();
        decide(RALT, true, &mut s, &k, true, true);
        let (e, swallow, mask) = decide(0xe8, true, &mut s, &k, true, true);
        assert_eq!((e, swallow, mask), (None, false, false));
        let (e, swallow, mask) = decide(0xe8, false, &mut s, &k, true, true);
        assert_eq!((e, swallow, mask), (None, false, false));
    }

    #[test]
    fn ctrl_then_win_then_win_up_sends_the_mask_when_masking_is_on() {
        let k = keys(&[LCTRL, LWIN]);
        let mut s = HookState::default();
        decide(LCTRL, true, &mut s, &k, false, true);
        decide(LWIN, true, &mut s, &k, false, true);
        let (e, swallow, mask) = decide(LWIN, false, &mut s, &k, false, true);
        assert_eq!(e, Some(key_ev(false, "MetaLeft")));
        assert!(!swallow);
        assert!(mask, "Win up after Ctrl+Win must mask the Start menu");
    }

    #[test]
    fn win_then_ctrl_then_win_up_also_masks() {
        let k = keys(&[LCTRL, LWIN]);
        let mut s = HookState::default();
        decide(LWIN, true, &mut s, &k, false, true);
        decide(LCTRL, true, &mut s, &k, false, true);
        decide(LCTRL, false, &mut s, &k, false, true);
        let (_, _, mask) = decide(LWIN, false, &mut s, &k, false, true);
        assert!(mask);
    }

    #[test]
    fn win_alone_never_masks() {
        let k = keys(&[LCTRL, LWIN]);
        let mut s = HookState::default();
        decide(LWIN, true, &mut s, &k, false, true);
        let (_, _, mask) = decide(LWIN, false, &mut s, &k, false, true);
        assert!(!mask);
    }

    #[test]
    fn masking_off_never_masks_even_after_ctrl_win() {
        let k = keys(&[LCTRL, LWIN]);
        let mut s = HookState::default();
        decide(LCTRL, true, &mut s, &k, false, false);
        decide(LWIN, true, &mut s, &k, false, false);
        let (_, _, mask) = decide(LWIN, false, &mut s, &k, false, false);
        assert!(!mask);
    }

    #[test]
    fn a_second_win_use_does_not_inherit_the_first_ctrl() {
        let k = keys(&[LCTRL, LWIN]);
        let mut s = HookState::default();
        decide(LCTRL, true, &mut s, &k, false, true);
        decide(LWIN, true, &mut s, &k, false, true);
        decide(LWIN, false, &mut s, &k, false, true);
        decide(LCTRL, false, &mut s, &k, false, true);
        decide(LWIN, true, &mut s, &k, false, true);
        let (_, _, mask) = decide(LWIN, false, &mut s, &k, false, true);
        assert!(!mask);
    }

    #[test]
    fn a_win_down_repeat_does_not_reset_the_ctrl_flag() {
        let k = keys(&[LCTRL, LWIN]);
        let mut s = HookState::default();
        decide(LCTRL, true, &mut s, &k, false, true);
        decide(LWIN, true, &mut s, &k, false, true);
        decide(LCTRL, false, &mut s, &k, false, true);
        decide(LWIN, true, &mut s, &k, false, true); // auto-repeat
        let (_, _, mask) = decide(LWIN, false, &mut s, &k, false, true);
        assert!(mask);
    }

    #[test]
    fn the_generic_ctrl_key_counts_for_the_win_rule_only_when_it_goes_down_with_win() {
        let k = keys(&[LWIN]);
        let mut s = HookState::default();
        decide(LWIN, true, &mut s, &k, false, true);
        decide(0x11, true, &mut s, &k, false, true);
        let (_, _, mask) = decide(LWIN, false, &mut s, &k, false, true);
        assert!(mask);
    }

    #[test]
    fn the_hook_state_resets_clean() {
        let k = keys(&[RALT]);
        let mut s = HookState::default();
        decide(RALT, true, &mut s, &k, false, false);
        s.reset();
        let (e, _, _) = decide(0x41, true, &mut s, &k, false, false);
        assert_eq!(e, None, "nothing is held after a reset");
    }

    #[test]
    fn the_lock_is_held_by_one_hook_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let first = take_lock(dir.path()).expect("the first takes the lock");
        let second = take_lock(dir.path());
        assert!(
            second.is_err(),
            "the second must not get it while the first holds"
        );
        assert!(is_busy(&second.unwrap_err()));
        drop(first);
        assert!(take_lock(dir.path()).is_ok(), "let go, it is free again");
    }

    #[test]
    fn the_lock_file_is_named_for_flare() {
        assert_eq!(LOCK_NAME, "Flare-dictation-key.lock");
    }

    #[test]
    fn stop_lets_the_lock_go() {
        let dir = tempfile::tempdir().unwrap();
        let held = take_lock(dir.path()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        // Another copy owns the key: this hook waits and never installs anything.
        let hook = KeyHook::start_waiting_only(dir.path().to_path_buf(), tx);
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(HookEvent::Waiting)
        );
        hook.stop();
        drop(held);
        assert!(take_lock(dir.path()).is_ok());
    }

    #[test]
    #[ignore = "by hand: SendInput reaches the desktop in front (docs/dictation-by-hand.md)"]
    fn the_hook_reports_a_binding_key_sent_through_sendinput() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let hook = KeyHook::start(dir.path().to_path_buf(), tx);
        hook.keys(vec![0x91]);
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(HookEvent::Ready)
        );
        send_key(0x91, false);
        send_key(0x91, true);
        let down = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(matches!(down, HookEvent::Key { down: true, .. }));
        hook.stop();
    }

    #[test]
    #[ignore = "by hand: SendInput reaches the desktop in front (docs/dictation-by-hand.md)"]
    fn the_hook_says_other_for_a_key_pressed_while_a_binding_key_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let hook = KeyHook::start(dir.path().to_path_buf(), tx);
        hook.keys(vec![0x91]);
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(HookEvent::Ready)
        );
        send_key(0x91, false);
        send_key(0x41, false);
        send_key(0x41, true);
        send_key(0x91, true);
        let mut seen = Vec::new();
        while let Ok(e) = rx.recv_timeout(std::time::Duration::from_secs(1)) {
            seen.push(e);
        }
        assert!(seen.contains(&HookEvent::Other));
        hook.stop();
    }
}
