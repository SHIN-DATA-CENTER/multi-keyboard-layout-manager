//! The restart screen (plan 3.6; design m3 B.8): why a restart is needed, what the user must know
//! before it (save files; a password with symbols may need other keys; sign in with a PIN), the
//! acknowledgement, and "restart now" only once the helper has flushed `PendingReboot`.
//!
//! The static texts (Restart, not Shut down; the sign-in help; save your files; what "later"
//! costs) are `@tr` labels of ui/screens/restart.slint; this view-model says what differs: the
//! reasons, whether the restart may start, and whether a layout changes with it.

use mklm_client::gate::restart_reasons;
use mklm_core::{BootId, Journal, JournalEntry, OpState, PendingAction, value_eq};

use super::SnapshotText;
use super::journal::{NameOf, kind_text};
use crate::i18n::{self, Lang, journal_pages as text};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Restart {
    /// One line per entry of `mklm_client::gate::restart_reasons`.
    pub reasons: Vec<String>,
    /// Some entry is `PendingReboot` / `RevertedPendingReboot` in the journal (flushed, plan
    /// 2.3), or carries an `apply_pending` restart of this boot.
    pub ready: bool,
    /// The layout of some keyboard changes at the restart: show the password warning ("同じ
    /// キーで別の記号が入力されます。PIN（数字）でサインインするか…", review U15 d).
    pub layout_changes: bool,
    /// Some reason is a change that still waits (`PendingReboot`): "変更を元に戻す…"
    /// (`Request::Undo`) can take it back before the restart. A revert or a kept change that only
    /// waits for the restart cannot be undone.
    pub can_undo: bool,
    /// No reason in the journal: "PC の再起動を待っている変更はありません。".
    pub empty_note: String,
}

/// The page when the journal could not be read (nothing to restart for).
pub fn nothing(lang: Lang) -> Restart {
    Restart {
        empty_note: text::restart_nothing(lang),
        ..Restart::default()
    }
}

/// Why `entry` needs the restart, in words ("PC の再起動待ち", "元に戻しました（PC の再起動が
/// 必要）", "保存済み（反映待ち: PC の再起動が必要）").
fn why(entry: &JournalEntry, lang: Lang) -> String {
    match entry.state {
        OpState::PendingReboot | OpState::RevertedPendingReboot => i18n::state(entry.state, lang),
        _ => i18n::pending(PendingAction::RestartPc, lang),
    }
}

/// True when the restart changes what some keyboard types: the entry wrote a value other than
/// the one it found (a reverted entry puts `before` back, which differs from what was in effect).
fn changes_a_layout(entry: &JournalEntry) -> bool {
    entry
        .records
        .iter()
        .any(|record| !value_eq(&record.name, &record.before, &record.intended))
}

/// The restart page from `mklm_client::gate::restart_reasons(journal, boot)`; each reason is the
/// operation in words (`vm::journal::kind_text`) and why it waits.
pub fn restart(journal: &Journal, boot: BootId, name_of: NameOf<'_>, lang: Lang) -> Restart {
    let reasons = restart_reasons(journal, boot);
    Restart {
        ready: !reasons.is_empty(),
        layout_changes: reasons.iter().any(|entry| changes_a_layout(entry)),
        can_undo: reasons
            .iter()
            .any(|entry| entry.state == OpState::PendingReboot),
        empty_note: if reasons.is_empty() {
            text::restart_nothing(lang)
        } else {
            String::new()
        },
        reasons: reasons
            .iter()
            .map(|entry| {
                text::restart_reason(
                    &kind_text(&entry.kind, name_of, lang),
                    &why(entry, lang),
                    lang,
                )
            })
            .collect(),
    }
}

impl SnapshotText for Restart {
    fn snapshot_text(&self) -> String {
        let mut out = String::new();
        for reason in &self.reasons {
            out.push_str(&format!("reason: {reason}\n"));
        }
        if !self.empty_note.is_empty() {
            out.push_str(&format!("empty: {}\n", self.empty_note));
        }
        out.push_str(&format!(
            "ready: {}\nlayout changes: {}\nundo: {}\n",
            self.ready, self.layout_changes, self.can_undo
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use mklm_core::{ApplyPending, Layout, OpKind, WriteTarget, value_names};

    use super::*;
    use crate::vm::test_journal::{self, BOOT, KEYCHRON, LATER_BOOT, entry};

    fn name_of(id: &str) -> String {
        if id.eq_ignore_ascii_case(KEYCHRON) {
            "Keychron Receiver".into()
        } else {
            id.into()
        }
    }

    fn migration() -> JournalEntry {
        let mut migrate = entry(
            "7a7a7a7a-0000-4000-8000-00000000000b",
            11,
            OpState::PendingReboot,
        );
        migrate.kind = OpKind::Migrate {
            standard: Layout::Jis,
            assignments: vec![(KEYCHRON.into(), mklm_core::LayoutChoice::Us)],
        };
        migrate.apply = Some(PendingAction::RestartPc);
        for record in &mut migrate.records {
            record.target = WriteTarget::Global;
            record.name = if record.name == value_names::HID_TYPE {
                value_names::PS2_TYPE.into()
            } else {
                value_names::PS2_SUBTYPE.into()
            };
            record.before = record.intended.clone();
            record.intended = mklm_core::RegValue::Absent;
        }
        migrate
    }

    #[test]
    fn a_migration_waits_for_the_restart() {
        let mut journal = test_journal::history();
        journal.entries.push(migration());
        let ja = restart(&journal, BOOT, &name_of, Lang::Ja);
        assert_eq!(
            ja.snapshot_text(),
            "reason: キーボードごとモードへ移行（標準配列 JIS）: PC の再起動待ち\n\
             ready: true\nlayout changes: true\nundo: true\n"
        );
        for reason in &ja.reasons {
            assert!(
                crate::vm::unexpected_latin(reason, &[]).is_empty(),
                "{reason}"
            );
        }
        let en = restart(&journal, BOOT, &name_of, Lang::En);
        assert_eq!(
            en.reasons,
            ["Switch to per-keyboard mode (standard JIS): Waiting for a PC restart"]
        );
        // After the restart the entry is the post-reboot check's, not a reason to restart.
        let later = restart(&journal, LATER_BOOT, &name_of, Lang::Ja);
        assert_eq!(
            later.snapshot_text(),
            "empty: PC の再起動を待っている変更はありません。\nready: false\nlayout changes: false\nundo: false\n"
        );
        assert_eq!(
            nothing(Lang::En).empty_note,
            "No change waits for a PC restart."
        );
    }

    #[test]
    fn a_kept_change_that_needs_the_restart_and_a_revert() {
        let mut journal = test_journal::history();
        let mut kept = entry(
            "7b7b7b7b-0000-4000-8000-00000000000c",
            12,
            OpState::Confirmed,
        );
        kept.apply_pending = Some(ApplyPending {
            action: PendingAction::RestartPc,
            instance_ids: vec![KEYCHRON.into()],
            since: BOOT,
        });
        journal.entries.push(kept);
        let mut reverted = entry(
            "7c7c7c7c-0000-4000-8000-00000000000d",
            13,
            OpState::RevertedPendingReboot,
        );
        reverted.kind = OpKind::Migrate {
            standard: Layout::Jis,
            assignments: Vec::new(),
        };
        journal.entries.push(reverted);
        let ja = restart(&journal, BOOT, &name_of, Lang::Ja);
        assert_eq!(
            ja.reasons,
            [
                "Keychron Receiver を JIS に: 保存済み（反映待ち: PC の再起動が必要）",
                "キーボードごとモードへ移行（標準配列 JIS）: 元に戻しました（PC の再起動が必要）",
            ]
        );
        assert!(ja.ready && ja.layout_changes);
        // Neither can be taken back before the restart: no "変更を元に戻す…".
        assert!(!ja.can_undo);
    }
}
