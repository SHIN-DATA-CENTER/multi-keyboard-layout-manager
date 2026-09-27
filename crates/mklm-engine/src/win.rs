//! The engine's traits on the real system, through `mklm-win` (Windows only).
//!
//! Construct them only in an elevated process (`mklm-helper`, or `mklm-cli` when
//! `mklm_win::elevation::is_elevated()`): opening the keys for writing fails otherwise.

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(unused_variables, dead_code)]

use std::time::Duration;

use mklm_core::{
    BootId, KeyboardType, Liveness, OpId, ProcessIdentity, RecoveryAssets, RegValue, Timestamp,
    WriteTarget,
};
use mklm_win::journal_store::JournalStore;
use mklm_win::protected_dir::{FileLock, ProtectedDir};

use crate::backend::{BackendError, JournalDump, JournalSlot, RegistryBackend};
use crate::device::{Arrival, DeviceController, DeviceError, Inventory, RestartOutcome};
use crate::host::{Host, HostError};

/// [`RegistryBackend`] over `mklm_win::regwrite` and `mklm_win::journal_store`. Opens each key per
/// call (no cached handles survive a devnode removal).
#[derive(Debug)]
pub struct WinRegistry {
    journal: Option<JournalStore>,
}

impl WinRegistry {
    pub fn new() -> Self {
        Self { journal: None }
    }
}

impl Default for WinRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RegistryBackend for WinRegistry {
    fn read_value(&self, target: &WriteTarget, name: &str) -> Result<RegValue, BackendError> {
        todo!("M2")
    }

    fn list_values(&self, target: &WriteTarget) -> Result<Vec<(String, RegValue)>, BackendError> {
        todo!("M2")
    }

    fn write_value(
        &mut self,
        target: &WriteTarget,
        name: &str,
        value: &RegValue,
    ) -> Result<(), BackendError> {
        todo!("M2")
    }

    fn flush_target(&mut self, target: &WriteTarget) -> Result<(), BackendError> {
        todo!("M2")
    }

    fn read_journal(&self) -> Result<JournalDump, BackendError> {
        todo!("M2")
    }

    fn write_journal(&mut self, slot: &JournalSlot, json: &str) -> Result<(), BackendError> {
        todo!("M2")
    }

    fn delete_journal(&mut self, slot: &JournalSlot) -> Result<(), BackendError> {
        todo!("M2")
    }

    fn flush_journal(&mut self) -> Result<(), BackendError> {
        todo!("M2")
    }
}

/// [`DeviceController`] over `mklm_win::devices`, `mklm_win::rawinfo` and `mklm_win::devctl`.
///
/// `restart` runs `devctl::restart_device` on a worker thread and waits up to
/// `EngineConfig::restart_timeout` ([`RestartOutcome::TimedOut`]); a worker that is still running
/// is kept in `pending_restarts`, and [`WinDevices::join_pending`] (called by the helper before it
/// exits) waits for it, so that a process exit never cuts a class-installer call short
/// (design review C18).
#[derive(Debug, Default)]
pub struct WinDevices {
    /// Poll interval while waiting for a keyboard to come back.
    poll: Option<Duration>,
    /// Deadline of one restart.
    restart_timeout: Option<Duration>,
    pending_restarts: Vec<std::thread::JoinHandle<()>>,
}

impl WinDevices {
    /// Waits up to `timeout` for restart workers that outlived their deadline. Returns false when
    /// one is still running (the helper logs it and still waits before exiting).
    pub fn join_pending(&mut self, timeout: Duration) -> bool {
        todo!("M2")
    }
}

impl DeviceController for WinDevices {
    fn keyboards(&mut self) -> Result<Inventory, DeviceError> {
        todo!(
            "M2: read_keyboards(true, raw, issues); issues whose kind blocks writes → Incomplete, \
             the others → warnings"
        )
    }

    fn restart(&mut self, instance_id: &str) -> Result<RestartOutcome, DeviceError> {
        todo!("M2: worker thread + deadline")
    }

    fn wait_for_arrival(
        &mut self,
        instance_id: &str,
        timeout: Duration,
    ) -> Result<Arrival, DeviceError> {
        todo!("M2: poll devnode_state + raw_keyboards every 250 ms")
    }

    fn reported_type(&mut self, instance_id: &str) -> Result<Option<KeyboardType>, DeviceError> {
        todo!("M2")
    }
}

/// [`Host`] over `mklm_win::protected_dir`, `mklm_win::session` and `mklm_win::proc_identity`.
#[derive(Debug)]
pub struct WinHost {
    base: Option<ProtectedDir>,
}

impl WinHost {
    /// Validates (and creates) the protected base directory up front. A squatted directory is
    /// quarantined (`protected_dir::ensure_protected_dir`) and reported through
    /// [`Host::drain_warnings`] (design review S1).
    pub fn new() -> Result<Self, HostError> {
        todo!("M2")
    }
}

impl Host for WinHost {
    type Lock = FileLock;

    fn acquire_lock(&mut self, timeout: Duration) -> Result<Self::Lock, HostError> {
        todo!("M2")
    }

    fn now(&self) -> Timestamp {
        todo!("M2: SystemTime::now")
    }

    fn monotonic(&self) -> Duration {
        todo!("M2: Instant since construction")
    }

    fn new_op_id(&mut self) -> Result<OpId, HostError> {
        todo!("M2: session::new_uuid")
    }

    fn boot_id(&self) -> Result<BootId, HostError> {
        todo!("M2: session::boot_id")
    }

    fn boot_time_hint(&self) -> Option<u64> {
        todo!("M2: session::boot_time_hint")
    }

    fn current_process(&self) -> Result<ProcessIdentity, HostError> {
        todo!("M2")
    }

    fn liveness(&self, process: &ProcessIdentity) -> Liveness {
        todo!("M2: proc_identity::process_liveness")
    }

    fn system32_file_exists(&self, file_name: &str) -> Result<bool, HostError> {
        todo!("M2: GetSystemDirectoryW + plain file name + regular-file check")
    }

    fn write_recovery_assets(&mut self, assets: &RecoveryAssets) -> Result<(), HostError> {
        todo!("M2: ensure_protected_dir(Recovery) + durable replace_file ×3")
    }

    fn drain_warnings(&mut self) -> Vec<String> {
        todo!("M2")
    }
}
