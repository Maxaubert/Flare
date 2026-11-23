//! `emulator -accel-check` read back: whether the hypervisor the emulator needs is there (Windows Hypervisor Platform
//! on Windows, Hypervisor.framework on macOS). The emu host runs it once per Flare session, on a worker.
//!
//! Its output (37.2.13 on Windows 11, 2026-11-22):
//!
//! ```text
//! accel:
//! 0
//! WHPX(10.0.26300) is installed and usable.
//! accel
//! ```
//!
//! The number is the status: 0 is usable, anything else is a problem the line names.

/// The check's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accel {
    pub status: i32,
    /// The emulator's own line.
    pub line: String,
}

impl Accel {
    pub fn usable(&self) -> bool {
        self.status == 0
    }

    /// A short name for the hello facts: `whpx`, `hvf`, `aehd`, `kvm`, or `unknown`.
    pub fn kind(&self) -> &'static str {
        let l = self.line.to_ascii_lowercase();
        if l.contains("whpx") {
            "whpx"
        } else if l.contains("hypervisor.framework") || l.contains("hvf") {
            "hvf"
        } else if l.contains("aehd") || l.contains("gvm") {
            "aehd"
        } else if l.contains("kvm") {
            "kvm"
        } else {
            "unknown"
        }
    }
}

/// The check's output, or None when it is not shaped as above.
pub fn parse(out: &str) -> Option<Accel> {
    let lines: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let start = lines.iter().position(|l| *l == "accel:")?;
    let status = lines.get(start + 1)?.parse::<i32>().ok()?;
    let line = lines
        .get(start + 2)
        .filter(|l| **l != "accel")
        .map_or_else(String::new, |l| (*l).to_string());
    Some(Accel { status, line })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accel_check_says_whether_the_hypervisor_is_usable() {
        let a = parse("accel:\n0\nWHPX(10.0.26300) is installed and usable.\naccel\n").unwrap();
        assert!(a.usable());
        assert_eq!(a.kind(), "whpx");
        assert_eq!(a.line, "WHPX(10.0.26300) is installed and usable.");
        let mac = parse("INFO | something\r\naccel:\r\n0\r\nHypervisor.Framework OS X Version 26.1\r\naccel\r\n").unwrap();
        assert_eq!((mac.usable(), mac.kind()), (true, "hvf"));
        let off = parse("accel:\n8\nPlease enable Windows Hypervisor Platform\naccel\n").unwrap();
        assert!(!off.usable());
        assert_eq!(off.status, 8);
        assert_eq!(parse("accel:\n0\naccel\n").unwrap().line, "");
        assert_eq!(parse("nothing"), None);
        assert_eq!(parse("accel:\nx\n"), None);
    }
}
