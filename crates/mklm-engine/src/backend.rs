//! The registry as the engine sees it: values on a device's hardware key, on the global i8042prt
//! key and in the journal store, plus flushes (section B.1 of the design doc).
//!
//! Every call is one registry operation, so every call is atomic. Nothing is durable until the
//! matching flush returns: device and global values live in the SYSTEM hive
//! ([`RegistryBackend::flush_target`]), the journal in the SOFTWARE hive
//! ([`RegistryBackend::flush_journal`]). Mutating calls (writes, deletes and flushes) are what the
//! in-memory backend counts for crash injection.

use mklm_core::{OpId, RegValue, WriteTarget};

/// Where a journal record lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JournalSlot {
    /// `Journal\Ops\<op id>`: a [`mklm_core::JournalEntry`] as JSON.
    Op(OpId),
    /// `Journal\Baselines\<canonical value key>`: a [`mklm_core::BaselineRecord`] as JSON.
    Baseline(String),
}

/// The raw journal store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalDump {
    /// `Journal\StoreVersion`, when present.
    pub store_version: Option<u32>,
    /// `(value name, JSON)` under `Journal\Ops`.
    pub ops: Vec<(String, String)>,
    /// `(value name, JSON)` under `Journal\Baselines`.
    pub baselines: Vec<(String, String)>,
}

/// A registry call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// The key does not exist and may not be created (the global key, the journal store while
    /// reading).
    #[error("{what}: not found")]
    NotFound { what: String },
    /// The devnode no longer exists, not even as a phantom (removed in Device Manager). Never
    /// reported as [`RegValue::Absent`]: a restore then skips the record
    /// (`mklm_core::SkipReason::DeviceRemoved`) instead of failing forever (design review C3).
    #[error("{instance_id}: the device was removed")]
    DeviceRemoved { instance_id: String },
    /// The value name is not one MKLM writes on that key (`mklm_core::DEVICE_VALUE_NAMES`,
    /// `GLOBAL_VALUE_NAMES`, `MACHINE_SETTING_NAMES`). Both implementations enforce it, like
    /// `mklm_win::regwrite` and `mklm_win::machine_settings` do.
    #[error("{name:?}: not a value MKLM may write here")]
    NameNotAllowed { name: String },
    #[error("{what}: access denied")]
    AccessDenied { what: String },
    /// The key's owner or DACL failed validation (journal key).
    #[error("{what}: insecure ({reason})")]
    Insecure { what: String, reason: String },
    /// Any other OS error.
    #[error("{what}: error {code}")]
    Os { what: String, code: u32 },
    /// Fault injection: the process "died" at this call; every later call fails too.
    #[error("simulated crash")]
    Crashed,
    /// Fault injection: this call fails, later calls work.
    #[error("injected failure at mutating call {call}")]
    Injected { call: usize },
}

/// Engine-facing registry. Implementations: `memory::MemoryRegistry` (tests) and
/// `win::WinRegistry` (the real system through `mklm-win`).
///
/// Device targets are addressed by instance ID and opened only through
/// `CM_Open_DevNode_Key(CM_REGISTRY_HARDWARE)` (plan 1.2). Implementations must reject any target
/// that is not a Keyboard-class devnode.
pub trait RegistryBackend {
    /// Reads one value; [`RegValue::Absent`] when the value (or the devnode's "Device Parameters"
    /// key) does not exist; [`BackendError::DeviceRemoved`] when the devnode itself is gone.
    fn read_value(&self, target: &WriteTarget, name: &str) -> Result<RegValue, BackendError>;

    /// Every value of the key (plan 1.3 step 1 snapshot). Empty when the key does not exist.
    fn list_values(&self, target: &WriteTarget) -> Result<Vec<(String, RegValue)>, BackendError>;

    /// Writes one value; [`RegValue::Absent`] deletes it (deleting a missing value succeeds).
    /// Creates the device's "Device Parameters" key when missing, never the global key.
    /// Refuses names outside the value-name allowlist ([`BackendError::NameNotAllowed`]).
    fn write_value(
        &mut self,
        target: &WriteTarget,
        name: &str,
        value: &RegValue,
    ) -> Result<(), BackendError>;

    /// `RegFlushKey` on the target's key: everything written to its hive so far becomes durable.
    fn flush_target(&mut self, target: &WriteTarget) -> Result<(), BackendError>;

    /// Reads the whole journal store (readable unelevated as well).
    fn read_journal(&self) -> Result<JournalDump, BackendError>;

    /// Creates or replaces one journal record (one `RegSetValueExW`, hence atomic). Creates the
    /// journal keys with their protected DACL when missing, and sets `StoreVersion`.
    fn write_journal(&mut self, slot: &JournalSlot, json: &str) -> Result<(), BackendError>;

    /// Deletes one journal record (missing is fine).
    fn delete_journal(&mut self, slot: &JournalSlot) -> Result<(), BackendError>;

    /// `RegFlushKey` on the journal key (SOFTWARE hive).
    fn flush_journal(&mut self) -> Result<(), BackendError>;

    /// Writes one `REG_DWORD` under `mklm_core::MACHINE_SETTINGS_KEY` and flushes it (SOFTWARE
    /// hive), creating the key with the journal's protected DACL when missing (design m3 A.5,
    /// WP-E2). Refuses names outside `mklm_core::MACHINE_SETTING_NAMES`
    /// ([`BackendError::NameNotAllowed`]). Durable once it returns.
    fn write_machine_setting(&mut self, name: &str, value: u32) -> Result<(), BackendError>;
}
