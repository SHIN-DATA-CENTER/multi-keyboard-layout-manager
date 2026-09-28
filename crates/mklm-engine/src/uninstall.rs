//! The silent restore run by the uninstaller (M5; design m2 C.12, C14): `mklm-helper.exe
//! --uninstall-restore` puts every value MKLM changed back to its baseline, skipping conflicts,
//! without resetting a keyboard and without asking anything, then maps the result to the exit
//! code the uninstaller reads.

use mklm_core::{
    ApplyOptions, ConflictPolicy, OperationResult, Outcome, PendingAction, RestoreScope,
};

use crate::{EngineError, RestoreBaselineParams, RestoreMode};

/// Some restored values take effect only after the PC restarts (`ERROR_SUCCESS_REBOOT_REQUIRED`,
/// the code installers use for it).
pub const EXIT_REBOOT_REQUIRED: u32 = 3010;
/// Another MKLM process holds the write lock.
pub const EXIT_BUSY: u32 = 6;
/// Anything else: the values stay as they are; the journal and the Recovery files remain.
pub const EXIT_FAILED: u32 = 1;

/// The parameters of the uninstall restore: everything, conflicts skipped, silent, no reset.
pub fn params() -> RestoreBaselineParams {
    RestoreBaselineParams {
        scope: RestoreScope::All,
        on_conflict: ConflictPolicy::Skip,
        mode: RestoreMode::Silent,
        apply: ApplyOptions {
            allow_live_reset: false,
            other_input_available: false,
            ..ApplyOptions::default()
        },
    }
}

/// The exit code for the engine's result:
/// - 0: nothing to restore, or the values are back (keyboards that need a reconnect pick them up
///   on their next reconnect; nobody is left to ask after the uninstall);
/// - [`EXIT_REBOOT_REQUIRED`]: back, but some take effect only after a PC restart;
/// - [`EXIT_BUSY`], [`EXIT_FAILED`].
pub fn exit_code(result: &Result<OperationResult, EngineError>) -> u32 {
    match result {
        Ok(result) => match result.outcome {
            Outcome::PendingReboot | Outcome::RevertedPendingReboot => EXIT_REBOOT_REQUIRED,
            Outcome::NoChange | Outcome::Confirmed
                if result.pending_action == Some(PendingAction::RestartPc) =>
            {
                EXIT_REBOOT_REQUIRED
            }
            Outcome::NoChange | Outcome::Confirmed => 0,
            _ => EXIT_FAILED,
        },
        Err(EngineError::Busy) => EXIT_BUSY,
        Err(_) => EXIT_FAILED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(outcome: Outcome, pending_action: Option<PendingAction>) -> OperationResult {
        OperationResult {
            op_id: None,
            outcome,
            failure: None,
            pending_action,
            conflicts: Vec::new(),
            inv_ps2_violation: None,
            recovered: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn the_restore_is_silent_and_never_resets() {
        let params = params();
        assert_eq!(params.mode, RestoreMode::Silent);
        assert_eq!(params.on_conflict, ConflictPolicy::Skip);
        assert_eq!(params.scope, RestoreScope::All);
        assert!(!params.apply.allow_live_reset);
        assert!(!params.apply.other_input_available);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(&Ok(result(Outcome::NoChange, None))), 0);
        assert_eq!(exit_code(&Ok(result(Outcome::Confirmed, None))), 0);
        assert_eq!(
            exit_code(&Ok(result(
                Outcome::Confirmed,
                Some(PendingAction::Reconnect)
            ))),
            0
        );
        assert_eq!(
            exit_code(&Ok(result(
                Outcome::Confirmed,
                Some(PendingAction::RestartPc)
            ))),
            EXIT_REBOOT_REQUIRED
        );
        assert_eq!(
            exit_code(&Ok(result(Outcome::PendingReboot, None))),
            EXIT_REBOOT_REQUIRED
        );
        assert_eq!(exit_code(&Ok(result(Outcome::Conflict, None))), EXIT_FAILED);
        assert_eq!(exit_code(&Err(EngineError::Busy)), EXIT_BUSY);
    }
}
