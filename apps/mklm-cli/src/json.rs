//! JSON documents printed by `status --json` and `global status --json`.

use std::fmt::Write as _;

use mklm_core::{
    Assessment, GlobalAnomaly, GlobalMode, GlobalSettings, InputMethods, InputWarning, LayoutTable,
    SystemSnapshot, global_anomalies, input_warnings,
};
use serde::Serialize;

/// Something that could not be read. The affected snapshot fields are `null` / empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// What was being read: an instance ID, an interface path or a registry path.
    pub subject: String,
    pub error: String,
}

/// `status --json`: everything read from the system and its evaluation.
#[derive(Debug, Serialize)]
pub struct StatusDocument<'a> {
    pub snapshot: &'a SystemSnapshot,
    pub assessment: &'a Assessment,
    /// Non-fatal read failures. Anything about to write must stop while this is non-empty.
    pub issues: &'a [Issue],
}

/// `global status --json`: the global values and the input methods, without keyboards.
#[derive(Debug, Serialize)]
pub struct GlobalStatusDocument<'a> {
    pub global: &'a GlobalSettings,
    pub mode: GlobalMode,
    pub standard_layout: LayoutTable,
    pub global_anomalies: Vec<GlobalAnomaly>,
    pub input: &'a InputMethods,
    pub input_warnings: Vec<InputWarning>,
    pub issues: &'a [Issue],
}

impl<'a> GlobalStatusDocument<'a> {
    /// Evaluates the global values and input methods the same way [`mklm_core::assess()`] does.
    pub fn new(global: &'a GlobalSettings, input: &'a InputMethods, issues: &'a [Issue]) -> Self {
        Self {
            global,
            mode: global.mode(),
            standard_layout: global.standard_layout(),
            global_anomalies: global_anomalies(global),
            input,
            input_warnings: input_warnings(input),
            issues,
        }
    }
}

/// Pretty-printed JSON followed by a newline. With `ascii_only`, every non-ASCII character is
/// written as a `\uXXXX` escape, so the output survives any code page conversion on the way.
pub fn to_json<T: Serialize>(value: &T, ascii_only: bool) -> serde_json::Result<String> {
    let mut json = serde_json::to_string_pretty(value)?;
    if ascii_only {
        json = escape_non_ascii(&json);
    }
    json.push('\n');
    Ok(json)
}

/// Escapes every non-ASCII character. Valid JSON has them only inside strings, where an escape
/// means the same character.
fn escape_non_ascii(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                write!(out, "\\u{unit:04x}").expect("writing to a String cannot fail");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn ascii_escaping_round_trips() {
        let value = json!({
            "display_name": "日本語 PS/2 キーボード (106/109 キー Ctrl+英数)",
            "copyright": "© 2026",
            "emoji": "⌨️😀",
            "path": "HID\\VID_3434&PID_D027",
        });
        let plain = to_json(&value, false).unwrap();
        assert!(plain.contains("日本語"));
        let ascii = to_json(&value, true).unwrap();
        assert!(ascii.is_ascii());
        let escaped = |units: &[&str]| units.iter().map(|u| format!("\\u{u}")).collect::<String>();
        assert!(ascii.contains(&escaped(&["65e5", "672c", "8a9e"])));
        assert!(ascii.contains(&escaped(&["d83d", "de00"])));
        assert!(ascii.ends_with("}\n"));
        assert_eq!(serde_json::from_str::<Value>(&ascii).unwrap(), value);
        assert_eq!(serde_json::from_str::<Value>(&plain).unwrap(), value);
    }

    #[test]
    fn global_document_shape() {
        let global = GlobalSettings {
            layer_driver_jpn: Some("kbd106.dll".into()),
            layer_driver_kor: Some("kbd101a.dll".into()),
            override_keyboard_identifier: Some("PCAT_106KEY".into()),
            override_keyboard_type: None,
            override_keyboard_subtype: None,
        };
        let input = InputMethods {
            user_preload: vec!["00000411".into(), "00000409".into()],
            sign_in_preload: vec!["00000411".into()],
            loaded_layouts: vec![0x0411_0411, 0x0409_0409],
        };
        let issues = [Issue {
            subject: r"HKU\.DEFAULT\Keyboard Layout\Preload".into(),
            error: "access denied".into(),
        }];
        let doc = GlobalStatusDocument::new(&global, &input, &issues);
        let value: Value = serde_json::from_str(&to_json(&doc, true).unwrap()).unwrap();
        assert_eq!(value["mode"], "per-keyboard");
        assert_eq!(value["standard_layout"], "jis");
        assert_eq!(value["global"]["layer_driver_jpn"], "kbd106.dll");
        assert_eq!(value["global"]["override_keyboard_type"], Value::Null);
        assert_eq!(value["global_anomalies"], json!([]));
        assert_eq!(
            value["input"]["user_preload"],
            json!(["00000411", "00000409"])
        );
        assert_eq!(value["input_warnings"][0]["kind"], "non-japanese-layouts");
        assert_eq!(value["input_warnings"][0]["loaded"], json!(["04090409"]));
        assert_eq!(value["issues"][0]["error"], "access denied");
    }
}
