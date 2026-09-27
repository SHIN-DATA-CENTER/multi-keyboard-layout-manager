//! The history (design m3 B.11): journal entries newest first, in local time (design m3 G.1),
//! with the reason worded by whether the reset was reached (design m3 G.2).

use mklm_client::describe::reset_phase;
use mklm_core::{
    Attention, FailureReason, Journal, JournalEntry, LayoutChoice, OpKind, OpState, RestoreScope,
    Timestamp,
};

use super::Tone;
use crate::i18n::{self, Lang};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JournalRow {
    /// The short operation ID (8 hex digits).
    pub op: String,
    pub when: String,
    pub what: String,
    pub state: String,
    pub reason: String,
    pub tone: Tone,
    pub can_revert: bool,
    /// The revert button's accessible label ("2026/09/27 22:25 の変更を元に戻す", design m3 E.2).
    pub revert_label: String,
}

/// Formats a timestamp for display: the GUI passes `mklm_win::time::local_time` (local time with
/// the offset); tests pass a fixed formatter.
pub type TimeText<'a> = &'a dyn Fn(Timestamp) -> String;

/// The display name of a keyboard by instance ID (from the snapshot; the ID when unknown).
pub type NameOf<'a> = &'a dyn Fn(&str) -> String;

fn choice(choice: LayoutChoice, lang: Lang) -> String {
    match (choice, lang) {
        (LayoutChoice::Jis, _) => "JIS".into(),
        (LayoutChoice::Us, _) => "US".into(),
        (LayoutChoice::Standard, Lang::Ja) => "標準に従う".into(),
        (LayoutChoice::Standard, Lang::En) => "follow the standard".into(),
    }
}

/// What an operation was for, in one line.
pub fn kind_text(kind: &OpKind, name_of: NameOf<'_>, lang: Lang) -> String {
    match kind {
        OpKind::SetLayout {
            requested, layout, ..
        } => match lang {
            Lang::Ja => format!("{} を {}", name_of(requested), choice(*layout, lang)),
            Lang::En => format!("{} to {}", name_of(requested), choice(*layout, lang)),
        },
        OpKind::Migrate { standard, .. } => {
            let standard = i18n::table(&(*standard).into(), lang);
            match lang {
                Lang::Ja => format!("キーボードごとモードへ移行（標準配列 {standard}）"),
                Lang::En => format!("Switch to per-keyboard mode (standard {standard})"),
            }
        }
        OpKind::RestoreBaseline { scope, .. } => match (scope, lang) {
            (RestoreScope::All, Lang::Ja) => "MKLM 導入前に戻す（すべて）".into(),
            (RestoreScope::All, Lang::En) => "Back to before MKLM (everything)".into(),
            (RestoreScope::Device { instance_id }, Lang::Ja) => {
                format!("MKLM 導入前に戻す（{}）", name_of(instance_id))
            }
            (RestoreScope::Device { instance_id }, Lang::En) => {
                format!("Back to before MKLM ({})", name_of(instance_id))
            }
        },
        OpKind::Cleanup { instance_id, .. } => i18n::cleanup_text(&name_of(instance_id), lang),
    }
}
fn tone(entry: &JournalEntry) -> Tone {
    match entry.state {
        OpState::Confirmed => Tone::Success,
        OpState::Conflict => Tone::Danger,
        // The user chose to keep the values changed outside MKLM.
        OpState::Failed if entry.failure == Some(FailureReason::ConflictKeptCurrent) => {
            Tone::Neutral
        }
        OpState::Failed | OpState::Reverted | OpState::RevertedPendingReboot
            if entry.failure.is_some() =>
        {
            Tone::Warning
        }
        state if state.is_open() => Tone::Info,
        _ => Tone::Neutral,
    }
}

/// The rows, newest first. `revertible` tells whether the GUI offers "revert" for an entry (the
/// latest operation on its values, as the engine requires; design m2 C.8); `attention` is the
/// entry's `mklm_core::attention` now (an interrupted write whose writer still runs is "処理中",
/// not "途中で止まりました").
pub fn journal_rows(
    journal: &Journal,
    time_text: TimeText<'_>,
    name_of: NameOf<'_>,
    revertible: &dyn Fn(&JournalEntry) -> bool,
    attention: &dyn Fn(&JournalEntry) -> Attention,
    lang: Lang,
) -> Vec<JournalRow> {
    let mut entries: Vec<&JournalEntry> = journal.entries.iter().collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.seq));
    entries
        .into_iter()
        .map(|entry| JournalRow {
            op: entry.op_id.short().to_string(),
            when: time_text(entry.created_at),
            what: kind_text(&entry.kind, name_of, lang),
            state: i18n::entry_state(entry.state, entry.failure.as_ref(), attention(entry), lang),
            reason: entry
                .failure
                .as_ref()
                .map(|failure| i18n::failure(failure, reset_phase(entry), lang))
                .unwrap_or_default(),
            tone: tone(entry),
            can_revert: revertible(entry),
            revert_label: match lang {
                Lang::Ja => format!("{} の変更を元に戻す", time_text(entry.created_at)),
                Lang::En => format!("Revert the change of {}", time_text(entry.created_at)),
            },
        })
        .collect()
}
