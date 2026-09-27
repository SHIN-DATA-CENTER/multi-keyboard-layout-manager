//! In-memory implementations of the engine's traits, with crash and fault injection (section B.4
//! and H of the design doc). Public so that `mklm-helper` / `mklm-cli` tests can drive a whole
//! session without touching the system.
//!
//! Crash model. Device and global values live in the SYSTEM hive, the journal in the SOFTWARE hive.
//! [`MemoryRegistry`] logs every mutating call (write, delete, flush) in order. After a simulated
//! crash, [`MemoryRegistry::crash_images`] lists every state the machine can come back with:
//! - [`CrashImage::ProcessKill`]: every completed call survives (the OS outlives the process);
//! - [`CrashImage::PowerLoss`]: per hive, any prefix of that hive's calls that contains at least
//!   everything before its last completed flush (Windows' lazy writer may have flushed more, but
//!   always a consistent prefix).
//!
//! A test runs an operation with `crash_after = n` for every `n`, rebuilds each image with
//! [`MemoryRegistry::after_crash`], runs recovery and checks the invariants of section H.1 (I1-I7).

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(unused_variables, dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use mklm_core::Event;
use mklm_core::{
    BootId, KeyboardDevice, KeyboardType, Liveness, OpId, ProcessIdentity, RecoveryAssets,
    RegValue, Timestamp, WriteTarget,
};

use crate::backend::{BackendError, JournalDump, JournalSlot, RegistryBackend};
use crate::device::{Arrival, DeviceController, DeviceError, Inventory, RestartOutcome};
use crate::host::{Host, HostError};
use crate::sink::{DecisionPoll, EventSink};

/// Which hive a call touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hive {
    /// `HKLM\SYSTEM`: device hardware keys and `Services\i8042prt\Parameters`.
    System,
    /// `HKLM\SOFTWARE`: the journal.
    Software,
}

/// One mutating call, as logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    WriteValue {
        target: WriteTarget,
        name: String,
        value: RegValue,
    },
    WriteJournal {
        slot: JournalSlot,
        json: String,
    },
    DeleteJournal {
        slot: JournalSlot,
    },
    Flush {
        hive: Hive,
    },
}

/// Faults to inject, counted over mutating calls (1-based).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FaultPlan {
    /// Calls `1..=n` complete; call `n + 1` and every later call fail with
    /// [`BackendError::Crashed`] (the process died). `Some(0)` crashes before the first write.
    pub crash_after: Option<usize>,
    /// Call `n` fails with [`BackendError::Injected`]; later calls work (error-path tests).
    pub fail_at: Option<usize>,
    /// Every write to this target fails with [`BackendError::AccessDenied`], in this engine run
    /// and after [`MemoryRegistry::after_crash`] too (EDR or tamper protection; design review C3).
    /// Recovery must end in `Conflict` rather than retry forever.
    pub deny_target: Option<WriteTarget>,
}

/// A state the registry can be found in after a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrashImage {
    ProcessKill,
    /// Number of logged calls kept per hive.
    PowerLoss {
        system_calls: usize,
        software_calls: usize,
    },
}

/// Registry contents without the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryContents {
    /// Upper-case instance ID → value name → value.
    pub devices: BTreeMap<String, BTreeMap<String, RegValue>>,
    pub global: BTreeMap<String, RegValue>,
    pub store_version: Option<u32>,
    pub ops: BTreeMap<String, String>,
    pub baselines: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
struct MemoryState {
    /// Durable contents at creation (seeded values).
    initial: RegistryContents,
    log: Vec<Mutation>,
    faults: FaultPlan,
    mutating_calls: usize,
    crashed: bool,
}

/// In-memory [`RegistryBackend`]. Cloning shares the state (the fake devices read it too).
#[derive(Debug, Clone, Default)]
pub struct MemoryRegistry {
    state: Rc<RefCell<MemoryState>>,
}

impl MemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a durable value before the test starts (not logged, not counted).
    pub fn seed(&self, target: &WriteTarget, name: &str, value: RegValue) {
        todo!("M2")
    }

    /// Somebody else (Settings app, Windows Update, another admin) changes a value: applied and
    /// durable at once, not counted, never faulted. Used for conflict tests.
    pub fn outside_edit(&self, target: &WriteTarget, name: &str, value: RegValue) {
        todo!("M2")
    }

    pub fn set_faults(&self, faults: FaultPlan) {
        todo!("M2")
    }

    /// The devnode disappears (removed in Device Manager): reads and writes of its values fail
    /// with [`BackendError::DeviceRemoved`]. Not logged, not counted.
    pub fn remove_devnode(&self, instance_id: &str) {
        todo!("M2")
    }

    /// Mutating calls made so far (the upper bound for `crash_after` loops).
    pub fn mutating_calls(&self) -> usize {
        todo!("M2")
    }

    /// The logged calls, in order.
    pub fn mutations(&self) -> Vec<Mutation> {
        todo!("M2")
    }

    /// Current (volatile) contents.
    pub fn contents(&self) -> RegistryContents {
        todo!("M2")
    }

    /// Every durable state consistent with the flush history (see the module docs).
    pub fn crash_images(&self) -> Vec<CrashImage> {
        todo!("M2")
    }

    /// A new, independent registry holding `image`'s contents, with no faults and an empty log.
    pub fn after_crash(&self, image: CrashImage) -> MemoryRegistry {
        todo!("M2")
    }
}

impl RegistryBackend for MemoryRegistry {
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

/// How a fake keyboard reacts to a live reset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FakeReset {
    /// Comes back reporting the type the registry now predicts (USB, M0 #2b).
    #[default]
    Applies,
    /// `DI_NEEDREBOOT`.
    NeedsReboot,
    /// Never comes back within the timeout.
    NeverArrives,
    /// Comes back with the old type.
    ArrivesUnchanged,
    /// The reset call fails.
    Fails,
    /// The class installer never returns within the deadline ([`RestartOutcome::TimedOut`]).
    Hangs,
}

/// In-memory [`DeviceController`]: a fixed keyboard list whose reported types follow the
/// [`MemoryRegistry`] values on reset or reconnect, as kbdhid does.
#[derive(Debug, Clone)]
pub struct FakeDevices {
    registry: MemoryRegistry,
    keyboards: Vec<KeyboardDevice>,
    resets: BTreeMap<String, FakeReset>,
    restarted: Vec<String>,
    incomplete: bool,
    warnings: Vec<String>,
}

impl FakeDevices {
    pub fn new(registry: MemoryRegistry, keyboards: Vec<KeyboardDevice>) -> Self {
        Self {
            registry,
            keyboards,
            resets: BTreeMap::new(),
            restarted: Vec::new(),
            incomplete: false,
            warnings: Vec::new(),
        }
    }

    pub fn set_reset(&mut self, instance_id: &str, behaviour: FakeReset) {
        todo!("M2")
    }

    /// Makes [`DeviceController::keyboards`] fail with [`DeviceError::Incomplete`] (a
    /// write-blocking read issue: devnode set, driver or presence unknown).
    pub fn set_incomplete(&mut self, incomplete: bool) {
        todo!("M2")
    }

    /// Non-blocking read issues [`DeviceController::keyboards`] reports as
    /// [`Inventory::warnings`] (names, Raw Input, a `REG_SZ` override…); writes must go on.
    pub fn set_warnings(&mut self, warnings: Vec<String>) {
        todo!("M2")
    }

    /// Unplug and replug: the reported type is re-read from the registry.
    pub fn reconnect(&mut self, instance_id: &str) {
        todo!("M2")
    }

    /// PC restart: every keyboard's reported type is re-read from the registry (with the global
    /// values, as `mklm_core::predict_type` does).
    pub fn reboot(&mut self) {
        todo!("M2")
    }

    /// The devnode is removed: it leaves the inventory and [`MemoryRegistry::remove_devnode`]
    /// applies.
    pub fn remove(&mut self, instance_id: &str) {
        todo!("M2")
    }

    /// Instance IDs reset so far, in order.
    pub fn restarted(&self) -> &[String] {
        &self.restarted
    }
}

impl DeviceController for FakeDevices {
    fn keyboards(&mut self) -> Result<Inventory, DeviceError> {
        todo!("M2")
    }

    fn restart(&mut self, instance_id: &str) -> Result<RestartOutcome, DeviceError> {
        todo!("M2")
    }

    fn wait_for_arrival(
        &mut self,
        instance_id: &str,
        timeout: Duration,
    ) -> Result<Arrival, DeviceError> {
        todo!("M2")
    }

    fn reported_type(&mut self, instance_id: &str) -> Result<Option<KeyboardType>, DeviceError> {
        todo!("M2")
    }
}

/// Lock shared by every [`FakeHost`] of one test (two engines contend for it).
#[derive(Debug, Clone, Default)]
pub struct FakeLockCell(Rc<Cell<bool>>);

/// Guard of a [`FakeLockCell`]; releases it on drop.
#[derive(Debug)]
pub struct FakeLockGuard(Rc<Cell<bool>>);

impl Drop for FakeLockGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// In-memory [`Host`] with a manual clock, settable boot ID and process table.
#[derive(Debug, Clone)]
pub struct FakeHost {
    lock: FakeLockCell,
    now: Timestamp,
    monotonic: Duration,
    boot: BootId,
    process: ProcessIdentity,
    /// Processes that run, besides `process`.
    alive: Vec<ProcessIdentity>,
    next_op: u64,
    assets: Option<RecoveryAssets>,
    fail_assets: bool,
    /// System32 files that do not exist (default: every file exists).
    missing_system32: Vec<String>,
    warnings: Vec<String>,
}

impl FakeHost {
    pub fn new(lock: FakeLockCell) -> Self {
        Self {
            lock,
            now: Timestamp(1_790_000_000_000),
            monotonic: Duration::ZERO,
            boot: BootId(1),
            process: ProcessIdentity {
                pid: 1000,
                creation_time: 1,
            },
            alive: Vec::new(),
            next_op: 1,
            assets: None,
            fail_assets: false,
            missing_system32: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Makes [`Host::system32_file_exists`] answer false for `file_name` (plan 1.5 check).
    pub fn remove_system32_file(&mut self, file_name: &str) {
        self.missing_system32.push(file_name.to_ascii_lowercase());
    }

    /// Queues a warning for [`Host::drain_warnings`] (e.g. a quarantined data directory).
    pub fn push_warning(&mut self, warning: &str) {
        self.warnings.push(warning.to_string());
    }

    /// A new boot: new boot ID, no process of the old boot runs.
    pub fn reboot(&mut self) {
        todo!("M2")
    }

    /// Continue as another process (e.g. the GUI that recovers after the helper died); the old
    /// one stays alive unless [`FakeHost::kill`] is called.
    pub fn become_process(&mut self, process: ProcessIdentity) {
        todo!("M2")
    }

    pub fn kill(&mut self, process: &ProcessIdentity) {
        todo!("M2")
    }

    /// Moves both the wall clock and the monotonic clock.
    pub fn advance(&mut self, by: Duration) {
        todo!("M2")
    }

    /// Makes [`Host::write_recovery_assets`] fail: an operation that changes boot-time values then
    /// fails with `RecoveryAssetsUnavailable` before journaling anything; a HID-only one goes on
    /// with a warning (design review C10).
    pub fn set_fail_assets(&mut self, fail: bool) {
        self.fail_assets = fail;
    }

    /// The last recovery files written.
    pub fn recovery_assets(&self) -> Option<&RecoveryAssets> {
        self.assets.as_ref()
    }
}

impl Host for FakeHost {
    type Lock = FakeLockGuard;

    fn acquire_lock(&mut self, timeout: Duration) -> Result<Self::Lock, HostError> {
        todo!("M2")
    }

    fn now(&self) -> Timestamp {
        self.now
    }

    fn monotonic(&self) -> Duration {
        self.monotonic
    }

    fn new_op_id(&mut self) -> Result<OpId, HostError> {
        todo!("M2: deterministic UUIDs from next_op")
    }

    fn boot_id(&self) -> Result<BootId, HostError> {
        Ok(self.boot)
    }

    fn boot_time_hint(&self) -> Option<u64> {
        None
    }

    fn current_process(&self) -> Result<ProcessIdentity, HostError> {
        Ok(self.process)
    }

    fn liveness(&self, process: &ProcessIdentity) -> Liveness {
        todo!("M2")
    }

    fn system32_file_exists(&self, file_name: &str) -> Result<bool, HostError> {
        Ok(!self
            .missing_system32
            .contains(&file_name.to_ascii_lowercase()))
    }

    fn write_recovery_assets(&mut self, assets: &RecoveryAssets) -> Result<(), HostError> {
        todo!("M2")
    }

    fn drain_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}

/// [`EventSink`] that records events and answers from a script (then `NoDecision` forever).
/// With [`ScriptedSink::cancel_after`], the caller "goes away" after that many events:
/// [`EventSink::check_cancelled`] turns true and every later wait is `Disconnected`
/// (design review S4).
#[derive(Debug, Clone, Default)]
pub struct ScriptedSink {
    pub events: Vec<Event>,
    script: VecDeque<DecisionPoll>,
    cancel_after_events: Option<usize>,
}

impl ScriptedSink {
    pub fn new(script: impl IntoIterator<Item = DecisionPoll>) -> Self {
        Self {
            events: Vec::new(),
            script: script.into_iter().collect(),
            cancel_after_events: None,
        }
    }

    /// The caller disconnects once `n` events were received (`0`: before the first one).
    pub fn cancel_after(mut self, n: usize) -> Self {
        self.cancel_after_events = Some(n);
        self
    }

    fn cancelled(&self) -> bool {
        self.cancel_after_events
            .is_some_and(|n| self.events.len() >= n)
    }
}

impl EventSink for ScriptedSink {
    fn event(&mut self, event: &Event) {
        self.events.push(event.clone());
    }

    fn check_cancelled(&mut self) -> bool {
        self.cancelled()
    }

    fn wait_decision(&mut self, _timeout: Duration) -> DecisionPoll {
        if self.cancelled() {
            return DecisionPoll::Disconnected;
        }
        self.script.pop_front().unwrap_or(DecisionPoll::NoDecision)
    }
}
