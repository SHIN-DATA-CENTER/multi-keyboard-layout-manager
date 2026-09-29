//! Recovery decisions (plan 2.3): what to do with a journal entry that its writer left behind.
//!
//! Pure functions over a [`JournalEntry`], the values found in the registry now and the facts about
//! the current boot. `mklm-engine` executes the decision **under the write lock**; the GUI and CLI
//! call [`attention`] unelevated to decide whether to ask for elevation at all.
//! The full decision table is section C.7 of docs/design/m2-engine.md.
//!
//! Eligibility follows from the lock (design review C3): an owner holds the lock for as long as its
//! entry is in flight (and during a countdown), so an in-flight or counting-down entry that the
//! engine finds while holding the lock is abandoned, whether or not its owner process still runs.
//! Process liveness only matters to the unelevated [`attention`].

use serde::{Deserialize, Serialize};

use crate::allowlist::WriteTarget;
use crate::journal::{
    ApplyPending, BootId, FailureReason, JournalEntry, Liveness, OpKind, OpState, RegValue,
    SkipReason, ValueRecord, value_eq,
};
use crate::layout::PendingAction;
use crate::safety::InvPs2Violation;

/// Every history reason of a transition made by recovery starts with this (e.g.
/// `"recover:roll-back"`), so that [`apply_pending_on_close`] can tell values that recovery wrote
/// (and never re-applied by a reset) from values an interactive request wrote.
pub const RECOVERY_REASON_PREFIX: &str = "recover";

/// Facts recovery needs besides the entry and the current values. Only the engine builds it, while
/// it holds the write lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryContext {
    pub current_boot: BootId,
    /// INV-PS2 on the values as they are now (every keyboard, phantoms included, and the global
    /// values, all re-read). A roll-forward or a reboot observation that would close an entry
    /// while INV-PS2 is broken becomes a roll-back or a `Conflict` instead (design review C12).
    pub inv_ps2: Option<InvPs2Violation>,
}

/// Where one value stands relative to its record. Compared with [`crate::value_eq`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Observation {
    /// Equals `before` (and not `intended`).
    AtBefore,
    /// Equals `intended` (and not `before`).
    AtIntended,
    /// `before == intended == current`: the record changes nothing.
    Unchanged,
    /// Neither: somebody else changed it.
    Elsewhere,
}

/// Classifies `current` against `record` (with [`crate::value_eq`] for `record.name`).
pub fn observe(record: &ValueRecord, current: &RegValue) -> Observation {
    let at_before = value_eq(&record.name, current, &record.before);
    let at_intended = value_eq(&record.name, current, &record.intended);
    match (at_before, at_intended) {
        (true, true) => Observation::Unchanged,
        (true, false) => Observation::AtBefore,
        (false, true) => Observation::AtIntended,
        (false, false) => Observation::Elsewhere,
    }
}

/// Why recovery leaves an entry alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeaveReason {
    /// `AwaitingConfirm` without a countdown: the user decides (`keep`, `revert` or `undo`).
    WaitingForUser,
    /// `PendingReboot` / `RevertedPendingReboot` in the boot that wrote it.
    WaitingForReboot,
    /// Closed states need nothing (apart from clearing `apply_pending`, which is housekeeping).
    Closed,
    /// Already in `Conflict`: the user resolves it (or undoes it).
    Conflict,
}

/// Why recovery restores `before`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RollBackReason {
    /// Some values are at `intended`, some at `before` (plan 2.3: "途中まで書かれていれば戻す").
    PartiallyWritten,
    /// A live-reset operation that never reached the user's keep: its countdown's safety net applies.
    LiveResetUnconfirmed,
    /// `AwaitingConfirm` with a countdown found under the lock: the countdown expired unanswered.
    CountdownExpired,
    /// Every value is at `intended`, but the values as they are now break INV-PS2 (an unpinned
    /// i8042prt devnode appeared, or a pin vanished): rolling forward would close an unsafe state
    /// (design review C12).
    InvPs2,
}

impl RollBackReason {
    /// The [`FailureReason`] a roll-back for this reason leaves on the entry. `InvPs2` has no
    /// failure reason of its own and is recorded as [`FailureReason::Interrupted`] (the writer
    /// stopped before its change could be confirmed; the INV-PS2 details go into the result).
    pub fn failure(self) -> FailureReason {
        match self {
            RollBackReason::PartiallyWritten | RollBackReason::InvPs2 => FailureReason::Interrupted,
            RollBackReason::LiveResetUnconfirmed => FailureReason::LiveResetUnconfirmed,
            RollBackReason::CountdownExpired => FailureReason::CountdownExpired,
        }
    }
}

/// What recovery does with one entry. Every decision is idempotent: running recovery again on the
/// result (or after a crash during recovery) converges to the same end state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RecoveryDecision {
    Leave {
        reason: LeaveReason,
    },
    /// Every value is still at `before`: → `Failed` (`NothingWritten`).
    MarkNothingWritten,
    /// Every value is at `intended` and the change is one that is kept after a crash (reconnect or
    /// PC restart): → `to` (`PendingReboot` in the same boot when a restart is needed, else
    /// `AwaitingConfirm` without a countdown). Never auto-confirms, except a silent
    /// restore-to-baseline (`to = Confirmed`).
    RollForward {
        to: OpState,
    },
    /// Restore-to-baseline only (design review C14): write the records still at `before` to
    /// `intended` (the baseline) with compare-and-swap on `before`, then go to `to` as
    /// [`RecoveryDecision::RollForward`] does. A restore is never undone by recovery.
    CompleteForward {
        to: OpState,
    },
    /// → `RevertPending` ([`crate::RevertMode::Rollback`]), restore `before` with
    /// compare-and-swap, then `Reverted` / `RevertedPendingReboot` with `failure` set.
    RollBack {
        reason: RollBackReason,
    },
    /// `RevertPending` found under the lock: continue what [`JournalEntry::revert_mode`] says.
    ContinueRevert,
    /// `PendingReboot` → `AwaitingConfirm`, or `RevertedPendingReboot` → `Reverted`, because the boot
    /// changed. For `PendingReboot` every value must still be at `intended` and INV-PS2 must hold,
    /// else `Conflict`.
    RebootObserved {
        to: OpState,
    },
    /// Values outside {before, intended}, or INV-PS2 broken where no roll-back is possible:
    /// → `Conflict`, touching nothing. `records` are indices into `entry.records`.
    Conflict {
        records: Vec<usize>,
        inv_ps2: Option<InvPs2Violation>,
    },
}

/// Decides what to do with `entry`, **under the write lock**. `current` holds the current value of
/// every record, in `entry.records` order.
///
/// Eligibility: in-flight states ([`OpState::is_in_flight`]) and `AwaitingConfirm` with a countdown
/// are always acted on (their owner would hold the lock); the reboot states only when the boot
/// differs. `RestoreBaseline` entries complete forward instead of rolling back (C.7).
///
/// A record without a current value (`current` shorter than `entry.records`) counts as
/// [`Observation::Elsewhere`]: nothing is decided on a value that was not read. Use
/// [`decide_recovery_with_removed`] when some devnodes no longer exist.
pub fn decide_recovery(
    entry: &JournalEntry,
    current: &[RegValue],
    context: &RecoveryContext,
) -> RecoveryDecision {
    let observations: Vec<Option<Observation>> = entry
        .records
        .iter()
        .enumerate()
        .map(|(i, record)| {
            Some(
                current
                    .get(i)
                    .map_or(Observation::Elsewhere, |value| observe(record, value)),
            )
        })
        .collect();
    decide(entry, &observations, context)
}

/// [`decide_recovery`] where `current[i] == None` means that the devnode of record `i` no longer
/// exists (the backend's `DeviceRemoved`): such records are left out of the observation (C.7),
/// never listed as conflicts, and a restore skips them (`SkipReason::DeviceRemoved`).
pub fn decide_recovery_with_removed(
    entry: &JournalEntry,
    current: &[Option<RegValue>],
    context: &RecoveryContext,
) -> RecoveryDecision {
    let observations: Vec<Option<Observation>> = entry
        .records
        .iter()
        .enumerate()
        .map(|(i, record)| match current.get(i) {
            Some(Some(value)) => Some(observe(record, value)),
            Some(None) => None,
            None => Some(Observation::Elsewhere),
        })
        .collect();
    decide(entry, &observations, context)
}

/// What kind of operation an entry is, as far as recovery cares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    /// `SetLayout`, `Migrate`, `SetStandard` and `Cleanup`: rolled back or forward. A cleanup has
    /// no `apply`, so once every value is deleted it rolls forward to `AwaitingConfirm` without
    /// a countdown, where the user keeps or reverts it (design m3 A.5).
    Change,
    /// Interactive restore-to-baseline: completed forward, never confirmed by recovery.
    InteractiveRestore,
    /// Silent restore-to-baseline (uninstall): completed forward up to `Confirmed`.
    SilentRestore,
}

fn entry_kind(entry: &JournalEntry) -> EntryKind {
    match entry.kind {
        OpKind::RestoreBaseline { silent: true, .. } => EntryKind::SilentRestore,
        OpKind::RestoreBaseline { silent: false, .. } => EntryKind::InteractiveRestore,
        OpKind::SetLayout { .. }
        | OpKind::Migrate { .. }
        | OpKind::Cleanup { .. }
        | OpKind::SetStandard { .. } => EntryKind::Change,
    }
}

/// The decision table of C.7 over observations (`None` = devnode gone, left out).
fn decide(
    entry: &JournalEntry,
    observations: &[Option<Observation>],
    context: &RecoveryContext,
) -> RecoveryDecision {
    let kind = entry_kind(entry);
    let same_boot = entry.boot_id == context.current_boot;
    let has = |wanted: Observation| observations.iter().flatten().any(|o| *o == wanted);
    let conflict = |pick: &dyn Fn(Observation) -> bool| RecoveryDecision::Conflict {
        records: observations
            .iter()
            .enumerate()
            .filter_map(|(i, o)| o.filter(|o| pick(*o)).map(|_| i))
            .collect(),
        inv_ps2: context.inv_ps2.clone(),
    };
    let at_before = has(Observation::AtBefore);
    let at_intended = has(Observation::AtIntended);
    let elsewhere = has(Observation::Elsewhere);

    match entry.state {
        OpState::Planned | OpState::Written | OpState::Restarting => match kind {
            EntryKind::SilentRestore if entry.state != OpState::Restarting => {
                // Anything goes: conflicting values are skipped and logged while completing.
                RecoveryDecision::CompleteForward {
                    to: OpState::Confirmed,
                }
            }
            EntryKind::SilentRestore | EntryKind::InteractiveRestore => {
                if elsewhere {
                    conflict(&|o| o == Observation::Elsewhere)
                } else {
                    RecoveryDecision::CompleteForward {
                        to: complete_forward_target(entry, observations, same_boot),
                    }
                }
            }
            EntryKind::Change => {
                if entry.state == OpState::Planned {
                    if !at_intended && !elsewhere {
                        return RecoveryDecision::MarkNothingWritten;
                    }
                    if elsewhere {
                        return conflict(&|o| o == Observation::Elsewhere);
                    }
                    if at_before {
                        return RecoveryDecision::RollBack {
                            reason: RollBackReason::PartiallyWritten,
                        };
                    }
                } else if elsewhere || at_before {
                    // After `Written` every value was flushed at `intended`; anything else is an
                    // outside change (a value put back at `before` included).
                    return conflict(&|o| {
                        matches!(o, Observation::AtBefore | Observation::Elsewhere)
                    });
                }
                // Every value is at `intended` (or unchanged).
                if entry.state == OpState::Restarting
                    || entry.apply == Some(PendingAction::ResetKeyboard)
                {
                    RecoveryDecision::RollBack {
                        reason: RollBackReason::LiveResetUnconfirmed,
                    }
                } else if context.inv_ps2.is_some() {
                    RecoveryDecision::RollBack {
                        reason: RollBackReason::InvPs2,
                    }
                } else if entry.apply == Some(PendingAction::RestartPc) && same_boot {
                    RecoveryDecision::RollForward {
                        to: OpState::PendingReboot,
                    }
                } else {
                    RecoveryDecision::RollForward {
                        to: OpState::AwaitingConfirm,
                    }
                }
            }
        },
        OpState::RevertPending => RecoveryDecision::ContinueRevert,
        OpState::AwaitingConfirm => match (entry.countdown.is_some(), kind) {
            (false, _) => RecoveryDecision::Leave {
                reason: LeaveReason::WaitingForUser,
            },
            (true, EntryKind::Change) => RecoveryDecision::RollBack {
                reason: RollBackReason::CountdownExpired,
            },
            // A restore is never undone by recovery: drop the countdown, the user decides.
            (true, EntryKind::InteractiveRestore | EntryKind::SilentRestore) => {
                RecoveryDecision::RollForward {
                    to: OpState::AwaitingConfirm,
                }
            }
        },
        OpState::PendingReboot => {
            if same_boot {
                RecoveryDecision::Leave {
                    reason: LeaveReason::WaitingForReboot,
                }
            } else if !at_before && !elsewhere && context.inv_ps2.is_none() {
                RecoveryDecision::RebootObserved {
                    to: OpState::AwaitingConfirm,
                }
            } else {
                conflict(&|o| matches!(o, Observation::AtBefore | Observation::Elsewhere))
            }
        }
        OpState::RevertedPendingReboot => {
            if same_boot {
                RecoveryDecision::Leave {
                    reason: LeaveReason::WaitingForReboot,
                }
            } else {
                RecoveryDecision::RebootObserved {
                    to: OpState::Reverted,
                }
            }
        }
        OpState::Conflict => RecoveryDecision::Leave {
            reason: LeaveReason::Conflict,
        },
        OpState::Confirmed | OpState::Reverted | OpState::Failed => RecoveryDecision::Leave {
            reason: LeaveReason::Closed,
        },
    }
}

/// Where an interactive restore-to-baseline stops once recovery has written it through: never
/// `Confirmed` (the user has not seen it; C14). `PendingReboot` when boot-time values are involved
/// and not all of them have been read by the drivers yet: the entry was written in this boot, or
/// recovery is about to write some of them now (records still at `before`). A boot change alone
/// does not apply what recovery writes after it. `Restarting` only happens to HID-only restores
/// and can only continue to `AwaitingConfirm` (C.4).
fn complete_forward_target(
    entry: &JournalEntry,
    observations: &[Option<Observation>],
    same_boot: bool,
) -> OpState {
    if entry.state == OpState::Restarting || !entry.touches_boot_time_values() {
        return OpState::AwaitingConfirm;
    }
    let writes_boot_time_now = entry
        .records
        .iter()
        .zip(observations)
        .any(|(record, o)| record.is_boot_time() && *o == Some(Observation::AtBefore));
    if same_boot || writes_boot_time_now {
        OpState::PendingReboot
    } else {
        OpState::AwaitingConfirm
    }
}

/// What an entry needs, judged without the lock and without reading the target values (the
/// unelevated GUI/CLI start-up check).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Attention {
    None,
    /// In flight, same boot, owner alive: another MKLM process is working. Show "busy"; do not
    /// launch the helper.
    Busy,
    /// In flight or counting down with its owner dead (or its liveness unknown), or a reboot
    /// state seen from a new boot: launch the helper with `Recover`.
    Recover,
    /// `AwaitingConfirm` without a countdown: ask keep, revert or undo.
    AwaitingUser,
    /// `PendingReboot` in the boot that wrote it: show "restart needed".
    WaitingForReboot,
    /// `Conflict`: show the values (and any write error).
    Conflict,
    /// Closed, but not in effect yet: `RevertedPendingReboot` in the boot that wrote it (restart
    /// needed), or [`JournalEntry::apply_pending`] still applies. Show "not in effect yet" with
    /// the action. Never blocks writes. The caller hides it once [`apply_pending_cleared`] says
    /// Raw Input already reports the stored types.
    NeedsApply,
}

impl Attention {
    /// True when the entry stops new operations (`Recover`, `Busy`, and the open states).
    /// `NeedsApply` and `None` never do.
    pub fn blocks_writes(self) -> bool {
        !matches!(self, Attention::None | Attention::NeedsApply)
    }
}

/// See [`Attention`]. `owner` is the liveness of `entry.owner` (ignored for states that are not in
/// flight and for a boot that differs).
pub fn attention(entry: &JournalEntry, current_boot: BootId, owner: Liveness) -> Attention {
    let same_boot = entry.boot_id == current_boot;
    let busy_or_recover = if same_boot && owner == Liveness::Alive {
        Attention::Busy
    } else {
        Attention::Recover
    };
    match entry.state {
        state if state.is_in_flight() => busy_or_recover,
        OpState::AwaitingConfirm if entry.countdown.is_some() => busy_or_recover,
        OpState::AwaitingConfirm => Attention::AwaitingUser,
        OpState::PendingReboot if same_boot => Attention::WaitingForReboot,
        OpState::PendingReboot => Attention::Recover,
        OpState::Conflict => Attention::Conflict,
        OpState::RevertedPendingReboot if same_boot => Attention::NeedsApply,
        _ => {
            // Closed. Without Raw Input at hand only the boot can clear `apply_pending` here; the
            // caller hides the result once Raw Input reports the stored types.
            let pending = entry
                .apply_pending
                .as_ref()
                .is_some_and(|pending| !apply_pending_cleared(pending, current_boot, &|_| false));
            if pending {
                Attention::NeedsApply
            } else {
                Attention::None
            }
        }
    }
}

/// The state an operation ends in once its values are where `current` says, after the user resolved
/// a conflict: all at `intended` → `Confirmed`; all at `before` → `Reverted` (or
/// `RevertedPendingReboot` when boot-time values are involved); otherwise `Failed`
/// (`ConflictKeptCurrent`). The engine refuses a resolution whose end state breaks INV-PS2
/// before calling this (design review C12).
///
/// Records skipped because their devnode is gone ([`SkipReason::DeviceRemoved`]) are not
/// considered; a record without a value in `current` is neither at `intended` nor at `before`.
pub fn state_after_resolution(entry: &JournalEntry, current: &[RegValue]) -> OpState {
    let considered = || {
        entry
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.skipped != Some(SkipReason::DeviceRemoved))
    };
    let all_at = |pick: fn(&ValueRecord) -> &RegValue| {
        considered().all(|(i, record)| {
            current
                .get(i)
                .is_some_and(|value| value_eq(&record.name, value, pick(record)))
        })
    };
    if all_at(|record| &record.intended) {
        OpState::Confirmed
    } else if all_at(|record| &record.before) {
        if entry.touches_boot_time_values() {
            OpState::RevertedPendingReboot
        } else {
            OpState::Reverted
        }
    } else {
        OpState::Failed
    }
}

/// States an operation passes through once all its values are written (or recovery observed them
/// at `intended` and rolled forward).
const REACHED_WRITTEN: [OpState; 5] = [
    OpState::Written,
    OpState::Restarting,
    OpState::AwaitingConfirm,
    OpState::PendingReboot,
    OpState::Confirmed,
];

/// What a closing (or reconnect-waiting) entry leaves for the drivers to pick up
/// (design review C1). `None` when every changed value is known to be in effect:
/// - boot-time values are covered by the `RevertedPendingReboot` / `PendingReboot` states and are
///   not listed here, except for a `Failed` entry that touched them (`RestartPc`);
/// - a HID keyboard is listed when its stored values may differ from the running ones: the
///   operation reached `Written` (or recovery observed `AtIntended`), and no successful reset of
///   that keyboard after the last write is in `reapplied`. The action is
///   `mklm_core::device_apply_action` of the keyboards (as the operation's `apply` recorded it,
///   see below), but never lighter than `Reconnect` when recovery wrote the values (recovery does
///   not reset).
///
/// Call it with the entry as it closes: in its new state, with `last_written` and `failure` set
/// and the history line of the transition appended. The engine does not call it where it knows
/// that no driver can have read the values, i.e. a roll-back right after `Written` and before any
/// reset (design D.2 a step 1, `CallerDisconnected`: "apply_pending もない"). The rules in detail:
/// - `None` for a cleanup ([`OpKind::Cleanup`]): it only deletes values no driver reads, and puts
///   back only those, so no driver runs with anything but its stored values (design m3 A.5).
/// - `None` when the last write phase happened in an earlier boot (`entry.boot_id` differs from
///   `current_boot`): every driver has read every value since; and `None` when the operation
///   never reached `Written` (no forward state in its history; recovery's roll-forward counts).
/// - Only records whose `before` and `intended` differ count; records skipped because their
///   devnode is gone do not.
/// - `PendingReboot` / `RevertedPendingReboot`: the state asks for the restart; HID keyboards
///   still waiting are listed with `RestartPc` (e.g. a reset keyboard that did not come back).
/// - Any other state with boot-time values: `RestartPc`, listing the HID and i8042prt keyboards
///   (a closed entry no longer shows the restart through its state).
/// - Otherwise the action is the operation's own `apply` (the heaviest `device_apply_action` of
///   its keyboards, as [`crate::apply_method`] chose it; there is no inventory here), at least
///   `Reconnect` when no reset followed the values: recovery wrote them (a history reason
///   starting with [`RECOVERY_REASON_PREFIX`]), a roll-back or failure closed the entry
///   (`failure` set), or the entry waits in `AwaitingConfirm` for a reconnect.
pub fn apply_pending_on_close(
    entry: &JournalEntry,
    current_boot: BootId,
    reapplied: &[String],
) -> Option<ApplyPending> {
    if entry.boot_id != current_boot || matches!(entry.kind, OpKind::Cleanup { .. }) {
        return None;
    }
    let reached_written = REACHED_WRITTEN.contains(&entry.state)
        || entry
            .history
            .iter()
            .any(|h| REACHED_WRITTEN.contains(&h.to));
    if !reached_written {
        return None;
    }
    let is_reapplied = |id: &str| reapplied.iter().any(|r| r.eq_ignore_ascii_case(id));
    let push_unique = |ids: &mut Vec<String>, id: &str| {
        if !ids.iter().any(|x| x.eq_ignore_ascii_case(id)) {
            ids.push(id.to_string());
        }
    };
    let mut hid = Vec::new();
    let mut ps2 = Vec::new();
    let mut boot_time = false;
    let changed = entry.records.iter().filter(|record| {
        record.skipped != Some(SkipReason::DeviceRemoved)
            && !value_eq(&record.name, &record.before, &record.intended)
    });
    for record in changed {
        match &record.target {
            WriteTarget::Global => boot_time = true,
            WriteTarget::Device { instance_id } => {
                if record.is_boot_time() {
                    boot_time = true;
                    if !is_reapplied(instance_id) {
                        push_unique(&mut ps2, instance_id);
                    }
                } else if !is_reapplied(instance_id) {
                    push_unique(&mut hid, instance_id);
                }
            }
        }
    }

    let pending = |action, instance_ids| {
        Some(ApplyPending {
            action,
            instance_ids,
            since: current_boot,
        })
    };
    if matches!(
        entry.state,
        OpState::PendingReboot | OpState::RevertedPendingReboot
    ) {
        return if hid.is_empty() {
            None
        } else {
            pending(PendingAction::RestartPc, hid)
        };
    }
    if boot_time {
        let mut ids = hid;
        for id in &ps2 {
            push_unique(&mut ids, id);
        }
        return pending(PendingAction::RestartPc, ids);
    }
    if hid.is_empty() {
        return None;
    }
    let written_by_recovery = entry
        .history
        .last()
        .is_some_and(|h| h.reason.starts_with(RECOVERY_REASON_PREFIX));
    let no_reset_followed =
        written_by_recovery || entry.failure.is_some() || entry.state == OpState::AwaitingConfirm;
    let mut action = entry.apply.unwrap_or(PendingAction::Reconnect);
    if no_reset_followed {
        action = action.max(PendingAction::Reconnect);
    }
    pending(action, hid)
}

/// True when `pending` no longer applies: the boot changed since `pending.since`, or (for
/// `ResetKeyboard` / `Reconnect`) `reports_stored_type(id)` holds for every listed keyboard
/// (Raw Input reports the type its stored values predict). The engine clears the field as
/// housekeeping under the lock; the unelevated UI uses the same test to hide `NeedsApply`.
pub fn apply_pending_cleared(
    pending: &ApplyPending,
    current_boot: BootId,
    reports_stored_type: &dyn Fn(&str) -> bool,
) -> bool {
    if pending.since != current_boot {
        return true;
    }
    match pending.action {
        PendingAction::RestartPc => false,
        PendingAction::ResetKeyboard | PendingAction::Reconnect => pending
            .instance_ids
            .iter()
            .all(|id| reports_stored_type(id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Countdown, Timestamp, TransitionRecord};
    use crate::model::value_names::*;
    use crate::test_support::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const SECOND: &str = r"HID\VID_3434&PID_D027&MI_01&COL01\8&2&0&0000";
    const PS2: &str = r"ACPI\FUJ0309\4&320DB4C2&0";

    fn violation() -> InvPs2Violation {
        InvPs2Violation {
            keyboards: vec![PS2.to_string()],
        }
    }

    #[test]
    fn observations() {
        let changing = record(device(KEYCHRON), HID_TYPE, dword(4), dword(7));
        assert_eq!(observe(&changing, &dword(4)), Observation::AtBefore);
        assert_eq!(observe(&changing, &dword(7)), Observation::AtIntended);
        assert_eq!(
            observe(&changing, &RegValue::Absent),
            Observation::Elsewhere
        );
        assert_eq!(observe(&changing, &sz("7")), Observation::Elsewhere);
        let same = record(device(KEYCHRON), HID_TYPE, dword(4), dword(4));
        assert_eq!(observe(&same, &dword(4)), Observation::Unchanged);
        assert_eq!(observe(&same, &dword(7)), Observation::Elsewhere);
        // C16: a rewritten case is not an outside change.
        let standard = record(
            WriteTarget::Global,
            LAYER_DRIVER_JPN,
            sz("kbd101.dll"),
            sz("kbd106.dll"),
        );
        assert_eq!(
            observe(&standard, &sz("KBD106.DLL")),
            Observation::AtIntended
        );
        assert_eq!(observe(&standard, &sz("Kbd101.Dll")), Observation::AtBefore);
    }

    #[test]
    fn roll_back_failures() {
        assert_eq!(
            RollBackReason::PartiallyWritten.failure(),
            FailureReason::Interrupted
        );
        assert_eq!(
            RollBackReason::LiveResetUnconfirmed.failure(),
            FailureReason::LiveResetUnconfirmed
        );
        assert_eq!(
            RollBackReason::CountdownExpired.failure(),
            FailureReason::CountdownExpired
        );
        assert_eq!(RollBackReason::InvPs2.failure(), FailureReason::Interrupted);
    }

    const OBSERVATIONS: [Observation; 4] = [
        Observation::AtBefore,
        Observation::AtIntended,
        Observation::Unchanged,
        Observation::Elsewhere,
    ];

    /// A record in `target`/`name` and the current value that gives it `observation`.
    fn observed(
        target: WriteTarget,
        name: &str,
        observation: Observation,
    ) -> (ValueRecord, RegValue) {
        let (before, intended, current) = match observation {
            Observation::AtBefore => (4, 7, 4),
            Observation::AtIntended => (4, 7, 7),
            Observation::Unchanged => (4, 4, 4),
            Observation::Elsewhere => (4, 7, 9),
        };
        (
            record(target, name, dword(before), dword(intended)),
            dword(current),
        )
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Set,
        Migrate,
        /// Decided like a migration (design standard-layout B.6).
        SetStandard,
        /// Decided like a set (design m3 A.5): rolled back or forward, never completed.
        Cleanup,
        InteractiveRestore,
        SilentRestore,
    }

    #[derive(Debug)]
    struct Case {
        state: OpState,
        observations: [Observation; 2],
        same_boot: bool,
        apply: Option<PendingAction>,
        countdown: bool,
        kind: Kind,
        inv_ps2_broken: bool,
        /// Record 1 is a global value (record 0 is always a HID value).
        boot_time: bool,
    }

    /// Design C.7, transcribed row by row as a first-match table. Where two rows of the design
    /// table both apply, the order here decides: nothing-written first, then the silent restore,
    /// outside values, a partial write, a live reset, INV-PS2, the roll-forwards. `Restarting`
    /// counts as a live reset whatever `apply` says. Conflicts list the records that are not
    /// where the state expects them, and always carry the current INV-PS2 violation.
    fn expected(case: &Case) -> RecoveryDecision {
        use OpState::*;
        let obs = &case.observations;
        let has = |o: Observation| obs.contains(&o);
        let at_before = has(Observation::AtBefore);
        let at_intended = has(Observation::AtIntended);
        let elsewhere = has(Observation::Elsewhere);
        let all_intended_or_unchanged = !at_before && !elsewhere;
        let all_before_or_unchanged = !at_intended && !elsewhere;
        let change = matches!(
            case.kind,
            Kind::Set | Kind::Migrate | Kind::SetStandard | Kind::Cleanup
        );
        let silent = case.kind == Kind::SilentRestore;
        let inv = case.inv_ps2_broken.then(violation);
        let conflict = |pick: &dyn Fn(Observation) -> bool| RecoveryDecision::Conflict {
            records: (0..2).filter(|i| pick(obs[*i])).collect(),
            inv_ps2: inv.clone(),
        };
        let in_flight = matches!(case.state, Planned | Written | Restarting);
        let early = matches!(case.state, Planned | Written);
        let state = case.state;
        let unexpected_after_written =
            |o: Observation| matches!(o, Observation::AtBefore | Observation::Elsewhere);

        if state == Planned && change && all_before_or_unchanged {
            return RecoveryDecision::MarkNothingWritten;
        }
        if early && silent {
            return RecoveryDecision::CompleteForward { to: Confirmed };
        }
        if in_flight && elsewhere {
            return if change && state != Planned {
                conflict(&unexpected_after_written)
            } else {
                conflict(&|o| o == Observation::Elsewhere)
            };
        }
        if state == Planned && change && at_before && at_intended {
            return RecoveryDecision::RollBack {
                reason: RollBackReason::PartiallyWritten,
            };
        }
        if matches!(state, Written | Restarting) && change && at_before {
            return conflict(&unexpected_after_written);
        }
        if in_flight
            && change
            && all_intended_or_unchanged
            && (case.apply == Some(PendingAction::ResetKeyboard) || state == Restarting)
        {
            return RecoveryDecision::RollBack {
                reason: RollBackReason::LiveResetUnconfirmed,
            };
        }
        if early && change && all_intended_or_unchanged && case.inv_ps2_broken {
            return RecoveryDecision::RollBack {
                reason: RollBackReason::InvPs2,
            };
        }
        if early
            && change
            && all_intended_or_unchanged
            && case.apply == Some(PendingAction::RestartPc)
            && case.same_boot
        {
            return RecoveryDecision::RollForward { to: PendingReboot };
        }
        if early && change && all_intended_or_unchanged {
            return RecoveryDecision::RollForward {
                to: AwaitingConfirm,
            };
        }
        if in_flight && !change && !elsewhere {
            // Interactive restore (or a silent one caught in `Restarting`): complete forward.
            let boot_time_written_now = case.boot_time && obs[1] == Observation::AtBefore;
            let to = if state == Restarting || !case.boot_time {
                AwaitingConfirm
            } else if case.same_boot || boot_time_written_now {
                PendingReboot
            } else {
                AwaitingConfirm
            };
            return RecoveryDecision::CompleteForward { to };
        }
        match state {
            RevertPending => RecoveryDecision::ContinueRevert,
            AwaitingConfirm if case.countdown && change => RecoveryDecision::RollBack {
                reason: RollBackReason::CountdownExpired,
            },
            AwaitingConfirm if case.countdown => RecoveryDecision::RollForward {
                to: AwaitingConfirm,
            },
            AwaitingConfirm => RecoveryDecision::Leave {
                reason: LeaveReason::WaitingForUser,
            },
            PendingReboot if case.same_boot => RecoveryDecision::Leave {
                reason: LeaveReason::WaitingForReboot,
            },
            PendingReboot if all_intended_or_unchanged && !case.inv_ps2_broken => {
                RecoveryDecision::RebootObserved {
                    to: AwaitingConfirm,
                }
            }
            PendingReboot => conflict(&unexpected_after_written),
            RevertedPendingReboot if case.same_boot => RecoveryDecision::Leave {
                reason: LeaveReason::WaitingForReboot,
            },
            RevertedPendingReboot => RecoveryDecision::RebootObserved { to: Reverted },
            Conflict => RecoveryDecision::Leave {
                reason: LeaveReason::Conflict,
            },
            Confirmed | Reverted | Failed => RecoveryDecision::Leave {
                reason: LeaveReason::Closed,
            },
            _ => panic!("no row of the decision table covers {case:?}"),
        }
    }

    fn build(case: &Case) -> (JournalEntry, Vec<RegValue>, RecoveryContext) {
        let second = if case.boot_time {
            WriteTarget::Global
        } else {
            device(SECOND)
        };
        let second_name = if case.boot_time { PS2_TYPE } else { HID_TYPE };
        let (r0, v0) = observed(device(KEYCHRON), HID_TYPE, case.observations[0]);
        let (r1, v1) = observed(second, second_name, case.observations[1]);
        let kind = match case.kind {
            Kind::Set => set_kind(KEYCHRON),
            Kind::Migrate => migrate_kind(),
            Kind::SetStandard => standard_kind(),
            Kind::Cleanup => cleanup_kind(PS2),
            Kind::InteractiveRestore => restore_kind(false),
            Kind::SilentRestore => restore_kind(true),
        };
        let mut e = entry(1, kind, case.state, vec![r0, r1]);
        e.apply = case.apply;
        e.countdown = case.countdown.then_some(Countdown {
            seconds: 20,
            deadline: Timestamp(0),
        });
        let context = RecoveryContext {
            current_boot: if case.same_boot { boot(1) } else { boot(2) },
            inv_ps2: case.inv_ps2_broken.then(violation),
        };
        (e, vec![v0, v1], context)
    }

    /// Every combination H.1 lists: state × observations × boot × apply × countdown × kind ×
    /// INV-PS2 (and whether a boot-time value is involved).
    #[test]
    fn decide_recovery_matches_the_decision_table_for_every_combination() {
        let applies = [
            None,
            Some(PendingAction::ResetKeyboard),
            Some(PendingAction::Reconnect),
            Some(PendingAction::RestartPc),
        ];
        let kinds = [
            Kind::Set,
            Kind::Migrate,
            Kind::SetStandard,
            Kind::Cleanup,
            Kind::InteractiveRestore,
            Kind::SilentRestore,
        ];
        let mut count = 0;
        for state in OpState::ALL {
            for o0 in OBSERVATIONS {
                for o1 in OBSERVATIONS {
                    for same_boot in [true, false] {
                        for apply in applies {
                            for countdown in [false, true] {
                                for kind in kinds {
                                    for inv_ps2_broken in [false, true] {
                                        for boot_time in [false, true] {
                                            let case = Case {
                                                state,
                                                observations: [o0, o1],
                                                same_boot,
                                                apply,
                                                countdown,
                                                kind,
                                                inv_ps2_broken,
                                                boot_time,
                                            };
                                            let (e, current, context) = build(&case);
                                            assert_eq!(
                                                decide_recovery(&e, &current, &context),
                                                expected(&case),
                                                "{case:?}"
                                            );
                                            count += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(count, 11 * 16 * 2 * 4 * 2 * 6 * 2 * 2);
    }

    /// Spot checks of the table in the words of the design (C.7).
    #[test]
    fn decide_recovery_examples() {
        let ctx = RecoveryContext {
            current_boot: boot(1),
            inv_ps2: None,
        };
        let usb_set = |state, current: u32| {
            let mut e = entry(
                1,
                set_kind(KEYCHRON),
                state,
                vec![record(device(KEYCHRON), HID_TYPE, dword(4), dword(7))],
            );
            e.apply = Some(PendingAction::ResetKeyboard);
            decide_recovery(&e, &[dword(current)], &ctx)
        };
        // Killed after the first write of a USB set: the countdown's rule rolls it back.
        assert_eq!(
            usb_set(OpState::Planned, 7),
            RecoveryDecision::RollBack {
                reason: RollBackReason::LiveResetUnconfirmed
            }
        );
        assert_eq!(
            usb_set(OpState::Planned, 4),
            RecoveryDecision::MarkNothingWritten
        );
        assert_eq!(
            usb_set(OpState::Written, 4),
            RecoveryDecision::Conflict {
                records: vec![0],
                inv_ps2: None
            }
        );
        // A migration written in full, found in the same boot: wait for the restart.
        let mut migration = entry(
            2,
            migrate_kind(),
            OpState::Written,
            vec![
                record(device(PS2), PS2_TYPE, RegValue::Absent, dword(7)),
                record(WriteTarget::Global, PS2_TYPE, dword(7), RegValue::Absent),
            ],
        );
        migration.apply = Some(PendingAction::RestartPc);
        let done = [dword(7), RegValue::Absent];
        assert_eq!(
            decide_recovery(&migration, &done, &ctx),
            RecoveryDecision::RollForward {
                to: OpState::PendingReboot
            }
        );
        let next_boot = RecoveryContext {
            current_boot: boot(2),
            inv_ps2: None,
        };
        assert_eq!(
            decide_recovery(&migration, &done, &next_boot),
            RecoveryDecision::RollForward {
                to: OpState::AwaitingConfirm
            }
        );
        // C12: an unpinned phantom appeared.
        let broken = RecoveryContext {
            current_boot: boot(1),
            inv_ps2: Some(violation()),
        };
        assert_eq!(
            decide_recovery(&migration, &done, &broken),
            RecoveryDecision::RollBack {
                reason: RollBackReason::InvPs2
            }
        );
        migration.state = OpState::PendingReboot;
        assert_eq!(
            decide_recovery(
                &migration,
                &done,
                &RecoveryContext {
                    current_boot: boot(2),
                    inv_ps2: Some(violation()),
                }
            ),
            RecoveryDecision::Conflict {
                records: vec![],
                inv_ps2: Some(violation())
            }
        );
        assert_eq!(
            decide_recovery(&migration, &done, &next_boot),
            RecoveryDecision::RebootObserved {
                to: OpState::AwaitingConfirm
            }
        );
        // Stopped in the middle of the global step (only the type deleted).
        migration.state = OpState::Planned;
        migration.records.push(record(
            WriteTarget::Global,
            PS2_SUBTYPE,
            dword(2),
            RegValue::Absent,
        ));
        assert_eq!(
            decide_recovery(&migration, &[dword(7), RegValue::Absent, dword(2)], &ctx),
            RecoveryDecision::RollBack {
                reason: RollBackReason::PartiallyWritten
            }
        );
    }

    #[test]
    fn restore_in_a_new_boot_waits_for_a_restart_when_recovery_writes_boot_time_values() {
        let restore = entry(
            3,
            restore_kind(false),
            OpState::Planned,
            vec![
                record(WriteTarget::Global, PS2_TYPE, RegValue::Absent, dword(7)),
                record(device(PS2), PS2_TYPE, dword(7), RegValue::Absent),
            ],
        );
        let new_boot = RecoveryContext {
            current_boot: boot(9),
            inv_ps2: None,
        };
        // Nothing written before the crash: recovery writes the global value now.
        assert_eq!(
            decide_recovery(&restore, &[RegValue::Absent, dword(7)], &new_boot),
            RecoveryDecision::CompleteForward {
                to: OpState::PendingReboot
            }
        );
        // Everything was written before the restart: in effect now.
        assert_eq!(
            decide_recovery(&restore, &[dword(7), RegValue::Absent], &new_boot),
            RecoveryDecision::CompleteForward {
                to: OpState::AwaitingConfirm
            }
        );
    }

    #[test]
    fn removed_devnodes_are_left_out_of_the_observation() {
        let ctx = RecoveryContext {
            current_boot: boot(1),
            inv_ps2: None,
        };
        let mut e = entry(
            1,
            set_kind(KEYCHRON),
            OpState::Written,
            vec![
                record(device(KEYCHRON), HID_TYPE, dword(4), dword(7)),
                record(device(SECOND), HID_TYPE, dword(4), dword(7)),
            ],
        );
        e.apply = Some(PendingAction::Reconnect);
        // The phantom collection is gone: only the Keychron counts.
        assert_eq!(
            decide_recovery_with_removed(&e, &[Some(dword(7)), None], &ctx),
            RecoveryDecision::RollForward {
                to: OpState::AwaitingConfirm
            }
        );
        assert_eq!(
            decide_recovery_with_removed(&e, &[Some(dword(9)), None], &ctx),
            RecoveryDecision::Conflict {
                records: vec![0],
                inv_ps2: None
            }
        );
        // A value that was not read is never taken as a fact.
        assert_eq!(
            decide_recovery(&e, &[dword(7)], &ctx),
            RecoveryDecision::Conflict {
                records: vec![1],
                inv_ps2: None
            }
        );
        assert_eq!(
            decide_recovery_with_removed(&e, &[Some(dword(7))], &ctx),
            RecoveryDecision::Conflict {
                records: vec![1],
                inv_ps2: None
            }
        );
    }

    /// Design standard-layout B.7, SAFETY-7: the check-only records of a standard change turn a
    /// fixed-mode pair that appears into a conflict, through the existing rules.
    #[test]
    fn a_fixed_pair_that_appears_makes_a_standard_change_a_conflict() {
        let records = vec![
            record(device(KEYCHRON), HID_TYPE, RegValue::Absent, dword(7)),
            record(
                WriteTarget::Global,
                PS2_TYPE,
                RegValue::Absent,
                RegValue::Absent,
            ),
            record(
                WriteTarget::Global,
                PS2_SUBTYPE,
                RegValue::Absent,
                RegValue::Absent,
            ),
            record(
                WriteTarget::Global,
                LAYER_DRIVER_JPN,
                RegValue::Sz {
                    value: "kbd106.dll".into(),
                },
                RegValue::Sz {
                    value: "kbd101.dll".into(),
                },
            ),
        ];
        let written = |state| {
            let mut e = entry(1, standard_kind(), state, records.clone());
            e.apply = Some(PendingAction::RestartPc);
            e
        };
        let intended: Vec<RegValue> = records.iter().map(|r| r.intended.clone()).collect();
        let fixed_english = {
            let mut values = intended.clone();
            values[1] = dword(7);
            values[2] = dword(0);
            values
        };
        let later_boot = RecoveryContext {
            current_boot: boot(2),
            inv_ps2: None,
        };
        let same_boot = RecoveryContext {
            current_boot: boot(1),
            inv_ps2: None,
        };
        // Without the pair: the restart is observed, as for a migration.
        assert_eq!(
            decide_recovery(&written(OpState::PendingReboot), &intended, &later_boot),
            RecoveryDecision::RebootObserved {
                to: OpState::AwaitingConfirm
            }
        );
        // With it (the Settings app's "English keyboard" after the change): a conflict on the
        // check-only records, after the restart and for an entry found in flight.
        assert_eq!(
            decide_recovery(
                &written(OpState::PendingReboot),
                &fixed_english,
                &later_boot
            ),
            RecoveryDecision::Conflict {
                records: vec![1, 2],
                inv_ps2: None
            }
        );
        assert_eq!(
            decide_recovery(&written(OpState::Planned), &fixed_english, &same_boot),
            RecoveryDecision::Conflict {
                records: vec![1, 2],
                inv_ps2: None
            }
        );
        assert_eq!(
            decide_recovery(&written(OpState::Written), &fixed_english, &same_boot),
            RecoveryDecision::Conflict {
                records: vec![1, 2],
                inv_ps2: None
            }
        );
        // The check-only records alone never make an entry look written: nothing else written
        // yet is "nothing written".
        let nothing: Vec<RegValue> = records.iter().map(|r| r.before.clone()).collect();
        assert_eq!(
            decide_recovery(&written(OpState::Planned), &nothing, &same_boot),
            RecoveryDecision::MarkNothingWritten
        );
    }

    /// Design C.7 "非昇格の判断", with `blocks_writes`.
    #[test]
    fn attention_table() {
        use Attention as A;
        let pending = |since| ApplyPending {
            action: PendingAction::RestartPc,
            instance_ids: vec![],
            since,
        };
        for state in OpState::ALL {
            for same_boot in [true, false] {
                for owner in [Liveness::Alive, Liveness::Dead, Liveness::Unknown] {
                    for countdown in [false, true] {
                        for apply_pending in [None, Some(pending(boot(1))), Some(pending(boot(0)))]
                        {
                            let mut e = entry(1, migrate_kind(), state, Vec::new());
                            e.countdown = countdown.then_some(Countdown {
                                seconds: 20,
                                deadline: Timestamp(0),
                            });
                            e.apply_pending = apply_pending.clone();
                            let current = if same_boot { boot(1) } else { boot(2) };
                            let busy_or_recover = if same_boot && owner == Liveness::Alive {
                                A::Busy
                            } else {
                                A::Recover
                            };
                            // Recorded in this boot and not cleared by a restart since.
                            let pending_now =
                                apply_pending.as_ref().is_some_and(|p| p.since == current);
                            let want = match state {
                                s if s.is_in_flight() => busy_or_recover,
                                OpState::AwaitingConfirm if countdown => busy_or_recover,
                                OpState::AwaitingConfirm => A::AwaitingUser,
                                OpState::PendingReboot if same_boot => A::WaitingForReboot,
                                OpState::PendingReboot => A::Recover,
                                OpState::Conflict => A::Conflict,
                                OpState::RevertedPendingReboot if same_boot => A::NeedsApply,
                                _ if pending_now => A::NeedsApply,
                                _ => A::None,
                            };
                            assert_eq!(
                                attention(&e, current, owner),
                                want,
                                "{state:?} same_boot={same_boot} {owner:?} countdown={countdown} {apply_pending:?}"
                            );
                            assert_eq!(want.blocks_writes(), state.is_open(), "{state:?}");
                        }
                    }
                }
            }
        }
        for (attention, blocks) in [
            (A::None, false),
            (A::Busy, true),
            (A::Recover, true),
            (A::AwaitingUser, true),
            (A::WaitingForReboot, true),
            (A::Conflict, true),
            (A::NeedsApply, false),
        ] {
            assert_eq!(attention.blocks_writes(), blocks, "{attention:?}");
        }
    }

    #[test]
    fn state_after_resolution_cases() {
        let hid = entry(
            1,
            set_kind(KEYCHRON),
            OpState::Conflict,
            vec![
                record(device(KEYCHRON), HID_TYPE, dword(4), dword(7)),
                record(device(KEYCHRON), HID_SUBTYPE, dword(0), dword(2)),
            ],
        );
        assert_eq!(
            state_after_resolution(&hid, &[dword(7), dword(2)]),
            OpState::Confirmed
        );
        assert_eq!(
            state_after_resolution(&hid, &[dword(4), dword(0)]),
            OpState::Reverted
        );
        assert_eq!(
            state_after_resolution(&hid, &[dword(4), dword(2)]),
            OpState::Failed
        );
        assert_eq!(
            state_after_resolution(&hid, &[dword(5), dword(0)]),
            OpState::Failed
        );
        assert_eq!(state_after_resolution(&hid, &[dword(7)]), OpState::Failed);
        let mut migration = entry(
            2,
            migrate_kind(),
            OpState::Conflict,
            vec![
                record(device(PS2), PS2_TYPE, RegValue::Absent, dword(7)),
                record(
                    WriteTarget::Global,
                    LAYER_DRIVER_JPN,
                    sz("kbd101.dll"),
                    sz("kbd106.dll"),
                ),
            ],
        );
        assert_eq!(
            state_after_resolution(&migration, &[RegValue::Absent, sz("KBD101.DLL")]),
            OpState::RevertedPendingReboot
        );
        assert_eq!(
            state_after_resolution(&migration, &[dword(7), sz("KBD106.dll")]),
            OpState::Confirmed
        );
        // A removed devnode does not keep the resolution from closing.
        migration.records[0].skipped = Some(SkipReason::DeviceRemoved);
        assert_eq!(
            state_after_resolution(&migration, &[RegValue::Absent, sz("kbd106.dll")]),
            OpState::Confirmed
        );
    }

    /// An entry of a HID change that went through `Written`, closing into `state`.
    fn closing(state: OpState, reason: &str, apply: PendingAction) -> JournalEntry {
        let mut e = entry(
            1,
            set_kind(KEYCHRON),
            state,
            vec![
                record(device(KEYCHRON), HID_TYPE, dword(4), dword(7)),
                record(device(KEYCHRON), HID_SUBTYPE, dword(0), dword(2)),
                record(device(SECOND), HID_TYPE, dword(4), dword(7)),
            ],
        );
        e.apply = Some(apply);
        for (to, why) in [(OpState::Written, "written"), (state, reason)] {
            e.history.push(TransitionRecord {
                from: None,
                to,
                at: Timestamp(0),
                boot: boot(1),
                by: OWNER,
                reason: why.to_string(),
                boot_time_hint: None,
            });
        }
        e
    }

    #[test]
    fn apply_pending_on_close_rules() {
        let now = boot(1);
        let reset = PendingAction::ResetKeyboard;
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // A user's revert without a reset: the keyboards' own action (C9).
        let revert = closing(OpState::Reverted, "revert", reset);
        assert_eq!(
            apply_pending_on_close(&revert, now, &[]),
            Some(ApplyPending {
                action: reset,
                instance_ids: ids(&[KEYCHRON, SECOND]),
                since: now,
            })
        );
        // Keyboards reset after the last write are in effect.
        assert_eq!(
            apply_pending_on_close(&revert, now, &ids(&[&KEYCHRON.to_ascii_lowercase()]))
                .unwrap()
                .instance_ids,
            ids(&[SECOND])
        );
        assert_eq!(
            apply_pending_on_close(&revert, now, &ids(&[KEYCHRON, SECOND])),
            None
        );
        // Recovery wrote the values (and does not reset): never lighter than Reconnect.
        let mut rolled_back = closing(OpState::Reverted, "recover:roll-back", reset);
        assert_eq!(
            apply_pending_on_close(&rolled_back, now, &[])
                .unwrap()
                .action,
            PendingAction::Reconnect
        );
        rolled_back.history.last_mut().unwrap().reason = "rollback".into();
        rolled_back.failure = Some(FailureReason::CountdownExpired);
        assert_eq!(
            apply_pending_on_close(&rolled_back, now, &[])
                .unwrap()
                .action,
            PendingAction::Reconnect
        );
        let forward = closing(OpState::AwaitingConfirm, "recover:roll-forward", reset);
        assert_eq!(
            apply_pending_on_close(&forward, now, &[]).unwrap().action,
            PendingAction::Reconnect
        );
        // A heavier own action stays.
        let only_usable = closing(
            OpState::Reverted,
            "recover:roll-back",
            PendingAction::RestartPc,
        );
        assert_eq!(
            apply_pending_on_close(&only_usable, now, &[])
                .unwrap()
                .action,
            PendingAction::RestartPc
        );
        // Kept before Raw Input changed on the reconnect path (C1).
        let kept = closing(OpState::Confirmed, "keep", PendingAction::Reconnect);
        assert_eq!(
            apply_pending_on_close(&kept, now, &[]).unwrap().action,
            PendingAction::Reconnect
        );
        // The reset keyboard did not come back: restart.
        let lost = closing(OpState::RevertedPendingReboot, "did-not-return", reset);
        assert_eq!(
            apply_pending_on_close(&lost, now, &[]),
            Some(ApplyPending {
                action: PendingAction::RestartPc,
                instance_ids: ids(&[KEYCHRON, SECOND]),
                since: now,
            })
        );
        // Written in an earlier boot: every driver has read the values since.
        assert_eq!(apply_pending_on_close(&revert, boot(2), &[]), None);
    }

    #[test]
    fn apply_pending_on_close_needs_a_written_operation() {
        let now = boot(1);
        // Rolled back from Planned (never reached Written), or nothing written at all.
        let mut partial = entry(
            1,
            set_kind(KEYCHRON),
            OpState::Reverted,
            vec![record(device(KEYCHRON), HID_TYPE, dword(4), dword(7))],
        );
        partial.apply = Some(PendingAction::ResetKeyboard);
        partial.failure = Some(FailureReason::Interrupted);
        partial.history.push(TransitionRecord {
            from: Some(OpState::Planned),
            to: OpState::RevertPending,
            at: Timestamp(0),
            boot: now,
            by: OWNER,
            reason: "recover:roll-back".into(),
            boot_time_hint: None,
        });
        assert_eq!(apply_pending_on_close(&partial, now, &[]), None);
        partial.state = OpState::Failed;
        partial.failure = Some(FailureReason::NothingWritten);
        assert_eq!(apply_pending_on_close(&partial, now, &[]), None);
        // Records that change nothing, or whose devnode is gone, do not count.
        let mut idle = closing(OpState::Confirmed, "keep", PendingAction::ResetKeyboard);
        for r in &mut idle.records {
            r.intended = r.before.clone();
        }
        assert_eq!(apply_pending_on_close(&idle, now, &[]), None);
        let mut gone = closing(OpState::Reverted, "revert", PendingAction::ResetKeyboard);
        for r in &mut gone.records {
            r.skipped = Some(SkipReason::DeviceRemoved);
        }
        assert_eq!(apply_pending_on_close(&gone, now, &[]), None);
    }

    #[test]
    fn apply_pending_on_close_with_boot_time_values() {
        let now = boot(1);
        let mut e = entry(
            1,
            migrate_kind(),
            OpState::Failed,
            vec![
                record(device(PS2), PS2_TYPE, RegValue::Absent, dword(7)),
                record(device(KEYCHRON), HID_TYPE, RegValue::Absent, dword(4)),
                record(WriteTarget::Global, PS2_TYPE, dword(7), RegValue::Absent),
            ],
        );
        e.apply = Some(PendingAction::RestartPc);
        e.failure = Some(FailureReason::Superseded { by: op_id(2) });
        e.history.push(TransitionRecord {
            from: Some(OpState::Written),
            to: OpState::PendingReboot,
            at: Timestamp(0),
            boot: now,
            by: OWNER,
            reason: "pending-reboot".into(),
            boot_time_hint: None,
        });
        // A closed entry no longer shows the restart through its state.
        assert_eq!(
            apply_pending_on_close(&e, now, &[]),
            Some(ApplyPending {
                action: PendingAction::RestartPc,
                instance_ids: vec![KEYCHRON.to_string(), PS2.to_string()],
                since: now,
            })
        );
        // PendingReboot / RevertedPendingReboot show it themselves; only HID keyboards are listed.
        e.state = OpState::RevertedPendingReboot;
        assert_eq!(
            apply_pending_on_close(&e, now, &[]).unwrap().instance_ids,
            vec![KEYCHRON.to_string()]
        );
        e.records.remove(1);
        assert_eq!(apply_pending_on_close(&e, now, &[]), None);
        // Global values only, closed: a restart without keyboards to list.
        e.state = OpState::Failed;
        e.records.remove(0);
        assert_eq!(
            apply_pending_on_close(&e, now, &[]),
            Some(ApplyPending {
                action: PendingAction::RestartPc,
                instance_ids: vec![],
                since: now,
            })
        );
    }

    /// A cleanup deletes values no driver reads (design m3 A.5): whatever state it closes in,
    /// nothing waits to take effect, and none of its values waits for a restart.
    #[test]
    fn a_cleanup_leaves_nothing_pending() {
        let now = boot(1);
        let records = vec![
            record(device(PS2), HID_TYPE, dword(7), RegValue::Absent),
            record(device(PS2), HID_SUBTYPE, dword(2), RegValue::Absent),
            record(device(KEYCHRON), PS2_TYPE, dword(7), RegValue::Absent),
        ];
        for state in [
            OpState::Confirmed,
            OpState::Reverted,
            OpState::AwaitingConfirm,
            OpState::Failed,
        ] {
            let mut e = entry(1, cleanup_kind(PS2), state, records.clone());
            e.history.push(TransitionRecord {
                from: Some(OpState::Planned),
                to: OpState::Written,
                at: Timestamp(0),
                boot: now,
                by: OWNER,
                reason: "written".into(),
                boot_time_hint: None,
            });
            assert_eq!(apply_pending_on_close(&e, now, &[]), None, "{state:?}");
            assert!(!e.touches_boot_time_values(), "{state:?}");
        }
        // The same records in a set of the PS/2 keyboard would be listed (they are not boot-time,
        // so the HID path applies): only the kind makes the difference.
        let mut e = entry(1, set_kind(PS2), OpState::Confirmed, records);
        e.apply = Some(PendingAction::RestartPc);
        assert!(apply_pending_on_close(&e, now, &[]).is_some());
        // A cleanup is decided like a set: fully written, it waits for the user.
        let mut planned = entry(
            1,
            cleanup_kind(PS2),
            OpState::Planned,
            vec![record(device(PS2), HID_TYPE, dword(7), RegValue::Absent)],
        );
        planned.apply = None;
        let ctx = RecoveryContext {
            current_boot: now,
            inv_ps2: None,
        };
        assert_eq!(
            decide_recovery(&planned, &[RegValue::Absent], &ctx),
            RecoveryDecision::RollForward {
                to: OpState::AwaitingConfirm
            }
        );
        assert_eq!(
            decide_recovery(&planned, &[dword(7)], &ctx),
            RecoveryDecision::MarkNothingWritten
        );
        assert_eq!(
            attention(
                &entry(1, cleanup_kind(PS2), OpState::AwaitingConfirm, Vec::new()),
                now,
                Liveness::Dead
            ),
            Attention::AwaitingUser
        );
    }

    #[test]
    fn apply_pending_cleared_rules() {
        let pending = |action| ApplyPending {
            action,
            instance_ids: vec![KEYCHRON.to_string(), SECOND.to_string()],
            since: boot(1),
        };
        let all = |_: &str| true;
        let none = |_: &str| false;
        let only_keychron = |id: &str| id == KEYCHRON;
        for action in [PendingAction::ResetKeyboard, PendingAction::Reconnect] {
            assert!(apply_pending_cleared(&pending(action), boot(2), &none));
            assert!(apply_pending_cleared(&pending(action), boot(1), &all));
            assert!(!apply_pending_cleared(
                &pending(action),
                boot(1),
                &only_keychron
            ));
            assert!(!apply_pending_cleared(&pending(action), boot(1), &none));
        }
        // Raw Input cannot show that a restart happened.
        assert!(!apply_pending_cleared(
            &pending(PendingAction::RestartPc),
            boot(1),
            &all
        ));
        assert!(apply_pending_cleared(
            &pending(PendingAction::RestartPc),
            boot(2),
            &none
        ));
        let empty = ApplyPending {
            action: PendingAction::Reconnect,
            instance_ids: vec![],
            since: boot(1),
        };
        assert!(apply_pending_cleared(&empty, boot(1), &none));
    }
}
