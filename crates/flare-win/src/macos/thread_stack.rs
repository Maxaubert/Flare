//! The UI-thread stack sampler for the stall watchdog on macOS (rule 1116), the twin of the Windows
//! `.pdata` walk.
//!
//! `raw_frames` suspends the sampled thread (`thread_suspend`), reads its registers
//! (`thread_get_state`, `ARM_THREAD_STATE64`), walks the frame-pointer chain inside the thread's own
//! stack (Apple's arm64 ABI keeps x29 as the frame pointer in every frame) into a fixed buffer, and
//! resumes the thread through a guard (so a panic resumes too). Between suspend and resume nothing
//! allocates, locks, logs or formats: every read is a plain load from the sampled thread's stack,
//! checked against the bounds taken when the sampler was made. Names come from `describe`
//! (`dladdr`), called only after the thread runs again. Apple silicon only; an Intel Mac keeps no
//! sampler (the stall line then names the work in flight alone).
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::ffi::CStr;

/// The most frames a record carries (`flare_diag::stack::MAX_FRAMES`).
pub const MAX_FRAMES: usize = 48;

/// `ARM_THREAD_STATE64` and its count in 32-bit words (`sizeof(arm_thread_state64_t) / 4`).
const ARM_THREAD_STATE64: i32 = 6;
const ARM_THREAD_STATE64_COUNT: u32 = 68;

/// A user-space address on Apple silicon: the low 47 bits (a signed return address keeps its
/// authentication code above them in an arm64e library).
const ADDR_MASK: u64 = (1 << 47) - 1;

/// `arm_thread_state64_t`.
#[repr(C)]
#[derive(Default)]
struct ArmThreadState64 {
    x: [u64; 29],
    fp: u64,
    lr: u64,
    sp: u64,
    pc: u64,
    cpsr: u32,
    pad: u32,
}

unsafe extern "C" {
    fn thread_suspend(thread: u32) -> i32;
    fn thread_resume(thread: u32) -> i32;
    fn thread_get_state(thread: u32, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
}

/// One thread, ready to be sampled from another.
pub struct ThreadSampler {
    /// The thread's Mach port (a send right kept for the process's life).
    port: u32,
    /// The thread's stack: `[low, high)`.
    low: u64,
    high: u64,
}

/// Resumes the thread on drop, so every exit path (a panic included) resumes it.
struct ResumeGuard(u32);

impl Drop for ResumeGuard {
    fn drop(&mut self) {
        // SAFETY: the port names a thread this guard's creator suspended once.
        unsafe { thread_resume(self.0) };
    }
}

impl ThreadSampler {
    /// A sampler for the thread that calls this (sample it from ANOTHER thread, never itself).
    pub fn for_current_thread() -> Result<Self, String> {
        // SAFETY: plain calls on the calling thread; the stack address is the top (highest) address
        // of its stack and the size its length, both from the thread's own pthread record.
        let (port, high, size) = unsafe {
            let me = libc::pthread_self();
            #[allow(deprecated)]
            let port = libc::mach_thread_self();
            (
                port,
                libc::pthread_get_stackaddr_np(me) as u64,
                libc::pthread_get_stacksize_np(me) as u64,
            )
        };
        if port == 0 || high == 0 || size == 0 || size > high {
            return Err("the thread's port or stack could not be read".to_owned());
        }
        Ok(Self {
            port,
            low: high - size,
            high,
        })
    }

    /// The instruction addresses of the thread's stack, innermost first, or `None` when it cannot
    /// be suspended or read (a dead thread).
    pub fn raw_frames(&self) -> Option<([u64; MAX_FRAMES], usize)> {
        let mut state = ArmThreadState64::default();
        let mut count = ARM_THREAD_STATE64_COUNT;
        // SAFETY: the port is a thread of this process. From here to the guard's drop nothing
        // allocates or locks, so the suspended thread can hold any lock without a deadlock.
        if unsafe { thread_suspend(self.port) } != 0 {
            return None;
        }
        let _resume = ResumeGuard(self.port);
        // SAFETY: `state` is an `arm_thread_state64_t` of `count` words.
        let kr = unsafe {
            thread_get_state(
                self.port,
                ARM_THREAD_STATE64,
                (&mut state as *mut ArmThreadState64).cast(),
                &mut count,
            )
        };
        if kr != 0 {
            return None;
        }
        let mut out = [0u64; MAX_FRAMES];
        let n = walk(state.pc, state.fp, self.low, self.high, &mut out, |at| {
            // SAFETY: `walk` asks only for 8-aligned words inside the sampled thread's own stack,
            // which stays mapped while the thread is suspended.
            unsafe { std::ptr::read_volatile(at as *const u64) }
        });
        Some((out, n))
    }
}

/// Pure: the frame-pointer walk. `pc` first, then each frame's return address (the word above its
/// saved frame pointer), while the chain stays inside `[low, high)`, aligned and climbing.
fn walk(
    pc: u64,
    fp: u64,
    low: u64,
    high: u64,
    out: &mut [u64; MAX_FRAMES],
    read: impl Fn(u64) -> u64,
) -> usize {
    let mut n = 0;
    let pc = pc & ADDR_MASK;
    if pc != 0 {
        out[n] = pc;
        n += 1;
    }
    let mut fp = fp;
    while n < MAX_FRAMES && fp.is_multiple_of(8) && fp >= low && fp.saturating_add(16) <= high {
        let next = read(fp);
        let ret = read(fp + 8) & ADDR_MASK;
        if ret == 0 {
            break;
        }
        out[n] = ret;
        n += 1;
        if next <= fp {
            break;
        }
        fp = next;
    }
    n
}

/// `image+0xoffset` for an address inside a loaded image, `0x..` when unknown. Call only after the
/// sampled thread runs again (it allocates).
pub fn describe(addr: u64) -> String {
    // SAFETY: Dl_info is plain data (pointers and nothing else), valid when zeroed.
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // SAFETY: dladdr only reads the loader's tables; `info` is a valid out record.
    let ok = unsafe { libc::dladdr(addr as usize as *const libc::c_void, &mut info) };
    if ok == 0 || info.dli_fname.is_null() {
        return format!("0x{addr:x}");
    }
    // SAFETY: dli_fname is the loader's NUL-terminated path of the image.
    let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    let offset = addr.wrapping_sub(info.dli_fbase as usize as u64);
    format!("{name}+0x{offset:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    #[test]
    fn the_walk_follows_the_chain_inside_the_stack_and_stops_outside_it() {
        // A fake stack: frames at 0x1000 -> 0x1040 -> 0x1080 -> outside.
        let words = |at: u64| match at {
            0x1000 => 0x1040,
            0x1008 => 0xAAA0,
            0x1040 => 0x1080,
            0x1048 => 0xBBB0,
            0x1080 => 0x9000,
            0x1088 => 0xCCC0,
            _ => 0,
        };
        let mut out = [0u64; MAX_FRAMES];
        let n = walk(0x5550, 0x1000, 0x1000, 0x2000, &mut out, words);
        assert_eq!(&out[..n], &[0x5550, 0xAAA0, 0xBBB0, 0xCCC0]);
        // A frame pointer outside the stack, unaligned or going down is never read.
        let n = walk(0x5550, 0x0800, 0x1000, 0x2000, &mut out, |_| panic!("read"));
        assert_eq!(n, 1);
        let n = walk(0x5550, 0x1003, 0x1000, 0x2000, &mut out, |_| panic!("read"));
        assert_eq!(n, 1);
        let n = walk(0, 0x1000, 0x1000, 0x2000, &mut out, |at| {
            if at == 0x1000 { 0x0F00 } else { 0x1234 }
        });
        assert_eq!(&out[..n], &[0x1234]);
        // A signed return address loses its code.
        let n = walk(0, 0x1000, 0x1000, 0x2000, &mut out, |at| {
            if at == 0x1008 {
                0x002A_0000_0000_1234
            } else {
                0
            }
        });
        assert_eq!(&out[..n], &[0x1234]);
    }

    #[inline(never)]
    fn busy_here(stop: &AtomicBool, counter: &AtomicU64) {
        while !stop.load(Ordering::Relaxed) {
            counter.fetch_add(1, Ordering::Relaxed);
            let v: Vec<u8> = vec![7; 1024];
            std::hint::black_box(&v);
        }
    }

    #[test]
    fn a_busy_thread_is_sampled_200_times_named_and_keeps_running() {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let counter = Arc::new(AtomicU64::new(0));
        let (s2, c2) = (stop.clone(), counter.clone());
        let h = std::thread::spawn(move || {
            tx.send(ThreadSampler::for_current_thread().unwrap())
                .unwrap();
            busy_here(&s2, &c2);
        });
        let sampler = rx.recv().unwrap();
        let start = Instant::now();
        let mut deepest = 0;
        let mut last = None;
        for _ in 0..200 {
            let (frames, n) = sampler.raw_frames().expect("a live thread is sampled");
            deepest = deepest.max(n);
            last = Some((frames, n));
        }
        let seen = counter.load(Ordering::Relaxed);
        let t = Instant::now();
        while counter.load(Ordering::Relaxed) <= seen + 10 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(
            counter.load(Ordering::Relaxed) > seen + 10,
            "the thread runs again"
        );
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(
            deepest >= 3,
            "a walk climbs past the innermost frame ({deepest})"
        );
        let (frames, n) = last.unwrap();
        let named: Vec<String> = frames[..n].iter().map(|a| describe(*a)).collect();
        assert!(
            named.iter().any(|f| f.contains("+0x")),
            "frames are named by their image: {named:?}"
        );
    }

    #[test]
    fn a_finished_thread_is_not_sampled() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            tx.send(ThreadSampler::for_current_thread().unwrap())
                .unwrap();
        })
        .join()
        .unwrap();
        let sampler = rx.recv().unwrap();
        assert!(sampler.raw_frames().is_none());
    }
}
