//! The recovery page (design m2 D.7, m3 B.12; review U9): each entry that needs something, with
//! what recovering will do to it — computed unelevated with `mklm_core::decide_recovery` from the
//! values the display snapshot reads, the same pure rule the helper applies under its lock (the
//! helper decides again; the page only predicts).
//!
//! "回復する（おすすめ）" is the one primary button. "確認待ちの変更をすべて元に戻す" appears only
//! when an entry waits for the user (`AwaitingUser`). "後で" says what it costs: until recovery,
//! no layout can be changed.
//!
//! The same page previews the two other requests that put values back: "確認待ちの変更をすべて
//! 元に戻す" (`Request::Undo`, from `mklm_client::preview::undo_preview`) and the history's
//! "元に戻す…" of one operation (`Request::Revert`). Each offers the two ways a HID keyboard
//! takes the old values (design m3 B.5) when a HID keyboard is involved.

use mklm_client::describe::{ResetPhase, reset_phase};
use mklm_client::preview::UndoPreview;
use mklm_core::{
    Attention, JournalEntry, OpState, RecoveryContext, RecoveryDecision, RegValue, ValueRecord,
    WriteTarget, decide_recovery_with_removed, value_names,
};

use super::conflict::main_pair_name;
use super::journal::{NameOf, kind_text};
use super::{SnapshotText, Tone};
use crate::i18n::{Lang, journal_pages as text};

/// Which request the page previews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecoveryMode {
    /// What the journal needs: `Request::Recover`.
    #[default]
    Attention,
    /// "確認待ちの変更をすべて元に戻す": `Request::Undo`.
    Undo,
    /// The history's "元に戻す…": `Request::Revert`.
    Revert,
}

impl RecoveryMode {
    fn title(self) -> text::RecoveryTitle {
        match self {
            RecoveryMode::Attention => text::RecoveryTitle::Attention,
            RecoveryMode::Undo => text::RecoveryTitle::Undo,
            RecoveryMode::Revert => text::RecoveryTitle::Revert,
        }
    }
}

/// One entry on the page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryItem {
    /// The full operation ID (what the item's buttons pass back).
    pub op_id: String,
    /// "Keychron Receiver を JIS に（途中で止まりました）".
    pub operation: String,
    /// "→ 回復すると US（変更前）に戻ります。キーボードの動作は変わっていません" /
    /// "→ 書き込みは終わっています。回復後に、このままにするか元に戻すかを選べます".
    pub outcome: String,
    pub tone: Tone,
    /// It waits for keep or revert: "このままにする" (`Request::Confirm`).
    pub can_keep: bool,
    /// ... and "元に戻す…" (the revert preview), when the helper would accept it.
    pub can_revert: bool,
    /// The accessible labels of those two buttons, naming the operation ("Keychron Receiver を
    /// JIS に（確認待ち）をこのままにする"; design m3 E.2): with two entries, the buttons would
    /// otherwise be read with the same names. Empty without the button.
    pub keep_label: String,
    pub revert_label: String,
}

/// The two ways a HID keyboard takes the values back (design m3 B.5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MethodChoice {
    /// A HID keyboard is involved: the choice is shown (otherwise nothing is reset).
    pub visible: bool,
    /// 0 = switch back now (reset in place), 1 = when replugged or at the restart.
    pub selected: usize,
    pub live_text: String,
    pub live_detail: String,
    pub later_text: String,
    pub later_detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryPage {
    pub mode: RecoveryMode,
    pub title: String,
    pub items: Vec<RecoveryItem>,
    /// The primary button may be pressed (something to recover, undo or revert).
    pub can_recover: bool,
    /// "回復する（おすすめ）" / "すべて元に戻す" / "元に戻す".
    pub primary_text: String,
    /// Some entry waits for keep or revert: "確認待ちの変更をすべて元に戻す" (`Request::Undo`).
    pub can_undo: bool,
    /// "後で" / "キャンセル".
    pub dismiss_text: String,
    /// "回復するまで、キーボードの配列は変更できません".
    pub later_note: String,
    pub method: MethodChoice,
    /// The line about the Windows prompt the primary button brings up (design m3 B.5).
    pub uac_line: String,
    /// Nothing to show: why.
    pub empty_note: String,
}

/// An entry of the start-up summary with what the page needs to predict its recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionEntry<'a> {
    pub entry: &'a JournalEntry,
    pub attention: Attention,
    /// The value stored now for each record, from the display snapshot (`None` inside: the
    /// keyboard no longer exists); `None` when the keyboards could not be read.
    pub current: Option<Vec<Option<RegValue>>>,
    /// The history's rule: the helper would revert it (`vm::journal::revertible`).
    pub revertible: bool,
    /// Keep or revert may be sent now: no interrupted or busy entry stops them first (the
    /// helper's gate for existing operations); otherwise the buttons wait for the recovery.
    pub decidable: bool,
}

/// True when `entry` writes a HID keyboard's type (a reset in place can apply it).
pub fn touches_hid(entry: &JournalEntry) -> bool {
    if entry
        .records
        .iter()
        .any(|r| r.target == WriteTarget::Global && !r.is_check_only())
    {
        return false;
    }
    entry.records.iter().any(|record| {
        matches!(record.target, WriteTarget::Device { .. })
            && (record.name.eq_ignore_ascii_case(value_names::HID_TYPE)
                || record.name.eq_ignore_ascii_case(value_names::HID_SUBTYPE))
    })
}

/// The HID keyboards (instance IDs) `entry` writes: the targets of the apply method's default
/// (`vm::change::default_apply_method`).
pub fn hid_targets(entry: &JournalEntry) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for record in &entry.records {
        if let WriteTarget::Device { instance_id } = &record.target
            && (record.name.eq_ignore_ascii_case(value_names::HID_TYPE)
                || record.name.eq_ignore_ascii_case(value_names::HID_SUBTYPE))
            && !ids.iter().any(|id| id.eq_ignore_ascii_case(instance_id))
        {
            ids.push(instance_id.clone());
        }
    }
    ids
}

/// What recovery would decide for `entry` now, from the values the display snapshot read.
pub fn predict(
    entry: &JournalEntry,
    current: &[Option<RegValue>],
    context: &RecoveryContext,
) -> RecoveryDecision {
    decide_recovery_with_removed(entry, current, context)
}

fn before_name(entry: &JournalEntry, lang: Lang) -> String {
    main_pair_name(
        entry,
        &|record: &ValueRecord| Some(record.before.clone()),
        lang,
    )
}

/// The prediction for one entry, in words (WP-U5): `RollBack` → back to the value before (and
/// whether the keyboard switched, `describe::reset_phase`); `RollForward` / `CompleteForward` →
/// the write is complete and waits for keep or revert (or the restart); `Conflict` → the conflict
/// page follows; `MarkNothingWritten` → nothing had been written.
pub fn outcome_text(entry: &JournalEntry, decision: &RecoveryDecision, lang: Lang) -> String {
    let before = before_name(entry, lang);
    let outcome = match decision {
        RecoveryDecision::Leave { reason } => match reason {
            mklm_core::LeaveReason::WaitingForUser => text::RecoveryOutcome::WaitsForUser,
            mklm_core::LeaveReason::WaitingForReboot => text::RecoveryOutcome::WaitsForReboot,
            mklm_core::LeaveReason::Conflict => text::RecoveryOutcome::WaitsForConflict,
            mklm_core::LeaveReason::Closed => return String::new(),
        },
        RecoveryDecision::MarkNothingWritten => text::RecoveryOutcome::NothingWritten,
        RecoveryDecision::RollForward { to } => text::RecoveryOutcome::Forward {
            restart: *to == OpState::PendingReboot,
        },
        RecoveryDecision::CompleteForward { to } => text::RecoveryOutcome::CompleteRestore {
            restart: *to == OpState::PendingReboot,
        },
        RecoveryDecision::RollBack { .. } => text::RecoveryOutcome::RollBack {
            before: &before,
            switched: reset_phase(entry) == ResetPhase::Reached,
            restart: entry.touches_boot_time_values(),
        },
        RecoveryDecision::ContinueRevert => text::RecoveryOutcome::ContinueRevert,
        RecoveryDecision::RebootObserved { to } if *to == OpState::Reverted => {
            text::RecoveryOutcome::RevertApplied
        }
        RecoveryDecision::RebootObserved { .. } => text::RecoveryOutcome::RebootObserved,
        RecoveryDecision::Conflict { .. } => text::RecoveryOutcome::Conflict,
    };
    text::recovery_outcome(&outcome, lang)
}

/// Where an entry stands, for the operation line.
fn phase(entry: &JournalEntry, attention: Attention) -> text::EntryPhase {
    match (attention, entry.state) {
        (Attention::Busy, _) => text::EntryPhase::Busy,
        (Attention::AwaitingUser, _) => text::EntryPhase::AwaitingUser,
        (Attention::WaitingForReboot, _) => text::EntryPhase::WaitingForReboot,
        (Attention::Conflict, _) => text::EntryPhase::Conflict,
        (_, OpState::RevertPending) => text::EntryPhase::RevertInterrupted,
        (_, OpState::AwaitingConfirm) => text::EntryPhase::CountdownAbandoned,
        (_, OpState::PendingReboot) => text::EntryPhase::Restarted,
        _ => text::EntryPhase::Interrupted,
    }
}

fn method(visible: bool, live: bool, lang: Lang) -> MethodChoice {
    let (live_text, live_detail) = text::revert_method(true, lang);
    let (later_text, later_detail) = text::revert_method(false, lang);
    MethodChoice {
        visible,
        selected: if live { 0 } else { 1 },
        live_text,
        live_detail,
        later_text,
        later_detail,
    }
}

/// The page (WP-U5): per `Recover`, `AwaitingUser` or `Busy` entry of the start-up summary,
/// `decide_recovery(entry, current, context)` with `current` from the display snapshot. A
/// `PendingReboot` seen from a new boot is the post-reboot check's (design m3 B.9), not listed
/// here. `context` is `None` when the boot could not be read (no prediction then). `live`:
/// "switch back now" is the selected apply method (preset from
/// `vm::change::default_apply_method` when the page opened, then the user's choice).
pub fn recovery_page(
    entries: &[AttentionEntry<'_>],
    context: Option<&RecoveryContext>,
    name_of: NameOf<'_>,
    live: bool,
    lang: Lang,
) -> RecoveryPage {
    let mode = RecoveryMode::Attention;
    let mut items = Vec::new();
    let mut method_visible = false;
    for item in entries {
        let entry = item.entry;
        let listed = match item.attention {
            Attention::Recover => entry.state != OpState::PendingReboot,
            Attention::AwaitingUser | Attention::Busy => true,
            _ => false,
        };
        if !listed {
            continue;
        }
        let decision = match (&item.current, context) {
            (Some(current), Some(context)) if item.attention == Attention::Recover => {
                Some(predict(entry, current, context))
            }
            _ => None,
        };
        let outcome = match (item.attention, &decision) {
            (Attention::AwaitingUser, _) => {
                text::recovery_outcome(&text::RecoveryOutcome::WaitsForUser, lang)
            }
            (Attention::Busy, _) => text::recovery_outcome(&text::RecoveryOutcome::Busy, lang),
            (_, Some(decision)) => outcome_text(entry, decision, lang),
            (_, None) => text::recovery_outcome(&text::RecoveryOutcome::Unknown, lang),
        };
        // A reset in place matters when a HID keyboard switched and goes back, or when the
        // prediction is not known.
        if item.attention == Attention::Recover
            && touches_hid(entry)
            && match &decision {
                Some(RecoveryDecision::RollBack { .. }) => {
                    reset_phase(entry) == ResetPhase::Reached
                }
                Some(RecoveryDecision::ContinueRevert) | None => true,
                Some(_) => false,
            }
        {
            method_visible = true;
        }
        let operation = text::recovery_operation(
            &kind_text(&entry.kind, name_of, lang),
            &text::entry_phase(phase(entry, item.attention), lang),
            lang,
        );
        let can_keep = item.attention == Attention::AwaitingUser && item.decidable;
        let can_revert = can_keep && item.revertible;
        items.push(RecoveryItem {
            op_id: entry.op_id.to_string(),
            keep_label: if can_keep {
                text::recovery_keep_label(&operation, lang)
            } else {
                String::new()
            },
            revert_label: if can_revert {
                text::recovery_revert_label(&operation, lang)
            } else {
                String::new()
            },
            operation,
            outcome,
            tone: match (item.attention, &decision) {
                (Attention::Busy, _) => Tone::Neutral,
                (Attention::AwaitingUser, _) | (_, Some(RecoveryDecision::Conflict { .. })) => {
                    Tone::Warning
                }
                _ => Tone::Info,
            },
            can_keep,
            can_revert,
        });
    }
    let can_recover = entries.iter().any(|item| {
        item.attention == Attention::Recover && item.entry.state != OpState::PendingReboot
    });
    let can_undo = entries
        .iter()
        .any(|item| item.attention == Attention::AwaitingUser);
    RecoveryPage {
        mode,
        title: text::recovery_title(mode.title(), lang),
        empty_note: if items.is_empty() {
            text::recovery_nothing(mode.title(), lang)
        } else {
            String::new()
        },
        items,
        can_recover,
        primary_text: text::recovery_primary(mode.title(), lang),
        can_undo,
        dismiss_text: text::recovery_dismiss(mode.title(), lang),
        later_note: if can_recover {
            text::recovery_later_note(lang)
        } else {
            String::new()
        },
        method: method(method_visible && can_recover, live, lang),
        uac_line: if can_recover || can_undo {
            crate::i18n::uac_line(lang)
        } else {
            String::new()
        },
    }
}

/// The preview of "確認待ちの変更をすべて元に戻す" (design m2 D.10): interrupted entries are
/// recovered first, then every entry that waits for the user is put back, newest first.
#[cfg(test)]
pub fn undo_page(
    preview: &UndoPreview<'_>,
    name_of: NameOf<'_>,
    live: bool,
    lang: Lang,
) -> RecoveryPage {
    undo_page_at(preview, name_of, live, lang, None)
}

pub fn undo_page_at(
    preview: &UndoPreview<'_>,
    name_of: NameOf<'_>,
    live: bool,
    lang: Lang,
    boot: Option<mklm_core::BootId>,
) -> RecoveryPage {
    let mode = RecoveryMode::Undo;
    let mut items: Vec<RecoveryItem> = preview
        .recovered_first
        .iter()
        .map(|entry| RecoveryItem {
            op_id: entry.op_id.to_string(),
            operation: text::recovery_operation(
                &kind_text(&entry.kind, name_of, lang),
                &text::entry_phase(text::EntryPhase::Interrupted, lang),
                lang,
            ),
            outcome: text::undo_outcome(&text::UndoOutcome::RecoveredFirst, lang),
            tone: Tone::Info,
            ..RecoveryItem::default()
        })
        .collect();
    let before: Vec<String> = preview
        .undone
        .iter()
        .map(|undo| before_name(undo.entry, lang))
        .collect();
    items.extend(
        preview
            .undone
            .iter()
            .zip(&before)
            .map(|(undo, before)| RecoveryItem {
                op_id: undo.entry.op_id.to_string(),
                operation: text::recovery_operation(
                    &kind_text(&undo.entry.kind, name_of, lang),
                    &text::shown_state(mklm_client::describe::shown_state(undo.entry, boot), lang),
                    lang,
                ),
                outcome: text::undo_outcome(
                    &text::UndoOutcome::Back {
                        before,
                        restart: undo.restart_needed,
                        kept_outside: undo.values.iter().any(|value| !value.restorable),
                    },
                    lang,
                ),
                tone: Tone::Info,
                ..RecoveryItem::default()
            }),
    );
    let method_visible = preview.undone.iter().any(|undo| touches_hid(undo.entry));
    let can = !preview.is_empty();
    RecoveryPage {
        mode,
        title: text::recovery_title(mode.title(), lang),
        empty_note: if can {
            String::new()
        } else {
            text::recovery_nothing(mode.title(), lang)
        },
        items,
        can_recover: can,
        primary_text: text::recovery_primary(mode.title(), lang),
        can_undo: false,
        dismiss_text: text::recovery_dismiss(mode.title(), lang),
        later_note: String::new(),
        method: method(method_visible && can, live, lang),
        uac_line: if can {
            crate::i18n::uac_line(lang)
        } else {
            String::new()
        },
    }
}

/// The preview of the history's "元に戻す…" (design m2 D.4): `entry` when the helper would
/// revert it (`revertible`), else why not.
#[cfg(test)]
pub fn revert_page(
    entry: Option<&JournalEntry>,
    revertible: bool,
    when: &str,
    name_of: NameOf<'_>,
    live: bool,
    lang: Lang,
) -> RecoveryPage {
    revert_page_at(entry, revertible, when, name_of, live, lang, None)
}

pub fn revert_page_at(
    entry: Option<&JournalEntry>,
    revertible: bool,
    when: &str,
    name_of: NameOf<'_>,
    live: bool,
    lang: Lang,
    boot: Option<mklm_core::BootId>,
) -> RecoveryPage {
    let mode = RecoveryMode::Revert;
    let entry = entry.filter(|_| revertible);
    let items: Vec<RecoveryItem> = entry
        .map(|entry| RecoveryItem {
            op_id: entry.op_id.to_string(),
            operation: text::recovery_operation(
                &format!("{when} {}", kind_text(&entry.kind, name_of, lang)),
                &text::shown_state(mklm_client::describe::shown_state(entry, boot), lang),
                lang,
            ),
            outcome: text::undo_outcome(
                &text::UndoOutcome::Back {
                    before: &before_name(entry, lang),
                    restart: entry.touches_boot_time_values(),
                    kept_outside: false,
                },
                lang,
            ),
            tone: Tone::Info,
            ..RecoveryItem::default()
        })
        .into_iter()
        .collect();
    let can = entry.is_some();
    RecoveryPage {
        mode,
        title: text::recovery_title(mode.title(), lang),
        empty_note: if can {
            String::new()
        } else {
            text::recovery_nothing(mode.title(), lang)
        },
        items,
        can_recover: can,
        primary_text: text::recovery_primary(mode.title(), lang),
        can_undo: false,
        dismiss_text: text::recovery_dismiss(mode.title(), lang),
        later_note: String::new(),
        method: method(entry.is_some_and(touches_hid), live, lang),
        uac_line: if can {
            crate::i18n::uac_line(lang)
        } else {
            String::new()
        },
    }
}

impl SnapshotText for RecoveryPage {
    fn snapshot_text(&self) -> String {
        let mut out = format!("title: {}\n", self.title);
        for item in &self.items {
            out.push_str(&format!(
                "item: {}\n  {} ({:?}){}{}\n",
                item.operation,
                item.outcome,
                item.tone,
                if item.can_keep { " [keep]" } else { "" },
                if item.can_revert { " [revert]" } else { "" }
            ));
            for label in [&item.keep_label, &item.revert_label] {
                if !label.is_empty() {
                    out.push_str(&format!("  button: {label}\n"));
                }
            }
        }
        for (label, value) in [
            ("empty", &self.empty_note),
            ("note", &self.later_note),
            ("uac", &self.uac_line),
        ] {
            if !value.is_empty() {
                out.push_str(&format!("{label}: {value}\n"));
            }
        }
        if self.method.visible {
            let mark = |index| {
                if self.method.selected == index {
                    "◉"
                } else {
                    "○"
                }
            };
            out.push_str(&format!(
                "method: {} {} / {} {}\n",
                mark(0),
                self.method.live_text,
                mark(1),
                self.method.later_text
            ));
        }
        out.push_str(&format!("buttons: [{}]", self.dismiss_text));
        if self.can_undo {
            out.push_str(" [undo]");
        }
        if self.can_recover {
            out.push_str(&format!(" [{}]", self.primary_text));
        }
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use mklm_client::preview::undo_preview;
    use mklm_core::{BootId, Countdown, Journal, Timestamp};

    use super::*;
    use crate::vm::test_journal::{self, BOOT, KEYCHRON, LATER_BOOT, entry, went_through};

    fn name_of(id: &str) -> String {
        if id.eq_ignore_ascii_case(KEYCHRON) {
            "Keychron Receiver".into()
        } else {
            id.into()
        }
    }

    fn context(boot: BootId) -> RecoveryContext {
        RecoveryContext {
            current_boot: boot,
            inv_ps2: None,
        }
    }

    fn values(pairs: &[u32]) -> Option<Vec<Option<RegValue>>> {
        Some(
            pairs
                .iter()
                .map(|value| Some(RegValue::Dword { value: *value }))
                .collect(),
        )
    }

    fn page(item: AttentionEntry<'_>, lang: Lang) -> RecoveryPage {
        recovery_page(&[item], Some(&context(BOOT)), &name_of, false, lang)
    }

    #[test]
    fn the_r5_case_rolls_back_without_a_switch() {
        // Written, the writer killed before the reset (R5): every value at `intended`.
        let written = entry("aaaaaaaa-0000-4000-8000-000000000011", 17, OpState::Written);
        let ja = page(
            AttentionEntry {
                entry: &written,
                attention: Attention::Recover,
                current: values(&[7, 2]),
                revertible: false,
                decidable: true,
            },
            Lang::Ja,
        );
        assert_eq!(
            ja.snapshot_text(),
            "title: 確認が必要なことがあります\n\
             item: Keychron Receiver を JIS に（途中で止まりました）\n\
             \x20 → 回復すると US（変更前）に戻ります。キーボードの動作は変わっていません (Info)\n\
             note: 回復するまで、キーボードの配列は変更できません。\n\
             uac: 次に Windows の確認画面が出ます（発行元は「不明」）。mklm-helper.exe であることを確かめて「はい」を押してください。\n\
             buttons: [後で] [回復する（おすすめ）]\n"
        );
        for item in &ja.items {
            for text in [&item.operation, &item.outcome] {
                assert!(
                    crate::vm::unexpected_latin(text, &["Keychron Receiver"]).is_empty(),
                    "{text}"
                );
            }
        }
        let en = page(
            AttentionEntry {
                entry: &written,
                attention: Attention::Recover,
                current: values(&[7, 2]),
                revertible: false,
                decidable: true,
            },
            Lang::En,
        );
        assert_eq!(
            en.items[0].outcome,
            "→ Recovering puts it back to US (as before); the keyboard never switched"
        );
    }

    #[test]
    fn an_abandoned_countdown_rolls_back_after_the_switch() {
        let mut counting = entry(
            "bbbbbbbb-0000-4000-8000-000000000012",
            18,
            OpState::AwaitingConfirm,
        );
        went_through(&mut counting, OpState::Written);
        went_through(&mut counting, OpState::Restarting);
        went_through(&mut counting, OpState::AwaitingConfirm);
        counting.countdown = Some(Countdown {
            seconds: 20,
            deadline: Timestamp(0),
        });
        let ja = recovery_page(
            &[AttentionEntry {
                entry: &counting,
                attention: Attention::Recover,
                current: values(&[7, 2]),
                revertible: false,
                decidable: true,
            }],
            Some(&context(BOOT)),
            &name_of,
            true,
            Lang::Ja,
        );
        assert_eq!(
            ja.items[0].operation,
            "Keychron Receiver を JIS に（試している途中で MKLM が終了しました）"
        );
        assert_eq!(ja.items[0].outcome, "→ 回復すると US（変更前）に戻ります");
        // The keyboard switched: how it switches back is a choice, "now" preselected.
        assert!(ja.method.visible);
        assert_eq!(ja.method.selected, 0);
        assert_eq!(ja.method.live_text, "すぐに元の配列に戻す");
    }

    #[test]
    fn other_predictions_in_words() {
        let planned = entry("cccccccc-0000-4000-8000-000000000013", 19, OpState::Planned);
        // Nothing written yet.
        let nothing = page(
            AttentionEntry {
                entry: &planned,
                attention: Attention::Recover,
                current: values(&[4, 0]),
                revertible: false,
                decidable: true,
            },
            Lang::Ja,
        );
        assert_eq!(
            nothing.items[0].outcome,
            "→ 何も書き込まれていませんでした。記録を閉じるだけです"
        );
        // Changed outside MKLM meanwhile.
        let outside = page(
            AttentionEntry {
                entry: &planned,
                attention: Attention::Recover,
                current: values(&[8, 2]),
                revertible: false,
                decidable: true,
            },
            Lang::Ja,
        );
        assert_eq!(
            (outside.items[0].outcome.as_str(), outside.items[0].tone),
            (
                "→ MKLM 以外の変更が見つかりました。回復の後で、どうするかを選びます",
                Tone::Warning
            )
        );
        // A restart-path change whose writer stopped after writing: rolled forward.
        let mut restart = entry("dddddddd-0000-4000-8000-000000000014", 20, OpState::Written);
        restart.apply = Some(mklm_core::PendingAction::RestartPc);
        let forward = page(
            AttentionEntry {
                entry: &restart,
                attention: Attention::Recover,
                current: values(&[7, 2]),
                revertible: false,
                decidable: true,
            },
            Lang::Ja,
        );
        assert_eq!(
            forward.items[0].outcome,
            "→ 書き込みは終わっています。回復の後、PC を再起動すると反映されます。その後、このままにするか元に戻すかを選べます"
        );
        // Without the keyboards, no prediction.
        let unknown = page(
            AttentionEntry {
                entry: &restart,
                attention: Attention::Recover,
                current: None,
                revertible: false,
                decidable: true,
            },
            Lang::Ja,
        );
        assert!(
            unknown.items[0].outcome.contains("mklm-helper.exe"),
            "{}",
            unknown.items[0].outcome
        );
        assert!(unknown.method.visible);
    }

    #[test]
    fn a_change_waiting_for_the_user_offers_keep_revert_and_undo() {
        let waiting = entry(
            "eeeeeeee-0000-4000-8000-000000000015",
            21,
            OpState::AwaitingConfirm,
        );
        let ja = page(
            AttentionEntry {
                entry: &waiting,
                attention: Attention::AwaitingUser,
                current: values(&[7, 2]),
                revertible: true,
                decidable: true,
            },
            Lang::Ja,
        );
        assert_eq!(
            ja.snapshot_text(),
            "title: 確認が必要なことがあります\n\
             item: Keychron Receiver を JIS に（確認待ち）\n\
             \x20 このままにするか、元に戻すかを選んでください (Warning) [keep] [revert]\n\
             \x20 button: Keychron Receiver を JIS に（確認待ち）をこのままにする\n\
             \x20 button: Keychron Receiver を JIS に（確認待ち）を元に戻す…\n\
             uac: 次に Windows の確認画面が出ます（発行元は「不明」）。mklm-helper.exe であることを確かめて「はい」を押してください。\n\
             buttons: [後で] [undo]\n"
        );
        // The post-reboot check's entry is not listed here (design m3 B.12).
        let pending = entry(
            "ffffffff-0000-4000-8000-000000000016",
            22,
            OpState::PendingReboot,
        );
        let none = recovery_page(
            &[AttentionEntry {
                entry: &pending,
                attention: Attention::Recover,
                current: values(&[7, 2]),
                revertible: false,
                decidable: true,
            }],
            Some(&context(LATER_BOOT)),
            &name_of,
            false,
            Lang::Ja,
        );
        assert_eq!(
            none.snapshot_text(),
            "title: 確認が必要なことがあります\nempty: 回復や確認が必要なものはありません。\nbuttons: [後で]\n"
        );
    }

    #[test]
    fn the_undo_and_revert_previews() {
        let mut journal = test_journal::history();
        journal.entries.push(entry(
            "abababab-0000-4000-8000-000000000017",
            23,
            OpState::AwaitingConfirm,
        ));
        let now = |_: &ValueRecord| Some(RegValue::Dword { value: 7 });
        let preview = undo_preview(&journal, &now);
        let ja = undo_page(&preview, &name_of, false, Lang::Ja);
        assert_eq!(
            ja.snapshot_text(),
            "title: 確認待ちの変更をすべて元に戻す\n\
             item: Keychron Receiver を JIS に（確認待ち）\n\
             \x20 → US（変更前）に戻します (Info)\n\
             uac: 次に Windows の確認画面が出ます（発行元は「不明」）。mklm-helper.exe であることを確かめて「はい」を押してください。\n\
             method: ○ すぐに元の配列に戻す / ◉ 抜き差しか PC の再起動で戻す\n\
             buttons: [キャンセル] [すべて元に戻す]\n"
        );
        let empty = undo_page(
            &undo_preview(&Journal::default(), &now),
            &name_of,
            false,
            Lang::En,
        );
        assert_eq!(
            empty.snapshot_text(),
            "title: Undo every change waiting for you\nempty: No change is waiting for you.\nbuttons: [Cancel]\n"
        );
        // The history's revert of the kept US (schema 1): back to no value, no restart.
        let history = test_journal::history();
        let kept = history
            .entries
            .iter()
            .find(|entry| entry.state == OpState::Confirmed)
            .unwrap();
        let revert = revert_page(
            Some(kept),
            true,
            "2026/09/27 22:30",
            &name_of,
            true,
            Lang::Ja,
        );
        assert_eq!(
            revert.snapshot_text(),
            "title: 変更を元に戻す\n\
             item: 2026/09/27 22:30 Keychron Receiver を US に（このままにしました）\n\
             \x20 → 値なし（標準に従う）（変更前）に戻します (Info)\n\
             uac: 次に Windows の確認画面が出ます（発行元は「不明」）。mklm-helper.exe であることを確かめて「はい」を押してください。\n\
             method: ◉ すぐに元の配列に戻す / ○ 抜き差しか PC の再起動で戻す\n\
             buttons: [キャンセル] [元に戻す]\n"
        );
        let refused = revert_page(Some(kept), false, "", &name_of, true, Lang::Ja);
        assert!(!refused.can_recover && refused.items.is_empty());
        assert!(!refused.method.visible);
    }
}
