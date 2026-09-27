//! Everything else the engine needs from its process and machine (section B.3 of the design doc).

use std::time::Duration;

use mklm_core::{BootId, Liveness, OpId, ProcessIdentity, RecoveryAssets, Timestamp};

/// A host call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// Another process holds the write lock and did not release it within the timeout.
    #[error("another MKLM process is writing")]
    Busy,
    /// A protected directory or file failed validation (owner, DACL, reparse point) and could not
    /// be quarantined (see `mklm_win::protected_dir`).
    #[error("{path}: insecure ({reason})")]
    Insecure { path: String, reason: String },
    #[error("{what}: error {code}")]
    Os { what: String, code: u32 },
}

/// Process- and machine-level services.
pub trait Host {
    /// Held while the engine writes; dropping it releases the lock.
    type Lock;

    /// Takes the exclusive write lock (`LockFileEx` on
    /// `%ProgramData%\SHIN DATA CENTER\MKLM\mklm.lock`, plan 2.2), waiting up to `timeout`.
    /// Every mutating engine method takes it first and holds it until its result is returned,
    /// countdowns included. Holding it is what makes every in-flight entry found in the journal an
    /// abandoned one (design review C3).
    fn acquire_lock(&mut self, timeout: Duration) -> Result<Self::Lock, HostError>;

    /// Wall clock, for display and pruning only.
    fn now(&self) -> Timestamp;

    /// Monotonic time since an arbitrary start. Bounds waits whose length the engine does not
    /// control (a countdown whose sink blocks too long, design review C18). The fake advances only
    /// when the test says so.
    fn monotonic(&self) -> Duration;

    /// A new random operation ID.
    fn new_op_id(&mut self) -> Result<OpId, HostError>;

    /// The current boot's ID (see [`BootId`]).
    fn boot_id(&self) -> Result<BootId, HostError>;

    /// Kernel boot time minus its bias, for the history's diagnostic field only.
    fn boot_time_hint(&self) -> Option<u64>;

    /// This process (the owner of the entries it writes).
    fn current_process(&self) -> Result<ProcessIdentity, HostError>;

    /// Whether `process` still runs (same PID and creation time). Informational under the lock.
    fn liveness(&self, process: &ProcessIdentity) -> Liveness;

    /// Plan 1.5: true when `file_name` (e.g. `kbd106.dll`) exists in System32 as a regular file.
    fn system32_file_exists(&self, file_name: &str) -> Result<bool, HostError>;

    /// Replaces the files in the protected `Recovery` directory **durably** (design review C10):
    /// for each file, write a temporary file, `FlushFileBuffers` it, keep the old file as
    /// `<name>.prev`, `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`, then `FlushFileBuffers` on
    /// the directory handle. Returns only once all three files are on disk.
    fn write_recovery_assets(&mut self, assets: &RecoveryAssets) -> Result<(), HostError>;

    /// Warnings the host produced on its own since the last call (e.g. a squatted data directory
    /// that was quarantined, design review S1). The engine appends them to the result.
    fn drain_warnings(&mut self) -> Vec<String>;
}
