//! SessionEndWatch off Windows. macOS: phase 3 of the port plan (NSWorkspace will-power-off, sleep
//! and screen notifications). Until then the watch refuses to start, so the app runs without it.

/// What the machine's sleep and its screen tell the app (the same shape as on Windows).
pub struct PowerEvents {
    pub on_suspend: Box<dyn Fn() + Send>,
    pub on_resume: Box<dyn Fn() + Send>,
    pub on_screen: Box<dyn Fn(bool) + Send>,
}

/// None can exist until phase 3: `start` always fails.
pub struct SessionEndWatch(std::convert::Infallible);

impl SessionEndWatch {
    pub fn start(on_end: impl Fn() + Send + 'static) -> Result<Self, String> {
        Self::start_with_power(on_end, None)
    }

    pub fn start_with_power(
        _on_end: impl Fn() + Send + 'static,
        _power: Option<PowerEvents>,
    ) -> Result<Self, String> {
        Err("the session-end watch is not built on this platform yet".to_string())
    }

    pub fn hwnd(&self) -> isize {
        match self.0 {}
    }

    pub fn stop(self) {
        match self.0 {}
    }
}
