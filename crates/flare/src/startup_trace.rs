//! Start-up phase marks (Task 3 fills the body: the trace and the phase clock). Task 1 pins the
//! phase names and the call sites; `mark` does nothing yet.

// ShellSpawn, FirstPrompt and Usable get their call sites in Task 3 (Appendix F).
#![allow(dead_code)]

/// One named point of the start-up path, in the order the app reaches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Main,
    Args,
    Backend,
    Component,
    Profile,
    ShellSpawn,
    RenderSetup,
    FirstFrame,
    FirstPrompt,
    Usable,
}

/// Records that `phase` was reached. A no-op until Task 3.
pub fn mark(phase: Phase) {
    let _ = phase;
}
