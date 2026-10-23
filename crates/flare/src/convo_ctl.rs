//! ConvoCtl: the Changes and Tasks panels of the conversation (Surfaces stage 6).
//!
//! A tab whose open panel is Changes or Tasks, and whose Claude session is known, gets one transcript tracker
//! (`flare_convo::track`): a worker that follows the session's file and hands back snapshots. Nothing runs for a tab
//! without such a panel (the panels cost nothing while closed): closing the panel, switching it to another kind, losing
//! the session or closing the tab drops the tracker, which stops its thread.
//!
//! The snapshots arrive as `UiMsg::Convo`; this controller keeps the newest per tab and writes the front tab's into
//! the window's `panel-convo-*` properties. The diff of the selected file is made on the worker (the tracker is told
//! which file), so the UI thread only wraps rows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use flare_convo::track::{Snapshot, Source, Tracker};
use flare_convo::view::{DiffRow, FileSummary, LINE_CHARS, MAX_TASK_ROWS, RowKind, TaskGroup};
use flare_surfaces::PanelKind;
use serde_json::{Value, json};
use slint::{ModelRc, SharedString, VecModel};

use crate::appctl::{Post, UiMsg};
use crate::{AppWindow, ConvoDiffRow, ConvoFileRow, ConvoTaskRow};

/// The window's `panel-kind` of the two panels.
pub const KIND_CHANGES: i32 = 4;
pub const KIND_TASKS: i32 = 5;

/// The most characters of a diff line the viewport is made wide for.
const COLS_MAX: usize = LINE_CHARS;
/// The most changed files `convo.state` lists.
const STATE_FILES: usize = 20;

/// What a worker (or a click) tells the controller.
pub enum ConvoMsg {
    /// A tracker made a snapshot. `session` says which session it follows: one for a session since left is dropped.
    Snap {
        tab: String,
        session: String,
        snap: Arc<Snapshot>,
    },
    /// A path clicked in a diff, re-resolved on a worker (rule 13) and cleared to be shown (rule 14).
    OpenFile {
        tab: String,
        kind: PanelKind,
        abs: String,
        /// The file the diff showed when the person clicked; an answer for another pick (or a panel since changed) is
        /// stale and dropped.
        picked: Option<String>,
    },
}

/// A `ConvoMsg` that can sit in a `UiMsg` (which is `Debug`): it prints its kind only, never a path.
pub struct ConvoMsgBox(pub ConvoMsg);

impl std::fmt::Debug for ConvoMsgBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            ConvoMsg::Snap { .. } => "ConvoMsg::Snap",
            ConvoMsg::OpenFile { .. } => "ConvoMsg::OpenFile",
        })
    }
}

/// A tab that wants a tracker: its panel is open, and this is what is known of its agent.
pub struct Want {
    pub tab: String,
    /// The tab's folder (where Claude Code files the session).
    pub folder: String,
    /// The Claude session the hooks named, if any.
    pub session: Option<String>,
}

struct Tracked {
    session: String,
    tracker: Tracker,
    snap: Option<Arc<Snapshot>>,
    /// Count up when that piece of the snapshot changed, so the window rebuilds only the model that is out of date.
    files_v: u64,
    diff_v: u64,
    tasks_v: u64,
}

/// What the person did in a tab's panel; it lasts as long as the tracker.
#[derive(Default)]
struct TabView {
    selected: Option<String>,
    /// Subagent groups folded shut, by key.
    collapsed: HashSet<String>,
    /// Counts up with each pick or fold, so the window knows its rows are out of date.
    version: u64,
}

/// What the window was last given, so a refresh that changes nothing rebuilds nothing.
#[derive(PartialEq, Eq)]
struct Pushed {
    tab: String,
    kind: i32,
    seq: u64,
    session: Option<String>,
    version: u64,
    files_v: u64,
    diff_v: u64,
    tasks_v: u64,
}

pub struct ConvoCtl {
    post: Post,
    /// The Claude folder to read (tests); None: `CLAUDE_CONFIG_DIR`, else `<profile>\.claude`.
    claude_dir: Option<PathBuf>,
    tracked: HashMap<String, Tracked>,
    views: HashMap<String, TabView>,
    pushed: Option<Pushed>,
    /// The window's models hold rows of ours.
    populated: bool,
}

impl ConvoCtl {
    pub fn new(post: Post) -> Self {
        Self {
            post,
            claude_dir: None,
            tracked: HashMap::new(),
            views: HashMap::new(),
            pushed: None,
            populated: false,
        }
    }

    /// Points the reader at a Claude folder of its own (a test's temp folder).
    #[cfg(test)]
    pub fn set_claude_dir(&mut self, dir: Option<PathBuf>) {
        self.claude_dir = dir;
    }

    /// The Claude folder: the one set, else `CLAUDE_CONFIG_DIR` (a Claude Code setting every tab inherits), else
    /// `<profile>\.claude`.
    fn claude_dir(&self) -> Option<PathBuf> {
        if let Some(d) = &self.claude_dir {
            return Some(d.clone());
        }
        let home = flare_agent::resume::claude_home()?;
        let cfg = std::env::var("CLAUDE_CONFIG_DIR").ok();
        Some(flare_convo::claude_dir(cfg.as_deref(), &home))
    }

    /// Brings the trackers in line with the tabs that want one. True when a tracker started or stopped.
    pub fn sync(&mut self, wants: &[Want]) -> bool {
        let mut changed = false;
        let live: HashSet<&str> = wants
            .iter()
            .filter(|w| w.session.is_some())
            .map(|w| w.tab.as_str())
            .collect();
        let before = self.tracked.len();
        self.tracked.retain(|tab, _| live.contains(tab.as_str()));
        changed |= self.tracked.len() != before;
        // A tab without a panel loses what the person picked in it; one that only lacks a session keeps it.
        let open: HashSet<&str> = wants.iter().map(|w| w.tab.as_str()).collect();
        self.views.retain(|tab, _| open.contains(tab.as_str()));
        let Some(dir) = self.claude_dir() else {
            return changed;
        };
        for w in wants {
            let Some(session) = w.session.as_deref() else {
                continue;
            };
            if self
                .tracked
                .get(&w.tab)
                .is_some_and(|t| t.session == session)
            {
                continue;
            }
            // A new session (a /clear, a resume, the first prompt): the old file and the pick are let go.
            self.views.remove(&w.tab);
            let Some(transcript) = flare_convo::transcript_path_in(&dir, &w.folder, session) else {
                self.tracked.remove(&w.tab);
                changed = true;
                continue;
            };
            let post = Arc::clone(&self.post);
            let (tab, sess) = (w.tab.clone(), session.to_string());
            let tracker = Tracker::start(
                Source {
                    transcript,
                    claude_dir: dir.clone(),
                    session: session.to_string(),
                },
                Arc::new(move |snap| {
                    post(UiMsg::Convo(ConvoMsgBox(ConvoMsg::Snap {
                        tab: tab.clone(),
                        session: sess.clone(),
                        snap: Arc::new(snap),
                    })));
                }),
            );
            self.tracked.insert(
                w.tab.clone(),
                Tracked {
                    session: session.to_string(),
                    tracker,
                    snap: None,
                    files_v: 0,
                    diff_v: 0,
                    tasks_v: 0,
                },
            );
            changed = true;
        }
        changed
    }

    /// A tab is gone.
    pub fn forget(&mut self, tab: &str) {
        self.tracked.remove(tab);
        self.views.remove(tab);
    }

    /// How many trackers run (the tests, and the "nothing runs when no panel is open" check).
    pub fn tracker_count(&self) -> usize {
        self.tracked.len()
    }

    /// The sessions the trackers follow, sorted (the e2e probe).
    pub fn sessions(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tracked.values().map(|t| t.session.clone()).collect();
        v.sort();
        v
    }

    /// The session tab's tracker follows.
    #[cfg(test)]
    pub fn session_of(&self, tab: &str) -> Option<&str> {
        self.tracked.get(tab).map(|t| t.session.as_str())
    }

    /// The file picked in the tab's Changes panel.
    pub fn selected(&self, tab: &str) -> Option<String> {
        self.views.get(tab).and_then(|v| v.selected.clone())
    }

    /// A tracker's exit flag (a test waits for it to see the worker really stop).
    #[cfg(test)]
    pub fn exit_flag(&self, tab: &str) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        self.tracked.get(tab).map(|t| t.tracker.exit_flag())
    }

    /// Whether any tracker runs.
    pub fn active(&self) -> bool {
        !self.tracked.is_empty()
    }

    /// A tracker's snapshot. False when it is for a session the tab has left, or changes nothing on show.
    pub fn on_snap(&mut self, tab: &str, session: &str, mut snap: Arc<Snapshot>) -> bool {
        let Some(t) = self.tracked.get_mut(tab) else {
            return false;
        };
        if t.session != session {
            return false;
        }
        // The same content again (the worker woke for a pick that is already on show): keep what is shown.
        if let Some(old) = &t.snap
            && same_content(old, &snap)
        {
            return false;
        }
        // The file the person picked is no longer in the list (the session started again): back to the list.
        if let Some(view) = self.views.get_mut(tab)
            && let Some(sel) = view.selected.clone()
            && !snap.files.iter().any(|f| same_path(&f.path, &sel))
            && snap.found
        {
            view.selected = None;
            view.version += 1;
            t.tracker.select(None);
            snap = Arc::new(Snapshot {
                diff: None,
                ..(*snap).clone()
            });
        }
        // Which models the window has to rebuild: only the ones whose piece changed (rule 34).
        match &t.snap {
            Some(o) => {
                t.files_v += u64::from(o.files != snap.files);
                t.diff_v += u64::from(o.diff != snap.diff);
                t.tasks_v += u64::from(o.groups != snap.groups);
            }
            None => {
                t.files_v += 1;
                t.diff_v += 1;
                t.tasks_v += 1;
            }
        }
        t.snap = Some(snap);
        true
    }

    /// The models the window holds, as (files, rows, tasks) rebuild counts (a test reads them).
    #[cfg(test)]
    pub fn piece_versions(&self, tab: &str) -> Option<(u64, u64, u64)> {
        self.tracked
            .get(tab)
            .map(|t| (t.files_v, t.diff_v, t.tasks_v))
    }

    /// Shows one file's diff (None: the list).
    pub fn select(&mut self, tab: &str, path: Option<String>) -> bool {
        let Some(t) = self.tracked.get(tab) else {
            return false;
        };
        let view = self.views.entry(tab.to_string()).or_default();
        if view.selected == path {
            return false;
        }
        view.selected = path.clone();
        view.version += 1;
        t.tracker.select(path);
        true
    }

    /// Folds a subagent's list shut, or opens it again.
    pub fn toggle(&mut self, tab: &str, key: &str) -> bool {
        if !self.tracked.contains_key(tab) {
            return false;
        }
        let view = self.views.entry(tab.to_string()).or_default();
        if !view.collapsed.remove(key) {
            view.collapsed.insert(key.to_string());
        }
        view.version += 1;
        true
    }

    /// What `convo.state` answers for the tab.
    pub fn state_json(&self, tab: &str) -> Value {
        let Some(t) = self.tracked.get(tab) else {
            return json!({
                "tracking": false,
                "note": "Flare reads the conversation only while the Changes or Tasks panel is open on this tab. \
                         panel.open with kind changes or tasks opens it.",
            });
        };
        let Some(s) = &t.snap else {
            return json!({"tracking": true, "session": t.session, "found": false, "files": 0,
                "edits": 0, "failed": 0, "tasks": {"pending": 0, "in_progress": 0, "done": 0, "other": 0},
                "changed": []});
        };
        let changed: Vec<Value> = s
            .files
            .iter()
            .take(STATE_FILES)
            .map(|f| json!({"path": f.path, "edits": f.edits, "failed": f.failed}))
            .collect();
        let mut v = json!({
            "tracking": true,
            "session": t.session,
            "found": s.found,
            "files": s.summary.files,
            "edits": s.summary.edits,
            "failed": s.summary.failed,
            "tasks": {
                "pending": s.summary.tasks.pending,
                "in_progress": s.summary.tasks.in_progress,
                "done": s.summary.tasks.done,
                "other": s.summary.tasks.other,
            },
            "subagent_lists": s.summary.subagent_lists,
            "changed": changed,
        });
        if let Some(a) = &s.summary.active {
            v["active"] = json!(a);
        }
        v
    }

    /// Writes the front tab's panel into the window, or lets go of the rows when the front shows no conversation.
    /// `front` is the tab in front and the kind of its OPEN panel, when that kind is one of ours.
    pub fn push(&mut self, app: &AppWindow, front: Option<(&str, &PanelKind)>) {
        let Some((tab, kind)) = front.filter(|(_, k)| k.is_convo()) else {
            self.release(app);
            return;
        };
        let code = if *kind == PanelKind::Changes {
            KIND_CHANGES
        } else {
            KIND_TASKS
        };
        let t = self.tracked.get(tab);
        let view = self.views.get(tab);
        let key = Pushed {
            tab: tab.to_string(),
            kind: code,
            seq: t.and_then(|t| t.snap.as_ref()).map_or(0, |s| s.seq),
            session: t.map(|t| t.session.clone()),
            version: view.map_or(0, |v| v.version),
            files_v: t.map_or(0, |t| t.files_v),
            diff_v: t.map_or(0, |t| t.diff_v),
            tasks_v: t.map_or(0, |t| t.tasks_v),
        };
        if self.pushed.as_ref() == Some(&key) && app.get_panel_kind() == code {
            return;
        }
        // A model is written again only when its piece changed (or the tab, kind, session or the person's pick did).
        let (all, files_new, diff_new, tasks_new) = match &self.pushed {
            Some(p)
                if self.populated
                    && p.tab == key.tab
                    && p.kind == key.kind
                    && p.session == key.session
                    && app.get_panel_kind() == code =>
            {
                let moved = p.version != key.version;
                (
                    false,
                    p.files_v != key.files_v,
                    moved || p.diff_v != key.diff_v,
                    moved || p.tasks_v != key.tasks_v,
                )
            }
            _ => (true, true, true, true),
        };
        let snap = t.and_then(|t| t.snap.as_ref());
        let sel = view.and_then(|v| v.selected.clone());
        let none = TabView::default();
        let view = view.unwrap_or(&none);

        app.set_panel_kind(code);
        app.set_panel_picture(slint::Image::default());
        app.set_panel_alpha(false);
        app.set_panel_actual(false);
        app.set_panel_lines(ModelRc::default());
        app.set_panel_max_cols(0);
        app.set_panel_foot("".into());
        let (note, sub);
        match kind {
            PanelKind::Changes => {
                let list: &[FileSummary] = snap.map_or(&[], |s| s.files.as_slice());
                if files_new {
                    app.set_panel_convo_files(model(file_rows(list)));
                }
                if diff_new {
                    let (r, c) = match (&sel, snap.and_then(|s| s.diff.as_ref())) {
                        (Some(sel), Some((p, d))) if same_path(p, sel) => diff_rows(&d.rows),
                        _ => (Vec::new(), 0),
                    };
                    app.set_panel_convo_rows(model(r));
                    app.set_panel_convo_max_cols(i32::try_from(c).unwrap_or(i32::MAX));
                }
                if all {
                    app.set_panel_convo_tasks(ModelRc::default());
                }
                sub = match list.len() {
                    0 => String::new(),
                    1 => "1 file".to_string(),
                    n => format!("{n} files"),
                };
                note = if list.is_empty() {
                    note_for(t.is_some(), snap, "Claude has not changed a file yet.")
                } else {
                    String::new()
                };
                app.set_panel_title("Changes".into());
            }
            _ => {
                if all {
                    app.set_panel_convo_files(ModelRc::default());
                    app.set_panel_convo_rows(ModelRc::default());
                    app.set_panel_convo_max_cols(0);
                }
                let groups: &[TaskGroup] = snap.map_or(&[], |s| s.groups.as_slice());
                if tasks_new {
                    app.set_panel_convo_tasks(model(task_rows(groups, &view.collapsed)));
                }
                sub = snap
                    .and_then(|s| s.groups.first().filter(|g| g.key.is_empty()))
                    .map(|g| format!("{} of {} done", g.done, g.tasks.len()))
                    .unwrap_or_default();
                note = if groups.is_empty() {
                    note_for(t.is_some(), snap, "Claude has no task list yet.")
                } else {
                    String::new()
                };
                app.set_panel_title("Tasks".into());
            }
        }
        app.set_panel_sub(sub.into());
        app.set_panel_note(note.into());
        app.set_panel_convo_selected(sel.unwrap_or_default().into());
        self.populated = true;
        self.pushed = Some(key);
    }

    /// The window shows no conversation: its rows go, and the panel kind goes back to nothing when it was ours.
    fn release(&mut self, app: &AppWindow) {
        self.pushed = None;
        if !self.populated {
            return;
        }
        self.populated = false;
        app.set_panel_convo_files(ModelRc::default());
        app.set_panel_convo_rows(ModelRc::default());
        app.set_panel_convo_tasks(ModelRc::default());
        app.set_panel_convo_selected("".into());
        app.set_panel_convo_max_cols(0);
        if matches!(app.get_panel_kind(), KIND_CHANGES | KIND_TASKS) {
            app.set_panel_kind(0);
            app.set_panel_title("".into());
            app.set_panel_sub("".into());
            app.set_panel_note("".into());
        }
    }
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// The empty-state words for a panel that has nothing to list yet.
fn note_for(tracking: bool, snap: Option<&Arc<Snapshot>>, empty: &str) -> String {
    if !tracking {
        return "Claude is not running in this tab yet. What it changes and plans will show here."
            .to_string();
    }
    match snap {
        None => "Reading the conversation\u{2026}".to_string(),
        Some(s) if !s.found => "Waiting for the conversation file.".to_string(),
        Some(_) => empty.to_string(),
    }
}

/// Two spellings of one Windows path.
pub fn same_path(a: &str, b: &str) -> bool {
    a.replace('/', "\\")
        .eq_ignore_ascii_case(&b.replace('/', "\\"))
}

/// Everything a window shows is equal (the sequence number aside).
fn same_content(a: &Snapshot, b: &Snapshot) -> bool {
    a.found == b.found
        && a.files == b.files
        && a.groups == b.groups
        && a.summary == b.summary
        && a.diff == b.diff
}

pub fn file_rows(files: &[FileSummary]) -> Vec<ConvoFileRow> {
    files
        .iter()
        .map(|f| {
            let edits = if f.edits == 1 {
                "1 edit".to_string()
            } else {
                format!("{} edits", f.edits)
            };
            let meta = if f.failed > 0 {
                format!("{edits}, {} failed", f.failed)
            } else {
                edits
            };
            ConvoFileRow {
                path: f.path.as_str().into(),
                name: f.name.as_str().into(),
                dir: f.dir.as_str().into(),
                meta: meta.into(),
                failed: f.failed > 0,
            }
        })
        .collect()
}

/// The rows and the widest line in characters.
pub fn diff_rows(rows: &[DiffRow]) -> (Vec<ConvoDiffRow>, usize) {
    let mut cols = 0;
    let out = rows
        .iter()
        .map(|r| {
            cols = cols.max(r.text.chars().count().min(COLS_MAX));
            let no = |n: u32| {
                if n == 0 {
                    SharedString::new()
                } else {
                    n.to_string().into()
                }
            };
            ConvoDiffRow {
                kind: match r.kind {
                    RowKind::Header => 0,
                    RowKind::Context => 1,
                    RowKind::Removed => 2,
                    RowKind::Added => 3,
                    RowKind::Note => 4,
                },
                old_no: no(r.old_no),
                new_no: no(r.new_no),
                text: r.text.as_str().into(),
            }
        })
        .collect();
    (out, cols)
}

/// The task list as rows: a header per group when there is more than one list, then the group's tasks unless folded.
pub fn task_rows(groups: &[TaskGroup], collapsed: &HashSet<String>) -> Vec<ConvoTaskRow> {
    let headers = groups.len() > 1;
    let mut out: Vec<ConvoTaskRow> = Vec::new();
    for g in groups {
        let folded = headers && collapsed.contains(&g.key);
        if headers {
            out.push(ConvoTaskRow {
                kind: 0,
                state: 0,
                text: g.title.as_str().into(),
                sub: SharedString::new(),
                active: false,
                key: g.key.as_str().into(),
                collapsed: folded,
                count: format!("{} of {} done", g.done, g.tasks.len()).into(),
            });
        }
        if folded {
            continue;
        }
        for t in &g.tasks {
            if out.len() >= MAX_TASK_ROWS {
                return out;
            }
            out.push(ConvoTaskRow {
                kind: 1,
                state: t.state.code(),
                text: t.title.as_str().into(),
                sub: if t.doing == t.title {
                    SharedString::new()
                } else {
                    t.doing.as_str().into()
                },
                active: t.active,
                key: SharedString::new(),
                collapsed: false,
                count: SharedString::new(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_convo::fold::Convo;
    use flare_convo::view;

    fn convo_with_two_lists() -> Convo {
        let mut c = Convo::new();
        c.fold_line(
            &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t",
                "name": "TodoWrite", "input": {"todos": [
                    {"content": "Read", "status": "completed", "activeForm": "Reading"},
                    {"content": "Edit", "status": "in_progress", "activeForm": "Editing the file"},
                    {"content": "Test", "status": "pending", "activeForm": "Testing"}]}}]}})
            .to_string(),
        );
        c.fold_from(
            "agent-abcdef0123.jsonl",
            &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "s",
                "name": "TodoWrite", "input": {"todos": [
                    {"content": "Look", "status": "pending", "activeForm": "Looking"}]}}]}})
            .to_string(),
        );
        c
    }

    #[test]
    fn a_single_list_has_no_header_and_marks_the_active_task() {
        let mut c = Convo::new();
        c.fold_line(
            &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t",
                "name": "TodoWrite", "input": {"todos": [
                    {"content": "Edit", "status": "in_progress", "activeForm": "Editing"}]}}]}})
            .to_string(),
        );
        let rows = task_rows(&view::task_groups(&c), &HashSet::new());
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].kind, rows[0].state, rows[0].active), (1, 1, true));
        assert_eq!(rows[0].sub.as_str(), "Editing");
    }

    #[test]
    fn two_lists_get_headers_and_a_folded_one_hides_its_tasks() {
        let groups = view::task_groups(&convo_with_two_lists());
        let open = task_rows(&groups, &HashSet::new());
        assert_eq!(
            open.len(),
            2 + 3 + 1,
            "two headers, three main tasks, one subagent task"
        );
        assert_eq!(open[0].kind, 0);
        assert_eq!(open[0].count.as_str(), "1 of 3 done");
        assert_eq!(open[4].kind, 0);
        assert_eq!(open[4].key.as_str(), "agent-abcdef0123.jsonl");
        assert_eq!(open[4].text.as_str(), "Subagent abcdef0");
        let fold: HashSet<String> = ["agent-abcdef0123.jsonl".to_string()].into();
        let folded = task_rows(&groups, &fold);
        assert_eq!(folded.len(), 5, "the subagent's task is hidden");
        assert!(folded[4].collapsed);
        let fold_main: HashSet<String> = [String::new()].into();
        assert_eq!(
            task_rows(&groups, &fold_main).len(),
            3,
            "the main list folds too"
        );
    }

    #[test]
    fn a_long_list_is_cut_at_the_row_cap() {
        let todos: Vec<Value> = (0..800)
            .map(|i| json!({"content": format!("t{i}"), "status": "pending", "activeForm": "x"}))
            .collect();
        let mut c = Convo::new();
        c.fold_line(
            &json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t",
                "name": "TodoWrite", "input": {"todos": todos}}]}})
            .to_string(),
        );
        assert_eq!(
            task_rows(&view::task_groups(&c), &HashSet::new()).len(),
            MAX_TASK_ROWS
        );
    }

    #[test]
    fn file_rows_say_how_many_edits_and_how_many_failed() {
        let f = |edits, failed| FileSummary {
            path: r"C:\p\a.rs".into(),
            name: "a.rs".into(),
            dir: r"C:\p".into(),
            edits,
            failed,
            by_subagent: false,
        };
        let rows = file_rows(&[f(1, 0), f(3, 0), f(2, 1)]);
        let meta: Vec<&str> = rows.iter().map(|r| r.meta.as_str()).collect();
        assert_eq!(meta, ["1 edit", "3 edits", "2 edits, 1 failed"]);
        assert!(!rows[0].failed && rows[2].failed);
    }

    #[test]
    fn diff_rows_carry_kind_and_blank_numbers_and_the_widest_line() {
        let rows = vec![
            DiffRow {
                kind: RowKind::Header,
                old_no: 0,
                new_no: 0,
                text: "Edit".into(),
            },
            DiffRow {
                kind: RowKind::Context,
                old_no: 4,
                new_no: 4,
                text: "ab".into(),
            },
            DiffRow {
                kind: RowKind::Removed,
                old_no: 5,
                new_no: 0,
                text: "abcdefg".into(),
            },
            DiffRow {
                kind: RowKind::Added,
                old_no: 0,
                new_no: 5,
                text: "x".into(),
            },
        ];
        let (out, cols) = diff_rows(&rows);
        let kinds: Vec<i32> = out.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [0, 1, 2, 3]);
        assert_eq!(out[2].new_no.as_str(), "");
        assert_eq!(out[3].old_no.as_str(), "");
        assert_eq!(out[3].new_no.as_str(), "5");
        assert_eq!(cols, 7);
    }

    #[test]
    fn a_capped_diff_gives_five_thousand_rows_no_wider_than_the_viewport() {
        let wide = "w".repeat(30 * 1024);
        let lines: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        let big = format!("{wide}\n{lines}");
        let mut c = Convo::new();
        for i in 0..4 {
            c.fold_line(
                &json!({"type": "assistant", "message": {"content": [{"type": "tool_use",
                    "id": format!("e{i}"), "name": "Edit",
                    "input": {"file_path": r"C:\p\big.txt", "old_string": "", "new_string": big}}]}})
                .to_string(),
            );
        }
        let d = view::file_diff(&c, r"C:\p\big.txt");
        assert!(d.truncated);
        let (rows, cols) = diff_rows(&d.rows);
        assert_eq!(
            rows.len(),
            view::MAX_ROWS + 1,
            "the cap and the closing note"
        );
        assert_eq!(cols, COLS_MAX);
        assert!(
            rows.iter().all(|r| r.text.chars().count() <= COLS_MAX),
            "no row hands the window more than a viewport of text"
        );
    }

    #[test]
    fn the_empty_words_say_what_is_missing() {
        assert!(note_for(false, None, "x").starts_with("Claude is not running"));
        assert!(note_for(true, None, "x").starts_with("Reading"));
        let missing = Arc::new(Snapshot::default());
        assert!(note_for(true, Some(&missing), "x").starts_with("Waiting"));
        let found = Arc::new(Snapshot {
            found: true,
            ..Snapshot::default()
        });
        assert_eq!(note_for(true, Some(&found), "nothing yet"), "nothing yet");
    }

    #[test]
    fn two_spellings_of_a_path_are_one() {
        assert!(same_path(r"C:\p\A.rs", "c:/p/a.rs"));
        assert!(!same_path(r"C:\p\a.rs", r"C:\p\b.rs"));
    }
}
