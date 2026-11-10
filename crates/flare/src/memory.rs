//! This process's memory use, for the overlay and the benchmark.

#[cfg(windows)]
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::GetCurrentProcess;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessMemory {
    pub working_set: usize,
    pub private: usize,
}

/// Working set and private bytes of this process, or None when the query fails.
#[cfg(windows)]
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

/// The page addresses of the resident pages no other process shares.
#[cfg(windows)]
fn private_pages() -> Option<Vec<usize>> {
    use windows_sys::Win32::System::ProcessStatus::QueryWorkingSet;
    // The buffer is a count followed by one word per resident page: the page number in the high
    // bits (above 12), the Shared flag at bit 8.
    let mut words: Vec<usize> = vec![0; 1 << 15];
    for _ in 0..6 {
        // SAFETY: `words` is writable for `len * size_of::<usize>()` bytes and the pseudo handle
        // from GetCurrentProcess is always valid.
        let ok = unsafe {
            QueryWorkingSet(
                GetCurrentProcess(),
                words.as_mut_ptr().cast(),
                (words.len() * size_of::<usize>()) as u32,
            )
        };
        if ok != 0 {
            let n = words[0].min(words.len() - 1);
            return Some(
                words[1..=n]
                    .iter()
                    .filter(|w| *w & (1 << 8) == 0)
                    .map(|w| w & !0xfff)
                    .collect(),
            );
        }
        let n = words[0];
        words = vec![0; (n + n / 4 + 256).max(words.len() * 2)];
    }
    None
}

/// The private working set in bytes: the resident pages that no other process shares. This is the
/// number the memory bar (B2) reads (Task Manager's "Memory (private working set)").
#[cfg(windows)]
pub fn private_working_set() -> Option<usize> {
    private_pages().map(|p| p.len() * 4096)
}

/// The private working set by kind of memory, and the process heap, for the breakdown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Regions {
    /// Private pages inside mapped images (data and relocated pages of the exe and each dll), by
    /// file name, largest first.
    pub image: Vec<(String, usize)>,
    /// Private pages of mapped (section) memory.
    pub mapped: usize,
    /// Private pages of plain allocations (heaps, stacks, driver memory).
    pub private: usize,
    /// Bytes the process heaps hold allocated and committed (the Rust allocator lives here).
    pub heap_allocated: usize,
    pub heap_committed: usize,
    /// The largest private allocations by resident bytes: (base address, bytes, is a process heap).
    pub largest_private: Vec<(usize, usize, bool)>,
}

/// Walks this process's private pages (UI thread, a few ms; only when the overlay or a probe asks).
#[cfg(windows)]
pub fn regions() -> Option<Regions> {
    use windows_sys::Win32::System::Memory::{
        MEM_IMAGE, MEM_MAPPED, MEMORY_BASIC_INFORMATION, VirtualQuery,
    };
    use windows_sys::Win32::System::ProcessStatus::GetMappedFileNameW;
    let mut pages = private_pages()?;
    pages.sort_unstable();
    let mut out = Regions::default();
    let mut images: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut names: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut allocs: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut i = 0;
    while i < pages.len() {
        // SAFETY: `info` is a zeroed plain-data struct and VirtualQuery only writes into it.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the address is a resident page of this process and info is the struct's size.
        let got = unsafe {
            VirtualQuery(
                pages[i] as *const _,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if got == 0 {
            i += 1;
            continue;
        }
        let end = info.BaseAddress as usize + info.RegionSize;
        let start_i = i;
        while i < pages.len() && pages[i] < end {
            i += 1;
        }
        let bytes = (i - start_i).max(1) * 4096;
        if info.Type == MEM_IMAGE {
            let base = info.AllocationBase as usize;
            let name = names.entry(base).or_insert_with(|| {
                let mut buf = [0u16; 520];
                // SAFETY: `buf` is writable for its length and the handle is the pseudo handle.
                let n = unsafe {
                    GetMappedFileNameW(
                        GetCurrentProcess(),
                        base as *const _,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                    )
                } as usize;
                let full = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
                full.rsplit('\\').next().unwrap_or("?").to_string()
            });
            *images.entry(name.clone()).or_default() += bytes;
        } else if info.Type == MEM_MAPPED {
            out.mapped += bytes;
        } else {
            out.private += bytes;
            *allocs.entry(info.AllocationBase as usize).or_default() += bytes;
        }
    }
    let mut image: Vec<(String, usize)> = images.into_iter().collect();
    image.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    out.image = image;
    let (a, c, handles) = heaps();
    out.heap_allocated = a;
    out.heap_committed = c;
    let mut big: Vec<(usize, usize, bool)> = allocs
        .into_iter()
        .map(|(base, bytes)| (base, bytes, handles.iter().any(|h| h & !0xffff == base)))
        .collect();
    big.sort_by_key(|(_, b, _)| std::cmp::Reverse(*b));
    big.truncate(10);
    out.largest_private = big;
    Some(out)
}

/// (allocated, committed) bytes summed over this process's heaps, and their handles.
#[cfg(windows)]
fn heaps() -> (usize, usize, Vec<usize>) {
    use windows_sys::Win32::System::Memory::{GetProcessHeaps, HEAP_SUMMARY, HeapSummary};
    let mut handles = vec![std::ptr::null_mut(); 64];
    // SAFETY: `handles` has room for the 64 handles asked for.
    let n = unsafe { GetProcessHeaps(handles.len() as u32, handles.as_mut_ptr()) } as usize;
    let (mut allocated, mut committed) = (0usize, 0usize);
    let mut out = Vec::new();
    for h in handles.iter().take(n.min(64)) {
        // SAFETY: a zeroed HEAP_SUMMARY with `cb` set is what HeapSummary expects; `h` came from
        // GetProcessHeaps.
        unsafe {
            let mut s: HEAP_SUMMARY = std::mem::zeroed();
            s.cb = size_of::<HEAP_SUMMARY>() as u32;
            if HeapSummary(*h, 0, &mut s) != 0 {
                allocated += s.cbAllocated;
                committed += s.cbCommitted;
                out.push(*h as usize);
            }
        }
    }
    (allocated, committed, out)
}
/// Where the private working set goes, counted from the owners (M10 Task 66, step 1). A part the
/// app cannot count (the graphics driver, Slint, the allocator) shows up as `unaccounted`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Breakdown {
    /// The glyph atlas, CPU side (slot table, uploads waiting).
    pub atlas_cpu: usize,
    /// The glyph atlas texture (GPU memory; mostly not in the working set).
    pub atlas_gpu: usize,
    /// The grid texture the GPU painter draws into.
    pub grid_texture: usize,
    /// The CPU frame buffer (the software painter's image, plus the grid copy it is drawn from).
    pub cpu_frame_buffer: usize,
    /// The rasteriser's bitmap cache.
    pub glyph_cache: usize,
    /// Each tab's scrollback.
    pub scrollback: Vec<(String, usize)>,
    /// The face files the font database holds (memory mapped, so mostly not private).
    pub font_database: usize,
    /// The help search index.
    pub help_index: usize,
    /// The Settings catalogue (static data: counted, but not private).
    pub settings_catalogue: usize,
    /// The process's private working set.
    pub private_ws: usize,
    /// The private working set by kind of memory.
    pub regions: Option<Regions>,
}

impl Breakdown {
    pub fn scrollback_total(&self) -> usize {
        self.scrollback.iter().map(|(_, b)| b).sum()
    }

    /// Everything counted. The GPU-side textures and the memory-mapped faces and the static
    /// catalogue are listed but are not private working set, so they are left out of the sum.
    pub fn accounted(&self) -> usize {
        self.atlas_cpu
            + self.cpu_frame_buffer
            + self.glyph_cache
            + self.scrollback_total()
            + self.help_index
    }

    pub fn unaccounted(&self) -> usize {
        self.private_ws.saturating_sub(self.accounted())
    }

    fn mb(b: usize) -> f64 {
        b as f64 / (1024.0 * 1024.0)
    }

    /// One line for the F12 overlay.
    pub fn line(&self) -> String {
        let heap = self.regions.as_ref().map_or_else(String::new, |r| {
            format!(
                ", heap {:.1} MB, dll data {:.1} MB",
                Self::mb(r.heap_allocated),
                Self::mb(r.image.iter().map(|(_, b)| b).sum::<usize>())
            )
        });
        format!(
            "atlas {:.1}+{:.1} MB, grid tex {:.1} MB, cpu frame {:.1} MB, glyphs {:.1} MB, scrollback {:.1} MB, fonts {:.1} MB, help {:.1} MB, unaccounted {:.1} MB{heap}",
            Self::mb(self.atlas_cpu),
            Self::mb(self.atlas_gpu),
            Self::mb(self.grid_texture),
            Self::mb(self.cpu_frame_buffer),
            Self::mb(self.glyph_cache),
            Self::mb(self.scrollback_total()),
            Self::mb(self.font_database),
            Self::mb(self.help_index),
            Self::mb(self.unaccounted()),
        )
    }

    /// The probe's answer (`memory.breakdown`): every part in bytes.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "atlas_cpu": self.atlas_cpu,
            "atlas_gpu": self.atlas_gpu,
            "grid_texture": self.grid_texture,
            "cpu_frame_buffer": self.cpu_frame_buffer,
            "glyph_cache": self.glyph_cache,
            "scrollback": self.scrollback.iter().map(|(id, b)| serde_json::json!({"tab": id, "bytes": b})).collect::<Vec<_>>(),
            "scrollback_total": self.scrollback_total(),
            "font_database": self.font_database,
            "help_index": self.help_index,
            "settings_catalogue": self.settings_catalogue,
            "private_ws": self.private_ws,
            "regions": self.regions.as_ref().map(|r| serde_json::json!({
                "image_total": r.image.iter().map(|(_, b)| b).sum::<usize>(),
                "image": r.image.iter().take(12).map(|(n, b)| serde_json::json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
                "mapped": r.mapped,
                "private": r.private,
                "heap_allocated": r.heap_allocated,
                "heap_committed": r.heap_committed,
                "largest_private": r.largest_private.iter().map(|(a, b, h)| serde_json::json!({"base": format!("{a:#x}"), "bytes": b, "heap": h})).collect::<Vec<_>>(),
            })),
            "accounted": self.accounted(),
            "unaccounted": self.unaccounted(),
        })
    }
}

type BreakdownSource = std::rc::Rc<dyn Fn() -> Breakdown>;

thread_local! {
    static SOURCE: std::cell::RefCell<Option<BreakdownSource>> = const { std::cell::RefCell::new(None) };
}

/// The window glue hands over the function that gathers the parts (UI thread only).
pub fn set_breakdown_source(f: BreakdownSource) {
    SOURCE.with(|s| *s.borrow_mut() = Some(f));
}

/// The breakdown now, or None before the window glue has set a source.
pub fn breakdown() -> Option<Breakdown> {
    let f = SOURCE.with(|s| s.borrow().clone())?;
    let mut b = f();
    b.regions = regions();
    b.private_ws = b.regions.as_ref().map_or_else(
        || private_working_set().unwrap_or(0),
        |r| r.image.iter().map(|(_, b)| b).sum::<usize>() + r.mapped + r.private,
    );
    Some(b)
}

/// macOS: phase 3 of the port plan (mach task_info). None until then: the query fails.
#[cfg(not(windows))]
pub fn process_memory() -> Option<ProcessMemory> {
    None
}

/// macOS: phase 3 of the port plan. None until then.
#[cfg(not(windows))]
pub fn private_working_set() -> Option<usize> {
    None
}

/// macOS: phase 3 of the port plan. None until then.
#[cfg(not(windows))]
pub fn regions() -> Option<Regions> {
    None
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

    #[cfg(windows)]
    #[test]
    fn reports_nonzero_memory() {
        let m = process_memory().expect("query works");
        assert!(m.working_set > 0);
        assert!(m.private > 0);
    }

    #[cfg(windows)]
    #[test]
    fn the_private_working_set_is_resident_and_below_the_working_set_plus_slack() {
        let private = private_working_set().expect("the page walk works");
        let m = process_memory().expect("query works");
        assert!(private > 0);
        // Private pages are part of the working set; the two reads are a moment apart.
        assert!(
            private <= m.working_set + (8 << 20),
            "{private} vs {}",
            m.working_set
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_regions_add_up_to_the_private_working_set() {
        let r = regions().expect("regions");
        let sum = r.image.iter().map(|(_, b)| b).sum::<usize>() + r.mapped + r.private;
        let private = private_working_set().expect("private");
        // The two walks are a moment apart and allocate between them (the page list, a name map). In a test process the
        // private set is only about 2 MB, so a tenth of it is under what that moves; the slack is a tenth of a bigger
        // set, and at least 1 MiB.
        let slack = (private / 10).max(1 << 20);
        let (lo, hi) = (private.saturating_sub(slack), private + slack);
        assert!(
            (lo..=hi).contains(&sum),
            "regions {sum} vs private {private}"
        );
        assert!(r.heap_committed >= r.heap_allocated);
        assert!(r.heap_allocated > 0);
    }

    #[test]
    fn unaccounted_is_the_private_set_minus_the_counted_parts() {
        let b = Breakdown {
            atlas_cpu: 100,
            cpu_frame_buffer: 200,
            glyph_cache: 300,
            scrollback: vec![("a".into(), 400), ("b".into(), 500)],
            help_index: 600,
            // Listed but not counted in the sum: GPU textures, mapped faces, static data.
            atlas_gpu: 9_000_000,
            grid_texture: 9_000_000,
            font_database: 9_000_000,
            settings_catalogue: 9_000_000,
            private_ws: 5000,
            regions: None,
        };
        assert_eq!(b.scrollback_total(), 900);
        assert_eq!(b.accounted(), 100 + 200 + 300 + 900 + 600);
        assert_eq!(b.unaccounted(), 5000 - 2100);
        // Never negative.
        let small = Breakdown { private_ws: 1, ..b };
        assert_eq!(small.unaccounted(), 0);
    }

    #[test]
    fn the_overlay_line_and_the_probe_json_name_every_part() {
        let b = Breakdown {
            scrollback: vec![("t1".into(), 1_048_576)],
            private_ws: 10 << 20,
            ..Breakdown::default()
        };
        let line = b.line();
        for word in [
            "atlas",
            "grid tex",
            "cpu frame",
            "glyphs",
            "scrollback 1.0 MB",
            "fonts",
            "help",
            "unaccounted",
        ] {
            assert!(line.contains(word), "{word:?} missing from {line}");
        }
        let j = b.to_json();
        for key in [
            "atlas_cpu",
            "atlas_gpu",
            "grid_texture",
            "cpu_frame_buffer",
            "glyph_cache",
            "scrollback",
            "scrollback_total",
            "font_database",
            "help_index",
            "settings_catalogue",
            "private_ws",
            "accounted",
            "unaccounted",
        ] {
            assert!(j.get(key).is_some(), "{key} missing from the probe answer");
        }
    }

    #[cfg_attr(
        not(windows),
        ignore = "process memory is read through Win32; macOS phase 3 of the port plan"
    )]
    #[test]
    fn no_source_means_no_breakdown_until_the_window_sets_one() {
        // A fresh test thread has no source.
        assert!(std::thread::spawn(breakdown).join().unwrap().is_none());
        set_breakdown_source(std::rc::Rc::new(|| Breakdown {
            help_index: 7,
            ..Breakdown::default()
        }));
        let b = breakdown().expect("a source is set");
        assert_eq!(b.help_index, 7);
        assert!(b.private_ws > 0);
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
