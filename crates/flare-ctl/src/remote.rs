//! The words of a remote link both ends share (remote sessions PR 2): the typed intents `remote.input` carries, and the
//! limits of the `remote.*` methods. The host checks every one of them again (rule 279); the viewer uses the same
//! functions to build what it sends, so the two can never disagree about what is allowed.
//!
//! Typing crosses the link as intents, never as raw bytes (rule 2, and rule 279 for the one exception): a key is the
//! key the person pressed (its name, its physical code, its text and modifiers), encoded into bytes on the host by the
//! same encoder a local key goes through; a text is what an input method or dictation committed, with no control
//! character; a paste is sanitised on the host the way a local paste is (rule 1).

use serde_json::{Map, Value, json};

use crate::proto::{CtlError, code};

/// The most bytes of text one `remote.input` carries, all its intents together.
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
/// The most intents one `remote.input` carries (a burst of keys typed between two polls).
pub const MAX_INTENTS: usize = 1024;
/// The longest `key` or `code` name of a key intent (W3C names are short: `ArrowLeft`, `NumpadEnter`).
pub const MAX_KEY_NAME: usize = 32;
/// The longest `text` of a key intent (one character, or a dead key's pair).
pub const MAX_KEY_TEXT: usize = 16;
/// The sizes `remote.resize` takes; outside them a size is clamped, never refused.
pub const MIN_COLS: u16 = 2;
pub const MAX_COLS: u16 = 1000;
pub const MIN_ROWS: u16 = 1;
pub const MAX_ROWS: u16 = 500;
/// The bytes one `remote.output` answers when the viewer names no `max`, and the most it may ask for. Base64 makes it a
/// third bigger, well under the 16 MB line.
pub const OUTPUT_DEFAULT: usize = 256 * 1024;
pub const OUTPUT_MAX: usize = 1024 * 1024;
/// The most entries `remote.ls` answers; past it the answer says `truncated`.
pub const LS_MAX: usize = 500;
/// The most lines of history and screen `remote.snapshot` sends.
pub const SNAPSHOT_LINES: usize = 1000;
/// The most entries one `remote.convo` answers, and the most the host keeps numbered for a tab (the last ones).
pub const CONVO_MAX: usize = 200;
/// The longest question text `remote.status` carries, in characters.
pub const QUESTION_MAX: usize = 500;
/// One chunk of `remote.fetch` and `remote.put` (remote sessions PR 6): base64 makes it a third bigger, far under the
/// 16 MB line.
pub const FILE_CHUNK: usize = 256 * 1024;
/// The biggest file `remote.fetch` reads and `remote.put` writes.
pub const FILE_MAX: u64 = 32 * 1024 * 1024;
/// The most chunks one file is cut into.
pub const FILE_CHUNKS_MAX: u32 = (FILE_MAX / FILE_CHUNK as u64) as u32;
/// The longest file name `remote.put` keeps, in bytes.
pub const PUT_NAME_MAX: usize = 100;
/// The most bytes of one styled `remote.snapshot {styled: true}` (rule 1213), before base64: a third bigger, well
/// under the 16 MB line. Past it the oldest history lines are left out; the screen is always sent.
pub const SNAPSHOT_STYLED_MAX: usize = 8 * 1024 * 1024;
/// A dropped folder copied to the host (rule 1213): at most this many bytes in all its files, and this many files. Past
/// either nothing is copied.
pub const FOLDER_MAX: u64 = 200 * 1024 * 1024;
pub const FOLDER_FILES_MAX: usize = 5000;
/// The most folders one dropped folder holds, itself included, and how deep they go below it.
pub const FOLDER_DIRS_MAX: usize = 5000;
pub const FOLDER_DEPTH_MAX: usize = 64;
/// The longest `under` of a `remote.put` (the folder below the tab's upload folder a file or folder goes in), in bytes.
pub const UNDER_MAX: usize = 4096;

/// Pure: the parts of a `remote.put`'s `under` (rule 1213), when every one of them is a name [`put_inner_name`] keeps as
/// it is (so no `.` or `..`, no separator, no device, nothing a shell acts on), there is at least one and at most
/// [`FOLDER_DEPTH_MAX`] + 1, and the whole is at most [`UNDER_MAX`] bytes. Parts are joined by `/`.
pub fn under_parts(under: &str) -> Option<Vec<&str>> {
    if under.is_empty() || under.len() > UNDER_MAX {
        return None;
    }
    let parts: Vec<&str> = under.split('/').collect();
    if parts.len() > FOLDER_DEPTH_MAX + 1
        || parts
            .iter()
            .any(|p| p.is_empty() || put_inner_name(p) != *p)
    {
        return None;
    }
    Some(parts)
}

/// The longest `remote.open` id (rule 1210), in bytes.
pub const OPEN_ID_MAX: usize = 64;

/// Pure: whether `id` is a `remote.open` id a host takes (rule 1210): 1 to [`OPEN_ID_MAX`] ASCII letters, digits, `-`
/// or `_`.
pub fn valid_open_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= OPEN_ID_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Pure: the number of chunks a file of `size` bytes is sent in (an empty file is one empty chunk).
pub fn chunks_of(size: u64) -> u32 {
    (size.div_ceil(FILE_CHUNK as u64)).max(1) as u32
}

/// Windows' device names: a file named like one (any extension, any case) is the device, on a PC host.
const DEVICES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether a file name is one of Windows' device names (`NUL`, `con.txt`, `COM1.log`): never a file to read or write.
pub fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    DEVICES.iter().any(|d| d.eq_ignore_ascii_case(stem))
}

/// Pure: the name a `remote.put` file is kept under on the host (rule 304): only its last part, with every character
/// that is not a letter, a digit, a space or one of `-_.()+,` made `_`, no dot or space at either end, no device name,
/// at most [`PUT_NAME_MAX`] bytes with its extension kept. So the name is a plain word in every shell's quotes (cmd's
/// `%` and `!`, sh's `$` and backquote never reach it) and names no other folder. An empty result is `file`. The name is
/// made NFC first (rule 1215), so a decomposed name (`e` and a combining accent, as some macOS apps write it) keeps its
/// letter instead of an `_`; a combining mark with no letter to join is still `_`.
pub fn put_name(raw: &str) -> String {
    use unicode_normalization::UnicodeNormalization as _;
    let last = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last
        .nfc()
        .map(|c| {
            if c.is_alphanumeric() && !c.is_control() || " -_.()+,".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut name = clean.trim_matches(['.', ' ']).to_string();
    if name.len() > PUT_NAME_MAX {
        let ext = name
            .rfind('.')
            .map(|i| name[i..].to_string())
            .filter(|e| e.len() <= 16)
            .unwrap_or_default();
        let mut stem = name[..name.len() - ext.len()].to_string();
        while stem.len() + ext.len() > PUT_NAME_MAX {
            stem.pop();
        }
        name = format!("{}{ext}", stem.trim_end_matches(['.', ' ']));
    }
    if name.is_empty() || name.trim_matches(['.', ' ']).is_empty() {
        return "file".to_string();
    }
    if is_device_name(&name) {
        name.insert(0, '_');
    }
    name
}

/// Pure: the name a file or folder INSIDE a dropped folder is kept under on the host (rule 1213): [`put_name`], except
/// that one leading dot is kept (`.gitignore`, `.env`), since a project's dot files must keep their names. `.` and `..`
/// are never kept as they are.
pub fn put_inner_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    match last.strip_prefix('.') {
        Some(rest) if !rest.is_empty() && !rest.starts_with('.') => {
            let n = put_name(rest);
            if n.len() < PUT_NAME_MAX {
                format!(".{n}")
            } else {
                n
            }
        }
        _ => put_name(last),
    }
}

/// What a host tab's agent is doing, as `remote.status` says it (rule 294): one word, from the host's own marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabState {
    Working,
    Question,
    Done,
    Failed,
    Idle,
}

impl TabState {
    /// Every state, in the order of weight [`TabState::of`] reads them.
    pub const ALL: [TabState; 5] = [
        TabState::Question,
        TabState::Failed,
        TabState::Working,
        TabState::Done,
        TabState::Idle,
    ];

    /// The word on the wire.
    pub fn word(self) -> &'static str {
        match self {
            TabState::Working => "working",
            TabState::Question => "question",
            TabState::Done => "done",
            TabState::Failed => "failed",
            TabState::Idle => "idle",
        }
    }

    /// The state a word names. An unknown word is none (a newer host's word reads as no news, rule 40).
    pub fn parse(word: &str) -> Option<TabState> {
        TabState::ALL.into_iter().find(|s| s.word() == word)
    }

    /// The one word for a tab's marks: a question weighs most, then a failure, then work, then a finished turn.
    pub fn of(question: bool, failed: bool, working: bool, done: bool) -> TabState {
        if question {
            TabState::Question
        } else if failed {
            TabState::Failed
        } else if working {
            TabState::Working
        } else if done {
            TabState::Done
        } else {
            TabState::Idle
        }
    }
}

/// One key press as the viewer's window reported it (the fields of `flare_term::keys::KeyEvent`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyIntent {
    pub key: String,
    pub code: String,
    pub text: Option<String>,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// One thing a remote viewer typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// A key, encoded on the host (`encode_key`) with the program's own modes.
    Key(KeyIntent),
    /// Text an input method or dictation committed: no control character at all.
    Text(String),
    /// A paste: sanitised on the host (`sanitize_paste`) and bracketed when the program asked.
    Paste(String),
}

fn bad(msg: impl Into<String>) -> CtlError {
    CtlError::new(code::BAD_PARAMS, msg)
}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

fn key_name(o: &Map<String, Value>, field: &str, need: bool) -> Result<String, CtlError> {
    match o.get(field) {
        None | Some(Value::Null) if !need => Ok(String::new()),
        Some(Value::String(s)) if s.len() <= MAX_KEY_NAME && !has_control(s) => Ok(s.clone()),
        Some(Value::String(_)) => Err(bad(format!(
            "a key's {field} is at most {MAX_KEY_NAME} bytes with no control character"
        ))),
        _ => Err(bad(format!("a key needs its {field}, a string"))),
    }
}

fn key_flag(o: &Map<String, Value>, field: &str) -> Result<bool, CtlError> {
    match o.get(field) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(bad(format!("a key's {field} is true or false"))),
    }
}

fn parse_key(v: &Value) -> Result<KeyIntent, CtlError> {
    let Some(o) = v.as_object() else {
        return Err(bad("a key is an object"));
    };
    for k in o.keys() {
        if !matches!(
            k.as_str(),
            "key" | "code" | "text" | "ctrl" | "shift" | "alt" | "meta"
        ) {
            return Err(bad(format!("a key has no field {k}")));
        }
    }
    let key = key_name(o, "key", true)?;
    if key.is_empty() {
        return Err(bad("a key needs its name"));
    }
    let text = match o.get("text") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.len() <= MAX_KEY_TEXT && !has_control(s) => Some(s.clone()),
        Some(Value::String(_)) => {
            return Err(bad(format!(
                "a key's text is at most {MAX_KEY_TEXT} bytes with no control character"
            )));
        }
        _ => return Err(bad("a key's text is a string")),
    };
    Ok(KeyIntent {
        key,
        code: key_name(o, "code", false)?,
        text,
        ctrl: key_flag(o, "ctrl")?,
        shift: key_flag(o, "shift")?,
        alt: key_flag(o, "alt")?,
        meta: key_flag(o, "meta")?,
    })
}

/// The intents of a `remote.input` call's `intents` param, every one checked: a list of 1 to [`MAX_INTENTS`] objects,
/// each with exactly one of `key` (an object), `text` or `paste` (strings); text with no control character; at most
/// [`MAX_INPUT_BYTES`] of text in all. Anything else is `bad_params`, and nothing of the call is typed.
pub fn parse_intents(v: Option<&Value>) -> Result<Vec<Intent>, CtlError> {
    let Some(list) = v.and_then(Value::as_array) else {
        return Err(bad("intents is a list"));
    };
    if list.is_empty() {
        return Err(bad("intents is empty: there is nothing to type"));
    }
    if list.len() > MAX_INTENTS {
        return Err(bad(format!(
            "at most {MAX_INTENTS} intents a call; send the rest in the next"
        )));
    }
    let mut bytes = 0usize;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        let Some(o) = item.as_object().filter(|o| o.len() == 1) else {
            return Err(bad("an intent is an object with one of key, text or paste"));
        };
        let (kind, value) = o.iter().next().expect("one field");
        let intent = match (kind.as_str(), value) {
            ("key", v) => Intent::Key(parse_key(v)?),
            ("text", Value::String(s)) => {
                if s.is_empty() {
                    return Err(bad("a text intent is empty"));
                }
                if has_control(s) {
                    return Err(bad(
                        "a text intent holds a control character; send the key instead",
                    ));
                }
                Intent::Text(s.clone())
            }
            ("paste", Value::String(s)) => {
                if s.is_empty() {
                    return Err(bad("a paste intent is empty"));
                }
                Intent::Paste(s.clone())
            }
            ("text" | "paste", _) => return Err(bad(format!("a {kind} intent is a string"))),
            _ => return Err(bad(format!("no intent is called {kind}"))),
        };
        bytes += match &intent {
            Intent::Key(k) => k.key.len() + k.code.len() + k.text.as_ref().map_or(0, String::len),
            Intent::Text(s) | Intent::Paste(s) => s.len(),
        };
        if bytes > MAX_INPUT_BYTES {
            return Err(bad(format!(
                "one remote.input carries at most {} KB",
                MAX_INPUT_BYTES / 1024
            )));
        }
        out.push(intent);
    }
    Ok(out)
}

/// The wire form of `intents` (what the viewer sends; [`parse_intents`] reads it back).
pub fn intents_json(intents: &[Intent]) -> Value {
    Value::Array(
        intents
            .iter()
            .map(|i| match i {
                Intent::Key(k) => {
                    let mut o = json!({"key": k.key, "code": k.code});
                    if let Some(t) = &k.text {
                        o["text"] = json!(t);
                    }
                    for (name, on) in [
                        ("ctrl", k.ctrl),
                        ("shift", k.shift),
                        ("alt", k.alt),
                        ("meta", k.meta),
                    ] {
                        if on {
                            o[name] = json!(true);
                        }
                    }
                    json!({"key": o})
                }
                Intent::Text(s) => json!({"text": s}),
                Intent::Paste(s) => json!({"paste": s}),
            })
            .collect(),
    )
}

/// A viewer's size, held to what a terminal can be: never refused, clamped.
pub fn clamp_size(cols: u64, rows: u64) -> (u16, u16) {
    let c = cols.clamp(u64::from(MIN_COLS), u64::from(MAX_COLS)) as u16;
    let r = rows.clamp(u64::from(MIN_ROWS), u64::from(MAX_ROWS)) as u16;
    (c, r)
}

/// How many bytes a `remote.output` answers: the viewer's `max` (when it gave one), at most [`OUTPUT_MAX`], at least 1.
pub fn output_max(asked: Option<u64>) -> usize {
    asked.map_or(OUTPUT_DEFAULT, |m| {
        usize::try_from(m)
            .unwrap_or(OUTPUT_MAX)
            .clamp(1, OUTPUT_MAX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(v: Value) -> Result<Vec<Intent>, CtlError> {
        parse_intents(Some(&v))
    }

    /// Rule 294: one word a tab, read from the host's marks in their order of weight; a word round-trips, and a word
    /// this end does not know is no state.
    #[test]
    fn a_tabs_status_word_follows_the_weight_of_its_marks() {
        assert_eq!(TabState::of(false, false, false, false), TabState::Idle);
        assert_eq!(TabState::of(false, false, false, true), TabState::Done);
        assert_eq!(TabState::of(false, false, true, true), TabState::Working);
        assert_eq!(TabState::of(false, true, true, true), TabState::Failed);
        assert_eq!(TabState::of(true, true, true, true), TabState::Question);
        for s in TabState::ALL {
            assert_eq!(TabState::parse(s.word()), Some(s));
        }
        assert_eq!(TabState::parse("blocked"), None);
        assert_eq!(TabState::parse(""), None);
    }

    #[test]
    fn remote_input_intents_round_trip_as_keys_text_and_pastes() {
        let intents = vec![
            Intent::Text("echo hi".into()),
            Intent::Key(KeyIntent {
                key: "Enter".into(),
                code: "Enter".into(),
                ..KeyIntent::default()
            }),
            Intent::Key(KeyIntent {
                key: "c".into(),
                code: "KeyC".into(),
                text: Some("c".into()),
                ctrl: true,
                ..KeyIntent::default()
            }),
            Intent::Paste("one\r\ntwo\t\x1b[201~".into()),
        ];
        assert_eq!(parsed(intents_json(&intents)).unwrap(), intents);
    }

    #[test]
    fn remote_input_refuses_an_empty_list_and_anything_that_is_no_intent() {
        for v in [
            json!(null),
            json!("echo"),
            json!([]),
            json!([{}]),
            json!([{"text": "a", "paste": "b"}]),
            json!([{"bytes": "a"}]),
            json!([{"text": 3}]),
            json!([{"text": ""}]),
            json!([{"paste": ""}]),
            json!([{"key": "Enter"}]),
            json!([{"key": {}}]),
            json!([{"key": {"key": ""}}]),
            json!([{"key": {"key": "a", "raw": "x"}}]),
            json!([{"key": {"key": "a", "ctrl": "yes"}}]),
        ] {
            let e = parsed(v.clone()).unwrap_err();
            assert_eq!(e.code, code::BAD_PARAMS, "{v}");
        }
        assert!(parse_intents(None).is_err());
    }

    /// Rule 279: a text intent carries no control character (an Enter or a Ctrl+C is a key), and neither does a key's
    /// name or text, so no raw escape sequence can ride on either.
    #[test]
    fn remote_input_refuses_control_characters_in_text_and_keys() {
        for v in [
            json!([{"text": "ls\r"}]),
            json!([{"text": "a\u{1b}[31m"}]),
            json!([{"text": "\u{9b}2J"}]),
            json!([{"text": "tab\there"}]),
            json!([{"key": {"key": "a", "text": "\u{1b}[200~"}}]),
            json!([{"key": {"key": "\r"}}]),
            json!([{"key": {"key": "a", "code": "Key\u{7}"}}]),
        ] {
            assert_eq!(parsed(v.clone()).unwrap_err().code, code::BAD_PARAMS, "{v}");
        }
        // A paste may hold line breaks and escapes: the host sanitises it as a local paste.
        assert!(parsed(json!([{"paste": "a\r\nb\u{1b}[201~"}])).is_ok());
    }

    #[test]
    fn remote_input_is_capped_at_64_kb_and_1024_intents_a_call() {
        let big = "x".repeat(MAX_INPUT_BYTES);
        assert!(parsed(json!([{"paste": big}])).is_ok());
        let over = "x".repeat(MAX_INPUT_BYTES + 1);
        assert_eq!(
            parsed(json!([{"paste": over}])).unwrap_err().code,
            code::BAD_PARAMS
        );
        // The cap is all the intents together.
        let half = "y".repeat(MAX_INPUT_BYTES / 2 + 1);
        assert!(parsed(json!([{"text": half}, {"paste": half}])).is_err());
        let many: Vec<Value> = (0..=MAX_INTENTS).map(|_| json!({"text": "a"})).collect();
        assert!(parsed(Value::Array(many)).is_err());
        let most: Vec<Value> = (0..MAX_INTENTS).map(|_| json!({"text": "a"})).collect();
        assert_eq!(parsed(Value::Array(most)).unwrap().len(), MAX_INTENTS);
    }

    #[test]
    fn a_remote_size_is_clamped_and_never_refused() {
        assert_eq!(clamp_size(0, 0), (MIN_COLS, MIN_ROWS));
        assert_eq!(clamp_size(120, 40), (120, 40));
        assert_eq!(clamp_size(u64::MAX, u64::MAX), (MAX_COLS, MAX_ROWS));
    }

    /// Rule 304: a put file's name is one plain word on the host: its last part, no character a shell acts on, no device
    /// name, no dot or space at its ends, and short with its extension kept.
    #[test]
    fn a_put_name_is_its_last_part_with_nothing_a_shell_acts_on() {
        assert_eq!(put_name("shot.png"), "shot.png");
        assert_eq!(put_name("C:\\Users\\max\\Pictures\\a b.png"), "a b.png");
        assert_eq!(put_name("/tmp/../../etc/passwd"), "passwd");
        assert_eq!(put_name(".."), "file");
        assert_eq!(put_name(""), "file");
        assert_eq!(put_name("..\\"), "file");
        assert_eq!(put_name("100%!.txt"), "100__.txt");
        assert_eq!(put_name("a\"b'c$d`e;f&g|h.txt"), "a_b_c_d_e_f_g_h.txt");
        assert_eq!(put_name("line\nbreak\u{1b}.txt"), "line_break_.txt");
        assert_eq!(put_name(" .hidden. "), "hidden");
        assert_eq!(put_name("NUL"), "_NUL");
        assert_eq!(put_name("con.txt"), "_con.txt");
        assert_eq!(put_name("com1.log"), "_com1.log");
        assert_eq!(put_name("console.txt"), "console.txt");
        assert_eq!(put_name("été (2).jpg"), "été (2).jpg");
        let long = format!("{}.png", "x".repeat(300));
        let kept = put_name(&long);
        assert!(
            kept.len() <= PUT_NAME_MAX && kept.ends_with(".png"),
            "{kept}"
        );
        for raw in ["a%b", "a!b", "a\"b", "~/x", "$(rm)", "`x`"] {
            let n = put_name(raw);
            assert!(
                !n.contains(['%', '!', '"', '$', '`', '/', '\\', '~']),
                "{raw} -> {n}"
            );
        }
    }

    /// Rule 1215: a decomposed name (a letter and a combining accent, as some macOS apps write it) is made NFC and keeps
    /// its letter, as a file or as a dropped folder's inner name; every refusal still holds, and a lone combining mark
    /// with no letter before it is still `_`.
    #[test]
    fn a_decomposed_put_name_keeps_its_accented_letters() {
        assert_eq!(put_name("cafe\u{301}.txt"), "caf\u{e9}.txt");
        assert_eq!(
            put_name("e\u{301}te\u{301} (2).jpg"),
            "\u{e9}t\u{e9} (2).jpg"
        );
        assert_eq!(put_inner_name(".cafe\u{301}"), ".caf\u{e9}");
        assert_eq!(
            under_parts("proj/caf\u{e9}"),
            Some(vec!["proj", "caf\u{e9}"])
        );
        assert_eq!(under_parts("proj/cafe\u{301}"), None, "the host wants NFC");
        assert_eq!(put_name("\u{301}x"), "_x");
        assert_eq!(put_name("/tmp/cafe\u{301}/../x$y.txt"), "x_y.txt");
        assert_eq!(put_name("a\u{301}/NUL"), "_NUL");
        assert_eq!(put_name(" .e\u{301}. "), "\u{e9}");
    }

    #[test]
    fn a_file_is_cut_into_chunks_and_an_empty_file_is_one() {
        assert_eq!(chunks_of(0), 1);
        assert_eq!(chunks_of(1), 1);
        assert_eq!(chunks_of(FILE_CHUNK as u64), 1);
        assert_eq!(chunks_of(FILE_CHUNK as u64 + 1), 2);
        assert_eq!(chunks_of(FILE_MAX), FILE_CHUNKS_MAX);
        assert!(is_device_name("aux.c") && !is_device_name("auxiliary"));
    }

    /// Rule 1213: a folder's place below the tab's upload folder is clean names joined by `/`, never `..`, an absolute
    /// path, a backslash or a name the host would clean.
    #[test]
    fn an_under_is_clean_names_joined_by_slashes_and_nothing_else() {
        assert_eq!(under_parts("proj"), Some(vec!["proj"]));
        assert_eq!(
            under_parts("proj/src/a b"),
            Some(vec!["proj", "src", "a b"])
        );
        assert_eq!(under_parts("proj/.git"), Some(vec!["proj", ".git"]));
        assert_eq!(put_inner_name(".gitignore"), ".gitignore");
        assert_eq!(put_inner_name(".."), "file");
        assert_eq!(put_inner_name("..x"), "x");
        assert_eq!(put_inner_name("a%b"), "a_b");
        for bad in [
            "",
            "/proj",
            "proj/",
            "proj//src",
            "..",
            "proj/../x",
            "proj\\src",
            "C:",
            "a%b",
            "nul",
            "...",
            ".",
        ] {
            assert_eq!(under_parts(bad), None, "{bad}");
        }
        let deep = vec!["d"; FOLDER_DEPTH_MAX + 2].join("/");
        assert_eq!(under_parts(&deep), None);
    }

    #[test]
    fn an_output_poll_answers_at_most_a_megabyte() {
        assert_eq!(output_max(None), OUTPUT_DEFAULT);
        assert_eq!(output_max(Some(0)), 1);
        assert_eq!(output_max(Some(10)), 10);
        assert_eq!(output_max(Some(u64::MAX)), OUTPUT_MAX);
    }
}
