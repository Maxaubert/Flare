//! `--screenshot` and `--bench-frames`: the window is drawn by Slint's software renderer into a
//! buffer, and no OS window is ever created or shown.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use flare_agent::kinds::{AgentKind, DetectedAgent};
use flare_render::gpu::headless_device;
use flare_render::grid::Grid;
use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use flare_settings::kv::PrefStore;
use flare_settings::look::{AgentIndicator, LookStore};
use flare_settings::recent_roots::{remember_root, toggle_pin};
use flare_settings::sidebar::{SidebarMode, set_sidebar_mode_stored};
use flare_settings::tab_layout::{TabLayout, set_tab_layout};
use flare_tabs::restore::{SavedTab, SavedTabs, SessionLookup, tabs_json};
use flare_tabs::tabs_store::TabsStore;
use flare_term::emulator::TermEvent;
use flare_term::fake::{FakeShells, FakeSpawner};
use flare_term::handle::{StartOptions, TermHandle};
use flare_term::pty::PortableSpawner;
use flare_term::session::{RealShells, SessionManager, SessionOptions};
#[cfg(test)]
use flare_theme::look::AppliedLook;
use flare_win::taskbar::{badge_px, compose_disc, text_mask};
use flare_win::window_state::WindowStateStore;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{
    ComponentHandle, Image, LogicalPosition, Model, PhysicalSize, Rgba8Pixel, SharedPixelBuffer,
    SharedString,
};

use crate::appctl::{AppCtl, Deps, Post, TermPorts, UiMsg};
use crate::chrome_apply::{apply_chrome, apply_to_host};
use crate::config::{Args, RendererChoice, SettingsOpen};
use crate::host::{GridHost, GridPainter, GridSource, SampleSource};
use crate::live::{Live, LiveSource, shutdown};
use crate::memory::process_memory;
use crate::settings_ctl::SettingsCtl;
use crate::settings_scene;
use crate::tab_host::TabHost;
use crate::term_ui::{DecorInputs, PointerIn};
use crate::theme_scene::{SAMPLE_STREAM, StreamSource, scene_font, scene_look};
use crate::update_ctl::{PostEvents, UpdateCtl, UpdateDeps, UpdateMsg};
use crate::{AppWindow, MarkMotion, SetKind, SettingsBridge};
use flare_settings::pages::DEFAULT_PAGE;
use flare_settings::window_colours::ACCENT;
use flare_term::keys::KeyEvent;

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
        remote: None,
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
pub(crate) fn straight_rgba(p: &PremultipliedRgbaColor) -> [u8; 4] {
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
pub const SCENES: [&str; 60] = [
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
    "marks-ring",
    "marks-bar",
    "marks-bar-flow",
    "marks-sidebar",
    "marks-sidebar-bar",
    "marks-icons",
    "marks-overflow",
    "marks-tip",
    "marks-skeleton",
    "marks-close",
    "badge",
    "settings",
    "update-chip",
    "update-chip-notice",
    "update-window",
    "update-window-running",
    "update-window-installing",
    "update-window-notice",
    "update-window-small",
    "update-window-empty",
    // The dictation scenes (M8, Task 35): the pill in the window, the Dictation page, the tab mark and the widget.
    "dictation-pill-listening",
    "dictation-pill-starting",
    "dictation-pill-transcribing",
    "dictation-pill-message",
    "dictation-mark-strip",
    "dictation-mark-sidebar",
    "dictation-page-top",
    "dictation-page-models",
    "dictation-page-ask",
    "dictation-page-end",
    "dictation-widget-listening",
    "dictation-widget-starting",
    "dictation-widget-pick",
    "dictation-widget-sent",
    "dictation-widget-warning",
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

/// Moves the scene clock on and runs the animations and timers that came due (the component scenes).
pub(crate) fn advance_scene_clock(ms: u64) {
    SCENE_MS.with(|c| c.set(c.get() + ms));
    slint::platform::update_timers_and_animations();
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
pub(crate) fn offer_scene_window(win: &Rc<MinimalSoftwareWindow>) -> Result<(), String> {
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
                remote: None,
            })
            .collect(),
        active,
    }
}

/// The strip along the top. The scenes were drawn with it; vertical is the default since 2026-11-11,
/// so a scene that wants the strip says so.
fn horizontal(p: &PrefStore) {
    set_tab_layout(p, TabLayout::Horizontal);
}

fn vertical(p: &PrefStore) {
    set_tab_layout(p, TabLayout::Vertical);
}

fn collapsed(p: &PrefStore) {
    vertical(p);
    set_sidebar_mode_stored(p, SidebarMode::Collapsed);
}

/// Two pinned places and two recent ones: the start screen.
fn start_places(p: &PrefStore) {
    horizontal(p);
    toggle_pin(p, &github("Atlas"));
    toggle_pin(p, &github("Flare"));
    remember_root(p, &github("Sonara"));
    remember_root(p, &github("notes"));
}

/// Two pinned places and three recent ones: the + button's right-click menu lists them.
fn menu_places(p: &PrefStore) {
    horizontal(p);
    toggle_pin(p, &github("Atlas"));
    toggle_pin(p, &github("Flare"));
    remember_root(p, &github("Sonara"));
    remember_root(p, &github("notes"));
    remember_root(p, &github("Docs"));
}

/// The tabs a scene restores and the preferences it sets before the window is made.
pub(crate) fn setup(name: &str) -> (SavedTabs, fn(&PrefStore)) {
    let three = [github("Atlas"), github("Flare"), github("Sonara")];
    // Thirty tabs: at 1000 px the strip shows icons and the +N control (sixteen all fit).
    let many: Vec<String> = (1..=30).map(|i| github(&format!("Tab{i:02}"))).collect();
    // Two rows share the name "Flare" (two folders): the sidebar must show both.
    let side = [
        github("api"),
        github("web"),
        github("Atlas"),
        github("Flare"),
        r"C:\Users\Admin\Documents\Claude\zz-Misc\Flare".to_string(),
        github("notes"),
    ];
    match name {
        "strip-many" | "strip-list" => (saved(&many, 0), horizontal),
        "sidebar-open" | "sidebar-find" => (saved(&side, 1), vertical),
        "sidebar-collapsed" => (saved(&side, 1), collapsed),
        "start" => (saved(&[], 0), start_places),
        "menu-places" => (saved(&three, 0), menu_places),
        "dialog-close" => (saved(&three, 0), horizontal),
        // M3's strip-3 window with a single tab.
        "theme-sample" => (saved(&[github("Flare")], 0), horizontal),
        // The Settings tab alone, as a fresh Prism Terminal profile shows it, in a fixed store
        // (M7, Task 51).
        "settings" => (saved(&[], 0), crate::settings_scene::prep),
        // The Dictation page alone, with dictation on and the Base model in use (M8, Task 35).
        name if is_dictation_page_scene(name) => (saved(&[], 0), settings_scene::prep_dictation),
        // The pill over a Flare tab's terminal.
        name if is_dictation_pill_scene(name) => (saved(&[github("Flare")], 0), horizontal),
        // The terminal scenes: one live tab in the Flare folder, its shell fed with the text.
        name if is_term_scene(name) => (saved(&[github("Flare")], 0), horizontal),
        name if is_mark_scene(name) => marks_setup(name),
        // The update scenes: one tab; the update controller is made over it when the scene stages.
        name if is_update_scene(name) => (saved(&[github("Flare")], 0), horizontal),
        _ => (saved(&three, 0), horizontal),
    }
}

// ---- the update scenes (M9, Task 40) -------------------------------------------------------------

/// The scenes that show the update chip, its notice and the update window.
fn is_update_scene(name: &str) -> bool {
    name.starts_with("update-")
}

/// The arguments a scene is drawn with: the window size a scene needs is part of the scene, not of
/// the command line (sixteen tabs overflow a 700 px strip; the small update window is 420 high).
pub(crate) fn scene_args(name: &str, args: &Args) -> Args {
    let mut args = args.clone();
    if name == "marks-overflow" {
        args.size.0 = 700;
    }
    if name == "update-window-small" {
        args.size = (1000, 420);
    }
    if is_dictation_page_scene(name) {
        args.settings_page = Some("dictation".to_string());
    }
    args
}

// ---- the dictation scenes (M8, Task 35) ----------------------------------------------------------

/// The scenes that draw the Dictation page through the real Settings frame.
pub(crate) fn is_dictation_page_scene(name: &str) -> bool {
    name.starts_with("dictation-page-")
}

/// The scenes that draw the pill over the front tab's terminal, in the real window.
pub(crate) fn is_dictation_pill_scene(name: &str) -> bool {
    name.starts_with("dictation-pill-")
}

/// How far the Dictation page is scrolled (css px) when the scene shows the models card.
pub(crate) const DICTATION_MODELS_SCROLL: f32 = 520.0;
/// More than the page is tall: the view clamps it to the foot.
pub(crate) const DICTATION_END_SCROLL: f32 = 5000.0;

/// The version the update scenes run as: the offer is the next minor, `0.56.0`.
const UPDATE_SCENE_CURRENT: &str = "0.55.6";

/// A release body of `n` entries, each long enough to wrap in the window.
fn update_scene_notes(n: usize) -> String {
    let mut lines = vec!["## What's Changed".to_string()];
    for i in 1..=n {
        lines.push(format!(
            "* Change number {i}: a title long enough that it wraps onto a second line in the window by @Maxaubert in https://github.com/Maxaubert/Flare/pull/{i}"
        ));
    }
    lines.join("\n")
}

/// The scenes never wait: no install runs in them.
struct SceneNoWait;

impl flare_update::preview::Waiter for SceneNoWait {
    fn wait(&self, _ms: u64) {}
}
// ---- the marks scenes (M6, Task 33) -----------------------------------------------------------

/// The scenes that show the agent marks: the strip, the sidebar, the +N list, the tooltip, the
/// resume skeleton and the close question.
fn is_mark_scene(name: &str) -> bool {
    name.starts_with("marks-")
}

/// The tabs of the mark scenes: a Claude tab, a Codex tab, shells, and `site`, a Claude tab that
/// comes back (its saved agent resumes a session, so it shows the loading ring).
fn mark_tab(folder: &str, agent: Option<AgentKind>) -> SavedTab {
    SavedTab {
        cwd: github(folder),
        agent,
        remote: None,
    }
}

/// Sets the Bar mark (the stored word `minimal`) and nothing else.
fn mark_bar(p: &PrefStore) {
    horizontal(p);
    LookStore::new(p.clone()).set_agent_indicator(AgentIndicator::Minimal);
}

fn mark_bar_vertical(p: &PrefStore) {
    mark_bar(p);
    vertical(p);
}

/// The saved strip and the preferences of each mark scene.
fn marks_setup(name: &str) -> (SavedTabs, fn(&PrefStore)) {
    let claude = Some(AgentKind::Claude);
    let codex = Some(AgentKind::Codex);
    match name {
        "marks-ring" | "marks-bar" | "marks-bar-flow" => {
            let tabs = vec![
                mark_tab("Atlas", claude),
                mark_tab("Flare", None),
                mark_tab("Sonara", None),
                mark_tab("api", None),
                mark_tab("docs", None),
                mark_tab("site", claude),
                mark_tab("tools", None),
            ];
            let prep: fn(&PrefStore) = if name == "marks-ring" {
                horizontal
            } else {
                mark_bar
            };
            (SavedTabs { tabs, active: 6 }, prep)
        }
        "marks-sidebar" | "marks-sidebar-bar" => {
            let tabs = vec![
                mark_tab("Atlas", claude),
                mark_tab("Flare", codex),
                mark_tab("Sonara", None),
                mark_tab("api", None),
                mark_tab("docs", None),
            ];
            let prep: fn(&PrefStore) = if name == "marks-sidebar" {
                vertical
            } else {
                mark_bar_vertical
            };
            (SavedTabs { tabs, active: 0 }, prep)
        }
        "marks-icons" => {
            // Clawd, the OpenAI mark (Flare, a Codex tab), the prompt (docs, a plain shell) and,
            // staged, the cog of Settings.
            let tabs = vec![
                mark_tab("Atlas", claude),
                mark_tab("Flare", codex),
                mark_tab("Sonara", None),
                mark_tab("docs", None),
            ];
            (SavedTabs { tabs, active: 0 }, horizontal)
        }
        "marks-overflow" => {
            // Sixteen tabs at 700 px: the strip shows icons, one name and the +N control.
            let tabs = (1..=16)
                .map(|i| mark_tab(&format!("Tab{i:02}"), None))
                .collect();
            (SavedTabs { tabs, active: 0 }, horizontal)
        }
        "marks-tip" => {
            let tabs = vec![
                mark_tab("Atlas", None),
                mark_tab("Flare", None),
                mark_tab("Sonara", None),
                mark_tab("api", None),
            ];
            (SavedTabs { tabs, active: 0 }, horizontal)
        }
        "marks-skeleton" => {
            // A Claude tab coming back, in front: the skeleton draws over its grid.
            let tabs = vec![mark_tab("Atlas", claude), mark_tab("Flare", claude)];
            (SavedTabs { tabs, active: 1 }, horizontal)
        }
        // marks-close: one working Claude tab. Saved as a plain shell, so it is not coming back to
        // a session: the question stands over the shell, not over a resume skeleton.
        _ => (
            SavedTabs {
                tabs: vec![mark_tab("Atlas", None)],
                active: 0,
            },
            horizontal,
        ),
    }
}

/// Which tab wears which agent state, by the hook signal the agent would send. The folder is
/// under the Github folder; the signal is the one the shell's output carries.
fn mark_plan(name: &str) -> Vec<(String, &'static str)> {
    let working = OSC_WORKING;
    let question = OSC_QUESTION;
    let done = OSC_DONE;
    let failed = OSC_FAILED;
    let plan: Vec<(&str, &'static str)> = match name {
        "marks-ring" | "marks-bar" | "marks-bar-flow" => vec![
            ("Atlas", working),
            ("Flare", question),
            ("Sonara", done),
            ("api", failed),
            ("tools", working),
        ],
        "marks-sidebar" | "marks-sidebar-bar" => vec![
            ("Atlas", working),
            ("Flare", question),
            ("Sonara", done),
            ("api", failed),
        ],
        "marks-icons" => vec![("Atlas", working), ("Flare", question), ("Sonara", done)],
        // The hidden tabs carry the states: the +N list shows them and the +N control shows a dot
        // of each mark colour.
        // More tabs need you than the strip has room for: the hidden ones are rows of the list and
        // dots on the +N control, one per mark colour.
        "marks-overflow" => vec![
            ("Tab04", question),
            ("Tab05", done),
            ("Tab06", failed),
            ("Tab07", question),
            ("Tab08", done),
            ("Tab09", failed),
            ("Tab10", working),
            ("Tab11", question),
            ("Tab12", failed),
            ("Tab13", question),
            ("Tab14", done),
            ("Tab15", failed),
            ("Tab16", question),
        ],
        "marks-tip" => vec![("Atlas", working), ("Flare", question), ("api", failed)],
        "marks-close" => vec![("Atlas", working)],
        _ => Vec::new(),
    };
    plan.into_iter()
        .map(|(folder, osc)| (github(folder), osc))
        .collect()
}

const OSC_WORKING: &str = "\x1b]777;flare-agent;state=working\x07";
const OSC_QUESTION: &str = "\x1b]777;flare-agent;state=question\x07";
const OSC_DONE: &str = "\x1b]777;flare-agent;state=done\x07";
const OSC_FAILED: &str = "\x1b]777;flare-agent;state=failed;kind=rate_limit\x07";

/// The one session every folder has: a Claude tab coming back resumes it (the loading ring).
const SCENE_SESSION: &str = "3f2a9c1e-7b4d-4e6f-8a1b-2c3d4e5f6a7b";

/// The scenes' session lookup: one session in every folder, never read from the disk.
struct SceneSessions;

impl SessionLookup for SceneSessions {
    fn sessions(&self, _cwd: &str) -> Vec<String> {
        vec![SCENE_SESSION.to_string()]
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

/// The PowerShell session most scenes show: ten rows, the url on row 5, the path on row 6 (between the file-link marks a
/// session writes, `file_marks`).
fn term_rows() -> Vec<String> {
    vec![
        format!("{PROMPT}cargo nextest run"),
        "    Starting 214 tests across 12 binaries".to_string(),
        "error[E0432]: unresolved import `flare_term::nope`".to_string(),
        "warning: unused variable `x`".to_string(),
        r"    --> crates\flare\src\main.rs:12:9".to_string(),
        format!("    log at {RUN_URL}"),
        format!("    source \u{27E6}{README}\u{27E7}"),
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
            format!("    source \u{27E6}{README}\u{27E7}"),
            // The url starts 20 cells from the right edge, so it wraps over two rows.
            format!("{}{RUN_URL}", " ".repeat(cols.saturating_sub(20))),
            PROMPT.to_string(),
        ],
        _ => term_rows(),
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
    /// The scene's own in-memory store (the Settings scene hands it to its controller).
    prefs: PrefStore,
    /// The update controller of an update scene (kept: it owns the window's notes model).
    update: RefCell<Option<Rc<RefCell<UpdateCtl>>>>,
    /// The shell the grid shows (the terminal scenes feed it and read its cells).
    front: Rc<RefCell<Option<Arc<TermHandle>>>>,
    /// Every fake shell the tabs spawned (the mark scenes feed them the agents' signals).
    spawner: Arc<FakeSpawner>,
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

        let spawner = FakeSpawner::new();
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
            host: TabHost::new(
                spawner.clone(),
                FakeShells::new(),
                prefs.clone(),
                waker,
                None,
            ),
            lookup: Arc::new(SceneSessions),
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
            starter_folders: Rc::new(|_| {}),
            launch_folders: Vec::new(),
            e2e: true,
            term: TermPorts {
                // The grid's own cell metrics, so a scene at 150% or 225% hits the cells it draws.
                metrics: {
                    let host = host.clone();
                    Rc::new(move || host.borrow().metrics())
                },
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
            prefs,
            update: RefCell::new(None),
            front,
            spawner,
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
        // A frame as the clock leaves and one where it lands: a sidebar slide starts at its first frame.
        self.frame();
        SCENE_MS.with(|c| c.set(c.get() + ms));
        slint::platform::update_timers_and_animations();
        self.frame();
    }

    /// A frame begins (the live window's RedrawRequested): the sidebar's slide takes its place.
    fn frame(&self) {
        if let Ok(mut c) = self.ctl.try_borrow_mut() {
            c.on_frame();
        }
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
        self.frame();
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

    /// The Settings scene (M7, Task 51): the Settings tab opened over the restored tab, on the
    /// page and with the overlay the flags name. The controller is made here, over the scene's own
    /// in-memory store and fixed ports, and everything is driven through its own callbacks.
    pub(crate) fn stage_settings(&self, args: &Args) -> Result<(), String> {
        let scene = args.scene.as_deref().unwrap_or_default();
        if is_dictation_page_scene(scene) {
            // Base in use, Small at half its download, the rest not downloaded; the question open on the third scene.
            crate::dictation_ctl::scene_page(
                &["base"],
                Some(("small", 50)),
                (scene == "dictation-page-ask").then_some("parakeet-v3"),
            );
        }
        let post: Post = {
            let queue = self.queue.clone();
            Arc::new(move |m| {
                queue
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back(m);
            })
        };
        let ports = settings_scene::deps(&self.prefs, Arc::clone(&post), Rc::new(scene_now));
        let settings = SettingsCtl::new(&self.app, ports);
        // Settings > Machines shows the scene's machines (remote sessions PR 7): no file, no link.
        settings
            .borrow_mut()
            .set_machines(crate::machines_page::MachinesPage::new(
                Arc::new(crate::machines_page::SceneMachinesIo),
                post,
            ));
        {
            let mut ctl = self.ctl.borrow_mut();
            ctl.set_settings(settings);
            ctl.model_mut().open_settings();
            ctl.refresh();
        }
        // The shell list and the Explorer verb answer on workers: wait for both.
        let until = Instant::now() + Duration::from_secs(5);
        let mut answers = 0;
        while answers < 2 {
            let next = self
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            match next {
                Some(m) => {
                    answers += usize::from(matches!(m, UiMsg::Settings(_)));
                    self.ctl.borrow_mut().on_msg(m);
                }
                None if Instant::now() >= until => {
                    return Err("the settings workers did not answer within 5 s".to_string());
                }
                None => std::thread::sleep(Duration::from_millis(5)), // sync-ok: scene tooling, waits for the settings workers
            }
        }
        // One paint lays the view out and tells the controller its frame.
        self.settle(4);
        let page = args
            .settings_page
            .clone()
            .unwrap_or_else(|| DEFAULT_PAGE.to_string());
        let bridge = self.app.global::<SettingsBridge>();
        bridge.invoke_page_chosen(page.as_str().into());
        self.settle(4);
        if page == "machines" {
            // The page's read and its checks answer on workers: take them in before the picture.
            let until = Instant::now() + Duration::from_secs(2);
            while Instant::now() < until {
                let next = self
                    .queue
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .pop_front();
                match next {
                    Some(m) => self.ctl.borrow_mut().on_msg(m),
                    None => std::thread::sleep(Duration::from_millis(10)), // sync-ok: scene tooling, waits for the page's workers
                }
            }
            self.settle(4);
        }
        if let Some(query) = &args.settings_find {
            bridge.set_find_text(query.as_str().into());
            bridge.invoke_find_edited(query.as_str().into());
            self.settle(4);
        }
        let css = (
            f64::from(self.phys.0) / f64::from(self.scale),
            f64::from(self.phys.1) / f64::from(self.scale),
        );
        let anchor = |kind: SetKind, what: &str| -> Result<(String, [f32; 4]), String> {
            let id = settings_scene::first_row_of(&self.app, kind)
                .ok_or_else(|| format!("the {page} page has no {what} to open"))?;
            let place = || {
                settings_scene::control_anchor(&self.app, css, f64::from(self.scale), &id, kind)
                    .ok_or_else(|| "the settings view has not been laid out".to_string())
            };
            let mut at = place()?;
            // A control below the fold is scrolled to the middle first, as a click would.
            if let Some(y) = settings_scene::reveal_scroll(&self.app, css, at) {
                bridge.set_scroll_y(y);
                self.settle(2);
                at = place()?;
            }
            Ok((id, at.map(|v| v as f32)))
        };
        let cards = bridge.get_wall_cards();
        let card_id = |chosen: bool, edit: bool| -> Result<SharedString, String> {
            (0..cards.row_count())
                .filter_map(|i| cards.row_data(i))
                .find(|c| if edit { c.edit } else { c.on == chosen })
                .or_else(|| {
                    (0..cards.row_count())
                        .filter_map(|i| cards.row_data(i))
                        .find(|c| c.on == chosen)
                })
                .map(|c| c.id)
                .ok_or_else(|| format!("the {page} page has no theme wall"))
        };
        match args.settings_open {
            SettingsOpen::None => {}
            SettingsOpen::Select => {
                let (id, a) = anchor(SetKind::Select, "select")?;
                bridge.invoke_select_open_request(id.as_str().into(), a[0], a[1], a[2], a[3]);
            }
            SettingsOpen::Picker => {
                let (id, a) = anchor(SetKind::Colour, "colour")?;
                // The scene stands for a click on the swatch (the Prism Terminal capture clicks).
                self.app
                    .global::<crate::PickerOpening>()
                    .set_by_pointer(true);
                bridge.invoke_colour_swatch(id.as_str().into(), a[0], a[1], a[2], a[3]);
            }
            SettingsOpen::Editor => bridge.invoke_wall_edit(card_id(true, true)?),
            SettingsOpen::Ask => {
                ACCENT.set(&self.prefs, Some(settings_scene::DIRTY_ACCENT));
                self.settle(4);
                bridge.invoke_wall_pick(card_id(false, false)?);
                if !bridge.get_ask_open() {
                    return Err("the switch question did not open".to_string());
                }
            }
            SettingsOpen::Help => self.term_key("F1", "F1", false),
            SettingsOpen::WallExpanded => {
                if cards.row_count() == 0 {
                    return Err(format!("the {page} page has no theme wall"));
                }
                bridge.invoke_wall_toggle();
                // The opened wall reaches below the first screen: both captures scroll the same
                // distance, so the cards the toggle revealed are in the picture.
                bridge.set_scroll_y(settings_scene::WALL_SCROLL);
            }
        }
        match scene {
            "dictation-page-models" | "dictation-page-ask" => {
                bridge.set_scroll_y(DICTATION_MODELS_SCROLL)
            }
            // Past the end: the view stops at its foot, with the GPU card and From any app.
            "dictation-page-end" => bridge.set_scroll_y(DICTATION_END_SCROLL),
            _ => {}
        }
        // Long enough for the help panel's slide (460 ms) and the flashes to be over.
        self.settle(30);
        Ok(())
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
                // Back from the first match wraps to the last, past the thousand that are
                // counted: the readout is `1000+` (xterm's -1 index).
                self.app.invoke_find_prev();
                self.settle(4);
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

    /// Writes `text` to the fake shell that was spawned in `cwd` (its output is the agent's word).
    /// The tabs' shells are spawned as the window settles, so on a slow machine (the release
    /// runner, 2026-10-21) a later tab's shell may not exist yet: wait for it, never fail at once.
    fn feed(&self, cwd: &str, text: &str) -> Result<(), String> {
        let until = Instant::now() + Duration::from_secs(30);
        let shell = loop {
            if let Some(m) = self
                .spawner
                .made()
                .into_iter()
                .find(|m| m.spec().cwd == cwd)
            {
                break m;
            }
            if Instant::now() >= until {
                return Err(format!("the scene has no shell in {cwd}"));
            }
            self.pump();
            self.ctl.borrow_mut().tick();
            std::thread::sleep(Duration::from_millis(5)); // sync-ok: scene tooling, waits for the tab's shell
        };
        shell.feed(text.as_bytes());
        Ok(())
    }

    /// Whether every fed tab's row shows a mark: as many rows wear a state as the plan gives one.
    /// (The loading ring of a Claude tab coming back is not a state.)
    fn marks_shown(&self, plan: &[(String, &'static str)]) -> bool {
        let rows = self.app.get_tab_rows();
        let marked = (0..rows.row_count())
            .filter(|&i| rows.row_data(i).is_some_and(|r| r.mark_state != 0))
            .count();
        marked >= plan.len()
    }

    /// The mark scenes: the agents' signals, the menu or tooltip the scene shows, then the marks'
    /// motion pinned to the named phase (spinner at 0 degrees, the run bar at half, the rainbow at
    /// flow 0 or 57 px). Nothing advances the clock after that, so the phase holds.
    fn stage_marks(&self, name: &str) -> Result<(), String> {
        // The Codex tab is told by the process poll before its signal arrives, as a real one is.
        if matches!(name, "marks-icons" | "marks-sidebar" | "marks-sidebar-bar") {
            let codex = github("Flare");
            let rows = self.app.get_tab_rows();
            let id = (0..rows.row_count())
                .filter_map(|i| rows.row_data(i))
                .find(|r| r.tip.as_str() == codex)
                .map(|r| r.id.to_string())
                .ok_or("the scene has no Codex tab")?;
            let mut ctl = self.ctl.borrow_mut();
            ctl.hub_mut()
                .queue_poll_answer(&id, true, Some(DetectedAgent::Codex));
            ctl.hub_mut().drain_poll();
            ctl.refresh();
        }
        for (cwd, osc) in mark_plan(name) {
            self.feed(&cwd, osc)?;
        }
        // The shell's output reaches the window on its own thread: wait until every fed tab shows
        // a state (the Claude tabs coming back show the loading ring, not one of these).
        let plan = mark_plan(name);
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump();
            if self.marks_shown(&plan) {
                break;
            }
            if Instant::now() >= until {
                return Err(format!(
                    "the agent states did not reach the window for {name}"
                ));
            }
            std::thread::sleep(Duration::from_millis(5)); // sync-ok: scene tooling, waits for the shells' output
        }
        match name {
            "marks-icons" | "marks-sidebar" | "marks-sidebar-bar" => {
                // The cog: the Settings tab sits in the strip beside the agent tabs.
                self.ctl.borrow_mut().model_mut().open_settings();
                self.ctl.borrow_mut().refresh();
            }
            "marks-overflow" => {
                let at = self.more_centre();
                self.click(at, PointerEventButton::Left);
            }
            "marks-tip" => {
                // The failed tab's tooltip: the delay passes and the tip fades in.
                let (x, y) = self.tab_centre(3)?;
                self.hover(x, y);
                self.advance(1100);
                self.ctl.borrow_mut().tick();
                self.advance(300);
            }
            "marks-close" => {
                // A working Claude tab, two minutes into its answer: closing it asks first.
                let id = self.tab_id(0)?;
                SCENE_MS.with(|c| c.set(120_000));
                self.ctl.borrow_mut().request_close(&id);
            }
            _ => {}
        }
        // The close X slides out of a tab that just got a ring (150 ms): let it finish, as it would
        // on screen, before the motion is pinned.
        self.advance(200);
        self.pump();
        let g = self.app.global::<MarkMotion>();
        g.set_still(false);
        g.set_spin(0.0);
        g.set_breathe(1.0);
        g.set_pulse(1.0);
        g.set_run(0.5);
        g.set_flow(if name == "marks-bar-flow" { 57.0 } else { 0.0 });
        Ok(())
    }

    /// The pill scenes: the window's own pill properties, as `dictation_ctl` feeds them, over the front tab. The
    /// spinner is pinned to its start and the clock runs on so the pill's fade and the bars have settled.
    fn stage_pill(&self, name: &str) -> Result<(), String> {
        let app = &self.app;
        match name {
            "dictation-pill-listening" => {
                app.set_dict_phase(1);
                app.set_dict_live_text("the newest words of a long sentence".into());
                app.set_dict_level(0.5);
            }
            "dictation-pill-starting" => {
                app.set_dict_phase(1);
                app.set_dict_starting(true);
            }
            "dictation-pill-transcribing" => app.set_dict_phase(2),
            "dictation-pill-message" => {
                app.set_dict_phase(3);
                app.set_dict_message("Typed into the tab in front".into());
            }
            _ => return Err(format!("no dictation pill scene '{name}'")),
        }
        self.advance(500);
        self.pump();
        let g = self.app.global::<MarkMotion>();
        g.set_still(false);
        g.set_spin(0.0);
        Ok(())
    }

    /// The update scenes: the update controller over a fake engine, told an offer and then driven
    /// through the window's own callbacks (the chip click, Install), exactly as a person would.
    fn stage_update(&self, name: &str) -> Result<(), String> {
        use flare_update::engine::{CondvarSleeper, Engine, Mode};
        use flare_update::feed::FakeFeed;
        use flare_update::types::Offer;

        let post: Post = {
            let queue = self.queue.clone();
            Arc::new(move |m| {
                queue
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back(m);
            })
        };
        let engine = Engine::new(
            Mode::Off,
            Arc::new(FakeFeed::new()),
            UPDATE_SCENE_CURRENT,
            None,
            Arc::new(PostEvents { post: post.clone() }),
            Arc::new(CondvarSleeper::new()),
            Arc::new(SceneNoWait),
        );
        let ctl = UpdateCtl::new(
            &self.app,
            UpdateDeps {
                engine,
                post,
                current_version: UPDATE_SCENE_CURRENT.to_string(),
                ask_install: Rc::new(|answer| answer(true)),
                agree: Arc::new(|| {}),
                // The install worker never runs: the scene tells the controller what it saw.
                spawn: Rc::new(|_job| {}),
                e2e_dir: None,
                quit: Rc::new(|| {}),
            },
        );
        let version = flare_update::preview::preview_version(UPDATE_SCENE_CURRENT);
        let notes = match name {
            "update-window-small" => update_scene_notes(25),
            "update-window-empty" => String::new(),
            _ => flare_update::preview::SAMPLE_NOTES.to_string(),
        };
        // The failure under the chip is a real offer's; every other scene shows the preview.
        let mock = name != "update-chip-notice";
        {
            let mut c = ctl.borrow_mut();
            let css = (
                self.phys.0 as f32 / self.scale,
                self.phys.1 as f32 / self.scale,
            );
            c.on_scale_or_size(css.0, css.1, self.scale);
            c.on_msg(UpdateMsg::Offer(Offer {
                version,
                notes,
                mock,
                token: 1,
            }));
        }
        match name {
            "update-chip" => {}
            "update-chip-notice" => {
                let mut c = ctl.borrow_mut();
                c.install_pressed();
                c.hide();
                c.on_msg(UpdateMsg::Settled(false));
            }
            "update-window" | "update-window-small" | "update-window-empty" => {
                ctl.borrow_mut().chip_clicked();
            }
            "update-window-running" | "update-window-installing" | "update-window-notice" => {
                let mut c = ctl.borrow_mut();
                c.chip_clicked();
                c.install_pressed();
                match name {
                    "update-window-running" => c.on_msg(UpdateMsg::Progress(42)),
                    "update-window-installing" => c.on_msg(UpdateMsg::Progress(100)),
                    _ => c.on_msg(UpdateMsg::Settled(false)),
                }
            }
            _ => return Err(format!("no update scene '{name}'")),
        }
        *self.update.borrow_mut() = Some(ctl);
        // The window fades in (200 ms) and the bar fills (300 ms): let both finish before the picture.
        self.advance(600);
        self.pump();
        Ok(())
    }

    pub(crate) fn stage(&self, name: &str) -> Result<(), String> {
        // The first paint lays the window out, so the pointer and the focus find their targets.
        self.render();
        if is_term_scene(name) {
            return self.stage_term(name);
        }
        if is_mark_scene(name) {
            return self.stage_marks(name);
        }
        if is_update_scene(name) {
            return self.stage_update(name);
        }
        if is_dictation_pill_scene(name) {
            return self.stage_pill(name);
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
    // `--scene list` prints every scene name, one per line (the DPI matrix reads them from here).
    if name == "list" {
        for scene in SCENES {
            println!("{scene}");
        }
        return Ok(());
    }
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
    if name == "badge" {
        return write_badge_sheet(path);
    }
    if crate::dictation_scenes::is_component_scene(name) {
        let (w, h, rgba) = crate::dictation_scenes::render(name, args.scale)?;
        write_png(path, w, h, &rgba)?;
        println!("screenshot: {}", path.display());
        return Ok(());
    }
    if is_dictation_page_scene(name) {
        // The GPU card is offered only on an NVIDIA machine: the scene says it is one, as the e2e does, so the
        // page shows the same cards on every PC.
        // SAFETY: the scene runs before any other thread of this process reads the environment.
        unsafe { std::env::set_var(flare_win::names::SEAM_NVIDIA, "1") };
        // The seam reads only through the gate; a scene run says so for its own fixed answer.
        flare_win::seam::set_open(true);
    }
    let args = scene_args(name, args);
    let (saved, prep) = setup(name);
    let scene = Scene::build_with(
        &args,
        saved,
        prep,
        name == "theme-sample",
        is_term_scene(name),
    )?;
    if name == "settings" || is_dictation_page_scene(name) {
        scene.stage_settings(&args)?;
    } else {
        scene.stage(name)?;
    }
    let rgba = scene.render();
    write_png(path, scene.phys.0, scene.phys.1, &rgba)?;
    println!("screenshot: {}", path.display());
    Ok(())
}

// ---- --scene badge: the taskbar disc sheet (M6, Task 33) ----------------------------------------

/// The eight display scalings the disc is drawn at; its size is 16 logical px times the scale.
const BADGE_SCALES: [f64; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];
/// The counts the sheet shows, one per column: 1 to 9, then the "9+" of ten or more.
const BADGE_COUNTS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "9+"];
/// Each cell is 200 px; the disc (at most 48 px) is magnified 4x, nearest neighbour.
const BADGE_CELL: u32 = 200;
const BADGE_ZOOM: u32 = 4;
/// The sheet's ground, `#1f1f1f`.
const BADGE_GROUND: [u8; 3] = [0x1f, 0x1f, 0x1f];

/// `--scene badge`: the taskbar disc for each count at each scaling, drawn to `path`. Not a window:
/// the same pixels `flare-win` composes (`compose_disc`), premultiplied over the ground.
fn write_badge_sheet(path: &Path) -> Result<(), String> {
    let columns = BADGE_COUNTS.len() as u32;
    let rows = BADGE_SCALES.len() as u32;
    let (width, height) = (columns * BADGE_CELL, rows * BADGE_CELL);
    let ground = [BADGE_GROUND[0], BADGE_GROUND[1], BADGE_GROUND[2], 0xff];
    let mut rgba = ground.repeat((width * height) as usize);
    for (row, scale) in BADGE_SCALES.iter().enumerate() {
        let size = badge_px(*scale);
        for (col, text) in BADGE_COUNTS.iter().enumerate() {
            let disc = compose_disc(size, &text_mask(size, text));
            let shown = size * BADGE_ZOOM;
            let x0 = col as u32 * BADGE_CELL + (BADGE_CELL - shown) / 2;
            let y0 = row as u32 * BADGE_CELL + (BADGE_CELL - shown) / 2;
            for y in 0..shown {
                for x in 0..shown {
                    let src = ((y / BADGE_ZOOM) * size + x / BADGE_ZOOM) as usize * 4;
                    // The disc is premultiplied BGRA: its colour plus the ground under its gap.
                    let alpha = u32::from(disc.bgra[src + 3]);
                    let dst = (((y0 + y) * width + x0 + x) * 4) as usize;
                    for (k, channel) in [2usize, 1, 0].into_iter().enumerate() {
                        let under = u32::from(BADGE_GROUND[k]) * (255 - alpha) / 255;
                        rgba[dst + k] =
                            (u32::from(disc.bgra[src + channel]) + under).min(255) as u8;
                    }
                    rgba[dst + 3] = 0xff;
                }
            }
        }
    }
    write_png(path, width, height, &rgba)?;
    println!("screenshot: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_args;

    /// The eight display scalings, as the screenshots take them.
    const MARK_BAR_SCALES: [f32; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    /// The mark scene at `scale`, staged and drawn; its straight RGBA pixels are returned too.
    fn staged_marks(name: &str, scale: f32) -> (Scene, Vec<u8>) {
        let argv: Vec<String> = [
            "--scene",
            name,
            "--scale",
            &scale.to_string(),
            "--size",
            // As `run_scene`: sixteen tabs overflow a 700 px strip.
            if name == "marks-overflow" {
                "700x640"
            } else {
                "1000x640"
            },
            "--renderer",
            "software",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let args = parse_args(&argv, None).expect("the arguments parse");
        let (saved, prep) = setup(name);
        let scene = Scene::build_with(&args, saved, prep, false, false).expect("the scene builds");
        scene.stage(name).expect("the scene stages");
        let pixels = scene.render();
        (scene, pixels)
    }

    /// Task 33: on Bar, every marked tab's band (its line, not the rainbow) is round(3 * scale)
    /// device pixels thick, at every scaling. The band is the longest run of the line's colour
    /// down the middle of the tab, read from the drawn frame.
    #[test]
    fn the_bar_band_is_the_same_weight_on_every_marked_tab_at_every_scale() {
        for scale in MARK_BAR_SCALES {
            let (scene, pixels) = staged_marks("marks-bar", scale);
            let w = scene.phys.0 as usize;
            let rows = scene.app.get_tab_rows();
            let want = (3.0 * scale).round() as usize;
            let mut checked = 0;
            for i in 0..rows.row_count() {
                let row = rows.row_data(i).expect("a row");
                // A line and a run bar are both three device pixels thick; the rainbow is skipped.
                if row.mark_state == 0 || row.mark_kind == 0 || row.mark_rainbow {
                    continue;
                }
                let x = (scene.strip_css(row.x + row.w / 2) * scale).round() as usize;
                let colour = [
                    row.mark_color.red(),
                    row.mark_color.green(),
                    row.mark_color.blue(),
                ];
                let mut longest = 0;
                let mut run = 0;
                for y in 0..(40.0 * scale) as usize {
                    let p = (y * w + x) * 4;
                    if pixels[p..p + 3] == colour {
                        run += 1;
                        longest = longest.max(run);
                    } else {
                        run = 0;
                    }
                }
                assert_eq!(
                    longest, want,
                    "tab {i} at scale {scale}: the band thickness"
                );
                checked += 1;
            }
            assert!(checked >= 3, "at scale {scale} only {checked} marked tabs");
        }
    }

    /// An idle window settles (regression rule 1181): once a scene with nothing moving has drawn, a
    /// second of frames asks for no redraw at all, so the live window would present nothing new.
    #[test]
    fn an_idle_window_asks_for_no_frame() {
        for name in ["theme-sample", "sidebar-open"] {
            let argv: Vec<String> = [
                "--scene",
                name,
                "--size",
                "1000x640",
                "--renderer",
                "software",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            let args = parse_args(&argv, None).expect("the arguments parse");
            let (saved, prep) = setup(name);
            let scene =
                Scene::build_with(&args, saved, prep, false, false).expect("the scene builds");
            scene.stage(name).expect("the scene stages");
            // Settle: whatever the restore started (a slide, a fade) has a second to end.
            for _ in 0..60 {
                scene.advance(16);
                scene.pump();
                scene.win.draw_if_needed(|_| {});
            }
            let mut asked = 0;
            for _ in 0..60 {
                scene.advance(16);
                scene.pump();
                if scene.win.draw_if_needed(|_| {}) {
                    asked += 1;
                }
            }
            assert_eq!(
                asked, 0,
                "{name}: an idle window asked for {asked} of 60 frames"
            );
        }
    }

    /// The straight RGB of device pixel (x, y).
    fn rgb(scene: &Scene, pixels: &[u8], x: usize, y: usize) -> [u8; 3] {
        let p = (y * scene.phys.0 as usize + x) * 4;
        [pixels[p], pixels[p + 1], pixels[p + 2]]
    }

    fn saturation(c: [u8; 3]) -> u8 {
        c.iter().max().unwrap() - c.iter().min().unwrap()
    }

    /// The columns (device px, counted back from the tab's right edge) in the last 28 css px of strip
    /// tab `i` (its end box and padding, never its name) that hold any ink between the strip's
    /// rows 8 and 28 css.
    fn end_ink(scene: &Scene, pixels: &[u8], i: usize) -> Vec<usize> {
        let s = scene.scale;
        let rows = scene.app.get_tab_rows();
        let row = rows.row_data(i).expect("a row");
        let right = (scene.strip_css(row.x + row.w) * s).round() as usize - 2;
        let ground = rgb(scene, pixels, right - 1, (4.0 * s) as usize);
        let mut cols = Vec::new();
        for back in 0..(28.0 * s) as usize {
            let x = right - back;
            let inked = ((8.0 * s) as usize..(28.0 * s) as usize).any(|y| {
                let c = rgb(scene, pixels, x, y);
                c.iter().zip(ground).any(|(a, b)| a.abs_diff(b) > 24)
            });
            if inked {
                cols.push(back);
            }
        }
        cols
    }

    /// Task 34 (the screenshots showed the front `tools` tab with both its spinner and the close X):
    /// a ringed tab hides its X until hovered, so the front working tab's end draws exactly what a
    /// working tab behind it draws, at every scaling.
    #[test]
    fn a_ringed_front_tab_shows_its_ring_and_no_close_at_every_scale() {
        for scale in MARK_BAR_SCALES {
            let (scene, pixels) = staged_marks("marks-ring", scale);
            let rows = scene.app.get_tab_rows();
            let front = (0..rows.row_count())
                .find(|&i| rows.row_data(i).is_some_and(|r| r.active))
                .expect("a front tab");
            assert_eq!(
                rows.row_data(front).unwrap().mark_state,
                1,
                "the front tab works"
            );
            // The same shape; a tab one device pixel wider may place it one pixel further in.
            let shape = |cols: Vec<usize>| {
                let first = cols.first().copied().unwrap_or(0);
                (
                    first,
                    cols.into_iter().map(|c| c - first).collect::<Vec<_>>(),
                )
            };
            let (at_front, front_shape) = shape(end_ink(&scene, &pixels, front));
            let (at_back, back_shape) = shape(end_ink(&scene, &pixels, 0));
            assert!(
                front_shape == back_shape && at_front.abs_diff(at_back) <= 1,
                "at scale {scale} the front tab's end is not the spinner alone: {at_front} {front_shape:?} vs {at_back} {back_shape:?}"
            );
        }
    }

    /// Task 34 (the Codex tab wore Clawd and no shell tab was in the strip): `marks-icons` shows the
    /// OpenAI mark in the ink on the Codex tab and the prompt on a shell tab, neither in Clawd's orange.
    #[test]
    fn the_icons_scene_shows_clawd_openai_and_the_prompt() {
        for scale in [1.0, 1.5, 2.25] {
            let (scene, pixels) = staged_marks("marks-icons", scale);
            let s = scene.scale;
            let rows = scene.app.get_tab_rows();
            let mut kinds = Vec::new();
            for i in 0..rows.row_count() {
                let row = rows.row_data(i).unwrap();
                if row.settings {
                    continue;
                }
                // The icon: 18 css px from the tab's 10 px padding, centred on the strip.
                let x0 = ((scene.strip_css(row.x) + 9.0) * s) as usize;
                let (mut orange, mut ink) = (0, 0);
                for y in (8.0 * s) as usize..(28.0 * s) as usize {
                    for x in x0..x0 + (20.0 * s) as usize {
                        let c = rgb(&scene, &pixels, x, y);
                        if c[0].abs_diff(0xd7) < 12
                            && c[1].abs_diff(0x77) < 12
                            && c[2].abs_diff(0x57) < 12
                        {
                            orange += 1;
                        } else if saturation(c) < 16 && c[0] > 0x80 {
                            ink += 1;
                        }
                    }
                }
                kinds.push((row.label.to_string(), orange > 10, ink > 10));
            }
            let claude = kinds.iter().filter(|k| k.1).count();
            let inked = kinds.iter().filter(|k| !k.1 && k.2).count();
            assert!(claude >= 1, "at {scale} no Clawd: {kinds:?}");
            assert!(
                inked >= 2,
                "at {scale} no OpenAI mark and prompt in the ink: {kinds:?}"
            );
        }
    }

    /// Task 34 (the +N button had no dots: every tab that needed you fitted in the strip): the
    /// overflow scene hides tabs that need you, so the button shows the question's and the failed
    /// tab's colours.
    #[test]
    fn the_overflow_button_shows_the_mark_colours_of_hidden_tabs() {
        for scale in [1.0, 1.75, 3.0] {
            let (scene, pixels) = staged_marks("marks-overflow", scale);
            let s = scene.scale;
            let g = scene.app.get_strip_geom();
            let x0 = (scene.strip_css(g.more_x) * s) as usize;
            let x1 = (scene.strip_css(g.more_x + g.more_w) * s) as usize;
            let mut seen = Vec::new();
            for y in (8.0 * s) as usize..(28.0 * s) as usize {
                for x in x0..x1 {
                    let c = rgb(&scene, &pixels, x, y);
                    if saturation(c) > 120 {
                        seen.push(c);
                    }
                }
            }
            let blue = seen.iter().any(|c| c[2] > 0xe0 && c[0] < 0x60);
            let red = seen
                .iter()
                .any(|c| c[0] > 0xf0 && c[1] < 0x70 && c[2] < 0x70);
            assert!(
                blue && red,
                "at {scale} the +N dots: blue {blue}, red {red}"
            );
        }
    }

    /// Task 34 (with four dots the +N button's count touched its left edge: the button kept the
    /// plan's 72 px while Prism Terminal's grows to its content, `px-2`): the count and the chevron
    /// keep at least 6 css px from the button's edges, at every scaling.
    #[test]
    fn the_overflow_button_keeps_its_padding_with_four_dots() {
        for scale in MARK_BAR_SCALES {
            let (scene, pixels) = staged_marks("marks-overflow", scale);
            let s = scene.scale;
            let g = scene.app.get_strip_geom();
            assert_eq!(g.dots.row_count(), 4, "four dots");
            let x0 = (scene.strip_css(g.more_x) * s).round() as usize;
            let x1 = (scene.strip_css(g.more_x + g.more_w) * s).round() as usize;
            let ground = rgb(&scene, &pixels, (x0 + x1) / 2, (6.0 * s) as usize);
            let inked: Vec<usize> = (x0..x1)
                .filter(|&x| {
                    ((10.0 * s) as usize..(26.0 * s) as usize).any(|y| {
                        let c = rgb(&scene, &pixels, x, y);
                        c.iter().zip(ground).any(|(a, b)| a.abs_diff(b) > 40)
                    })
                })
                .collect();
            let (first, last) = (inked[0], inked[inked.len() - 1]);
            let pad = (6.0 * s).floor() as usize;
            assert!(
                first - x0 >= pad && x1 - 1 - last >= pad,
                "at {scale}: ink {first}..{last} in the button {x0}..{x1}"
            );
        }
    }

    /// Task 34 (the close question stood over a resume skeleton while its tab already worked): the
    /// grid behind the dialog is the shell's, so the skeleton's first line is not drawn.
    #[test]
    fn the_close_scene_has_no_skeleton_behind_the_question() {
        let (scene, pixels) = staged_marks("marks-close", 1.5);
        let s = scene.scale;
        // The skeleton's first line: x 16..80 css, 11 px tall from y 52; its gap below at y 66.
        let line = rgb(&scene, &pixels, (60.0 * s) as usize, (57.0 * s) as usize);
        let gap = rgb(&scene, &pixels, (60.0 * s) as usize, (66.0 * s) as usize);
        let ground = rgb(&scene, &pixels, (900.0 * s) as usize, (600.0 * s) as usize);
        assert!(
            !(line != gap && gap == ground),
            "a skeleton line at the top of the grid: {line:?} over {gap:?}"
        );
    }

    /// Task 34 (the sidebar's rainbow had a ground-coloured column at 175%): no device pixel
    /// inside a Bar line is the ground, in the strip or the sidebar, at every scaling.
    #[test]
    fn the_rainbow_line_has_no_gap_at_any_scale() {
        for name in ["marks-bar", "marks-bar-flow", "marks-sidebar-bar"] {
            for scale in MARK_BAR_SCALES {
                let (scene, pixels) = staged_marks(name, scale);
                let (w, h) = (scene.phys.0 as usize, scene.phys.1 as usize);
                let s = scene.scale as f64;
                // The strip's rows, or the sidebar's column (the terminal's coloured text is not a mark).
                let (xs, ys) = if name == "marks-sidebar-bar" {
                    ((220.0 * s) as usize, (420.0 * s) as usize)
                } else {
                    (w, (40.0 * s) as usize)
                };
                for y in 0..h.min(ys) {
                    let sat: Vec<u8> = (0..xs)
                        .map(|x| saturation(rgb(&scene, &pixels, x, y)))
                        .collect();
                    // Only a row a line runs along (an icon's few coloured pixels are not one).
                    if sat.iter().filter(|&&v| v > 90).count() < (100.0 * s) as usize {
                        continue;
                    }
                    // A seam: one or two ground columns with the line's colour on both sides.
                    let seams: Vec<usize> = (1..xs - 2)
                        .filter(|&x| {
                            sat[x] < 40 && sat[x - 1] > 90 && (sat[x + 1] > 90 || sat[x + 2] > 90)
                        })
                        .collect();
                    assert!(
                        seams.is_empty(),
                        "{name} at {scale}, row {y}: ground at {seams:?}"
                    );
                }
            }
        }
    }

    fn staged(name: &str) -> Scene {
        let argv: Vec<String> = [
            "--scene",
            name,
            "--scale",
            "1",
            "--size",
            "1000x640",
            "--renderer",
            "software",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let args = parse_args(&argv, None).expect("the arguments parse");
        let (saved, prep) = setup(name);
        let scene = Scene::build_with(&args, saved, prep, false, true).expect("the scene builds");
        scene.stage(name).expect("the scene stages");
        scene
    }

    /// Task 27: the screenshot of `term-find-many` read "1 of 1000"; the plan's scene shows the
    /// readout `1000+` (the active match past the thousand that are counted).
    #[test]
    fn the_many_scene_reads_a_thousand_plus() {
        let scene = staged("term-find-many");
        assert_eq!(scene.app.get_find_readout().as_str(), "1000+");
    }

    // ---- the update scenes (M9, Task 40) ----------------------------------------------------

    const UPDATE_SCENES: [&str; 8] = [
        "update-chip",
        "update-chip-notice",
        "update-window",
        "update-window-running",
        "update-window-installing",
        "update-window-notice",
        "update-window-small",
        "update-window-empty",
    ];

    /// The update scene at `scale`, sized as `run_scene` sizes it, staged and drawn.
    fn staged_update(name: &str, scale: f32) -> (Scene, Vec<u8>) {
        let argv: Vec<String> = [
            "--scene",
            name,
            "--scale",
            &scale.to_string(),
            "--renderer",
            "software",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let args = scene_args(name, &parse_args(&argv, None).expect("the arguments parse"));
        let (saved, prep) = setup(name);
        let scene = Scene::build_with(&args, saved, prep, false, false).expect("the scene builds");
        scene.stage(name).expect("the scene stages");
        let pixels = scene.render();
        (scene, pixels)
    }

    #[test]
    fn the_scene_list_names_the_eight_update_scenes_once_each() {
        for name in UPDATE_SCENES {
            assert_eq!(
                SCENES.iter().filter(|s| **s == name).count(),
                1,
                "{name} is listed once"
            );
            assert!(is_update_scene(name));
        }
        assert!(!is_update_scene("settings"));
        let mut all: Vec<&str> = SCENES.to_vec();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), SCENES.len(), "no scene name repeats");
    }

    #[test]
    fn the_small_update_scene_is_1000_by_420_and_the_others_keep_the_default() {
        let args = parse_args(
            &["--scene".to_string(), "update-window-small".to_string()],
            None,
        )
        .unwrap();
        assert_eq!(scene_args("update-window-small", &args).size, (1000, 420));
        assert_eq!(scene_args("update-window", &args).size, (1000, 640));
    }

    #[test]
    fn every_update_scene_shows_its_state_at_every_scaling() {
        for scale in MARK_BAR_SCALES {
            for name in UPDATE_SCENES {
                let (scene, pixels) = staged_update(name, scale);
                let app = &scene.app;
                assert_eq!(
                    pixels.len(),
                    (scene.phys.0 * scene.phys.1 * 4) as usize,
                    "{name} at {scale}"
                );
                assert!(
                    pixels.chunks(4).any(|p| p[..3] != pixels[..3]),
                    "{name} at {scale} is not one flat colour"
                );
                assert!(app.get_update_chip_shown(), "{name}: the chip is up");
                let open = app.get_update_open();
                let notice = app.get_update_notice().to_string();
                match name {
                    "update-chip" => {
                        assert!(!open && notice.is_empty());
                        assert!(app.get_update_chip_label().starts_with("Update "));
                    }
                    "update-chip-notice" => {
                        assert!(!open, "the window is away");
                        assert_eq!(notice, flare_update::flow::INSTALL_FAILED);
                    }
                    "update-window" => {
                        assert!(open && !app.get_update_running());
                        assert!(app.get_update_sections().row_count() >= 3);
                    }
                    "update-window-running" => {
                        assert!(open && app.get_update_running());
                        assert_eq!(app.get_update_pct(), 42);
                    }
                    "update-window-installing" => {
                        assert!(open && app.get_update_running());
                        assert_eq!(app.get_update_pct(), 100);
                    }
                    "update-window-notice" => {
                        assert!(open && !app.get_update_running());
                        assert_eq!(app.get_update_status(), flare_update::flow::PREVIEW_DONE);
                        assert_eq!(app.get_update_cancel_label(), "Close");
                    }
                    "update-window-small" => {
                        assert!(open);
                        assert_eq!(
                            scene.phys,
                            (
                                (1000.0 * scale).round() as u32,
                                (420.0 * scale).round() as u32
                            )
                        );
                        let entries: usize = app
                            .get_update_sections()
                            .iter()
                            .map(|s| s.entries.row_count())
                            .sum();
                        assert_eq!(entries, 20, "the cap");
                        assert_eq!(app.get_update_more().as_str(), "+ 5 more");
                    }
                    "update-window-empty" => {
                        assert!(open);
                        assert_eq!(app.get_update_sections().row_count(), 0);
                        assert_eq!(
                            app.get_update_empty_note().as_str(),
                            flare_update::notes::NO_NOTES
                        );
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}
