//! This process's memory use, for the overlay and the benchmark.

use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessMemory {
    pub working_set: usize,
    pub private: usize,
}

/// Working set and private bytes of this process, or None when the query fails.
pub fn process_memory() -> Option<ProcessMemory> {
    // SAFETY: `counters` is a zeroed plain-data struct whose `cb` is set to its own size, and the
    // pseudo handle from GetCurrentProcess is always valid.
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            counters.cb,
        );
        if ok == 0 {
            return None;
        }
        Some(ProcessMemory {
            working_set: counters.WorkingSetSize,
            private: counters.PrivateUsage,
        })
    }
}

impl ProcessMemory {
    /// `WS 140.2 MB, private 370.1 MB`.
    pub fn summary(&self) -> String {
        const MB: f64 = 1024.0 * 1024.0;
        format!(
            "WS {:.1} MB, private {:.1} MB",
            self.working_set as f64 / MB,
            self.private as f64 / MB
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_nonzero_memory() {
        let m = process_memory().expect("query works");
        assert!(m.working_set > 0);
        assert!(m.private > 0);
    }

    #[test]
    fn summary_format() {
        let m = ProcessMemory {
            working_set: 1024 * 1024 * 3 / 2,
            private: 1024 * 1024 * 2,
        };
        assert_eq!(m.summary(), "WS 1.5 MB, private 2.0 MB");
    }
}
