//! M9 Task 14: the Explorer verb keys, their specs and the pure rules.
//!
//! Everything here is pure: no registry call, no process, no network. The verb operations that use
//! these rules (install, remove, status, relabel) are Task 15 (`verb.rs`). Keys are relative to the
//! current user's hive (`HKCU`), never machine-wide. Names come from `names.rs`.

use crate::names::{VERB_KEY, VERB_LABEL};

/// The per-user classes root that the verb keys live under (`HKCU\Software\Classes`).
pub const DEFAULT_CLASSES_ROOT: &str = "Software\\Classes";

/// The two keys: the folder verb (`%1`) and the folder background verb (`%V`).
pub fn verb_keys(root: &str) -> [String; 2] {
    [
        format!("{root}\\Directory\\shell\\{VERB_KEY}"),
        format!("{root}\\Directory\\Background\\shell\\{VERB_KEY}"),
    ]
}

/// What a key shows and what Explorer substitutes for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbSpec {
    pub label: &'static str,
    pub arg: &'static str,
}

/// The spec of one of the two keys. The background key gets `%V`; the folder key gets `%1`.
pub fn verb_spec(root: &str, key: &str) -> VerbSpec {
    // A background click has no %1: the folder you are IN arrives as %V.
    let background = verb_keys(root)[1].as_str() == key;
    VerbSpec {
        label: VERB_LABEL,
        arg: if background { "%V" } else { "%1" },
    }
}

/// One registry value to write: `name` None is the key's default value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegWrite {
    pub key: String,
    pub name: Option<String>,
    pub value: String,
}

/// The writes that create the verb, per key and in order: the label, the icon, then the command.
pub fn add_writes(root: &str, exe: &str) -> Vec<RegWrite> {
    let mut out = Vec::with_capacity(6);
    for key in verb_keys(root) {
        let spec = verb_spec(root, &key);
        out.push(RegWrite {
            key: key.clone(),
            name: None,
            value: spec.label.to_owned(),
        });
        out.push(RegWrite {
            key: key.clone(),
            name: Some("Icon".to_owned()),
            value: format!("{exe},0"),
        });
        // The substitution is quoted inside the value: a path with spaces is one argument.
        out.push(RegWrite {
            key: command_key(&key),
            name: None,
            value: format!("\"{exe}\" \"{}\"", spec.arg),
        });
    }
    out
}

/// The keys to delete on uninstall: the same two keys the install writes.
pub fn remove_keys(root: &str) -> Vec<String> {
    verb_keys(root).to_vec()
}

/// The command subkey of a verb key.
pub fn command_key(key: &str) -> String {
    format!("{key}\\command")
}

/// Does the command value name THIS executable? Case-insensitive, as Windows paths are.
pub fn points_at(command_value: &str, exe: &str) -> bool {
    command_value.to_lowercase().contains(&exe.to_lowercase())
}

/// Should the verb be (re)written on this launch? Never over a deliberate NO.
pub fn should_write_verb(said_no: bool, installed: bool) -> bool {
    !said_no && !installed
}

/// The label out of what `reg query <key> /ve` printed, or None when there is nothing to read.
/// Matched on `REG_SZ`, never on the value's name (reg.exe translates "(Default)").
///
/// The rule is the reg.exe line `^[ \t]+.*?[ \t]+REG_SZ(?:[ \t]{1,4}(.*?))?[ \t]*\r?$` (multi-line),
/// written out by hand so this crate needs no regex dependency. The first line that matches wins.
pub fn read_label_from_reg_output(reg_output: &str) -> Option<String> {
    reg_output.split('\n').find_map(read_label_line)
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// One line (without its newline): a leading blank, then anything, a blank, `REG_SZ`, then the value.
fn read_label_line(line: &str) -> Option<String> {
    let b = line.as_bytes();
    if b.first().is_none_or(|&c| !is_ws(c)) {
        return None;
    }
    // `REG_SZ` follows a blank, and the leading blank is at least one character before that blank.
    for k in 2..b.len() {
        if !is_ws(b[k - 1]) || !line[k..].starts_with("REG_SZ") {
            continue;
        }
        if let Some(v) = label_after_type(&line[k + "REG_SZ".len()..]) {
            return Some(v);
        }
    }
    None
}

/// What follows `REG_SZ`: an optional blank run (up to four), then the value up to the line end.
fn label_after_type(rest: &str) -> Option<String> {
    let b = rest.as_bytes();
    if b.first().is_some_and(|&c| is_ws(c)) {
        let blanks = b.iter().take_while(|&&c| is_ws(c)).count().min(4);
        let value = rest[blanks..].strip_suffix('\r').unwrap_or(&rest[blanks..]);
        return Some(value.trim_end_matches([' ', '\t']).to_owned());
    }
    match rest {
        "" | "\r" => Some(String::new()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\Admin\AppData\Local\Programs\Flare\Flare.exe";
    const ROOT: &str = DEFAULT_CLASSES_ROOT;

    #[test]
    fn the_verb_keys_are_relative_to_the_current_user_hive() {
        for k in verb_keys(ROOT) {
            assert!(!k.starts_with("HKLM"), "{k}");
            assert!(!k.starts_with("HKEY_LOCAL_MACHINE"), "{k}");
            assert!(!k.starts_with('\\'), "{k}");
        }
    }

    #[test]
    fn the_registry_source_writes_only_under_the_current_user_hive() {
        // A source scan of registry.rs: the machine hive is named only inside the read-only
        // LocalMachineRead impl, and that impl has no write call.
        // LF whatever the checkout's line endings are.
        let src = &include_str!("registry.rs").replace("\r\n", "\n");
        let without_use = remove_span(src, "use windows_sys::Win32::System::Registry::{", "};");
        let without_lm = remove_span(&without_use, "impl LocalMachineRead {", "\n}\n");
        assert!(!without_lm.contains("HKEY_LOCAL_MACHINE"));
        let lm = src
            .find("impl LocalMachineRead {")
            .expect("LocalMachineRead impl");
        let lm_body = &src[lm..lm + src[lm..].find("\n}\n").expect("impl end") + 3];
        assert!(lm_body.contains("HKEY_LOCAL_MACHINE"));
        assert!(!lm_body.contains("KEY_WRITE"));
        assert!(!lm_body.contains("RegSetValueExW"));
    }

    /// `src` without the first `start`..`end` span (end searched after start, inclusive).
    fn remove_span(src: &str, start: &str, end: &str) -> String {
        let Some(s) = src.find(start) else {
            return src.to_owned();
        };
        let e = s + src[s..].find(end).expect("span end") + end.len();
        format!("{}{}", &src[..s], &src[e..])
    }

    #[test]
    fn covers_folders_and_the_folder_background_and_not_files() {
        let keys = verb_keys(ROOT);
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| !k.contains("\\*\\")));
        assert!(keys.iter().any(|k| k.contains("\\Directory\\shell\\")));
        assert!(
            keys.iter()
                .any(|k| k.contains("\\Directory\\Background\\shell\\"))
        );
    }

    #[test]
    fn uses_its_own_key_name_and_never_the_neighbours_ones() {
        let other = ["pri", "sm"].concat();
        for k in verb_keys(ROOT) {
            assert!(k.ends_with("\\Flare"), "{k}");
            let low = k.to_ascii_lowercase();
            assert!(!low.contains(&other), "{k}");
            assert!(!low.contains(&format!("openwith{other}")), "{k}");
        }
    }

    #[test]
    fn the_background_verb_gets_percent_v() {
        let keys = verb_keys(ROOT);
        assert_eq!(
            verb_spec(ROOT, &keys[1]),
            VerbSpec {
                label: "Open in Flare",
                arg: "%V"
            }
        );
        assert_eq!(
            verb_spec(ROOT, &keys[0]),
            VerbSpec {
                label: "Open in Flare",
                arg: "%1"
            }
        );
    }

    #[test]
    fn says_open_in_flare_on_both_entries() {
        assert_eq!(VERB_LABEL, "Open in Flare");
        let labels: Vec<_> = add_writes(ROOT, EXE)
            .into_iter()
            .filter(|w| w.name.is_none() && !w.key.ends_with("\\command"))
            .map(|w| w.value)
            .collect();
        assert_eq!(
            labels,
            vec!["Open in Flare".to_owned(), "Open in Flare".to_owned()]
        );
    }

    #[test]
    fn quotes_the_path_inside_the_command() {
        let cmd = add_writes(ROOT, EXE)
            .into_iter()
            .find(|w| w.key.ends_with("\\command"))
            .expect("a command write");
        assert_eq!(cmd.value, format!("\"{EXE}\" \"%1\""));
    }

    #[test]
    fn names_the_menu_item_and_gives_it_the_app_icon() {
        let writes = add_writes(ROOT, EXE);
        assert!(writes.iter().any(|w| w.value == "Open in Flare"));
        assert!(
            writes
                .iter()
                .any(|w| w.name.as_deref() == Some("Icon") && w.value == format!("{EXE},0"))
        );
    }

    #[test]
    fn a_path_with_quotes_and_an_ampersand_is_kept_verbatim() {
        let weird = r#"C:\Program Files\A "quoted" & piped\Flare.exe"#;
        let cmd = add_writes(ROOT, weird)
            .into_iter()
            .find(|w| w.key.ends_with("\\command"))
            .expect("a command write");
        assert!(cmd.value.contains(weird));
    }

    #[test]
    fn the_writes_are_label_icon_command_in_that_order_per_key() {
        let writes = add_writes(ROOT, EXE);
        assert_eq!(writes.len(), 6);
        let keys = verb_keys(ROOT);
        for (i, key) in keys.iter().enumerate() {
            let w = &writes[i * 3..i * 3 + 3];
            assert_eq!(w[0].key, *key);
            assert_eq!(w[0].name, None);
            assert_eq!(w[1].key, *key);
            assert_eq!(w[1].name.as_deref(), Some("Icon"));
            assert_eq!(w[2].key, command_key(key));
            assert_eq!(w[2].name, None);
        }
    }

    #[test]
    fn recognises_a_verb_pointing_at_this_build() {
        let output = format!(
            "\r\nHKEY_CURRENT_USER\\Software\\Classes\\Directory\\shell\\Flare\\command\r\n    (Default)    REG_SZ    \"{EXE}\" \"%1\"\r\n"
        );
        assert!(points_at(&output, EXE));
    }

    #[test]
    fn points_at_is_case_insensitive() {
        assert!(points_at(&format!("\"{EXE}\" \"%1\""), &EXE.to_uppercase()));
    }

    #[test]
    fn does_not_claim_a_verb_pointing_at_another_copy() {
        assert!(!points_at(
            &format!("\"{EXE}\" \"%1\""),
            r"D:\builds\Flare\Flare.exe"
        ));
    }

    #[test]
    fn reads_an_empty_answer_as_absent() {
        assert!(!points_at("", EXE));
    }

    #[test]
    fn reads_the_label_without_its_line_ending() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Classes\\Directory\\shell\\Flare\r\n    (Default)    REG_SZ    Open in Flare\r\n\r\n";
        assert_eq!(
            read_label_from_reg_output(out).as_deref(),
            Some("Open in Flare")
        );
    }

    #[test]
    fn does_not_depend_on_the_word_default() {
        let out = "    (Standard)    REG_SZ    Open in Flare\r\n";
        assert_eq!(
            read_label_from_reg_output(out).as_deref(),
            Some("Open in Flare")
        );
    }

    #[test]
    fn a_current_label_reads_back_as_itself() {
        let out = format!("    (Default)    REG_SZ    {VERB_LABEL}\r\n");
        assert_eq!(
            read_label_from_reg_output(&out).as_deref(),
            Some(VERB_LABEL)
        );
    }

    #[test]
    fn nothing_to_read_is_none() {
        assert_eq!(read_label_from_reg_output(""), None);
        assert_eq!(
            read_label_from_reg_output(
                "ERROR: The system was unable to find the specified registry key or value."
            ),
            None
        );
    }

    #[test]
    fn puts_an_absent_verb_back_when_nobody_said_no() {
        assert!(should_write_verb(false, false));
    }

    #[test]
    fn leaves_a_working_verb_alone() {
        assert!(!should_write_verb(false, true));
    }

    #[test]
    fn never_argues_with_a_deliberate_off() {
        assert!(!should_write_verb(true, false));
        assert!(!should_write_verb(true, true));
    }

    #[test]
    fn the_uninstall_keys_are_the_install_keys() {
        assert_eq!(remove_keys(ROOT), verb_keys(ROOT).to_vec());
    }

    #[test]
    fn command_key_appends_the_command_subkey() {
        assert_eq!(command_key(r"A\Flare"), r"A\Flare\command");
    }

    #[test]
    fn the_verb_keys_follow_the_root_they_are_given() {
        let keys = verb_keys(r"Software\FlareE2E\abc\Classes");
        assert!(keys[0].starts_with(r"Software\FlareE2E\abc\Classes\Directory\shell\Flare"));
    }
}
