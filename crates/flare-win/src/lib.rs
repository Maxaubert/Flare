//! flare-win: the Windows glue of the window: command-line folders, window state, monitors, the
//! single-instance lock and hand-off, the session-end watcher, the cursor at the edge, the
//! clipboard write and read, the folder picker, the shell open, the path openers and the
//! double-click metrics.

pub mod agent_cursor;
pub mod argv;
#[cfg(target_os = "macos")]
#[path = "macos/chrome.rs"]
pub mod chrome;
pub mod clipboard;
pub mod clipboard_port;
pub mod dblclick;
#[cfg(target_os = "macos")]
#[path = "macos/dock.rs"]
pub mod dock;
pub mod dwm;
pub mod edge_watch;
pub mod eyedrop;
pub mod eyedrop_math;
#[cfg(target_os = "macos")]
#[path = "macos/float_panel.rs"]
pub mod float_panel;
#[cfg(target_os = "macos")]
#[path = "macos/glass.rs"]
pub mod glass;
pub mod material;
#[cfg(target_os = "macos")]
#[path = "macos/menu_bar.rs"]
pub mod menu_bar;
pub mod menu_spec;
pub mod monitors;
pub mod motion;
#[cfg(target_os = "macos")]
#[path = "macos/notify.rs"]
pub mod notify;
#[cfg(target_os = "macos")]
#[path = "macos/open_in.rs"]
pub mod open_in;
pub mod openers;
pub mod os_build;
#[cfg(target_os = "macos")]
#[path = "macos/page_overlay.rs"]
pub mod page_overlay;
#[cfg(target_os = "macos")]
#[path = "macos/pasteboard.rs"]
pub mod pasteboard;
pub mod paths;
pub mod pick;
pub mod procs;
pub mod scroll_phase;
#[cfg(windows)]
pub mod session_end;
#[cfg(not(windows))]
#[path = "macos/session_end.rs"]
pub mod session_end;
pub mod shell_open;
pub mod single_instance;
#[cfg(windows)]
pub mod sysmenu;
#[cfg(not(windows))]
#[path = "macos/sysmenu.rs"]
pub mod sysmenu;
pub mod system_caret;
pub mod taskbar;
pub mod taskbar_flash;
pub mod tcp_owner;
pub mod toast;
pub mod window_edge;
#[cfg(windows)]
pub mod window_park;
#[cfg(not(windows))]
#[path = "macos/window_park.rs"]
pub mod window_park;
pub mod window_state;
#[cfg(target_os = "macos")]
#[path = "macos/workspace.rs"]
pub mod workspace;

pub mod names;
pub mod packaged;
pub mod registry;
#[cfg(windows)]
pub mod resize_edges;
#[cfg(not(windows))]
#[path = "macos/resize_edges.rs"]
pub mod resize_edges;
pub mod seam;
#[cfg(not(target_os = "macos"))]
pub mod thread_stack;
#[cfg(target_os = "macos")]
#[path = "macos/thread_stack.rs"]
pub mod thread_stack;
pub mod verb;
pub mod verb_keys;
pub mod verb_switch;
