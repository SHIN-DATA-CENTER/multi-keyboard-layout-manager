//! Decisions taken from the journal alone, unelevated (design m2 C.7 `attention`, D.7, F.2, F.4):
//! whether a request can start, whether the post-reboot check must be registered, and which
//! entries `reboot`, the post-reboot check and `keep` deal with.
//!
//! The engine checks the same under the lock (design m2 D.1 step 3); checking first saves a UAC
//! prompt that could only end in a refusal.

use mklm_core::{
    Attention, BootId, Journal, JournalEntry, Liveness, OpId, OpKind, OpState, PendingAction,
    ProcessIdentity, UnreadableEntry, attention,
};

use crate::OutcomeClass;

/// What a request needs from the journal before it launches the helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// `set`, `migrate`: no open entry at all.
    NewOp,
    /// `restore --baseline`: open entries that are not in flight are superseded.
    Restore,
    /// `revert`, `keep`, `resolve`: interrupted entries must be recovered first.
    Existing,
    /// `recover`, `undo`: they recover interrupted entries themselves.
    Recover,
}

/// The operation a [`BlockReason`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRef {
    pub op_id: OpId,
    pub kind: OpKind,
    pub state: OpState,
}

impl OpRef {
    pub fn of(entry: &JournalEntry) -> Self {
        Self {
            op_id: entry.op_id.clone(),
            kind: entry.kind.clone(),
            state: entry.state,
        }
    }
}

/// Why a request cannot start now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// The journal has entries this build cannot read: nothing may be written until MKLM is
    /// updated (design m2 C.10).
    JournalUnreadable {
        count: usize,
        first: UnreadableEntry,
    },
    /// Another MKLM process is working on the operation (in flight, owner alive).
    Busy(OpRef),
    /// A `PendingReboot` seen from a new boot: the post-reboot check comes first.
    PostRebootCheck(OpRef),
    /// Interrupted: recover first.
    NeedsRecovery(OpRef),
    /// Waits for keep or revert (new operations only).
    AwaitingUser(OpRef),
    /// Waits for a PC restart (new operations only).
    WaitingForReboot(OpRef),
    /// In conflict (new operations only).
    Conflict(OpRef),
}

impl BlockReason {
    /// Failed for an unreadable journal, blocked otherwise.
    pub fn class(&self) -> OutcomeClass {
        match self {
            BlockReason::JournalUnreadable { .. } => OutcomeClass::Failed,
            _ => OutcomeClass::Blocked,
        }
    }

    /// The operation it is about, if any.
    pub fn op(&self) -> Option<&OpRef> {
        match self {
            BlockReason::JournalUnreadable { .. } => None,
            BlockReason::Busy(op)
            | BlockReason::PostRebootCheck(op)
            | BlockReason::NeedsRecovery(op)
            | BlockReason::AwaitingUser(op)
            | BlockReason::WaitingForReboot(op)
            | BlockReason::Conflict(op) => Some(op),
        }
    }
}

/// The first reason in `journal` that stops a request behind `gate`; `None` when it may go on.
/// `liveness` tells whether an entry's owner still runs (the unelevated `attention` needs it).
pub fn blocker(
    journal: &Journal,
    boot: BootId,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
    gate: Gate,
) -> Option<BlockReason> {
    if let Some(first) = journal.unreadable.first() {
        return Some(BlockReason::JournalUnreadable {
            count: journal.unreadable.len(),
            first: first.clone(),
        });
    }
    for entry in &journal.entries {
        let op = || OpRef::of(entry);
        match attention(entry, boot, liveness(&entry.owner)) {
            Attention::None | Attention::NeedsApply => {}
            Attention::Busy => return Some(BlockReason::Busy(op())),
            Attention::Recover if gate == Gate::Recover => {}
            Attention::Recover if entry.state == OpState::PendingReboot => {
                return Some(BlockReason::PostRebootCheck(op()));
            }
            Attention::Recover => return Some(BlockReason::NeedsRecovery(op())),
            Attention::AwaitingUser if gate == Gate::NewOp => {
                return Some(BlockReason::AwaitingUser(op()));
            }
            Attention::WaitingForReboot if gate == Gate::NewOp => {
                return Some(BlockReason::WaitingForReboot(op()));
            }
            Attention::Conflict if gate == Gate::NewOp => {
                return Some(BlockReason::Conflict(op()));
            }
            Attention::AwaitingUser | Attention::WaitingForReboot | Attention::Conflict => {}
        }
    }
    None
}

/// What to do about the post-reboot RunOnce entry after a session (design m2 F.4, review C17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOnce {
    /// Nothing waits for a restart.
    NotNeeded,
    /// Register the post-reboot check for the current user.
    Register,
    /// Needed, but this process runs elevated (an elevated console, or `--in-process`) and may
    /// belong to another administrator than the signed-in user (a standard user who typed an
    /// administrator's credentials): tell the user to run the check after the restart instead of
    /// writing another account's `RunOnce`.
    TellUser,
}

/// The rule of design m2 F.4: decided by the journal (`Journal::needs_post_reboot_check`), not by
/// the result the caller happened to receive. Only an unelevated caller registers.
pub fn run_once(journal: &Journal, boot: BootId, elevated: bool) -> RunOnce {
    if !journal.needs_post_reboot_check(boot) {
        RunOnce::NotNeeded
    } else if elevated {
        RunOnce::TellUser
    } else {
        RunOnce::Register
    }
}

/// True for an operation that takes effect through a PC restart (a migration, a PS/2 assignment,
/// a restore with boot-time values): `keep` shows the post-reboot check for it (design m2 D.6,
/// C2).
pub fn takes_effect_at_restart(entry: &JournalEntry) -> bool {
    entry.apply == Some(PendingAction::RestartPc) || entry.touches_boot_time_values()
}

/// The entries the post-reboot check asks about (design m2 D.7): `PendingReboot`, and
/// `AwaitingConfirm` of an operation that takes effect at a restart. Oldest first.
pub fn post_reboot_entries(journal: &Journal) -> Vec<&JournalEntry> {
    journal
        .open_entries()
        .into_iter()
        .filter(|entry| match entry.state {
            OpState::PendingReboot => true,
            OpState::AwaitingConfirm => takes_effect_at_restart(entry),
            _ => false,
        })
        .collect()
}

/// The entries that make a restart necessary now (design m2 F.4 `reboot`): `PendingReboot` or
/// `RevertedPendingReboot` in this boot, or an `apply_pending` restart recorded in this boot.
pub fn restart_reasons(journal: &Journal, boot: BootId) -> Vec<&JournalEntry> {
    journal
        .entries
        .iter()
        .filter(|entry| {
            let state = matches!(
                entry.state,
                OpState::PendingReboot | OpState::RevertedPendingReboot
            ) && entry.boot_id == boot;
            let pending = entry.apply_pending.as_ref().is_some_and(|pending| {
                pending.action == PendingAction::RestartPc && pending.since == boot
            });
            state || pending
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mklm_core::{
        JournalError, LayoutChoice, RegValue, Timestamp, ValueKey, ValueRecord, WriteTarget,
        fixtures, value_names,
    };

    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const BOOT: BootId = BootId(0x9b1c_0d6e_2f4a_4c8b_a1d3_5e6f_7a8b_9c0d);
    const LATER_BOOT: BootId = BootId(0x1111_2222_3333_4444_5555_6666_7777_8888);

    fn entry(state: OpState, apply: PendingAction) -> JournalEntry {
        let target = WriteTarget::Device {
            instance_id: KEYCHRON.into(),
        };
        JournalEntry {
            schema_version: 1,
            op_id: OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-000000000001").unwrap(),
            seq: 1,
            kind: OpKind::SetLayout {
                requested: KEYCHRON.into(),
                instance_ids: vec![KEYCHRON.into()],
                layout: LayoutChoice::Jis,
            },
            state,
            boot_id: BOOT,
            owner: ProcessIdentity {
                pid: 4242,
                creation_time: 1,
            },
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
            apply: Some(apply),
            countdown: None,
            records: vec![ValueRecord {
                key_path: ValueKey {
                    target: target.clone(),
                    name: value_names::HID_TYPE.into(),
                }
                .key_path(),
                target,
                name: value_names::HID_TYPE.into(),
                baseline: RegValue::Dword { value: 4 },
                before: RegValue::Dword { value: 4 },
                intended: RegValue::Dword { value: 7 },
                last_written: Some(RegValue::Dword { value: 7 }),
                conflict: None,
                resolve_to: None,
                write_error: None,
                skipped: None,
            }],
            context: Vec::new(),
            failure: None,
            revert_mode: None,
            apply_pending: None,
            history: Vec::new(),
        }
    }

    fn journal(entries: Vec<JournalEntry>) -> Journal {
        Journal {
            entries,
            ..Journal::default()
        }
    }

    #[test]
    fn typed_reasons() {
        let dead = |_: &ProcessIdentity| Liveness::Dead;
        let alive = |_: &ProcessIdentity| Liveness::Alive;
        let waiting = journal(vec![entry(
            OpState::AwaitingConfirm,
            PendingAction::Reconnect,
        )]);
        assert!(matches!(
            blocker(&waiting, BOOT, &dead, Gate::NewOp),
            Some(BlockReason::AwaitingUser(_))
        ));
        assert_eq!(blocker(&waiting, BOOT, &dead, Gate::Restore), None);
        let restart = journal(vec![entry(
            OpState::PendingReboot,
            PendingAction::RestartPc,
        )]);
        assert!(matches!(
            blocker(&restart, LATER_BOOT, &dead, Gate::Existing),
            Some(BlockReason::PostRebootCheck(_))
        ));
        let writing = journal(vec![entry(OpState::Written, PendingAction::ResetKeyboard)]);
        let busy = blocker(&writing, BOOT, &alive, Gate::Recover).unwrap();
        assert_eq!(busy.class(), OutcomeClass::Blocked);
        assert_eq!(busy.op().unwrap().state, OpState::Written);
        let mut unreadable = journal(vec![]);
        unreadable.unreadable.push(UnreadableEntry {
            name: "x".into(),
            error: JournalError::NewerSchema {
                found: 2,
                supported: 1,
            },
        });
        assert_eq!(
            blocker(&unreadable, BOOT, &dead, Gate::Recover)
                .unwrap()
                .class(),
            OutcomeClass::Failed
        );
    }

    /// The journal 0.1.0 left on the desktop PC of the boot-ID bug, read as
    /// `journal::read_journal` reads it (`journal::parse_journal`) in a boot with `boot_time`.
    fn legacy_journal(boot_time: u64) -> (Journal, BootId) {
        let (ops, baselines) = fixtures::legacy_guid_journal();
        let current = fixtures::legacy_pc_boot(boot_time);
        let read = crate::journal::parse_journal(&ops, &baselines, Some(1), Some(&current));
        (read.journal, current.id)
    }

    fn ids(entries: &[&JournalEntry]) -> Vec<String> {
        entries.iter().map(|e| e.op_id.to_string()).collect()
    }

    /// After the restart (the reported case): the migration waiting for it needs the
    /// post-reboot check, the reverted one needs nothing, and no restart is asked for.
    #[test]
    fn the_legacy_journal_after_the_restart() {
        let (journal, boot) = legacy_journal(fixtures::LEGACY_PC_BOOT_TIME_AFTER_RESTART);
        let dead = |_: &ProcessIdentity| Liveness::Dead;
        for gate in [Gate::NewOp, Gate::Restore, Gate::Existing] {
            let reason = blocker(&journal, boot, &dead, gate);
            assert!(
                matches!(&reason, Some(BlockReason::PostRebootCheck(op))
                    if op.op_id.as_str() == fixtures::LEGACY_PENDING_OP),
                "{gate:?}: {reason:?}"
            );
        }
        assert_eq!(blocker(&journal, boot, &dead, Gate::Recover), None);
        assert!(restart_reasons(&journal, boot).is_empty());
        assert_eq!(
            ids(&post_reboot_entries(&journal)),
            [fixtures::LEGACY_PENDING_OP]
        );
        // The RunOnce rule is by state: still registered until the check is done.
        assert_eq!(run_once(&journal, boot, false), RunOnce::Register);
    }

    /// The same journal in the boot of its writes (the fix installed before the restart): both
    /// entries wait for the restart.
    #[test]
    fn the_legacy_journal_before_the_restart() {
        let (journal, boot) = legacy_journal(fixtures::LEGACY_PC_BOOT_TIME_OF_WRITES);
        let dead = |_: &ProcessIdentity| Liveness::Dead;
        assert!(matches!(
            blocker(&journal, boot, &dead, Gate::NewOp),
            Some(BlockReason::WaitingForReboot(op)) if op.op_id.as_str() == fixtures::LEGACY_PENDING_OP
        ));
        assert_eq!(blocker(&journal, boot, &dead, Gate::Existing), None);
        assert_eq!(
            ids(&restart_reasons(&journal, boot)),
            [fixtures::LEGACY_REVERTED_OP, fixtures::LEGACY_PENDING_OP]
        );
        assert_eq!(run_once(&journal, boot, false), RunOnce::Register);
    }
}
