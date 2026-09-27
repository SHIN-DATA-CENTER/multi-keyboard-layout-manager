//! Conflict resolution (design m2 D.8; m3 B.10): an operation in `Conflict`, shown per keyboard
//! (the records of one target: a device's Type/Subtype pair, or the PC-wide values) in layout
//! names, with a recommendation (review U6). The per-value numbers and choices are folded into
//! "詳細" for whoever needs them.
//!
//! Terms, the same everywhere: 操作の前の値 (`before`), 操作で書こうとした値 (`intended`), MKLM
//! 導入前の値 (`baseline`), 今の値 (`current`).

use mklm_core::{JournalEntry, KeyboardType, RegValue, ResolutionChoice, ValueChoice, ValueRecord};

use crate::i18n::Lang;

/// One way to resolve a keyboard, by layout name, duplicates removed ("変更前の US に戻す",
/// "MKLM が設定した JIS にする", "今の値のまま"; "導入前" only when it differs from "変更前").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictOption {
    pub label: String,
    pub choice: ResolutionChoice,
}

/// One value, for the folded details.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictValue {
    /// The record's index in the entry.
    pub record: usize,
    pub key: String,
    pub name: String,
    pub current: String,
    pub last_written: String,
    pub before: String,
    pub intended: String,
    pub baseline: String,
    pub write_error: String,
    /// A per-value choice made in the details; `None` follows the keyboard's option.
    pub choice: Option<ResolutionChoice>,
}

/// One keyboard (or the PC-wide values) of the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictKeyboard {
    /// "Keychron Receiver", or "PC 全体の設定".
    pub name: String,
    /// "今の値: 不明な種類（8/2）— MKLM 以外が変更".
    pub now: String,
    pub options: Vec<ConflictOption>,
    /// The option chosen (starts at the recommendation).
    pub selected: usize,
    /// The recommended option, marked "（おすすめ）".
    pub recommended: Option<usize>,
    pub values: Vec<ConflictValue>,
}

/// The recommendation for a device whose Type/Subtype pair is `current` now (review U6): a
/// pair MKLM would never write (outside {4/0, 7/2}) is most likely a mistake — go back to the
/// value before the operation; a valid pair (e.g. set in the Settings app) is kept.
pub fn recommend(current: Option<KeyboardType>) -> ResolutionChoice {
    match current {
        Some(KeyboardType::US | KeyboardType::JIS) => ResolutionChoice::KeepCurrent,
        _ => ResolutionChoice::UseBefore,
    }
}

/// The keyboards of `entry` (WP-U5): records grouped by target, `current` reads the value stored
/// now (the display snapshot), `name` names a device (the snapshot's display name). Options by
/// layout name with duplicates removed; `selected` = `recommended`.
pub fn conflict_keyboards(
    entry: &JournalEntry,
    current: &dyn Fn(&ValueRecord) -> Option<RegValue>,
    name: &dyn Fn(&str) -> String,
    lang: Lang,
) -> Vec<ConflictKeyboard> {
    let _ = (entry, current, name, lang);
    todo!("WP-U5: the conflict view-model")
}

/// The request's choices: every value of a keyboard gets the keyboard's option, unless the
/// details chose otherwise for that value.
pub fn choices(keyboards: &[ConflictKeyboard]) -> Vec<ValueChoice> {
    let mut choices: Vec<ValueChoice> = keyboards
        .iter()
        .flat_map(|keyboard| {
            let option = keyboard
                .options
                .get(keyboard.selected)
                .map_or(ResolutionChoice::KeepCurrent, |option| option.choice);
            keyboard.values.iter().map(move |value| ValueChoice {
                record: value.record,
                choice: value.choice.unwrap_or(option),
            })
        })
        .collect();
    choices.sort_by_key(|choice| choice.record);
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(choice: ResolutionChoice) -> ConflictOption {
        ConflictOption {
            label: format!("{choice:?}"),
            choice,
        }
    }

    #[test]
    fn recommendations_and_choices() {
        assert_eq!(
            recommend(Some(KeyboardType { ty: 8, subtype: 2 })),
            ResolutionChoice::UseBefore
        );
        assert_eq!(recommend(None), ResolutionChoice::UseBefore);
        assert_eq!(
            recommend(Some(KeyboardType::US)),
            ResolutionChoice::KeepCurrent
        );
        let keychron = ConflictKeyboard {
            name: "Keychron Receiver".into(),
            now: String::new(),
            options: vec![
                option(ResolutionChoice::UseBefore),
                option(ResolutionChoice::UseIntended),
                option(ResolutionChoice::KeepCurrent),
            ],
            selected: 0,
            recommended: Some(0),
            values: vec![
                ConflictValue {
                    record: 1,
                    ..ConflictValue::default()
                },
                ConflictValue {
                    record: 0,
                    choice: Some(ResolutionChoice::UseBaseline),
                    ..ConflictValue::default()
                },
            ],
        };
        // Both values of the pair follow the keyboard's option, unless the details said
        // otherwise; a pair cannot be split by accident (review U6).
        assert_eq!(
            choices(&[keychron]),
            vec![
                ValueChoice {
                    record: 0,
                    choice: ResolutionChoice::UseBaseline
                },
                ValueChoice {
                    record: 1,
                    choice: ResolutionChoice::UseBefore
                },
            ]
        );
    }
}
