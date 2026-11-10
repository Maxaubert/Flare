//! `webLogins`: a site's login is kept by the web panel (2026-11-10). A local site of the scenario's own (one thread on
//! `127.0.0.1`) answers `/login` with a persistent cookie, a session-only cookie and a page that writes `localStorage`,
//! the way a sign-in does. Then the panel is closed and opened again, and the app quits and starts again on the same
//! throwaway profile; each time the page reads `document.cookie` and `localStorage` itself (never the cookie files).
//! Last, an installed copy (a temp install, as `clickInstall` makes) installs a staged build and restarts itself. The
//! persistent cookies and the storage must be back every time; a session-only cookie ends with the panel's browser.
//!
//! The window is parked off the screen and never takes the foreground, like every scenario.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use flare_ctl::Method;
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::input_b::prompt;
use super::staged;
use super::web::{hello, LOAD};
use crate::ctx::{App, Ctx, Launch};
use crate::session::Session;

/// What the page reads: both cookies and the stored value, as one line.
const READ: &str = "document.cookie.split('; ').filter(function(c){return c.indexOf('flare_')===0;}).sort().join(',') + '|' + String(localStorage.getItem('flare_ls'))";

/// The scenario's site: `/login` signs in (a cookie for a year, a cookie for the session, and `localStorage`), every
/// other path is a plain page that sets nothing. It answers until it is dropped.
struct Site {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Site {
    fn start() -> Result<Site, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("bind: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                        let mut buf = [0u8; 4096];
                        let n = s.read(&mut buf).unwrap_or(0);
                        let head = String::from_utf8_lossy(&buf[..n]);
                        let path = head.split(' ').nth(1).unwrap_or("/").to_string();
                        let _ = s.write_all(reply(&path).as_bytes());
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(15)),
                }
            }
        });
        Ok(Site {
            port,
            stop,
            thread: Some(thread),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The answer to one request: the login page sets the cookies (as a site's sign-in answer does) and the storage;
/// `/late` and `/update` set one more cookie for a year, the moment before a quit or an update.
fn reply(path: &str) -> String {
    let (title, cookies, script) = match path {
        "/login" => (
            "Signed in",
            "Set-Cookie: flare_kept=yes; Max-Age=31536000; Path=/; SameSite=Lax\r\nSet-Cookie: flare_session=yes; Path=/; SameSite=Lax\r\n",
            "<script>localStorage.setItem('flare_ls','yes');</script>",
        ),
        "/late" => (
            "Late",
            "Set-Cookie: flare_late=yes; Max-Age=31536000; Path=/; SameSite=Lax\r\n",
            "",
        ),
        "/update" => (
            "Update",
            "Set-Cookie: flare_update=yes; Max-Age=31536000; Path=/; SameSite=Lax\r\n",
            "",
        ),
        _ => ("Home", "", ""),
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body><h1>{title}</h1>{script}</body></html>"
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\n{cookies}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn launch(ctx: &mut Ctx) -> Result<App, String> {
    let alpha = ctx.ensure_world().alpha.clone();
    ctx.launch(Launch {
        renderer: Some("gpu"),
        folders: vec![alpha],
        ..Launch::default()
    })
}

/// The pipe and the front tab's secret, once the app names a tab in front.
fn channel(ctx: &mut Ctx, s: &mut Session) -> Result<(String, String), String> {
    let mut got = None;
    ctx.until("a tab in front with a secret", 20_000, &mut || {
        let v = s.probe("ctl.info", Value::Null).unwrap_or(Value::Null);
        let pipe = v["pipe"].as_str().map(str::to_string);
        let token = v["tabs"]
            .as_array()
            .and_then(|t| t.iter().find(|t| t["active"] == json!(true)))
            .and_then(|t| t["token"].as_str())
            .map(str::to_string);
        got = pipe
            .zip(token)
            .filter(|(p, t)| !p.is_empty() && !t.is_empty());
        got.is_some()
    });
    got.ok_or_else(|| "no tab in front".to_string())
}

/// Opens `url` in the front tab's web panel through the channel; true once it has loaded.
fn open(ctx: &mut Ctx, s: &mut Session, url: &str) -> Result<bool, String> {
    let (pipe, token) = channel(ctx, s)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let r = c.call(Method::BrowserOpen, json!({ "url": url }), LOAD);
    Ok(r.is_ok_and(|v| v["loaded"] == json!(true)))
}

/// Closes the front tab's panel and waits for its browser to be let go.
fn close_panel(ctx: &mut Ctx, s: &mut Session) -> Result<bool, String> {
    let (pipe, token) = channel(ctx, s)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let closed = c.call(Method::PanelClose, json!({}), LOAD);
    let freed = ctx.until("the panel's browser to be let go", 10_000, &mut || {
        let st = s.probe("web.state", Value::Null).unwrap_or(Value::Null);
        st["controllers"] == json!(0) && st["backend"] == json!(false)
    });
    Ok(closed.is_ok_and(|v| v["closed"] == json!(true)) && freed)
}

/// What the page reads, unquoted ("" when it did not answer).
fn read(ctx: &mut Ctx, s: &mut Session) -> String {
    if s.call(&Request::WebScript {
        js: READ.to_string(),
    })
    .is_err()
    {
        return String::new();
    }
    let mut out = String::new();
    ctx.until("the page to answer", 15_000, &mut || {
        let r = s
            .probe("web.script.result", Value::Null)
            .unwrap_or(Value::Null);
        if r["done"] != json!(true) {
            return false;
        }
        out = if r["ok"] == json!(true) {
            let raw = r["value"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(raw)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| raw.to_string())
        } else {
            format!("ERROR {}", r["error"])
        };
        true
    });
    out
}

/// The cookies and the stored value of a reading.
fn parts(got: &str) -> (Vec<&str>, &str) {
    let (cookies, stored) = got.split_once('|').unwrap_or((got, ""));
    (cookies.split(',').collect(), stored)
}

pub fn web_logins(ctx: &mut Ctx) -> Result<(), String> {
    let site = Site::start()?;
    let mut app = launch(ctx)?;
    let r = first_run(ctx, &mut app, &site);
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r?;
    // The same profile again: what the site stored must be back.
    let mut app = launch(ctx)?;
    let r = second_run(ctx, &mut app, &site);
    let left = app.close();
    ctx.ok(left, "the second run quits by itself");
    r?;
    // The same profile once more, from an installed copy that installs a staged build and restarts itself.
    update_run(ctx, &site)
}

fn first_run(ctx: &mut Ctx, app: &mut App, site: &Site) -> Result<(), String> {
    prompt(app)?;
    let s = &mut app.session;
    let loaded = open(ctx, s, &site.url("/login"))?;
    ctx.ok(loaded, "the site's login page opens in the web panel");
    let got = read(ctx, s);
    ctx.ok(
        got == "flare_kept=yes,flare_session=yes|yes",
        format!("the sign-in left both cookies and the storage ({got})"),
    );
    // The panel closes and opens again in the same run. Its browser ends with the last panel (panels cost nothing
    // while closed), so the session-only cookie ends there too, as it would when a browser is closed.
    let closed = close_panel(ctx, s)?;
    ctx.ok(closed, "closing the panel lets its browser go");
    let loaded = open(ctx, s, &site.url("/"))?;
    ctx.ok(loaded, "the site opens again in a new panel");
    let got = read(ctx, s);
    let (cookies, stored) = parts(&got);
    ctx.ok(
        cookies.contains(&"flare_kept=yes") && stored == "yes",
        format!("after the panel closed and opened again, the login and the storage are still there ({got})"),
    );
    ctx.ok(
        !cookies.contains(&"flare_session=yes"),
        format!("and the session-only cookie ended with the panel's browser ({got})"),
    );
    // A cookie set the moment before the quit: the browser writes cookies to disk lazily, so the quit must let it.
    let loaded = open(ctx, s, &site.url("/late"))?;
    ctx.ok(
        loaded,
        "a page that sets one more cookie opens just before the quit",
    );
    Ok(())
}

fn second_run(ctx: &mut Ctx, app: &mut App, site: &Site) -> Result<(), String> {
    prompt(app)?;
    let s = &mut app.session;
    let loaded = open(ctx, s, &site.url("/"))?;
    ctx.ok(loaded, "after the restart the site opens in the web panel");
    let got = read(ctx, s);
    let (cookies, stored) = parts(&got);
    ctx.ok(
        cookies.contains(&"flare_kept=yes"),
        format!("after the restart the persistent cookie is back ({got})"),
    );
    ctx.ok(
        cookies.contains(&"flare_late=yes"),
        format!("and so is the one set just before the quit ({got})"),
    );
    ctx.ok(
        stored == "yes",
        format!("after the restart the site's localStorage is back ({got})"),
    );
    ctx.ok(
        !cookies.contains(&"flare_session=yes"),
        format!("and the session-only cookie ended with the run ({got})"),
    );
    Ok(())
}

/// An installed copy (a temp install in the world, as `clickInstall` makes it) with a staged build: the login is set,
/// Build ready is clicked, the app swaps its files, quits and starts the new exe on the same profile, and the page
/// finds the login there.
fn update_run(ctx: &mut Ctx, site: &Site) -> Result<(), String> {
    let (root, alpha, profile) = {
        let w = ctx.ensure_world();
        (w.root.clone(), w.alpha.clone(), w.profile.clone())
    };
    let inst = root.join("inst");
    let exe = staged::make_install(&ctx.exe.clone(), &inst)?;
    staged::stage(&exe, &inst)?;
    let real_exe = std::mem::replace(&mut ctx.exe, exe);
    let launched = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        env: vec![(staged::COUNTDOWN_VAR.into(), "1500".into())],
        ..Launch::default()
    });
    ctx.exe = real_exe;
    let mut app = launched?;
    let old_pid = app.hello()?.pid;
    prompt(&mut app)?;
    let loaded = open(ctx, &mut app.session, &site.url("/update"))?;
    ctx.ok(
        loaded,
        "the installed copy opens a page that sets a cookie before the update",
    );
    let chip = ctx.until("the Build ready chip", 15_000, &mut || {
        staged::state(&profile)["chip"] == json!(true)
    });
    ctx.ok(chip, "the installed copy offers the staged build");
    staged::click_chip(&mut app.session)?;
    let exited = ctx.until("the old app to quit for the install", 20_000, &mut || {
        app.child.exited().is_some()
    });
    ctx.ok(exited, "a click installs the build and the old app quits");

    let mut new = Session::connect(&profile, Duration::from_secs(30))?;
    let pid = new.hello()?.pid;
    ctx.ok(pid != old_pid, "the new build answers on the same profile");
    let installed = ctx.until("the new run's state", 15_000, &mut || {
        staged::state(&profile)["start"]
            .as_str()
            .is_some_and(|t| t.starts_with("Installed"))
    });
    ctx.ok(installed, "the new run knows it is a new build");
    let loaded = open(ctx, &mut new, &site.url("/"))?;
    ctx.ok(loaded, "after the update the site opens in the web panel");
    let got = read(ctx, &mut new);
    let (cookies, stored) = parts(&got);
    ctx.ok(
        cookies.contains(&"flare_kept=yes")
            && cookies.contains(&"flare_late=yes")
            && cookies.contains(&"flare_update=yes")
            && stored == "yes",
        format!("after the update every persistent cookie and the storage are back ({got})"),
    );
    let _ = new.call(&Request::Quit { force: false });
    let gone = ctx.until("the updated app to quit", 15_000, &mut || {
        Session::connect(&profile, Duration::from_millis(100)).is_err()
    });
    ctx.ok(gone, "the updated app quits");
    crate::proc::reap_by_profile(&profile);
    drop(app);
    Ok(())
}
