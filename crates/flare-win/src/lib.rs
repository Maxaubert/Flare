//! flare-win: the Windows glue of the window: command-line folders, window state, monitors, the
//! single-instance lock and hand-off, the session-end watcher, the cursor at the edge, the
//! clipboard write and read, the folder picker, the shell open, the path openers and the
//! double-click metrics.

pub mod argv;
pub mod clipboard;
pub mod clipboard_port;
pub mod dblclick;
pub mod dwm;
pub mod edge_watch;
pub mod eyedrop;
pub mod eyedrop_math;
pub mod material;
pub mod monitors;
pub mod motion;
pub mod openers;
pub mod os_build;
pub mod paths;
pub mod pick;
pub mod procs;
pub mod session_end;
pub mod shell_open;
pub mod single_instance;
pub mod sysmenu;
pub mod taskbar;
pub mod taskbar_flash;
pub mod tcp_owner;
pub mod window_edge;
pub mod window_park;
pub mod window_state;

pub mod names;
pub mod packaged;
pub mod registry;
pub mod resize_edges;
pub mod seam;
pub mod thread_stack;
pub mod verb;
pub mod verb_keys;
pub mod verb_switch;
