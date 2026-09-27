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

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use serde::{Deserialize, Serialize};

use crate::journal::{
    ApplyPending, BootId, JournalEntry, Liveness, OpState, RegValue, ValueRecord,
};
use crate::safety::InvPs2Violation;

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
    todo!("M2: compare with before/intended")
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
pub fn decide_recovery(
    entry: &JournalEntry,
    current: &[RegValue],
    context: &RecoveryContext,
) -> RecoveryDecision {
    todo!("M2: decision table C.7")
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
        todo!("M2")
    }
}

/// See [`Attention`]. `owner` is the liveness of `entry.owner` (ignored for states that are not in
/// flight and for a boot that differs).
pub fn attention(entry: &JournalEntry, current_boot: BootId, owner: Liveness) -> Attention {
    todo!("M2")
}

/// The state an operation ends in once its values are where `current` says, after the user resolved
/// a conflict: all at `intended` → `Confirmed`; all at `before` → `Reverted` (or
/// `RevertedPendingReboot` when boot-time values are involved); otherwise `Failed`
/// (`ConflictKeptCurrent`). The engine refuses a resolution whose end state breaks INV-PS2
/// before calling this (design review C12).
pub fn state_after_resolution(entry: &JournalEntry, current: &[RegValue]) -> OpState {
    todo!("M2")
}

/// What a closing (or reconnect-waiting) entry leaves for the drivers to pick up
/// (design review C1). `None` when every changed value is known to be in effect:
/// - boot-time values are covered by the `RevertedPendingReboot` / `PendingReboot` states and are
///   not listed here, except for a `Failed` entry that touched them (`RestartPc`);
/// - a HID keyboard is listed when its stored values may differ from the running ones: the
///   operation reached `Written` (or recovery observed `AtIntended`), and no successful reset of
///   that keyboard after the last write is in `reapplied`. The action is
///   `mklm_core::device_apply_action` of the keyboard, but never lighter than `Reconnect` when
///   recovery wrote the values (recovery does not reset).
pub fn apply_pending_on_close(
    entry: &JournalEntry,
    current_boot: BootId,
    reapplied: &[String],
) -> Option<ApplyPending> {
    todo!("M2")
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
    todo!("M2")
}
