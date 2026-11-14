//! The agent prompt as a text box (Max's picks, 2026-11-14; rule 1235). With agent hotkeys on and Claude in the
//! tab in front, Ctrl+A selects the prompt being typed, Shift and the arrows select, typing over a selection
//! replaces it, Ctrl+X cuts, Ctrl+Z undoes, and a few keys are translated to Claude Code's own editor keys.
//!
//! Claude Code has no selection of its own, so Flare keeps one: its own selection highlight over the prompt's
//! cells, and the char range it stands for. A selection is deleted the way rule 17 deletes one on a shell's line:
//! the caret walks to its end with arrows, then one Backspace per character.
//!
//! Pure: the screen comes in as `BoxLine`s, the answer goes out as a `Plan` (bytes for the pty, the selection
//! to show, text to copy, and whether the key then goes on its usual way).
//!
//! Claude Code's editor keys, from its interactive-mode docs (code.claude.com/docs/en/interactive-mode, read
//! 2026-11-14): Ctrl+A and Ctrl+E go to the start and end of the line, Alt+B and Alt+F jump a word back and
//! forward, Alt+D deletes to the end of the word, Ctrl+Backspace (Windows, ^H) and Option+Delete (macOS, ESC DEL)
//! delete the word before the caret, Ctrl+_ undoes the last edit. A word is a run of letters and digits.

use crate::click_caret::arrow_keys;
use crate::keys::{KeyEvent, KeyOs, is_key};

/// A prompt key: what it does (the key that does it depends on the system, see `prompt_key_of`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptKey {
    /// Ctrl+A (Cmd+A): the whole prompt is selected.
    SelectAll,
    /// Shift+Left and Shift+Right: the selection grows or shrinks by a letter.
    SelectLeft,
    SelectRight,
    /// Ctrl+Shift+Left and Right (Option+Shift on a Mac): by a word.
    SelectWordLeft,
    SelectWordRight,
    /// Shift+Home and Shift+End (Cmd+Shift+Left and Right on a Mac): to the line's start or end.
    SelectHome,
    SelectEnd,
    /// Ctrl+Left and Right (Option on a Mac): a word back or forward (Alt+B, Alt+F).
    WordLeft,
    WordRight,
    /// Home and End (Cmd+Left and Right on a Mac): the line's start or end (Ctrl+A, Ctrl+E).
    Home,
    End,
    /// Ctrl+Backspace (Option+Backspace): the word before the caret.
    DeleteWordBack,
    /// Ctrl+Delete (Option+Delete): the word after the caret (Alt+D).
    DeleteWordForward,
    /// Ctrl+Z (Cmd+Z): Claude Code's undo (Ctrl+_).
    Undo,
    /// Ctrl+X (Cmd+X): copy the selection, then delete it. Without a selection the key is Claude's.
    Cut,
    /// Ctrl+Shift+Backspace (Cmd+Shift+Backspace): the whole prompt goes, the agent keeps working.
    ClearPrompt,
    /// Ctrl+Shift+Up and Down (Cmd+Shift on a Mac): the view to your previous or next message.
    PrevMessage,
    NextMessage,
}

/// Every prompt key, in the order the Settings list names them.
pub const PROMPT_KEYS: [PromptKey; 18] = [
    PromptKey::SelectAll,
    PromptKey::SelectLeft,
    PromptKey::SelectRight,
    PromptKey::SelectWordLeft,
    PromptKey::SelectWordRight,
    PromptKey::SelectHome,
    PromptKey::SelectEnd,
    PromptKey::WordLeft,
    PromptKey::WordRight,
    PromptKey::Home,
    PromptKey::End,
    PromptKey::DeleteWordBack,
    PromptKey::DeleteWordForward,
    PromptKey::Undo,
    PromptKey::Cut,
    PromptKey::ClearPrompt,
    PromptKey::PrevMessage,
    PromptKey::NextMessage,
];

/// The Mac's Cmd+Backspace in its glyphs: it deletes to the line's start (`Input::DeleteLineBack`, Mac only).
pub const MAC_DELETE_LINE_LABEL: &str = "\u{2318}\u{232b}";

/// What the UI calls a prompt key on this system (the Mac in its glyphs).
pub fn prompt_key_label(os: KeyOs, k: PromptKey) -> &'static str {
    use PromptKey as P;
    match os {
        KeyOs::Windows => match k {
            P::SelectAll => "Ctrl+A",
            P::SelectLeft => "Shift+Left",
            P::SelectRight => "Shift+Right",
            P::SelectWordLeft => "Ctrl+Shift+Left",
            P::SelectWordRight => "Ctrl+Shift+Right",
            P::SelectHome => "Shift+Home",
            P::SelectEnd => "Shift+End",
            P::WordLeft => "Ctrl+Left",
            P::WordRight => "Ctrl+Right",
            P::Home => "Home",
            P::End => "End",
            P::DeleteWordBack => "Ctrl+Backspace",
            P::DeleteWordForward => "Ctrl+Delete",
            P::Undo => "Ctrl+Z",
            P::Cut => "Ctrl+X",
            P::ClearPrompt => "Ctrl+Shift+Backspace",
            P::PrevMessage => "Ctrl+Shift+Up",
            P::NextMessage => "Ctrl+Shift+Down",
        },
        KeyOs::Mac => match k {
            P::SelectAll => "\u{2318}A",
            P::SelectLeft => "\u{21e7}\u{2190}",
            P::SelectRight => "\u{21e7}\u{2192}",
            P::SelectWordLeft => "\u{2325}\u{21e7}\u{2190}",
            P::SelectWordRight => "\u{2325}\u{21e7}\u{2192}",
            P::SelectHome => "\u{21e7}\u{2318}\u{2190}",
            P::SelectEnd => "\u{21e7}\u{2318}\u{2192}",
            P::WordLeft => "\u{2325}\u{2190}",
            P::WordRight => "\u{2325}\u{2192}",
            P::Home => "\u{2318}\u{2190}",
            P::End => "\u{2318}\u{2192}",
            P::DeleteWordBack => "\u{2325}\u{232b}",
            P::DeleteWordForward => "\u{2325}\u{2326}",
            P::Undo => "\u{2318}Z",
            P::Cut => "\u{2318}X",
            P::ClearPrompt => "\u{21e7}\u{2318}\u{232b}",
            P::PrevMessage => "\u{21e7}\u{2318}\u{2191}",
            P::NextMessage => "\u{21e7}\u{2318}\u{2193}",
        },
    }
}

fn mods(e: &KeyEvent) -> (bool, bool, bool, bool) {
    (e.ctrl, e.shift, e.alt, e.meta)
}

/// The prompt key this key press is, on this system; None for any other key.
pub fn prompt_key_of(e: &KeyEvent, os: KeyOs) -> Option<PromptKey> {
    use PromptKey as P;
    if !e.pressed {
        return None;
    }
    let key = e.key.as_str();
    match os {
        KeyOs::Windows => match (mods(e), key) {
            ((true, false, false, false), _) if is_key(e, 'a') => Some(P::SelectAll),
            ((true, false, false, false), _) if is_key(e, 'z') => Some(P::Undo),
            ((true, false, false, false), _) if is_key(e, 'x') => Some(P::Cut),
            ((false, true, false, false), "ArrowLeft") => Some(P::SelectLeft),
            ((false, true, false, false), "ArrowRight") => Some(P::SelectRight),
            ((true, true, false, false), "ArrowLeft") => Some(P::SelectWordLeft),
            ((true, true, false, false), "ArrowRight") => Some(P::SelectWordRight),
            ((false, true, false, false), "Home") => Some(P::SelectHome),
            ((false, true, false, false), "End") => Some(P::SelectEnd),
            ((true, false, false, false), "ArrowLeft") => Some(P::WordLeft),
            ((true, false, false, false), "ArrowRight") => Some(P::WordRight),
            ((false, false, false, false), "Home") => Some(P::Home),
            ((false, false, false, false), "End") => Some(P::End),
            ((true, false, false, false), "Backspace") => Some(P::DeleteWordBack),
            ((true, false, false, false), "Delete") => Some(P::DeleteWordForward),
            ((true, true, false, false), "Backspace") => Some(P::ClearPrompt),
            ((true, true, false, false), "ArrowUp") => Some(P::PrevMessage),
            ((true, true, false, false), "ArrowDown") => Some(P::NextMessage),
            _ => None,
        },
        KeyOs::Mac => {
            match (mods(e), key) {
                ((false, false, false, true), _) if is_key(e, 'a') => Some(P::SelectAll),
                ((false, false, false, true), _) if is_key(e, 'z') => Some(P::Undo),
                ((false, false, false, true), _) if is_key(e, 'x') => Some(P::Cut),
                ((false, true, false, false), "ArrowLeft") => Some(P::SelectLeft),
                ((false, true, false, false), "ArrowRight") => Some(P::SelectRight),
                ((false, true, true, false), "ArrowLeft") => Some(P::SelectWordLeft),
                ((false, true, true, false), "ArrowRight") => Some(P::SelectWordRight),
                ((false, true, false, true), "ArrowLeft")
                | ((false, true, false, false), "Home") => Some(P::SelectHome),
                ((false, true, false, true), "ArrowRight")
                | ((false, true, false, false), "End") => Some(P::SelectEnd),
                ((false, false, true, false), "ArrowLeft") => Some(P::WordLeft),
                ((false, false, true, false), "ArrowRight") => Some(P::WordRight),
                ((false, false, false, true), "ArrowLeft")
                | ((false, false, false, false), "Home") => Some(P::Home),
                ((false, false, false, true), "ArrowRight")
                | ((false, false, false, false), "End") => Some(P::End),
                ((false, false, true, false), "Backspace") => Some(P::DeleteWordBack),
                ((false, false, true, false), "Delete") => Some(P::DeleteWordForward),
                ((false, true, false, true), "Backspace") => Some(P::ClearPrompt),
                ((false, true, false, true), "ArrowUp") => Some(P::PrevMessage),
                ((false, true, false, true), "ArrowDown") => Some(P::NextMessage),
                _ => None,
            }
        }
    }
}

// ---- the prompt on the screen ------------------------------------------------------------------------------------

/// One cell of a screen row, as the prompt reader needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxCell {
    /// The character (' ' for an empty cell).
    pub c: char,
    /// 1, 2 for a wide character, 0 for the spacer after one.
    pub width: u8,
    /// Reverse video: Claude draws its caret this way.
    pub caret: bool,
    /// Dim or grey: a placeholder or a suggestion, not typed text.
    pub ghost: bool,
}

impl BoxCell {
    pub fn plain(c: char) -> Self {
        Self {
            c,
            width: 1,
            caret: false,
            ghost: false,
        }
    }

    fn blank(&self) -> bool {
        self.c == ' ' || self.c == '\0' || self.c == '\u{a0}'
    }

    /// Typed text: a character that is not a ghost.
    fn ink(&self) -> bool {
        !self.blank() && !self.ghost && self.width != 0
    }
}

/// One row of the live screen: its absolute row (0 is the top of the scrollback) and its cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxLine {
    pub row: usize,
    pub cells: Vec<BoxCell>,
}

impl BoxLine {
    /// A row of plain text (tests and probes).
    pub fn text(row: usize, s: &str) -> Self {
        Self {
            row,
            cells: s.chars().map(BoxCell::plain).collect(),
        }
    }
}

/// One character of the prompt and where it is drawn. A line break has the cell just after its line's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PChar {
    pub c: char,
    pub row: usize,
    pub col: usize,
    pub width: u8,
}

/// The prompt being typed, as read from the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptText {
    pub chars: Vec<PChar>,
    /// The caret, as a char index (0..=len).
    pub caret: usize,
    /// The first and last screen rows of the box's text (absolute).
    pub rows: (usize, usize),
}

/// A rule of the prompt box: 20 or more box-drawing dashes and nothing else.
fn is_rule(l: &BoxLine) -> bool {
    let mut dashes = 0;
    for c in &l.cells {
        if c.c == '\u{2500}' {
            dashes += 1;
        } else if !c.blank() {
            return false;
        }
    }
    dashes >= 20
}

/// The prompt box in the live screen's rows: Claude Code's input between its two rules, the first row starting
/// with the prompt mark (`❯`, or `!` and the like in its other modes). None when there is no such box (a question
/// is up, another program draws) or no caret can be found in it.
///
/// Claude wraps a long line itself, so a row break is either a soft wrap or a typed line break. Judged as Claude's
/// word wrap does: when the next row's first word would have fitted on this row (with a column or two of margin),
/// the break was typed. A line break typed where the next word would not have fitted reads as a wrap: one
/// character fewer than the prompt holds (approximation, said in the PR).
pub fn read_box(lines: &[BoxLine], cols: usize) -> Option<PromptText> {
    let low = lines.iter().rposition(is_rule)?;
    let top = lines[..low].iter().rposition(is_rule)?;
    if low < top + 2 {
        return None;
    }
    let first = &lines[top + 1];
    let m = first.cells.iter().position(|c| !c.blank())?;
    let mark = first.cells[m];
    if m > 3 || mark.c.is_alphanumeric() || mark.caret || mark.ghost {
        return None;
    }
    if first
        .cells
        .get(m + 1)
        .is_some_and(|c| !c.blank() && !c.caret)
    {
        return None;
    }
    let text_col = m + 2;
    let rows = &lines[top + 1..low];
    // Typed text anywhere in the box (the placeholder test below needs it).
    let caret_at = rows.iter().enumerate().find_map(|(ri, l)| {
        l.cells
            .iter()
            .enumerate()
            .skip(text_col)
            .find(|(_, c)| c.caret)
            .map(|(ci, _)| (ri, ci))
    })?;
    let ink_elsewhere = rows.iter().enumerate().any(|(ri, l)| {
        l.cells
            .iter()
            .enumerate()
            .skip(text_col)
            .any(|(ci, c)| c.ink() && (ri, ci) != caret_at)
    });

    let mut chars: Vec<PChar> = Vec::new();
    let mut caret = None;
    let width_est = cols.saturating_sub(text_col + 1);
    // The previous row's text: how many cells it used, and the column after it.
    let mut prev: Option<(usize, usize)> = None;
    for (ri, l) in rows.iter().enumerate() {
        // A continuation row is indented like the first; a row with text in the indent starts at 0.
        let start = if l.cells.iter().take(text_col).any(BoxCell::ink) && ri > 0 {
            0
        } else {
            text_col
        };
        let is_caret_row = caret_at.0 == ri;
        // The row's text ends after its last typed cell, or after the caret.
        let end = l
            .cells
            .iter()
            .enumerate()
            .skip(start)
            .filter(|(ci, c)| c.ink() || (is_caret_row && *ci == caret_at.1))
            .map(|(ci, c)| ci + usize::from(c.width.max(1)))
            .max()
            .unwrap_or(start);
        // The caret cell is a character when typed text follows it on its row, or when it covers a typed
        // character; a blank caret at a row's end, or one on a placeholder's first letter, is not.
        let caret_is_char = is_caret_row && {
            let cell = l.cells[caret_at.1];
            let ink_after = l.cells.iter().skip(caret_at.1 + 1).any(BoxCell::ink);
            let next_ghost = l.cells.get(caret_at.1 + 1).is_some_and(|c| c.ghost);
            ink_after || (!cell.blank() && (ink_elsewhere || !next_ghost))
        };
        let row_text_end = if is_caret_row && !caret_is_char && caret_at.1 + 1 >= end {
            caret_at.1
        } else {
            end
        };
        let first_word = l
            .cells
            .iter()
            .skip(start)
            .take_while(|c| c.ink())
            .map(|c| usize::from(c.width))
            .sum::<usize>();
        if let Some((used, end_col)) = prev {
            let soft =
                used > 0 && first_word > 0 && used + 1 + first_word > width_est.saturating_sub(2);
            if !soft {
                // The break sits in the cell after the line's text, on the line it ends.
                chars.push(PChar {
                    c: '\n',
                    row: rows[ri - 1].row,
                    col: end_col,
                    width: 1,
                });
            }
        }
        for (ci, cell) in l.cells.iter().enumerate().take(end).skip(start) {
            if cell.width == 0 {
                continue;
            }
            if is_caret_row && ci == caret_at.1 {
                caret = Some(chars.len());
                if !caret_is_char {
                    continue;
                }
            }
            if ci >= row_text_end {
                continue;
            }
            chars.push(PChar {
                c: if cell.blank() { ' ' } else { cell.c },
                row: l.row,
                col: ci,
                width: cell.width,
            });
        }
        prev = Some((row_text_end.saturating_sub(start), row_text_end));
    }
    Some(PromptText {
        chars,
        caret: caret?,
        rows: (rows[0].row, rows[rows.len() - 1].row),
    })
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

impl PromptText {
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn text(&self) -> String {
        self.chars.iter().map(|p| p.c).collect()
    }

    /// The text of chars `a..b`.
    pub fn slice(&self, a: usize, b: usize) -> String {
        self.chars[a.min(self.len())..b.min(self.len())]
            .iter()
            .map(|p| p.c)
            .collect()
    }

    /// The start of the word before `i` (Claude's Alt+B: a word is letters and digits).
    pub fn word_left(&self, i: usize) -> usize {
        let mut j = i.min(self.len());
        while j > 0 && !is_word(self.chars[j - 1].c) {
            j -= 1;
        }
        while j > 0 && is_word(self.chars[j - 1].c) {
            j -= 1;
        }
        j
    }

    /// The end of the word after `i` (Claude's Alt+F).
    pub fn word_right(&self, i: usize) -> usize {
        let n = self.len();
        let mut j = i.min(n);
        while j < n && !is_word(self.chars[j].c) {
            j += 1;
        }
        while j < n && is_word(self.chars[j].c) {
            j += 1;
        }
        j
    }

    /// The start of the line `i` is on.
    pub fn line_start(&self, i: usize) -> usize {
        let mut j = i.min(self.len());
        while j > 0 && self.chars[j - 1].c != '\n' {
            j -= 1;
        }
        j
    }

    /// The end of the line `i` is on.
    pub fn line_end(&self, i: usize) -> usize {
        let n = self.len();
        let mut j = i.min(n);
        while j < n && self.chars[j].c != '\n' {
            j += 1;
        }
        j
    }

    /// The cells chars `a..b` cover, as selection boundaries: (row, col) of the first, (row, col after) the last.
    pub fn cells_of(&self, a: usize, b: usize) -> Option<((usize, usize), (usize, usize))> {
        if a >= b || b > self.len() {
            return None;
        }
        let s = self.chars[a];
        let e = self.chars[b - 1];
        Some(((s.row, s.col), (e.row, e.col + usize::from(e.width.max(1)))))
    }

    /// The char range a selection between boundaries `s` and `e` (end exclusive) covers, when it lies in the box.
    pub fn range_of_cells(&self, s: (usize, usize), e: (usize, usize)) -> Option<(usize, usize)> {
        let (lo, hi) = self.rows;
        if s.0 < lo || e.0 > hi || s >= e {
            return None;
        }
        let at = |p: &PChar| (p.row, p.col);
        let a = self.chars.iter().position(|p| at(p) >= s)?;
        let b = self
            .chars
            .iter()
            .position(|p| at(p) >= e)
            .unwrap_or(self.len());
        (a < b).then_some((a, b))
    }
}

// ---- the plan -----------------------------------------------------------------------------------------------------

/// Flare's selection over the prompt: the fixed end, the moving end, where Claude's caret is now (Flare walked it
/// there; the screen may not show it yet), and the prompt's text it was made on (a different text drops it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptSel {
    pub anchor: usize,
    pub head: usize,
    pub caret: usize,
    pub text: String,
}

impl PromptSel {
    pub fn range(&self) -> Option<(usize, usize)> {
        let (a, b) = (self.anchor.min(self.head), self.anchor.max(self.head));
        (a < b).then_some((a, b))
    }
}

/// A key as the prompt sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Key(PromptKey),
    /// Typed text or a paste: replaces a selection.
    Type,
    /// Backspace or Delete alone: deletes a selection.
    Erase,
    /// Cmd+Backspace on a Mac: deletes a selection, else the line before the caret (^U).
    DeleteLineBack,
    /// Left or Right alone: a selection collapses to that end.
    Collapse(bool),
    /// A modifier going down: changes nothing.
    Ignore,
    /// Anything else: the selection is let go (the highlight stays a plain selection).
    Other,
}

/// What a key is to the prompt, on this system. `option_as_meta` is the Mac's "Use Option as Meta" setting: with
/// it on, Option+letter goes out as ESC and the letter (a Meta key for Claude's editor), not as typed text.
pub fn classify(e: &KeyEvent, os: KeyOs, option_as_meta: bool) -> Input {
    if !e.pressed {
        return Input::Ignore;
    }
    if matches!(
        e.key.as_str(),
        "Shift" | "Control" | "Alt" | "AltGraph" | "Meta" | "Super" | "CapsLock" | "Fn"
    ) {
        return Input::Ignore;
    }
    if let Some(k) = prompt_key_of(e, os) {
        return Input::Key(k);
    }
    let paste = match os {
        KeyOs::Windows => e.ctrl && !e.alt && !e.meta && is_key(e, 'v'),
        KeyOs::Mac => e.meta && !e.ctrl && !e.alt && is_key(e, 'v'),
    };
    if paste {
        return Input::Type;
    }
    let none = !e.ctrl && !e.alt && !e.shift && !e.meta;
    match e.key.as_str() {
        "Backspace" | "Delete" if none => return Input::Erase,
        "ArrowLeft" if none => return Input::Collapse(false),
        "ArrowRight" if none => return Input::Collapse(true),
        "Backspace" if os.is_mac() && e.meta && !e.ctrl && !e.alt && !e.shift => {
            return Input::DeleteLineBack;
        }
        _ => {}
    }
    // Option as Meta: Option+B, F, D and the like are Claude's word keys, not text over the selection.
    if os.is_mac() && option_as_meta && e.alt && !e.ctrl {
        return Input::Other;
    }
    // Typed text: no Ctrl (AltGr is Ctrl+Alt on Windows) and no Cmd, and no control character.
    let typed = e
        .text
        .as_deref()
        .is_some_and(|t| !t.is_empty() && !t.chars().any(char::is_control));
    if typed && !e.meta && (!e.ctrl || e.alt) {
        return Input::Type;
    }
    Input::Other
}

/// What the highlight does after the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    /// Leave Flare's selection as it is.
    Keep,
    /// Clear it.
    Clear,
    /// Show chars `a..b` of the prompt.
    Set(usize, usize),
}

/// What to do with a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Bytes for the pty, first.
    pub send: String,
    /// The prompt selection after the key.
    pub sel: Option<PromptSel>,
    pub highlight: Highlight,
    /// Text for the clipboard (a cut).
    pub copy: Option<String>,
    /// The key then goes on its usual way (typed text, a paste, a key that is not the prompt's).
    pub pass: bool,
}

impl Plan {
    fn pass(highlight: Highlight) -> Self {
        Self {
            send: String::new(),
            sel: None,
            highlight,
            copy: None,
            pass: true,
        }
    }

    fn done(send: String, highlight: Highlight) -> Self {
        Self {
            send,
            sel: None,
            highlight,
            copy: None,
            pass: false,
        }
    }
}

/// The keys that move Claude's caret from `from` to `to` (one arrow per character, a line break is one).
fn walk(from: usize, to: usize, app_cursor: bool) -> String {
    let d = i64::try_from(to).unwrap_or(0) - i64::try_from(from).unwrap_or(0);
    arrow_keys(i32::try_from(d).unwrap_or(0), app_cursor)
}

/// The keys that delete chars `a..b` with the caret at `caret`: walk to `b`, one Backspace per char.
pub fn delete_keys(caret: usize, a: usize, b: usize, app_cursor: bool) -> String {
    let mut s = walk(caret, b, app_cursor);
    s.push_str(&"\x7f".repeat(b.saturating_sub(a)));
    s
}

/// The plan for a key, given the prompt on the screen, Flare's prompt selection (already checked against this
/// text by the caller, or made from a mouse selection in the box), the cursor-key mode and the system.
pub fn plan(
    input: Input,
    t: &PromptText,
    sel: Option<&PromptSel>,
    app_cursor: bool,
    os: KeyOs,
) -> Plan {
    use PromptKey as P;
    let n = t.len();
    let caret = sel.map_or(t.caret, |s| s.caret).min(n);
    let range = sel.and_then(PromptSel::range);
    let text = t.text();
    let mk = |anchor: usize, head: usize, caret: usize| PromptSel {
        anchor,
        head,
        caret,
        text: text.clone(),
    };
    let shown = |s: &PromptSel| match s.range() {
        Some((a, b)) => Highlight::Set(a, b),
        None => Highlight::Clear,
    };
    // A move of the selection's head: Claude's caret walks with it.
    let extend = |to: &dyn Fn(usize) -> usize| {
        let (anchor, head) = sel.map_or((caret, caret), |s| (s.anchor, s.head));
        let new_head = to(head).min(n);
        let s = mk(anchor, new_head, new_head);
        Plan {
            send: walk(caret, new_head, app_cursor),
            highlight: shown(&s),
            sel: Some(s),
            copy: None,
            pass: false,
        }
    };
    let delete_range =
        |a: usize, b: usize| Plan::done(delete_keys(caret, a, b, app_cursor), Highlight::Clear);
    match input {
        Input::Ignore => Plan {
            sel: sel.cloned(),
            ..Plan::pass(Highlight::Keep)
        },
        Input::Other => Plan::pass(Highlight::Keep),
        Input::Key(P::SelectAll) => {
            if n == 0 {
                return Plan::done(String::new(), Highlight::Clear);
            }
            let s = mk(0, n, caret);
            Plan {
                send: String::new(),
                highlight: shown(&s),
                sel: Some(s),
                copy: None,
                pass: false,
            }
        }
        Input::Key(P::SelectLeft) => extend(&|h| h.saturating_sub(1)),
        Input::Key(P::SelectRight) => extend(&|h| h + 1),
        Input::Key(P::SelectWordLeft) => extend(&|h| t.word_left(h)),
        Input::Key(P::SelectWordRight) => extend(&|h| t.word_right(h)),
        Input::Key(P::SelectHome) => extend(&|h| t.line_start(h)),
        Input::Key(P::SelectEnd) => extend(&|h| t.line_end(h)),
        Input::Key(P::WordLeft) => match range {
            Some((a, _)) => Plan::done(walk(caret, a, app_cursor), Highlight::Clear),
            None => Plan::done("\x1bb".into(), Highlight::Keep),
        },
        Input::Key(P::WordRight) => match range {
            Some((_, b)) => Plan::done(walk(caret, b, app_cursor), Highlight::Clear),
            None => Plan::done("\x1bf".into(), Highlight::Keep),
        },
        Input::Key(P::Home) => Plan::done("\x01".into(), clear_if(range)),
        Input::Key(P::End) => Plan::done("\x05".into(), clear_if(range)),
        Input::Key(P::DeleteWordBack) => match range {
            Some((a, b)) => delete_range(a, b),
            // ^H is Claude's word rubout under Windows Terminal; ESC DEL is Option+Delete's.
            None => Plan::done(
                if os.is_mac() { "\x1b\x7f" } else { "\x08" }.into(),
                Highlight::Keep,
            ),
        },
        Input::Key(P::DeleteWordForward) => match range {
            Some((a, b)) => delete_range(a, b),
            None => Plan::done("\x1bd".into(), Highlight::Keep),
        },
        Input::Key(P::Undo) => Plan::done("\x1f".into(), clear_if(range)),
        Input::Key(P::Cut) => match range {
            Some((a, b)) => Plan {
                copy: Some(t.slice(a, b)),
                ..delete_range(a, b)
            },
            // Ctrl+X starts Claude's own two-key chords (Ctrl+X Ctrl+K, Ctrl+X Ctrl+E).
            None => Plan::pass(Highlight::Keep),
        },
        Input::Key(P::ClearPrompt) => {
            if n == 0 {
                Plan::done(String::new(), clear_if(range))
            } else {
                delete_range(0, n)
            }
        }
        // The message keys are the caller's (they scroll the view, the prompt plays no part).
        Input::Key(P::PrevMessage | P::NextMessage) => Plan::pass(Highlight::Keep),
        Input::Type => match range {
            Some((a, b)) => Plan {
                pass: true,
                ..delete_range(a, b)
            },
            None => Plan::pass(Highlight::Keep),
        },
        Input::Erase => match range {
            Some((a, b)) => delete_range(a, b),
            None => Plan::pass(Highlight::Keep),
        },
        // ^U is Claude's (and readline's) delete to the line's start, what the Mac's Cmd+Backspace sends.
        Input::DeleteLineBack => match range {
            Some((a, b)) => delete_range(a, b),
            None => Plan::done("\x15".into(), Highlight::Keep),
        },
        Input::Collapse(right) => match range {
            Some((a, b)) => Plan::done(
                walk(caret, if right { b } else { a }, app_cursor),
                Highlight::Clear,
            ),
            None => Plan::pass(Highlight::Keep),
        },
    }
}

fn clear_if(range: Option<(usize, usize)>) -> Highlight {
    if range.is_some() {
        Highlight::Clear
    } else {
        Highlight::Keep
    }
}

// ---- your messages ------------------------------------------------------------------------------------------------

/// The scroll that brings your previous (`up`) or next message to the top of the view, in lines (positive is older),
/// given the rows your messages start on (`❯` in the first column, Claude's mark for what you sent), the view's top
/// row and the top row the view has at the bottom. Up with no older message, or a message already at the very top,
/// does nothing (None); down past the last one goes to the bottom.
pub fn message_scroll(marks: &[usize], top: usize, bottom_top: usize, up: bool) -> Option<i32> {
    let target = if up {
        *marks.iter().rev().find(|&&r| r < top)?
    } else {
        marks
            .iter()
            .copied()
            .find(|&r| r > top)
            .unwrap_or(bottom_top)
            .min(bottom_top)
    };
    let d = i64::try_from(top).unwrap_or(0) - i64::try_from(target).unwrap_or(0);
    (d != 0).then(|| i32::try_from(d).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: &str = "\x1b[D";
    const R: &str = "\x1b[C";
    const BS: &str = "\x7f";
    const W: KeyOs = KeyOs::Windows;

    fn rule(row: usize) -> BoxLine {
        BoxLine::text(row, &"\u{2500}".repeat(40))
    }

    /// A Claude prompt box, 40 columns: rows of text after the mark (first) or the indent, with the caret drawn
    /// in reverse video on the cell `caret` = (row index, column).
    fn screen(rows: &[&str], caret: (usize, usize)) -> Vec<BoxLine> {
        let mut out = vec![BoxLine::text(0, "done"), rule(1)];
        for (i, r) in rows.iter().enumerate() {
            let s = if i == 0 {
                format!("\u{276f} {r}")
            } else {
                format!("  {r}")
            };
            let mut l = BoxLine::text(2 + i, &s);
            while l.cells.len() <= caret.1 {
                l.cells.push(BoxCell::plain(' '));
            }
            if i == caret.0 {
                l.cells[caret.1].caret = true;
            }
            out.push(l);
        }
        out.push(rule(2 + rows.len()));
        out.push(BoxLine::text(3 + rows.len(), "  auto mode on"));
        out
    }

    fn read(rows: &[&str], caret: (usize, usize)) -> PromptText {
        read_box(&screen(rows, caret), 40).expect("a box")
    }

    fn ev(key: &str) -> KeyEvent {
        let code = if key.chars().count() == 1 {
            format!("Key{}", key.to_uppercase())
        } else {
            key.to_string()
        };
        KeyEvent::new(key, &code)
    }

    #[test]
    fn the_box_reads_the_text_and_the_caret_at_its_end() {
        let t = read(&["fix the thing"], (0, 15));
        assert_eq!(t.text(), "fix the thing");
        assert_eq!(t.caret, 13);
        assert_eq!(t.rows, (2, 2));
    }

    #[test]
    fn a_caret_on_a_character_is_before_it() {
        let t = read(&["fix the thing"], (0, 6));
        assert_eq!(t.text(), "fix the thing");
        assert_eq!(t.caret, 4);
        // On the last character too.
        let t = read(&["abc"], (0, 4));
        assert_eq!((t.text().as_str(), t.caret), ("abc", 2));
    }

    #[test]
    fn a_placeholder_is_no_text() {
        let mut s = screen(&["Try \"fix lint\""], (0, 2));
        for c in s[2].cells.iter_mut().skip(3) {
            c.ghost = true;
        }
        let t = read_box(&s, 40).expect("a box");
        assert_eq!(t.text(), "");
        assert_eq!(t.caret, 0);
    }

    #[test]
    fn rows_are_a_typed_break_or_a_wrap() {
        // "hello" then "world" fits on one row: the break was typed.
        let t = read(&["hello", "world"], (1, 7));
        assert_eq!(t.text(), "hello\nworld");
        assert_eq!(t.caret, 11);
        // A full row whose next word would not fit: Claude wrapped it.
        let long = "aaaa bbbb cccc dddd eeee ffff gggg";
        let t = read(&[long, "hhhhhh"], (1, 8));
        assert_eq!(t.text(), format!("{long}hhhhhh"));
    }

    #[test]
    fn a_caret_at_the_end_of_a_middle_line_is_before_the_break() {
        let t = read(&["abc", "def"], (0, 5));
        assert_eq!(t.text(), "abc\ndef");
        assert_eq!(t.caret, 3);
    }

    #[test]
    fn no_box_no_prompt() {
        let lines = vec![BoxLine::text(0, "PS C:\\> dir")];
        assert_eq!(read_box(&lines, 40), None);
        // A question box: no mark row between rules.
        let mut s = screen(&["x"], (0, 3));
        s[2] = BoxLine::text(2, "  1. Yes");
        assert_eq!(read_box(&s, 40), None);
        // No caret drawn.
        let mut s = screen(&["x"], (0, 3));
        s[2].cells[3].caret = false;
        assert_eq!(read_box(&s, 40), None);
    }

    #[test]
    fn words_lines_and_cells() {
        let t = read(&["src/utils/foo.ts go"], (0, 21));
        assert_eq!(t.word_left(16), 14);
        assert_eq!(t.word_left(14), 10);
        assert_eq!(t.word_right(0), 3);
        assert_eq!(t.word_right(3), 9);
        let t = read(&["ab", "cd"], (1, 4));
        assert_eq!(t.line_start(4), 3);
        assert_eq!(t.line_end(0), 2);
        assert_eq!(t.cells_of(0, 2), Some(((2, 2), (2, 4))));
        assert_eq!(t.cells_of(1, 5), Some(((2, 3), (3, 4))));
        assert_eq!(t.range_of_cells((2, 3), (3, 3)), Some((1, 4)));
        assert_eq!(t.range_of_cells((0, 0), (3, 3)), None);
    }

    #[test]
    fn the_keys_on_windows() {
        let w = KeyOs::Windows;
        let cases = [
            (ev("a").ctrl(), PromptKey::SelectAll),
            (ev("ArrowLeft").shift(), PromptKey::SelectLeft),
            (ev("ArrowRight").shift(), PromptKey::SelectRight),
            (ev("ArrowLeft").ctrl().shift(), PromptKey::SelectWordLeft),
            (ev("ArrowRight").ctrl().shift(), PromptKey::SelectWordRight),
            (ev("Home").shift(), PromptKey::SelectHome),
            (ev("End").shift(), PromptKey::SelectEnd),
            (ev("ArrowLeft").ctrl(), PromptKey::WordLeft),
            (ev("ArrowRight").ctrl(), PromptKey::WordRight),
            (ev("Home"), PromptKey::Home),
            (ev("End"), PromptKey::End),
            (ev("Backspace").ctrl(), PromptKey::DeleteWordBack),
            (ev("Delete").ctrl(), PromptKey::DeleteWordForward),
            (ev("z").ctrl(), PromptKey::Undo),
            (ev("x").ctrl(), PromptKey::Cut),
            (ev("Backspace").ctrl().shift(), PromptKey::ClearPrompt),
            (ev("ArrowUp").ctrl().shift(), PromptKey::PrevMessage),
            (ev("ArrowDown").ctrl().shift(), PromptKey::NextMessage),
        ];
        for (e, k) in cases {
            assert_eq!(prompt_key_of(&e, w), Some(k), "{e:?}");
        }
        assert_eq!(prompt_key_of(&ev("a").meta(), w), None);
        assert_eq!(prompt_key_of(&ev("a").ctrl().alt(), w), None);
        assert_eq!(prompt_key_of(&ev("a").ctrl().released(), w), None);
        // Dvorak's Ctrl+A is the key labelled A wherever it sits.
        assert_eq!(
            prompt_key_of(&KeyEvent::new("a", "KeyQ").ctrl(), w),
            Some(PromptKey::SelectAll)
        );
    }

    #[test]
    fn the_keys_on_the_mac() {
        let m = KeyOs::Mac;
        let cases = [
            (ev("a").meta(), PromptKey::SelectAll),
            (ev("ArrowLeft").shift(), PromptKey::SelectLeft),
            (ev("ArrowRight").shift(), PromptKey::SelectRight),
            (ev("ArrowLeft").alt().shift(), PromptKey::SelectWordLeft),
            (ev("ArrowRight").alt().shift(), PromptKey::SelectWordRight),
            (ev("ArrowRight").meta().shift(), PromptKey::SelectEnd),
            (ev("ArrowLeft").meta().shift(), PromptKey::SelectHome),
            (ev("Home").shift(), PromptKey::SelectHome),
            (ev("End").shift(), PromptKey::SelectEnd),
            (ev("ArrowLeft").alt(), PromptKey::WordLeft),
            (ev("ArrowRight").alt(), PromptKey::WordRight),
            (ev("ArrowLeft").meta(), PromptKey::Home),
            (ev("ArrowRight").meta(), PromptKey::End),
            (ev("Home"), PromptKey::Home),
            (ev("End"), PromptKey::End),
            (ev("Backspace").alt(), PromptKey::DeleteWordBack),
            (ev("Delete").alt(), PromptKey::DeleteWordForward),
            (ev("z").meta(), PromptKey::Undo),
            (ev("x").meta(), PromptKey::Cut),
            (ev("Backspace").meta().shift(), PromptKey::ClearPrompt),
            (ev("ArrowUp").meta().shift(), PromptKey::PrevMessage),
            (ev("ArrowDown").meta().shift(), PromptKey::NextMessage),
        ];
        for (e, k) in &cases {
            assert_eq!(prompt_key_of(e, m), Some(*k), "{e:?}");
        }
        // Every kind of prompt key has a Mac key above.
        let seen: std::collections::HashSet<_> = cases.iter().map(|(_, k)| *k).collect();
        assert_eq!(seen.len(), PROMPT_KEYS.len());
        // Ctrl+A on a Mac is the line start, Claude's own key; Ctrl+Z its suspend, Ctrl+X its chords.
        assert_eq!(prompt_key_of(&ev("a").ctrl(), m), None);
        assert_eq!(prompt_key_of(&ev("z").ctrl(), m), None);
        assert_eq!(prompt_key_of(&ev("x").ctrl(), m), None);
        // The Windows keys are not the Mac's.
        assert_eq!(prompt_key_of(&ev("ArrowLeft").ctrl().shift(), m), None);
        assert_eq!(prompt_key_of(&ev("Backspace").ctrl(), m), None);
    }

    #[test]
    fn every_label_names_a_key_on_both_systems() {
        for k in PROMPT_KEYS {
            assert!(!prompt_key_label(KeyOs::Windows, k).is_empty(), "{k:?}");
            assert!(!prompt_key_label(KeyOs::Mac, k).is_empty(), "{k:?}");
            assert!(!prompt_key_label(KeyOs::Windows, k).contains('\u{2014}'));
        }
    }

    #[test]
    fn classify_types_erases_and_ignores_modifiers() {
        let w = KeyOs::Windows;
        assert_eq!(classify(&ev("q").text("q"), w, false), Input::Type);
        assert_eq!(classify(&ev("Q").shift().text("Q"), w, false), Input::Type);
        // AltGr on Windows is Ctrl+Alt with text.
        assert_eq!(
            classify(
                &KeyEvent::new("@", "Digit2").ctrl().alt().text("@"),
                w,
                false
            ),
            Input::Type
        );
        assert_eq!(classify(&ev("v").ctrl(), w, false), Input::Type);
        assert_eq!(classify(&ev("Backspace"), w, false), Input::Erase);
        assert_eq!(classify(&ev("Delete"), w, false), Input::Erase);
        assert_eq!(classify(&ev("ArrowLeft"), w, false), Input::Collapse(false));
        assert_eq!(classify(&ev("Shift").shift(), w, false), Input::Ignore);
        assert_eq!(classify(&ev("Control").ctrl(), w, false), Input::Ignore);
        assert_eq!(classify(&ev("Enter").text("\r"), w, false), Input::Other);
        assert_eq!(classify(&ev("c").ctrl(), w, false), Input::Other);
        assert_eq!(classify(&ev("Escape"), w, false), Input::Other);
        assert_eq!(classify(&ev("v").meta(), KeyOs::Mac, false), Input::Type);
    }

    #[test]
    fn cmd_backspace_on_the_mac_deletes_the_selection_or_the_line_before_the_caret() {
        let m = KeyOs::Mac;
        let cmd_bs = ev("Backspace").meta();
        assert_eq!(classify(&cmd_bs, m, false), Input::DeleteLineBack);
        assert_eq!(classify(&cmd_bs, m, true), Input::DeleteLineBack);
        // Cmd+Shift+Backspace stays the clear-prompt key; on Windows the Win key is no prompt key.
        assert_eq!(
            classify(&ev("Backspace").meta().shift(), m, false),
            Input::Key(PromptKey::ClearPrompt)
        );
        assert_eq!(classify(&cmd_bs, W, false), Input::Other);
        // "fix the login bug" with "the" selected: only "the" goes, from Flare's walked caret.
        let t = read(&["fix the login bug"], (0, 19));
        let s = PromptSel {
            anchor: 4,
            head: 7,
            caret: 7,
            text: t.text(),
        };
        let p = plan(Input::DeleteLineBack, &t, Some(&s), false, m);
        assert_eq!(p.send, BS.repeat(3));
        assert_eq!(p.highlight, Highlight::Clear);
        assert!(!p.pass);
        // Without a selection it is Claude's delete to the line's start.
        let p = plan(Input::DeleteLineBack, &t, None, false, m);
        assert_eq!(p.send, "\x15");
        assert!(!p.pass);
        assert_eq!(MAC_DELETE_LINE_LABEL, "\u{2318}\u{232b}");
    }

    #[test]
    fn option_letters_type_unless_option_is_meta() {
        let m = KeyOs::Mac;
        let opt_d = KeyEvent::new("\u{2202}", "KeyD").alt().text("\u{2202}");
        let opt_2 = KeyEvent::new("@", "Digit2").alt().text("@");
        // Off: Option types the layout's character (Norwegian Option+2 is @), so it replaces a selection.
        assert_eq!(classify(&opt_d, m, false), Input::Type);
        assert_eq!(classify(&opt_2, m, false), Input::Type);
        // On: Option+letter is ESC and the letter, Claude's word keys; the selection is let go, not erased.
        assert_eq!(classify(&opt_d, m, true), Input::Other);
        assert_eq!(classify(&opt_2, m, true), Input::Other);
        let t = read(&["fix the login bug"], (0, 19));
        let s = PromptSel {
            anchor: 4,
            head: 7,
            caret: 7,
            text: t.text(),
        };
        let p = plan(classify(&opt_d, m, true), &t, Some(&s), false, m);
        assert_eq!(p.send, "");
        assert!(p.pass);
        // Option's prompt keys are still the prompt's with the setting on.
        assert_eq!(
            classify(&ev("ArrowLeft").alt(), m, true),
            Input::Key(PromptKey::WordLeft)
        );
        assert_eq!(
            classify(&ev("Backspace").alt(), m, true),
            Input::Key(PromptKey::DeleteWordBack)
        );
        // Windows ignores the setting: AltGr text still types.
        assert_eq!(
            classify(
                &KeyEvent::new("@", "Digit2").ctrl().alt().text("@"),
                W,
                true
            ),
            Input::Type
        );
    }

    #[test]
    fn select_all_shows_the_whole_prompt_and_sends_nothing() {
        let t = read(&["fix it"], (0, 5));
        let p = plan(Input::Key(PromptKey::SelectAll), &t, None, false, W);
        assert_eq!(p.send, "");
        assert_eq!(p.highlight, Highlight::Set(0, 6));
        assert!(!p.pass);
        let s = p.sel.expect("a selection");
        assert_eq!((s.anchor, s.head, s.caret), (0, 6, 3));
        // Typing over it walks to the end and deletes six, then the key goes on.
        let p = plan(Input::Type, &t, Some(&s), false, W);
        assert_eq!(p.send, format!("{}{}", R.repeat(3), BS.repeat(6)));
        assert!(p.pass);
        assert_eq!(p.highlight, Highlight::Clear);
        assert_eq!(p.sel, None);
    }

    #[test]
    fn shift_arrows_walk_the_caret_and_grow_the_selection() {
        let t = read(&["hello world"], (0, 13));
        let p = plan(Input::Key(PromptKey::SelectLeft), &t, None, false, W);
        assert_eq!(p.send, L);
        assert_eq!(p.highlight, Highlight::Set(10, 11));
        let s = p.sel.expect("sel");
        // The screen has not caught up: the next press reads Flare's own caret.
        let p = plan(
            Input::Key(PromptKey::SelectWordLeft),
            &t,
            Some(&s),
            false,
            W,
        );
        assert_eq!(p.send, L.repeat(4));
        assert_eq!(p.highlight, Highlight::Set(6, 11));
        let s = p.sel.expect("sel");
        // Back over the anchor: the selection is empty, nothing is shown.
        let p = plan(Input::Key(PromptKey::SelectEnd), &t, Some(&s), false, W);
        assert_eq!(p.send, R.repeat(5));
        assert_eq!(p.highlight, Highlight::Clear);
        let s = p.sel.expect("sel");
        let p = plan(Input::Key(PromptKey::SelectHome), &t, Some(&s), true, W);
        assert_eq!(p.send, "\x1bOD".repeat(11));
        assert_eq!(p.highlight, Highlight::Set(0, 11));
    }

    #[test]
    fn a_selection_is_erased_cut_or_collapsed() {
        let t = read(&["hello world"], (0, 13));
        let s = PromptSel {
            anchor: 0,
            head: 5,
            caret: 5,
            text: t.text(),
        };
        let p = plan(Input::Erase, &t, Some(&s), false, W);
        assert_eq!(p.send, BS.repeat(5));
        assert!(!p.pass);
        let p = plan(Input::Key(PromptKey::Cut), &t, Some(&s), false, W);
        assert_eq!(p.copy.as_deref(), Some("hello"));
        assert_eq!(p.send, BS.repeat(5));
        let p = plan(Input::Collapse(false), &t, Some(&s), false, W);
        assert_eq!(p.send, L.repeat(5));
        assert_eq!(p.highlight, Highlight::Clear);
        let p = plan(
            Input::Key(PromptKey::DeleteWordForward),
            &t,
            Some(&s),
            false,
            W,
        );
        assert_eq!(p.send, BS.repeat(5));
    }

    #[test]
    fn without_a_selection_the_keys_are_claudes_own() {
        let t = read(&["hello world"], (0, 13));
        let send = |k| plan(Input::Key(k), &t, None, false, W).send;
        assert_eq!(send(PromptKey::WordLeft), "\x1bb");
        assert_eq!(send(PromptKey::WordRight), "\x1bf");
        assert_eq!(send(PromptKey::Home), "\x01");
        assert_eq!(send(PromptKey::End), "\x05");
        assert_eq!(send(PromptKey::DeleteWordForward), "\x1bd");
        assert_eq!(send(PromptKey::Undo), "\x1f");
        assert_eq!(send(PromptKey::DeleteWordBack), "\x08");
        let mac = plan(
            Input::Key(PromptKey::DeleteWordBack),
            &t,
            None,
            false,
            KeyOs::Mac,
        );
        assert_eq!(mac.send, "\x1b\x7f");
        // Cut and the erase keys without a selection go on as they are.
        assert!(plan(Input::Key(PromptKey::Cut), &t, None, false, W).pass);
        assert!(plan(Input::Erase, &t, None, false, W).pass);
        assert!(plan(Input::Type, &t, None, false, W).pass);
        assert!(plan(Input::Collapse(true), &t, None, false, W).pass);
    }

    #[test]
    fn clear_prompt_deletes_every_line_from_anywhere() {
        let t = read(&["abc", "de"], (0, 3));
        assert_eq!(t.caret, 1);
        let p = plan(Input::Key(PromptKey::ClearPrompt), &t, None, false, W);
        assert_eq!(p.send, format!("{}{}", R.repeat(5), BS.repeat(6)));
        let empty = read(&[""], (0, 2));
        assert_eq!(
            plan(Input::Key(PromptKey::ClearPrompt), &empty, None, false, W).send,
            ""
        );
    }

    #[test]
    fn a_modifier_keeps_the_selection_and_another_key_lets_it_go() {
        let t = read(&["abc"], (0, 5));
        let s = PromptSel {
            anchor: 0,
            head: 2,
            caret: 2,
            text: t.text(),
        };
        let p = plan(Input::Ignore, &t, Some(&s), false, W);
        assert_eq!(p.sel.as_ref(), Some(&s));
        assert!(p.pass);
        let p = plan(Input::Other, &t, Some(&s), false, W);
        assert_eq!(p.sel, None);
        assert_eq!(p.highlight, Highlight::Keep);
    }

    #[test]
    fn the_message_keys_scroll_to_your_messages() {
        let marks = [10, 40, 90];
        // At the bottom (top row 100): up goes to 90, then 40, then 10, then nothing.
        assert_eq!(message_scroll(&marks, 100, 100, true), Some(10));
        assert_eq!(message_scroll(&marks, 90, 100, true), Some(50));
        assert_eq!(message_scroll(&marks, 10, 100, true), None);
        // Down: the next one, then the bottom.
        assert_eq!(message_scroll(&marks, 10, 100, false), Some(-30));
        assert_eq!(message_scroll(&marks, 90, 100, false), Some(-10));
        assert_eq!(message_scroll(&marks, 100, 100, false), None);
    }
}
