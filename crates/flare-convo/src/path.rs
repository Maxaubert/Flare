//! Where Claude Code keeps a session's transcript: `<home>\.claude\projects\<slug>\<session-id>.jsonl`.
//!
//! The slug is the folder with every non-alphanumeric UTF-16 unit replaced by `-` (the same rule the resume lookup
//! uses; this module calls `flare_agent::resume::encode_cwd` and does not copy it).

use std::path::{Path, PathBuf};

use flare_agent::resume::encode_cwd;

/// A session id is a UUID in practice. Anything else (a separator, a dot, a space) never becomes a path.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The transcript of `session_id` for the tab folder `folder`, under `home` (the user's profile folder, the one
/// `flare_agent::resume::claude_home` returns). `None` when the id is not a safe file name.
pub fn transcript_path(home: &Path, folder: &str, session_id: &str) -> Option<PathBuf> {
    if !valid_session_id(session_id) {
        return None;
    }
    Some(
        projects_dir(home)
            .join(encode_cwd(folder))
            .join(format!("{session_id}.jsonl")),
    )
}

/// The fallback when the folder's slug is not known (a tab moved, the folder was renamed): the first
/// `<session-id>.jsonl` found in any project folder under `home`. `None` when no folder has one.
pub fn find_transcript(home: &Path, session_id: &str) -> Option<PathBuf> {
    if !valid_session_id(session_id) {
        return None;
    }
    let name = format!("{session_id}.jsonl");
    let listing = std::fs::read_dir(projects_dir(home)); // sync-ok: runs on a worker (the conversation loader)
    let Ok(folders) = listing else {
        return None;
    };
    for folder in folders.flatten() {
        let candidate = folder.path().join(&name);
        if candidate.is_file() {
            // sync-ok: runs on a worker (the conversation loader)
            return Some(candidate);
        }
    }
    None
}

fn projects_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("projects")
}
