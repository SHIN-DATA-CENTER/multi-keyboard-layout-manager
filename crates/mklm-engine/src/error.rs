//! Errors of engine operations and their mapping to [`mklm_core::ErrorInfo`].

use mklm_core::{
    AllowlistError, ErrorCode, ErrorInfo, ExpectedPlan, JournalError, OpId, OpState,
    OperationError, PlanError, RestoreError, WriteTarget,
};

use crate::backend::BackendError;
use crate::device::DeviceError;
use crate::host::HostError;

/// An engine request failed. Whatever was written is either rolled back or left in a journaled
/// state that recovery handles; the error says which operation (if any) is affected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("another MKLM operation holds the write lock")]
    Busy,
    #[error("operation {op_id} is still open ({state:?})")]
    OpInProgress { op_id: OpId, state: OpState },
    #[error("run recovery first: {op_ids:?}")]
    RecoveryNeeded { op_ids: Vec<OpId> },
    #[error("the journal cannot be read: {0}")]
    JournalUnreadable(JournalError),
    #[error(transparent)]
    Operation(#[from] OperationError),
    #[error(transparent)]
    Restore(#[from] RestoreError),
    /// The engine's plan (steps or apply method) differs from what the user approved.
    #[error("the plan changed since it was shown")]
    PlanChanged { plan: ExpectedPlan },
    #[error("no operation {op_id}")]
    UnknownOp { op_id: String },
    #[error("operation {op_id} was overwritten by {later}; restore to baseline instead")]
    NotLatest { op_id: OpId, later: OpId },
    #[error("operation {op_id} is {state:?}; cannot {action}")]
    InvalidState {
        op_id: OpId,
        state: OpState,
        action: &'static str,
    },
    /// The recovery files could not be written durably before a change of boot-time values;
    /// nothing was journaled or written (design review C10).
    #[error("the offline recovery files could not be written: {0}")]
    RecoveryAssetsUnavailable(HostError),
    /// The caller went away before the operation was journaled; nothing was written.
    #[error("cancelled by the caller")]
    Cancelled,
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Host(#[from] HostError),
    /// A rule of the engine itself was broken (an invalid state transition, a journal entry that
    /// cannot be serialized). Never expected; reported as [`ErrorCode::Internal`].
    #[error("internal error: {0}")]
    Internal(String),
}

/// The allowlist form of a restore that names a value MKLM may not write.
fn restore_plan_error(error: &RestoreError) -> Option<PlanError> {
    match error {
        RestoreError::InvPs2(violation) => Some(PlanError::InvPs2(violation.clone())),
        RestoreError::NameNotAllowed { target, name } => {
            let error = AllowlistError::ValueNotAllowed { name: name.clone() };
            Some(match target {
                WriteTarget::Device { instance_id } => PlanError::Device {
                    instance_id: instance_id.clone(),
                    error,
                },
                WriteTarget::Global => PlanError::Global { error },
            })
        }
        RestoreError::NothingToCompare { .. } | RestoreError::NoResolution { .. } => None,
    }
}

impl EngineError {
    /// The report form (code, English message, plan error details).
    pub fn to_info(&self) -> ErrorInfo {
        let (code, op_id, plan_error) = match self {
            EngineError::Busy => (ErrorCode::Busy, None, None),
            EngineError::OpInProgress { op_id, .. } => {
                (ErrorCode::OpInProgress, Some(op_id.clone()), None)
            }
            EngineError::RecoveryNeeded { op_ids } => {
                (ErrorCode::RecoveryNeeded, op_ids.first().cloned(), None)
            }
            EngineError::JournalUnreadable(_) => (ErrorCode::JournalUnreadable, None, None),
            EngineError::Operation(error) => match error {
                OperationError::UnknownKeyboard { .. } => (ErrorCode::UnknownKeyboard, None, None),
                OperationError::MigrationRequired { .. } => {
                    (ErrorCode::MigrationRequired, None, None)
                }
                OperationError::NotFixedMode => (ErrorCode::NotFixedMode, None, None),
                OperationError::Plan(plan) => (ErrorCode::PlanRejected, None, Some(plan.clone())),
                OperationError::InconsistentGlobal
                | OperationError::StandardNotAllowed { .. }
                | OperationError::LayerDriverMissing { .. } => {
                    (ErrorCode::PlanRejected, None, None)
                }
            },
            EngineError::Restore(error) => {
                (ErrorCode::PlanRejected, None, restore_plan_error(error))
            }
            EngineError::PlanChanged { .. } => (ErrorCode::PlanChanged, None, None),
            EngineError::UnknownOp { .. } => (ErrorCode::UnknownOp, None, None),
            EngineError::NotLatest { op_id, .. } => {
                (ErrorCode::NotLatest, Some(op_id.clone()), None)
            }
            EngineError::InvalidState { op_id, .. } => {
                (ErrorCode::InvalidState, Some(op_id.clone()), None)
            }
            EngineError::RecoveryAssetsUnavailable(_) => {
                (ErrorCode::RecoveryAssetsUnavailable, None, None)
            }
            EngineError::Cancelled => (ErrorCode::Cancelled, None, None),
            EngineError::Backend(_) => (ErrorCode::Registry, None, None),
            EngineError::Device(DeviceError::Incomplete { .. }) => {
                (ErrorCode::InventoryIncomplete, None, None)
            }
            EngineError::Device(_) => (ErrorCode::Device, None, None),
            EngineError::Host(HostError::Busy) => (ErrorCode::Busy, None, None),
            EngineError::Host(_) => (ErrorCode::Host, None, None),
            EngineError::Internal(_) => (ErrorCode::Internal, None, None),
        };
        ErrorInfo {
            code,
            message: self.to_string(),
            op_id,
            plan_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mklm_core::InvPs2Violation;

    fn op() -> OpId {
        OpId::parse("0000000a-0000-4000-8000-000000000000").unwrap()
    }

    #[test]
    fn codes() {
        let violation = InvPs2Violation {
            keyboards: vec![r"ACPI\PNP0303\0".into()],
        };
        let cases: Vec<(EngineError, ErrorCode)> = vec![
            (EngineError::Busy, ErrorCode::Busy),
            (EngineError::Host(HostError::Busy), ErrorCode::Busy),
            (
                EngineError::OpInProgress {
                    op_id: op(),
                    state: OpState::PendingReboot,
                },
                ErrorCode::OpInProgress,
            ),
            (
                EngineError::RecoveryNeeded { op_ids: vec![op()] },
                ErrorCode::RecoveryNeeded,
            ),
            (
                EngineError::JournalUnreadable(JournalError::NewerSchema {
                    found: 2,
                    supported: 1,
                }),
                ErrorCode::JournalUnreadable,
            ),
            (
                OperationError::MigrationRequired { fixed: None }.into(),
                ErrorCode::MigrationRequired,
            ),
            (OperationError::NotFixedMode.into(), ErrorCode::NotFixedMode),
            (
                OperationError::UnknownKeyboard {
                    instance_id: "x".into(),
                }
                .into(),
                ErrorCode::UnknownKeyboard,
            ),
            (
                OperationError::LayerDriverMissing {
                    dll: "kbd101.dll".into(),
                }
                .into(),
                ErrorCode::PlanRejected,
            ),
            (
                RestoreError::InvPs2(violation.clone()).into(),
                ErrorCode::PlanRejected,
            ),
            (
                EngineError::PlanChanged {
                    plan: ExpectedPlan {
                        steps: Vec::new(),
                        apply: None,
                    },
                },
                ErrorCode::PlanChanged,
            ),
            (
                EngineError::UnknownOp { op_id: "x".into() },
                ErrorCode::UnknownOp,
            ),
            (
                EngineError::NotLatest {
                    op_id: op(),
                    later: op(),
                },
                ErrorCode::NotLatest,
            ),
            (
                EngineError::InvalidState {
                    op_id: op(),
                    state: OpState::Confirmed,
                    action: "revert",
                },
                ErrorCode::InvalidState,
            ),
            (
                EngineError::RecoveryAssetsUnavailable(HostError::Os {
                    what: "x".into(),
                    code: 5,
                }),
                ErrorCode::RecoveryAssetsUnavailable,
            ),
            (EngineError::Cancelled, ErrorCode::Cancelled),
            (BackendError::Crashed.into(), ErrorCode::Registry),
            (
                DeviceError::Incomplete { issues: Vec::new() }.into(),
                ErrorCode::InventoryIncomplete,
            ),
            (DeviceError::Injected.into(), ErrorCode::Device),
            (
                HostError::Os {
                    what: "x".into(),
                    code: 1,
                }
                .into(),
                ErrorCode::Host,
            ),
            (EngineError::Internal("x".into()), ErrorCode::Internal),
        ];
        for (error, code) in cases {
            let info = error.to_info();
            assert_eq!(info.code, code, "{error:?}");
            assert_eq!(info.message, error.to_string());
        }
        let info = EngineError::from(RestoreError::InvPs2(violation.clone())).to_info();
        assert_eq!(info.plan_error, Some(PlanError::InvPs2(violation)));
        let info = EngineError::from(RestoreError::NameNotAllowed {
            target: WriteTarget::Global,
            name: "Start".into(),
        })
        .to_info();
        assert_eq!(
            info.plan_error,
            Some(PlanError::Global {
                error: AllowlistError::ValueNotAllowed {
                    name: "Start".into()
                }
            })
        );
        let info = EngineError::InvalidState {
            op_id: op(),
            state: OpState::Confirmed,
            action: "revert",
        }
        .to_info();
        assert_eq!(info.op_id, Some(op()));
    }
}
