//! SessionEndWatch (Task 16): learns that Windows is shutting down or logging off.
//!
//! No before-quit event arrives at a Windows shutdown or log-off, so the app listens for
//! `WM_ENDSESSION` on a message-only window of its own thread and flushes its state there.

use std::cell::RefCell;
use std::sync::Once;
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, WM_CLOSE, WM_DESTROY,
    WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW,
};

/// The window class, registered once per process.
const CLASS_NAME: &str = "Flare.SessionEnd";

static REGISTER: Once = Once::new();

thread_local! {
    /// The callback of the watch whose window lives on this thread.
    static ON_END: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // Never a veto: Flare has nothing that must stop a shutdown.
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0 {
                ON_END.with(|cell| {
                    if let Some(f) = cell.borrow().as_ref() {
                        // A panic must not unwind through the Win32 frame.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
                    }
                });
            }
            0
        }
        WM_CLOSE => {
            // SAFETY: hwnd is this window, called on its own thread.
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            // SAFETY: plain call on the window's own thread.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: forwarding the unhandled message with the arguments we received.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// A message-only window on its own thread. `WM_QUERYENDSESSION` is answered 1 (never a veto);
/// `WM_ENDSESSION` with `wParam != 0` runs `on_end` on that thread before the message returns;
/// `wParam == 0` (cancelled) runs nothing.
pub struct SessionEndWatch {
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl SessionEndWatch {
    pub fn start(on_end: impl Fn() + Send + 'static) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<Result<isize, String>>();
        let thread = std::thread::Builder::new()
            .name("flare-session-end".into())
            .spawn(move || {
                ON_END.with(|cell| *cell.borrow_mut() = Some(Box::new(on_end)));
                let class = wide(CLASS_NAME);
                // SAFETY: null asks for the current module's handle.
                let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
                REGISTER.call_once(|| {
                    let wc = WNDCLASSW {
                        style: 0,
                        lpfnWndProc: Some(wnd_proc),
                        cbClsExtra: 0,
                        cbWndExtra: 0,
                        hInstance: hinstance,
                        hIcon: std::ptr::null_mut(),
                        hCursor: std::ptr::null_mut(),
                        hbrBackground: std::ptr::null_mut(),
                        lpszMenuName: std::ptr::null(),
                        lpszClassName: class.as_ptr(),
                    };
                    // SAFETY: wc points at live data; class outlives the call.
                    unsafe { RegisterClassW(&wc) };
                });
                // SAFETY: the class is registered; the strings outlive the call.
                let hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        class.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        HWND_MESSAGE,
                        std::ptr::null_mut(),
                        hinstance,
                        std::ptr::null(),
                    )
                };
                if hwnd.is_null() {
                    let _ = tx.send(Err("could not create the session-end window".into()));
                    return;
                }
                let _ = tx.send(Ok(hwnd as isize));
                // SAFETY: MSG is plain data; the loop runs on the window's own thread.
                unsafe {
                    let mut msg: MSG = std::mem::zeroed();
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                ON_END.with(|cell| *cell.borrow_mut() = None);
            })
            .map_err(|e| e.to_string())?;
        match rx.recv() {
            // sync-ok: startup, waits for the watcher thread to report
            Ok(Ok(hwnd)) => Ok(Self {
                hwnd,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join(); // sync-ok: startup failure, the watcher ends at once
                Err(e)
            }
            Err(_) => Err("the session-end thread died".into()),
        }
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// Posts `WM_CLOSE`; the window destroys itself and the thread ends. Joins.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(t) = self.thread.take() {
            // SAFETY: posting to a window of ours; a dead window just fails.
            unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
            let _ = t.join(); // sync-ok: quit, the watcher ends after WM_CLOSE
        }
    }
}

impl Drop for SessionEndWatch {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, SendMessageW};

    fn counted() -> (SessionEndWatch, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let w = SessionEndWatch::start(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        })
        .expect("start");
        (w, n)
    }

    #[test]
    fn a_real_end_session_message_runs_the_callback() {
        let (w, n) = counted();
        // SAFETY: the window is alive; SendMessageW is synchronous.
        unsafe { SendMessageW(w.hwnd() as HWND, WM_ENDSESSION, 1, 0) };
        assert_eq!(n.load(Ordering::SeqCst), 1);
        w.stop();
    }

    #[test]
    fn a_cancelled_end_session_runs_nothing() {
        let (w, n) = counted();
        // SAFETY: the window is alive; SendMessageW is synchronous.
        unsafe { SendMessageW(w.hwnd() as HWND, WM_ENDSESSION, 0, 0) };
        assert_eq!(n.load(Ordering::SeqCst), 0);
        w.stop();
    }

    #[test]
    fn query_end_session_is_never_a_veto() {
        let (w, n) = counted();
        // SAFETY: the window is alive; SendMessageW is synchronous.
        let r = unsafe { SendMessageW(w.hwnd() as HWND, WM_QUERYENDSESSION, 0, 0) };
        assert_eq!(r, 1);
        assert_eq!(n.load(Ordering::SeqCst), 0);
        w.stop();
    }

    #[test]
    fn stop_ends_the_thread() {
        let (w, _n) = counted();
        let hwnd = w.hwnd();
        let t = Instant::now();
        w.stop();
        assert!(t.elapsed() < Duration::from_secs(2));
        // SAFETY: IsWindow accepts any handle value.
        assert_eq!(unsafe { IsWindow(hwnd as HWND) }, 0);
    }

    #[test]
    fn two_watches_coexist() {
        let (a, na) = counted();
        let (b, nb) = counted();
        // SAFETY: both windows are alive; SendMessageW is synchronous.
        unsafe { SendMessageW(b.hwnd() as HWND, WM_ENDSESSION, 1, 0) };
        assert_eq!(na.load(Ordering::SeqCst), 0);
        assert_eq!(nb.load(Ordering::SeqCst), 1);
        // SAFETY: as above.
        unsafe { SendMessageW(a.hwnd() as HWND, WM_ENDSESSION, 1, 0) };
        assert_eq!(na.load(Ordering::SeqCst), 1);
        assert_eq!(nb.load(Ordering::SeqCst), 1);
        a.stop();
        b.stop();
    }
}
