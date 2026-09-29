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
//!
//! Outside edits ([`MemoryRegistry::outside_edit`]) are durable at once: they model another process
//! that writes and flushes (`RegFlushKey` flushes the whole hive), so every earlier SYSTEM-hive call
//! is durable from then on as well.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use mklm_core::{
    BootId, DEVICE_VALUE_NAMES, Decision, DeviceOverrides, Event, GLOBAL_VALUE_NAMES,
    GlobalSettings, KeyboardDevice, KeyboardType, Liveness, MACHINE_SETTING_NAMES, OpId,
    ProcessIdentity, RecoveryAssets, RegValue, STORE_VERSION, Timestamp, WriteTarget, predict_type,
    value_names,
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
    /// A machine-wide setting (`RegistryBackend::write_machine_setting`; logged with the
    /// SOFTWARE-hive flush that the same call makes).
    WriteSetting {
        name: String,
        value: u32,
    },
}

impl Mutation {
    /// The hive the call touches.
    pub fn hive(&self) -> Hive {
        match self {
            Mutation::WriteValue { .. } => Hive::System,
            Mutation::WriteJournal { .. }
            | Mutation::DeleteJournal { .. }
            | Mutation::WriteSetting { .. } => Hive::Software,
            Mutation::Flush { hive } => *hive,
        }
    }
}

/// Faults to inject, counted over mutating calls (1-based, from the last
/// [`MemoryRegistry::set_faults`] or from creation).
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
    /// With `deny_target`: reads of that target fail with [`BackendError::AccessDenied`] too (a
    /// DACL that denies reading as well), also after [`MemoryRegistry::after_crash`].
    pub deny_reads: bool,
    /// Read `n` (`read_value` and `list_values`, 1-based, counted separately from the mutating
    /// calls) fails with [`BackendError::Os`]; later reads work. Error-path tests of the reads
    /// (design review C3: no non-crash error may leave an entry in flight).
    pub fail_read_at: Option<usize>,
}

/// `ERROR_REGISTRY_IO_FAILED`, the code of an injected read failure.
const INJECTED_READ_ERROR: u32 = 1016;

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct RegistryContents {
    /// Upper-case instance ID → value name → value.
    pub devices: BTreeMap<String, BTreeMap<String, RegValue>>,
    pub global: BTreeMap<String, RegValue>,
    pub store_version: Option<u32>,
    pub ops: BTreeMap<String, String>,
    pub baselines: BTreeMap<String, String>,
    /// Values under `mklm_core::MACHINE_SETTINGS_KEY`.
    pub settings: BTreeMap<String, u32>,
}

impl RegistryContents {
    /// The value `name` of `target` ([`RegValue::Absent`] when missing). Names compare
    /// case-insensitively, like registry value names.
    pub fn value(&self, target: &WriteTarget, name: &str) -> RegValue {
        let key = match target {
            WriteTarget::Device { instance_id } => {
                match self.devices.get(&instance_id.to_ascii_uppercase()) {
                    Some(key) => key,
                    None => return RegValue::Absent,
                }
            }
            WriteTarget::Global => &self.global,
        };
        key.iter()
            .find(|(stored, _)| stored.eq_ignore_ascii_case(name))
            .map_or(RegValue::Absent, |(_, value)| value.clone())
    }

    fn set_value(&mut self, target: &WriteTarget, name: &str, value: &RegValue) {
        let key = match target {
            WriteTarget::Device { instance_id } => self
                .devices
                .entry(instance_id.to_ascii_uppercase())
                .or_default(),
            WriteTarget::Global => &mut self.global,
        };
        key.retain(|stored, _| !stored.eq_ignore_ascii_case(name));
        if *value != RegValue::Absent {
            key.insert(name.to_string(), value.clone());
        }
    }

    fn slot_map(&mut self, slot: &JournalSlot) -> (&mut BTreeMap<String, String>, String) {
        match slot {
            JournalSlot::Op(op_id) => (&mut self.ops, op_id.as_str().to_string()),
            JournalSlot::Baseline(name) => (&mut self.baselines, name.clone()),
        }
    }

    fn apply(&mut self, item: &Logged) {
        match item {
            Logged::Call(Mutation::WriteValue {
                target,
                name,
                value,
            })
            | Logged::Outside {
                target,
                name,
                value,
            } => self.set_value(target, name, value),
            Logged::Call(Mutation::WriteJournal { slot, json }) => {
                self.store_version = Some(STORE_VERSION);
                let (map, name) = self.slot_map(slot);
                map.retain(|stored, _| !stored.eq_ignore_ascii_case(&name));
                map.insert(name, json.clone());
            }
            Logged::Call(Mutation::DeleteJournal { slot }) => {
                let (map, name) = self.slot_map(slot);
                map.retain(|stored, _| !stored.eq_ignore_ascii_case(&name));
            }
            Logged::Call(Mutation::WriteSetting { name, value }) => {
                self.settings
                    .retain(|stored, _| !stored.eq_ignore_ascii_case(name));
                self.settings.insert(name.clone(), *value);
            }
            Logged::Call(Mutation::Flush { .. }) => {}
        }
    }
}

/// One entry of the internal log: an engine call, or an outside edit (durable at once).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Logged {
    Call(Mutation),
    Outside {
        target: WriteTarget,
        name: String,
        value: RegValue,
    },
}

impl Logged {
    fn hive(&self) -> Hive {
        match self {
            Logged::Call(call) => call.hive(),
            Logged::Outside { .. } => Hive::System,
        }
    }

    /// True when everything logged before it in `hive` is durable once it completed.
    fn is_barrier(&self, hive: Hive) -> bool {
        match self {
            Logged::Call(Mutation::Flush { hive: flushed }) => *flushed == hive,
            Logged::Outside { .. } => hive == Hive::System,
            Logged::Call(_) => false,
        }
    }
}

#[derive(Debug, Default)]
struct MemoryState {
    /// Durable contents at creation (seeded values).
    initial: RegistryContents,
    /// Contents as the running system sees them.
    current: RegistryContents,
    log: Vec<Logged>,
    faults: FaultPlan,
    mutating_calls: usize,
    /// Reads made since the last `set_faults` (for `fail_read_at`).
    reads: usize,
    crashed: bool,
    /// Upper-case instance IDs of removed devnodes.
    removed: BTreeSet<String>,
}

impl MemoryState {
    fn check_alive(&self) -> Result<(), BackendError> {
        if self.crashed {
            Err(BackendError::Crashed)
        } else {
            Ok(())
        }
    }

    /// Counts one mutating call and applies `crash_after` / `fail_at`.
    fn begin_mutation(&mut self) -> Result<(), BackendError> {
        self.check_alive()?;
        self.mutating_calls += 1;
        if self
            .faults
            .crash_after
            .is_some_and(|n| self.mutating_calls > n)
        {
            self.crashed = true;
            return Err(BackendError::Crashed);
        }
        if self.faults.fail_at == Some(self.mutating_calls) {
            return Err(BackendError::Injected {
                call: self.mutating_calls,
            });
        }
        Ok(())
    }

    /// Counts one read of `target` and applies `fail_read_at` and `deny_reads`.
    fn begin_read(&mut self, target: &WriteTarget) -> Result<(), BackendError> {
        self.check_alive()?;
        self.reads += 1;
        if self.faults.fail_read_at == Some(self.reads) {
            return Err(BackendError::Os {
                what: target_text(target),
                code: INJECTED_READ_ERROR,
            });
        }
        if self.faults.deny_reads && self.faults.deny_target.as_ref() == Some(target) {
            return Err(BackendError::AccessDenied {
                what: target_text(target),
            });
        }
        Ok(())
    }

    fn check_device(&self, target: &WriteTarget) -> Result<(), BackendError> {
        match target {
            WriteTarget::Device { instance_id }
                if self.removed.contains(&instance_id.to_ascii_uppercase()) =>
            {
                Err(BackendError::DeviceRemoved {
                    instance_id: instance_id.clone(),
                })
            }
            _ => Ok(()),
        }
    }

    fn log(&mut self, item: Logged) {
        self.current.apply(&item);
        self.log.push(item);
    }
}

fn target_text(target: &WriteTarget) -> String {
    match target {
        WriteTarget::Device { instance_id } => format!(r"Enum\{instance_id}\Device Parameters"),
        WriteTarget::Global => r"Services\i8042prt\Parameters".to_string(),
    }
}

fn as_dword(value: &RegValue) -> Option<u32> {
    match value {
        RegValue::Dword { value } => Some(*value),
        _ => None,
    }
}

fn as_string(value: &RegValue) -> Option<String> {
    match value {
        RegValue::Sz { value } => Some(value.clone()),
        _ => None,
    }
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

    /// Sets a durable value before the test starts (not logged, not counted). Once calls were
    /// logged, it behaves like [`MemoryRegistry::outside_edit`].
    pub fn seed(&self, target: &WriteTarget, name: &str, value: RegValue) {
        let mut state = self.state.borrow_mut();
        if state.log.is_empty() {
            state.initial.set_value(target, name, &value);
            state.current.set_value(target, name, &value);
        } else {
            drop(state);
            self.outside_edit(target, name, value);
        }
    }

    /// Somebody else (Settings app, Windows Update, another admin) changes a value: applied and
    /// durable at once, not counted, never faulted. Used for conflict tests. Ignored for a removed
    /// devnode.
    pub fn outside_edit(&self, target: &WriteTarget, name: &str, value: RegValue) {
        let mut state = self.state.borrow_mut();
        if state.check_device(target).is_err() {
            return;
        }
        state.log(Logged::Outside {
            target: target.clone(),
            name: name.to_string(),
            value,
        });
    }

    /// Replaces the fault plan and restarts the call count, so that `crash_after` and `fail_at`
    /// count from here.
    pub fn set_faults(&self, faults: FaultPlan) {
        let mut state = self.state.borrow_mut();
        state.faults = faults;
        state.mutating_calls = 0;
        state.reads = 0;
    }

    /// Reads (`read_value`, `list_values`) made since the last [`MemoryRegistry::set_faults`]
    /// (the upper bound for `fail_read_at` loops).
    pub fn reads(&self) -> usize {
        self.state.borrow().reads
    }

    /// The devnode disappears (removed in Device Manager): reads and writes of its values fail
    /// with [`BackendError::DeviceRemoved`]. Not logged, not counted.
    pub fn remove_devnode(&self, instance_id: &str) {
        let mut state = self.state.borrow_mut();
        let id = instance_id.to_ascii_uppercase();
        state.current.devices.remove(&id);
        state.removed.insert(id);
    }

    /// Mutating calls made so far (the upper bound for `crash_after` loops).
    pub fn mutating_calls(&self) -> usize {
        self.state.borrow().mutating_calls
    }

    /// True once a `crash_after` fault fired.
    pub fn crashed(&self) -> bool {
        self.state.borrow().crashed
    }

    /// The logged calls, in order (outside edits are not calls and are left out).
    pub fn mutations(&self) -> Vec<Mutation> {
        self.state
            .borrow()
            .log
            .iter()
            .filter_map(|item| match item {
                Logged::Call(call) => Some(call.clone()),
                Logged::Outside { .. } => None,
            })
            .collect()
    }

    /// Current (volatile) contents.
    pub fn contents(&self) -> RegistryContents {
        self.state.borrow().current.clone()
    }

    /// The current value, bypassing faults (for tests and the fake devices).
    pub fn value(&self, target: &WriteTarget, name: &str) -> RegValue {
        self.state.borrow().current.value(target, name)
    }

    /// A machine-wide setting as written (`None`: never written), bypassing faults.
    pub fn machine_setting(&self, name: &str) -> Option<u32> {
        self.state
            .borrow()
            .current
            .settings
            .iter()
            .find(|(stored, _)| stored.eq_ignore_ascii_case(name))
            .map(|(_, value)| *value)
    }

    /// The device override values as a driver would read them now (`REG_DWORD` only).
    pub fn device_overrides(&self, instance_id: &str) -> DeviceOverrides {
        let state = self.state.borrow();
        let target = WriteTarget::Device {
            instance_id: instance_id.to_string(),
        };
        let dword = |name: &str| as_dword(&state.current.value(&target, name));
        DeviceOverrides {
            keyboard_type_override: dword(value_names::HID_TYPE),
            keyboard_subtype_override: dword(value_names::HID_SUBTYPE),
            override_keyboard_type: dword(value_names::PS2_TYPE),
            override_keyboard_subtype: dword(value_names::PS2_SUBTYPE),
            number_total_keys_override: dword(value_names::HID_TOTAL_KEYS),
            number_function_keys_override: dword(value_names::HID_FUNCTION_KEYS),
            number_indicators_override: dword(value_names::HID_INDICATORS),
        }
    }

    /// The global values as the drivers would read them now.
    pub fn global_settings(&self) -> GlobalSettings {
        let state = self.state.borrow();
        let value = |name: &str| state.current.value(&WriteTarget::Global, name);
        GlobalSettings {
            layer_driver_jpn: as_string(&value(value_names::LAYER_DRIVER_JPN)),
            layer_driver_kor: as_string(&value(value_names::LAYER_DRIVER_KOR)),
            override_keyboard_identifier: as_string(&value(value_names::KEYBOARD_IDENTIFIER)),
            override_keyboard_type: as_dword(&value(value_names::PS2_TYPE)),
            override_keyboard_subtype: as_dword(&value(value_names::PS2_SUBTYPE)),
        }
    }

    /// Every durable state consistent with the flush history (see the module docs). The
    /// power-loss image that keeps every call equals [`CrashImage::ProcessKill`] and is not listed
    /// twice; power-loss images differ from each other in content.
    pub fn crash_images(&self) -> Vec<CrashImage> {
        let state = self.state.borrow();
        let bounds = |hive: Hive| {
            let items: Vec<&Logged> = state.log.iter().filter(|i| i.hive() == hive).collect();
            // Everything up to the last barrier is durable; after it there is no flush of this
            // hive, so every longer prefix has different content.
            let durable = items
                .iter()
                .rposition(|item| item.is_barrier(hive))
                .map_or(0, |p| p + 1);
            (durable, items.len())
        };
        let (system_min, system_len) = bounds(Hive::System);
        let (software_min, software_len) = bounds(Hive::Software);
        let mut images = vec![CrashImage::ProcessKill];
        for system_calls in system_min..=system_len {
            for software_calls in software_min..=software_len {
                if (system_calls, software_calls) != (system_len, software_len) {
                    images.push(CrashImage::PowerLoss {
                        system_calls,
                        software_calls,
                    });
                }
            }
        }
        images
    }

    /// A new, independent registry holding `image`'s contents, with no faults except
    /// `deny_target`, an empty log and a zero call count. Removed devnodes stay removed.
    pub fn after_crash(&self, image: CrashImage) -> MemoryRegistry {
        let state = self.state.borrow();
        let mut contents = state.initial.clone();
        match image {
            CrashImage::ProcessKill => {
                for item in &state.log {
                    contents.apply(item);
                }
            }
            CrashImage::PowerLoss {
                system_calls,
                software_calls,
            } => {
                let mut kept_system = 0;
                let mut kept_software = 0;
                for item in &state.log {
                    let (kept, limit) = match item.hive() {
                        Hive::System => (&mut kept_system, system_calls),
                        Hive::Software => (&mut kept_software, software_calls),
                    };
                    if *kept < limit {
                        contents.apply(item);
                    }
                    *kept += 1;
                }
            }
        }
        for id in &state.removed {
            contents.devices.remove(id);
        }
        MemoryRegistry {
            state: Rc::new(RefCell::new(MemoryState {
                initial: contents.clone(),
                current: contents,
                log: Vec::new(),
                faults: FaultPlan {
                    deny_target: state.faults.deny_target.clone(),
                    deny_reads: state.faults.deny_reads,
                    ..FaultPlan::default()
                },
                mutating_calls: 0,
                reads: 0,
                crashed: false,
                removed: state.removed.clone(),
            })),
        }
    }

    /// A new, independent registry whose durable contents are everything written so far (as
    /// if every hive had been flushed). Keeps all faults' configuration out, like
    /// [`MemoryRegistry::after_crash`]. Tests use it to start a scenario from a prepared state.
    pub fn checkpoint(&self) -> MemoryRegistry {
        self.after_crash(CrashImage::ProcessKill)
    }
}

impl RegistryBackend for MemoryRegistry {
    fn read_value(&self, target: &WriteTarget, name: &str) -> Result<RegValue, BackendError> {
        let mut state = self.state.borrow_mut();
        state.begin_read(target)?;
        state.check_device(target)?;
        Ok(state.current.value(target, name))
    }

    fn list_values(&self, target: &WriteTarget) -> Result<Vec<(String, RegValue)>, BackendError> {
        let mut state = self.state.borrow_mut();
        state.begin_read(target)?;
        state.check_device(target)?;
        let key = match target {
            WriteTarget::Device { instance_id } => {
                state.current.devices.get(&instance_id.to_ascii_uppercase())
            }
            WriteTarget::Global => Some(&state.current.global),
        };
        Ok(key
            .map(|key| {
                key.iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn write_value(
        &mut self,
        target: &WriteTarget,
        name: &str,
        value: &RegValue,
    ) -> Result<(), BackendError> {
        let mut state = self.state.borrow_mut();
        state.begin_mutation()?;
        let allowed: &[&str] = match target {
            WriteTarget::Device { .. } => &DEVICE_VALUE_NAMES,
            WriteTarget::Global => &GLOBAL_VALUE_NAMES,
        };
        if !allowed.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            return Err(BackendError::NameNotAllowed {
                name: name.to_string(),
            });
        }
        state.check_device(target)?;
        if state.faults.deny_target.as_ref() == Some(target) {
            return Err(BackendError::AccessDenied {
                what: format!("{}\\{name}", target_text(target)),
            });
        }
        state.log(Logged::Call(Mutation::WriteValue {
            target: target.clone(),
            name: name.to_string(),
            value: value.clone(),
        }));
        Ok(())
    }

    fn flush_target(&mut self, target: &WriteTarget) -> Result<(), BackendError> {
        let mut state = self.state.borrow_mut();
        state.begin_mutation()?;
        state.check_device(target)?;
        state.log(Logged::Call(Mutation::Flush { hive: Hive::System }));
        Ok(())
    }

    fn read_journal(&self) -> Result<JournalDump, BackendError> {
        let state = self.state.borrow();
        state.check_alive()?;
        let pairs = |map: &BTreeMap<String, String>| {
            map.iter()
                .map(|(name, json)| (name.clone(), json.clone()))
                .collect()
        };
        Ok(JournalDump {
            store_version: state.current.store_version,
            ops: pairs(&state.current.ops),
            baselines: pairs(&state.current.baselines),
        })
    }

    fn write_journal(&mut self, slot: &JournalSlot, json: &str) -> Result<(), BackendError> {
        let mut state = self.state.borrow_mut();
        state.begin_mutation()?;
        state.log(Logged::Call(Mutation::WriteJournal {
            slot: slot.clone(),
            json: json.to_string(),
        }));
        Ok(())
    }

    fn delete_journal(&mut self, slot: &JournalSlot) -> Result<(), BackendError> {
        let mut state = self.state.borrow_mut();
        state.begin_mutation()?;
        state.log(Logged::Call(Mutation::DeleteJournal { slot: slot.clone() }));
        Ok(())
    }

    fn flush_journal(&mut self) -> Result<(), BackendError> {
        let mut state = self.state.borrow_mut();
        state.begin_mutation()?;
        state.log(Logged::Call(Mutation::Flush {
            hive: Hive::Software,
        }));
        Ok(())
    }

    /// One mutating call that writes and flushes, like the real one.
    fn write_machine_setting(&mut self, name: &str, value: u32) -> Result<(), BackendError> {
        let mut state = self.state.borrow_mut();
        state.begin_mutation()?;
        let Some(name) = MACHINE_SETTING_NAMES
            .iter()
            .find(|allowed| allowed.eq_ignore_ascii_case(name))
        else {
            return Err(BackendError::NameNotAllowed {
                name: name.to_string(),
            });
        };
        state.log(Logged::Call(Mutation::WriteSetting {
            name: (*name).to_string(),
            value,
        }));
        state.log(Logged::Call(Mutation::Flush {
            hive: Hive::Software,
        }));
        Ok(())
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
///
/// Each keyboard has a *running* type: what its driver reports now. It starts as the fixture's
/// `reported_type` (call [`FakeDevices::reboot`] after seeding the registry to derive it from the
/// values instead) and changes only on a reset that applies, [`FakeDevices::reconnect`] and
/// [`FakeDevices::reboot`].
#[derive(Debug, Clone)]
pub struct FakeDevices {
    registry: MemoryRegistry,
    keyboards: Vec<KeyboardDevice>,
    resets: BTreeMap<String, FakeReset>,
    /// One-shot behaviours, used before `resets`.
    queued_resets: BTreeMap<String, VecDeque<FakeReset>>,
    /// Upper-case instance ID → type the driver reports now (`None`: not listed by Raw Input).
    running: BTreeMap<String, Option<KeyboardType>>,
    /// Upper-case instance ID → behaviour of the last reset (decides the arrival).
    last_reset: BTreeMap<String, FakeReset>,
    restarted: Vec<String>,
    incomplete: bool,
    warnings: Vec<String>,
}

impl FakeDevices {
    pub fn new(registry: MemoryRegistry, keyboards: Vec<KeyboardDevice>) -> Self {
        let running = keyboards
            .iter()
            .map(|kb| {
                let reported = if kb.present { kb.reported_type } else { None };
                (kb.instance_id.to_ascii_uppercase(), reported)
            })
            .collect();
        Self {
            registry,
            keyboards,
            resets: BTreeMap::new(),
            queued_resets: BTreeMap::new(),
            running,
            last_reset: BTreeMap::new(),
            restarted: Vec::new(),
            incomplete: false,
            warnings: Vec::new(),
        }
    }

    /// The same devices (running types, behaviours, history) on another registry, e.g. one made
    /// by [`MemoryRegistry::after_crash`].
    pub fn with_registry(&self, registry: MemoryRegistry) -> Self {
        Self {
            registry,
            ..self.clone()
        }
    }

    pub fn set_reset(&mut self, instance_id: &str, behaviour: FakeReset) {
        self.resets
            .insert(instance_id.to_ascii_uppercase(), behaviour);
    }

    /// Makes only the next reset of `instance_id` behave as `behaviour` (queued in order, before
    /// the behaviour of [`FakeDevices::set_reset`]).
    pub fn push_reset(&mut self, instance_id: &str, behaviour: FakeReset) {
        self.queued_resets
            .entry(instance_id.to_ascii_uppercase())
            .or_default()
            .push_back(behaviour);
    }

    /// Makes [`DeviceController::keyboards`] fail with [`DeviceError::Incomplete`] (a
    /// write-blocking read issue: devnode set, driver or presence unknown).
    pub fn set_incomplete(&mut self, incomplete: bool) {
        self.incomplete = incomplete;
    }

    /// Non-blocking read issues [`DeviceController::keyboards`] reports as
    /// [`Inventory::warnings`] (names, Raw Input, a `REG_SZ` override…); writes must go on.
    pub fn set_warnings(&mut self, warnings: Vec<String>) {
        self.warnings = warnings;
    }

    /// Adds a keyboard to the inventory (e.g. a phantom PS/2 keyboard of a dock). Its running
    /// type is its fixture's `reported_type` when present.
    pub fn add_keyboard(&mut self, keyboard: KeyboardDevice) {
        let reported = if keyboard.present {
            keyboard.reported_type
        } else {
            None
        };
        self.running
            .insert(keyboard.instance_id.to_ascii_uppercase(), reported);
        self.keyboards.push(keyboard);
    }

    /// Unplug and replug: the reported type is re-read from the registry.
    pub fn reconnect(&mut self, instance_id: &str) {
        if let Some(kb) = self.find(instance_id).filter(|kb| kb.present) {
            let predicted = self.predicted(kb);
            self.running
                .insert(instance_id.to_ascii_uppercase(), predicted);
        }
    }

    /// PC restart: every keyboard's reported type is re-read from the registry (with the global
    /// values, as `mklm_core::predict_type` does).
    pub fn reboot(&mut self) {
        let running: Vec<(String, Option<KeyboardType>)> = self
            .keyboards
            .iter()
            .map(|kb| {
                let reported = if kb.present { self.predicted(kb) } else { None };
                (kb.instance_id.to_ascii_uppercase(), reported)
            })
            .collect();
        self.running = running.into_iter().collect();
        self.last_reset.clear();
    }

    /// The devnode is removed: it leaves the inventory and [`MemoryRegistry::remove_devnode`]
    /// applies.
    pub fn remove(&mut self, instance_id: &str) {
        self.keyboards
            .retain(|kb| !kb.instance_id.eq_ignore_ascii_case(instance_id));
        self.running.remove(&instance_id.to_ascii_uppercase());
        self.registry.remove_devnode(instance_id);
    }

    /// Instance IDs reset so far, in order.
    pub fn restarted(&self) -> &[String] {
        &self.restarted
    }

    /// The type `instance_id`'s driver reports now (`None`: not listed).
    pub fn running_type(&self, instance_id: &str) -> Option<KeyboardType> {
        self.running
            .get(&instance_id.to_ascii_uppercase())
            .copied()
            .flatten()
    }

    /// Every keyboard with its running type (for comparisons in tests).
    pub fn running_types(&self) -> BTreeMap<String, Option<KeyboardType>> {
        self.running.clone()
    }

    /// The keyboards as listed, with the registry's current values.
    pub fn listed(&self) -> Vec<KeyboardDevice> {
        self.keyboards
            .iter()
            .map(|kb| {
                let mut kb = kb.clone();
                kb.overrides = self.registry.device_overrides(&kb.instance_id);
                kb.reported_type = if kb.present {
                    self.running_type(&kb.instance_id)
                } else {
                    None
                };
                kb
            })
            .collect()
    }

    /// The type the registry's current values give `kb`.
    pub fn predicted(&self, kb: &KeyboardDevice) -> Option<KeyboardType> {
        predict_type(
            &kb.driver,
            &self.registry.device_overrides(&kb.instance_id),
            &self.registry.global_settings(),
        )
    }

    fn find(&self, instance_id: &str) -> Option<&KeyboardDevice> {
        self.keyboards
            .iter()
            .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
    }

    fn not_found(instance_id: &str) -> DeviceError {
        DeviceError::NotFound {
            instance_id: instance_id.to_string(),
        }
    }
}

impl DeviceController for FakeDevices {
    fn keyboards(&mut self) -> Result<Inventory, DeviceError> {
        if self.incomplete {
            return Err(DeviceError::Incomplete {
                issues: vec!["simulated: the Keyboard-class devnodes could not be listed".into()],
            });
        }
        Ok(Inventory {
            keyboards: self.listed(),
            warnings: self.warnings.clone(),
        })
    }

    fn restart(&mut self, instance_id: &str) -> Result<RestartOutcome, DeviceError> {
        let kb = self
            .find(instance_id)
            .cloned()
            .ok_or_else(|| Self::not_found(instance_id))?;
        let key = instance_id.to_ascii_uppercase();
        self.restarted.push(kb.instance_id.clone());
        let behaviour = self
            .queued_resets
            .get_mut(&key)
            .and_then(VecDeque::pop_front)
            .or_else(|| self.resets.get(&key).copied())
            .unwrap_or_default();
        self.last_reset.insert(key.clone(), behaviour);
        match behaviour {
            FakeReset::Applies => {
                let predicted = self.predicted(&kb);
                self.running.insert(key, predicted);
                Ok(RestartOutcome::Restarted)
            }
            FakeReset::NeedsReboot => Ok(RestartOutcome::NeedsReboot),
            FakeReset::NeverArrives => {
                self.running.insert(key, None);
                Ok(RestartOutcome::Restarted)
            }
            FakeReset::ArrivesUnchanged => Ok(RestartOutcome::Restarted),
            FakeReset::Fails => Err(DeviceError::Injected),
            FakeReset::Hangs => Ok(RestartOutcome::TimedOut),
        }
    }

    fn wait_for_arrival(
        &mut self,
        instance_id: &str,
        _timeout: Duration,
    ) -> Result<Arrival, DeviceError> {
        let kb = self
            .find(instance_id)
            .ok_or_else(|| Self::not_found(instance_id))?;
        if !kb.present {
            return Ok(Arrival::TimedOut);
        }
        let key = instance_id.to_ascii_uppercase();
        match self.last_reset.get(&key) {
            Some(FakeReset::NeverArrives) => Ok(Arrival::TimedOut),
            _ => Ok(Arrival::Started {
                reported: self.running_type(instance_id),
            }),
        }
    }

    fn reported_type(&mut self, instance_id: &str) -> Result<Option<KeyboardType>, DeviceError> {
        let kb = self
            .find(instance_id)
            .ok_or_else(|| Self::not_found(instance_id))?;
        if !kb.present {
            return Ok(None);
        }
        Ok(self.running_type(instance_id))
    }
}

/// Lock shared by every [`FakeHost`] of one test (two engines contend for it).
#[derive(Debug, Clone, Default)]
pub struct FakeLockCell(Rc<Cell<bool>>);

impl FakeLockCell {
    /// True while some [`FakeLockGuard`] of this cell is alive.
    pub fn is_locked(&self) -> bool {
        self.0.get()
    }
}

/// Guard of a [`FakeLockCell`]; releases it on drop.
#[derive(Debug)]
pub struct FakeLockGuard(Rc<Cell<bool>>);

impl Drop for FakeLockGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Wall and monotonic clock of a [`FakeHost`], shared by its clones: a test (or a sink it wrote)
/// keeps a handle and moves time while the engine runs.
#[derive(Debug, Clone)]
pub struct FakeClock(Rc<Cell<(u64, Duration)>>);

impl FakeClock {
    fn new(now_ms: u64) -> Self {
        Self(Rc::new(Cell::new((now_ms, Duration::ZERO))))
    }

    /// Moves both clocks.
    pub fn advance(&self, by: Duration) {
        let (now, monotonic) = self.0.get();
        let millis = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
        self.0.set((now.saturating_add(millis), monotonic + by));
    }

    pub fn now(&self) -> Timestamp {
        Timestamp(self.0.get().0)
    }

    pub fn monotonic(&self) -> Duration {
        self.0.get().1
    }
}

/// The loader GUID [`FakeHost`] reports in its first boot (one more in each later boot, unless a
/// test sets another with [`FakeHost::set_legacy_guid`]): what its boot ID was before 0.1.1.
const FAKE_FIRST_LEGACY_GUID: u128 = 0x9b1c_0d6e_2f4a_4c8b_a1d3_5e6f_0000_0001;

/// In-memory [`Host`] with a manual clock, settable boot ID and process table.
#[derive(Debug, Clone)]
pub struct FakeHost {
    lock: FakeLockCell,
    clock: FakeClock,
    /// `KUSER_SHARED_DATA.BootId`: 1 in the first boot, one more after each [`FakeHost::reboot`].
    boot_counter: u32,
    /// What [`Host::legacy_boot_guid`] answers when a test set it (kept across reboots, like the
    /// GUID of the desktop PC that 0.1.0 never saw restart); the default sequence otherwise.
    legacy_guid: Option<Option<BootId>>,
    /// What [`Host::boot_time_hint`] answers (`None` unless a test sets it, so that the history
    /// lines the tests compare stay as they were).
    boot_time: Option<u64>,
    process: ProcessIdentity,
    /// Processes that run, besides `process`.
    alive: Vec<ProcessIdentity>,
    next_op: u64,
    assets: Option<RecoveryAssets>,
    asset_writes: usize,
    fail_assets: bool,
    /// System32 files that do not exist (default: every file exists).
    missing_system32: Vec<String>,
    warnings: Vec<String>,
    /// A quarantine moved the recovery files away since the last `take_recovery_assets_moved`.
    assets_moved: bool,
}

impl FakeHost {
    pub fn new(lock: FakeLockCell) -> Self {
        Self {
            lock,
            clock: FakeClock::new(1_790_000_000_000),
            boot_counter: 1,
            legacy_guid: None,
            boot_time: None,
            process: ProcessIdentity {
                pid: 1000,
                creation_time: 1,
            },
            alive: Vec::new(),
            next_op: 1,
            assets: None,
            asset_writes: 0,
            fail_assets: false,
            missing_system32: Vec::new(),
            warnings: Vec::new(),
            assets_moved: false,
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

    /// A squatted base directory was quarantined (design review S1): the warning is queued and
    /// the recovery files are gone from their place until written again.
    pub fn quarantine_base(&mut self) {
        self.warnings
            .push("a squatted data directory was quarantined".to_string());
        self.assets = None;
        self.assets_moved = true;
    }

    /// A new boot: the boot counter goes up by one (a new boot ID), no process of the old boot
    /// runs, and this host continues as a new process of the new boot. A boot time or legacy GUID
    /// a test set stays as it was.
    pub fn reboot(&mut self) {
        self.boot_counter = self.boot_counter.wrapping_add(1);
        self.alive.clear();
        self.process = ProcessIdentity {
            pid: self.process.pid.wrapping_add(1),
            creation_time: self.process.creation_time.wrapping_add(1),
        };
    }

    /// Continue as another process (e.g. the GUI that recovers after the helper died); the old
    /// one stays alive unless [`FakeHost::kill`] is called.
    pub fn become_process(&mut self, process: ProcessIdentity) {
        if process != self.process && !self.alive.contains(&self.process) {
            self.alive.push(self.process);
        }
        self.alive.retain(|p| *p != process);
        self.process = process;
    }

    pub fn kill(&mut self, process: &ProcessIdentity) {
        self.alive.retain(|p| p != process);
    }

    /// The host of the same machine after the current process died (killed or crashed): same
    /// boot, clock, lock, System32 files and recovery files, now running as `process`.
    pub fn after_crash(&self, process: ProcessIdentity) -> FakeHost {
        let mut host = self.clone();
        let dead = host.process;
        host.become_process(process);
        host.kill(&dead);
        host
    }

    /// Moves both the wall clock and the monotonic clock.
    pub fn advance(&mut self, by: Duration) {
        self.clock.advance(by);
    }

    /// A handle on this host's clock (shared with its clones).
    pub fn clock(&self) -> FakeClock {
        self.clock.clone()
    }

    /// The current boot's ID: the counter form of the fake `KUSER_SHARED_DATA.BootId`.
    pub fn current_boot(&self) -> BootId {
        BootId::from_boot_counter(self.boot_counter)
    }

    /// Makes [`Host::legacy_boot_guid`] answer `guid` from now on, reboots included (`None`: it
    /// cannot be read).
    pub fn set_legacy_guid(&mut self, guid: Option<BootId>) {
        self.legacy_guid = Some(guid);
    }

    /// Makes [`Host::boot_time_hint`] answer `boot_time` (`BootTime - BootTimeBias`) from now on;
    /// a reboot does not change it. So a [`FakeHost::reboot`] alone, with a boot time set, is a
    /// counter that moved without a new boot (what the counter's safety net catches); a test of a
    /// real restart sets another boot time after it.
    pub fn set_boot_time(&mut self, boot_time: Option<u64>) {
        self.boot_time = boot_time;
    }

    /// The process this host runs as.
    pub fn process(&self) -> ProcessIdentity {
        self.process
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

    /// How many times the recovery files were written.
    pub fn recovery_asset_writes(&self) -> usize {
        self.asset_writes
    }
}

impl Host for FakeHost {
    type Lock = FakeLockGuard;

    fn acquire_lock(&mut self, _timeout: Duration) -> Result<Self::Lock, HostError> {
        if self.lock.0.get() {
            return Err(HostError::Busy);
        }
        self.lock.0.set(true);
        Ok(FakeLockGuard(Rc::clone(&self.lock.0)))
    }

    fn now(&self) -> Timestamp {
        self.clock.now()
    }

    fn monotonic(&self) -> Duration {
        self.clock.monotonic()
    }

    /// Deterministic: the process ID, a counter and the process creation time, so that two
    /// processes of one test never collide.
    fn new_op_id(&mut self) -> Result<OpId, HostError> {
        let n = self.next_op;
        self.next_op += 1;
        let text = format!(
            "{:08x}-{:04x}-4000-8000-{:012x}",
            self.process.pid,
            n & 0xffff,
            self.process.creation_time & 0xffff_ffff_ffff
        );
        OpId::parse(&text).map_err(|_| HostError::Os {
            what: "new operation ID".into(),
            code: 1,
        })
    }

    fn boot_id(&self) -> Result<BootId, HostError> {
        Ok(self.current_boot())
    }

    fn boot_time_hint(&self) -> Option<u64> {
        self.boot_time
    }

    fn legacy_boot_guid(&self) -> Option<BootId> {
        self.legacy_guid.unwrap_or_else(|| {
            Some(BootId(
                FAKE_FIRST_LEGACY_GUID.wrapping_add(u128::from(self.boot_counter.wrapping_sub(1))),
            ))
        })
    }

    fn current_process(&self) -> Result<ProcessIdentity, HostError> {
        Ok(self.process)
    }

    fn liveness(&self, process: &ProcessIdentity) -> Liveness {
        if *process == self.process || self.alive.contains(process) {
            Liveness::Alive
        } else {
            Liveness::Dead
        }
    }

    fn system32_file_exists(&self, file_name: &str) -> Result<bool, HostError> {
        Ok(!self
            .missing_system32
            .contains(&file_name.to_ascii_lowercase()))
    }

    fn write_recovery_assets(&mut self, assets: &RecoveryAssets) -> Result<(), HostError> {
        if self.fail_assets {
            return Err(HostError::Os {
                what: r"%ProgramData%\SHIN DATA CENTER\MKLM\Recovery".into(),
                code: 5,
            });
        }
        self.assets = Some(assets.clone());
        self.asset_writes += 1;
        Ok(())
    }

    fn drain_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    fn take_recovery_assets_moved(&mut self) -> bool {
        std::mem::take(&mut self.assets_moved)
    }
}

/// [`EventSink`] that records events and answers from a script (then `NoDecision` forever).
/// With [`ScriptedSink::cancel_after`], the caller "goes away" after that many events:
/// [`EventSink::check_cancelled`] turns true and every later wait is `Disconnected`
/// (design review S4).
///
/// A scripted [`Decision`] whose `op_id` is [`ScriptedSink::ANY_OP`] is answered for the
/// operation of the latest event that names one (the script is written before the operation ID
/// exists); see [`ScriptedSink::keep`] and [`ScriptedSink::revert_now`].
#[derive(Debug, Clone, Default)]
pub struct ScriptedSink {
    pub events: Vec<Event>,
    script: VecDeque<DecisionPoll>,
    cancel_after_events: Option<usize>,
    /// Moves this clock by the given time on every `wait_decision` (a sink that blocks too long).
    clock: Option<(FakeClock, Duration)>,
    /// How many times `wait_decision` was called.
    pub waits: usize,
}

impl ScriptedSink {
    /// Placeholder operation ID of scripted decisions (see the type docs).
    pub const ANY_OP: &'static str = "00000000-0000-0000-0000-000000000000";

    pub fn new(script: impl IntoIterator<Item = DecisionPoll>) -> Self {
        Self {
            events: Vec::new(),
            script: script.into_iter().collect(),
            cancel_after_events: None,
            clock: None,
            waits: 0,
        }
    }

    /// `Keep` for the running operation.
    pub fn keep() -> DecisionPoll {
        DecisionPoll::Decided(Decision::Keep {
            op_id: Self::any_op(),
        })
    }

    /// `RevertNow` for the running operation.
    pub fn revert_now() -> DecisionPoll {
        DecisionPoll::Decided(Decision::RevertNow {
            op_id: Self::any_op(),
        })
    }

    fn any_op() -> OpId {
        // The literal is a well-formed operation ID.
        OpId::parse(Self::ANY_OP).unwrap_or_else(|_| unreachable!("ANY_OP is well-formed"))
    }

    /// The caller disconnects once `n` events were received (`0`: before the first one).
    pub fn cancel_after(mut self, n: usize) -> Self {
        self.cancel_after_events = Some(n);
        self
    }

    /// Every `wait_decision` moves `clock` by `per_wait` before answering.
    pub fn with_clock(mut self, clock: FakeClock, per_wait: Duration) -> Self {
        self.clock = Some((clock, per_wait));
        self
    }

    fn cancelled(&self) -> bool {
        self.cancel_after_events
            .is_some_and(|n| self.events.len() >= n)
    }

    /// The operation named by the latest event that names one.
    fn current_op(&self) -> Option<OpId> {
        self.events.iter().rev().find_map(|event| match event {
            Event::Planned { op_id, .. }
            | Event::StateChanged { op_id, .. }
            | Event::StepWritten { op_id, .. }
            | Event::CountdownStarted { op_id, .. }
            | Event::CountdownTick { op_id, .. }
            | Event::WaitingForReconnect { op_id, .. } => Some(op_id.clone()),
            _ => None,
        })
    }

    /// Events of one kind, for assertions.
    pub fn count(&self, pick: impl Fn(&Event) -> bool) -> usize {
        self.events.iter().filter(|event| pick(event)).count()
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
        self.waits += 1;
        if let Some((clock, per_wait)) = &self.clock {
            clock.advance(*per_wait);
        }
        if self.cancelled() {
            return DecisionPoll::Disconnected;
        }
        let answer = self.script.pop_front().unwrap_or(DecisionPoll::NoDecision);
        let fill = |op_id: OpId, current: Option<OpId>| match current {
            Some(current) if op_id.as_str() == Self::ANY_OP => current,
            _ => op_id,
        };
        match answer {
            DecisionPoll::Decided(Decision::Keep { op_id }) => {
                DecisionPoll::Decided(Decision::Keep {
                    op_id: fill(op_id, self.current_op()),
                })
            }
            DecisionPoll::Decided(Decision::RevertNow { op_id }) => {
                DecisionPoll::Decided(Decision::RevertNow {
                    op_id: fill(op_id, self.current_op()),
                })
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mklm_core::fixtures;

    fn device(id: &str) -> WriteTarget {
        WriteTarget::Device {
            instance_id: id.to_string(),
        }
    }

    fn dword(value: u32) -> RegValue {
        RegValue::Dword { value }
    }

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";

    fn op(n: u32) -> JournalSlot {
        JournalSlot::Op(OpId::parse(&format!("{n:08x}-0000-4000-8000-000000000000")).unwrap())
    }

    #[test]
    fn values_names_and_devnodes() {
        let mut reg = MemoryRegistry::new();
        let kb = device(KEYCHRON);
        reg.seed(&kb, value_names::HID_TYPE, dword(4));
        assert_eq!(reg.read_value(&kb, "keyboardtypeoverride"), Ok(dword(4)));
        assert_eq!(
            reg.read_value(&kb, value_names::HID_SUBTYPE),
            Ok(RegValue::Absent)
        );
        assert_eq!(reg.mutating_calls(), 0, "seeding is not counted");

        reg.write_value(&kb, value_names::HID_TYPE, &dword(7))
            .unwrap();
        reg.write_value(&kb, value_names::HID_SUBTYPE, &RegValue::Absent)
            .unwrap();
        assert_eq!(reg.read_value(&kb, value_names::HID_TYPE), Ok(dword(7)));
        assert_eq!(
            reg.write_value(&kb, "Start", &dword(4)),
            Err(BackendError::NameNotAllowed {
                name: "Start".into()
            })
        );
        assert_eq!(
            reg.write_value(
                &WriteTarget::Global,
                value_names::LAYER_DRIVER_KOR,
                &dword(1)
            ),
            Err(BackendError::NameNotAllowed {
                name: value_names::LAYER_DRIVER_KOR.into()
            })
        );
        assert_eq!(
            reg.list_values(&kb),
            Ok(vec![(value_names::HID_TYPE.to_string(), dword(7))])
        );
        assert_eq!(reg.mutating_calls(), 4, "refused calls count too");

        reg.remove_devnode(KEYCHRON);
        let removed = Err(BackendError::DeviceRemoved {
            instance_id: KEYCHRON.into(),
        });
        assert_eq!(reg.read_value(&kb, value_names::HID_TYPE), removed);
        assert_eq!(
            reg.write_value(&kb, value_names::HID_TYPE, &dword(4)),
            removed.clone().map(|_| ())
        );
        assert_eq!(reg.flush_target(&kb), removed.map(|_| ()));
        // It stays removed after a crash.
        let after = reg.after_crash(CrashImage::ProcessKill);
        assert!(after.contents().devices.is_empty());
        assert!(matches!(
            after.read_value(&kb, value_names::HID_TYPE),
            Err(BackendError::DeviceRemoved { .. })
        ));
    }

    #[test]
    fn crash_after_and_fail_at() {
        let mut reg = MemoryRegistry::new();
        reg.set_faults(FaultPlan {
            crash_after: Some(2),
            ..FaultPlan::default()
        });
        reg.write_journal(&op(1), "{}").unwrap();
        reg.flush_journal().unwrap();
        assert_eq!(reg.write_journal(&op(2), "{}"), Err(BackendError::Crashed));
        assert!(reg.crashed());
        // Everything fails from now on, reads included.
        assert_eq!(reg.flush_journal(), Err(BackendError::Crashed));
        assert_eq!(reg.read_journal(), Err(BackendError::Crashed));
        assert_eq!(reg.mutations().len(), 2);

        let mut reg = MemoryRegistry::new();
        reg.set_faults(FaultPlan {
            crash_after: Some(0),
            ..FaultPlan::default()
        });
        assert_eq!(reg.flush_journal(), Err(BackendError::Crashed));
        assert!(reg.mutations().is_empty());

        let mut reg = MemoryRegistry::new();
        reg.set_faults(FaultPlan {
            fail_at: Some(2),
            ..FaultPlan::default()
        });
        reg.write_journal(&op(1), "{}").unwrap();
        assert_eq!(
            reg.write_journal(&op(2), "{}"),
            Err(BackendError::Injected { call: 2 })
        );
        reg.write_journal(&op(3), "{}").unwrap();
        assert_eq!(reg.read_journal().unwrap().ops.len(), 2);
        assert_eq!(
            reg.read_journal().unwrap().store_version,
            Some(STORE_VERSION)
        );
    }

    #[test]
    fn deny_target_survives_a_crash() {
        let mut reg = MemoryRegistry::new();
        let kb = device(KEYCHRON);
        reg.set_faults(FaultPlan {
            deny_target: Some(kb.clone()),
            crash_after: Some(5),
            ..FaultPlan::default()
        });
        assert!(matches!(
            reg.write_value(&kb, value_names::HID_TYPE, &dword(7)),
            Err(BackendError::AccessDenied { .. })
        ));
        reg.write_value(&WriteTarget::Global, value_names::PS2_TYPE, &dword(7))
            .unwrap();
        let mut after = reg.after_crash(CrashImage::ProcessKill);
        assert!(matches!(
            after.write_value(&kb, value_names::HID_TYPE, &dword(7)),
            Err(BackendError::AccessDenied { .. })
        ));
        // Only the deny fault is inherited: no crash_after any more.
        for _ in 0..10 {
            after.flush_journal().unwrap();
        }
    }

    #[test]
    fn read_faults() {
        let reg = MemoryRegistry::new();
        let kb = device(KEYCHRON);
        reg.set_faults(FaultPlan {
            fail_read_at: Some(2),
            ..FaultPlan::default()
        });
        assert_eq!(
            reg.read_value(&kb, value_names::HID_TYPE),
            Ok(RegValue::Absent)
        );
        assert!(matches!(
            reg.list_values(&WriteTarget::Global),
            Err(BackendError::Os { .. })
        ));
        assert_eq!(
            reg.read_value(&kb, value_names::HID_TYPE),
            Ok(RegValue::Absent)
        );
        assert_eq!(reg.reads(), 3);
        assert_eq!(reg.mutating_calls(), 0);

        // A denied target that cannot be read either; the fault survives a crash.
        reg.set_faults(FaultPlan {
            deny_target: Some(kb.clone()),
            deny_reads: true,
            ..FaultPlan::default()
        });
        assert!(matches!(
            reg.read_value(&kb, value_names::HID_TYPE),
            Err(BackendError::AccessDenied { .. })
        ));
        assert_eq!(
            reg.read_value(&WriteTarget::Global, value_names::PS2_TYPE),
            Ok(RegValue::Absent)
        );
        let after = reg.after_crash(CrashImage::ProcessKill);
        assert!(matches!(
            after.list_values(&kb),
            Err(BackendError::AccessDenied { .. })
        ));
    }

    /// The crash images are exactly the per-hive prefixes that keep every flushed call.
    #[test]
    fn crash_images_enumerate_flushed_prefixes() {
        let mut reg = MemoryRegistry::new();
        let kb = device(KEYCHRON);
        reg.seed(&kb, value_names::HID_TYPE, dword(4));
        // SOFTWARE: J1 FJ | J2 ; SYSTEM: T1 FT T2 T3
        reg.write_journal(&op(1), "a").unwrap();
        reg.flush_journal().unwrap();
        reg.write_value(&kb, value_names::HID_TYPE, &dword(7))
            .unwrap();
        reg.flush_target(&kb).unwrap();
        reg.write_journal(&op(2), "b").unwrap();
        reg.write_value(&kb, value_names::HID_SUBTYPE, &dword(2))
            .unwrap();
        reg.write_value(&kb, value_names::HID_TYPE, &dword(4))
            .unwrap();

        let images = reg.crash_images();
        // SYSTEM prefixes 2..=4 (3 choices), SOFTWARE 2..=3 (2 choices), minus the full/full one,
        // plus ProcessKill.
        assert_eq!(images.len(), 3 * 2);
        assert_eq!(images[0], CrashImage::ProcessKill);
        let unique: BTreeSet<String> = images.iter().map(|i| format!("{i:?}")).collect();
        assert_eq!(unique.len(), images.len());
        assert!(!images.contains(&CrashImage::PowerLoss {
            system_calls: 4,
            software_calls: 3
        }));
        assert!(images.contains(&CrashImage::PowerLoss {
            system_calls: 2,
            software_calls: 2
        }));

        let minimal = reg.after_crash(CrashImage::PowerLoss {
            system_calls: 2,
            software_calls: 2,
        });
        assert_eq!(minimal.read_value(&kb, value_names::HID_TYPE), Ok(dword(7)));
        assert_eq!(
            minimal.read_value(&kb, value_names::HID_SUBTYPE),
            Ok(RegValue::Absent)
        );
        assert_eq!(minimal.read_journal().unwrap().ops.len(), 1);
        assert_eq!(minimal.mutating_calls(), 0);
        assert!(minimal.mutations().is_empty());

        let partial = reg.after_crash(CrashImage::PowerLoss {
            system_calls: 3,
            software_calls: 3,
        });
        assert_eq!(
            partial.read_value(&kb, value_names::HID_SUBTYPE),
            Ok(dword(2))
        );
        assert_eq!(partial.read_value(&kb, value_names::HID_TYPE), Ok(dword(7)));
        assert_eq!(partial.read_journal().unwrap().ops.len(), 2);

        let all = reg.after_crash(CrashImage::ProcessKill);
        assert_eq!(all.contents(), reg.contents());

        // Every image differs in content.
        let contents: BTreeSet<String> = images
            .iter()
            .map(|i| format!("{:?}", reg.after_crash(*i).contents()))
            .collect();
        assert_eq!(contents.len(), images.len());
    }

    #[test]
    fn crash_images_without_writes_and_after_final_flushes() {
        let reg = MemoryRegistry::new();
        assert_eq!(reg.crash_images(), vec![CrashImage::ProcessKill]);

        let mut reg = MemoryRegistry::new();
        reg.write_journal(&op(1), "a").unwrap();
        reg.flush_journal().unwrap();
        reg.write_value(&WriteTarget::Global, value_names::PS2_TYPE, &dword(7))
            .unwrap();
        reg.flush_target(&WriteTarget::Global).unwrap();
        assert_eq!(reg.crash_images(), vec![CrashImage::ProcessKill]);
    }

    #[test]
    fn outside_edits_are_durable_and_flush_the_system_hive() {
        let mut reg = MemoryRegistry::new();
        let kb = device(KEYCHRON);
        reg.write_value(&kb, value_names::HID_TYPE, &dword(7))
            .unwrap();
        reg.outside_edit(&kb, value_names::HID_SUBTYPE, dword(5));
        reg.write_value(&kb, value_names::HID_TYPE, &dword(4))
            .unwrap();
        assert_eq!(reg.mutating_calls(), 2);
        assert_eq!(reg.mutations().len(), 2);
        let images = reg.crash_images();
        // SYSTEM: T, outside | T → the outside edit and the write before it always survive.
        assert_eq!(images.len(), 2);
        for image in images {
            let after = reg.after_crash(image);
            assert_eq!(
                after.read_value(&kb, value_names::HID_SUBTYPE),
                Ok(dword(5))
            );
            assert_ne!(
                after.read_value(&kb, value_names::HID_TYPE),
                Ok(RegValue::Absent)
            );
        }
    }

    #[test]
    fn fake_devices_follow_the_registry() {
        let reg = MemoryRegistry::new();
        let kb = device(KEYCHRON);
        reg.seed(&kb, value_names::HID_TYPE, dword(4));
        reg.seed(&kb, value_names::HID_SUBTYPE, dword(0));
        let mut devices = FakeDevices::new(reg.clone(), vec![fixtures::keychron()]);
        assert_eq!(devices.reported_type(KEYCHRON), Ok(Some(KeyboardType::US)));

        let mut writer = reg.clone();
        writer
            .write_value(&kb, value_names::HID_TYPE, &dword(7))
            .unwrap();
        writer
            .write_value(&kb, value_names::HID_SUBTYPE, &dword(2))
            .unwrap();
        // Values alone change nothing.
        assert_eq!(devices.reported_type(KEYCHRON), Ok(Some(KeyboardType::US)));
        assert_eq!(devices.restart(KEYCHRON), Ok(RestartOutcome::Restarted));
        assert_eq!(
            devices.wait_for_arrival(KEYCHRON, Duration::from_secs(15)),
            Ok(Arrival::Started {
                reported: Some(KeyboardType::JIS)
            })
        );
        assert_eq!(devices.restarted(), [KEYCHRON.to_string()]);

        devices.push_reset(KEYCHRON, FakeReset::NeverArrives);
        devices.set_reset(KEYCHRON, FakeReset::Hangs);
        devices.restart(KEYCHRON).unwrap();
        assert_eq!(
            devices.wait_for_arrival(KEYCHRON, Duration::from_secs(15)),
            Ok(Arrival::TimedOut)
        );
        assert_eq!(devices.reported_type(KEYCHRON), Ok(None));
        assert_eq!(devices.restart(KEYCHRON), Ok(RestartOutcome::TimedOut));
        devices.reconnect(KEYCHRON);
        assert_eq!(devices.reported_type(KEYCHRON), Ok(Some(KeyboardType::JIS)));

        devices.set_reset(KEYCHRON, FakeReset::ArrivesUnchanged);
        writer
            .write_value(&kb, value_names::HID_TYPE, &dword(4))
            .unwrap();
        writer
            .write_value(&kb, value_names::HID_SUBTYPE, &dword(0))
            .unwrap();
        devices.restart(KEYCHRON).unwrap();
        assert_eq!(
            devices.wait_for_arrival(KEYCHRON, Duration::from_secs(15)),
            Ok(Arrival::Started {
                reported: Some(KeyboardType::JIS)
            })
        );
        devices.set_reset(KEYCHRON, FakeReset::Fails);
        assert_eq!(devices.restart(KEYCHRON), Err(DeviceError::Injected));
        devices.set_reset(KEYCHRON, FakeReset::NeedsReboot);
        assert_eq!(devices.restart(KEYCHRON), Ok(RestartOutcome::NeedsReboot));
        devices.reboot();
        assert_eq!(devices.reported_type(KEYCHRON), Ok(Some(KeyboardType::US)));

        let inventory = devices.keyboards().unwrap();
        assert_eq!(
            inventory.keyboards[0].overrides.hid_type(),
            Some(KeyboardType::US)
        );
        devices.set_warnings(vec!["names".into()]);
        assert_eq!(devices.keyboards().unwrap().warnings, ["names".to_string()]);
        devices.set_incomplete(true);
        assert!(matches!(
            devices.keyboards(),
            Err(DeviceError::Incomplete { .. })
        ));
        devices.set_incomplete(false);
        devices.remove(KEYCHRON);
        assert!(devices.keyboards().unwrap().keyboards.is_empty());
        assert!(matches!(
            reg.read_value(&kb, value_names::HID_TYPE),
            Err(BackendError::DeviceRemoved { .. })
        ));
    }

    #[test]
    fn ps2_keyboards_follow_the_global_values_at_reboot() {
        let reg = MemoryRegistry::new();
        reg.seed(&WriteTarget::Global, value_names::PS2_TYPE, dword(7));
        reg.seed(&WriteTarget::Global, value_names::PS2_SUBTYPE, dword(2));
        let mut ps2 = fixtures::internal_ps2();
        ps2.overrides = DeviceOverrides::default();
        let id = ps2.instance_id.clone();
        let mut devices = FakeDevices::new(reg.clone(), vec![ps2]);
        devices.reboot();
        assert_eq!(devices.reported_type(&id), Ok(Some(KeyboardType::JIS)));
        let mut writer = reg.clone();
        writer
            .write_value(
                &WriteTarget::Global,
                value_names::PS2_TYPE,
                &RegValue::Absent,
            )
            .unwrap();
        writer
            .write_value(
                &WriteTarget::Global,
                value_names::PS2_SUBTYPE,
                &RegValue::Absent,
            )
            .unwrap();
        assert_eq!(devices.reported_type(&id), Ok(Some(KeyboardType::JIS)));
        devices.reboot();
        assert_eq!(devices.reported_type(&id), Ok(Some(KeyboardType::US)));
    }

    #[test]
    fn fake_host_lock_clock_processes_and_ids() {
        let cell = FakeLockCell::default();
        let mut a = FakeHost::new(cell.clone());
        let mut b = FakeHost::new(cell.clone());
        let guard = a.acquire_lock(Duration::from_secs(10)).unwrap();
        assert!(cell.is_locked());
        assert_eq!(
            b.acquire_lock(Duration::from_secs(10)).err(),
            Some(HostError::Busy)
        );
        drop(guard);
        assert!(!cell.is_locked());
        assert!(b.acquire_lock(Duration::from_secs(10)).is_ok());

        let clock = a.clock();
        let start = a.monotonic();
        let wall = a.now();
        clock.advance(Duration::from_secs(25));
        assert_eq!(a.monotonic() - start, Duration::from_secs(25));
        assert_eq!(a.now().0 - wall.0, 25_000);

        let first = a.new_op_id().unwrap();
        let second = a.new_op_id().unwrap();
        assert_ne!(first, second);
        let old = a.process();
        let mut c = a.after_crash(ProcessIdentity {
            pid: 2000,
            creation_time: 7,
        });
        assert_eq!(c.liveness(&old), Liveness::Dead);
        assert_ne!(c.new_op_id().unwrap(), first);
        let boot = c.current_boot();
        c.become_process(ProcessIdentity {
            pid: 3000,
            creation_time: 8,
        });
        assert_eq!(
            c.liveness(&ProcessIdentity {
                pid: 2000,
                creation_time: 7
            }),
            Liveness::Alive
        );
        assert_eq!(boot, BootId::from_boot_counter(1));
        assert_eq!(c.boot_id(), Ok(boot));
        assert_eq!(c.boot_time_hint(), None);
        let first_guid = BootId(0x9b1c_0d6e_2f4a_4c8b_a1d3_5e6f_0000_0001);
        assert_eq!(c.legacy_boot_guid(), Some(first_guid));
        c.reboot();
        assert_ne!(c.current_boot(), boot);
        assert_eq!(c.current_boot(), BootId::from_boot_counter(2));
        assert_eq!(c.legacy_boot_guid(), Some(BootId(first_guid.0 + 1)));
        assert_eq!(
            c.liveness(&ProcessIdentity {
                pid: 3000,
                creation_time: 8
            }),
            Liveness::Dead
        );
        assert_eq!(c.liveness(&c.process()), Liveness::Alive);
        // Set values stay across a reboot.
        c.set_legacy_guid(Some(first_guid));
        c.set_boot_time(Some(134_351_230_275_000_000));
        c.reboot();
        assert_eq!(c.current_boot(), BootId::from_boot_counter(3));
        assert_eq!(c.legacy_boot_guid(), Some(first_guid));
        assert_eq!(c.boot_time_hint(), Some(134_351_230_275_000_000));
        c.set_legacy_guid(None);
        assert_eq!(c.legacy_boot_guid(), None);

        c.remove_system32_file("KBD101.DLL");
        assert_eq!(c.system32_file_exists("kbd101.dll"), Ok(false));
        assert_eq!(c.system32_file_exists("kbd106.dll"), Ok(true));
        c.push_warning("quarantined");
        assert_eq!(c.drain_warnings(), ["quarantined".to_string()]);
        assert!(c.drain_warnings().is_empty());
    }

    #[test]
    fn scripted_sink_fills_in_the_operation_and_disconnects() {
        let op_id = OpId::parse("0000000a-0000-4000-8000-000000000000").unwrap();
        let mut sink = ScriptedSink::new([ScriptedSink::keep(), DecisionPoll::NoDecision]);
        sink.event(&Event::Locked);
        sink.event(&Event::CountdownTick {
            op_id: op_id.clone(),
            remaining: 20,
        });
        assert_eq!(
            sink.wait_decision(Duration::from_secs(1)),
            DecisionPoll::Decided(Decision::Keep {
                op_id: op_id.clone()
            })
        );
        assert_eq!(
            sink.wait_decision(Duration::from_secs(1)),
            DecisionPoll::NoDecision
        );
        assert_eq!(
            sink.wait_decision(Duration::from_secs(1)),
            DecisionPoll::NoDecision
        );

        let mut sink = ScriptedSink::default().cancel_after(1);
        assert!(!sink.check_cancelled());
        sink.event(&Event::Locked);
        assert!(sink.check_cancelled());
        assert_eq!(
            sink.wait_decision(Duration::from_secs(1)),
            DecisionPoll::Disconnected
        );

        let host = FakeHost::new(FakeLockCell::default());
        let mut sink = ScriptedSink::default().with_clock(host.clock(), Duration::from_secs(30));
        sink.wait_decision(Duration::from_secs(1));
        assert_eq!(host.monotonic(), Duration::from_secs(30));
        assert_eq!(sink.waits, 1);
    }
}
