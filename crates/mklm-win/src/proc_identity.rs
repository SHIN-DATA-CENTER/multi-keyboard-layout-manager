//! Process identity and liveness (M2): the journal owner check of plan 2.3 and the pipe peer's
//! image check.
//!
//! No process handle is opened. A normal process's DACL grants nothing to Administrators, so
//! `OpenProcess` fails when the helper runs as another user (a standard user who typed an
//! administrator's credentials into UAC; design review S3). Everything here reads the system
//! process table instead, which works across users and integrity levels:
//! - `NtQuerySystemInformation(SystemProcessInformation)`: PID and `CreateTime` of every process;
//! - `NtQuerySystemInformation(SystemProcessIdInformation)`: the image name of one PID, as an NT
//!   path (`\Device\HarddiskVolume3\…`). Paths are compared in that NT form on both sides, never
//!   converted to drive letters.

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use mklm_core::{Liveness, ProcessIdentity};

use crate::error::Error;

/// This process's PID and creation time (`GetProcessTimes` on the pseudo handle; the same
/// `CreateTime` that `SystemProcessInformation` reports).
pub fn current_process_identity() -> Result<ProcessIdentity, Error> {
    todo!("M2")
}

/// From `SystemProcessInformation`: a process with the same PID and creation time →
/// [`Liveness::Alive`]; none → [`Liveness::Dead`]; the query itself failed →
/// [`Liveness::Unknown`].
pub fn process_liveness(process: &ProcessIdentity) -> Liveness {
    todo!("M2")
}

/// NT image path of a running process (`SystemProcessIdInformation`), e.g.
/// `\Device\HarddiskVolume3\Program Files\MKLM\mklm-cli.exe`.
pub fn process_image_nt_path(pid: u32) -> Result<String, Error> {
    todo!("M2")
}

/// True when `pid`'s image is in the same directory as this process's image, both taken from
/// [`process_image_nt_path`] and compared case-insensitively (the helper's check of its caller,
/// design E.2).
pub fn same_image_directory(pid: u32) -> Result<bool, Error> {
    todo!("M2")
}
