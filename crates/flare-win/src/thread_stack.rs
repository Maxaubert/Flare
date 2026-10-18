//! M9 Task 22 (optional): the UI-thread stack sampler for the stall watchdog.
//!
//! `raw_frames` suspends the sampled thread, reads its context, walks the stack with the
//! `.pdata` unwinder (`RtlLookupFunctionEntry` + `RtlVirtualUnwind`: no dbghelp, no symbols, no
//! allocation) into a fixed buffer, and resumes the thread through a guard (so a panic resumes
//! too). Between suspend and resume nothing allocates, locks, logs or formats. Names come from
//! `describe`, called only after the thread runs again.
#![cfg(target_arch = "x86_64")]

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
    THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};

/// `CONTEXT_FULL` for x86-64 (`CONTEXT_AMD64 | CONTROL | INTEGER | FLOATING_POINT`).
const CONTEXT_FULL_AMD64: u32 = 0x10000B;
/// `UNW_FLAG_NHANDLER`: unwind without looking for handlers.
const UNW_FLAG_NHANDLER: u32 = 0;

/// The x86-64 `CONTEXT` record (1232 bytes, 16-aligned), declared here because `windows-sys`
/// only exposes it (and `GetThreadContext`, `RtlVirtualUnwind`) behind `Win32_System_Kernel`,
/// which the workspace does not enable. Only the fields the walk reads have accessors.
#[repr(C, align(16))]
struct Context {
    raw: [u8; 1232],
}

impl Context {
    const FLAGS_AT: usize = 48;
    const RSP_AT: usize = 152;
    const RIP_AT: usize = 248;

    fn zeroed() -> Self {
        Self { raw: [0; 1232] }
    }
    fn u64_at(&self, at: usize) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.raw[at..at + 8]);
        u64::from_le_bytes(b)
    }
    fn set_flags(&mut self, flags: u32) {
        self.raw[Self::FLAGS_AT..Self::FLAGS_AT + 4].copy_from_slice(&flags.to_le_bytes());
    }
    fn rsp(&self) -> u64 {
        self.u64_at(Self::RSP_AT)
    }
    fn rip(&self) -> u64 {
        self.u64_at(Self::RIP_AT)
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetThreadContext(thread: HANDLE, context: *mut Context) -> i32;
    fn RtlLookupFunctionEntry(
        control_pc: u64,
        image_base: *mut u64,
        history: *mut core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
    #[allow(clippy::too_many_arguments)]
    fn RtlVirtualUnwind(
        handler_type: u32,
        image_base: u64,
        control_pc: u64,
        function_entry: *const core::ffi::c_void,
        context: *mut Context,
        handler_data: *mut *mut core::ffi::c_void,
        establisher_frame: *mut u64,
        context_pointers: *mut core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
}

/// The most frames one sample holds.
pub const MAX_FRAMES: usize = 48;

/// An open handle to one thread, ready to be sampled from another.
pub struct ThreadSampler {
    handle: HANDLE,
}

// SAFETY: a thread HANDLE is a process-wide kernel handle; the calls used are thread-safe.
unsafe impl Send for ThreadSampler {}
// SAFETY: as above; the sampler holds no state besides the handle.
unsafe impl Sync for ThreadSampler {}

/// Resumes the thread on drop, so every exit path (a panic included) resumes it.
struct ResumeGuard(HANDLE);

impl Drop for ResumeGuard {
    fn drop(&mut self) {
        // SAFETY: the handle is open and was suspended by this guard's creator.
        unsafe { ResumeThread(self.0) };
    }
}

impl ThreadSampler {
    /// A sampler for the thread that calls this (sample it from ANOTHER thread, never itself).
    pub fn for_current_thread() -> Result<Self, String> {
        // SAFETY: plain Win32 calls with valid arguments.
        let handle = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                GetCurrentThreadId(),
            )
        };
        if handle.is_null() {
            return Err("OpenThread failed".to_owned());
        }
        Ok(Self { handle })
    }

    /// A sampler over an already open handle (the sampler closes it). For tests of the failure path.
    #[doc(hidden)]
    pub fn from_raw_handle(handle: HANDLE) -> Self {
        Self { handle }
    }

    /// The instruction addresses of the thread's stack, innermost first, or `None` when it
    /// cannot be suspended or read (a dead thread, a handle without the right).
    pub fn raw_frames(&self) -> Option<([u64; MAX_FRAMES], usize)> {
        let mut frames = [0u64; MAX_FRAMES];
        let mut n = 0usize;
        // Built BEFORE the suspend so nothing is set up inside it.
        let mut ctx = Context::zeroed();
        ctx.set_flags(CONTEXT_FULL_AMD64);
        // SAFETY: the handle is open. u32::MAX is the documented failure value.
        if unsafe { SuspendThread(self.handle) } == u32::MAX {
            return None;
        }
        let _resume = ResumeGuard(self.handle);
        // ---- nothing below allocates, locks, logs or formats until the guard drops ----
        // SAFETY: ctx is a valid CONTEXT for the thread's architecture.
        if unsafe { GetThreadContext(self.handle, &mut ctx) } == 0 {
            return None;
        }
        while n < MAX_FRAMES {
            let rip = ctx.rip();
            if rip == 0 {
                break;
            }
            frames[n] = rip;
            n += 1;
            let mut base = 0u64;
            // SAFETY: a null history table is allowed; the call only reads the .pdata tables.
            let entry = unsafe { RtlLookupFunctionEntry(rip, &mut base, null_mut()) };
            if entry.is_null() {
                break;
            }
            let before = ctx.rsp();
            let mut handler_data = null_mut();
            let mut establisher = 0u64;
            // SAFETY: entry and base came from the lookup above; ctx belongs to the suspended thread.
            unsafe {
                RtlVirtualUnwind(
                    UNW_FLAG_NHANDLER,
                    base,
                    rip,
                    entry,
                    &mut ctx,
                    &mut handler_data,
                    &mut establisher,
                    null_mut(),
                );
            }
            if ctx.rsp() <= before {
                break;
            }
        }
        Some((frames, n))
    }
}

impl Drop for ThreadSampler {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value.
        unsafe { CloseHandle(self.handle) };
    }
}

/// `exe+0xoffset` for an address inside a loaded module, `0x..` when unknown. Call only after the
/// sampled thread runs again (it allocates).
pub fn describe(addr: u64) -> String {
    let mut module: HMODULE = null_mut();
    // SAFETY: FROM_ADDRESS treats the pointer as an address; UNCHANGED_REFCOUNT takes no reference.
    let ok = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            addr as usize as *const u16,
            &mut module,
        )
    };
    if ok == 0 || module.is_null() {
        return format!("0x{addr:x}");
    }
    let mut buf = [0u16; 520];
    // SAFETY: buf is valid for its length.
    let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if len == 0 || len >= buf.len() {
        return format!("0x{addr:x}");
    }
    let path = String::from_utf16_lossy(&buf[..len]);
    let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
    let offset = addr.wrapping_sub(module as usize as u64);
    format!("{name}+0x{offset:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    type Spawned = (ThreadSampler, Arc<AtomicBool>, std::thread::JoinHandle<()>);

    fn spawn_sampled<F: FnOnce(Arc<AtomicBool>) + Send + 'static>(body: F) -> Spawned {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let h = std::thread::spawn(move || {
            tx.send(ThreadSampler::for_current_thread().unwrap())
                .unwrap();
            body(s2);
        });
        (rx.recv().unwrap(), stop, h)
    }

    #[test]
    fn a_thread_that_allocates_in_a_loop_is_sampled_200_times_without_a_deadlock() {
        let start = Instant::now();
        let (sampler, stop, h) = spawn_sampled(|stop| {
            while !stop.load(Ordering::Relaxed) {
                let v: Vec<u8> = vec![7; 4096];
                std::hint::black_box(&v);
            }
        });
        let mut got = 0;
        for _ in 0..200 {
            if sampler.raw_frames().is_some() {
                got += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert_eq!(got, 200);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_thread_blocked_on_a_mutex_is_sampled_and_resumed() {
        let lock = Arc::new(Mutex::new(()));
        let counter = Arc::new(AtomicU64::new(0));
        let guard = lock.lock().unwrap();
        let (l2, c2) = (lock.clone(), counter.clone());
        let (sampler, stop, h) = spawn_sampled(move |stop| {
            let _g = l2.lock().unwrap();
            while !stop.load(Ordering::Relaxed) {
                c2.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        assert!(sampler.raw_frames().is_some());
        drop(guard);
        let seen = counter.load(Ordering::Relaxed);
        let t = Instant::now();
        while counter.load(Ordering::Relaxed) <= seen + 2 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            counter.load(Ordering::Relaxed) > seen + 2,
            "the thread did not run on"
        );
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn frames_name_the_exe() {
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().to_string();
        let sampler = ThreadSampler::for_current_thread().unwrap();
        let (tx, rx) = mpsc::channel::<()>();
        // The sampled thread (this one) blocks on the channel while a helper samples it.
        let helper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let r = sampler.raw_frames();
            tx.send(()).unwrap();
            r.map(|(f, n)| f[..n].iter().map(|a| describe(*a)).collect::<Vec<_>>())
        });
        rx.recv().unwrap();
        let described = helper.join().unwrap().expect("a sample");
        assert!(
            described.iter().any(|d| d.starts_with(&name)),
            "no frame names {name}: {described:?}"
        );
    }

    #[test]
    fn no_frames_from_a_dead_thread() {
        let (sampler, stop, h) = spawn_sampled(|_| {});
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(sampler.raw_frames().is_none());
    }

    #[test]
    fn suspend_failure_returns_none() {
        // SAFETY: opens a handle with query rights only, so SuspendThread must be refused.
        let h = unsafe { OpenThread(THREAD_QUERY_INFORMATION, 0, GetCurrentThreadId()) };
        assert!(!h.is_null());
        let sampler = ThreadSampler::from_raw_handle(h);
        assert!(sampler.raw_frames().is_none());
    }

    #[test]
    fn describe_an_unknown_address_is_hex() {
        assert_eq!(describe(0x10), "0x10");
    }
}
