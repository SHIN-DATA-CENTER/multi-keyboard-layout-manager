//! Decisions the CLI takes from the journal alone, unelevated (design C.7 `attention`, D.7, F.2,
//! F.4): whether a command can start, whether the post-reboot check must be registered, and which
//! entries `reboot`, `post-reboot` and `keep` deal with.

use mklm_core::{
    Attention, BootId, Journal, JournalEntry, Liveness, OpState, PendingAction, ProcessIdentity,
    attention,
};

use super::exit_code;
use super::render::kind_text;

/// What a command needs from the journal before it launches the helper (the engine checks the
/// same under the lock, design D.1 step 3; checking first saves a UAC prompt that could only end
/// in a refusal).
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

/// Why a command cannot start now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub exit_code: i32,
    pub message: String,
}

/// The first reason in `journal` that stops a command behind `gate`; `None` when it may go on.
/// `liveness` tells whether an entry's owner still runs (the unelevated `attention` needs it).
pub fn blocker(
    journal: &Journal,
    boot: BootId,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
    gate: Gate,
) -> Option<Blocker> {
    if let Some(bad) = journal.unreadable.first() {
        return Some(Blocker {
            exit_code: exit_code::FAILURE,
            message: format!(
                "the journal has {} entr{} this MKLM cannot read ({}: {}); nothing may be \
                 written until MKLM is updated",
                journal.unreadable.len(),
                if journal.unreadable.len() == 1 {
                    "y"
                } else {
                    "ies"
                },
                bad.name,
                bad.error
            ),
        });
    }
    for entry in &journal.entries {
        let blocked = |message: String| {
            Some(Blocker {
                exit_code: exit_code::BLOCKED,
                message,
            })
        };
        let op = entry.op_id.short();
        let what = kind_text(&entry.kind);
        match attention(entry, boot, liveness(&entry.owner)) {
            Attention::None | Attention::NeedsApply => {}
            Attention::Busy => {
                return blocked(format!(
                    "another MKLM process is working on operation {op} ({what}); try again when \
                     it has finished"
                ));
            }
            Attention::Recover if gate == Gate::Recover => {}
            Attention::Recover if entry.state == OpState::PendingReboot => {
                return blocked(format!(
                    "operation {op} ({what}) waits for the check after the restart; run \
                     `mklm-cli post-reboot` (or `mklm-cli undo`)"
                ));
            }
            Attention::Recover => {
                return blocked(format!(
                    "operation {op} ({what}) was interrupted; run `mklm-cli recover` first"
                ));
            }
            Attention::AwaitingUser if gate == Gate::NewOp => {
                return blocked(format!(
                    "operation {op} ({what}) waits for keep or revert; run `mklm-cli keep {op}` \
                     or `mklm-cli revert {op}` (or `mklm-cli undo`)"
                ));
            }
            Attention::WaitingForReboot if gate == Gate::NewOp => {
                return blocked(format!(
                    "operation {op} ({what}) waits for a PC restart; run `mklm-cli reboot` (or \
                     undo it with `mklm-cli undo`)"
                ));
            }
            Attention::Conflict if gate == Gate::NewOp => {
                return blocked(format!(
                    "operation {op} ({what}) is in conflict; run `mklm-cli resolve {op}` (or \
                     `mklm-cli undo`)"
                ));
            }
            Attention::AwaitingUser | Attention::WaitingForReboot | Attention::Conflict => {}
        }
    }
    None
}

/// What to do about the post-reboot RunOnce entry after a session (design F.4, review C17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOnce {
    /// Nothing waits for a restart.
    NotNeeded,
    /// Register `mklm-cli post-reboot` for the current user.
    Register,
    /// Needed, but this process runs elevated in-process and may be another administrator's:
    /// tell the user to run `mklm-cli post-reboot` after the restart instead.
    TellUser,
}

/// The rule of design F.4: decided by the journal (`Journal::needs_post_reboot_check`), not by the
/// result the caller happened to receive.
pub fn run_once(journal: &Journal, boot: BootId, in_process: bool) -> RunOnce {
    if !journal.needs_post_reboot_check(boot) {
        RunOnce::NotNeeded
    } else if in_process {
        RunOnce::TellUser
    } else {
        RunOnce::Register
    }
}

/// True for an operation that takes effect through a PC restart (a migration, a PS/2 assignment,
/// a restore with boot-time values): `keep` shows the post-reboot check for it (design D.6, C2).
pub fn takes_effect_at_restart(entry: &JournalEntry) -> bool {
    entry.apply == Some(PendingAction::RestartPc) || entry.touches_boot_time_values()
}

/// The entries `post-reboot` asks about (design D.7): `PendingReboot`, and `AwaitingConfirm` of an
/// operation that takes effect at a restart. Oldest first.
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

/// The entries that make a restart necessary now (design F.4 `reboot`): `PendingReboot` or
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
        ApplyPending, Countdown, JournalError, LayoutChoice, OpId, OpKind, RegValue, Timestamp,
        UnreadableEntry, ValueRecord, WriteTarget, value_names,
    };

    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const PS2: &str = r"ACPI\FUJ0309\4&320DB4C2&0";
    const BOOT: BootId = BootId(0x9b1c_0d6e_2f4a_4c8b_a1d3_5e6f_7a8b_9c0d);
    const LATER_BOOT: BootId = BootId(0x1111_2222_3333_4444_5555_6666_7777_8888);
    const OWNER: ProcessIdentity = ProcessIdentity {
        pid: 4242,
        creation_time: 134_036_790_000_000_000,
    };

    fn record(target: WriteTarget, name: &str) -> ValueRecord {
        let key_path = mklm_core::ValueKey {
            target: target.clone(),
            name: name.into(),
        }
        .key_path();
        ValueRecord {
            target,
            key_path,
            name: name.into(),
            baseline: RegValue::Dword { value: 4 },
            before: RegValue::Dword { value: 4 },
            intended: RegValue::Dword { value: 7 },
            last_written: Some(RegValue::Dword { value: 7 }),
            conflict: None,
            resolve_to: None,
            write_error: None,
            skipped: None,
        }
    }

    fn entry(seq: u64, state: OpState, apply: PendingAction, target: WriteTarget) -> JournalEntry {
        let name = match target {
            WriteTarget::Device { .. } if apply == PendingAction::RestartPc => {
                value_names::PS2_TYPE
            }
            WriteTarget::Device { .. } => value_names::HID_TYPE,
            WriteTarget::Global => value_names::PS2_TYPE,
        };
        JournalEntry {
            schema_version: 1,
            op_id: OpId::parse(&format!("3f2a9c1e-5b7d-4e8a-9c0f-{seq:012x}")).unwrap(),
            seq,
            kind: OpKind::SetLayout {
                requested: KEYCHRON.into(),
                instance_ids: vec![KEYCHRON.into()],
                layout: LayoutChoice::Jis,
            },
            state,
            boot_id: BOOT,
            owner: OWNER,
            created_at: Timestamp(1_790_500_000_000),
            updated_at: Timestamp(1_790_500_004_000),
            apply: Some(apply),
            countdown: None,
            records: vec![record(target, name)],
            context: Vec::new(),
            failure: None,
            revert_mode: None,
            apply_pending: None,
            history: Vec::new(),
        }
    }

    fn hid(seq: u64, state: OpState, apply: PendingAction) -> JournalEntry {
        entry(
            seq,
            state,
            apply,
            WriteTarget::Device {
                instance_id: KEYCHRON.into(),
            },
        )
    }

    fn ps2(seq: u64, state: OpState) -> JournalEntry {
        entry(
            seq,
            state,
            PendingAction::RestartPc,
            WriteTarget::Device {
                instance_id: PS2.into(),
            },
        )
    }

    fn journal(entries: Vec<JournalEntry>) -> Journal {
        Journal {
            entries,
            ..Journal::default()
        }
    }

    fn alive(_: &ProcessIdentity) -> Liveness {
        Liveness::Alive
    }

    fn dead(_: &ProcessIdentity) -> Liveness {
        Liveness::Dead
    }

    #[test]
    fn run_once_follows_the_journal() {
        // Nothing open, or only closed entries: nothing to register.
        assert_eq!(run_once(&journal(vec![]), BOOT, false), RunOnce::NotNeeded);
        assert_eq!(
            run_once(
                &journal(vec![ps2(1, OpState::Confirmed), ps2(2, OpState::Reverted)]),
                BOOT,
                false
            ),
            RunOnce::NotNeeded
        );
        // PendingReboot, in this boot or seen from a later one (not confirmed yet).
        for boot in [BOOT, LATER_BOOT] {
            let pending = journal(vec![ps2(1, OpState::PendingReboot)]);
            assert_eq!(run_once(&pending, boot, false), RunOnce::Register);
            assert_eq!(run_once(&pending, boot, true), RunOnce::TellUser);
        }
        // AwaitingConfirm counts only for an operation that takes effect at a restart.
        let restart = journal(vec![ps2(1, OpState::AwaitingConfirm)]);
        assert_eq!(run_once(&restart, LATER_BOOT, false), RunOnce::Register);
        let reconnect = journal(vec![hid(
            1,
            OpState::AwaitingConfirm,
            PendingAction::Reconnect,
        )]);
        assert_eq!(run_once(&reconnect, BOOT, false), RunOnce::NotNeeded);
        // RevertedPendingReboot needs a restart, not a post-reboot question.
        let reverted = journal(vec![ps2(1, OpState::RevertedPendingReboot)]);
        assert_eq!(run_once(&reverted, BOOT, false), RunOnce::NotNeeded);
    }

    #[test]
    fn gates() {
        let open = |state| journal(vec![hid(1, state, PendingAction::Reconnect)]);
        // Nothing open: every command may start.
        for gate in [Gate::NewOp, Gate::Restore, Gate::Existing, Gate::Recover] {
            assert_eq!(blocker(&journal(vec![]), BOOT, &dead, gate), None);
            assert_eq!(
                blocker(&open(OpState::Confirmed), BOOT, &dead, gate),
                None,
                "{gate:?}"
            );
        }
        // Open, waiting for the user: only new operations stop.
        for state in [OpState::AwaitingConfirm, OpState::Conflict] {
            let journal = open(state);
            let blocked = blocker(&journal, BOOT, &dead, Gate::NewOp).unwrap();
            assert_eq!(blocked.exit_code, exit_code::BLOCKED);
            for gate in [Gate::Restore, Gate::Existing, Gate::Recover] {
                assert_eq!(
                    blocker(&journal, BOOT, &dead, gate),
                    None,
                    "{state:?} {gate:?}"
                );
            }
        }
        let waiting = journal(vec![ps2(1, OpState::PendingReboot)]);
        let blocked = blocker(&waiting, BOOT, &dead, Gate::NewOp).unwrap();
        assert!(
            blocked.message.contains("mklm-cli reboot"),
            "{}",
            blocked.message
        );
        let after_restart = blocker(&waiting, LATER_BOOT, &dead, Gate::Existing).unwrap();
        assert!(after_restart.message.contains("post-reboot"));

        // In flight: its owner still runs → busy for everyone; else recover first.
        let in_flight = open(OpState::Written);
        for gate in [Gate::NewOp, Gate::Restore, Gate::Existing, Gate::Recover] {
            let busy = blocker(&in_flight, BOOT, &alive, gate).unwrap();
            assert!(busy.message.contains("another MKLM process"), "{gate:?}");
        }
        for gate in [Gate::NewOp, Gate::Restore, Gate::Existing] {
            let stuck = blocker(&in_flight, BOOT, &dead, gate).unwrap();
            assert!(stuck.message.contains("mklm-cli recover"), "{gate:?}");
        }
        assert_eq!(blocker(&in_flight, BOOT, &dead, Gate::Recover), None);
        // A countdown whose owner died is recovered too.
        let mut counting = hid(1, OpState::AwaitingConfirm, PendingAction::ResetKeyboard);
        counting.countdown = Some(Countdown {
            seconds: 20,
            deadline: Timestamp(1_790_500_024_000),
        });
        assert!(blocker(&journal(vec![counting]), BOOT, &dead, Gate::Existing).is_some());

        // An unreadable journal stops everything.
        let mut unreadable = journal(vec![]);
        unreadable.unreadable.push(UnreadableEntry {
            name: "x".into(),
            error: JournalError::NewerSchema {
                found: 2,
                supported: 1,
            },
        });
        for gate in [Gate::NewOp, Gate::Restore, Gate::Existing, Gate::Recover] {
            assert_eq!(
                blocker(&unreadable, BOOT, &dead, gate).unwrap().exit_code,
                exit_code::FAILURE
            );
        }
    }

    #[test]
    fn restart_and_post_reboot_entries() {
        let mut pending_hid = hid(3, OpState::Reverted, PendingAction::ResetKeyboard);
        pending_hid.apply_pending = Some(ApplyPending {
            action: PendingAction::RestartPc,
            instance_ids: vec![KEYCHRON.into()],
            since: BOOT,
        });
        let journal = journal(vec![
            ps2(1, OpState::PendingReboot),
            ps2(2, OpState::RevertedPendingReboot),
            pending_hid,
            hid(4, OpState::AwaitingConfirm, PendingAction::Reconnect),
            ps2(5, OpState::AwaitingConfirm),
        ]);
        let seqs = |entries: Vec<&JournalEntry>| entries.iter().map(|e| e.seq).collect::<Vec<_>>();
        assert_eq!(seqs(restart_reasons(&journal, BOOT)), vec![1, 2, 3]);
        assert!(restart_reasons(&journal, LATER_BOOT).is_empty());
        assert_eq!(seqs(post_reboot_entries(&journal)), vec![1, 5]);
        assert!(takes_effect_at_restart(&ps2(1, OpState::Confirmed)));
        assert!(!takes_effect_at_restart(&hid(
            1,
            OpState::Confirmed,
            PendingAction::ResetKeyboard
        )));
    }
}
