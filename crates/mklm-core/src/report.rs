//! Vocabulary shared by the engine and everything that drives it: request options, progress events,
//! the user's decisions and the results (design sections B, D and E.6).
//!
//! Pure data. `mklm-engine` produces and consumes these; `mklm-ipc` carries them over the pipe;
//! the GUI and CLI show them. They live here rather than in `mklm-ipc` so that the engine does not
//! depend on the wire protocol, and so that the wire can only expose what `mklm-ipc` chooses to
//! (design review S5: the silent uninstall mode is not reachable through the pipe).

use serde::{Deserialize, Serialize};

use crate::allowlist::{PlanError, PlanStep};
use crate::journal::{FailureReason, OpId, OpState, RegValue};
use crate::layout::{LayoutTable, PendingAction};
use crate::model::KeyboardType;
use crate::safety::InvPs2Violation;

/// Whether a request may restart keyboards in place (plan 1.4). Every request that can end with a
/// live reset carries it: set, revert, restore, conflict resolution, recover and undo
/// (design review C9). The default (both false) never resets: the change then waits for a
/// reconnect or a PC restart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApplyOptions {
    /// False: never reset a keyboard in place (CLI `--no-reset`).
    pub allow_live_reset: bool,
    /// The caller saw recent input from another keyboard, or the user confirmed another way to
    /// type. When false the target counts as the only usable keyboard (plan 1.4) and is not reset.
    pub other_input_available: bool,
}

/// What the user approved in the unelevated dry run. When the engine's own plan differs in any
/// step or in how the change takes effect, the request fails with [`ErrorCode::PlanChanged`] and
/// nothing is written (design review S6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedPlan {
    pub steps: Vec<PlanStep>,
    pub apply: Option<PendingAction>,
}

/// What to do with a value whose current content is not what MKLM last wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictPolicy {
    /// Stop at `Conflict` and report the three values (interactive default).
    Report,
    /// Leave the value alone and log it (silent mode, plan 2.3).
    Skip,
    /// Write the baseline anyway (the user already chose "use baseline" for every conflict).
    Overwrite,
}

/// The user's choice for one value of an operation in `Conflict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionChoice {
    /// Leave the current value; MKLM records it as what it last saw there.
    KeepCurrent,
    UseBefore,
    /// MKLM's `intended`.
    UseIntended,
    UseBaseline,
}

/// Choice for one record of the operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueChoice {
    /// Index into the operation's records (as in [`ConflictInfo::record`]).
    pub record: usize,
    pub choice: ResolutionChoice,
}

/// The user's answer during a countdown or a confirmation wait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Decision {
    Keep { op_id: OpId },
    RevertNow { op_id: OpId },
}

/// What one keyboard should report / type once the change is in effect (for the confirmation UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedKeyboard {
    pub instance_id: String,
    pub display_name: String,
    /// Type the driver should report (Raw Input), e.g. 0x4/0x0; `None` for other drivers.
    pub expected_type: Option<KeyboardType>,
    /// Layout it types with afterwards (Japanese IME assumed).
    pub layout_after: Option<LayoutTable>,
    /// True when this keyboard's layout changes (a migration also lists unchanged keyboards).
    pub changes: bool,
}

/// Progress of a running request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Event {
    /// The write lock is held.
    Locked,
    /// The operation is journaled (`Planned`, flushed); nothing is written yet.
    Planned {
        op_id: OpId,
        steps: Vec<PlanStep>,
        apply: Option<PendingAction>,
        keyboards: Vec<ExpectedKeyboard>,
    },
    /// A state transition was flushed.
    StateChanged {
        op_id: OpId,
        state: OpState,
    },
    /// A step's values were written and flushed.
    StepWritten {
        op_id: OpId,
        step: usize,
        of: usize,
    },
    ResettingKeyboard {
        instance_id: String,
    },
    KeyboardArrived {
        instance_id: String,
        reported: Option<KeyboardType>,
        expected: KeyboardType,
    },
    /// Keep-or-revert countdown after a live reset; no answer means revert.
    CountdownStarted {
        op_id: OpId,
        seconds: u32,
        /// Raw Input reports the intended type (plan gate G3).
        verified: bool,
    },
    CountdownTick {
        op_id: OpId,
        remaining: u32,
    },
    /// Reconnect path: waiting for the keyboard to come back; then a [`Decision`] is awaited
    /// without a countdown.
    WaitingForReconnect {
        op_id: OpId,
        instance_ids: Vec<String>,
    },
    RecoveryAssetsWritten {
        directory: String,
        skipped: usize,
    },
    Warning {
        message: String,
    },
    /// Sent by the helper's writer thread every 10 s whatever the engine is doing (design review
    /// S7). The engine itself never emits it.
    Heartbeat,
}

/// How a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Every value already had the requested content; nothing was journaled.
    NoChange,
    Confirmed,
    /// Waiting for keep/revert (reconnect path, or a later confirm).
    AwaitingConfirm,
    PendingReboot,
    /// Values are back at `before`. With [`OperationResult::failure`] set, the revert was not the
    /// user's request (rollback after an error, an expired countdown, a disconnect).
    Reverted,
    RevertedPendingReboot,
    /// Closed without its change ever taking effect (see [`OperationResult::failure`]).
    Failed,
    Conflict,
    /// A `Recover` or `Undo` request finished (see [`OperationResult::recovered`]).
    Recovered,
}

/// A value in conflict: the values plan 2.3 shows the user, plus the recorded ends of the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub op_id: OpId,
    /// Index into the operation's records.
    pub record: usize,
    pub key_path: String,
    pub name: String,
    pub baseline: RegValue,
    pub before: RegValue,
    pub intended: RegValue,
    pub last_written: Option<RegValue>,
    pub current: RegValue,
    /// Set when the conflict is a write that kept failing (design review C3), not an outside
    /// change: the error text of the last attempt.
    pub write_error: Option<String>,
}

/// One entry a `Recover` or `Undo` request acted on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredOp {
    pub op_id: OpId,
    pub from: OpState,
    pub to: OpState,
    /// The decision taken, e.g. `"roll-back"`, `"roll-forward"`, `"reboot-observed"`, `"undo"`.
    pub decision: String,
}

/// Final answer to a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationResult {
    /// The operation created or acted on; `None` for `NoChange`, `Recover` and `Undo`.
    pub op_id: Option<OpId>,
    pub outcome: Outcome,
    /// Why the operation did not end as requested (rollback, supersede, nothing written).
    pub failure: Option<FailureReason>,
    /// What the user still has to do for the stored values to take effect. Also persisted in the
    /// journal ([`crate::JournalEntry::apply_pending`]) so that it survives the process (design
    /// review C1). For [`PendingAction::RestartPc`] the caller registers the post-reboot RunOnce
    /// entry and offers the restart (the helper never restarts the PC).
    pub pending_action: Option<PendingAction>,
    pub conflicts: Vec<ConflictInfo>,
    /// Set when the operation stopped at `Conflict` because the values as they are now break
    /// INV-PS2 (design review C12); lists the i8042prt keyboards without a pin.
    pub inv_ps2_violation: Option<InvPs2Violation>,
    pub recovered: Vec<RecoveredOp>,
    pub warnings: Vec<String>,
}

/// Machine-readable error class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// Another MKLM process holds the write lock.
    Busy,
    /// An open operation blocks new ones (plan 2.2).
    OpInProgress,
    /// An entry needs `Recover` first.
    RecoveryNeeded,
    /// The journal has entries this build cannot read; writes stop.
    JournalUnreadable,
    /// Allowlist or INV-PS2 (see [`ErrorInfo::plan_error`]).
    PlanRejected,
    /// The engine's plan differs from the approved [`ExpectedPlan`].
    PlanChanged,
    MigrationRequired,
    NotFixedMode,
    UnknownKeyboard,
    UnknownOp,
    /// `Revert` of an operation a later operation overwrote; use restore-to-baseline.
    NotLatest,
    /// The operation is not in a state that allows the request.
    InvalidState,
    /// The device enumeration could not establish every Keyboard-class devnode, its driver and
    /// its presence; nothing may be written.
    InventoryIncomplete,
    /// The offline recovery files could not be written durably before a change of boot-time
    /// values (design review C10); nothing was written.
    RecoveryAssetsUnavailable,
    /// The caller went away before anything was journaled; nothing was written.
    Cancelled,
    Registry,
    Device,
    /// Lock, protected directory, process or clock failures.
    Host,
    Protocol,
    Internal,
}

/// A failed request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    /// English, for logs and the CLI.
    pub message: String,
    pub op_id: Option<OpId>,
    pub plan_error: Option<PlanError>,
}
