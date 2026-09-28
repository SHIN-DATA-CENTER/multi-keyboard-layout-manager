//! The history (design m3 B.11): journal entries newest first, in local time (design m3 G.1),
//! with the state in words (`i18n::entry_state`) and the reason worded by whether the reset was
//! reached (design m3 G.2). "元に戻す…" is offered only where the helper would accept it: an
//! entry that waits for keep or revert, or a kept change that is not a restore to before MKLM,
//! whose values no later operation changed (design m2 C.8, D.4), and only while nothing
//! interrupted or busy blocks a revert (`gate::Gate::Existing`).

use mklm_client::describe::reset_phase;
use mklm_client::startup::StartupSummary;
use mklm_core::{
    Attention, FailureReason, Journal, JournalEntry, OpKind, OpState, Timestamp, value_eq,
};

use super::{SnapshotText, Tone};
use crate::i18n::{self, Lang, journal_pages as text};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JournalRow {
    /// The short operation ID (8 hex digits).
    pub op: String,
    /// The full operation ID (what the revert button passes back).
    pub op_id: String,
    pub when: String,
    pub what: String,
    pub state: String,
    pub reason: String,
    pub tone: Tone,
    pub can_revert: bool,
    /// The revert button's accessible label ("2026/09/27 22:25 の変更を元に戻す", design m3 E.2).
    pub revert_label: String,
}

/// The history page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JournalPage {
    pub rows: Vec<JournalRow>,
    /// "まだ履歴はありません。" when there are no rows.
    pub empty_text: String,
    /// Entries of a newer MKLM: "MKLM を更新してください" first (design m2 C.10).
    pub notice: String,
    /// Why no revert is offered now (an interrupted or busy entry); empty otherwise.
    pub blocked_note: String,
}

/// Formats a timestamp for display: the GUI passes `mklm_win::time::local_time` (local time with
/// the offset); tests pass a fixed formatter.
pub type TimeText<'a> = &'a dyn Fn(Timestamp) -> String;

/// The display name of a keyboard by instance ID (from the snapshot; the ID when unknown).
pub type NameOf<'a> = &'a dyn Fn(&str) -> String;

/// What an operation was for, in one line (`i18n::journal_pages::operation`).
pub fn kind_text(kind: &OpKind, name_of: NameOf<'_>, lang: Lang) -> String {
    text::operation(kind, name_of, lang)
}

/// The UTC date and time of a timestamp (the fallback when the local time cannot be computed):
/// `(year, month, day, hour, minute)`.
pub fn utc_parts(at: Timestamp) -> (u16, u16, u16, u16, u16) {
    let seconds = at.0 / 1000;
    let days = seconds / 86_400;
    let of_day = seconds % 86_400;
    // Howard Hinnant's civil_from_days, for days since 1970-01-01 (never negative here).
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let clamp = |value: i64| u16::try_from(value).unwrap_or(u16::MAX);
    (
        clamp(year),
        clamp(month),
        clamp(day),
        clamp((of_day / 3600) as i64),
        clamp((of_day % 3600 / 60) as i64),
    )
}

/// "2026/09/27 13:36 UTC": the history's time when the local time is not available.
pub fn utc_time_text(at: Timestamp, lang: Lang) -> String {
    text::history_time(utc_parts(at), true, lang)
}

/// Only the operation that last changed a value may put it back (design m2 C.8; the engine's
/// `check_latest`, which decides again under its lock): a later operation that wrote one of this
/// entry's values counts, unless it was itself put back and left the value where this one had
/// left it.
pub fn is_latest(journal: &Journal, entry: &JournalEntry) -> bool {
    let this = (entry.seq, &entry.op_id);
    entry.records.iter().all(|record| {
        let Some(ours) = &record.last_written else {
            return true;
        };
        let key = record.key().canonical();
        !journal
            .entries
            .iter()
            .filter(|later| later.op_id != entry.op_id && (later.seq, &later.op_id) > this)
            .any(|later| {
                later
                    .records
                    .iter()
                    .filter(|r| r.key().canonical().eq_ignore_ascii_case(&key))
                    .filter_map(|r| r.last_written.as_ref().map(|written| (r, written)))
                    .any(|(r, written)| {
                        let put_back = matches!(
                            later.state,
                            OpState::Reverted | OpState::RevertedPendingReboot | OpState::Failed
                        );
                        !(put_back && value_eq(&r.name, written, ours))
                    })
            })
    })
}

/// The helper would revert `entry` (design m2 D.4): it waits for keep or revert, or it is a kept
/// change other than a restore to before MKLM (design review C6), and it is the latest on its
/// values.
pub fn revertible(journal: &Journal, entry: &JournalEntry) -> bool {
    let state = match entry.state {
        OpState::AwaitingConfirm => entry.countdown.is_none(),
        OpState::PendingReboot => true,
        OpState::Confirmed => !matches!(entry.kind, OpKind::RestoreBaseline { .. }),
        _ => false,
    };
    state && is_latest(journal, entry)
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
    entries.sort_by(|a, b| (b.seq, &b.op_id).cmp(&(a.seq, &a.op_id)));
    entries
        .into_iter()
        .map(|entry| {
            let when = time_text(entry.created_at);
            JournalRow {
                op: entry.op_id.short().to_string(),
                op_id: entry.op_id.to_string(),
                what: kind_text(&entry.kind, name_of, lang),
                state: i18n::entry_state(
                    entry.state,
                    entry.failure.as_ref(),
                    attention(entry),
                    lang,
                ),
                reason: entry
                    .failure
                    .as_ref()
                    .map(|failure| i18n::failure(failure, reset_phase(entry), lang))
                    .unwrap_or_default(),
                tone: tone(entry),
                can_revert: revertible(entry),
                revert_label: text::history_revert_label(&when, lang),
                when,
            }
        })
        .collect()
}

/// The history page from the last read (`journal` is `None` when it could not be read).
pub fn journal_page(
    journal: Option<&Journal>,
    summary: &StartupSummary,
    time_text: TimeText<'_>,
    name_of: NameOf<'_>,
    lang: Lang,
) -> JournalPage {
    let busy = summary.with(Attention::Busy).next().is_some();
    // The helper's gate for existing operations: an interrupted entry stops a revert, a
    // `PendingReboot` seen from a new boot does not (its own check decides it).
    let interrupted = summary
        .with(Attention::Recover)
        .any(|item| item.op.state != OpState::PendingReboot);
    let blocked = summary.unreadable > 0 || interrupted || busy;
    let attention = |entry: &JournalEntry| {
        summary
            .items
            .iter()
            .find(|item| item.op.op_id == entry.op_id)
            .map_or(Attention::None, |item| item.attention)
    };
    let rows = journal.map_or_else(Vec::new, |journal| {
        journal_rows(
            journal,
            time_text,
            name_of,
            &|entry| !blocked && revertible(journal, entry),
            &attention,
            lang,
        )
    });
    let offered = journal.is_some_and(|journal| {
        journal
            .entries
            .iter()
            .any(|entry| revertible(journal, entry))
    });
    JournalPage {
        empty_text: if rows.is_empty() {
            text::history_empty(lang)
        } else {
            String::new()
        },
        rows,
        notice: if summary.unreadable > 0 {
            text::history_unreadable(summary.unreadable, lang)
        } else {
            String::new()
        },
        // Said only when it hides a button the user would otherwise see.
        blocked_note: if blocked && summary.unreadable == 0 && offered {
            text::history_blocked(busy, lang)
        } else {
            String::new()
        },
    }
}

impl SnapshotText for JournalPage {
    fn snapshot_text(&self) -> String {
        let mut out = String::new();
        for (label, value) in [
            ("notice", &self.notice),
            ("blocked", &self.blocked_note),
            ("empty", &self.empty_text),
        ] {
            if !value.is_empty() {
                out.push_str(&format!("{label}: {value}\n"));
            }
        }
        for row in &self.rows {
            out.push_str(&format!(
                "{} {} | {} | {} ({:?}){}{}\n",
                row.op,
                row.when,
                row.what,
                row.state,
                row.tone,
                if row.reason.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", row.reason)
                },
                if row.can_revert {
                    format!(" [{}]", row.revert_label)
                } else {
                    String::new()
                }
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use mklm_client::startup::summarize;
    use mklm_core::{BootId, Liveness, ProcessIdentity};

    use super::*;
    use crate::vm::test_journal::{self, KEYCHRON, entry};

    fn fixed_time(at: Timestamp) -> String {
        text::history_time(utc_parts(at), false, Lang::Ja)
    }

    fn name_of(id: &str) -> String {
        if id.eq_ignore_ascii_case(KEYCHRON) {
            "Keychron Receiver".into()
        } else {
            id.into()
        }
    }

    fn page(journal: &Journal, boot: BootId, lang: Lang) -> JournalPage {
        let summary = summarize(journal, boot, &|_: &ProcessIdentity| Liveness::Dead);
        let time = |at: Timestamp| text::history_time(utc_parts(at), false, lang);
        journal_page(Some(journal), &summary, &time, &name_of, lang)
    }

    #[test]
    fn utc_parts_of_known_instants() {
        assert_eq!(utc_parts(Timestamp(0)), (1970, 1, 1, 0, 0));
        // 2026-09-27 13:36:00 UTC (22:36 in Japan), the R4 history example.
        assert_eq!(
            utc_parts(Timestamp(1_790_516_160_000)),
            (2026, 9, 27, 13, 36)
        );
        // A leap day.
        assert_eq!(utc_parts(Timestamp(951_782_400_000)), (2000, 2, 29, 0, 0));
        assert_eq!(
            utc_time_text(Timestamp(1_790_516_160_000), Lang::En),
            "2026-09-27 13:36 UTC"
        );
        assert_eq!(fixed_time(Timestamp(1_790_516_160_000)), "2026/09/27 13:36");
    }

    #[test]
    fn the_history_in_both_languages() {
        let journal = test_journal::history();
        let ja = page(&journal, test_journal::BOOT, Lang::Ja);
        assert_eq!(
            ja.snapshot_text(),
            "8e9a9970 2026/09/27 13:31 | Keychron Receiver を JIS に | 元に戻しました (Warning) \
             — キーボードをリセットする前に止まったため、回復で元に戻しました（キーボードの動作は変わっていません）\n\
             3f2a9c1e 2026/09/27 13:30 | Keychron Receiver を US に | このままにしました (Success) \
             [2026/09/27 13:30 の変更を元に戻す]\n\
             1ef48b2f 2026/09/27 13:20 | Keychron Receiver を JIS に | 元に戻しました (Warning) \
             — 確定する前に MKLM が終了したため、元に戻しました\n"
        );
        for row in &ja.rows {
            for text in [&row.what, &row.state, &row.reason, &row.revert_label] {
                assert!(
                    crate::vm::unexpected_latin(text, &["Keychron Receiver"]).is_empty(),
                    "{text}"
                );
            }
        }
        let en = page(&journal, test_journal::BOOT, Lang::En);
        assert_eq!(en.rows[1].what, "Keychron Receiver to US");
        assert_eq!(en.rows[1].state, "Kept");
        assert_eq!(
            en.rows[1].revert_label,
            "Revert the change of 2026-09-27 13:30"
        );
        assert_eq!(en.rows[0].op_id, "8e9a9970-f7bf-46c7-b779-f914f17bd40d");
    }

    #[test]
    fn revert_is_offered_only_for_the_latest_change() {
        let mut journal = test_journal::history();
        // A later confirmed change of the same values: the earlier one is not the latest.
        let mut later = entry(
            "5c5c5c5c-0000-4000-8000-000000000009",
            9,
            OpState::Confirmed,
        );
        later.created_at = Timestamp(1_790_520_000_000);
        journal.entries.push(later);
        let rows = page(&journal, test_journal::BOOT, Lang::Ja).rows;
        let revertible: Vec<&str> = rows
            .iter()
            .filter(|row| row.can_revert)
            .map(|row| row.op.as_str())
            .collect();
        assert_eq!(revertible, ["5c5c5c5c"]);
        // A later change that was itself put back where this one left the values does not count
        // (design m2 C.8): the R5 entry (seq 7, reverted to 4/0) does not hide the kept US.
        let history = test_journal::history();
        let kept = history
            .entries
            .iter()
            .find(|entry| entry.state == OpState::Confirmed)
            .unwrap();
        assert!(is_latest(&history, kept));
    }

    #[test]
    fn nothing_is_revertible_while_an_entry_needs_recovery() {
        let mut journal = test_journal::history();
        journal.entries.push(entry(
            "6d6d6d6d-0000-4000-8000-00000000000a",
            10,
            OpState::Planned,
        ));
        let ja = page(&journal, test_journal::BOOT, Lang::Ja);
        assert!(ja.rows.iter().all(|row| !row.can_revert));
        assert_eq!(
            ja.blocked_note,
            "途中で止まった操作があるため、今は元に戻せません。先に回復してください。"
        );
        assert_eq!(ja.rows[0].state, "途中で止まりました（回復が必要）");
    }

    #[test]
    fn unreadable_entries_and_an_empty_journal() {
        let mut journal = Journal::default();
        let empty = page(&journal, test_journal::BOOT, Lang::Ja);
        assert_eq!(empty.snapshot_text(), "empty: まだ履歴はありません。\n");
        journal.unreadable.push(mklm_core::UnreadableEntry {
            name: "x".into(),
            error: mklm_core::JournalError::NewerSchema {
                found: 3,
                supported: 2,
            },
        });
        let ja = page(&journal, test_journal::BOOT, Lang::Ja);
        assert!(
            ja.notice.contains("MKLM を更新してください"),
            "{}",
            ja.notice
        );
        assert_eq!(ja.blocked_note, "");
    }
}
