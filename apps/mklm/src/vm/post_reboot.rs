//! The check after a restart (plan 3.6; design m2 D.7, m3 B.9): per keyboard the layout its
//! stored values predict, what Windows reports, and what typing showed — in layout names, the
//! numbers under "技術的な詳細" (review U5). The migration note, the Shift+2 test. Never reverts
//! by itself.

use mklm_client::preview::CheckRow;
use mklm_core::{BootId, JournalEntry};

use super::Tone;
use crate::i18n::Lang;

/// What typing on a keyboard showed during this check (review U2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Typed {
    /// Not tried yet: "未".
    #[default]
    NotYet,
    /// Shift+2 gave the expected character: "✓".
    AsExpected,
    /// Shift+2 gave the other layout's character: "⚠" (keep stays possible, with a warning).
    Differs,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckLine {
    pub name: String,
    /// The layout the change set: "JIS" / "US" / "標準".
    pub setting: String,
    /// What Windows reports, in words, with ✓ / "違います" / "未接続".
    pub recognized: String,
    pub tone: Tone,
    /// "未" / "✓" / "⚠".
    pub typed: String,
    pub typed_tone: Tone,
    /// "0x7/0x2 → 0x7/0x2" for the technical details.
    pub details: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostReboot {
    pub operation: String,
    /// `PendingReboot` of this boot: "not in effect yet: Restart, not Shut down".
    pub not_restarted: bool,
    pub rows: Vec<CheckLine>,
    /// For a migration: Windows cannot show it (the built-in keyboard reports 7/2 either way);
    /// the typing test decides (design m2 0.1).
    pub migration_note: String,
    /// A changed keyboard typed the other layout: "このままにする" stays enabled, with this.
    pub keep_warning: String,
    /// "元に戻す", or "元に戻す（もう一度 PC の再起動が必要）" when the revert preview ends in a
    /// restart (a migration or a built-in keyboard, review U15 c).
    pub revert_text: String,
}

/// Rules (WP-U4): `rows` from `mklm_client::preview::check_rows`; "✓" when `CheckRow::matches`,
/// "未接続" when not reported, "違います" (warning) otherwise; `typed` from the key tests of this
/// check, per keyboard (`vm::keytest` with the row's expected layout).
pub fn post_reboot(
    entry: &JournalEntry,
    rows: &[CheckRow],
    typed: &[(String, Typed)],
    boot: BootId,
    lang: Lang,
) -> PostReboot {
    let _ = (entry, rows, typed, boot, lang);
    todo!("WP-U4: the post-reboot view-model")
}
