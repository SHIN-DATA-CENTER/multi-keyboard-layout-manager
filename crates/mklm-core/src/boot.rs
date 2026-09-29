//! Which boot of Windows a journal entry was written in (design review C2, docs/design/m2-engine.md
//! C.10): the current boot as the reader sees it, and how the boot IDs an entry recorded are
//! judged against it.
//!
//! Since 0.1.1 a [`BootId`] is the counter form of `KUSER_SHARED_DATA.BootId`
//! ([`BootId::from_boot_counter`]), which the OS loader increases on every boot: a restart or a
//! full shutdown. Sleep, hibernation, a Fast Startup "shutdown" and clock changes are expected to
//! keep it (design H.2 MT-2..MT-7 check this before a release). 0.1.x recorded the loader's boot
//! GUID instead, which a desktop PC kept across full restarts (docs/research/boot-id.md), so an
//! entry it wrote could wait for a restart forever.
//!
//! Recorded ids are judged once, when the journal is read (the engine under its lock, the GUI and
//! the CLI unelevated), and **in memory only**: [`Journal::adopt_current_boot`] replaces the
//! `boot_id` and `apply_pending.since` of an entry that stand for the current boot with the
//! current counter id, and leaves every other id alone, which every comparison then reads as an
//! earlier boot. An adopted entry reaches the store in the new form only when the engine writes it
//! for its own reasons. `history[].boot` is never rewritten: it is the audit trail, and the rules
//! below read it. [`JournalEntry::boot_is_current`] has two rules:
//!
//! - A **legacy** id (0.1.x, [`JournalEntry::legacy_boot_is_current`]) never equals a counter id.
//!   The `boot_time_hint` of the history lines written under it is compared with the current
//!   boot's `BootTime - BootTimeBias`: both are taken in the boot they describe, the bias absorbs
//!   every clock change, and two boots start far more than [`BOOT_TIME_TOLERANCE`] apart. Without
//!   a hinted line (M2-era entries), or without the boot time, 0.1.x's own rule applies: the GUID
//!   decides, and "not known" counts as the current boot (not restarted: the safe side).
//! - A **counter** id is the current boot when it equals the current id, and also (the safety
//!   net, [`JournalEntry::counter_boot_is_current`]) when a history line written under it carries
//!   a hint within [`BOOT_TIME_TOLERANCE`] of the current boot time: should a resume from
//!   hibernation or a Fast Startup boot ever move the counter, the unchanged boot time still says
//!   "same boot". The net only adds "same boot" answers, never "restarted" ones (C2: a false
//!   "restarted" would let the user keep a change the drivers never read).

use serde::{Deserialize, Serialize};

use crate::journal::{BootId, Journal, JournalEntry};

/// The current boot as a journal reader sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CurrentBoot {
    /// The boot ID every new write records: the counter form of `KUSER_SHARED_DATA.BootId`
    /// (`mklm_win::session::boot_id`).
    pub id: BootId,
    /// `BootTime - BootTimeBias` of this boot (FILETIME units), as
    /// [`crate::TransitionRecord::boot_time_hint`] records it; `None` when it could not be read.
    pub boot_time: Option<u64>,
    /// The loader's boot GUID (`SystemBootEnvironmentInformation.BootIdentifier`) that 0.1.x
    /// recorded as the boot ID; `None` when it could not be read. Only legacy ids are compared
    /// with it.
    pub legacy_guid: Option<BootId>,
}

/// Largest difference between a history line's `boot_time_hint` and the current boot time for
/// that line to count as written in this boot: 10 s in FILETIME units (100 ns). The value is the
/// boot's RTC second plus 0.5 s and stays put within a boot; two boots start at least tens of
/// seconds apart (sign-in, the change itself, the restart, the firmware).
pub const BOOT_TIME_TOLERANCE: u64 = 100_000_000;

impl JournalEntry {
    /// True when `id` (this entry's `boot_id` or `apply_pending.since`) stands for the current
    /// boot: [`Self::legacy_boot_is_current`] for a legacy (0.1.x) id,
    /// [`Self::counter_boot_is_current`] for a counter id.
    pub fn boot_is_current(&self, id: BootId, current: &CurrentBoot) -> bool {
        if id.is_legacy() {
            self.legacy_boot_is_current(id, current)
        } else {
            self.counter_boot_is_current(id, current)
        }
    }

    /// True when the legacy boot ID `legacy` (recorded by 0.1.x as this entry's `boot_id` or
    /// `apply_pending.since`) stands for the current boot:
    /// 1. `legacy == current.id` (only in tests);
    /// 2. else, when some history lines under `legacy` carry a `boot_time_hint` and the current
    ///    boot time is known: some hint is within [`BOOT_TIME_TOLERANCE`] of it (a write phase
    ///    under `legacy` happened in this boot);
    /// 3. else 0.1.x's own rule: the current loader GUID equals `legacy`, or it could not be read
    ///    (nothing tells, so the safe side: not restarted).
    pub fn legacy_boot_is_current(&self, legacy: BootId, current: &CurrentBoot) -> bool {
        if legacy == current.id {
            return true;
        }
        if let Some(boot_time) = current.boot_time
            && let Some(matched) = self.hinted_lines_match(legacy, boot_time)
        {
            return matched;
        }
        current.legacy_guid.is_none_or(|guid| guid == legacy)
    }

    /// True when the counter-form boot ID `id` stands for the current boot: it equals
    /// `current.id`, or (the safety net of design C.10) the current boot time is known and some
    /// history line written under `id` carries a `boot_time_hint` within [`BOOT_TIME_TOLERANCE`]
    /// of it. Without such a line, or without the boot time, the counter alone decides.
    ///
    /// The net can only turn "restarted" into "not restarted" (the safe side). It errs only when
    /// two real boots start within the tolerance of each other in RTC time before any clock
    /// change (an RTC that starts from the same value at every power-on); the entry then waits
    /// for a restart that starts at another time, and can still be undone.
    pub fn counter_boot_is_current(&self, id: BootId, current: &CurrentBoot) -> bool {
        id == current.id
            || current
                .boot_time
                .is_some_and(|boot_time| self.hinted_lines_match(id, boot_time) == Some(true))
    }

    /// Whether some history line written under `id` carries a `boot_time_hint` within
    /// [`BOOT_TIME_TOLERANCE`] of `boot_time`; `None` when no line under `id` carries a hint.
    fn hinted_lines_match(&self, id: BootId, boot_time: u64) -> Option<bool> {
        let mut hints = self
            .history
            .iter()
            .filter(|line| line.boot == id)
            .filter_map(|line| line.boot_time_hint)
            .peekable();
        hints.peek()?;
        Some(hints.any(|hint| hint.abs_diff(boot_time) <= BOOT_TIME_TOLERANCE))
    }

    /// Replaces `boot_id` and `apply_pending.since` with `current.id` where they are other ids
    /// judged to be the current boot ([`JournalEntry::boot_is_current`]). Ids of an earlier boot
    /// and `history` stay as they are; adopting twice changes nothing more. In memory only: the
    /// caller never writes the entry for this alone.
    pub fn adopt_current_boot(&mut self, current: &CurrentBoot) {
        if self.boot_id != current.id && self.boot_is_current(self.boot_id, current) {
            self.boot_id = current.id;
        }
        let since = self.apply_pending.as_ref().map(|pending| pending.since);
        if let Some(since) = since
            && since != current.id
            && self.boot_is_current(since, current)
            && let Some(pending) = &mut self.apply_pending
        {
            pending.since = current.id;
        }
    }
}

impl Journal {
    /// [`JournalEntry::adopt_current_boot`] on every entry, each judged on its own history (the
    /// same GUID may stand for different boots in different entries).
    pub fn adopt_current_boot(&mut self, current: &CurrentBoot) {
        for entry in &mut self.entries {
            entry.adopt_current_boot(current);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{
        LEGACY_PC_BOOT_COUNTER, LEGACY_PC_BOOT_TIME_AFTER_RESTART, LEGACY_PC_BOOT_TIME_OF_WRITES,
        LEGACY_PC_GUID, LEGACY_PENDING_OP, LEGACY_REVERTED_OP, legacy_guid_journal, legacy_pc_boot,
    };
    use crate::journal::{ApplyPending, OpId, OpState, RegValue, TransitionRecord};
    use crate::layout::PendingAction;
    use crate::recovery::{
        Attention, LeaveReason, RecoveryContext, RecoveryDecision, apply_pending_cleared,
        attention, decide_recovery,
    };
    use crate::test_support::*;
    use crate::{JOURNAL_SCHEMA_V1, JOURNAL_SCHEMA_VERSION, Liveness};

    const T: u64 = LEGACY_PC_BOOT_TIME_AFTER_RESTART;
    const COUNTER: BootId = BootId::from_boot_counter(LEGACY_PC_BOOT_COUNTER);

    fn live_journal() -> Journal {
        let (ops, baselines) = legacy_guid_journal();
        let journal = Journal::parse(&ops, &baselines);
        assert!(journal.unreadable.is_empty(), "{:?}", journal.unreadable);
        assert_eq!((journal.entries.len(), journal.baselines.len()), (2, 8));
        journal
    }

    fn live_entry(journal: &Journal, op: &str) -> JournalEntry {
        journal
            .entry(&OpId::parse(op).unwrap())
            .cloned()
            .unwrap_or_else(|| panic!("no {op}"))
    }

    fn intended(entry: &JournalEntry) -> Vec<RegValue> {
        entry.records.iter().map(|r| r.intended.clone()).collect()
    }

    fn context(current: &CurrentBoot) -> RecoveryContext {
        RecoveryContext {
            current_boot: current.id,
            inv_ps2: None,
        }
    }

    /// A history line under `boot` with `hint`.
    fn line(boot: BootId, hint: Option<u64>) -> TransitionRecord {
        TransitionRecord {
            from: None,
            to: OpState::Planned,
            at: crate::Timestamp(1),
            boot,
            by: OWNER,
            reason: "test".into(),
            boot_time_hint: hint,
        }
    }

    /// A pending-reboot entry with `boot_id`, `apply_pending.since` and `history` (legacy or
    /// counter ids).
    fn legacy_entry(
        boot_id: BootId,
        since: BootId,
        history: Vec<TransitionRecord>,
    ) -> JournalEntry {
        let mut e = entry(1, migrate_kind(), OpState::PendingReboot, Vec::new());
        e.boot_id = boot_id;
        e.apply = Some(PendingAction::RestartPc);
        e.apply_pending = Some(ApplyPending {
            action: PendingAction::RestartPc,
            instance_ids: vec!["HID\\X".into()],
            since,
        });
        e.history = history;
        e
    }

    /// The reported case: 0.1.0 wrote both entries under the GUID that did not change; the fix
    /// is installed after the 11:37 restart. Both are judged as written in an earlier boot.
    #[test]
    fn legacy_live_ops_after_restart() {
        let current = legacy_pc_boot(T);
        let journal = live_journal();
        let reverted = live_entry(&journal, LEGACY_REVERTED_OP);
        let pending = live_entry(&journal, LEGACY_PENDING_OP);
        assert_eq!(reverted.state, OpState::RevertedPendingReboot);
        assert_eq!(pending.state, OpState::PendingReboot);
        for entry in [&reverted, &pending] {
            assert_eq!(entry.boot_id, LEGACY_PC_GUID);
            assert!(entry.boot_id.is_legacy());
            assert!(!entry.legacy_boot_is_current(LEGACY_PC_GUID, &current));
        }
        let since = pending.apply_pending.as_ref().unwrap().since;
        assert_eq!(since, LEGACY_PC_GUID);
        assert!(!pending.legacy_boot_is_current(since, &current));

        let mut adopted = journal.clone();
        adopted.adopt_current_boot(&current);
        assert_eq!(adopted, journal, "nothing is the current boot");

        assert_eq!(
            attention(&pending, current.id, Liveness::Dead),
            Attention::Recover
        );
        assert_eq!(
            attention(&reverted, current.id, Liveness::Dead),
            Attention::None
        );
        assert_eq!(
            decide_recovery(&pending, &intended(&pending), &context(&current)),
            RecoveryDecision::RebootObserved {
                to: OpState::AwaitingConfirm
            }
        );
        assert_eq!(
            decide_recovery(&reverted, &intended(&reverted), &context(&current)),
            RecoveryDecision::RebootObserved {
                to: OpState::Reverted
            }
        );
        assert!(apply_pending_cleared(
            pending.apply_pending.as_ref().unwrap(),
            current.id,
            &|_| false
        ));
    }

    /// The same entries if the fix had been installed before the 11:37 restart: both were
    /// written in this boot, so they are adopted and keep waiting for the restart.
    #[test]
    fn legacy_live_ops_same_boot() {
        let current = legacy_pc_boot(LEGACY_PC_BOOT_TIME_OF_WRITES);
        let mut journal = live_journal();
        journal.adopt_current_boot(&current);
        let reverted = live_entry(&journal, LEGACY_REVERTED_OP);
        let pending = live_entry(&journal, LEGACY_PENDING_OP);
        assert_eq!(reverted.boot_id, COUNTER);
        assert_eq!(pending.boot_id, COUNTER);
        assert_eq!(pending.apply_pending.as_ref().unwrap().since, COUNTER);
        assert_eq!(COUNTER.to_text(), "00000007-0000-8000-8000-000000000000");
        assert_eq!(
            attention(&pending, current.id, Liveness::Dead),
            Attention::WaitingForReboot
        );
        assert_eq!(
            attention(&reverted, current.id, Liveness::Dead),
            Attention::NeedsApply
        );
        for entry in [&pending, &reverted] {
            assert_eq!(
                decide_recovery(entry, &intended(entry), &context(&current)),
                RecoveryDecision::Leave {
                    reason: LeaveReason::WaitingForReboot
                },
                "{}",
                entry.op_id
            );
        }
        assert!(!apply_pending_cleared(
            pending.apply_pending.as_ref().unwrap(),
            current.id,
            &|_| true
        ));
    }

    #[test]
    fn the_tolerance_is_ten_seconds_either_way() {
        let legacy = boot(0x1234);
        let judge = |hint: u64, boot_time: u64| {
            let e = legacy_entry(legacy, legacy, vec![line(legacy, Some(hint))]);
            let current = CurrentBoot {
                id: BootId::from_boot_counter(3),
                boot_time: Some(boot_time),
                // A GUID that says "another boot": only the hint decides.
                legacy_guid: Some(boot(0x9999)),
            };
            e.legacy_boot_is_current(legacy, &current)
        };
        assert_eq!(BOOT_TIME_TOLERANCE, 100_000_000);
        assert!(judge(T, T));
        assert!(judge(T + 100_000_000, T));
        assert!(judge(T - 100_000_000, T));
        assert!(!judge(T + 100_000_001, T));
        assert!(!judge(T - 100_000_001, T));
        // No overflow at either end.
        assert!(judge(0, 100_000_000));
        assert!(!judge(0, 100_000_001));
        assert!(judge(100_000_000, 0));
        assert!(judge(u64::MAX, u64::MAX - 100_000_000));
        assert!(!judge(u64::MAX, u64::MAX - 100_000_001));
        assert!(!judge(0, u64::MAX));
        assert!(!judge(u64::MAX, 0));
    }

    /// Without a hinted line under the legacy id, or without the boot time, 0.1.x's rule: the
    /// GUID decides, and an unknown GUID counts as this boot (not restarted).
    #[test]
    fn without_hints_the_guid_decides() {
        let legacy = boot(0x4c70_3377);
        let other = boot(0x5555);
        let counter = BootId::from_boot_counter(9);
        let current = |boot_time: Option<u64>, guid: Option<BootId>| CurrentBoot {
            id: counter,
            boot_time,
            legacy_guid: guid,
        };
        // No hint at all (an M2-era entry), a hint only under another boot, or engine take-over
        // lines without a hint.
        let histories = [
            vec![line(legacy, None), line(legacy, None)],
            vec![line(other, Some(T)), line(legacy, None)],
            Vec::new(),
        ];
        for history in histories {
            let e = legacy_entry(legacy, legacy, history.clone());
            for boot_time in [Some(T), None] {
                assert!(
                    e.legacy_boot_is_current(legacy, &current(boot_time, Some(legacy))),
                    "{history:?}"
                );
                assert!(
                    !e.legacy_boot_is_current(legacy, &current(boot_time, Some(other))),
                    "{history:?}"
                );
                assert!(
                    e.legacy_boot_is_current(legacy, &current(boot_time, None)),
                    "{history:?}"
                );
            }
        }
        // With hints but no boot time, the same three answers.
        let hinted = legacy_entry(
            legacy,
            legacy,
            vec![line(legacy, Some(T + 999_000_000_000))],
        );
        assert!(hinted.legacy_boot_is_current(legacy, &current(None, Some(legacy))));
        assert!(!hinted.legacy_boot_is_current(legacy, &current(None, Some(other))));
        assert!(hinted.legacy_boot_is_current(legacy, &current(None, None)));
        // With the boot time, the hints win over the GUID either way.
        assert!(!hinted.legacy_boot_is_current(legacy, &current(Some(T), Some(legacy))));
        let near = legacy_entry(legacy, legacy, vec![line(legacy, Some(T + 1))]);
        assert!(near.legacy_boot_is_current(legacy, &current(Some(T), Some(other))));
        // Rule 1: an id equal to the current one is this boot.
        assert!(near.legacy_boot_is_current(counter, &current(Some(0), Some(other))));
    }

    /// The disconnected M2 entry of the GUI's fixtures (no hints at all) follows the GUID rule.
    #[test]
    fn an_m2_entry_without_hints_follows_the_guid() {
        let (ops, baselines) = crate::fixtures::schema_1_journal();
        let journal = Journal::parse(&ops, &baselines);
        // Every schema-1 fixture entry carries hints; strip them to get an M2-era document.
        let mut e = journal.entries[0].clone();
        for line in &mut e.history {
            line.boot_time_hint = None;
        }
        let legacy = e.boot_id;
        assert!(legacy.is_legacy());
        let current = |guid| CurrentBoot {
            id: BootId::from_boot_counter(2),
            boot_time: Some(T),
            legacy_guid: guid,
        };
        let mut same = e.clone();
        same.adopt_current_boot(&current(Some(legacy)));
        assert_eq!(same.boot_id, BootId::from_boot_counter(2));
        let mut earlier = e.clone();
        earlier.adopt_current_boot(&current(Some(LEGACY_PC_GUID)));
        assert_eq!(earlier, e);
        let mut unknown = e.clone();
        unknown.adopt_current_boot(&current(None));
        assert_eq!(unknown.boot_id, BootId::from_boot_counter(2));
    }

    /// `boot_id` and `apply_pending.since` are judged separately, and so is every entry.
    #[test]
    fn fields_and_entries_are_judged_on_their_own() {
        let (l1, l2) = (boot(0x1111), boot(0x2222));
        let current = CurrentBoot {
            id: COUNTER,
            boot_time: Some(T),
            legacy_guid: Some(LEGACY_PC_GUID),
        };
        let far = T - 3_000_000_000;
        // boot_id far, since current.
        let mut a = legacy_entry(l1, l2, vec![line(l1, Some(far)), line(l2, Some(T))]);
        a.adopt_current_boot(&current);
        assert_eq!(a.boot_id, l1);
        assert_eq!(a.apply_pending.as_ref().unwrap().since, COUNTER);
        // The reverse.
        let mut b = legacy_entry(l1, l2, vec![line(l1, Some(T)), line(l2, Some(far))]);
        b.adopt_current_boot(&current);
        assert_eq!(b.boot_id, COUNTER);
        assert_eq!(b.apply_pending.as_ref().unwrap().since, l2);

        // The same GUID in two entries: A from an earlier boot, B from this one.
        let g = LEGACY_PC_GUID;
        let mut earlier = legacy_entry(g, g, vec![line(g, Some(far))]);
        earlier.op_id = op_id(1);
        let mut now = legacy_entry(g, g, vec![line(g, Some(far)), line(g, Some(T - 5))]);
        now.op_id = op_id(2);
        now.seq = 2;
        let mut journal = Journal {
            entries: vec![earlier.clone(), now.clone()],
            ..Journal::default()
        };
        journal.adopt_current_boot(&current);
        assert_eq!(journal.entries[0], earlier);
        assert_eq!(journal.entries[1].boot_id, COUNTER);
        assert_eq!(
            journal.entries[1].apply_pending.as_ref().unwrap().since,
            COUNTER
        );
        assert_eq!(journal.entries[1].history, now.history);
    }

    #[test]
    fn adoption_keeps_history_counter_ids_and_the_stored_form() {
        let current = legacy_pc_boot(LEGACY_PC_BOOT_TIME_OF_WRITES);
        let journal = live_journal();
        let mut once = journal.clone();
        once.adopt_current_boot(&current);
        assert_ne!(once, journal);
        for (before, after) in journal.entries.iter().zip(&once.entries) {
            assert_eq!(after.history, before.history, "history is never rewritten");
            assert!(after.history.iter().all(|h| h.boot == LEGACY_PC_GUID));
        }
        let mut twice = once.clone();
        twice.adopt_current_boot(&current);
        assert_eq!(twice, once, "idempotent");
        // Also when judged from a later boot: the adopted counter id has no line under it, so it
        // stays (an earlier boot).
        let mut later = once.clone();
        later.adopt_current_boot(&CurrentBoot {
            id: BootId::from_boot_counter(8),
            boot_time: Some(T + 3_000_000_000),
            legacy_guid: Some(LEGACY_PC_GUID),
        });
        assert_eq!(later, once);

        // A journal without legacy ids is unchanged.
        let mut modern = once.clone();
        for entry in &mut modern.entries {
            entry.history.clear();
        }
        let copy = modern.clone();
        modern.adopt_current_boot(&legacy_pc_boot(T));
        assert_eq!(modern, copy);

        // The stored form: schema and field order as before, and it reads back.
        let (ops, _) = legacy_guid_journal();
        for entry in &once.entries {
            let json = entry.to_json().unwrap();
            assert!(
                json.starts_with("{\"schema_version\":1,\"op_id\":"),
                "{json}"
            );
            let stored = &ops
                .iter()
                .find(|(name, _)| name == entry.op_id.as_str())
                .unwrap()
                .1;
            let guid = "\"9845bda6-baa7-11f1-adca-ca988d513a4f\"";
            assert_eq!(
                json.replace("\"00000007-0000-8000-8000-000000000000\"", guid),
                *stored,
                "only the adopted boot ids differ"
            );
            assert!(
                json.contains("\"boot_id\":\"00000007-0000-8000-8000-000000000000\""),
                "{json}"
            );
            assert_eq!(JournalEntry::from_json(&json).unwrap(), *entry);
        }
        // A cleanup keeps schema 2.
        let mut cleanup = entry(3, cleanup_kind("ACPI\\X"), OpState::AwaitingConfirm, vec![]);
        cleanup.boot_id = LEGACY_PC_GUID;
        cleanup.history = vec![line(LEGACY_PC_GUID, Some(T))];
        cleanup.adopt_current_boot(&legacy_pc_boot(T));
        assert_eq!(cleanup.boot_id, COUNTER);
        let json = cleanup.to_json().unwrap();
        assert!(json.starts_with(&format!("{{\"schema_version\":{JOURNAL_SCHEMA_VERSION},")));
        assert_eq!(JournalEntry::from_json(&json).unwrap(), cleanup);
        assert_eq!(JOURNAL_SCHEMA_V1, 1);
    }

    /// The safety net of the counter (design C.10): a counter id other than the current one is
    /// the current boot when a line written under it carries this boot's time (the counter
    /// moved without a new boot); otherwise the counter decides. It only adds "same boot".
    #[test]
    fn a_counter_id_with_this_boot_time_is_the_current_boot() {
        let (n, next) = (BootId::from_boot_counter(7), BootId::from_boot_counter(8));
        let current = |boot_time| CurrentBoot {
            id: next,
            boot_time,
            legacy_guid: Some(LEGACY_PC_GUID),
        };
        // Two minutes later: a real restart.
        let far = T + 1_200_000_000;
        let written = legacy_entry(n, n, vec![line(n, Some(T)), line(n, None)]);

        // Resumed, or started from Fast Startup, with a counter that moved: the same boot time.
        for boot_time in [T, T + BOOT_TIME_TOLERANCE, T - BOOT_TIME_TOLERANCE] {
            assert!(written.counter_boot_is_current(n, &current(Some(boot_time))));
            assert!(written.boot_is_current(n, &current(Some(boot_time))));
        }
        // A real restart, no boot time, or no hinted line under the id: the counter decides.
        assert!(!written.counter_boot_is_current(n, &current(Some(far))));
        assert!(!written.counter_boot_is_current(n, &current(Some(T + BOOT_TIME_TOLERANCE + 1))));
        assert!(!written.counter_boot_is_current(n, &current(None)));
        let unhinted = legacy_entry(n, n, vec![line(n, None), line(next, Some(T))]);
        assert!(!unhinted.counter_boot_is_current(n, &current(Some(T))));
        assert!(!legacy_entry(n, n, Vec::new()).counter_boot_is_current(n, &current(Some(T))));
        // A counter that went back (bootstat.dat recreated) with another boot time: restarted.
        let nine = BootId::from_boot_counter(9);
        let back = legacy_entry(nine, nine, vec![line(nine, Some(far))]);
        assert!(!back.boot_is_current(nine, &current(Some(T))));
        // The current id is always this boot, whatever the time says.
        assert!(written.counter_boot_is_current(next, &current(Some(far))));
        assert!(written.counter_boot_is_current(next, &current(None)));
        // The GUID plays no part for a counter id.
        for guid in [None, Some(n), Some(next)] {
            let judged = CurrentBoot {
                legacy_guid: guid,
                ..current(Some(far))
            };
            assert!(!written.boot_is_current(n, &judged), "{guid:?}");
        }

        // Adopted in memory like a legacy id: both fields, history untouched, idempotent.
        let now = current(Some(T + 5));
        let mut adopted = written.clone();
        adopted.adopt_current_boot(&now);
        assert_eq!(adopted.boot_id, next);
        assert_eq!(adopted.apply_pending.as_ref().unwrap().since, next);
        assert_eq!(adopted.history, written.history);
        let mut twice = adopted.clone();
        twice.adopt_current_boot(&now);
        assert_eq!(twice, adopted);
        let mut restarted = written.clone();
        restarted.adopt_current_boot(&current(Some(far)));
        assert_eq!(restarted, written);

        // What every caller then decides: the entry still waits for the restart. Without the net
        // the same entry would read as restarted.
        assert_eq!(
            attention(&adopted, now.id, Liveness::Dead),
            Attention::WaitingForReboot
        );
        assert_eq!(
            decide_recovery(&adopted, &intended(&adopted), &context(&now)),
            RecoveryDecision::Leave {
                reason: LeaveReason::WaitingForReboot
            }
        );
        assert!(!apply_pending_cleared(
            adopted.apply_pending.as_ref().unwrap(),
            now.id,
            &|_| true
        ));
        assert_eq!(
            attention(&written, now.id, Liveness::Dead),
            Attention::Recover
        );
    }

    /// `boot_id` and `apply_pending.since` of either form are judged on their own.
    #[test]
    fn counter_and_legacy_fields_are_judged_on_their_own() {
        let (n, next) = (BootId::from_boot_counter(3), BootId::from_boot_counter(4));
        let current = CurrentBoot {
            id: next,
            boot_time: Some(T),
            legacy_guid: None,
        };
        let far = T - 3_000_000_000;
        // A legacy boot_id of an earlier boot, a counter since of this boot time.
        let mut a = legacy_entry(
            LEGACY_PC_GUID,
            n,
            vec![line(LEGACY_PC_GUID, Some(far)), line(n, Some(T))],
        );
        a.adopt_current_boot(&current);
        assert_eq!(a.boot_id, LEGACY_PC_GUID);
        assert_eq!(a.apply_pending.as_ref().unwrap().since, next);
        // The reverse.
        let mut b = legacy_entry(
            n,
            LEGACY_PC_GUID,
            vec![line(n, Some(far)), line(LEGACY_PC_GUID, Some(T))],
        );
        b.adopt_current_boot(&current);
        assert_eq!(b.boot_id, n);
        assert_eq!(b.apply_pending.as_ref().unwrap().since, next);
    }
}
