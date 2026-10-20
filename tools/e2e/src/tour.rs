//! The machinery of the visual tour (audit 2026-10-20) that is not a scenario: copying a window from
//! outside from process creation, the wait that spins its last millisecond, the PNG writer and the judge
//! of a launch's pictures. `scenarios/visual.rs` is the flow; this is what it stands on.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::ctx::{park_problems, App, Ctx};
use crate::proc::Child;
use crate::session::Session;
use crate::winshot::{self, Look, Shot};
use crate::world::{prompt_showing, rows_text, Shell, World};

pub fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

pub fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// The tabs as pp.state answers them, read raw: the typed view cannot read a tab whose rectangle is
/// null (a tab in the vertical layout has none).
pub struct Tabs {
    pub list: Vec<Value>,
    pub front: Option<String>,
}

impl Tabs {
    pub fn read(app: &mut App) -> Option<Tabs> {
        let v = app.session.probe_str("app.state").ok()?;
        Some(Tabs {
            list: v["tabs"].as_array().cloned().unwrap_or_default(),
            front: v["front"].as_str().map(str::to_string),
        })
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn front_index(&self) -> Option<usize> {
        self.list.iter().position(|t| t["front"] == json!(true))
    }

    pub fn cwds(&self) -> Vec<String> {
        self.list
            .iter()
            .map(|t| t["cwd"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The middle of tab i's rectangle, when it has one.
    pub fn centre(&self, i: usize) -> Option<(f32, f32)> {
        let r = &self.list.get(i)?["rect"];
        Some((
            (r["x"].as_f64()? + r["w"].as_f64()? / 2.0) as f32,
            (r["y"].as_f64()? + r["h"].as_f64()? / 2.0) as f32,
        ))
    }
}
/// Writes an RGBA picture as a PNG, quickly.
pub fn write_png(path: &Path, shot: &Shot) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), shot.w, shot.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&shot.rgba).map_err(|e| e.to_string())
}

/// Wakes the monitor so frame pacing does not drop while the display sleeps: a mouse move of zero
/// distance. It touches no window and moves nothing.
pub fn wake_display() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{mouse_event, MOUSEEVENTF_MOVE};
    // SAFETY: a relative move of (0, 0): no key, no click, no window is addressed.
    unsafe { mouse_event(MOUSEEVENTF_MOVE, 0, 0, 0, 0) };
}

/// Waits until `at`, sleeping most of it and spinning the last millisecond (a sleep alone is 15 ms coarse).
pub fn wait_until(at: Instant) {
    loop {
        let now = Instant::now();
        if now >= at {
            return;
        }
        let left = at - now;
        if left > Duration::from_millis(3) {
            std::thread::sleep(left - Duration::from_millis(2)); // sync-ok: test harness, off the app
        } else {
            std::hint::spin_loop();
        }
    }
}

// ---- start-up and resume ----------------------------------------------------------------------------------

/// One launch watched from outside: every changed picture of the window from process creation, and the
/// state the pipe reports once it opens.
pub struct Watch {
    pub frames: Vec<Value>,
    pub timeline: Vec<Value>,
    pub trace: Value,
    pub series: Value,
    pub first_window_ms: Option<f64>,
    /// When the window was first on the screen (shown and not cloaked).
    pub first_visible_ms: Option<f64>,
    pub first_pipe_ms: Option<f64>,
    pub ready_ms: Option<f64>,
}

/// What the polling thread tells the copying loop.
enum Poll {
    State(Value),
    Ready(f64),
}

/// Asks the app for its tabs every 15 ms until told to stop (or the tabs are all there with a prompt
/// and `stop` is set by the loop). Returns the app.
fn poll_app(
    mut app: App,
    began: Instant,
    want_tabs: usize,
    stop: &AtomicBool,
    tx: &mpsc::Sender<Poll>,
) -> App {
    let ms = || began.elapsed().as_secs_f64() * 1000.0;
    let mut said_ready = false;
    while !stop.load(Ordering::SeqCst) {
        if let Some(st) = Tabs::read(&mut app) {
            let _ = tx.send(Poll::State(json!({
                "t_ms": ms(), "tabs": st.len(), "front": st.front_index(),
                "cwds": st.cwds(),
            })));
            if st.len() >= want_tabs && !said_ready {
                let id = st.front.clone().unwrap_or_default();
                if prompt_showing(&rows_text(&mut app, &id), Shell::Cmd) {
                    said_ready = true;
                    let _ = tx.send(Poll::Ready(ms()));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(15)); // sync-ok: test harness, off the app
    }
    app
}

/// Launches `exe` on `w` and copies the window every few milliseconds, keeping each changed picture in
/// `frames_dir`, until `want_tabs` tabs show a prompt and the picture has been still for 700 ms.
pub fn watch_launch(
    ctx: &mut Ctx,
    w: &World,
    renderer: &'static str,
    folders: &[PathBuf],
    want_tabs: usize,
    frames_dir: &Path,
) -> Result<(Watch, App), String> {
    let _ = fs::remove_dir_all(frames_dir);
    fs::create_dir_all(frames_dir).map_err(|e| e.to_string())?;
    let profile = w.profile.clone();
    let env = w.env();
    let mut args: Vec<String> = vec!["--no-import".into(), "--renderer".into(), renderer.into()];
    args.extend(folders.iter().map(|f| f.to_string_lossy().into_owned()));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let began = Instant::now();
    let child = Child::launch(&ctx.exe, &refs, &env, &profile);
    let pid = child.pid();

    let (tx, rx) = mpsc::sync_channel::<(PathBuf, Shot)>(3);
    let writer = std::thread::spawn(move || {
        for (p, s) in rx {
            let _ = write_png(&p, &s);
        }
    });

    let mut child = Some(child);
    let mut poller: Option<std::thread::JoinHandle<App>> = None;
    let stop = Arc::new(AtomicBool::new(false));
    let (ptx, prx) = mpsc::channel::<Poll>();
    let mut hwnd: Option<isize> = None;
    let mut last: Option<(u64, u32, u32, bool, bool)> = None;
    let mut changed = Instant::now();
    let mut watch = Watch {
        frames: Vec::new(),
        timeline: Vec::new(),
        trace: Value::Null,
        series: Value::Null,
        first_window_ms: None,
        first_visible_ms: None,
        first_pipe_ms: None,
        ready_ms: None,
    };
    let ms = |t: Instant| (t - began).as_secs_f64() * 1000.0;
    let deadline = began + Duration::from_secs(40);
    // The software renderer's window is copied from its own surface; a GPU swapchain through PrintWindow.
    let method = if renderer == "software" {
        winshot::Method::Gdi
    } else {
        winshot::Method::PrintWindow
    };
    loop {
        let now = Instant::now();
        if hwnd.is_none() {
            hwnd = winshot::windows_of(pid).first().map(|w| w.hwnd);
            if hwnd.is_some() {
                watch.first_window_ms = Some(ms(now));
            }
        }
        // The window manager's view first: whether this picture can be on the screen at all.
        let st = hwnd.and_then(winshot::state);
        let copy = hwnd.and_then(|h| winshot::capture_with(h, method).ok());
        if let Some(shot) = copy {
            let l: Look = winshot::look(&shot);
            let on_screen = st.is_some_and(|s| s.on_screen);
            let framed = st.is_some_and(|s| s.framed());
            let key = (l.hash, l.w, l.h, on_screen, framed);
            if last != Some(key) {
                last = Some(key);
                changed = Instant::now();
                if on_screen && watch.first_visible_ms.is_none() {
                    watch.first_visible_ms = Some(ms(now));
                }
                let n = watch.frames.len();
                let tag = frame_tag(on_screen, framed, l.bar, l.foot);
                let suffix = if tag.is_empty() {
                    String::new()
                } else {
                    format!("-{tag}")
                };
                let file = frames_dir.join(format!(
                    "{n:03}-t{:05}ms{suffix}.png",
                    ms(now).round() as i64
                ));
                watch.frames.push(json!({
                    "n": n, "t_ms": ms(now), "w": l.w, "h": l.h, "blank": l.blank,
                    "mean": l.mean, "bar": l.bar, "foot": l.foot,
                    "on_screen": on_screen, "framed": framed, "tag": tag,
                    "client": st.map(|s| [s.cw, s.ch]),
                    "style": st.map(|s| format!("{:08x}", s.style)),
                    "ex_style": st.map(|s| format!("{:08x}", s.ex_style)),
                    "file": file.file_name().map(|f| f.to_string_lossy()),
                }));
                if watch.frames.len() <= 80 {
                    let _ = tx.send((file, shot));
                }
            }
        }
        let session = if poller.is_none() {
            Session::connect(&profile, Duration::ZERO).ok()
        } else {
            None
        };
        if let Some(session) = session {
            watch.first_pipe_ms = Some(ms(Instant::now()));
            let app = App {
                child: child
                    .take()
                    .expect("the child is still ours until the pipe opens"),
                session,
                profile: profile.clone(),
            };
            let (stop, ptx) = (Arc::clone(&stop), ptx.clone());
            // The pipe answers when the app's UI thread is idle; asking from a thread of its own keeps
            // the window copying going meanwhile.
            poller = Some(std::thread::spawn(move || {
                poll_app(app, began, want_tabs, &stop, &ptx)
            }));
        }
        while let Ok(p) = prx.try_recv() {
            match p {
                Poll::State(entry) => {
                    if watch.timeline.last().is_none_or(|l| {
                        l["tabs"] != entry["tabs"]
                            || l["front"] != entry["front"]
                            || l["cwds"] != entry["cwds"]
                    }) {
                        watch.timeline.push(entry);
                    }
                }
                Poll::Ready(t) => {
                    watch.ready_ms.get_or_insert(t);
                }
            }
        }
        let still = changed.elapsed() >= Duration::from_millis(700);
        if watch.ready_ms.is_some() && still {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(4)); // sync-ok: test harness, off the app
    }
    drop(tx);
    let _ = writer.join();
    stop.store(true, Ordering::SeqCst);
    let Some(mut app) = poller.and_then(|h| h.join().ok()) else {
        return Err("the pipe never opened".into());
    };
    watch.trace = app
        .session
        .probe_str("startup.trace")
        .unwrap_or(Value::Null);
    watch.series = app
        .session
        .probe_str("frames.series")
        .unwrap_or(Value::Null);
    let problems = park_problems(&probe(&mut app, "window.park", Value::Null));
    if !problems.is_empty() {
        return Err(format!(
            "the --e2e window is not safe: {}",
            problems.join("; ")
        ));
    }
    Ok((watch, app))
}

/// What a copied picture stands for, from the window manager's facts at the moment of the copy (audit
/// V1). `PrintWindow` asks the window to paint itself, so it also copies a window DWM is not composing
/// yet, and it paints the classic frame that the window's style bits describe even when the window has
/// none (winit keeps `WS_CAPTION | WS_THICKFRAME` on a frameless window and takes the frame away in
/// `WM_NCCALCSIZE`, so its client rectangle is the whole window):
/// - `"hidden"`: the window is not shown or is cloaked; nothing of this picture reaches the screen.
/// - `"copyframe"`: the window is on the screen with no frame, yet the copy's top and bottom edges are
///   the light classic caption and border: the copier drew them, the screen does not.
/// - `""`: a picture the screen can show; it is judged.
pub fn frame_tag(on_screen: bool, framed: bool, bar: [u8; 3], foot: [u8; 3]) -> &'static str {
    let classic_light = |c: [u8; 3]| c.iter().all(|v| *v >= 180);
    if !on_screen {
        "hidden"
    } else if !framed && classic_light(bar) && classic_light(foot) {
        "copyframe"
    } else {
        ""
    }
}

/// What the frames of a launch say about flashes, jumps and a late theme. The plain counts take every
/// copied picture; the `seen_` counts take only the pictures the screen can show (`frame_tag` is empty;
/// a frame from before the window-manager facts were recorded counts as seen).
pub fn judge_frames(frames: &[Value]) -> Value {
    let mut out = judge_all(frames);
    let tag = |f: &Value| f["tag"].as_str().unwrap_or("").to_string();
    let seen: Vec<&Value> = frames.iter().filter(|f| tag(f).is_empty()).collect();
    let size_of = |f: &Value| (num(&f["w"]) as u32, num(&f["h"]) as u32);
    let final_size = seen.last().map(|f| size_of(f));
    let first = seen.first();
    let painted = first.is_some_and(|f| {
        f["blank"] != json!(true)
            && f["mean"]
                .as_array()
                .is_some_and(|m| !m.is_empty() && m.iter().all(|c| num(c) > 10.0))
    });
    let extra = json!({
        "hidden_frames": frames.iter().filter(|f| tag(f) == "hidden").count(),
        "copier_frame_frames": frames.iter().filter(|f| tag(f) == "copyframe").count(),
        "seen_frames": seen.len(),
        "first_seen_ms": first.map(|f| f["t_ms"].clone()),
        "first_seen_size": first.map(|f| json!([f["w"], f["h"]])),
        "first_seen_painted": painted,
        "seen_blank_frames": seen.iter().filter(|f| f["blank"] == json!(true)).count(),
        "seen_framed_frames": seen.iter().filter(|f| f["framed"] == json!(true)).count(),
        "seen_wrong_size_frames": seen.iter().filter(|f| Some(size_of(f)) != final_size).count(),
    });
    if let (Some(o), Some(e)) = (out.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    out
}

/// The counts over every copied picture, compared with the last one.
fn judge_all(frames: &[Value]) -> Value {
    let n = frames.len();
    let Some(last) = frames.last() else {
        return json!({ "frames": 0 });
    };
    let colour = |f: &Value, k: &str| -> Vec<f64> {
        f[k].as_array()
            .map(|a| a.iter().map(num).collect())
            .unwrap_or_default()
    };
    let far = |a: &[f64], b: &[f64]| -> bool {
        a.len() == b.len() && a.iter().zip(b).any(|(x, y)| (x - y).abs() > 6.0)
    };
    let (final_bar, final_foot) = (colour(last, "bar"), colour(last, "foot"));
    let size_of = |f: &Value| (num(&f["w"]) as u32, num(&f["h"]) as u32);
    let final_size = size_of(last);
    let blank = frames.iter().filter(|f| f["blank"] == json!(true)).count();
    let wrong_size = frames.iter().filter(|f| size_of(f) != final_size).count();
    let bright = |f: &Value| {
        let m = colour(f, "mean");
        m.iter().sum::<f64>() / 3.0
    };
    let final_bright = bright(last);
    // A frame much lighter than the last one while the last is dark (a white flash), or the reverse.
    let flashes = frames
        .iter()
        .filter(|f| (bright(f) - final_bright).abs() > 60.0 && f["blank"] != json!(true))
        .count();
    let wrong_bar = frames
        .iter()
        .filter(|f| far(&colour(f, "bar"), &final_bar) && f["blank"] != json!(true))
        .count();
    let wrong_foot = frames
        .iter()
        .filter(|f| far(&colour(f, "foot"), &final_foot) && f["blank"] != json!(true))
        .count();
    let first = &frames[0];
    json!({
        "frames": n,
        "first_frame_ms": first["t_ms"],
        "last_change_ms": last["t_ms"],
        "blank_frames": blank,
        "wrong_size_frames": wrong_size,
        "off_brightness_frames": flashes,
        "wrong_bar_colour_frames": wrong_bar,
        "wrong_foot_colour_frames": wrong_foot,
        "first_size": [first["w"], first["h"]],
        "final_size": [last["w"], last["h"]],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_judge_counts_blank_flash_and_wrong_size_frames() {
        let f = |t: f64, w: u32, mean: f64, blank: bool| {
            json!({ "t_ms": t, "w": w, "h": 100, "blank": blank,
                    "mean": [mean, mean, mean], "bar": [mean as u8, 0, 0], "foot": [10, 10, 10] })
        };
        let frames = vec![
            f(10.0, 800, 255.0, true),
            f(40.0, 800, 250.0, false),
            f(80.0, 1000, 20.0, false),
            f(120.0, 1000, 22.0, false),
        ];
        let j = judge_frames(&frames);
        assert_eq!(j["frames"], 4);
        assert_eq!(j["blank_frames"], 1);
        assert_eq!(j["wrong_size_frames"], 2);
        assert_eq!(
            j["off_brightness_frames"], 1,
            "the white frame that is not blank"
        );
        assert_eq!(j["first_frame_ms"], 10.0);
        assert_eq!(judge_frames(&[])["frames"], 0);
    }

    fn seen(t: f64, w: u32, mean: f64, blank: bool, tag: &str, framed: bool) -> Value {
        json!({ "t_ms": t, "w": w, "h": 100, "blank": blank, "mean": [mean, mean, mean],
                "bar": [mean as u8, 0, 0], "foot": [10, 10, 10],
                "on_screen": tag != "hidden", "framed": framed, "tag": tag })
    }

    #[test]
    fn a_hidden_window_and_a_copier_frame_are_tagged() {
        // Not shown (or cloaked): DWM composes nothing, whatever the copy shows (audit V1).
        assert_eq!(
            frame_tag(false, false, [191, 206, 220], [215, 228, 242]),
            "hidden"
        );
        assert_eq!(frame_tag(false, true, [0, 0, 0], [0, 0, 0]), "hidden");
        // On the screen, with no frame by the window manager, yet the copy has the light classic
        // caption and border: PrintWindow painted the style's frame itself.
        assert_eq!(
            frame_tag(true, false, [191, 206, 220], [215, 227, 241]),
            "copyframe"
        );
        // A real picture on the screen, or a real frame, is judged.
        assert_eq!(frame_tag(true, false, [23, 23, 23], [23, 23, 23]), "");
        assert_eq!(frame_tag(true, true, [191, 206, 220], [215, 227, 241]), "");
    }

    #[test]
    fn the_judge_reads_only_what_the_screen_can_show() {
        let frames = vec![
            seen(600.0, 1280, 0.0, true, "hidden", true),
            seen(650.0, 1000, 12.0, false, "hidden", false),
            seen(700.0, 1000, 23.0, false, "", false),
            seen(800.0, 1000, 23.0, false, "copyframe", false),
            seen(900.0, 1000, 24.0, false, "", false),
        ];
        let j = judge_frames(&frames);
        assert_eq!(j["hidden_frames"], 2);
        assert_eq!(j["copier_frame_frames"], 1);
        assert_eq!(j["seen_frames"], 2);
        assert_eq!(j["first_seen_ms"], 700.0);
        assert_eq!(j["seen_blank_frames"], 0);
        assert_eq!(j["seen_framed_frames"], 0);
        assert_eq!(j["seen_wrong_size_frames"], 0);
        assert_eq!(j["first_seen_painted"], true);
        assert_eq!(j["first_seen_size"], json!([1000, 100]));
        // Every picture is still counted in the plain columns.
        assert_eq!(j["frames"], 5);
        assert_eq!(j["blank_frames"], 1);

        // A black or framed or wrong-size picture once on the screen is counted.
        let bad = vec![
            seen(700.0, 900, 0.0, true, "", false),
            seen(750.0, 1000, 23.0, false, "", true),
            seen(800.0, 1000, 23.0, false, "", false),
        ];
        let j = judge_frames(&bad);
        assert_eq!(j["seen_blank_frames"], 1);
        assert_eq!(j["seen_framed_frames"], 1);
        assert_eq!(j["seen_wrong_size_frames"], 1);
        assert_eq!(j["first_seen_painted"], false);

        // Older numbers with no window-manager facts count every picture as seen.
        let old = [seen(10.0, 800, 20.0, false, "", false)];
        let mut o = old[0].clone();
        o.as_object_mut().unwrap().remove("on_screen");
        o.as_object_mut().unwrap().remove("tag");
        assert_eq!(judge_frames(&[o])["seen_frames"], 1);
        // Nothing on the screen at all: no first seen picture.
        let none = judge_frames(&[seen(5.0, 800, 0.0, true, "hidden", false)]);
        assert_eq!(none["seen_frames"], 0);
        assert_eq!(none["first_seen_ms"], Value::Null);
        assert_eq!(none["first_seen_painted"], false);
    }
}
