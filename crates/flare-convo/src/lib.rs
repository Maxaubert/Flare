//! flare-convo: the conversation reader's library (Surfaces stage 6, no UI).
//!
//! Three pieces, all plain Rust with no window or thread of the app's own:
//! - `path`: where Claude Code keeps a session's transcript (the folder slug is `flare_agent::resume::encode_cwd`).
//! - `tail`: reads a growing transcript from an offset, bounded, restarting on truncation or replacement, with worker
//!   helpers (`tail::subscribe`, `tail::subscribe_session` for the session file and its subagent files) that poll only
//!   while someone listens and hold a bounded queue.
//! - `fold`: parses each line leniently and folds it into the model (edits, tasks, named files).
//! - `diff`: a line diff of one edit, as capped hunks for the Changes panel.
//!
//! Fold and diff are not cheap (a diff can run a table of 4 million cells): call them on the tail worker or a
//! background thread, never on the UI thread.
//!
//! The transcript format as Flare reads it, with what is known and what is guessed: `docs/surfaces/convo.md`.

pub mod diff;
pub mod fold;
pub mod path;
pub mod tail;

pub use diff::{Diff, DiffLine, Hunk, Kind};
pub use fold::{Convo, Edit, EditOp, NamedFile, Outcome, Task, TaskStatus, Tool};
pub use path::{find_transcript, subagent_dir, transcript_path, valid_session_id};
