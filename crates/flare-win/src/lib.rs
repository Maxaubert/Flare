//! flare-win: the Windows glue of the window: command-line folders, window state, monitors, the
//! single-instance lock and hand-off, the session-end watcher, the cursor at the edge, the
//! clipboard write, the folder picker and the shell open.

pub mod argv;
pub mod clipboard;
pub mod dwm;
pub mod edge_watch;
pub mod material;
pub mod monitors;
pub mod os_build;
pub mod pick;
pub mod session_end;
pub mod shell_open;
pub mod single_instance;
pub mod window_edge;
pub mod window_state;
