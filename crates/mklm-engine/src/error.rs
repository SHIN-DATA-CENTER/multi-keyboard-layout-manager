//! Errors of engine operations and their mapping to [`mklm_core::ErrorInfo`].

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use mklm_core::{
    ErrorInfo, ExpectedPlan, JournalError, OpId, OpState, OperationError, RestoreError,
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
}

impl EngineError {
    /// The report form (code, English message, plan error details).
    pub fn to_info(&self) -> ErrorInfo {
        todo!("M2: map to ErrorCode")
    }
}
