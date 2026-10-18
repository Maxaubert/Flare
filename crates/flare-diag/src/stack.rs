//! M9 Task 22 (optional): the UI-thread stack sampler's record format.
//!
//! `flare-diag` does not depend on `flare-win`, so the Win32 side is reached through
//! [`FrameSource`]; the glue (Task 31) implements it over `flare_win::thread_stack::ThreadSampler`
//! and `describe`. Naming happens here, after the thread has been resumed.

use crate::stall::StackSampler;

/// The most frames a record carries.
pub const MAX_FRAMES: usize = 48;

/// Where raw stack addresses come from (the suspended-thread walk) and how they get a name.
pub trait FrameSource: Send + Sync {
    /// Innermost first; `None` when the thread cannot be sampled.
    fn raw_frames(&self) -> Option<([u64; MAX_FRAMES], usize)>;
    /// `exe+0xoffset` for one address; called only after the thread runs again.
    fn describe(&self, addr: u64) -> String;
}

/// A [`StackSampler`] over a [`FrameSource`].
pub struct Win32Sampler<S: FrameSource>(pub S);

impl<S: FrameSource> StackSampler for Win32Sampler<S> {
    fn sample(&self) -> Option<Vec<String>> {
        let (frames, n) = self.0.raw_frames()?;
        if n == 0 {
            return None;
        }
        Some(frames[..n].iter().map(|a| self.0.describe(*a)).collect())
    }
}

/// The safe empty sampler: the work-table names only (the O16 minimum).
pub struct NoSampler;

impl StackSampler for NoSampler {
    fn sample(&self) -> Option<Vec<String>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Option<([u64; MAX_FRAMES], usize)>);
    impl FrameSource for Fake {
        fn raw_frames(&self) -> Option<([u64; MAX_FRAMES], usize)> {
            self.0
        }
        fn describe(&self, addr: u64) -> String {
            format!("Flare.exe+0x{addr:x}")
        }
    }

    #[test]
    fn frames_are_named_innermost_first() {
        let mut f = [0u64; MAX_FRAMES];
        f[0] = 0x10;
        f[1] = 0x20;
        let s = Win32Sampler(Fake(Some((f, 2))));
        assert_eq!(
            s.sample().unwrap(),
            vec!["Flare.exe+0x10", "Flare.exe+0x20"]
        );
    }

    #[test]
    fn no_frames_means_none() {
        assert!(Win32Sampler(Fake(None)).sample().is_none());
        assert!(
            Win32Sampler(Fake(Some(([0; MAX_FRAMES], 0))))
                .sample()
                .is_none()
        );
        assert!(NoSampler.sample().is_none());
    }
}
