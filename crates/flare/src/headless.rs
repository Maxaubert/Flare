//! `--screenshot` and `--bench-frames`: the window is drawn by Slint's software renderer into a
//! buffer, and no OS window is ever created or shown.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use flare_agent::kinds::AgentKind;
use flare_render::gpu::headless_device;
use flare_render::grid::Grid;
use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use flare_settings::kv::PrefStore;
use flare_settings::recent_roots::{remember_root, toggle_pin};
use flare_settings::sidebar::{SidebarMode, set_sidebar_mode_stored};
use flare_settings::tab_layout::{TabLayout, set_tab_layout};
use flare_tabs::restore::{NoSessions, SavedTab, SavedTabs, tabs_json};
use flare_tabs::tabs_store::TabsStore;
use flare_term::emulator::TermEvent;
use flare_term::fake::{FakeShells, FakeSpawner};
use flare_term::handle::{StartOptions, TermHandle};
use flare_term::pty::PortableSpawner;
use flare_term::session::{RealShells, SessionManager, SessionOptions};
#[cfg(test)]
use flare_theme::look::AppliedLook;
use flare_win::window_state::WindowStateStore;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{
    ComponentHandle, Image, LogicalPosition, Model, PhysicalSize, Rgba8Pixel, SharedPixelBuffer,
    SharedString,
};

use crate::AppWindow;
use crate::appctl::{AppCtl, Deps, Post, TermPorts, UiMsg};
use crate::chrome_apply::{apply_chrome, apply_to_host};
use crate::config::{Args, RendererChoice};
use crate::host::{GridHost, GridPainter, GridSource, SampleSource};
use crate::live::{Live, LiveSource, shutdown};
use crate::memory::process_memory;
use crate::tab_host::TabHost;
use crate::term_ui::{DecorInputs, PointerIn};
use crate::theme_scene::{SAMPLE_STREAM, StreamSource, scene_font, scene_look};
use flare_term::keys::KeyEvent;
use flare_term::path_open::{PathKind, StatFn};

/// The line the live tab types; its echo and its output both show it on screen.
const ECHO_MARK: &str = "flare-m2-ok";
/// How long the live tab may take to show its output before the run fails.
const ECHO_WAIT: Duration = Duration::from_secs(15);

/// Hands Slint the one software window, so nothing is created on the desktop.
struct HeadlessPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// Runs the headless tools. Errors starting with "GPU unavailable" mean exit code 2.
pub fn run(args: Args, start: Instant) -> Result<(), String> {
    headless(args, start, false)
}

/// `--live-screenshot`: the same headless window, but the grid is a live shell that is sent
/// `echo flare-m2-ok` and drawn once the output shows. The shell is shut down on every path.
pub fn run_live(args: Args, start: Instant) -> Result<(), String> {
    headless(args, start, true)
}

fn headless(args: Args, start: Instant, live_mode: bool) -> Result<(), String> {
    let scale = args.scale;
    let width = (args.size.0 as f32 * scale).round() as u32;
    let height = (args.size.1 as f32 * scale).round() as u32;

    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(HeadlessPlatform(win.clone())))
        .map_err(|e| format!("cannot set the headless platform: {e}"))?;

    let glyphs = Box::new(Rasterizer::new(DEFAULT_FAMILIES, 1.0));
    let mut host = match args.renderer {
        RendererChoice::Software => GridHost::new(glyphs, GridPainter::Cpu),
        RendererChoice::Gpu => {
            let (device, queue, info) =
                headless_device().map_err(|e| format!("GPU unavailable: {e}"))?;
            log::info!("headless adapter: {} ({:?})", info.name, info.backend);
            let mut host = GridHost::new(glyphs, GridPainter::Gpu);
            host.attach_gpu(&device, &queue);
            host
        }
    };

    let app = AppWindow::new().map_err(|e| format!("cannot create the window: {e}"))?;
    win.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    win.set_size(PhysicalSize::new(width, height));
    app.show()
        .map_err(|e| format!("cannot show the window: {e}"))?;
    host.resize(
        app.get_grid_phys_width().max(0) as u32,
        app.get_grid_phys_height().max(0) as u32,
        app.get_scale(),
    );

    let frame = |host: &mut GridHost| -> Result<Vec<u8>, String> {
        let grid = host
            .render_rgba()
            .ok_or_else(|| format!("the grid has no area at {width}x{height}"))?;
        let buffer =
            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&grid.data, grid.width, grid.height);
        app.set_grid_image(Image::from_rgba8(buffer));
        win.request_redraw();
        let mut pixels = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
        win.draw_if_needed(|r| {
            r.render(&mut pixels, width as usize);
        });
        Ok(pixels.iter().flat_map(straight_rgba).collect())
    };

    // The shell starts only now, after every fallible window step, so no error leaves one behind.
    let live: Option<Live> = live_mode.then(|| start_shell_tab(args.live_shell.as_deref()));
    let first = match &live {
        Some(live) => {
            host.set_source(Box::new(LiveSource::new(Arc::clone(&live.handle))));
            let shown = drive_live(live, &mut host, &frame);
            shutdown(live);
            shown?
        }
        None => frame(&mut host)?,
    };
    let startup_ms = start.elapsed().as_secs_f64() * 1000.0;

    let output = if live_mode {
        args.live_screenshot.as_ref()
    } else {
        args.screenshot.as_ref()
    };
    if let Some(path) = output {
        write_png(path, width, height, &first)?;
        println!("screenshot: {}", path.display());
    }

    if let Some(frames) = args.bench_frames {
        for _ in 0..frames {
            // Each frame is laid out, painted and (on the GPU) read back.
            host.render_rgba()
                .ok_or_else(|| "the grid has no area to benchmark".to_string())?;
        }
        println!("frames: {}", host.stats.summary());
        let memory = process_memory().map_or_else(|| "unavailable".to_string(), |m| m.summary());
        println!("memory: {memory}");
        println!("startup: {startup_ms:.0} ms to the first rendered grid");
    }
    Ok(())
}

/// Starts one tab in the temp folder with the given shell (the saved or default one when None).
/// No waker: nothing waits on an event loop here, the poll in `drive_live` reads the grid.
fn start_shell_tab(shell_id: Option<&str>) -> Live {
    let mgr = SessionManager::new(
        Arc::new(PortableSpawner),
        Arc::new(RealShells),
        SessionOptions::default(),
    );
    let handle = TermHandle::new("live-screenshot", Arc::clone(&mgr), 80, 24);
    handle.start(StartOptions {
        root: std::env::temp_dir().to_string_lossy().into_owned(),
        shell_id: shell_id.map(str::to_string),
        resume: None,
        plugin: None,
    });
    Live { mgr, handle }
}

/// Waits until the typed line shows twice (the echo and the output), re-rendering every 50 ms,
/// and returns that frame. Errors when the shell fails to start or the wait runs out.
fn drive_live(
    live: &Live,
    host: &mut GridHost,
    frame: &dyn Fn(&mut GridHost) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + ECHO_WAIT;
    let mut started = false;
    let mut typed = false;
    loop {
        for event in live.handle.drain_events() {
            match event {
                TermEvent::Started => started = true,
                TermEvent::StartFailed => return Err("the live shell did not start".to_string()),
                _ => {}
            }
        }
        if started && !typed {
            live.handle.send_text(&format!("echo {ECHO_MARK}\r"));
            typed = true;
        }
        let pixels = frame(host)?;
        if typed && marked_rows(&live.handle.snapshot()) >= 2 {
            return Ok(pixels);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the live shell did not show {ECHO_MARK} within {} s",
                ECHO_WAIT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50)); // sync-ok: headless scene tooling, not the live window
    }
}

/// How many screen rows hold the echo mark.
fn marked_rows(grid: &Grid) -> usize {
    (0..grid.rows())
        .filter(|&row| {
            let text: String = (0..grid.cols()).map(|col| grid.cell(col, row).ch).collect();
            text.contains(ECHO_MARK)
        })
        .count()
}

/// One premultiplied pixel as straight RGBA bytes.
fn straight_rgba(p: &PremultipliedRgbaColor) -> [u8; 4] {
    let unpremultiply = |c: u8| -> u8 {
        if p.alpha == 0 {
            0
        } else {
            (u32::from(c) * 255 / u32::from(p.alpha)).min(255) as u8
        }
    };
    [
        unpremultiply(p.red),
        unpremultiply(p.green),
        unpremultiply(p.blue),
        p.alpha,
    ]
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path) // sync-ok: headless scene tooling, not the live window
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

// ---- --scene: the window in one fixed state ----------------------------------------------------

/// The scenes `--scene` takes, in the order the screenshot plan lists them.
pub const SCENES: [&str; 25] = [
    "strip-3",
    "strip-hover",
    "strip-many",
    "strip-list",
    "sidebar-open",
    "sidebar-find",
    "sidebar-collapsed",
    "start",
    "menu-places",
    "menu-tab",
    "dialog-close",
    "tip",
    "theme-sample",
    "term-selection",
    "term-selection-wide",
    "term-find",
    "term-find-none",
    "term-find-many",
    "term-find-empty",
    "term-links",
    "term-menu-plain",
    "term-menu-selection",
    "term-menu-link",
    "term-menu-path",
    "term-copied",
];

/// The folder the scenes' tabs sit in. Inert: nothing is spawned in it and nothing is read from it.
const GITHUB: &str = r"C:\Users\Admin\Documents\Claude\Github";
const HOME: &str = r"C:\Users\Admin";
/// Where the strip's first tab starts in css pixels: after the mark's column and its hairline.
const STRIP_X: f32 = 42.0;
/// The middle of the top row in css pixels.
const BAR_MID_Y: f32 = 17.0;
/// The debounce of the scene's state files (the window's own delay).
const SAVE_DELAY: Duration = Duration::from_millis(30);

thread_local! {
    /// The scene clock in milliseconds. Animations and the controller's timers both follow it.
    static SCENE_MS: Cell<u64> = const { Cell::new(0) };
}

fn scene_now() -> u64 {
    SCENE_MS.with(Cell::get)
}

/// The scene window's platform: it hands Slint the window the next scene made and reads the scene
/// clock. Slint's platform is set once per thread, so a thread that builds many scenes (the pixel
/// tests) swaps the window through `NEXT_SCENE_WINDOW` instead of setting the platform again.
struct ScenePlatform;

thread_local! {
    static NEXT_SCENE_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    static SCENE_PLATFORM_SET: Cell<bool> = const { Cell::new(false) };
}

impl Platform for ScenePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        NEXT_SCENE_WINDOW
            .with(|w| w.borrow_mut().take())
            .map(|w| w as Rc<dyn WindowAdapter>)
            .ok_or_else(|| slint::PlatformError::Other("no scene window is waiting".into()))
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(scene_now())
    }
}

/// Makes `win` the window the next component creation takes; the platform is set on first use.
fn offer_scene_window(win: &Rc<MinimalSoftwareWindow>) -> Result<(), String> {
    if !SCENE_PLATFORM_SET.with(Cell::get) {
        slint::platform::set_platform(Box::new(ScenePlatform))
            .map_err(|e| format!("cannot set the scene platform: {e}"))?;
        SCENE_PLATFORM_SET.with(|s| s.set(true));
    }
    NEXT_SCENE_WINDOW.with(|w| *w.borrow_mut() = Some(win.clone()));
    Ok(())
}

/// The headless GPU, opened once per process: a test that builds many scenes shares it. A static is
/// never dropped, so no device is torn down while a thread-local of the window is already gone.
static SCENE_GPU: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();

/// The headless GPU device and queue; an error starting with "GPU unavailable" means exit code 2.
pub(crate) fn scene_gpu() -> Result<(wgpu::Device, wgpu::Queue), String> {
    if let Some((device, queue)) = SCENE_GPU.get() {
        return Ok((device.clone(), queue.clone()));
    }
    let (device, queue, info) = headless_device().map_err(|e| format!("GPU unavailable: {e}"))?;
    log::info!("scene adapter: {} ({:?})", info.name, info.backend);
    let (device, queue) = SCENE_GPU.get_or_init(|| (device, queue));
    Ok((device.clone(), queue.clone()))
}

fn github(name: &str) -> String {
    format!(r"{GITHUB}\{name}")
}

fn saved(cwds: &[String], active: usize) -> SavedTabs {
    SavedTabs {
        tabs: cwds
            .iter()
            .map(|cwd| SavedTab {
                cwd: cwd.clone(),
                agent: None,
            })
            .collect(),
        active,
    }
}

fn no_prefs(_: &PrefStore) {}

fn vertical(p: &PrefStore) {
    set_tab_layout(p, TabLayout::Vertical);
}

fn collapsed(p: &PrefStore) {
    vertical(p);
    set_sidebar_mode_stored(p, SidebarMode::Collapsed);
}

/// Two pinned places and two recent ones: the start screen.
fn start_places(p: &PrefStore) {
    toggle_pin(p, &github("Prism"));
    toggle_pin(p, &github("Flare"));
    remember_root(p, &github("Sonara"));
    remember_root(p, &github("notes"));
}

/// Two pinned places and three recent ones: the + button's right-click menu lists them.
fn menu_places(p: &PrefStore) {
    toggle_pin(p, &github("Prism"));
    toggle_pin(p, &github("Flare"));
    remember_root(p, &github("Sonara"));
    remember_root(p, &github("notes"));
    remember_root(p, &github("Docs"));
}

/// The tabs a scene restores and the preferences it sets before the window is made.
pub(crate) fn setup(name: &str) -> (SavedTabs, fn(&PrefStore)) {
    let three = [github("Prism"), github("Flare"), github("Sonara")];
    // Thirty tabs: at 1000 px the strip shows icons and the +N control (sixteen all fit).
    let many: Vec<String> = (1..=30).map(|i| github(&format!("Tab{i:02}"))).collect();
    // Two rows share the name "Flare" (two folders): the sidebar must show both.
    let side = [
        github("api"),
        github("web"),
        github("Prism"),
        github("Flare"),
        r"C:\Users\Admin\Documents\Claude\zz-Misc\Flare".to_string(),
        github("notes"),
    ];
    match name {
        "strip-many" | "strip-list" => (saved(&many, 0), no_prefs),
        "sidebar-open" | "sidebar-find" => (saved(&side, 1), vertical),
        "sidebar-collapsed" => (saved(&side, 1), collapsed),
        "start" => (saved(&[], 0), start_places),
        "menu-places" => (saved(&three, 0), menu_places),
        "dialog-close" => (saved(&three, 0), no_prefs),
        // M3's strip-3 window with a single tab.
        "theme-sample" => (saved(&[github("Flare")], 0), no_prefs),
        // The terminal scenes: one live tab in the Flare folder, its shell fed with the text.
        name if is_term_scene(name) => (saved(&[github("Flare")], 0), no_prefs),
        _ => (saved(&three, 0), no_prefs),
    }
}

// ---- the terminal scenes (M5, Task 26) ---------------------------------------------------------

/// The scenes that draw a live shell tab: selection, find, links, the menus and the Copied badge.
fn is_term_scene(name: &str) -> bool {
    name.starts_with("term-")
}

const PROMPT: &str = r"PS C:\Users\Admin\Documents\Claude\Github\Flare> ";
const RUN_URL: &str = "https://github.com/Maxaubert/Flare/actions/runs/1";
const README: &str = r"C:\Users\Admin\Documents\Claude\Github\Flare\README.md";

/// The PowerShell session most scenes show: ten rows, the url on row 5, the path on row 6.
fn term_rows() -> Vec<String> {
    vec![
        format!("{PROMPT}cargo nextest run"),
        "    Starting 214 tests across 12 binaries".to_string(),
        "error[E0432]: unresolved import `flare_term::nope`".to_string(),
        "warning: unused variable `x`".to_string(),
        r"    --> crates\flare\src\main.rs:12:9".to_string(),
        format!("    log at {RUN_URL}"),
        format!("    source {README}"),
        "     Summary [   0.412s] 213 tests run: 212 passed, 1 failed".to_string(),
        "error: test run failed".to_string(),
        PROMPT.to_string(),
    ]
}

/// The rows a terminal scene feeds its shell. `cols` is the grid's width, so a wrapped line wraps
/// where the scene needs it to (a match over two rows, a url over two rows).
fn term_text(name: &str, cols: usize) -> Vec<String> {
    match name {
        "term-selection-wide" => vec![
            "日本語 emoji 😀 text".to_string(),
            "second row of the block".to_string(),
            "third row of the block".to_string(),
            "fourth row of the block".to_string(),
        ],
        "term-find" | "term-find-none" | "term-find-empty" => vec![
            format!("{PROMPT}cargo nextest run"),
            "error[E0432]: unresolved import".to_string(),
            "warning: err helper is unused".to_string(),
            // One match wraps: its "e" ends this row and its "rr" starts the next.
            format!("{}err tail", "-".repeat(cols.saturating_sub(2))),
            "error: test run failed".to_string(),
            "    Summary: 1 err".to_string(),
            PROMPT.to_string(),
        ],
        "term-find-many" => {
            let mut rows = term_rows();
            rows.extend((0..600).map(|_| "err err".to_string()));
            rows
        }
        "term-links" => vec![
            format!("{PROMPT}cargo nextest run"),
            format!("    log at {RUN_URL}"),
            format!("    source {README}"),
            // The url starts 20 cells from the right edge, so it wraps over two rows.
            format!("{}{RUN_URL}", " ".repeat(cols.saturating_sub(20))),
            PROMPT.to_string(),
        ],
        _ => term_rows(),
    }
}

/// The path links' stat: the README and the Flare folder exist; nothing else does.
struct SceneStat;

impl StatFn for SceneStat {
    fn kind(&self, abs: &str) -> Option<PathKind> {
        if abs.eq_ignore_ascii_case(README) {
            Some(PathKind::File)
        } else if abs.eq_ignore_ascii_case(&github("Flare")) {
            Some(PathKind::Dir)
        } else {
            None
        }
    }
}

/// A scene's window: the controller over fake shells, the messages its workers post and the
/// scratch folder its state files live in (removed on drop).
pub(crate) struct Scene {
    win: Rc<MinimalSoftwareWindow>,
    pub(crate) app: AppWindow,
    ctl: Rc<RefCell<AppCtl>>,
    pub(crate) host: Rc<RefCell<GridHost>>,
    queue: Arc<Mutex<VecDeque<UiMsg>>>,
    pub(crate) scale: f32,
    pub(crate) phys: (u32, u32),
    dir: PathBuf,
    /// The shell the grid shows (the terminal scenes feed it and read its cells).
    front: Rc<RefCell<Option<Arc<TermHandle>>>>,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir); // sync-ok: scene tooling, after the picture
    }
}

impl Scene {
    /// The window, the grid, the prefs and the saved tabs, restored before this returns.
    #[cfg(test)]
    pub(crate) fn build(
        args: &Args,
        saved: SavedTabs,
        prep: fn(&PrefStore),
        sample_stream: bool,
    ) -> Result<Self, String> {
        Self::build_with(args, saved, prep, sample_stream, false)
    }

    /// `build`, and `term`: the grid shows the front shell (the terminal scenes) instead of the
    /// sample grid.
    pub(crate) fn build_with(
        args: &Args,
        saved: SavedTabs,
        prep: fn(&PrefStore),
        sample_stream: bool,
        term: bool,
    ) -> Result<Self, String> {
        let scale = args.scale;
        let phys = (
            (args.size.0 as f32 * scale).round() as u32,
            (args.size.1 as f32 * scale).round() as u32,
        );
        let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        offer_scene_window(&win)?;
        let glyphs = Box::new(Rasterizer::new(DEFAULT_FAMILIES, 1.0));
        let host = match args.renderer {
            RendererChoice::Software => GridHost::new(glyphs, GridPainter::Cpu),
            RendererChoice::Gpu => {
                let (device, queue) = scene_gpu()?;
                let mut host = GridHost::new(glyphs, GridPainter::Gpu);
                host.attach_gpu(&device, &queue);
                host
            }
        };
        // The look the scene wears: pt-default, or what the one-run overrides name. A bad theme
        // id stops here, before anything is made.
        let look = scene_look(&args.look, true)?;
        let (families, base_px) = scene_font(&args.look);
        let families: Vec<&str> = families.iter().map(String::as_str).collect();
        let mut host = host;
        host.set_font(&families, base_px as f32);
        apply_to_host(&mut host, &look);
        let host = Rc::new(RefCell::new(host));

        static SCENE_SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "flare-scene-{}-{}",
            std::process::id(),
            SCENE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir) // sync-ok: scene tooling, before the window
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let tabs_file = dir.join("tabs.json");
        if !saved.tabs.is_empty() {
            std::fs::write(&tabs_file, tabs_json(&saved)) // sync-ok: scene tooling
                .map_err(|e| format!("cannot write {}: {e}", tabs_file.display()))?;
        }
        let prefs = PrefStore::memory();
        prep(&prefs);

        let app = AppWindow::new().map_err(|e| format!("cannot create the window: {e}"))?;
        win.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        win.set_size(PhysicalSize::new(phys.0, phys.1));
        app.show()
            .map_err(|e| format!("cannot show the window: {e}"))?;

        let queue: Arc<Mutex<VecDeque<UiMsg>>> = Arc::default();
        let post: Post = {
            let queue = queue.clone();
            Arc::new(move |m| {
                queue
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back(m)
            })
        };
        let waker: Arc<dyn Fn() + Send + Sync> = {
            let post = post.clone();
            Arc::new(move || post(UiMsg::HostWake))
        };
        let grid = host.clone();
        // The front shell, as the grid last drew it; the terminal's controller, once it exists.
        let front: Rc<RefCell<Option<Arc<TermHandle>>>> = Rc::default();
        let inputs_cell: Rc<RefCell<Option<Rc<RefCell<DecorInputs>>>>> = Rc::default();
        let grid_front = front.clone();
        let grid_inputs = inputs_cell.clone();
        let deps = Deps {
            prefs: prefs.clone(),
            tabs_store: Arc::new(TabsStore::new(tabs_file, SAVE_DELAY)),
            window_store: Arc::new(WindowStateStore::new(dir.join("window.json"), SAVE_DELAY)),
            host: TabHost::new(FakeSpawner::new(), FakeShells::new(), prefs.clone(), waker),
            lookup: Arc::new(NoSessions),
            is_dir: Arc::new(|_: &str| true),
            home: HOME.to_string(),
            clock: Rc::new(scene_now),
            post,
            pick_folder: Rc::new(|_, done: Arc<dyn Fn(Option<PathBuf>) + Send + Sync>| {
                done(None);
            }),
            clipboard: Rc::new(|_: &str| {}),
            open_url: Rc::new(|_: &str| {}),
            // A terminal scene draws its front shell with the controller's decoration; every other
            // scene draws the sample grid under every tab, whatever the tab's shell is.
            set_grid_source: Rc::new(move |handle: Option<Arc<TermHandle>>| {
                *grid_front.borrow_mut() = handle.clone();
                let source: Box<dyn GridSource> = match handle {
                    Some(h) if term => match grid_inputs.borrow().clone() {
                        Some(inputs) => Box::new(LiveSource::with_inputs(h, inputs)),
                        None => Box::new(LiveSource::new(h)),
                    },
                    _ if sample_stream => Box::new(StreamSource::new(SAMPLE_STREAM)),
                    _ => Box::new(SampleSource::new()),
                };
                grid.borrow_mut().set_source(source);
            }),
            raise: Rc::new(|| {}),
            quit: Rc::new(|| {}),
            hide: Rc::new(|| {}),
            launch_folders: Vec::new(),
            e2e: true,
            term: TermPorts {
                // The grid's own cell metrics, so a scene at 150% or 225% hits the cells it draws.
                metrics: {
                    let host = host.clone();
                    Rc::new(move || host.borrow().metrics())
                },
                stat: Arc::new(SceneStat),
                ..TermPorts::inert()
            },
        };
        let ctl = AppCtl::new(&app, deps);
        *inputs_cell.borrow_mut() = Some(ctl.borrow().decor_inputs());
        let scene = Self {
            win,
            app,
            ctl,
            host,
            queue,
            scale,
            phys,
            dir,
            front,
        };
        apply_chrome(&scene.app, &look);
        AppCtl::start_restore(&scene.ctl);
        scene.wait_restore()?;
        Ok(scene)
    }

    /// Wears another look in place: the chrome and the grid, nothing else moves (the pixel tests).
    #[cfg(test)]
    pub(crate) fn wear(&self, look: &AppliedLook) {
        apply_chrome(&self.app, look);
        apply_to_host(&mut self.host.borrow_mut(), look);
    }

    /// Hands every posted message to the controller, in order.
    fn pump(&self) {
        loop {
            let next = self
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            let Some(message) = next else { break };
            self.ctl.borrow_mut().on_msg(message);
        }
    }

    /// Waits until the restore has answered (its worker posts the answer).
    fn wait_restore(&self) -> Result<(), String> {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump();
            if self.ctl.borrow().model().restore_done() {
                return Ok(());
            }
            if Instant::now() >= until {
                return Err("the scene's restore did not answer within 5 s".to_string());
            }
            std::thread::sleep(Duration::from_millis(5)); // sync-ok: scene tooling, waits for the restore worker
        }
    }

    fn advance(&self, ms: u64) {
        SCENE_MS.with(|c| c.set(c.get() + ms));
        slint::platform::update_timers_and_animations();
    }

    fn hover(&self, x: f32, y: f32) {
        self.win.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(x, y),
        });
    }

    fn click(&self, (x, y): (f32, f32), button: PointerEventButton) {
        let position = LogicalPosition::new(x, y);
        self.win
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.win
            .dispatch_event(WindowEvent::PointerPressed { position, button });
        self.win
            .dispatch_event(WindowEvent::PointerReleased { position, button });
    }

    /// Types each char of `text` into whatever holds the keys.
    fn type_text(&self, text: &str) {
        for ch in text.chars() {
            let s = SharedString::from(ch.to_string());
            self.win
                .dispatch_event(WindowEvent::KeyPressed { text: s.clone() });
            self.win
                .dispatch_event(WindowEvent::KeyReleased { text: s });
        }
    }

    /// A css x from the strip's device-pixel x.
    fn strip_css(&self, phys: i32) -> f32 {
        STRIP_X + 1.0 / self.scale + phys as f32 / self.scale
    }

    fn tab_centre(&self, index: usize) -> Result<(f32, f32), String> {
        let row = self
            .app
            .get_tab_rows()
            .row_data(index)
            .ok_or_else(|| format!("the scene has no tab {index}"))?;
        Ok((self.strip_css(row.x + row.w / 2), BAR_MID_Y))
    }

    fn tab_id(&self, index: usize) -> Result<String, String> {
        let row = self
            .app
            .get_tab_rows()
            .row_data(index)
            .ok_or_else(|| format!("the scene has no tab {index}"))?;
        Ok(row.id.to_string())
    }

    fn plus_centre(&self) -> (f32, f32) {
        let g = self.app.get_strip_geom();
        (self.strip_css(g.plus_x + g.plus_w / 2), BAR_MID_Y)
    }

    fn more_centre(&self) -> (f32, f32) {
        let g = self.app.get_strip_geom();
        (self.strip_css(g.more_x + g.more_w / 2), BAR_MID_Y)
    }

    /// Lays the window out and paints it: the grid is drawn into the box the layout gave it.
    /// The grid image is premultiplied, as the live window declares it.
    pub(crate) fn render(&self) -> Vec<u8> {
        let (w, h) = self.phys;
        let mut pixels = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
        self.win.request_redraw();
        self.win.draw_if_needed(|r| {
            r.render(&mut pixels, w as usize);
        });
        {
            let mut host = self.host.borrow_mut();
            host.resize(
                self.app.get_grid_phys_width().max(0) as u32,
                self.app.get_grid_phys_height().max(0) as u32,
                self.app.get_scale(),
            );
            if let Some(grid) = host.render_rgba() {
                let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                    &grid.data,
                    grid.width,
                    grid.height,
                );
                self.app
                    .set_grid_image(Image::from_rgba8_premultiplied(buffer));
            }
        }
        self.win.request_redraw();
        self.win.draw_if_needed(|r| {
            r.render(&mut pixels, w as usize);
        });
        pixels.iter().flat_map(straight_rgba).collect()
    }

    /// Puts the window into the scene's state: pointer, keys, the clock and the controller's timers.
    /// The front shell the grid shows; a terminal scene cannot stage without one.
    fn term_handle(&self) -> Result<Arc<TermHandle>, String> {
        self.front
            .borrow()
            .clone()
            .ok_or_else(|| "the scene has no live tab".to_string())
    }

    /// A pointer position in css px from the grid's corner, at a cell (`right`: its right half),
    /// the way the grid's own hit testing reads it.
    fn cell_css(&self, col: usize, row: usize, right: bool) -> (f64, f64) {
        let m = self.host.borrow().metrics();
        let x = f64::from(m.padding)
            + col as f64 * f64::from(m.cell_w)
            + if right { 0.75 } else { 0.25 } * f64::from(m.cell_w);
        let y = f64::from(m.padding) + row as f64 * f64::from(m.cell_h) + f64::from(m.cell_h) / 2.0;
        let s = f64::from(self.scale);
        (x / s, y / s)
    }

    /// One pointer event over the front tab's grid (`kind`: 0 down, 1 up, 2 move).
    fn term_pointer(&self, kind: u8, button: u8, at: (f64, f64), shift: bool, alt: bool) {
        self.ctl.borrow_mut().grid_pointer(PointerIn {
            kind,
            button,
            x_css: at.0,
            y_css: at.1,
            ctrl: false,
            shift,
            alt,
        });
    }

    /// A press and a release at one place; `button` 2 is the right button (the menu).
    fn term_click(&self, at: (f64, f64), button: u8) {
        self.term_pointer(0, button, at, false, false);
        self.term_pointer(1, button, at, false, false);
    }

    /// A left drag from one place to another; `alt` makes it a block selection.
    fn term_drag(&self, from: (f64, f64), to: (f64, f64), alt: bool) {
        self.term_pointer(0, 0, from, false, alt);
        self.term_pointer(2, 0, to, false, alt);
        self.term_pointer(1, 0, to, false, alt);
    }

    /// A key press into the controller (chords first, then the terminal).
    fn term_key(&self, key: &str, code: &str, ctrl: bool) {
        let ev = KeyEvent {
            pressed: true,
            key: key.to_string(),
            code: code.to_string(),
            text: None,
            ctrl,
            shift: false,
            alt: false,
            meta: false,
        };
        self.ctl.borrow_mut().on_key(&ev);
    }

    /// Lets the terminal's workers answer (path questions, the copy) and its timers run, drawing
    /// each round so a question asked by a frame gets its answer.
    fn settle(&self, rounds: u32) {
        // A paint is costly at 300%, so the rounds only draw twice: before, and half way.
        self.render();
        for i in 0..rounds {
            self.pump();
            self.ctl.borrow_mut().tick();
            self.advance(50);
            std::thread::sleep(Duration::from_millis(5)); // sync-ok: scene tooling, waits for the terminal's workers
            if i == rounds / 2 {
                self.render();
            }
        }
        self.pump();
    }

    /// The terminal scenes: the shell is fed its text, then the controller is driven into the
    /// scene's state with the same pointer and key events a person would send.
    fn stage_term(&self, name: &str) -> Result<(), String> {
        let handle = self.term_handle()?;
        let cols = handle.with_emu_quiet(|e| e.cols());
        let text = term_text(name, cols).join("\r\n");
        handle.with_emu(|e| {
            e.feed(text.as_bytes());
        });
        let find = |query: &str| {
            self.term_key("f", "KeyF", true);
            self.type_text(query);
        };
        match name {
            "term-selection" => {
                // From inside the url on row 5, across to the end of "source" on row 6.
                let from = self.cell_css(23, 5, false);
                let to = self.cell_css(9, 6, true);
                self.settle(20);
                self.term_drag(from, to, false);
            }
            "term-selection-wide" => {
                // A block from the wide emoji on row 0 down three rows, to the right.
                let from = self.cell_css(13, 0, false);
                let to = self.cell_css(19, 2, true);
                self.settle(20);
                self.term_drag(from, to, true);
            }
            "term-find" => {
                find("err");
                self.settle(20);
                // The second match is the active one.
                self.app.invoke_find_next();
            }
            "term-find-none" => find("zzz"),
            "term-find-many" => {
                find("err");
                self.settle(60);
            }
            "term-find-empty" => self.term_key("f", "KeyF", true),
            "term-links" => {
                self.settle(20);
                // The pointer rests on the url: its underline is drawn in full.
                self.term_pointer(2, 0, self.cell_css(20, 1, false), false, false);
            }
            "term-menu-plain" => {
                self.settle(20);
                self.term_click(self.cell_css(3, 2, false), 2);
            }
            "term-menu-selection" => {
                let from = self.cell_css(0, 2, false);
                let to = self.cell_css(4, 2, true);
                self.settle(20);
                self.term_drag(from, to, false);
                // Right inside the selection: the menu offers Copy.
                self.term_click(self.cell_css(2, 2, false), 2);
            }
            "term-menu-link" => {
                self.settle(20);
                self.term_click(self.cell_css(20, 5, false), 2);
            }
            "term-menu-path" => {
                self.settle(20);
                self.term_click(self.cell_css(20, 6, false), 2);
            }
            "term-copied" => {
                let from = self.cell_css(0, 2, false);
                let to = self.cell_css(4, 2, true);
                self.settle(20);
                self.term_drag(from, to, false);
                self.term_key("c", "KeyC", true);
                // The copy runs on a worker; the badge shows once its answer is in.
                let until = Instant::now() + Duration::from_secs(5);
                while self.app.get_copied_phase() != 1 && Instant::now() < until {
                    self.pump();
                    self.ctl.borrow_mut().tick();
                    std::thread::sleep(Duration::from_millis(5)); // sync-ok: scene tooling, waits for the copy worker
                }
            }
            _ => return Err(format!("no terminal scene '{name}'")),
        }
        self.settle(2);
        Ok(())
    }

    fn stage(&self, name: &str) -> Result<(), String> {
        // The first paint lays the window out, so the pointer and the focus find their targets.
        self.render();
        if is_term_scene(name) {
            return self.stage_term(name);
        }
        match name {
            "strip-hover" => {
                let (x, y) = self.tab_centre(1)?;
                self.hover(x, y);
            }
            "strip-list" => {
                let (x, y) = self.more_centre();
                self.click((x, y), PointerEventButton::Left);
                // "2" keeps the hidden Tab20..Tab29: the list shows rows, not its empty line.
                self.type_text("2");
            }
            "sidebar-find" => {
                self.app.invoke_focus_sidebar_find();
                self.type_text("pri");
            }
            "sidebar-collapsed" => {
                // The pointer at the left edge opens the slid-out sidebar after its delay, and
                // the slide (240 ms) runs to its end.
                self.ctl.borrow_mut().on_cursor(8.0, 120.0);
                self.advance(100);
                self.ctl.borrow_mut().tick();
                self.advance(240);
            }
            "menu-places" => {
                // The places menu hangs from a right-click on the + button.
                let at = self.plus_centre();
                self.click(at, PointerEventButton::Right);
            }
            "menu-tab" => {
                let at = self.tab_centre(1)?;
                self.click(at, PointerEventButton::Right);
            }
            "dialog-close" => {
                // A Claude tab mid-answer: closing it asks first.
                let id = self.tab_id(0)?;
                let mut ctl = self.ctl.borrow_mut();
                ctl.model_mut().set_agent(&id, Some(AgentKind::Claude));
                ctl.set_working(&id, Some(scene_now()));
                ctl.request_close(&id);
            }
            "tip" => {
                let (x, y) = self.tab_centre(1)?;
                // The delay (1 s) passes, then the tip fades in: the fade runs on the clock too.
                self.hover(x, y);
                self.advance(1100);
                self.ctl.borrow_mut().tick();
                self.advance(300);
            }
            _ => {}
        }
        self.pump();
        Ok(())
    }
}

/// `--scene <name>`: the window in one fixed state, drawn to `--screenshot`. An unknown name is an
/// argument error (exit 1) and nothing is created.
pub fn run_scene(args: &Args) -> Result<(), String> {
    let name = args.scene.as_deref().unwrap_or_default();
    if !SCENES.contains(&name) {
        return Err(format!(
            "unknown scene '{name}': use one of {}",
            SCENES.join(", ")
        ));
    }
    let path = args
        .screenshot
        .as_ref()
        .ok_or_else(|| "--scene needs --screenshot <path>".to_string())?;
    let (saved, prep) = setup(name);
    let scene = Scene::build_with(
        args,
        saved,
        prep,
        name == "theme-sample",
        is_term_scene(name),
    )?;
    scene.stage(name)?;
    let rgba = scene.render();
    write_png(path, scene.phys.0, scene.phys.1, &rgba)?;
    println!("screenshot: {}", path.display());
    Ok(())
}
