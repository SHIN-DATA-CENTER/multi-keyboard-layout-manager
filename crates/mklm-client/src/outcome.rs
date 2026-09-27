//! How a request ended, as a class both front ends act on (design m2 F.5, m3 A.2): the CLI maps
//! [`OutcomeClass`] onto its exit codes, the GUI onto its result screens.

use std::fmt;

use mklm_core::{ErrorCode, FailureReason, OpState, OperationResult, Outcome, PendingAction};

/// The class of a finished request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutcomeClass {
    /// Kept, reverted on request, nothing to change, or recovered (CLI 0).
    Done,
    /// Failed; nothing changed or everything was rolled back after an error (CLI 1).
    Failed,
    /// The user cancelled before anything was written: declined UAC, answered "no", or the
    /// caller left before the plan was journaled (CLI 3).
    Cancelled,
    /// Reverted automatically: the countdown expired, the keyboard did not come back, the
    /// change was never kept, the caller left (CLI 4).
    RevertedAutomatically,
    /// Values in conflict: resolve or undo (CLI 5).
    Conflict,
    /// Another operation holds the lock, an open operation blocks this one, or recovery is
    /// needed first (CLI 6).
    Blocked,
    /// Written; waiting for a reconnect and keep / revert (CLI 10).
    AwaitingConfirm,
    /// Written or reverted; a PC restart is needed (CLI 3010).
    RestartRequired,
}

/// The class of a request that ended with a result.
///
/// - `NoChange`, `Confirmed`: done. A resolution that kept outside values (`Failed` with
///   `ConflictKeptCurrent`) did what the user chose: done.
/// - `AwaitingConfirm`: awaiting confirm. `PendingReboot`: restart required.
/// - `Reverted` / `RevertedPendingReboot` as requested (no `failure`): done / restart required.
///   Reverted automatically ([`reverted_automatically`]): reverted automatically. Rolled back
///   after an error or an interruption: failed.
/// - `Failed` otherwise: failed. `Conflict`: conflict.
/// - `Recovered` (`recover`, `undo`): conflict with conflicts left, restart required when a
///   restart is pending, else done.
pub fn classify_result(result: &OperationResult) -> OutcomeClass {
    match result.outcome {
        Outcome::NoChange | Outcome::Confirmed => OutcomeClass::Done,
        Outcome::AwaitingConfirm => OutcomeClass::AwaitingConfirm,
        Outcome::PendingReboot => OutcomeClass::RestartRequired,
        Outcome::Conflict => OutcomeClass::Conflict,
        Outcome::Failed => match result.failure {
            Some(FailureReason::ConflictKeptCurrent) => OutcomeClass::Done,
            _ => OutcomeClass::Failed,
        },
        Outcome::Reverted | Outcome::RevertedPendingReboot => match &result.failure {
            None if result.outcome == Outcome::RevertedPendingReboot => {
                OutcomeClass::RestartRequired
            }
            None => OutcomeClass::Done,
            Some(failure) if reverted_automatically(failure) => OutcomeClass::RevertedAutomatically,
            Some(_) => OutcomeClass::Failed,
        },
        Outcome::Recovered => {
            if !result.conflicts.is_empty() {
                OutcomeClass::Conflict
            } else if result.pending_action == Some(PendingAction::RestartPc) {
                OutcomeClass::RestartRequired
            } else {
                OutcomeClass::Done
            }
        }
    }
}

/// The class after the helper was lost and the caller recovered at once (design m2 E.7, review
/// C8), judged by where recovery left the interrupted operations (C.7 rolls forward as well as
/// back): a conflict; any operation now waiting for the restart; any waiting for keep / revert;
/// operations put back (an automatic revert); one that never wrote anything (failed); a restore
/// written through to `Confirmed` (done). When nothing was recovered, the recovery's own class.
pub fn classify_lost_recovery(result: &OperationResult) -> OutcomeClass {
    let ended = |states: &[OpState]| result.recovered.iter().any(|op| states.contains(&op.to));
    if !result.conflicts.is_empty() || ended(&[OpState::Conflict]) {
        OutcomeClass::Conflict
    } else if result.recovered.is_empty() {
        classify_result(result)
    } else if ended(&[OpState::PendingReboot]) {
        OutcomeClass::RestartRequired
    } else if ended(&[OpState::AwaitingConfirm]) {
        OutcomeClass::AwaitingConfirm
    } else if ended(&[OpState::Reverted, OpState::RevertedPendingReboot]) {
        OutcomeClass::RevertedAutomatically
    } else if ended(&[OpState::Failed]) {
        OutcomeClass::Failed
    } else {
        OutcomeClass::Done
    }
}

/// The reasons design m2 F.5 calls "reverted automatically", as opposed to errors.
pub fn reverted_automatically(failure: &FailureReason) -> bool {
    matches!(
        failure,
        FailureReason::CountdownExpired
            | FailureReason::KeyboardDidNotReturn
            | FailureReason::LiveResetUnconfirmed
            | FailureReason::CallerDisconnected
    )
}

/// The class of a request the engine refused or failed: blocked when something else holds the
/// way (the lock, an open operation, a needed recovery), cancelled when the caller went away
/// before anything was written, failed otherwise (nothing was written).
pub fn classify_error(code: ErrorCode) -> OutcomeClass {
    match code {
        ErrorCode::Busy | ErrorCode::OpInProgress | ErrorCode::RecoveryNeeded => {
            OutcomeClass::Blocked
        }
        ErrorCode::Cancelled => OutcomeClass::Cancelled,
        ErrorCode::JournalUnreadable
        | ErrorCode::PlanRejected
        | ErrorCode::PlanChanged
        | ErrorCode::MigrationRequired
        | ErrorCode::NotFixedMode
        | ErrorCode::UnknownKeyboard
        | ErrorCode::UnknownOp
        | ErrorCode::NotLatest
        | ErrorCode::InvalidState
        | ErrorCode::InventoryIncomplete
        | ErrorCode::RecoveryAssetsUnavailable
        | ErrorCode::Registry
        | ErrorCode::Device
        | ErrorCode::Host
        | ErrorCode::Protocol
        | ErrorCode::Internal => OutcomeClass::Failed,
    }
}

/// What the helper's exit code (design m2 E.8) means, when it exited before answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelperExitKind {
    /// 0: ended the session (`Bye`, a closed pipe, the idle timeout).
    Ended,
    /// 1: any other failure.
    Failed,
    /// 2: its command line was not the fixed format.
    BadCommandLine,
    /// 3: connection or handshake failed (PID, nonce, protocol version, build ID, timeout).
    HandshakeFailed,
    /// 4: not elevated.
    NotElevated,
    /// 5: unsupported Windows (build 26100 or later is needed).
    UnsupportedOs,
    /// Any other code.
    Unexpected,
}

/// A helper exit code with its meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HelperExit(pub u32);

impl HelperExit {
    pub fn kind(self) -> HelperExitKind {
        match self.0 {
            0 => HelperExitKind::Ended,
            1 => HelperExitKind::Failed,
            2 => HelperExitKind::BadCommandLine,
            3 => HelperExitKind::HandshakeFailed,
            4 => HelperExitKind::NotElevated,
            5 => HelperExitKind::UnsupportedOs,
            _ => HelperExitKind::Unexpected,
        }
    }
}

impl fmt::Display for HelperExit {
    /// `the helper <meaning> (exit code N)` (English, for logs and the CLI).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let meaning = match self.kind() {
            HelperExitKind::Ended => "ended the session",
            HelperExitKind::Failed => "failed",
            HelperExitKind::BadCommandLine => "refused its command line",
            HelperExitKind::HandshakeFailed => {
                "could not verify the connection (PID, nonce, protocol version or build ID)"
            }
            HelperExitKind::NotElevated => "is not running as administrator",
            HelperExitKind::UnsupportedOs => {
                "does not support this version of Windows (build 26100 or later is needed)"
            }
            HelperExitKind::Unexpected => "stopped unexpectedly",
        };
        write!(f, "the helper {meaning} (exit code {})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use mklm_core::{OpId, RecoveredOp};

    use super::*;

    fn result(outcome: Outcome, failure: Option<FailureReason>) -> OperationResult {
        OperationResult {
            op_id: None,
            outcome,
            failure,
            pending_action: None,
            conflicts: Vec::new(),
            inv_ps2_violation: None,
            recovered: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn results() {
        use OutcomeClass as C;
        let class = |outcome, failure| classify_result(&result(outcome, failure));
        assert_eq!(class(Outcome::Confirmed, None), C::Done);
        assert_eq!(class(Outcome::AwaitingConfirm, None), C::AwaitingConfirm);
        assert_eq!(class(Outcome::PendingReboot, None), C::RestartRequired);
        assert_eq!(
            class(Outcome::Reverted, Some(FailureReason::CountdownExpired)),
            C::RevertedAutomatically
        );
        assert_eq!(
            class(Outcome::Reverted, Some(FailureReason::Interrupted)),
            C::Failed
        );
        assert_eq!(
            class(Outcome::Failed, Some(FailureReason::ConflictKeptCurrent)),
            C::Done
        );
        assert_eq!(classify_error(ErrorCode::Busy), C::Blocked);
        assert_eq!(classify_error(ErrorCode::Cancelled), C::Cancelled);
        assert_eq!(classify_error(ErrorCode::PlanChanged), C::Failed);
    }

    #[test]
    fn lost_recoveries() {
        let op = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        let mut recovered = result(Outcome::Recovered, None);
        recovered.recovered.push(RecoveredOp {
            op_id: op,
            from: OpState::Planned,
            to: OpState::Reverted,
            decision: "roll-back".into(),
        });
        assert_eq!(
            classify_lost_recovery(&recovered),
            OutcomeClass::RevertedAutomatically
        );
    }

    #[test]
    fn helper_exits() {
        assert_eq!(HelperExit(3).kind(), HelperExitKind::HandshakeFailed);
        assert!(HelperExit(3).to_string().contains("build ID"));
        assert_eq!(
            HelperExit(99).to_string(),
            "the helper stopped unexpectedly (exit code 99)"
        );
    }
}
