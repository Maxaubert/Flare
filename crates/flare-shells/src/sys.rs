//! SysTools: Windows tools by full System32 path.

use std::sync::OnceLock;

/// The Windows tools Flare runs, each by its full path under the SystemRoot, never a bare name
/// (a bare name would be searched for in the current folder first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysTools {
    pub powershell: String,
    pub cmd: String,
    pub where_exe: String,
    pub wsl: String,
    pub reg: String,
}

/// root = the SystemRoot folder, e.g. "C:\\Windows" (a trailing backslash is ignored).
pub fn sys_tools_for(system_root: &str) -> SysTools {
    let root = system_root.trim_end_matches(['\\', '/']);
    let system32 = format!("{root}\\System32");
    SysTools {
        powershell: format!("{system32}\\WindowsPowerShell\\v1.0\\powershell.exe"),
        cmd: format!("{system32}\\cmd.exe"),
        where_exe: format!("{system32}\\where.exe"),
        wsl: format!("{system32}\\wsl.exe"),
        reg: format!("{system32}\\reg.exe"),
    }
}

/// Cached. Root from env SystemRoot, else windir, else "C:\\Windows".
pub fn sys_tools() -> &'static SysTools {
    static TOOLS: OnceLock<SysTools> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let root = ["SystemRoot", "windir"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "C:\\Windows".to_string());
        sys_tools_for(&root)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sys_tools_for_builds_full_paths() {
        let t = sys_tools_for("D:\\Win\\");
        assert_eq!(t.cmd, "D:\\Win\\System32\\cmd.exe");
        assert_eq!(t.where_exe, "D:\\Win\\System32\\where.exe");
        assert_eq!(t.wsl, "D:\\Win\\System32\\wsl.exe");
        assert_eq!(t.reg, "D:\\Win\\System32\\reg.exe");
        assert_eq!(
            t.powershell,
            "D:\\Win\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
    }

    #[test]
    fn every_windows_tool_lives_under_system32_never_a_bare_name() {
        let t = sys_tools();
        for p in [&t.powershell, &t.cmd, &t.where_exe, &t.wsl, &t.reg] {
            assert_eq!(p.as_bytes()[1], b':', "{p}");
            assert_eq!(p.as_bytes()[2], b'\\', "{p}");
            assert!(p.to_lowercase().contains("\\system32\\"), "{p}");
        }
    }
}
