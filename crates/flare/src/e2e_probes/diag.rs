//! The diagnostics log probes (Task 38): the buttons of a settings row, read from the Settings
//! bridge, so the scenario can see Mark's word (Mark, then Marked while its alt state holds). Read
//! only: nothing here sets a property. Nothing runs without --e2e (`e2e_glue::install` is the caller).

use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::{ProbeCtx, ProbeRegistry};
use crate::SettingsBridge;

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("diag.buttons", diag_buttons);
}

/// One button as the probe answers it: its id, both words, whether it is off, and `word`, the
/// word that shows (the second word while the button's alt state holds, else the first).
pub fn button_json(id: &str, label: &str, label_alt: &str, off: bool, alt: bool) -> Value {
    let word = if alt { label_alt } else { label };
    json!({
        "id": id,
        "label": label,
        "label_alt": label_alt,
        "off": off,
        "alt": alt,
        "word": word,
    })
}

/// `diag.buttons` `{ id }`: `{ id, buttons: [...] }` for the settings row `id`, `Null` when no
/// row drawn has that id.
fn diag_buttons(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let id = arg
        .get("id")
        .and_then(Value::as_str)
        .ok_or("diag.buttons needs an id")?;
    let b = ctx.app.global::<SettingsBridge>();
    for s in b.get_sections().iter() {
        for r in s.rows.iter() {
            if r.id.as_str() != id {
                continue;
            }
            let buttons: Vec<Value> = r
                .buttons
                .iter()
                .map(|x| {
                    button_json(
                        x.id.as_str(),
                        x.label.as_str(),
                        x.label_alt.as_str(),
                        x.off,
                        x.alt,
                    )
                })
                .collect();
            return Ok(json!({ "id": id, "buttons": buttons }));
        }
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::button_json;

    #[test]
    fn the_word_is_the_first_label_until_the_alt_state_holds() {
        assert_eq!(
            button_json("mark", "Mark", "Marked", false, false)["word"],
            "Mark"
        );
        assert_eq!(
            button_json("mark", "Mark", "Marked", false, true)["word"],
            "Marked"
        );
    }

    #[test]
    fn an_off_button_says_it_is_off() {
        let on = button_json("open", "Open folder", "", false, false);
        let off = button_json("open", "Open folder", "", true, false);
        assert_eq!(on["off"], false);
        assert_eq!(off["off"], true);
    }
}
