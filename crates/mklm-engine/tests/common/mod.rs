//! Shared set-up of the engine tests: the development machine of M0 on the in-memory fakes.

#![allow(dead_code)]

use std::collections::BTreeMap;

use mklm_core::{
    ApplyOptions, ConflictPolicy, DeviceOverrides, GlobalSettings, Journal, JournalEntry,
    KeyboardDevice, KeyboardDriver, Layout, LayoutChoice, OpId, OpState, OperationResult,
    ProcessIdentity, RegValue, RestoreScope, WriteTarget, fixtures, value_names,
};
use mklm_engine::memory::{
    CrashImage, FakeDevices, FakeHost, FakeLockCell, MemoryRegistry, RegistryContents, ScriptedSink,
};
use mklm_engine::{
    CleanupParams, Engine, EngineConfig, EngineError, JournalSlot, MigrateParams, RegistryBackend,
    RestoreBaselineParams, RestoreMode, SetLayoutParams,
};

pub type TestEngine = Engine<MemoryRegistry, FakeDevices, FakeHost>;

pub const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
pub const PS2: &str = r"ACPI\FUJ0309\4&320DB4C2&0";
pub const VXE: &str = r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F&COL02\B&B4852A&0&0001";
/// A phantom PS/2 keyboard of a dock (not present, no pin).
pub const DOCK_PS2: &str = r"ACPI\PNP0303\4&1D401FB5&0";

/// The caller declared another way to type and allows a live reset.
pub const LIVE: ApplyOptions = ApplyOptions {
    allow_live_reset: true,
    other_input_available: true,
    countdown_seconds: mklm_core::DEFAULT_COUNTDOWN_SECONDS,
};
/// No reset (the defaults).
pub const NO_RESET: ApplyOptions = ApplyOptions {
    allow_live_reset: false,
    other_input_available: false,
    countdown_seconds: mklm_core::DEFAULT_COUNTDOWN_SECONDS,
};

pub fn device(instance_id: &str) -> WriteTarget {
    WriteTarget::Device {
        instance_id: instance_id.to_string(),
    }
}

pub fn dword(value: u32) -> RegValue {
    RegValue::Dword { value }
}

pub fn sz(value: &str) -> RegValue {
    RegValue::Sz {
        value: value.to_string(),
    }
}

/// A phantom PS/2 keyboard without values.
pub fn dock_ps2() -> KeyboardDevice {
    let mut kb = fixtures::internal_ps2();
    kb.instance_id = DOCK_PS2.to_string();
    kb.display_name = "標準 PS/2 キーボード".to_string();
    kb.hardware_ids = vec![r"ACPI\PNP0303".to_string(), "*PNP0303".to_string()];
    kb.present = false;
    kb.overrides = DeviceOverrides::default();
    kb.reported_type = None;
    kb.dev_node_status = None;
    kb.problem_code = None;
    kb
}

/// Seeds `registry` with the values of `keyboards` and `global`.
pub fn seed(registry: &MemoryRegistry, keyboards: &[KeyboardDevice], global: &GlobalSettings) {
    for kb in keyboards {
        let target = device(&kb.instance_id);
        let o = &kb.overrides;
        for (name, value) in [
            (value_names::HID_TYPE, o.keyboard_type_override),
            (value_names::HID_SUBTYPE, o.keyboard_subtype_override),
            (value_names::PS2_TYPE, o.override_keyboard_type),
            (value_names::PS2_SUBTYPE, o.override_keyboard_subtype),
        ] {
            if let Some(value) = value {
                registry.seed(&target, name, dword(value));
            }
        }
    }
    let g = WriteTarget::Global;
    for (name, value) in [
        (value_names::LAYER_DRIVER_JPN, &global.layer_driver_jpn),
        (value_names::LAYER_DRIVER_KOR, &global.layer_driver_kor),
        (
            value_names::KEYBOARD_IDENTIFIER,
            &global.override_keyboard_identifier,
        ),
    ] {
        if let Some(value) = value {
            registry.seed(&g, name, sz(value));
        }
    }
    for (name, value) in [
        (value_names::PS2_TYPE, global.override_keyboard_type),
        (value_names::PS2_SUBTYPE, global.override_keyboard_subtype),
    ] {
        if let Some(value) = value {
            registry.seed(&g, name, dword(value));
        }
    }
}

/// A machine on the fakes. Every engine run goes through [`World::run`], which takes the parts
/// back afterwards.
#[derive(Debug, Clone)]
pub struct World {
    pub registry: MemoryRegistry,
    pub devices: FakeDevices,
    pub host: FakeHost,
    pub lock: FakeLockCell,
    pub config: EngineConfig,
    next_pid: u32,
}

impl World {
    pub fn new(keyboards: Vec<KeyboardDevice>, global: GlobalSettings) -> World {
        let registry = MemoryRegistry::new();
        seed(&registry, &keyboards, &global);
        let mut devices = FakeDevices::new(registry.clone(), keyboards);
        devices.reboot();
        let lock = FakeLockCell::default();
        World {
            registry,
            devices,
            host: FakeHost::new(lock.clone()),
            lock,
            config: EngineConfig::default(),
            next_pid: 5000,
        }
    }

    /// The development machine after M0: per-keyboard mode, the PS/2 keyboard pinned to JIS,
    /// the Keychron at US.
    pub fn dev_machine() -> World {
        let snapshot = fixtures::dev_machine();
        World::new(snapshot.keyboards, snapshot.global)
    }

    /// The development machine before M0: fixed JIS, no PS/2 pin, the Keychron at US.
    pub fn pre_m0() -> World {
        let mut snapshot = fixtures::dev_machine();
        snapshot.keyboards[0].overrides = DeviceOverrides::default();
        World::new(snapshot.keyboards, fixtures::global_fixed_jis())
    }

    pub fn engine(&self) -> TestEngine {
        Engine::new(
            self.registry.clone(),
            self.devices.clone(),
            self.host.clone(),
            self.config.clone(),
        )
    }

    /// Runs `f` on an engine over this world and keeps the parts' new state.
    pub fn run<T>(&mut self, f: impl FnOnce(&mut TestEngine) -> T) -> T {
        let mut engine = self.engine();
        let result = f(&mut engine);
        let (registry, devices, host) = engine.into_parts();
        self.registry = registry;
        self.devices = devices;
        self.host = host;
        result
    }

    /// An independent copy whose registry holds everything written so far as durable state.
    pub fn fork(&self) -> World {
        let registry = self.registry.checkpoint();
        World {
            devices: self.devices.with_registry(registry.clone()),
            registry,
            host: self.host.clone(),
            lock: self.lock.clone(),
            config: self.config.clone(),
            next_pid: self.next_pid,
        }
    }

    /// The machine after the process died in `image`; with `reboot`, after a PC restart too.
    pub fn after_crash(&mut self, image: CrashImage, reboot: bool) -> World {
        let registry = self.registry.after_crash(image);
        self.next_pid += 1;
        let mut host = self.host.after_crash(ProcessIdentity {
            pid: self.next_pid,
            creation_time: u64::from(self.next_pid),
        });
        let mut devices = self.devices.with_registry(registry.clone());
        if reboot {
            host.reboot();
            devices.reboot();
        }
        World {
            registry,
            devices,
            host,
            lock: self.lock.clone(),
            config: self.config.clone(),
            next_pid: self.next_pid,
        }
    }

    /// A PC restart with nothing else happening.
    pub fn reboot(&mut self) {
        self.host.reboot();
        self.devices.reboot();
    }

    pub fn journal(&self) -> Journal {
        journal_of(&self.registry)
    }

    pub fn entry(&self, op_id: &OpId) -> JournalEntry {
        self.journal()
            .entry(op_id)
            .cloned()
            .unwrap_or_else(|| panic!("no entry {op_id}"))
    }

    pub fn value(&self, target: &WriteTarget, name: &str) -> RegValue {
        self.registry.value(target, name)
    }

    /// The HID pair of a keyboard (`KeyboardTypeOverride`, `KeyboardSubtypeOverride`).
    pub fn hid(&self, instance_id: &str) -> (RegValue, RegValue) {
        let target = device(instance_id);
        (
            self.value(&target, value_names::HID_TYPE),
            self.value(&target, value_names::HID_SUBTYPE),
        )
    }

    /// The i8042prt pair of a device or of the global key.
    pub fn ps2(&self, target: &WriteTarget) -> (RegValue, RegValue) {
        (
            self.value(target, value_names::PS2_TYPE),
            self.value(target, value_names::PS2_SUBTYPE),
        )
    }

    // Requests with default sinks.

    pub fn set(
        &mut self,
        instance_id: &str,
        layout: LayoutChoice,
        apply: ApplyOptions,
        sink: &mut ScriptedSink,
    ) -> Result<OperationResult, EngineError> {
        let params = set_params(instance_id, layout, apply);
        self.run(|e| e.set_layout(&params, sink))
    }

    pub fn migrate(
        &mut self,
        standard: Layout,
        assignments: &[(&str, LayoutChoice)],
    ) -> Result<OperationResult, EngineError> {
        let params = migrate_params(standard, assignments);
        self.run(|e| e.migrate(&params, &mut ScriptedSink::default()))
    }

    pub fn recover(&mut self) -> Result<OperationResult, EngineError> {
        self.run(|e| e.recover(&ApplyOptions::default(), &mut ScriptedSink::default()))
    }

    pub fn undo(&mut self) -> Result<OperationResult, EngineError> {
        self.run(|e| e.undo_open(&ApplyOptions::default(), &mut ScriptedSink::default()))
    }

    pub fn confirm(&mut self, op_id: &OpId) -> Result<OperationResult, EngineError> {
        self.run(|e| e.confirm(op_id, &mut ScriptedSink::default()))
    }

    pub fn revert(
        &mut self,
        op_id: &OpId,
        apply: ApplyOptions,
    ) -> Result<OperationResult, EngineError> {
        self.run(|e| e.revert(op_id, &apply, &mut ScriptedSink::default()))
    }

    pub fn restore_all(&mut self, mode: RestoreMode) -> Result<OperationResult, EngineError> {
        let params = restore_params(RestoreScope::All, ConflictPolicy::Report, mode);
        self.run(|e| e.restore_baseline(&params, &mut ScriptedSink::default()))
    }

    /// "削除する" of `names` on `instance_id` (design m3 A.5).
    pub fn cleanup(
        &mut self,
        instance_id: &str,
        names: &[&str],
    ) -> Result<OperationResult, EngineError> {
        let params = CleanupParams {
            instance_id: instance_id.to_string(),
            names: names.iter().map(|n| n.to_string()).collect(),
        };
        self.run(|e| e.cleanup_values(&params, &mut ScriptedSink::default()))
    }

    /// Puts journal documents into the store as they are (e.g. `fixtures::schema_1_journal`),
    /// durable, and restarts the call count.
    pub fn seed_journal(&mut self, ops: &[(String, String)], baselines: &[(String, String)]) {
        for (name, json) in ops {
            let op = OpId::parse(name).unwrap_or_else(|error| panic!("{name}: {error}"));
            self.registry
                .write_journal(&JournalSlot::Op(op), json)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
        for (name, json) in baselines {
            self.registry
                .write_journal(&JournalSlot::Baseline(name.clone()), json)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
        self.registry
            .flush_journal()
            .unwrap_or_else(|error| panic!("flush: {error}"));
        *self = self.fork();
    }

    /// Runs `set`, `migrate`… and fails the test on an error.
    pub fn ok(result: Result<OperationResult, EngineError>) -> OperationResult {
        result.unwrap_or_else(|error| panic!("request failed: {error:?}"))
    }
}

pub fn journal_of(registry: &MemoryRegistry) -> Journal {
    let dump = registry
        .read_journal()
        .unwrap_or_else(|error| panic!("journal: {error}"));
    Journal::parse(&dump.ops, &dump.baselines)
}

pub fn set_params(instance_id: &str, layout: LayoutChoice, apply: ApplyOptions) -> SetLayoutParams {
    SetLayoutParams {
        instance_id: instance_id.to_string(),
        layout,
        apply,
        expected: None,
    }
}

pub fn migrate_params(standard: Layout, assignments: &[(&str, LayoutChoice)]) -> MigrateParams {
    MigrateParams {
        standard,
        assignments: assignments
            .iter()
            .map(|(id, choice)| (id.to_string(), *choice))
            .collect(),
        expected: None,
    }
}

pub fn restore_params(
    scope: RestoreScope,
    on_conflict: ConflictPolicy,
    mode: RestoreMode,
) -> RestoreBaselineParams {
    RestoreBaselineParams {
        scope,
        on_conflict,
        mode,
        apply: ApplyOptions::default(),
    }
}

/// Only the keyboard and global values (not the journal), for comparisons.
pub fn values_of(contents: &RegistryContents) -> RegistryContents {
    RegistryContents {
        // A key without values compares like a missing key.
        devices: contents
            .devices
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(id, values)| (id.clone(), values.clone()))
            .collect(),
        global: contents.global.clone(),
        ..RegistryContents::default()
    }
}

/// Every (target, name) MKLM may write that exists in `a` or `b`, with both values.
pub fn value_diff(
    a: &RegistryContents,
    b: &RegistryContents,
) -> Vec<(WriteTarget, String, RegValue, RegValue)> {
    let mut keys: BTreeMap<String, (WriteTarget, String)> = BTreeMap::new();
    for contents in [a, b] {
        for (id, values) in &contents.devices {
            for name in values.keys() {
                keys.insert(
                    format!("device|{id}|{}", name.to_ascii_lowercase()),
                    (device(id), name.clone()),
                );
            }
        }
        for name in contents.global.keys() {
            keys.insert(
                format!("global|{}", name.to_ascii_lowercase()),
                (WriteTarget::Global, name.clone()),
            );
        }
    }
    keys.into_values()
        .filter_map(|(target, name)| {
            let (x, y) = (a.value(&target, &name), b.value(&target, &name));
            (x != y).then_some((target, name, x, y))
        })
        .collect()
}

/// The keyboards of the world as the fake lists them (values from the registry).
pub fn keyboards_of(world: &World) -> Vec<KeyboardDevice> {
    world.devices.listed()
}

pub fn is_ps2(kb: &KeyboardDevice) -> bool {
    kb.driver == KeyboardDriver::I8042prt
}

/// Every entry in `state`.
pub fn entries_in(journal: &Journal, state: OpState) -> Vec<&JournalEntry> {
    journal
        .entries
        .iter()
        .filter(|e| e.state == state)
        .collect()
}
