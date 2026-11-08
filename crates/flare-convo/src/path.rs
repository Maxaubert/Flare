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
    transcript_path_in(&home.join(".claude"), folder, session_id)
}

/// Claude Code's own folder: `config_dir` (the value of `CLAUDE_CONFIG_DIR`) when it is an absolute path, else
/// `<home>\.claude`. A relative or empty value is ignored, so a stray variable never points the reader at the tab's
/// working folder.
pub fn claude_dir(config_dir: Option<&str>, home: &Path) -> PathBuf {
    match config_dir.map(str::trim) {
        Some(c) if !c.is_empty() && Path::new(c).is_absolute() => PathBuf::from(c),
        _ => home.join(".claude"),
    }
}

/// Like [`transcript_path`], under a Claude folder that is already known (`claude_dir`).
pub fn transcript_path_in(claude_dir: &Path, folder: &str, session_id: &str) -> Option<PathBuf> {
    if !valid_session_id(session_id) {
        return None;
    }
    Some(
        claude_dir
            .join("projects")
            .join(encode_cwd(folder))
            .join(format!("{session_id}.jsonl")),
    )
}

/// The folder of a session's subagent transcripts: `<slug>\<session-id>\subagents`. Each subagent writes its own
/// `agent-<id>.jsonl` there; its lines are not in the session file. `None` when the id is not a safe file name.
pub fn subagent_dir(home: &Path, folder: &str, session_id: &str) -> Option<PathBuf> {
    subagent_dir_of(&transcript_path(home, folder, session_id)?)
}

/// The subagent folder that belongs to a transcript file: the file's name without `.jsonl`, then `subagents`. `None`
/// for a path with no file name.
pub fn subagent_dir_of(transcript: &Path) -> Option<PathBuf> {
    let stem = transcript.file_stem()?;
    Some(transcript.parent()?.join(stem).join("subagents"))
}

/// The fallback when the folder's slug is not known (a tab moved, the folder was renamed): the first
/// `<session-id>.jsonl` found in any project folder under `home`. `None` when no folder has one.
pub fn find_transcript(home: &Path, session_id: &str) -> Option<PathBuf> {
    find_transcript_in(&home.join(".claude"), session_id)
}

/// Like [`find_transcript`], under a Claude folder that is already known (`claude_dir`).
pub fn find_transcript_in(claude_dir: &Path, session_id: &str) -> Option<PathBuf> {
    if !valid_session_id(session_id) {
        return None;
    }
    let name = format!("{session_id}.jsonl");
    let listing = std::fs::read_dir(claude_dir.join("projects")); // sync-ok: runs on a worker (the conversation loader)
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

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";

    #[cfg_attr(
        not(windows),
        ignore = "Windows path grammar in path links and openers; macOS phase 2 of the port plan (POSIX paths)"
    )]
    #[test]
    fn the_claude_folder_is_the_config_dir_when_it_is_absolute() {
        let home = Path::new(r"C:\Users\x");
        assert_eq!(claude_dir(None, home), home.join(".claude"));
        assert_eq!(claude_dir(Some(""), home), home.join(".claude"));
        assert_eq!(claude_dir(Some("  "), home), home.join(".claude"));
        assert_eq!(claude_dir(Some(r"rel\dir"), home), home.join(".claude"));
        assert_eq!(
            claude_dir(Some(r"D:\cfg\claude"), home),
            PathBuf::from(r"D:\cfg\claude")
        );
    }

    #[cfg_attr(
        not(windows),
        ignore = "Windows path grammar in path links and openers; macOS phase 2 of the port plan (POSIX paths)"
    )]
    #[test]
    fn a_transcript_lives_under_projects_in_the_claude_folder() {
        let dir = Path::new(r"D:\cfg\claude");
        let t = transcript_path_in(dir, r"C:\my proj", ID).expect("a path");
        assert_eq!(
            t,
            PathBuf::from(format!(r"D:\cfg\claude\projects\C--my-proj\{ID}.jsonl"))
        );
        assert_eq!(
            subagent_dir_of(&t),
            Some(PathBuf::from(format!(
                r"D:\cfg\claude\projects\C--my-proj\{ID}\subagents"
            )))
        );
        assert!(transcript_path_in(dir, r"C:\x", r"..\y").is_none());
        // The home-based form is the same with `.claude` added.
        let home = Path::new(r"C:\Users\x");
        assert_eq!(
            transcript_path(home, r"C:\p", ID),
            transcript_path_in(&home.join(".claude"), r"C:\p", ID)
        );
        assert_eq!(
            subagent_dir(home, r"C:\p", ID),
            subagent_dir_of(&transcript_path(home, r"C:\p", ID).unwrap())
        );
    }
}
