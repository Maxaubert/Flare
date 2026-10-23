//! PtyProcess trait, SpawnSpec, PortablePty.

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    pub exe: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

pub trait PtyProcess: Send {
    fn pid(&self) -> u32;
    /// Once.
    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Once.
    fn take_writer(&mut self) -> Option<Box<dyn Write + Send>>;
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String>;
    /// Ends the process (TerminateProcess). Never blocks. Safe to call twice.
    fn kill(&mut self);
    /// None while the process runs.
    fn exit_code(&mut self) -> Option<u32>;
    /// `cb` runs once on another thread when the process has exited (or at once, on this thread, if it already has).
    fn on_exit(&mut self, cb: Box<dyn FnOnce(Option<u32>) + Send>);
}

pub trait PtySpawner: Send + Sync {
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn PtyProcess>, String>;
}

/// Loads the bundled ConPTY first, then spawns through portable-pty.
pub struct PortableSpawner;

impl PtySpawner for PortableSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn PtyProcess>, String> {
        crate::conpty::load_bundled().map_err(|e| e.to_string())?;
        if !std::path::Path::new(&spec.cwd).is_dir() {
            return Err(format!("The folder does not exist: {}", spec.cwd));
        }
        Ok(Box::new(PortablePty::spawn(spec)?))
    }
}

type ExitCb = Box<dyn FnOnce(Option<u32>) + Send>;

#[derive(Default)]
struct ExitState {
    done: bool,
    code: Option<u32>,
    cb: Option<ExitCb>,
}

pub struct PortablePty {
    master: Option<Box<dyn MasterPty + Send>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    pid: u32,
    exit: Arc<Mutex<ExitState>>,
    waiter: Option<JoinHandle<()>>,
}

impl PortablePty {
    pub fn spawn(spec: &SpawnSpec) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new(&spec.exe);
        cmd.args(&spec.args);
        // portable-pty starts from the process and registry environment; Flare passes its own exactly.
        cmd.env_clear();
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        cmd.cwd(&spec.cwd);
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        let pid = child.process_id().unwrap_or(0);
        // Every shell joins the kill-on-close job: nothing outlives the process that started it.
        join_kill_on_close_job(pid);
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let exit = Arc::new(Mutex::new(ExitState::default()));
        let exit_w = Arc::clone(&exit);
        let waiter = std::thread::Builder::new()
            .name("flare-pty-wait".into())
            .spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code()); // sync-ok: the waiter thread of a session, ended when the process exits
                let cb = {
                    let mut st = exit_w.lock().unwrap_or_else(|p| p.into_inner());
                    st.done = true;
                    st.code = code;
                    st.cb.take()
                };
                if let Some(cb) = cb {
                    cb(code);
                }
            })
            .map_err(|e| e.to_string())?;
        // The slave handle is no longer needed here; the child holds its own.
        drop(pair.slave);
        Ok(PortablePty {
            master: Some(pair.master),
            killer,
            reader: Some(reader),
            writer: Some(writer),
            pid,
            exit,
            waiter: Some(waiter),
        })
    }
}

impl PtyProcess for PortablePty {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    fn take_writer(&mut self) -> Option<Box<dyn Write + Send>> {
        self.writer.take()
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        match &self.master {
            Some(m) => m
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| e.to_string()),
            None => Err("The terminal is closed".to_string()),
        }
    }

    fn kill(&mut self) {
        // The shell's children (an agent, a ping) would keep the pseudo console open after the shell is
        // gone, and closing it waits for them: end the tree first, while the shell still names them.
        let running = !self.exit.lock().unwrap_or_else(|p| p.into_inner()).done;
        if running && self.pid != 0 {
            kill_descendants(self.pid);
        }
        // Errors mean the process is already gone.
        let _ = self.killer.kill();
    }

    fn exit_code(&mut self) -> Option<u32> {
        let st = self.exit.lock().unwrap_or_else(|p| p.into_inner());
        st.code
    }

    fn on_exit(&mut self, cb: ExitCb) {
        let mut st = self.exit.lock().unwrap_or_else(|p| p.into_inner());
        if st.done {
            let code = st.code;
            drop(st);
            cb(code);
        } else {
            st.cb = Some(cb);
        }
    }
}

/// Pure: the processes below `root` in a (pid, parent pid) list, cycle-safe, `root` excluded. A child is
/// followed only when `accept(parent, child)` says so; a refused child and everything under it is left out.
fn walk(rows: &[(u32, u32)], root: u32, accept: &dyn Fn(u32, u32) -> bool) -> Vec<u32> {
    let mut seen = std::collections::HashSet::from([root]);
    let mut out = Vec::new();
    let mut queue = vec![root];
    let mut i = 0;
    while i < queue.len() {
        let p = queue[i];
        i += 1;
        for &(pid, ppid) in rows {
            if ppid == p && !seen.contains(&pid) && accept(p, pid) {
                seen.insert(pid);
                out.push(pid);
                queue.push(pid);
            }
        }
    }
    out
}

#[cfg(test)]
fn descendants_of(rows: &[(u32, u32)], root: u32) -> Vec<u32> {
    walk(rows, root, &|_, _| true)
}

/// What the kill needs to know about one process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcInfo {
    /// Creation time (FILETIME ticks); None when it cannot be read.
    created: Option<u64>,
    /// The image is GUI-subsystem: a window app, not a console client.
    gui: bool,
    /// The console host process the process is attached to (flags masked off); None when it
    /// cannot be read. A program the user opened in its own console window has another host.
    console: Option<usize>,
}

/// Pure: the processes below `root` that closing the tab may end. A GUI-subsystem process is an app the
/// user started from the shell (Notepad, Excel, Chrome): it is spared, with everything under it. A process
/// created before its listed parent only carries a recycled parent id (the real parent died and the number
/// was reused): it is spared too, and so is one whose creation time cannot be read (audit H1, L1). A
/// console process attached to another console host than the shell's (`start cmd /k npm run dev`, a
/// window of its own) does not hold this tab's pseudo console, and is spared like Prism Terminal does.
fn kill_set(rows: &[(u32, u32)], root: u32, info: &dyn Fn(u32) -> ProcInfo) -> Vec<u32> {
    let root_console = info(root).console;
    walk(rows, root, &|parent, child| {
        let c = info(child);
        if c.gui {
            return false;
        }
        if let (Some(mine), Some(theirs)) = (root_console, c.console)
            && mine != theirs
        {
            return false;
        }
        match (info(parent).created, c.created) {
            (_, None) => false,
            (Some(p), Some(c)) => c >= p,
            (None, Some(_)) => true,
        }
    })
}

/// The subsystem word of a PE image: 2 is GUI, 3 is console. None when the file is not a readable PE.
fn pe_subsystem(path: &std::path::Path) -> Option<u16> {
    use std::io::{Seek, SeekFrom};
    // sync-ok: runs on the closing thread for a few processes, reading 6 bytes of a header
    let mut f = std::fs::File::open(path).ok()?;
    let mut w = [0u8; 4];
    f.seek(SeekFrom::Start(0x3C)).ok()?;
    f.read_exact(&mut w).ok()?;
    let pe = u32::from_le_bytes(w) as u64;
    f.seek(SeekFrom::Start(pe)).ok()?;
    f.read_exact(&mut w).ok()?;
    if &w != b"PE\0\0" {
        return None;
    }
    // Optional header starts 24 bytes after the signature; the subsystem sits 68 bytes into it.
    f.seek(SeekFrom::Start(pe + 24 + 68)).ok()?;
    let mut s = [0u8; 2];
    f.read_exact(&mut s).ok()?;
    Some(u16::from_le_bytes(s))
}

fn proc_info(pid: u32) -> ProcInfo {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    let none = ProcInfo {
        created: None,
        gui: false,
        console: None,
    };
    // SAFETY: plain values in; a null handle means failure and is checked.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return none;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut cr, mut ex, mut kr, mut us) = (zero, zero, zero, zero);
    // SAFETY: `h` is a live handle; the four out structs are live.
    let ok = unsafe { GetProcessTimes(h, &mut cr, &mut ex, &mut kr, &mut us) } != 0;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` units and `len` is updated to the count written.
    let named = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) } != 0;
    // ProcessConsoleHostProcess (49): the pid of the process's console host, with flags in the
    // two low bits.
    let mut host: usize = 0;
    let mut got: u32 = 0;
    // SAFETY: `host` is a live usize of the size given; `got` is a live out value.
    let st = unsafe {
        windows_sys::Wdk::System::Threading::NtQueryInformationProcess(
            h,
            49,
            std::ptr::addr_of_mut!(host).cast(),
            std::mem::size_of::<usize>() as u32,
            &mut got,
        )
    };
    let console = (st >= 0 && host != 0).then_some(host & !3);
    // SAFETY: `h` is ours and closed once.
    unsafe { CloseHandle(h) };
    let gui = named
        && pe_subsystem(std::path::Path::new(&String::from_utf16_lossy(
            &buf[..len as usize],
        )))
        .is_some_and(|s| s == 2);
    ProcInfo {
        created: ok.then(|| (u64::from(cr.dwHighDateTime) << 32) | u64::from(cr.dwLowDateTime)),
        gui,
        console,
    }
}

/// Ends the console processes below `root` (TerminateProcess), sparing GUI apps. Never waits; failures are
/// ignored.
fn kill_descendants(root: u32) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: plain values in; an invalid handle is checked.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return;
    }
    let mut rows: Vec<(u32, u32)> = Vec::new();
    // SAFETY: PROCESSENTRY32W is plain data; zero is valid before dwSize is set.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a live snapshot handle and `e` has its size set.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok {
        rows.push((e.th32ProcessID, e.th32ParentProcessID));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: `snap` is closed once.
    unsafe { CloseHandle(snap) };
    // Each process is read once: the walk asks about a parent for every child it has.
    let cache = std::cell::RefCell::new(std::collections::HashMap::<u32, ProcInfo>::new());
    let info = |pid: u32| {
        *cache
            .borrow_mut()
            .entry(pid)
            .or_insert_with(|| proc_info(pid))
    };
    // Deepest first is not needed: a parent's death does not stop a child.
    for pid in kill_set(&rows, root, &info) {
        // SAFETY: OpenProcess takes plain values; a null handle means failure and is checked.
        let h = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if h.is_null() {
            continue;
        }
        // SAFETY: `h` is a live process handle with terminate access, closed once.
        unsafe {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}
/// One job object for the whole process, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: every shell the process
/// starts joins it, so when the process ends (a panic that aborts, a timeout kill, a crash) Windows
/// ends the shells with it. SILENT_BREAKAWAY_OK: only the shells themselves are members, what a shell starts
/// is not, so a window app the user opened (Notepad, Excel) outlives the tab, the quit and a crash. The
/// console processes of a shell end with their pseudo console, and kill ends them first (audit H1).
fn join_kill_on_close_job(pid: u32) {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };
    // The handle is kept for the life of the process on purpose; closing it is what kills the members.
    static JOB: OnceLock<usize> = OnceLock::new();
    if pid == 0 {
        return;
    }
    let job = *JOB.get_or_init(|| {
        // SAFETY: null attributes and name are valid; a null result means failure and is checked.
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if h.is_null() {
            return 0;
        }
        // SAFETY: plain data, zero is a valid initial state.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
        // SAFETY: `info` is a live struct of the size passed.
        let ok = unsafe {
            SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            // SAFETY: `h` is ours and closed once.
            unsafe { CloseHandle(h) };
            return 0;
        }
        h as usize
    });
    if job == 0 {
        return;
    }
    // SAFETY: plain values in; a null handle means failure and is checked.
    let p: HANDLE = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
    if p.is_null() {
        return;
    }
    // SAFETY: both handles are live; failure (already in a non-nestable job) leaves the old behaviour.
    unsafe {
        AssignProcessToJobObject(job as HANDLE, p);
        CloseHandle(p);
    }
}
impl Drop for PortablePty {
    fn drop(&mut self) {
        // End the shell so the waiter returns, then close the pseudo console, which is what finally
        // ends a blocked read (the reader gets no end-of-file when the shell exits on its own).
        self.kill();
        self.writer = None;
        self.master = None;
        if let Some(w) = self.waiter.take() {
            let _ = w.join(); // sync-ok: the process was just killed; wait returns at once
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn descendants_of_walks_the_tree_and_survives_a_cycle() {
        let rows = [(10, 1), (20, 10), (30, 20), (40, 1), (5, 6), (6, 5)];
        let mut d = descendants_of(&rows, 10);
        d.sort();
        assert_eq!(d, vec![20, 30]);
        assert!(descendants_of(&rows, 40).is_empty());
        assert_eq!(descendants_of(&rows, 5), vec![6]);
    }

    fn info_of(table: &[(u32, u64, bool)]) -> impl Fn(u32) -> ProcInfo + '_ {
        move |pid| {
            table
                .iter()
                .find(|r| r.0 == pid)
                .map(|r| ProcInfo {
                    created: Some(r.1),
                    gui: r.2,
                    console: Some(7),
                })
                .unwrap_or(ProcInfo {
                    created: None,
                    gui: false,
                    console: None,
                })
        }
    }

    #[test]
    fn a_console_window_of_its_own_is_spared() {
        // shell 10 (console host 7) -> `start cmd /k` 20 (its own host 8) -> node 30 (host 8);
        // shell 10 -> ping 40 (host 7).
        let rows = [(20, 10), (30, 20), (40, 10)];
        let info = |pid: u32| ProcInfo {
            created: Some(100 + u64::from(pid)),
            gui: false,
            console: Some(if pid == 20 || pid == 30 { 8 } else { 7 }),
        };
        let mut k = kill_set(&rows, 10, &info);
        k.sort();
        assert_eq!(k, vec![40]);
    }

    #[test]
    fn a_gui_child_and_everything_under_it_is_spared() {
        // shell 10 -> notepad 20 (GUI) -> helper 30; shell 10 -> ping 40 (console)
        let rows = [(20, 10), (30, 20), (40, 10)];
        let table = [
            (10, 100, false),
            (20, 110, true),
            (30, 120, false),
            (40, 130, false),
        ];
        let mut k = kill_set(&rows, 10, &info_of(&table));
        k.sort();
        assert_eq!(k, vec![40]);
    }

    #[test]
    fn a_process_older_than_its_parent_is_a_recycled_pid_and_is_spared() {
        // 20 claims parent 10 but was created before 10: its real parent died and the pid was reused.
        let rows = [(20, 10), (30, 20), (40, 10)];
        let table = [
            (10, 100, false),
            (20, 50, false),
            (30, 120, false),
            (40, 130, false),
        ];
        let mut k = kill_set(&rows, 10, &info_of(&table));
        k.sort();
        assert_eq!(k, vec![40]);
    }
    type Shared = Arc<Mutex<Vec<u8>>>;
    type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

    /// Reads into a shared buffer on a thread and answers the cursor query ConPTY sends at startup.
    fn pump(p: &mut Box<dyn PtyProcess>) -> (Shared, SharedWriter, JoinHandle<()>) {
        let mut reader = p.take_reader().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(p.take_writer().unwrap()));
        let buf: Shared = Arc::new(Mutex::new(Vec::new()));
        let (b2, w2) = (Arc::clone(&buf), Arc::clone(&writer));
        let h = std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                b2.lock().unwrap().extend_from_slice(&chunk[..n]);
                if chunk[..n].windows(4).any(|w| w == b"\x1b[6n") {
                    let mut w = w2.lock().unwrap();
                    let _ = w.write_all(b"\x1b[1;1R");
                    let _ = w.flush();
                }
            }
        });
        (buf, writer, h)
    }

    fn count(buf: &Shared, needle: &str) -> usize {
        String::from_utf8_lossy(&buf.lock().unwrap())
            .matches(needle)
            .count()
    }

    fn wait_for(buf: &Shared, needle: &str, times: usize) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if count(buf, needle) >= times {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    fn std_env() -> Vec<(String, String)> {
        std::env::vars().collect()
    }

    fn cmd_spec(args: &[&str], cwd: &str, env: Vec<(String, String)>) -> SpawnSpec {
        SpawnSpec {
            exe: "cmd.exe".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: cwd.into(),
            env,
            cols: 80,
            rows: 24,
        }
    }

    fn temp_cwd() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    struct Reap(Option<Box<dyn PtyProcess>>);
    impl Drop for Reap {
        fn drop(&mut self) {
            if let Some(p) = self.0.as_mut() {
                p.kill();
            }
        }
    }

    #[test]
    fn spawns_cmd_and_reads_its_banner() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, _w, _h) = pump(g.0.as_mut().unwrap());
        assert!(g.0.as_ref().unwrap().pid() > 0);
        assert!(wait_for(&buf, "Microsoft Windows", 1));
    }

    #[test]
    fn writes_reach_the_shell() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, w, _h) = pump(g.0.as_mut().unwrap());
        assert!(wait_for(&buf, "Microsoft Windows", 1));
        w.lock().unwrap().write_all(b"echo flare-m2-ok\r").unwrap();
        assert!(wait_for(&buf, "flare-m2-ok", 2));
    }

    #[test]
    fn exit_is_reported_without_an_eof() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, w, h) = pump(g.0.as_mut().unwrap());
        assert!(wait_for(&buf, "Microsoft Windows", 1));
        let (tx, rx) = std::sync::mpsc::channel();
        g.0.as_mut().unwrap().on_exit(Box::new(move |c| {
            let _ = tx.send(c);
        }));
        w.lock().unwrap().write_all(b"exit\r").unwrap();
        let code = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("exit callback");
        assert_eq!(code, Some(0));
        assert!(
            !h.is_finished(),
            "the reader must still be blocked, ConPTY sends no EOF"
        );
        drop(w);
        g.0 = None;
        let end = Instant::now() + Duration::from_secs(3);
        while !h.is_finished() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(h.is_finished(), "dropping the process ends the read");
    }

    #[test]
    fn kill_ends_the_process() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (_buf, _w, _h) = pump(g.0.as_mut().unwrap());
        let p = g.0.as_mut().unwrap();
        p.kill();
        p.kill();
        let end = Instant::now() + Duration::from_secs(5);
        while p.exit_code().is_none() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(p.exit_code().is_some());
    }

    #[test]
    fn a_missing_folder_is_an_error_not_home() {
        let r = PortableSpawner.spawn(&cmd_spec(&[], r"C:\no\such\folder\flare", std_env()));
        let Err(msg) = r else {
            panic!("expected an error")
        };
        assert!(msg.contains("folder"), "{msg}");
    }

    #[test]
    fn the_environment_is_exactly_what_was_given() {
        let pick = |k: &str| std::env::var(k).unwrap_or_default();
        let env = vec![
            ("PATH".to_string(), pick("PATH")),
            ("SystemRoot".to_string(), pick("SystemRoot")),
            ("FLARE_T".to_string(), "1".to_string()),
        ];
        let p = PortableSpawner
            .spawn(&cmd_spec(&["/c", "set"], &temp_cwd(), env))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, _w, _h) = pump(g.0.as_mut().unwrap());
        assert!(wait_for(&buf, "FLARE_T=1", 1));
        // Give the rest of the listing time to arrive before checking the absence.
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(count(&buf, "USERPROFILE="), 0);
    }

    #[test]
    fn resize_is_accepted() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, _w, _h) = pump(g.0.as_mut().unwrap());
        assert!(wait_for(&buf, "Microsoft Windows", 1));
        assert!(g.0.as_mut().unwrap().resize(100, 30).is_ok());
    }

    fn alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: plain values; null checked; handle closed once.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            ok != 0 && code == 259
        }
    }

    /// Helper half: only runs when the parent test sets the variable. Starts shells, writes their
    /// pids, then dies without any destructor, the way a crash or a timeout kill does.
    #[test]
    fn job_helper_spawns_shells_then_aborts() {
        let Ok(out) = std::env::var("FLARE_JOB_HELPER_OUT") else {
            return;
        };
        let mut pids = Vec::new();
        for i in 0..3 {
            let marker = format!("flare-job-marker-{}-{i}", std::process::id());
            let p = PortableSpawner
                .spawn(&SpawnSpec {
                    exe: "pwsh.exe".into(),
                    args: [
                        "-NoLogo",
                        "-NoProfile",
                        "-NoExit",
                        "-Command",
                        &format!("$m = '{marker}'"),
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                    ..cmd_spec(&[], &temp_cwd(), std_env())
                })
                .unwrap();
            pids.push(p.pid());
            std::mem::forget(p);
        }
        let text: Vec<String> = pids.iter().map(u32::to_string).collect();
        std::fs::write(&out, text.join(" ")).unwrap();
        std::process::abort();
    }

    /// Two copies of ping.exe in `dir`: a console one and one whose PE header says GUI subsystem (a window
    /// app with no window), named uniquely so they can be found by name.
    fn probes(dir: &std::path::Path, tag: &str) -> (String, String) {
        let src =
            std::path::Path::new(&std::env::var("SystemRoot").unwrap()).join("System32\\PING.EXE");
        let mut bytes = std::fs::read(src).unwrap();
        let con = format!("flare-con-{tag}.exe");
        let gui = format!("flare-gui-{tag}.exe");
        std::fs::write(dir.join(&con), &bytes).unwrap();
        let pe = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
        let at = pe + 24 + 68;
        assert_eq!(
            u16::from_le_bytes([bytes[at], bytes[at + 1]]),
            3,
            "ping is a console image"
        );
        bytes[at] = 2;
        std::fs::write(dir.join(&gui), &bytes).unwrap();
        (con, gui)
    }

    fn pids_named(name: &str) -> Vec<u32> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        let mut out = Vec::new();
        // SAFETY: plain values; the snapshot is closed once; the entry is plain data with its size set.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                let n = String::from_utf16_lossy(&e.szExeFile);
                if n.trim_end_matches('\0').eq_ignore_ascii_case(name) {
                    out.push(e.th32ProcessID);
                }
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        out
    }

    fn end_pid(pid: u32) {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    }

    /// A cmd shell that starts the console probe and the GUI probe, both under the shell.
    fn start_probes(dir: &std::path::Path, con: &str, gui: &str) -> Box<dyn PtyProcess> {
        let script = dir.join("go.cmd");
        std::fs::write(
            &script,
            format!(
                "@echo off\r
start \"\" /b \"{d}\\{con}\" -n 120 127.0.0.1 >nul\r
start \"\" /b \"{d}\\{gui}\" -n 120 127.0.0.1 >nul\r
",
                d = dir.display()
            ),
        )
        .unwrap();
        PortableSpawner
            .spawn(&cmd_spec(
                &["/k", &script.to_string_lossy()],
                &temp_cwd(),
                std_env(),
            ))
            .unwrap()
    }

    fn wait_pid(name: &str) -> Option<u32> {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if let Some(p) = pids_named(name).first() {
                return Some(*p);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn closing_a_tab_ends_its_console_children_and_spares_a_gui_app() {
        let dir = tempfile::tempdir().unwrap();
        let tag = format!("kill{}", std::process::id());
        let (con, gui) = probes(dir.path(), &tag);
        let p = start_probes(dir.path(), &con, &gui);
        let mut g = Reap(Some(p));
        let (_buf, _w, _h) = pump(g.0.as_mut().unwrap());
        let con_pid = wait_pid(&con).expect("the console child started");
        let gui_pid = wait_pid(&gui).expect("the GUI child started");
        g.0.as_mut().unwrap().kill();
        let end = Instant::now() + Duration::from_secs(10);
        while alive(con_pid) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        let gui_alive = alive(gui_pid);
        let con_alive = alive(con_pid);
        end_pid(gui_pid);
        end_pid(con_pid);
        assert!(!con_alive, "a console child ends with its tab");
        assert!(gui_alive, "a GUI app the user started outlives the tab");
    }

    /// Helper half: starts a shell with a GUI child, writes its pid, then dies without a destructor.
    #[test]
    fn gui_job_helper_starts_a_gui_app_then_aborts() {
        let (Ok(out), Ok(dir)) = (
            std::env::var("FLARE_GUI_JOB_HELPER_OUT"),
            std::env::var("FLARE_GUI_JOB_HELPER_DIR"),
        ) else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let (con, gui) = (
            format!(
                "flare-con-job{}.exe",
                std::env::var("FLARE_GUI_JOB_TAG").unwrap()
            ),
            format!(
                "flare-gui-job{}.exe",
                std::env::var("FLARE_GUI_JOB_TAG").unwrap()
            ),
        );
        let p = start_probes(&dir, &con, &gui);
        let gui_pid = wait_pid(&gui).expect("the GUI child started");
        let con_pid = wait_pid(&con).expect("the console child started");
        std::mem::forget(p);
        std::fs::write(&out, format!("{gui_pid} {con_pid}")).unwrap();
        std::process::abort();
    }

    #[test]
    fn a_gui_app_outlives_a_crash_of_the_terminal_that_started_it() {
        let dir = tempfile::tempdir().unwrap();
        let tag = format!("{}", std::process::id());
        probes(dir.path(), &format!("job{tag}"));
        let out = dir.path().join("pids.txt");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pty::tests::gui_job_helper_starts_a_gui_app_then_aborts",
                "--nocapture",
            ])
            .env("FLARE_GUI_JOB_HELPER_OUT", &out)
            .env("FLARE_GUI_JOB_HELPER_DIR", dir.path())
            .env("FLARE_GUI_JOB_TAG", &tag)
            .status()
            .unwrap();
        assert!(!status.success(), "the helper aborts");
        let pids: Vec<u32> = std::fs::read_to_string(&out)
            .expect("helper wrote its pids")
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        // Give the job time to end whatever it is going to end.
        std::thread::sleep(Duration::from_secs(2));
        let end = Instant::now() + Duration::from_secs(10);
        while alive(pids[1]) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        let gui_alive = alive(pids[0]);
        let con_alive = alive(pids[1]);
        for p in &pids {
            end_pid(*p);
        }
        assert!(gui_alive, "a GUI app the user started outlives the crash");
        assert!(
            !con_alive,
            "a console child still ends with a crash of its terminal"
        );
    }
    #[test]
    fn shells_die_with_the_process_that_started_them() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pids.txt");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pty::tests::job_helper_spawns_shells_then_aborts",
                "--nocapture",
            ])
            .env("FLARE_JOB_HELPER_OUT", &out)
            .status()
            .unwrap();
        assert!(!status.success(), "the helper aborts");
        let pids: Vec<u32> = std::fs::read_to_string(&out)
            .expect("helper wrote its pids")
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(pids.len(), 3);
        let end = Instant::now() + Duration::from_secs(10);
        while pids.iter().any(|p| alive(*p)) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        let left: Vec<u32> = pids.iter().copied().filter(|p| alive(*p)).collect();
        for p in &left {
            // Do not leave the orphan this very test proves.
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &p.to_string()])
                .output();
        }
        assert!(left.is_empty(), "shells outlived their owner: {left:?}");
    }
}
