//! DragInFlight: a drag in flight across windows (Task 9).

/// Whether a file drag hovers the window. Any pointer event ends it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DragInFlight {
    on: bool,
}

impl DragInFlight {
    /// A new state, off.
    pub fn new() -> Self {
        Self::default()
    }
    /// A file starts hovering the window.
    pub fn hovered_file(&mut self) {
        self.on = true;
    }
    /// The pointer moved.
    pub fn pointer_moved(&mut self) {
        self.on = false;
    }
    /// The pointer went down.
    pub fn pointer_down(&mut self) {
        self.on = false;
    }
    /// The file was dropped.
    pub fn dropped(&mut self) {
        self.on = false;
    }
    /// The hover was cancelled.
    pub fn cancelled(&mut self) {
        self.on = false;
    }
    /// Whether a drag is in flight.
    pub fn on(&self) -> bool {
        self.on
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_in_flight_is_on_while_a_file_hovers_and_off_after_any_pointer_event() {
        let mut d = DragInFlight::new();
        d.hovered_file();
        assert!(d.on());
        d.pointer_moved();
        assert!(!d.on());

        d.hovered_file();
        d.pointer_down();
        assert!(!d.on());

        d.hovered_file();
        d.dropped();
        assert!(!d.on());

        d.hovered_file();
        d.cancelled();
        assert!(!d.on());
    }
}
