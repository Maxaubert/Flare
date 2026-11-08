//! SessionEndWatch on macOS (port phase 3): the Mac meaning of "the session ends, save now".
//!
//! - **The app ends**: `NSApplicationWillTerminateNotification`. AppKit posts it on the main thread
//!   right before the process exits, for Quit (Cmd+Q, the Dock's Quit), a log out, a restart and a
//!   shut down (each sends the app a quit event that ends in `terminate:`). winit's delegate does
//!   not answer `applicationShouldTerminate:`, so the answer is "now", and this is the last moment
//!   the tabs, the window and the settings can be flushed.
//! - **The machine powers off**: `NSWorkspaceWillPowerOffNotification`, earlier than the quit
//!   event, so a shut down saves even when the quit is slow. It is NOT final: another app can still
//!   cancel the log out or shut down (Terminal with a running job, say), and Flare keeps running. So
//!   it flushes every time and never uses up the final save.
//! - **Sleep and the screens**: the workspace's will-sleep, did-wake, screens-did-sleep and
//!   screens-did-wake, as `PowerEvents` (the watchdog drops the gap across a sleep).
//!
//! The end handler runs once for the app's end, and again for every power-off notice before it
//! (flushing twice is harmless; a cancelled log out must leave the final save armed, regression rule
//! 235). Every observer is a block on a
//! notification centre with no queue, so it runs on the thread that posts (AppKit posts these on
//! the main thread). There is no hidden window as on Windows: `hwnd` is 0.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplicationWillTerminateNotification, NSWorkspace, NSWorkspaceDidWakeNotification,
    NSWorkspaceScreensDidSleepNotification, NSWorkspaceScreensDidWakeNotification,
    NSWorkspaceWillPowerOffNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol,
};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What the machine's sleep and its screen tell the app (the same shape as on Windows).
pub struct PowerEvents {
    pub on_suspend: Box<dyn Fn() + Send>,
    pub on_resume: Box<dyn Fn() + Send>,
    pub on_screen: Box<dyn Fn(bool) + Send>,
}

type Token = Retained<ProtocolObject<dyn NSObjectProtocol>>;

/// The registered observers; `stop` (or a drop) takes them off their centres.
pub struct SessionEndWatch {
    observers: Vec<(Retained<NSNotificationCenter>, Token)>,
}

/// Registers `f` for `name` on `center`.
fn observe(
    center: &Retained<NSNotificationCenter>,
    name: &NSNotificationName,
    f: impl Fn() + 'static,
) -> (Retained<NSNotificationCenter>, Token) {
    let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| f());
    // SAFETY: any sender of `name` is watched (object nil); the block owns everything it touches,
    // and with no queue it runs on the posting thread.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
    };
    (center.clone(), token)
}

impl SessionEndWatch {
    pub fn start(on_end: impl Fn() + Send + 'static) -> Result<Self, String> {
        Self::start_with_power(on_end, None)
    }

    pub fn start_with_power(
        on_end: impl Fn() + Send + 'static,
        power: Option<PowerEvents>,
    ) -> Result<Self, String> {
        let app = NSNotificationCenter::defaultCenter();
        let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
        let fired = Arc::new(AtomicBool::new(false));
        let on_end = Arc::new(on_end);
        // The power-off notice flushes each time and leaves `fired` alone.
        let flush_now = {
            let on_end = Arc::clone(&on_end);
            move || on_end()
        };
        let end = move || {
            let (fired, on_end) = (Arc::clone(&fired), Arc::clone(&on_end));
            move || {
                if !fired.swap(true, Ordering::SeqCst) {
                    on_end();
                }
            }
        };
        // SAFETY: AppKit's own constant notification names.
        let (terminate, power_off) = unsafe {
            (
                NSApplicationWillTerminateNotification,
                NSWorkspaceWillPowerOffNotification,
            )
        };
        let mut observers = vec![
            observe(&app, terminate, end()),
            observe(&workspace, power_off, flush_now),
        ];
        if let Some(p) = power {
            let PowerEvents {
                on_suspend,
                on_resume,
                on_screen,
            } = p;
            let on_screen = std::rc::Rc::new(on_screen);
            let screen_off = std::rc::Rc::clone(&on_screen);
            // SAFETY: as above.
            let (sleep, wake, screens_sleep, screens_wake) = unsafe {
                (
                    NSWorkspaceWillSleepNotification,
                    NSWorkspaceDidWakeNotification,
                    NSWorkspaceScreensDidSleepNotification,
                    NSWorkspaceScreensDidWakeNotification,
                )
            };
            observers.push(observe(&workspace, sleep, on_suspend));
            observers.push(observe(&workspace, wake, on_resume));
            observers.push(observe(&workspace, screens_sleep, move || {
                screen_off(false)
            }));
            observers.push(observe(&workspace, screens_wake, move || on_screen(true)));
        }
        Ok(Self { observers })
    }

    /// No hidden window on macOS: 0.
    pub fn hwnd(&self) -> isize {
        0
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for SessionEndWatch {
    fn drop(&mut self) {
        for (center, token) in self.observers.drain(..) {
            // SAFETY: `token` is the observer this centre handed out for it.
            unsafe { center.removeObserver(token.as_ref()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn post(center: &NSNotificationCenter, name: &NSNotificationName) {
        // SAFETY: an in-process post with no sender; the observers run synchronously here.
        unsafe { center.postNotificationName_object(name, None) };
    }

    fn counter() -> (Arc<AtomicUsize>, impl Fn() + Send + 'static) {
        let n = Arc::new(AtomicUsize::new(0));
        let m = Arc::clone(&n);
        (n, move || {
            m.fetch_add(1, Ordering::SeqCst);
        })
    }

    // One test: the notification centres are process-wide, so two tests in one process would hear
    // each other's posts.
    #[test]
    fn the_end_runs_once_a_power_off_never_disarms_it_and_the_power_events_arrive_until_the_watch_stops()
     {
        let (ends, on_end) = counter();
        let (sleeps, on_suspend) = counter();
        let (wakes, on_resume) = counter();
        let screens = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&screens);
        let watch = SessionEndWatch::start_with_power(
            on_end,
            Some(PowerEvents {
                on_suspend: Box::new(on_suspend),
                on_resume: Box::new(on_resume),
                on_screen: Box::new(move |on| seen.lock().expect("lock").push(on)),
            }),
        )
        .expect("the watch starts");
        assert_eq!(watch.hwnd(), 0);
        let app = NSNotificationCenter::defaultCenter();
        let ws = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: AppKit's own constant notification names.
        unsafe {
            post(&ws, NSWorkspaceWillSleepNotification);
            post(&ws, NSWorkspaceDidWakeNotification);
            post(&ws, NSWorkspaceScreensDidSleepNotification);
            post(&ws, NSWorkspaceScreensDidWakeNotification);
            assert_eq!(ends.load(Ordering::SeqCst), 0);
            // A shut down another app cancels: the power-off notice flushes, Flare keeps running.
            post(&ws, NSWorkspaceWillPowerOffNotification);
            assert_eq!(
                ends.load(Ordering::SeqCst),
                1,
                "the power-off notice flushes"
            );
            // The next shut down flushes again, and the quit after it still saves.
            post(&ws, NSWorkspaceWillPowerOffNotification);
            assert_eq!(ends.load(Ordering::SeqCst), 2);
            post(&app, NSApplicationWillTerminateNotification);
            assert_eq!(ends.load(Ordering::SeqCst), 3, "the final save stays armed");
            post(&app, NSApplicationWillTerminateNotification);
        }
        assert_eq!(ends.load(Ordering::SeqCst), 3, "the app's end runs once");
        assert_eq!(sleeps.load(Ordering::SeqCst), 1);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(*screens.lock().expect("lock"), vec![false, true]);
        watch.stop();
        // SAFETY: as above.
        unsafe { post(&ws, NSWorkspaceWillSleepNotification) };
        assert_eq!(
            sleeps.load(Ordering::SeqCst),
            1,
            "a stopped watch hears nothing"
        );
    }
}
