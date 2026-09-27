//! Live reset of one keyboard devnode (M2, plan 1.4) and its state while it comes back.
//!
//! The reset is what `pnputil /restart-device` does (verified for USB in M0 #2b):
//! `SetupDiCreateDeviceInfoList(GUID_DEVCLASS_KEYBOARD)` → `SetupDiOpenDeviceInfoW(instance ID)` →
//! `SetupDiSetClassInstallParamsW(SP_PROPCHANGE_PARAMS { DIF_PROPERTYCHANGE, DICS_PROPCHANGE,
//! DICS_FLAG_CONFIGSPECIFIC, HwProfile 0 })` → `SetupDiCallClassInstaller(DIF_PROPERTYCHANGE)` →
//! `SetupDiGetDeviceInstallParamsW`: `DI_NEEDREBOOT` or `DI_NEEDRESTART` in `Flags` means the
//! change needs a PC restart. Callers check `mklm_core::live_reset_bans` first; this module does
//! not repeat the bans.

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use crate::error::Error;

/// Result of [`restart_device`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestartResult {
    Restarted,
    /// `DI_NEEDREBOOT` / `DI_NEEDRESTART` was set.
    NeedsReboot,
}

/// Restarts one Keyboard-class devnode in place. Synchronous and without a deadline of its own
/// (`SetupDiCallClassInstaller` can block while PnP is busy): `mklm_engine::win::WinDevices` calls
/// it on a worker thread with a deadline and keeps the process alive until it returns
/// (design review C18).
pub fn restart_device(instance_id: &str) -> Result<RestartResult, Error> {
    todo!("M2")
}

/// A devnode's state, for polling its re-arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DevNodeState {
    pub present: bool,
    /// `DN_*` flags (`CM_Get_DevNode_Status`), when present.
    pub status: Option<u32>,
    /// `CM_PROB_*`, when present.
    pub problem: Option<u32>,
}

/// Reads [`DevNodeState`] (`CM_Locate_DevNodeW` without the phantom flag, then
/// `CM_Get_DevNode_Status`).
pub fn devnode_state(instance_id: &str) -> Result<DevNodeState, Error> {
    todo!("M2")
}
