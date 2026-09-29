//! Exit codes of the write commands (design F.5): the outcome classes of `mklm_client::outcome`
//! (shared with the GUI, design m3 A.2) mapped onto numbers.

use mklm_client::OutcomeClass;
use mklm_core::{ErrorCode, OperationResult};

use super::exit_code;

/// The exit code of an outcome class (design F.5).
pub fn class_exit_code(class: OutcomeClass) -> i32 {
    match class {
        OutcomeClass::Done => exit_code::OK,
        OutcomeClass::Failed => exit_code::FAILURE,
        OutcomeClass::Cancelled => exit_code::CANCELLED,
        OutcomeClass::RevertedAutomatically => exit_code::REVERTED,
        OutcomeClass::Conflict => exit_code::CONFLICT,
        OutcomeClass::Blocked => exit_code::BLOCKED,
        OutcomeClass::AwaitingConfirm => exit_code::AWAITING_CONFIRM,
        OutcomeClass::RestartRequired => exit_code::RESTART_REQUIRED,
    }
}

/// The exit code of a request that ended with a result (`mklm_client::outcome::classify_result`):
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
    class_exit_code(mklm_client::outcome::classify_result(result))
}

/// The exit code after the helper was lost and the CLI recovered at once (design E.7, review
/// C8), judged by where recovery left the interrupted operations
/// (`mklm_client::outcome::classify_lost_recovery`).
pub fn lost_recovery_exit_code(result: &OperationResult) -> i32 {
    class_exit_code(mklm_client::outcome::classify_lost_recovery(result))
}

/// The exit code of a request the engine refused or failed: 6 when something else holds the way
/// (the lock, an open operation, a needed recovery), 3 when the caller went away before anything
/// was written, 1 otherwise (nothing was written).
pub fn error_exit_code(code: ErrorCode) -> i32 {
    class_exit_code(mklm_client::outcome::classify_error(code))
}

#[cfg(test)]
mod tests {
    use mklm_core::{ConflictInfo, FailureReason, OpId, OpState, Outcome, PendingAction, RegValue};

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
                target: None,
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
    }
}
