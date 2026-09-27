//! The recovery page (design m2 D.7, m3 B.12; review U9): each entry that needs something, with
//! what recovering will do to it — computed unelevated with `mklm_core::decide_recovery` from the
//! values the display snapshot reads, the same pure rule the helper applies under its lock (the
//! helper decides again; the page only predicts).
//!
//! "回復する（おすすめ）" is the one primary button. "確認待ちの変更をすべて元に戻す" appears only
//! when an entry waits for the user (`AwaitingUser`). "後で" says what it costs: until recovery,
//! no layout can be changed.

use mklm_core::{JournalEntry, RecoveryContext, RecoveryDecision, RegValue};

use super::Tone;
use crate::i18n::Lang;

/// One entry on the page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryItem {
    /// "Keychron Receiver を JIS に（途中で止まりました）".
    pub operation: String,
    /// "→ 回復すると US（変更前）に戻ります。キーボードの動作は変わっていません" /
    /// "→ 書き込みは終わっています。回復後に、このままにするか元に戻すかを選べます".
    pub outcome: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryPage {
    pub items: Vec<RecoveryItem>,
    /// Some entry needs `Recover`.
    pub can_recover: bool,
    /// Some entry waits for keep or revert: "確認待ちの変更をすべて元に戻す" (`Request::Undo`).
    pub can_undo: bool,
    /// "回復するまで、キーボードの配列は変更できません".
    pub later_note: String,
}

/// The prediction for one entry, in words (WP-U5): `RollBack` → back to the value before (and
/// whether the keyboard switched, `describe::reset_phase`); `RollForward` / `CompleteForward` →
/// the write is complete and waits for keep or revert (or the restart); `Conflict` → the conflict
/// page follows; `MarkNothingWritten` → nothing had been written.
pub fn outcome_text(entry: &JournalEntry, decision: &RecoveryDecision, lang: Lang) -> String {
    let _ = (entry, decision, lang);
    todo!("WP-U5: the recovery prediction in words")
}

/// The page (WP-U5): per `Recover` or `AwaitingUser` entry of the start-up summary,
/// `decide_recovery(entry, current, context)` with `current` from the display snapshot.
pub fn recovery_page(
    entries: &[(&JournalEntry, Vec<RegValue>)],
    context: &RecoveryContext,
    lang: Lang,
) -> RecoveryPage {
    let _ = (entries, context, lang);
    todo!("WP-U5: the recovery page")
}
