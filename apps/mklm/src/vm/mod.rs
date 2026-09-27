//! View-models (design m3 B, H.1): pure functions from the data (snapshot, assessment, journal,
//! session view, settings, language) to what a screen shows. No Slint and no Windows here, so
//! every screen's text is unit- and snapshot-tested without a window; `app.rs` copies the result
//! into the generated Slint structs.

pub mod change;
pub mod conflict;
pub mod journal;
pub mod keyboards;
pub mod keytest;
pub mod post_reboot;
pub mod recovery;
pub mod restart;
pub mod result;
pub mod session;
pub mod status;
pub mod wizard;

/// Colour role of a badge, banner or state (matches `Tone` in ui/structs.slint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

/// A badge with its screen-reader text (design m3 E.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    pub tone: Tone,
    pub accessible: String,
}

impl Badge {
    pub fn new(kind: crate::i18n::BadgeKind, tone: Tone, lang: crate::i18n::Lang) -> Self {
        let (text, accessible) = crate::i18n::badge(kind, lang);
        Self {
            text,
            tone,
            accessible,
        }
    }
}

/// Plain-text rendering of a view-model for snapshot tests (design m3 H.3): one `label: value`
/// line per field, so a test compares what a screen would say in each language.
pub trait SnapshotText {
    fn snapshot_text(&self) -> String;
}

/// One change to a displayed list (design m3 A.6): the renderer keeps one Slint `VecModel` per
/// list and applies these, so that unchanged rows — and the focus inside them — survive a render
/// (a new model would make the Repeater rebuild every row, review A7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListOp<T> {
    Remove(usize),
    Insert(usize, T),
    Set(usize, T),
}

/// The operations that turn `old` into `new`, rows matched by `key`: removals of rows that are
/// gone (from the end), then per position an unchanged row (nothing), a changed row (`Set`), a
/// moved row (`Remove` + `Insert`) or a new one (`Insert`), then removal of what is left over.
pub fn list_ops<T: Clone + PartialEq>(
    old: &[T],
    new: &[T],
    key: impl Fn(&T) -> &str,
) -> Vec<ListOp<T>> {
    let mut ops = Vec::new();
    let mut current: Vec<T> = old.to_vec();
    for index in (0..current.len()).rev() {
        if !new.iter().any(|row| key(row) == key(&current[index])) {
            current.remove(index);
            ops.push(ListOp::Remove(index));
        }
    }
    for (index, row) in new.iter().enumerate() {
        match current.get(index) {
            Some(existing) if key(existing) == key(row) => {
                if existing != row {
                    current[index] = row.clone();
                    ops.push(ListOp::Set(index, row.clone()));
                }
            }
            _ => {
                if let Some(from) = current.iter().position(|r| key(r) == key(row)) {
                    current.remove(from);
                    ops.push(ListOp::Remove(from));
                }
                current.insert(index, row.clone());
                ops.push(ListOp::Insert(index, row.clone()));
            }
        }
    }
    while current.len() > new.len() {
        let last = current.len() - 1;
        current.pop();
        ops.push(ListOp::Remove(last));
    }
    ops
}

/// Applies `ops` to a plain vector (what the renderer does to its `VecModel`).
pub fn apply_list_ops<T>(rows: &mut Vec<T>, ops: Vec<ListOp<T>>) {
    for op in ops {
        match op {
            ListOp::Remove(index) => {
                rows.remove(index);
            }
            ListOp::Insert(index, row) => rows.insert(index, row),
            ListOp::Set(index, row) => rows[index] = row,
        }
    }
}

/// Words in Latin letters that a Japanese screen may contain (design m3 D.5, review U5): device
/// names come from Windows and are passed in by the test; hexadecimal IDs (VID:PID, KLIDs) are
/// skipped.
#[cfg(test)]
pub(crate) const LATIN_ALLOWED: &[&str] = &[
    "Shift",
    "Alt",
    "Ctrl",
    "Space",
    "Backspace",
    "Tab",
    "Esc",
    "Enter",
    "Win",
    "USB",
    "PS",
    "Bluetooth",
    "LE",
    "I2C",
    "SPI",
    "JIS",
    "US",
    "MKLM",
    "mklm",
    "helper",
    "exe",
    "PIN",
    "IME",
    "PC",
    "Microsoft",
    "Windows",
];

/// The Latin-letter words of `text` that are neither allowed nor part of `names`.
#[cfg(test)]
pub(crate) fn unexpected_latin(text: &str, names: &[&str]) -> Vec<String> {
    let mut stripped = text.to_string();
    for name in names {
        stripped = stripped.replace(name, " ");
    }
    stripped
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.chars().any(|c| c.is_ascii_alphabetic()))
        .filter(|word| !word.chars().all(|c| c.is_ascii_hexdigit()) || word.len() < 4)
        .filter(|word| !LATIN_ALLOWED.contains(word))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(keys: &str) -> Vec<(String, u32)> {
        keys.chars().map(|c| (c.to_string(), 0)).collect()
    }

    #[test]
    fn list_ops_keep_unchanged_rows() {
        fn key(row: &(String, u32)) -> &str {
            &row.0
        }
        let old = rows("abc");
        // Nothing changed: nothing to do (the rows and their focus stay).
        assert!(list_ops(&old, &old, key).is_empty());
        let mut changed = old.clone();
        changed[1].1 = 7;
        assert_eq!(
            list_ops(&old, &changed, key),
            vec![ListOp::Set(1, ("b".into(), 7))]
        );
        for new in ["bca", "ac", "xabc", "cab", "", "abcd", "dcba"] {
            let new = rows(new);
            let mut applied = old.clone();
            apply_list_ops(&mut applied, list_ops(&old, &new, key));
            assert_eq!(applied, new);
        }
    }

    #[test]
    fn latin_words() {
        assert!(
            unexpected_latin(
                "Keychron Receiver を JIS に（3434:D027）",
                &["Keychron Receiver"]
            )
            .is_empty()
        );
        assert_eq!(
            unexpected_latin("Raw Input: 0x7/0x2", &[]),
            vec!["Raw", "Input", "0x7", "0x2"]
        );
    }
}
