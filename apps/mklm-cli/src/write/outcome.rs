//! Exit codes of the write commands (design F.5).

use mklm_core::{ErrorCode, FailureReason, OpState, OperationResult, Outcome, PendingAction};

use super::exit_code;

/// The exit code of a request that ended with a result.
///
/// - `NoChange`, `Confirmed`: 0. A resolution that kept outside values (`Failed` with
///   `ConflictKeptCurrent`) did what the user chose: 0.
/// - `AwaitingConfirm`: 10 (written; waiting for a reconnect and `keep` / `revert`).
/// - `PendingReboot`: 3010.
/// - `Reverted` / `RevertedPendingReboot` as requested (no `failure`): 0 / 3010. Reverted
///   automatically (countdown expired, keyboard did not come back, the change was never kept, the
///   caller left): 4. Rolled back after an error or an interruption: 1.
/// - `Failed` otherwise: 1. `Conflict`: 5.
/// - `Recovered` (`recover`, `undo`): 5 with conflicts left, 3010 when a restart is needed, else 0.
pub fn result_exit_code(result: &OperationResult) -> i32 {
    match result.outcome {
        Outcome::NoChange | Outcome::Confirmed => exit_code::OK,
        Outcome::AwaitingConfirm => exit_code::AWAITING_CONFIRM,
        Outcome::PendingReboot => exit_code::RESTART_REQUIRED,
        Outcome::Conflict => exit_code::CONFLICT,
        Outcome::Failed => match result.failure {
            Some(FailureReason::ConflictKeptCurrent) => exit_code::OK,
            _ => exit_code::FAILURE,
        },
        Outcome::Reverted | Outcome::RevertedPendingReboot => match &result.failure {
            None if result.outcome == Outcome::RevertedPendingReboot => exit_code::RESTART_REQUIRED,
            None => exit_code::OK,
            Some(failure) if reverted_automatically(failure) => exit_code::REVERTED,
            Some(_) => exit_code::FAILURE,
        },
        Outcome::Recovered => {
            if !result.conflicts.is_empty() {
                exit_code::CONFLICT
            } else if result.pending_action == Some(PendingAction::RestartPc) {
                exit_code::RESTART_REQUIRED
            } else {
                exit_code::OK
            }
        }
    }
}

/// The exit code after the helper was lost and the CLI recovered at once (design E.7, review
/// C8), judged by where recovery left the interrupted operations (C.7 rolls forward as well as
/// back): a conflict 5; any operation now waiting for the restart 3010; any waiting for
/// `keep` / `revert` 10; operations put back (`Reverted`, `RevertedPendingReboot`) 4, as an
/// automatic revert; one that never wrote anything (`Failed`) 1; a restore written through to
/// `Confirmed` 0. When nothing was recovered, the recovery's own code.
pub fn lost_recovery_exit_code(result: &OperationResult) -> i32 {
    let ended = |states: &[OpState]| result.recovered.iter().any(|op| states.contains(&op.to));
    if !result.conflicts.is_empty() || ended(&[OpState::Conflict]) {
        exit_code::CONFLICT
    } else if result.recovered.is_empty() {
        result_exit_code(result)
    } else if ended(&[OpState::PendingReboot]) {
        exit_code::RESTART_REQUIRED
    } else if ended(&[OpState::AwaitingConfirm]) {
        exit_code::AWAITING_CONFIRM
    } else if ended(&[OpState::Reverted, OpState::RevertedPendingReboot]) {
        exit_code::REVERTED
    } else if ended(&[OpState::Failed]) {
        exit_code::FAILURE
    } else {
        exit_code::OK
    }
}

/// The reasons F.5 calls "reverted automatically" (exit code 4), as opposed to errors (1).
fn reverted_automatically(failure: &FailureReason) -> bool {
    matches!(
        failure,
        FailureReason::CountdownExpired
            | FailureReason::KeyboardDidNotReturn
            | FailureReason::LiveResetUnconfirmed
            | FailureReason::CallerDisconnected
    )
}

/// The exit code of a request the engine refused or failed: 6 when something else holds the way
/// (the lock, an open operation, a needed recovery), 3 when the caller went away before anything
/// was written, 1 otherwise (nothing was written).
pub fn error_exit_code(code: ErrorCode) -> i32 {
    match code {
        ErrorCode::Busy | ErrorCode::OpInProgress | ErrorCode::RecoveryNeeded => exit_code::BLOCKED,
        ErrorCode::Cancelled => exit_code::CANCELLED,
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
        | ErrorCode::Internal => exit_code::FAILURE,
    }
}

/// What the helper's exit code (design E.8) means, when it exited before answering.
pub fn helper_exit_text(code: u32) -> String {
    let meaning = match code {
        0 => "ended the session",
        1 => "failed",
        2 => "refused its command line",
        3 => "could not verify the connection (PID, nonce, protocol version or build ID)",
        4 => "is not running as administrator",
        5 => "does not support this version of Windows (build 26100 or later is needed)",
        _ => "stopped unexpectedly",
    };
    format!("the helper {meaning} (exit code {code})")
}

#[cfg(test)]
mod tests {
    use mklm_core::{ConflictInfo, OpId, RegValue};

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

    fn op() -> OpId {
        OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap()
    }

    #[test]
    fn outcomes_map_to_section_f5() {
        let code = |outcome, failure| result_exit_code(&result(outcome, failure));
        assert_eq!(code(Outcome::NoChange, None), 0);
        assert_eq!(code(Outcome::Confirmed, None), 0);
        assert_eq!(code(Outcome::AwaitingConfirm, None), 10);
        assert_eq!(code(Outcome::PendingReboot, None), 3010);
        assert_eq!(code(Outcome::Conflict, None), 5);
        assert_eq!(code(Outcome::Reverted, None), 0);
        assert_eq!(code(Outcome::RevertedPendingReboot, None), 3010);
        for automatic in [
            FailureReason::CountdownExpired,
            FailureReason::KeyboardDidNotReturn,
            FailureReason::LiveResetUnconfirmed,
            FailureReason::CallerDisconnected,
        ] {
            assert_eq!(code(Outcome::Reverted, Some(automatic.clone())), 4);
            assert_eq!(code(Outcome::RevertedPendingReboot, Some(automatic)), 4);
        }
        for error in [
            FailureReason::WriteError {
                message: "denied".into(),
            },
            FailureReason::Interrupted,
        ] {
            assert_eq!(code(Outcome::Reverted, Some(error.clone())), 1);
            assert_eq!(code(Outcome::RevertedPendingReboot, Some(error)), 1);
        }
        assert_eq!(
            code(
                Outcome::Failed,
                Some(FailureReason::ConcurrentChange {
                    name: "KeyboardTypeOverride".into()
                })
            ),
            1
        );
        assert_eq!(
            code(Outcome::Failed, Some(FailureReason::NothingWritten)),
            1
        );
        assert_eq!(
            code(Outcome::Failed, Some(FailureReason::ConflictKeptCurrent)),
            0
        );
        assert_eq!(
            code(
                Outcome::Failed,
                Some(FailureReason::Superseded { by: op() })
            ),
            1
        );
    }

    #[test]
    fn recovery_results() {
        let mut recovered = result(Outcome::Recovered, None);
        assert_eq!(result_exit_code(&recovered), 0);
        recovered.pending_action = Some(PendingAction::Reconnect);
        assert_eq!(result_exit_code(&recovered), 0);
        recovered.pending_action = Some(PendingAction::RestartPc);
        assert_eq!(result_exit_code(&recovered), 3010);
        recovered.conflicts.push(ConflictInfo {
            op_id: op(),
            record: 0,
            key_path: "Services\\i8042prt\\Parameters".into(),
            name: "OverrideKeyboardType".into(),
            baseline: RegValue::Absent,
            before: RegValue::Absent,
            intended: RegValue::Dword { value: 7 },
            last_written: None,
            current: RegValue::Dword { value: 4 },
            write_error: None,
        });
        assert_eq!(result_exit_code(&recovered), 5);
    }

    #[test]
    fn recovery_after_a_lost_helper_reports_where_the_operation_ended() {
        let with = |ends: &[OpState]| {
            let mut result = result(Outcome::Recovered, None);
            result.recovered = ends
                .iter()
                .map(|to| mklm_core::RecoveredOp {
                    op_id: op(),
                    from: OpState::Written,
                    to: *to,
                    decision: "test".into(),
                })
                .collect();
            lost_recovery_exit_code(&result)
        };
        // Rolled forward (C.7): the change waits for the restart or for keep / revert.
        assert_eq!(with(&[OpState::PendingReboot]), 3010);
        assert_eq!(with(&[OpState::AwaitingConfirm]), 10);
        // Put back: an automatic revert.
        assert_eq!(with(&[OpState::Reverted]), 4);
        assert_eq!(with(&[OpState::RevertedPendingReboot]), 4);
        assert_eq!(with(&[OpState::Failed]), 1);
        assert_eq!(with(&[OpState::Conflict]), 5);
        assert_eq!(with(&[OpState::Confirmed]), 0);
        assert_eq!(with(&[OpState::Reverted, OpState::PendingReboot]), 3010);
        // Nothing recovered: the recovery's own code.
        assert_eq!(with(&[]), 0);
    }

    #[test]
    fn errors_map_to_section_f5() {
        for blocked in [
            ErrorCode::Busy,
            ErrorCode::OpInProgress,
            ErrorCode::RecoveryNeeded,
        ] {
            assert_eq!(error_exit_code(blocked), 6);
        }
        assert_eq!(error_exit_code(ErrorCode::Cancelled), 3);
        for failed in [
            ErrorCode::PlanRejected,
            ErrorCode::PlanChanged,
            ErrorCode::MigrationRequired,
            ErrorCode::InventoryIncomplete,
            ErrorCode::RecoveryAssetsUnavailable,
            ErrorCode::JournalUnreadable,
            ErrorCode::Internal,
        ] {
            assert_eq!(error_exit_code(failed), 1);
        }
        assert!(helper_exit_text(3).contains("build ID"));
        assert!(helper_exit_text(99).contains("exit code 99"));
    }
}
