//! The engine's traits on the real system, through `mklm-win` (Windows only; design B.1-B.3).
//!
//! Construct them only in an elevated process (`mklm-helper`, or `mklm-cli` when
//! `mklm_win::elevation::is_elevated()`): opening the keys for writing fails otherwise. The one
//! exception is [`RegistryBackend::read_journal`], which works unelevated (design C.1).
//!
//! - [`WinRegistry`]: device hardware keys through `mklm_win::regwrite` (CfgMgr32 only, Keyboard
//!   class only, value-name gate), `i8042prt\Parameters` (never created) and the journal store
//!   (`mklm_win::journal_store`, owner and DACL checked before every write).
//! - [`WinDevices`]: the inventory with its read issues routed by `ReadIssueKind::blocks_writes`
//!   (design review S2, C3), the live reset on a worker thread with a deadline (C18), and the
//!   devnode / Raw Input polling after it.
//! - [`WinHost`]: the `LockFileEx` write lock in the protected data directory (D.9, S1), clocks,
//!   operation IDs, the per-boot ID (C2), process identity and liveness without `OpenProcess`
//!   (S3), the System32 check of layout DLLs (S8) and the durable recovery files (C10).
//!
//! `mklm_win::Error` is mapped onto the engine's error types here: a devnode that no longer exists
//! (`CR_NO_SUCH_DEVNODE`, or its key deleted under an open handle) is
//! [`BackendError::DeviceRemoved`], never "value absent" (design review C3); `regwrite`'s name gate
//! is [`BackendError::NameNotAllowed`] (S9); a failed owner/DACL check, or a devnode that is not
//! a keyboard, is [`BackendError::Insecure`].

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mklm_core::{
    BASELINE_REG_FILE, BootId, DEVICE_VALUE_NAMES, GLOBAL_VALUE_NAMES, KeyboardDevice,
    KeyboardType, Liveness, OpId, ProcessIdentity, README_FILE, RESTORE_CMD_FILE,
    RESTORE_ON_UNINSTALL_VALUE, RecoveryAssets, RegValue, Timestamp, WriteTarget,
};
use mklm_win::devctl::{self, DeviceArrival, PendingRestart, RestartResult, TimedRestart};
use mklm_win::global::I8042PRT_PARAMETERS;
use mklm_win::journal_store::{self, JournalStore, JournalSubkey};
use mklm_win::machine_settings;
use mklm_win::protected_dir::{self, DataDir, FileLock, ProtectedDir};
use mklm_win::regwrite::{self, ReadOnlyKey, WritableKey};
use mklm_win::{Error as WinError, ReadIssue, ReadIssueKind, elevation, proc_identity, session};

use crate::backend::{BackendError, JournalDump, JournalSlot, RegistryBackend};
use crate::device::{Arrival, DeviceController, DeviceError, Inventory, RestartOutcome};
use crate::host::{Host, HostError};

/// `ERROR_FILE_NOT_FOUND`: a registry key or value does not exist.
const ERROR_FILE_NOT_FOUND: u32 = 2;
/// `ERROR_ACCESS_DENIED`.
const ERROR_ACCESS_DENIED: u32 = 5;
/// `ERROR_INVALID_DATA`: the code reported for data of an unexpected type or shape.
const ERROR_INVALID_DATA: u32 = 13;
/// `ERROR_KEY_DELETED`: the key behind an open handle was deleted (the devnode was removed).
const ERROR_KEY_DELETED: u32 = 1018;
/// `ERROR_CANCELLED`.
const ERROR_CANCELLED: u32 = 1223;
/// `ERROR_TIMEOUT`.
const ERROR_TIMEOUT: u32 = 1460;
/// SetupAPI `ERROR_NO_SUCH_DEVINST` (`SetupDiOpenDeviceInfoW` for an unknown instance ID).
const ERROR_NO_SUCH_DEVINST: u32 = 0xE000_020B;
/// CONFIGRET `CR_NO_SUCH_DEVNODE`: the devnode does not exist, not even as a phantom.
const CR_NO_SUCH_DEVNODE: u32 = 0x0000_000D;
/// CONFIGRET `CR_ACCESS_DENIED`.
const CR_ACCESS_DENIED: u32 = 0x0000_0033;

/// Deadline of one live reset when none was configured (`EngineConfig::restart_timeout`).
const DEFAULT_RESTART_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval of [`DeviceController::wait_for_arrival`] (design B.2).
const DEFAULT_POLL: Duration = Duration::from_millis(250);
/// Shortest poll interval [`WinDevices::with_poll_interval`] accepts.
const MIN_POLL: Duration = Duration::from_millis(10);
/// Raw Input can still list the interface of the stopped devnode, with its old type, for a moment
/// after the class installer returns (M0 #2b read it 3 s later). The first reading is taken only
/// after this much of the arrival timeout has passed, so that a stale entry is not mistaken for
/// "came back unchanged".
const ARRIVAL_SETTLE: Duration = Duration::from_secs(2);
/// Upper bound of one wait on the system clock, so that `Instant + Duration` cannot overflow
/// whatever timeout a caller passes (`Duration::MAX` means "until done").
const MAX_WAIT_SLICE: Duration = Duration::from_secs(60 * 60);
/// How long dropping a [`WinDevices`] waits for restart workers that outlived their deadline, as a
/// safety net for an owner that did not call [`WinDevices::join_pending`] (design review C18).
const DROP_JOIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Label of the journal store in errors.
const JOURNAL_SUBJECT: &str = r"HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal";
/// Label of the machine-wide settings key in errors.
const SETTINGS_SUBJECT: &str = r"HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Settings";
/// Labels of the protected data directory and its files in errors.
const BASE_DIR_SUBJECT: &str = r"%ProgramData%\SHIN DATA CENTER\MKLM";
const LOCK_FILE_SUBJECT: &str = r"%ProgramData%\SHIN DATA CENTER\MKLM\mklm.lock";
const RECOVERY_DIR_SUBJECT: &str = r"%ProgramData%\SHIN DATA CENTER\MKLM\Recovery";

/// [`RegistryBackend`] over `mklm_win::regwrite` and `mklm_win::journal_store`. Opens each device
/// and global key per call (no cached handles survive a devnode removal).
///
/// Reads open the key read-only (`regwrite::open_device_key_read` / `open_global_key_read`),
/// never through the read-write open that A.3 lists for `WritableKey::read`: a key whose DACL
/// refuses writes can still be read, so recovery can judge it and end it in `Conflict` (design
/// review C3, I7), and a read never creates a missing "Device Parameters" key. Only writes and
/// flushes use `regwrite::open_device_key_rw`, which creates that (empty) key when it is missing.
#[derive(Debug)]
pub struct WinRegistry {
    /// The journal keys, opened and validated on the first journal write and kept afterwards
    /// (`JournalStore` checks their owner and DACL again before every write). Dropped after any
    /// failure, so that the next call opens and validates them again.
    journal: Option<JournalStore>,
}

impl WinRegistry {
    pub fn new() -> Self {
        Self { journal: None }
    }

    /// Runs `call` on the journal store, opening (creating) it first when needed.
    fn with_journal<T>(
        &mut self,
        call: impl FnOnce(&JournalStore) -> Result<T, WinError>,
    ) -> Result<T, BackendError> {
        let store = match self.journal.take() {
            Some(store) => store,
            None => JournalStore::open_or_create()
                .map_err(|error| backend_error(JOURNAL_SUBJECT, error))?,
        };
        let result = call(&store);
        if result.is_ok() {
            self.journal = Some(store);
        }
        result.map_err(|error| backend_error(JOURNAL_SUBJECT, error))
    }
}

impl Default for WinRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RegistryBackend for WinRegistry {
    fn read_value(&self, target: &WriteTarget, name: &str) -> Result<RegValue, BackendError> {
        match open_target_read(target)? {
            Some(key) => key.read(name).map_err(|error| target_error(target, error)),
            None => Ok(RegValue::Absent),
        }
    }

    fn list_values(&self, target: &WriteTarget) -> Result<Vec<(String, RegValue)>, BackendError> {
        match open_target_read(target)? {
            Some(key) => key.list().map_err(|error| target_error(target, error)),
            None => Ok(Vec::new()),
        }
    }

    fn write_value(
        &mut self,
        target: &WriteTarget,
        name: &str,
        value: &RegValue,
    ) -> Result<(), BackendError> {
        // Refused before the key is opened, so that a refused name does not even create a
        // missing "Device Parameters" key. `regwrite` checks again (design review S9).
        let Some(name) = allowed_name(target, name) else {
            return Err(BackendError::NameNotAllowed {
                name: name.to_string(),
            });
        };
        let key = open_target(target)?.ok_or_else(|| BackendError::NotFound {
            what: global_key_label(),
        })?;
        key.write(name, value)
            .map_err(|error| target_error(target, error))
    }

    fn flush_target(&mut self, target: &WriteTarget) -> Result<(), BackendError> {
        let key = match open_target(target) {
            Ok(Some(key)) => key,
            Ok(None) => {
                return Err(BackendError::NotFound {
                    what: global_key_label(),
                });
            }
            Err(removed @ BackendError::DeviceRemoved { .. }) => {
                // `RegFlushKey` flushes the whole hive. The removed devnode's key is gone, but
                // whatever else was written to the SYSTEM hive must still become durable: flush it
                // through the global key, which lives in the same hive.
                return match regwrite::open_global_key_rw() {
                    Ok(global) => global
                        .flush()
                        .map_err(|error| target_error(&WriteTarget::Global, error)),
                    Err(_) => Err(removed),
                };
            }
            Err(error) => return Err(error),
        };
        key.flush().map_err(|error| target_error(target, error))
    }

    fn read_journal(&self) -> Result<JournalDump, BackendError> {
        let raw = journal_store::read_journal_store()
            .map_err(|error| backend_error(JOURNAL_SUBJECT, error))?;
        Ok(JournalDump {
            store_version: raw.store_version,
            ops: raw.ops,
            baselines: raw.baselines,
        })
    }

    fn write_journal(&mut self, slot: &JournalSlot, json: &str) -> Result<(), BackendError> {
        let (subkey, name) = journal_record(slot);
        self.with_journal(|store| store.write(subkey, name, json))
    }

    fn delete_journal(&mut self, slot: &JournalSlot) -> Result<(), BackendError> {
        let (subkey, name) = journal_record(slot);
        self.with_journal(|store| store.delete(subkey, name))
    }

    fn flush_journal(&mut self) -> Result<(), BackendError> {
        self.with_journal(JournalStore::flush)
    }

    fn write_machine_setting(&mut self, name: &str, value: u32) -> Result<(), BackendError> {
        // Mapped explicitly, one setting at a time; `machine_settings` checks the name against
        // its static list again (design review S9).
        if name.eq_ignore_ascii_case(RESTORE_ON_UNINSTALL_VALUE) {
            machine_settings::write_restore_on_uninstall(value != 0)
                .map_err(|error| backend_error(SETTINGS_SUBJECT, error))
        } else {
            Err(BackendError::NameNotAllowed {
                name: name.to_string(),
            })
        }
    }
}

/// Sub-key and value name of a journal record.
fn journal_record(slot: &JournalSlot) -> (JournalSubkey, &str) {
    match slot {
        JournalSlot::Op(op_id) => (JournalSubkey::Ops, op_id.as_str()),
        JournalSlot::Baseline(canonical) => (JournalSubkey::Baselines, canonical.as_str()),
    }
}

/// The canonical spelling of `name` when MKLM may write it on `target`
/// (`mklm_core::DEVICE_VALUE_NAMES` / `GLOBAL_VALUE_NAMES`, compared case-insensitively like the
/// registry does).
fn allowed_name(target: &WriteTarget, name: &str) -> Option<&'static str> {
    let allowed: &[&'static str] = match target {
        WriteTarget::Device { .. } => &DEVICE_VALUE_NAMES,
        WriteTarget::Global => &GLOBAL_VALUE_NAMES,
    };
    allowed
        .iter()
        .copied()
        .find(|allowed| allowed.eq_ignore_ascii_case(name))
}

/// `HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters`.
fn global_key_label() -> String {
    format!(r"HKLM\{I8042PRT_PARAMETERS}")
}

/// Opens the target's key for reading only, creating nothing. `Ok(None)` when the key does not
/// exist (a devnode without "Device Parameters", or no global key): its values all read as
/// absent. A devnode that is gone is [`BackendError::DeviceRemoved`].
fn open_target_read(target: &WriteTarget) -> Result<Option<ReadOnlyKey>, BackendError> {
    match target {
        WriteTarget::Device { instance_id } => regwrite::open_device_key_read(instance_id),
        WriteTarget::Global => regwrite::open_global_key_read(),
    }
    .map_err(|error| target_error(target, error))
}

/// Opens the target's key for reading and writing. `Ok(None)` only when the global key does not
/// exist (it is never created; its values then all read as absent).
fn open_target(target: &WriteTarget) -> Result<Option<WritableKey>, BackendError> {
    match target {
        WriteTarget::Device { instance_id } => regwrite::open_device_key_rw(instance_id)
            .map(Some)
            .map_err(|error| target_error(target, error)),
        WriteTarget::Global => match regwrite::open_global_key_rw() {
            Ok(key) => Ok(Some(key)),
            Err(error) if error.win32_code() == Some(ERROR_FILE_NOT_FOUND) => Ok(None),
            Err(error) => Err(target_error(target, error)),
        },
    }
}

/// A failed call on `target`'s key.
fn target_error(target: &WriteTarget, error: WinError) -> BackendError {
    match target {
        WriteTarget::Device { instance_id } if devnode_gone(&error) => {
            BackendError::DeviceRemoved {
                instance_id: instance_id.clone(),
            }
        }
        WriteTarget::Device { instance_id } => backend_error(instance_id, error),
        WriteTarget::Global => backend_error(&global_key_label(), error),
    }
}

/// True when the error says that the devnode itself no longer exists (removed in Device Manager),
/// as opposed to a value or a "Device Parameters" key that does not exist (design review C3).
fn devnode_gone(error: &WinError) -> bool {
    error.config_ret() == Some(CR_NO_SUCH_DEVNODE) || error.win32_code() == Some(ERROR_KEY_DELETED)
}

fn is_access_denied(error: &WinError) -> bool {
    error.win32_code() == Some(ERROR_ACCESS_DENIED) || error.config_ret() == Some(CR_ACCESS_DENIED)
}

/// Maps a `mklm_win` error; `subject` names the key or devnode for errors that do not.
fn backend_error(subject: &str, error: WinError) -> BackendError {
    match error {
        WinError::ValueNotAllowed { name, .. } => BackendError::NameNotAllowed { name },
        WinError::Insecure { path, reason } => BackendError::Insecure { what: path, reason },
        WinError::NotKeyboard { instance_id } => BackendError::Insecure {
            what: instance_id,
            reason: "not a Keyboard-class devnode".to_string(),
        },
        WinError::OtherDevNode { found } => BackendError::Insecure {
            what: subject.to_string(),
            reason: format!("resolves to another devnode, {found}"),
        },
        error if is_access_denied(&error) => BackendError::AccessDenied {
            what: describe(subject, &error),
        },
        error => BackendError::Os {
            what: describe(subject, &error),
            code: error_code(&error),
        },
    }
}

/// Where an error happened, without its code (the engine's error types print the code).
fn describe(subject: &str, error: &WinError) -> String {
    match error {
        WinError::Registry { path, .. } => path.clone(),
        WinError::UnexpectedData { path } => format!("{path} (unexpected type or size)"),
        WinError::ConfigRet { function, .. }
        | WinError::Win32 { function, .. }
        | WinError::NtStatus { function, .. } => format!("{subject} ({function})"),
        other => format!("{subject} ({other})"),
    }
}

/// The OS code of an error: `WIN32_ERROR`, `CONFIGRET` or `NTSTATUS`, or the closest Win32 code
/// for the errors `mklm-win` raises itself.
fn error_code(error: &WinError) -> u32 {
    match error {
        WinError::ConfigRet { code, .. }
        | WinError::Win32 { code, .. }
        | WinError::Registry { code, .. } => *code,
        WinError::NtStatus { status, .. } => *status,
        WinError::Timeout { .. } => ERROR_TIMEOUT,
        WinError::Cancelled => ERROR_CANCELLED,
        WinError::Insecure { .. } | WinError::NotKeyboard { .. } => ERROR_ACCESS_DENIED,
        _ => ERROR_INVALID_DATA,
    }
}

/// [`DeviceController`] over `mklm_win::devices`, `mklm_win::rawinfo` and `mklm_win::devctl`.
///
/// `restart` runs `devctl::restart_device` on a worker thread and waits up to
/// `EngineConfig::restart_timeout` ([`RestartOutcome::TimedOut`]); a worker that is still running
/// is kept in `pending_restarts`, and [`WinDevices::join_pending`] (called by the helper before it
/// exits) waits for it, so that a process exit never cuts a class-installer call short
/// (design review C18). Dropping a `WinDevices` with workers still running waits for them too, up
/// to one minute, as a safety net.
#[derive(Debug, Default)]
pub struct WinDevices {
    /// Poll interval while waiting for a keyboard to come back (default 250 ms).
    poll: Option<Duration>,
    /// Deadline of one restart (default 20 s, `EngineConfig::restart_timeout`).
    restart_timeout: Option<Duration>,
    /// Restart workers that outlived their deadline, with the instance ID they reset.
    pending_restarts: Vec<(String, PendingRestart)>,
}

impl WinDevices {
    /// Default deadlines: 20 s per restart, 250 ms polling.
    pub fn new() -> Self {
        Self::default()
    }

    /// Deadline of one [`DeviceController::restart`]; pass `EngineConfig::restart_timeout`.
    pub fn with_restart_timeout(mut self, timeout: Duration) -> Self {
        self.restart_timeout = Some(timeout);
        self
    }

    /// Poll interval of [`DeviceController::wait_for_arrival`] (at least 10 ms).
    pub fn with_poll_interval(mut self, poll: Duration) -> Self {
        self.poll = Some(poll.max(MIN_POLL));
        self
    }

    fn restart_timeout(&self) -> Duration {
        self.restart_timeout.unwrap_or(DEFAULT_RESTART_TIMEOUT)
    }

    fn poll_interval(&self) -> Duration {
        self.poll.unwrap_or(DEFAULT_POLL)
    }

    /// Instance IDs whose restart worker outlived its deadline and still runs (for the log).
    pub fn pending_instance_ids(&self) -> Vec<&str> {
        self.pending_restarts
            .iter()
            .filter(|(_, pending)| !pending.is_finished())
            .map(|(instance_id, _)| instance_id.as_str())
            .collect()
    }

    /// Waits up to `timeout` for restart workers that outlived their deadline. Returns false when
    /// one is still running (the helper logs it and still waits before exiting).
    pub fn join_pending(&mut self, timeout: Duration) -> bool {
        let started = Instant::now();
        self.pending_restarts.retain(|(_, pending)| {
            loop {
                let remaining = timeout.saturating_sub(started.elapsed());
                let slice = remaining.min(MAX_WAIT_SLICE);
                if pending.wait(slice) {
                    // Finished: the worker has returned, nothing left to keep.
                    break false;
                }
                if remaining <= slice {
                    break true;
                }
            }
        });
        self.pending_restarts.is_empty()
    }
}

impl Drop for WinDevices {
    fn drop(&mut self) {
        // The owner should have called `join_pending` already (then this returns at once). If it
        // did not, give a class-installer call still in flight a bounded chance to return before
        // the process can exit under it.
        let _ = self.join_pending(DROP_JOIN_TIMEOUT);
    }
}

impl DeviceController for WinDevices {
    fn keyboards(&mut self) -> Result<Inventory, DeviceError> {
        let mut issues = Vec::new();
        let raw = mklm_win::raw_keyboards(&mut issues).unwrap_or_else(|error| {
            issues.push(ReadIssue {
                kind: ReadIssueKind::RawInput,
                subject: "Raw Input".to_string(),
                error,
            });
            Vec::new()
        });
        // Phantoms included: INV-PS2 needs every i8042prt devnode (design B.2).
        let keyboards = mklm_win::read_keyboards(true, &raw, &mut issues).map_err(|error| {
            // Without the list of Keyboard-class devnodes nothing about INV-PS2 is known.
            DeviceError::Incomplete {
                issues: vec![format!("Keyboard-class devnodes: {error}")],
            }
        })?;
        route_issues(keyboards, issues)
    }

    fn restart(&mut self, instance_id: &str) -> Result<RestartOutcome, DeviceError> {
        let deadline = self.restart_timeout();
        // One class-installer call at a time. A call that outlived its deadline most likely
        // waits for PnP, and a new one would only queue up behind it: give the old one this
        // restart's deadline to return, else do not start another (treated like a reboot being
        // needed, which is the safe answer).
        if !self.join_pending(deadline) {
            return Ok(RestartOutcome::TimedOut);
        }
        match devctl::restart_device_with_deadline(instance_id, deadline) {
            Ok(TimedRestart::Finished(Ok(RestartResult::Restarted))) => {
                Ok(RestartOutcome::Restarted)
            }
            Ok(TimedRestart::Finished(Ok(RestartResult::NeedsReboot))) => {
                Ok(RestartOutcome::NeedsReboot)
            }
            Ok(TimedRestart::TimedOut(pending)) => {
                self.pending_restarts
                    .push((instance_id.to_string(), pending));
                Ok(RestartOutcome::TimedOut)
            }
            Ok(TimedRestart::Finished(Err(error))) | Err(error) => {
                Err(device_error(instance_id, error))
            }
        }
    }

    fn wait_for_arrival(
        &mut self,
        instance_id: &str,
        timeout: Duration,
    ) -> Result<Arrival, DeviceError> {
        let timeout = timeout.min(MAX_WAIT_SLICE);
        let settle = ARRIVAL_SETTLE.min(timeout);
        thread::sleep(settle);
        let arrival = devctl::wait_for_arrival(instance_id, timeout - settle, self.poll_interval())
            .map_err(|error| device_error(instance_id, error))?;
        Ok(match arrival {
            DeviceArrival::Started { reported } => Arrival::Started { reported },
            DeviceArrival::TimedOut => Arrival::TimedOut,
        })
    }

    fn reported_type(&mut self, instance_id: &str) -> Result<Option<KeyboardType>, DeviceError> {
        devctl::reported_type(instance_id).map_err(|error| device_error(instance_id, error))
    }
}

/// Design A.3 / B.2: read issues whose kind blocks writes (the devnode set, identities, drivers,
/// presence) fail the inventory; every other one becomes a warning and the write goes on.
fn route_issues(
    keyboards: Vec<KeyboardDevice>,
    issues: Vec<ReadIssue>,
) -> Result<Inventory, DeviceError> {
    let (blocking, warnings): (Vec<ReadIssue>, Vec<ReadIssue>) = issues
        .into_iter()
        .partition(|issue| issue.kind.blocks_writes());
    if !blocking.is_empty() {
        return Err(DeviceError::Incomplete {
            issues: blocking.iter().map(ToString::to_string).collect(),
        });
    }
    Ok(Inventory {
        keyboards,
        warnings: warnings.iter().map(ToString::to_string).collect(),
    })
}

/// Maps a `mklm_win` error of a call on `instance_id`.
fn device_error(instance_id: &str, error: WinError) -> DeviceError {
    let gone = matches!(error, WinError::NotKeyboard { .. })
        || error.config_ret() == Some(CR_NO_SUCH_DEVNODE)
        || error.win32_code() == Some(ERROR_NO_SUCH_DEVINST);
    if gone {
        DeviceError::NotFound {
            instance_id: instance_id.to_string(),
        }
    } else {
        DeviceError::Os {
            what: describe(instance_id, &error),
            code: error_code(&error),
        }
    }
}

/// [`Host`] over `mklm_win::protected_dir`, `mklm_win::session` and `mklm_win::proc_identity`.
#[derive(Debug)]
pub struct WinHost {
    /// The validated base directory, every level pinned open. Validated again (and replaced) by
    /// every [`Host::acquire_lock`] (design D.9: `ensure_protected_dir(Base)` → `FileLock`).
    base: Option<ProtectedDir>,
    /// Origin of [`Host::monotonic`].
    started: Instant,
    /// Quarantine notices not drained yet (design review S1).
    warnings: Vec<String>,
    /// A base level was quarantined, `Recovery` with it (see [`Host::take_recovery_assets_moved`]).
    assets_moved: bool,
}

impl WinHost {
    /// Validates (and creates) the protected base directory up front. A squatted directory is
    /// quarantined (`protected_dir::ensure_protected_dir`) and reported through
    /// [`Host::drain_warnings`] (design review S1).
    pub fn new() -> Result<Self, HostError> {
        let mut host = Self {
            base: None,
            started: Instant::now(),
            warnings: Vec::new(),
            assets_moved: false,
        };
        host.validate_base()?;
        Ok(host)
    }

    /// Runs `ensure_protected_dir(Base)` again and keeps the result.
    fn validate_base(&mut self) -> Result<&ProtectedDir, HostError> {
        // Release the old pins first: the new validation pins every level again.
        self.base = None;
        let base = protected_dir::ensure_protected_dir(DataDir::Base)
            .map_err(|error| host_error(BASE_DIR_SUBJECT, error))?;
        self.note_quarantined(base.quarantined());
        // A quarantined base level took `Recovery` (and the recovery files) with it.
        self.assets_moved |= !base.quarantined().is_empty();
        Ok(self.base.insert(base))
    }

    fn note_quarantined<P: AsRef<Path>>(&mut self, paths: &[P]) {
        self.warnings
            .extend(paths.iter().map(|path| quarantine_warning(path.as_ref())));
    }
}

impl Host for WinHost {
    type Lock = FileLock;

    fn acquire_lock(&mut self, timeout: Duration) -> Result<Self::Lock, HostError> {
        let base = self.validate_base()?;
        let lock = FileLock::acquire(base, timeout).map_err(lock_error)?;
        if let Some(path) = lock.quarantined() {
            self.warnings.push(quarantine_warning(path));
        }
        Ok(lock)
    }

    fn now(&self) -> Timestamp {
        timestamp(SystemTime::now())
    }

    fn monotonic(&self) -> Duration {
        self.started.elapsed()
    }

    fn new_op_id(&mut self) -> Result<OpId, HostError> {
        let text = session::new_uuid().map_err(|error| host_error("new operation ID", error))?;
        OpId::parse(&text).map_err(|_| HostError::Os {
            what: format!("new operation ID {text:?}"),
            code: ERROR_INVALID_DATA,
        })
    }

    fn boot_id(&self) -> Result<BootId, HostError> {
        session::boot_id().map_err(|error| host_error("boot ID", error))
    }

    fn boot_time_hint(&self) -> Option<u64> {
        session::boot_time_hint().ok()
    }

    fn legacy_boot_guid(&self) -> Option<BootId> {
        session::legacy_boot_guid().ok()
    }

    fn current_process(&self) -> Result<ProcessIdentity, HostError> {
        proc_identity::current_process_identity()
            .map_err(|error| host_error("current process", error))
    }

    fn liveness(&self, process: &ProcessIdentity) -> Liveness {
        proc_identity::process_liveness(process)
    }

    fn system32_file_exists(&self, file_name: &str) -> Result<bool, HostError> {
        // GetSystemDirectoryW + a plain file name (no separator, drive or `..`) + a regular file
        // that is not a reparse point (plan 1.5, design review S8).
        elevation::system32_file_exists(file_name)
            .map_err(|error| host_error(&format!(r"System32\{file_name}"), error))
    }

    fn write_recovery_assets(&mut self, assets: &RecoveryAssets) -> Result<(), HostError> {
        let dir = protected_dir::ensure_protected_dir(DataDir::Recovery)
            .map_err(|error| host_error(RECOVERY_DIR_SUBJECT, error))?;
        self.note_quarantined(dir.quarantined());
        // Each file: temporary file, FlushFileBuffers, `.prev` kept, MoveFileExW(REPLACE_EXISTING
        // | WRITE_THROUGH), FlushFileBuffers on the directory (design review C10).
        for (name, bytes) in [
            (RESTORE_CMD_FILE, assets.cmd.as_bytes()),
            (BASELINE_REG_FILE, assets.reg.as_slice()),
            (README_FILE, assets.readme.as_bytes()),
        ] {
            dir.replace_file(name, bytes)
                .map_err(|error| recovery_file_error(name, error))?;
        }
        Ok(())
    }

    fn drain_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    fn take_recovery_assets_moved(&mut self) -> bool {
        std::mem::take(&mut self.assets_moved)
    }
}

/// Milliseconds since the Unix epoch; 0 for a clock set before 1970.
fn timestamp(time: SystemTime) -> Timestamp {
    Timestamp(time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    }))
}

/// The warning for a squatted level (or lock file) that was moved aside (design review S1).
fn quarantine_warning(path: &Path) -> String {
    format!(
        "An MKLM data folder or lock file that another account had created was not trusted: it \
         was moved to {} and created again with protected permissions",
        path.display()
    )
}

/// Maps a failed replace of the recovery file `name`. `ERROR_ACCESS_DENIED` and
/// `ERROR_SHARING_VIOLATION` (after `replace_file`'s retries) almost always mean that another
/// program holds the file or the folder open: Users may read them, and `MoveFileExW` cannot
/// replace a file that is open without `FILE_SHARE_DELETE` (residual risk, design I.18).
fn recovery_file_error(name: &str, error: WinError) -> HostError {
    const ERROR_SHARING_VIOLATION: u32 = 32;
    let subject = format!(r"{RECOVERY_DIR_SUBJECT}\{name}");
    match error.win32_code() {
        Some(code) if code == ERROR_ACCESS_DENIED || code == ERROR_SHARING_VIOLATION => {
            HostError::Os {
                what: format!(
                    "{} could not be replaced; another program (an editor, a copy in progress, \
                     the Explorer preview pane or antivirus software) probably has it or its \
                     folder open. Close it and try again",
                    describe(&subject, &error)
                ),
                code,
            }
        }
        _ => host_error(&subject, error),
    }
}

/// Maps a `mklm_win` error; `subject` names what was being done, for errors that do not say.
fn host_error(subject: &str, error: WinError) -> HostError {
    match error {
        WinError::Insecure { path, reason } => HostError::Insecure { path, reason },
        error => HostError::Os {
            what: describe(subject, &error),
            code: error_code(&error),
        },
    }
}

/// [`host_error`], with the lock's timeout as [`HostError::Busy`].
fn lock_error(error: WinError) -> HostError {
    match error {
        WinError::Timeout { .. } => HostError::Busy,
        error => host_error(LOCK_FILE_SUBJECT, error),
    }
}

#[cfg(test)]
mod tests {
    //! Unelevated and read-only: nothing here opens a key for writing, creates a directory or
    //! resets a device. The elevated checks are `#[ignore]` and belong to design H.2 R1.

    use super::*;

    /// No such devnode exists on any machine.
    const NO_SUCH_DEVICE: &str = r"HID\MKLM_NO_SUCH_DEVICE\0";

    fn device(instance_id: &str) -> WriteTarget {
        WriteTarget::Device {
            instance_id: instance_id.to_string(),
        }
    }

    /// A host without the protected directory (`WinHost::new` would create it).
    fn bare_host() -> WinHost {
        WinHost {
            base: None,
            started: Instant::now(),
            warnings: Vec::new(),
            assets_moved: false,
        }
    }

    #[test]
    fn writes_outside_the_value_allowlist_are_refused_before_any_call() {
        // The gate runs before any key is opened, so these never reach the registry.
        let mut registry = WinRegistry::new();
        for (target, name) in [
            (device(NO_SUCH_DEVICE), "Start"),
            (device(NO_SUCH_DEVICE), "LayerDriver JPN"),
            (device(NO_SUCH_DEVICE), "KeyboardNumberTotalKeysOverride"),
            (WriteTarget::Global, "KeyboardTypeOverride"),
            (WriteTarget::Global, "LayerDriver KOR"),
            (WriteTarget::Global, ""),
        ] {
            assert_eq!(
                registry.write_value(&target, name, &RegValue::Absent),
                Err(BackendError::NameNotAllowed {
                    name: name.to_string()
                }),
                "{target:?} {name}"
            );
        }
        // Machine settings: only `RestoreOnUninstall` (design m3 WP-E2), refused before the
        // Settings key is opened.
        for name in ["Start", "RestoreOnUninstall2", "InstallDir", ""] {
            assert_eq!(
                registry.write_machine_setting(name, 1),
                Err(BackendError::NameNotAllowed {
                    name: name.to_string()
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn allowed_names_follow_the_core_lists_case_insensitively() {
        let hid = device("HID\\X\\0");
        assert_eq!(
            allowed_name(&hid, "keyboardtypeoverride"),
            Some("KeyboardTypeOverride")
        );
        assert_eq!(
            allowed_name(&hid, "OverrideKeyboardSubtype"),
            Some("OverrideKeyboardSubtype")
        );
        assert_eq!(allowed_name(&hid, "OverrideKeyboardIdentifier"), None);
        assert_eq!(
            allowed_name(&WriteTarget::Global, "layerdriver jpn"),
            Some("LayerDriver JPN")
        );
        assert_eq!(
            allowed_name(&WriteTarget::Global, "KeyboardTypeOverride"),
            None
        );
        for name in DEVICE_VALUE_NAMES {
            assert_eq!(allowed_name(&hid, name), Some(name));
        }
        for name in GLOBAL_VALUE_NAMES {
            assert_eq!(allowed_name(&WriteTarget::Global, name), Some(name));
        }
    }

    #[test]
    fn a_removed_devnode_is_not_an_absent_value() {
        // Design review C3: CR_NO_SUCH_DEVNODE and a key deleted under the handle mean "removed".
        let located = WinError::ConfigRet {
            function: "CM_Locate_DevNodeW",
            code: CR_NO_SUCH_DEVNODE,
        };
        assert_eq!(
            target_error(&device("HID\\A\\0"), located.clone()),
            BackendError::DeviceRemoved {
                instance_id: "HID\\A\\0".to_string()
            }
        );
        let deleted = WinError::Registry {
            path: r"HKLM\...\Device Parameters\KeyboardTypeOverride".to_string(),
            code: ERROR_KEY_DELETED,
        };
        assert!(matches!(
            target_error(&device("HID\\A\\0"), deleted),
            BackendError::DeviceRemoved { .. }
        ));
        // Not for the global key, and not for a missing value or key.
        assert!(matches!(
            target_error(&WriteTarget::Global, located),
            BackendError::Os {
                code: CR_NO_SUCH_DEVNODE,
                ..
            }
        ));
        assert!(matches!(
            target_error(
                &device("HID\\A\\0"),
                WinError::Registry {
                    path: "p".to_string(),
                    code: ERROR_FILE_NOT_FOUND
                }
            ),
            BackendError::Os {
                code: ERROR_FILE_NOT_FOUND,
                ..
            }
        ));
    }

    #[test]
    fn win_errors_map_onto_backend_errors() {
        let hid = device("HID\\A\\0");
        assert_eq!(
            target_error(
                &hid,
                WinError::ValueNotAllowed {
                    path: "p".to_string(),
                    name: "Start".to_string()
                }
            ),
            BackendError::NameNotAllowed {
                name: "Start".to_string()
            }
        );
        assert!(matches!(
            target_error(
                &hid,
                WinError::NotKeyboard {
                    instance_id: "HID\\A\\0".to_string()
                }
            ),
            BackendError::Insecure { .. }
        ));
        assert!(matches!(
            target_error(
                &hid,
                WinError::OtherDevNode {
                    found: "HID\\B\\0".to_string()
                }
            ),
            BackendError::Insecure { .. }
        ));
        assert_eq!(
            backend_error(
                JOURNAL_SUBJECT,
                WinError::Insecure {
                    path: "HKLM\\SOFTWARE\\SHIN DATA CENTER".to_string(),
                    reason: "owner".to_string()
                }
            ),
            BackendError::Insecure {
                what: "HKLM\\SOFTWARE\\SHIN DATA CENTER".to_string(),
                reason: "owner".to_string()
            }
        );
        assert_eq!(
            target_error(
                &WriteTarget::Global,
                WinError::Registry {
                    path: "HKLM\\x".to_string(),
                    code: ERROR_ACCESS_DENIED
                }
            ),
            BackendError::AccessDenied {
                what: "HKLM\\x".to_string()
            }
        );
        assert!(matches!(
            target_error(
                &hid,
                WinError::ConfigRet {
                    function: "CM_Open_DevNode_Key",
                    code: CR_ACCESS_DENIED
                }
            ),
            BackendError::AccessDenied { .. }
        ));
        assert_eq!(
            target_error(
                &hid,
                WinError::ConfigRet {
                    function: "CM_Open_DevNode_Key",
                    code: 0x14
                }
            ),
            BackendError::Os {
                what: "HID\\A\\0 (CM_Open_DevNode_Key)".to_string(),
                code: 0x14
            }
        );
        assert_eq!(
            backend_error(
                JOURNAL_SUBJECT,
                WinError::UnexpectedData {
                    path: "HKLM\\x\\y".to_string()
                }
            ),
            BackendError::Os {
                what: "HKLM\\x\\y (unexpected type or size)".to_string(),
                code: ERROR_INVALID_DATA
            }
        );
    }

    #[test]
    fn journal_records_address_their_sub_keys() {
        let op = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").expect("valid op id");
        assert_eq!(
            journal_record(&JournalSlot::Op(op)),
            (JournalSubkey::Ops, "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f")
        );
        let baseline = JournalSlot::Baseline("global|LayerDriver JPN".to_string());
        assert_eq!(
            journal_record(&baseline),
            (JournalSubkey::Baselines, "global|LayerDriver JPN")
        );
    }

    #[test]
    fn the_journal_reads_unelevated() {
        // Read-only (KEY_READ). On a machine without MKLM the store is simply empty.
        let dump = WinRegistry::new().read_journal();
        assert!(dump.is_ok(), "{dump:?}");
    }

    fn issue(kind: ReadIssueKind, subject: &str) -> ReadIssue {
        ReadIssue {
            kind,
            subject: subject.to_string(),
            error: WinError::Win32 {
                function: "F",
                code: 5,
            },
        }
    }

    #[test]
    fn only_inventory_issues_block_writes() {
        // Design A.3 table: Locate, Identity, Driver and Presence stop the write; the others are
        // warnings (design review S2).
        let all = [
            ReadIssueKind::Locate,
            ReadIssueKind::Identity,
            ReadIssueKind::Driver,
            ReadIssueKind::Presence,
            ReadIssueKind::Status,
            ReadIssueKind::Container,
            ReadIssueKind::Topology,
            ReadIssueKind::Descriptive,
            ReadIssueKind::Values,
            ReadIssueKind::RawInput,
            ReadIssueKind::Environment,
        ];
        for kind in all {
            let result = route_issues(Vec::new(), vec![issue(kind, "HID\\A\\0")]);
            match (kind.blocks_writes(), result) {
                (true, Err(DeviceError::Incomplete { issues })) => {
                    assert_eq!(issues, ["HID\\A\\0: F failed with Win32 error 5"]);
                }
                (false, Ok(inventory)) => {
                    assert_eq!(
                        inventory.warnings,
                        ["HID\\A\\0: F failed with Win32 error 5"]
                    );
                }
                (blocks, other) => panic!("{kind:?} (blocks: {blocks}): {other:?}"),
            }
        }
    }

    #[test]
    fn blocking_issues_are_reported_alone() {
        let keyboards = vec![mklm_core::fixtures::keychron()];
        let result = route_issues(
            keyboards.clone(),
            vec![
                issue(ReadIssueKind::RawInput, "Raw Input"),
                issue(ReadIssueKind::Driver, "ACPI\\X\\0"),
                issue(ReadIssueKind::Values, "HID\\A\\0\\Device Parameters"),
            ],
        );
        assert_eq!(
            result,
            Err(DeviceError::Incomplete {
                issues: vec!["ACPI\\X\\0: F failed with Win32 error 5".to_string()]
            })
        );
        let result = route_issues(
            keyboards.clone(),
            vec![
                issue(ReadIssueKind::RawInput, "Raw Input"),
                issue(ReadIssueKind::Status, "HID\\A\\0"),
            ],
        );
        assert_eq!(
            result,
            Ok(Inventory {
                keyboards,
                warnings: vec![
                    "Raw Input: F failed with Win32 error 5".to_string(),
                    "HID\\A\\0: F failed with Win32 error 5".to_string()
                ]
            })
        );
    }

    #[test]
    fn device_errors_name_missing_keyboards() {
        for error in [
            WinError::NotKeyboard {
                instance_id: "X".to_string(),
            },
            WinError::ConfigRet {
                function: "CM_Locate_DevNodeW",
                code: CR_NO_SUCH_DEVNODE,
            },
            WinError::Win32 {
                function: "SetupDiOpenDeviceInfoW",
                code: ERROR_NO_SUCH_DEVINST,
            },
        ] {
            assert_eq!(
                device_error("HID\\A\\0", error),
                DeviceError::NotFound {
                    instance_id: "HID\\A\\0".to_string()
                }
            );
        }
        assert_eq!(
            device_error(
                "HID\\A\\0",
                WinError::Win32 {
                    function: "SetupDiCallClassInstaller",
                    code: 1234
                }
            ),
            DeviceError::Os {
                what: "HID\\A\\0 (SetupDiCallClassInstaller)".to_string(),
                code: 1234
            }
        );
    }

    #[test]
    fn host_errors_keep_insecure_and_turn_the_lock_timeout_into_busy() {
        assert_eq!(
            lock_error(WinError::Timeout {
                operation: "LockFileEx"
            }),
            HostError::Busy
        );
        assert_eq!(
            lock_error(WinError::Insecure {
                path: "C:\\ProgramData\\SHIN DATA CENTER".to_string(),
                reason: "owner".to_string()
            }),
            HostError::Insecure {
                path: "C:\\ProgramData\\SHIN DATA CENTER".to_string(),
                reason: "owner".to_string()
            }
        );
        // Outside the lock, a timeout is just an error.
        assert!(matches!(
            host_error("x", WinError::Timeout { operation: "x" }),
            HostError::Os {
                code: ERROR_TIMEOUT,
                ..
            }
        ));
        assert_eq!(
            host_error(
                "boot ID",
                WinError::NtStatus {
                    function: "NtQuerySystemInformation",
                    status: 0xC000_0003
                }
            ),
            HostError::Os {
                what: "boot ID (NtQuerySystemInformation)".to_string(),
                code: 0xC000_0003
            }
        );
        assert!(matches!(
            lock_error(WinError::Win32 {
                function: "CreateFileW",
                code: 32
            }),
            HostError::Os { what, code: 32 } if what.contains("mklm.lock")
        ));
    }

    #[test]
    fn no_pending_restart_means_nothing_to_join() {
        let mut devices = WinDevices::new();
        let started = Instant::now();
        assert!(devices.join_pending(Duration::from_secs(30)));
        assert!(devices.join_pending(Duration::MAX));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(devices.pending_instance_ids().is_empty());
    }

    #[test]
    fn configured_deadlines_are_kept() {
        let devices = WinDevices::new()
            .with_restart_timeout(Duration::from_secs(7))
            .with_poll_interval(Duration::ZERO);
        assert_eq!(devices.restart_timeout(), Duration::from_secs(7));
        assert_eq!(devices.poll_interval(), MIN_POLL);
        let defaults = WinDevices::new();
        assert_eq!(defaults.restart_timeout(), DEFAULT_RESTART_TIMEOUT);
        assert_eq!(defaults.poll_interval(), DEFAULT_POLL);
    }

    #[test]
    fn unknown_keyboards_are_not_reported_and_never_arrive() {
        // Read-only: Raw Input and CfgMgr32 state of a devnode that does not exist.
        let mut devices = WinDevices::new().with_poll_interval(Duration::from_millis(10));
        assert_eq!(devices.reported_type(NO_SUCH_DEVICE), Ok(None));
        assert_eq!(
            devices.wait_for_arrival(NO_SUCH_DEVICE, Duration::from_millis(60)),
            Ok(Arrival::TimedOut)
        );
    }

    #[test]
    fn restarting_an_unknown_devnode_fails_before_the_class_installer() {
        // `SetupDiOpenDeviceInfoW` refuses an instance ID that names no devnode, so this never
        // reaches `SetupDiCallClassInstaller`: no device is reset.
        let mut devices = WinDevices::new();
        assert_eq!(
            devices.restart(NO_SUCH_DEVICE),
            Err(DeviceError::NotFound {
                instance_id: NO_SUCH_DEVICE.to_string()
            })
        );
        assert!(devices.join_pending(Duration::ZERO));
    }

    #[test]
    fn a_recovery_file_held_open_is_named_with_the_likely_cause() {
        for code in [5, 32] {
            let error = recovery_file_error(
                "restore-offline.cmd",
                WinError::Win32 {
                    function: "MoveFileExW",
                    code,
                },
            );
            assert!(
                matches!(
                    &error,
                    HostError::Os { what, code: c }
                        if *c == code
                            && what.contains(r"Recovery\restore-offline.cmd")
                            && what.contains("another program")
                ),
                "{error:?}"
            );
        }
        let other = recovery_file_error(
            "README.txt",
            WinError::Win32 {
                function: "WriteFile",
                code: 112,
            },
        );
        assert!(
            matches!(&other, HostError::Os { what, code: 112 } if !what.contains("another program")),
            "{other:?}"
        );
    }

    /// The backend reads through read-only opens (design review C3, I7): a standard user can read
    /// every keyboard's values and the global key, which a read-write open would refuse, and a
    /// devnode that does not exist is `DeviceRemoved`, not "absent".
    #[test]
    fn values_read_unelevated_through_read_only_keys() {
        let registry = WinRegistry::new();
        let global = registry
            .list_values(&WriteTarget::Global)
            .expect("list the global key");
        for (name, value) in &global {
            assert_eq!(
                registry.read_value(&WriteTarget::Global, name).as_ref(),
                Ok(value)
            );
        }
        if let Ok(inventory) = WinDevices::new().keyboards() {
            for keyboard in inventory.keyboards.iter().take(5) {
                let target = device(&keyboard.instance_id);
                let listed = registry
                    .list_values(&target)
                    .unwrap_or_else(|error| panic!("{}: {error:?}", keyboard.instance_id));
                for (name, value) in &listed {
                    assert_eq!(registry.read_value(&target, name).as_ref(), Ok(value));
                }
            }
        }
        assert!(matches!(
            registry.read_value(&device(NO_SUCH_DEVICE), "KeyboardTypeOverride"),
            Err(BackendError::DeviceRemoved { .. })
        ));
    }

    #[test]
    fn the_inventory_reads_unelevated() {
        // Read-only, like `mklm-cli list`: CfgMgr32 properties, hardware keys with KEY_READ and
        // Raw Input. Machines without keyboards (some CI runners) have an empty list.
        match WinDevices::new().keyboards() {
            Ok(inventory) => {
                assert!(inventory.warnings.iter().all(|warning| !warning.is_empty()));
                for keyboard in &inventory.keyboards {
                    assert!(!keyboard.instance_id.is_empty());
                }
            }
            Err(DeviceError::Incomplete { issues }) => assert!(!issues.is_empty()),
            Err(other) => panic!("{other:?}"),
        }
    }

    #[test]
    fn timestamps_are_unix_milliseconds() {
        assert_eq!(timestamp(UNIX_EPOCH), Timestamp(0));
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_millis(1_790_000_000_123)),
            Timestamp(1_790_000_000_123)
        );
        assert_eq!(timestamp(UNIX_EPOCH - Duration::from_secs(1)), Timestamp(0));
        let host = bare_host();
        let now = host.now().0;
        assert!(now > 1_700_000_000_000, "{now}");
        let first = host.monotonic();
        assert!(host.monotonic() >= first);
    }

    #[test]
    fn identities_come_from_the_system() {
        // Read-only queries: randomness, the boot environment and the process table.
        let mut host = bare_host();
        let a = host.new_op_id().expect("op id");
        let b = host.new_op_id().expect("op id");
        assert_ne!(a, b);
        let boot = host.boot_id().expect("boot id");
        assert_eq!(host.boot_id(), Ok(boot));
        assert!(boot.boot_counter().is_some(), "{}", boot.to_text());
        assert!(host.boot_time_hint().is_some());
        let guid = host.legacy_boot_guid();
        assert!(guid.is_some_and(BootId::is_legacy), "{guid:?}");
        let me = host.current_process().expect("current process");
        assert_eq!(me.pid, std::process::id());
        assert_eq!(host.liveness(&me), Liveness::Alive);
        let gone = ProcessIdentity {
            creation_time: me.creation_time.wrapping_add(1),
            ..me
        };
        assert_eq!(host.liveness(&gone), Liveness::Dead);
    }

    #[test]
    fn layout_dlls_are_looked_up_in_system32_only() {
        let host = bare_host();
        assert_eq!(host.system32_file_exists("kernel32.dll"), Ok(true));
        assert_eq!(
            host.system32_file_exists("mklm-no-such-layout.dll"),
            Ok(false)
        );
        // A directory is not a layout DLL.
        assert_eq!(host.system32_file_exists("drivers"), Ok(false));
        for name in [r"..\kbd106.dll", "C:kbd106.dll", "drivers/x.dll", ""] {
            assert!(
                matches!(
                    host.system32_file_exists(name),
                    Err(HostError::Insecure { .. })
                ),
                "{name:?}"
            );
        }
    }

    #[test]
    fn quarantine_notices_are_drained_once() {
        let mut host = bare_host();
        host.note_quarantined(&[Path::new(
            r"C:\ProgramData\SHIN DATA CENTER.untrusted-3f2a9c1e",
        )]);
        let warnings = host.drain_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("SHIN DATA CENTER.untrusted-3f2a9c1e"));
        assert!(host.drain_warnings().is_empty());
    }

    /// Design H.2 R1, engine side, run by the orchestrator in an elevated console with the user's
    /// consent (`cargo test -p mklm-engine -- --ignored r1_`). Creates the protected
    /// `%ProgramData%\SHIN DATA CENTER\MKLM` directories and the lock file, as `mklm-cli recover`
    /// does, and reads the journal, every keyboard and every value the engine re-reads. It writes
    /// no registry value (opening a device key for reading creates only a missing, empty
    /// "Device Parameters" key; every keyboard of the development machine has one).
    #[test]
    #[ignore = "H.2 R1: needs elevation; creates the MKLM data directories and lock file"]
    fn r1_elevated_backends_read_the_machine() {
        assert_eq!(elevation::is_elevated(), Ok(true), "H.2 R1 runs elevated");
        let mut host = WinHost::new().expect("protected base directory");
        let lock = host
            .acquire_lock(Duration::from_secs(10))
            .expect("write lock");
        let registry = WinRegistry::new();
        let journal = registry.read_journal().expect("journal");
        let inventory = WinDevices::new().keyboards().expect("inventory");
        for keyboard in &inventory.keyboards {
            let target = device(&keyboard.instance_id);
            for (name, value) in registry.list_values(&target).expect("device values") {
                assert_eq!(registry.read_value(&target, &name), Ok(value));
            }
        }
        for (name, value) in registry
            .list_values(&WriteTarget::Global)
            .expect("global values")
        {
            assert_eq!(registry.read_value(&WriteTarget::Global, &name), Ok(value));
        }
        host.boot_id().expect("boot id");
        drop(lock);
        assert_eq!(
            registry.read_journal(),
            Ok(journal),
            "nothing was journaled"
        );
    }

    /// Design H.2 R1 through the engine: `recover` with an empty journal ends `Recovered` and
    /// writes nothing. Elevated, with the user's consent, after WP4b is merged.
    #[test]
    #[ignore = "H.2 R1: needs elevation; creates the MKLM data directories and lock file"]
    fn r1_recover_with_an_empty_journal_writes_nothing() {
        use crate::engine::{Engine, EngineConfig};
        use crate::sink::NullSink;

        assert_eq!(elevation::is_elevated(), Ok(true), "H.2 R1 runs elevated");
        let registry = WinRegistry::new();
        let before = registry.read_journal().expect("journal");
        assert!(
            before.ops.is_empty() && before.baselines.is_empty(),
            "H.2 R1 starts from an empty journal"
        );
        let host = WinHost::new().expect("protected base directory");
        let config = EngineConfig::default();
        let devices = WinDevices::new().with_restart_timeout(config.restart_timeout);
        let mut engine = Engine::new(registry, devices, host, config);
        // No live reset may happen: allow_live_reset is false.
        let result = engine
            .recover(&mklm_core::ApplyOptions::default(), &mut NullSink)
            .expect("recover");
        assert_eq!(result.outcome, mklm_core::Outcome::Recovered);
        let (registry, mut devices, _host) = engine.into_parts();
        assert_eq!(registry.read_journal(), Ok(before), "nothing was journaled");
        assert!(devices.join_pending(Duration::from_secs(60)));
    }
}
