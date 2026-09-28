//! The engine: one public method per request (section D of the design doc).
//!
//! Every mutating method:
//! 1. takes the write lock ([`Host::acquire_lock`]) and holds it until it returns;
//! 2. reads and parses the journal, refusing to write while any entry is unreadable, needs
//!    recovery (every in-flight entry does, since this process holds the lock), or (for `set` and
//!    `migrate`) is open; clears `apply_pending` fields that no longer apply (housekeeping);
//! 3. re-enumerates the keyboards ([`DeviceController::keyboards`]) and re-reads every value
//!    through the [`RegistryBackend`];
//! 4. validates with `mklm_core` (`plan_set_layout` / `plan_migration` → `check_plan` for new
//!    values: allowlist + INV-PS2 + order; `plan_restore` for restores) and follows the write/flush
//!    order of section C.5;
//! 5. never returns with an entry in flight unless the backend itself failed like a crash
//!    (`BackendError::Crashed`, or the journal could not be written): a non-crash write failure
//!    during a revert, rollback or resolution moves the entry to `Conflict` with the error
//!    recorded (design review C3).
//!
//! Write/flush order (C.5; J = journal write, FJ = journal flush, T = target write, FT = target
//! flush): recovery files → baselines + J(`Planned`) → FJ → per step T… → FT → J(`Written`) → FJ.
//! Every later transition is one J → FJ, and its `StateChanged` event follows the FJ. Restores
//! (revert, rollback, resolution, undo, restore to baseline) persist `RevertPending` (or `Planned`)
//! first and then write the steps of `plan_restore` in phase order, stopping after a step that
//! could not be written completely.
//!
//! The engine takes its own parameter types ([`crate::params`]) and reports with
//! `mklm_core::report`; it does not know the pipe protocol (design review S5).

use std::time::Duration;

use mklm_core::{
    ApplyOptions, ApplyPending, BASELINE_SCHEMA_VERSION, BaselineRecord, BootId, ConflictInfo,
    ConflictPolicy, ContextValue, Countdown, DN_STARTED, Decision, DeviceOverrides, Event, Expect,
    ExpectedKeyboard, ExpectedPlan, FailureReason, GlobalSettings, InputMethods, InvPs2Violation,
    Journal, JournalEntry, JournalError, KeyboardDevice, KeyboardDriver, KeyboardType,
    LayoutChoice, MAX_HISTORY, Observation, OpId, OpKind, OpState, OperationError, OperationPlan,
    OperationResult, OsInfo, Outcome, PendingAction, PlanError, PlanStep, PlannedWrite,
    ProcessIdentity, RESTORE_ON_UNINSTALL_VALUE, RecoveredOp, RecoveryContext, RecoveryDecision,
    RegValue, ResolutionChoice, RestoreError, RestorePlan, RestoreScope, RestoreTo, RevertMode,
    STORE_VERSION, STORE_VERSION_VALUE, SkipReason, SystemSnapshot, Timestamp, TransitionRecord,
    Transport, UnreadableEntry, ValueKey, ValueOp, ValueRecord, WriteTarget, apply_method,
    apply_pending_cleared, apply_pending_on_close, assess, check_cleanup, check_inv_ps2,
    check_restore_record, decide_recovery_with_removed, is_unread_value, live_reset_bans, observe,
    physical_device_members, plan_migration, plan_restore, plan_set_layout, render_recovery_assets,
    state_after_resolution, structural_reset_bans, value_eq, value_names,
};

use crate::backend::{BackendError, JournalSlot, RegistryBackend};
use crate::device::{Arrival, DeviceController, RestartOutcome};
use crate::error::EngineError;
use crate::host::{Host, HostError};
use crate::params::{
    CleanupParams, MachineSettingsParams, MigrateParams, ResolveParams, RestoreBaselineParams,
    RestoreMode, SetLayoutParams,
};
use crate::sink::{DecisionPoll, EventSink};

/// Tunables. The defaults are the product values; tests shorten nothing because the engine counts
/// countdown ticks and the fake host's monotonic clock only moves when the test moves it.
///
/// The countdown's length is not here: each request carries it (`ApplyOptions::countdown_seconds`,
/// 20 or 60 s; design m3 WP-E3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// Extra monotonic time a countdown may take beyond its seconds before it is treated as
    /// expired whatever the sink does (design review C18).
    pub countdown_slack: Duration,
    /// Deadline of one `DeviceController::restart` call (design review C18).
    pub restart_timeout: Duration,
    /// How long a reset keyboard may take to come back (plan 1.4: then revert and ask for a restart).
    pub arrival_timeout: Duration,
    /// Reconnect path: how long to wait for the keyboard to come back before returning
    /// `AwaitingConfirm` to the caller.
    pub reconnect_wait: Duration,
    /// Reconnect path: how long to wait for keep/revert after it came back.
    pub decision_wait: Duration,
    /// How long to wait for the write lock.
    pub lock_timeout: Duration,
    /// Closed operations kept when pruning (section C.9).
    pub keep_closed_ops: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            countdown_slack: Duration::from_secs(5),
            restart_timeout: Duration::from_secs(20),
            arrival_timeout: Duration::from_secs(15),
            reconnect_wait: Duration::from_secs(180),
            decision_wait: Duration::from_secs(600),
            lock_timeout: Duration::from_secs(10),
            keep_closed_ops: 32,
        }
    }
}

/// Where the recovery files live, as shown in [`Event::RecoveryAssetsWritten`].
const RECOVERY_DIRECTORY: &str = r"%ProgramData%\SHIN DATA CENTER\MKLM\Recovery";
/// Version written into the recovery files.
const GENERATOR_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Interval of [`Event::WaitingForReconnect`] (design D.2 b).
const RECONNECT_TICK: Duration = Duration::from_secs(10);
/// A sink that keeps answering with decisions for other operations cannot hold a wait forever.
const MAX_STRAY_DECISIONS: usize = 100;
/// Warning of every "follow the standard" change (design 0.1: not verified by typing in M0).
const STANDARD_UNVERIFIED: &str =
    "\"follow the standard\" (0x51) has not been verified by typing yet (M0); check the layout";

/// What the pre-flight checks of D.1 step 3 allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// `set`, `migrate`: no open entry at all.
    NewOp,
    /// `restore --baseline`: open entries are superseded, in-flight ones need recovery.
    Restore,
    /// `revert`, `confirm`, `resolve`: in-flight entries need recovery first.
    Existing,
    /// `recover`, `undo`: they recover in-flight entries themselves.
    Recover,
}

/// How the forward writes of a new operation ended.
enum Forward {
    Done,
    /// The first compare-and-swap failed: nothing was written.
    NothingWritten {
        name: String,
    },
    /// A write, flush or compare-and-swap failed after something was written.
    Failed {
        message: String,
    },
}

/// Records of an entry whose current value could not be read (index, error), so recovery cannot
/// judge the entry.
#[derive(Debug)]
struct Unobservable {
    records: Vec<(usize, BackendError)>,
}

/// Keyboards that came back after a reset, whether all did, and what each arrived one reports.
type ResetOutcome = (Vec<String>, bool, Vec<(String, Option<KeyboardType>)>);

/// How a keep-or-revert countdown ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CountdownEnd {
    Keep,
    RevertNow,
    Disconnected,
    Expired,
}

/// Per-request state: the journal as this request sees it, the re-read inventory and values, and
/// what the result will report.
struct Session<'s> {
    sink: &'s mut dyn EventSink,
    journal: Journal,
    /// Every Keyboard-class devnode, with values re-read through the backend.
    keyboards: Vec<KeyboardDevice>,
    global: GlobalSettings,
    boot: BootId,
    process: ProcessIdentity,
    warnings: Vec<String>,
    /// INV-PS2 violation that stopped an entry at `Conflict` (reported in the result).
    inv_ps2: Option<InvPs2Violation>,
}

impl Session<'_> {
    fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.sink.event(&Event::Warning {
            message: message.clone(),
        });
        self.warnings.push(message);
    }

    /// Replaces (or adds) `entry` in the request's view of the journal.
    fn put(&mut self, entry: &JournalEntry) {
        match self
            .journal
            .entries
            .iter_mut()
            .find(|e| e.op_id == entry.op_id)
        {
            Some(slot) => *slot = entry.clone(),
            None => {
                self.journal.entries.push(entry.clone());
                self.journal
                    .entries
                    .sort_by(|a, b| (a.seq, &a.op_id).cmp(&(b.seq, &b.op_id)));
            }
        }
    }

    fn keyboard(&self, instance_id: &str) -> Option<&KeyboardDevice> {
        self.keyboards
            .iter()
            .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
    }

    /// The type the stored values give the keyboard at its next start.
    fn expected_type(&self, instance_id: &str) -> Option<KeyboardType> {
        self.keyboard(instance_id)
            .and_then(|kb| kb.predicted_type(&self.global))
    }

    /// Raw Input (as enumerated) reports the type the stored values predict.
    fn reports_stored_type(&self, instance_id: &str) -> bool {
        self.keyboard(instance_id).is_some_and(|kb| {
            kb.present
                && kb.reported_type.is_some()
                && kb.reported_type == kb.predicted_type(&self.global)
        })
    }
}

fn internal(error: impl std::fmt::Display) -> EngineError {
    EngineError::Internal(error.to_string())
}

/// Errors worth one more attempt (C.5 step 3): anything but a crash or a refusal that cannot
/// change.
fn retryable(error: &BackendError) -> bool {
    matches!(
        error,
        BackendError::Injected { .. } | BackendError::AccessDenied { .. } | BackendError::Os { .. }
    )
}

/// An entry that recovery must handle before anything else happens: in flight, or counting down
/// (its owner would hold the lock; C.7).
fn needs_recovery(entry: &JournalEntry) -> bool {
    entry.state.is_in_flight()
        || (entry.state == OpState::AwaitingConfirm && entry.countdown.is_some())
}

/// True when the entry's `RevertPending` was entered from `Conflict`: an `undo` of a conflict
/// (D.10; a resolution also comes from there, but carries `RevertMode::Resolution`). Read from
/// the persisted history, so that recovery continues the undo the way it was started (I4).
fn reverts_a_conflict(entry: &JournalEntry) -> bool {
    entry
        .history
        .iter()
        .rev()
        .find(|line| line.to == OpState::RevertPending && line.from != Some(OpState::RevertPending))
        .is_some_and(|line| line.from == Some(OpState::Conflict))
}

fn is_restore(entry: &JournalEntry) -> bool {
    matches!(entry.kind, OpKind::RestoreBaseline { .. })
}

fn is_cleanup(entry: &JournalEntry) -> bool {
    matches!(entry.kind, OpKind::Cleanup { .. })
}

/// Design m3 WP-E3: a countdown of 20 or 60 s only; checked before anything else happens.
fn check_countdown(apply: &ApplyOptions) -> Result<(), EngineError> {
    if apply.countdown_allowed() {
        Ok(())
    } else {
        Err(EngineError::CountdownNotAllowed {
            seconds: apply.countdown_seconds,
        })
    }
}

fn device_target(instance_id: &str) -> WriteTarget {
    WriteTarget::Device {
        instance_id: instance_id.to_string(),
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

fn outcome_of(state: OpState) -> Outcome {
    match state {
        OpState::Confirmed => Outcome::Confirmed,
        OpState::PendingReboot => Outcome::PendingReboot,
        OpState::Reverted => Outcome::Reverted,
        OpState::RevertedPendingReboot => Outcome::RevertedPendingReboot,
        OpState::Failed => Outcome::Failed,
        OpState::Conflict => Outcome::Conflict,
        OpState::AwaitingConfirm
        | OpState::Planned
        | OpState::Written
        | OpState::Restarting
        | OpState::RevertPending => Outcome::AwaitingConfirm,
    }
}

fn empty_result(op_id: Option<OpId>, outcome: Outcome, warnings: &[String]) -> OperationResult {
    OperationResult {
        op_id,
        outcome,
        failure: None,
        pending_action: None,
        conflicts: Vec::new(),
        inv_ps2_violation: None,
        recovered: Vec::new(),
        warnings: warnings.to_vec(),
    }
}

/// What a revert (or rollback) expects to find: what the operation last wrote, else `intended`.
fn expect_written(record: &ValueRecord) -> Expect {
    Expect::Value {
        value: record
            .last_written
            .clone()
            .unwrap_or_else(|| record.intended.clone()),
    }
}

/// What a resolution expects to find: the value the user saw (C5), else what was last written.
fn expect_seen(record: &ValueRecord) -> Expect {
    Expect::Value {
        value: record
            .conflict
            .clone()
            .or_else(|| record.last_written.clone())
            .unwrap_or_else(|| record.intended.clone()),
    }
}

fn expect_before(record: &ValueRecord) -> Expect {
    Expect::Value {
        value: record.before.clone(),
    }
}

fn expect_matches(expect: &Expect, name: &str, current: &RegValue) -> bool {
    match expect {
        Expect::Any => true,
        Expect::Value { value } => value_eq(name, current, value),
    }
}

/// Groups record indices by registry key, in the order the keys first appear.
fn group_steps(records: &[ValueRecord]) -> Vec<(WriteTarget, Vec<usize>)> {
    let mut steps: Vec<(WriteTarget, Vec<usize>)> = Vec::new();
    for (index, record) in records.iter().enumerate() {
        match steps.last_mut() {
            Some((target, indices)) if *target == record.target => indices.push(index),
            _ => steps.push((record.target.clone(), vec![index])),
        }
    }
    steps
}

/// The records as plan steps (for [`Event::Planned`]); `Other` values have no planned form.
fn plan_steps(records: &[ValueRecord]) -> Vec<PlanStep> {
    let mut steps: Vec<PlanStep> = Vec::new();
    for record in records.iter().filter(|r| r.skipped.is_none()) {
        let write = match &record.intended {
            RegValue::Dword { value } => PlannedWrite::set(&record.name, *value),
            RegValue::Sz { value } => PlannedWrite::set_string(&record.name, value),
            RegValue::Absent => PlannedWrite::delete(&record.name),
            RegValue::Other { .. } => continue,
        };
        match steps.last_mut() {
            Some(step) if step.target == record.target => step.writes.push(write),
            _ => steps.push(PlanStep {
                target: record.target.clone(),
                writes: vec![write],
            }),
        }
    }
    steps
}

/// Applies one value to the model, as the drivers would read it.
fn apply_to_model(
    keyboards: &mut [KeyboardDevice],
    global: &mut GlobalSettings,
    target: &WriteTarget,
    name: &str,
    value: &RegValue,
) {
    match target {
        WriteTarget::Device { instance_id } => {
            let Some(kb) = keyboards
                .iter_mut()
                .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
            else {
                return;
            };
            let o = &mut kb.overrides;
            let slot = if name.eq_ignore_ascii_case(value_names::HID_TYPE) {
                &mut o.keyboard_type_override
            } else if name.eq_ignore_ascii_case(value_names::HID_SUBTYPE) {
                &mut o.keyboard_subtype_override
            } else if name.eq_ignore_ascii_case(value_names::PS2_TYPE) {
                &mut o.override_keyboard_type
            } else if name.eq_ignore_ascii_case(value_names::PS2_SUBTYPE) {
                &mut o.override_keyboard_subtype
            } else {
                return;
            };
            *slot = as_dword(value);
        }
        WriteTarget::Global => {
            if name.eq_ignore_ascii_case(value_names::PS2_TYPE) {
                global.override_keyboard_type = as_dword(value);
            } else if name.eq_ignore_ascii_case(value_names::PS2_SUBTYPE) {
                global.override_keyboard_subtype = as_dword(value);
            } else if name.eq_ignore_ascii_case(value_names::LAYER_DRIVER_JPN) {
                global.layer_driver_jpn = as_string(value);
            } else if name.eq_ignore_ascii_case(value_names::KEYBOARD_IDENTIFIER) {
                global.override_keyboard_identifier = as_string(value);
            }
        }
    }
}

fn snapshot(keyboards: Vec<KeyboardDevice>, global: GlobalSettings) -> SystemSnapshot {
    SystemSnapshot {
        keyboards,
        global,
        input: InputMethods::default(),
        os: OsInfo {
            build: 0,
            ubr: None,
            native_arch: String::new(),
            remote_session: false,
            client_keyboard_type: None,
        },
    }
}

/// True when some keyboard outside `targets` (and their external containers) is connected,
/// started and not virtual (the engine's copy of `plan_set_layout`'s rule, for restores).
fn other_keyboard_usable(keyboards: &[KeyboardDevice], targets: &[&KeyboardDevice]) -> bool {
    let containers: Vec<&str> = targets
        .iter()
        .filter(|kb| !kb.in_internal_container())
        .filter_map(|kb| kb.known_container_id())
        .collect();
    keyboards.iter().any(|kb| {
        let is_target = targets
            .iter()
            .any(|t| t.instance_id.eq_ignore_ascii_case(&kb.instance_id));
        let same_container = !kb.in_internal_container()
            && kb
                .known_container_id()
                .is_some_and(|c| containers.iter().any(|t| t.eq_ignore_ascii_case(c)));
        !is_target
            && !same_container
            && kb.present
            && kb.dev_node_status.is_some_and(|s| s & DN_STARTED != 0)
            && kb.transport != Transport::Virtual
    })
}

/// True when the operation's values may have reached a driver: it went through a reset, a
/// confirmation (after a reconnect or a reboot) or was kept, or a boot passed since it was written.
fn values_in_effect(entry: &JournalEntry, boot: BootId) -> bool {
    entry.history.iter().any(|h| {
        matches!(
            h.to,
            OpState::Restarting | OpState::AwaitingConfirm | OpState::Confirmed
        )
    }) || (entry.boot_id != boot && entry.history.iter().any(|h| h.to == OpState::Written))
}

fn check_expected(
    expected: Option<&ExpectedPlan>,
    plan: &OperationPlan,
) -> Result<(), EngineError> {
    match expected {
        Some(expected)
            if expected.steps != plan.checked.steps || expected.apply != Some(plan.apply) =>
        {
            Err(EngineError::PlanChanged {
                plan: ExpectedPlan {
                    steps: plan.checked.steps.clone(),
                    apply: Some(plan.apply),
                },
            })
        }
        _ => Ok(()),
    }
}

/// Both lists of keyboards, with the heavier action.
fn merge_pending(a: Option<ApplyPending>, b: Option<ApplyPending>) -> Option<ApplyPending> {
    match (a, b) {
        (None, other) | (other, None) => other,
        (Some(mut a), Some(b)) => {
            a.action = a.action.max(b.action);
            for id in b.instance_ids {
                if !a.instance_ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                    a.instance_ids.push(id);
                }
            }
            a.since = b.since;
            Some(a)
        }
    }
}

/// `plan_restore` accepts an end state that breaks INV-PS2 when it is the baseline of every
/// i8042prt and global value it writes. A boot-time value skipped because of a conflict is not
/// at its baseline, so that exception does not hold: refuse the plan.
fn check_skipped_boot_time(
    plan: &RestorePlan,
    records: &[ValueRecord],
) -> Result<(), RestoreError> {
    match &plan.restores_inv_ps2_violation {
        Some(violation)
            if records
                .iter()
                .any(|r| r.skipped == Some(SkipReason::ConflictSkipped) && r.is_boot_time()) =>
        {
            Err(RestoreError::InvPs2(violation.clone()))
        }
        _ => Ok(()),
    }
}

fn decision_op(decision: &Decision) -> &OpId {
    match decision {
        Decision::Keep { op_id } | Decision::RevertNow { op_id } => op_id,
    }
}

/// Journaled write transactions over the three system traits.
#[derive(Debug)]
pub struct Engine<R, D, H> {
    registry: R,
    devices: D,
    host: H,
    config: EngineConfig,
}

impl<R, D, H> Engine<R, D, H>
where
    R: RegistryBackend,
    D: DeviceController,
    H: Host,
{
    pub fn new(registry: R, devices: D, host: H, config: EngineConfig) -> Self {
        Self {
            registry,
            devices,
            host,
            config,
        }
    }

    /// Gives the parts back (tests inspect the fakes after a run).
    pub fn into_parts(self) -> (R, D, H) {
        (self.registry, self.devices, self.host)
    }

    /// Section D.2: set one keyboard's layout; live reset + countdown, reconnect or PC restart.
    pub fn set_layout(
        &mut self,
        params: &SetLayoutParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        check_countdown(&params.apply)?;
        let lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::NewOp)?;
        let plan = plan_set_layout(
            &s.keyboards,
            &s.global,
            &params.instance_id,
            params.layout,
            &params.apply,
        )?;
        check_expected(params.expected.as_ref(), &plan)?;
        let records = self.build_records(&s, &plan.checked.steps)?;
        if params.layout == LayoutChoice::Standard {
            s.warn(STANDARD_UNVERIFIED);
        }
        if records.is_empty() {
            return Ok(empty_result(None, Outcome::NoChange, &s.warnings));
        }
        let requested = s
            .keyboard(&params.instance_id)
            .map_or_else(|| params.instance_id.clone(), |kb| kb.instance_id.clone());
        let keyboards = self.expected_keyboards(
            &s,
            &plan.checked.keyboards,
            &plan.checked.global,
            Some(&plan.instance_ids),
        );
        let kind = OpKind::SetLayout {
            requested,
            instance_ids: plan.instance_ids.clone(),
            layout: params.layout,
        };
        let mut entry = self.create(
            &mut s,
            kind,
            records,
            Some(plan.apply),
            Vec::new(),
            keyboards,
        )?;
        if let Some(result) = self.write_new(&mut s, &mut entry)? {
            return Ok(result);
        }
        self.apply_change(&mut s, &mut entry, lock, params.apply)
    }

    /// Section D.3: fixed → per-keyboard mode (plan 1.3); always ends in `PendingReboot`.
    pub fn migrate(
        &mut self,
        params: &MigrateParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        let lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::NewOp)?;
        let plan = plan_migration(
            &s.keyboards,
            &s.global,
            params.standard,
            &params.assignments,
        )?;
        // Plan 1.5 / S8: the layer driver MKLM writes must exist.
        for step in plan
            .checked
            .steps
            .iter()
            .filter(|step| step.target == WriteTarget::Global)
        {
            for write in &step.writes {
                if let (true, ValueOp::SetString(dll)) = (
                    write
                        .name
                        .eq_ignore_ascii_case(value_names::LAYER_DRIVER_JPN),
                    &write.op,
                ) && !self.host.system32_file_exists(dll)?
                {
                    return Err(OperationError::LayerDriverMissing { dll: dll.clone() }.into());
                }
            }
        }
        check_expected(params.expected.as_ref(), &plan)?;
        let context = self.snapshot_context(&mut s)?;
        let records = self.build_records(&s, &plan.checked.steps)?;
        if records.is_empty() {
            return Ok(empty_result(None, Outcome::NoChange, &s.warnings));
        }
        let keyboards =
            self.expected_keyboards(&s, &plan.checked.keyboards, &plan.checked.global, None);
        let kind = OpKind::Migrate {
            standard: params.standard,
            assignments: params.assignments.clone(),
        };
        let mut entry = self.create(
            &mut s,
            kind,
            records,
            Some(PendingAction::RestartPc),
            context,
            keyboards,
        )?;
        if let Some(result) = self.write_new(&mut s, &mut entry)? {
            return Ok(result);
        }
        self.apply_change(&mut s, &mut entry, lock, ApplyOptions::default())
    }

    /// Section D.4: restore `before` of the latest operation on its values (compare-and-swap).
    /// Refuses a `Confirmed` restore-to-baseline (design review C6).
    pub fn revert(
        &mut self,
        op_id: &OpId,
        apply: &ApplyOptions,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        check_countdown(apply)?;
        let _lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::Existing)?;
        let mut entry = Self::find_entry(&s, op_id)?;
        match entry.state {
            OpState::AwaitingConfirm | OpState::PendingReboot => {}
            OpState::Confirmed if is_restore(&entry) => {
                return Err(EngineError::InvalidState {
                    op_id: op_id.clone(),
                    state: entry.state,
                    action: "revert a confirmed restore to baseline (set the layout again instead)",
                });
            }
            OpState::Confirmed => {}
            state => {
                return Err(EngineError::InvalidState {
                    op_id: op_id.clone(),
                    state,
                    action: "revert",
                });
            }
        }
        Self::check_latest(&s, &entry)?;
        let result = self.revert_entry(&mut s, &mut entry, apply, None, "revert")?;
        self.prune(&mut s)?;
        Ok(Self::with_warnings(result, &s))
    }

    /// Section D.6: keep an operation in `AwaitingConfirm` (or `PendingReboot` seen from a new
    /// boot). Checks Raw Input; keeps with `apply_pending` when a keyboard does not report its
    /// stored type yet (design review C1).
    pub fn confirm(
        &mut self,
        op_id: &OpId,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        let _lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::Existing)?;
        let mut entry = Self::find_entry(&s, op_id)?;
        match entry.state {
            OpState::AwaitingConfirm => {}
            OpState::PendingReboot if entry.boot_id != s.boot => {
                // The recovery transition first (INV-PS2 and the values included).
                self.refresh(&mut s)?;
                // Not in flight: an unreadable value only fails this request.
                let current = match self.observe_values(&entry)? {
                    Ok(current) => current,
                    Err(Unobservable { records }) => {
                        return Err(records
                            .into_iter()
                            .next()
                            .map_or_else(|| internal("no unreadable value"), |(_, e)| e.into()));
                    }
                };
                let context = RecoveryContext {
                    current_boot: s.boot,
                    inv_ps2: check_inv_ps2(&s.global, &s.keyboards).err(),
                };
                let decision = decide_recovery_with_removed(&entry, &current, &context);
                self.execute(&mut s, &mut entry, decision, &current)?;
                if entry.state != OpState::AwaitingConfirm {
                    self.prune(&mut s)?;
                    return Ok(self.entry_result(&s, &entry));
                }
            }
            OpState::PendingReboot => {
                return Err(EngineError::InvalidState {
                    op_id: op_id.clone(),
                    state: entry.state,
                    action: "confirm before the PC restarts (restart, not shut down)",
                });
            }
            state => {
                return Err(EngineError::InvalidState {
                    op_id: op_id.clone(),
                    state,
                    action: "confirm",
                });
            }
        }

        // Every value must still be what the operation wrote.
        let mut changed = false;
        for index in 0..entry.records.len() {
            let record = &entry.records[index];
            if record.skipped.is_some() {
                continue;
            }
            match self.read_current(&record.target, &record.name)? {
                None => entry.records[index].skipped = Some(SkipReason::DeviceRemoved),
                Some(current) if value_eq(&record.name, &current, &record.intended) => {}
                Some(current) => {
                    entry.records[index].conflict = Some(current);
                    changed = true;
                }
            }
        }
        if changed {
            self.transition(&mut s, &mut entry, OpState::Conflict, "confirm:changed")?;
            return Ok(self.entry_result(&s, &entry));
        }

        // Raw Input: which keyboards already run with the stored values (C1).
        self.refresh(&mut s)?;
        let mut reapplied = Vec::new();
        for id in Self::hid_keyboards(&s, &entry) {
            let Some(kb) = s.keyboard(&id).filter(|kb| kb.present) else {
                continue;
            };
            let expected = kb.predicted_type(&s.global);
            let reported = self.devices.reported_type(&id).ok().flatten();
            if reported.is_some() && reported == expected {
                reapplied.push(id);
            } else {
                s.warn(format!(
                    "{id}: Raw Input does not report the stored type yet; reconnect the keyboard"
                ));
            }
        }
        self.close_confirmed(&mut s, &mut entry, &reapplied, "keep")?;
        self.prune(&mut s)?;
        Ok(self.entry_result(&s, &entry))
    }

    /// Section D.5: "MKLM 導入前に戻す" as a new operation that writes the baselines. Closes open,
    /// not in-flight entries as `Failed(Superseded)` instead of refusing (design review C7).
    pub fn restore_baseline(
        &mut self,
        params: &RestoreBaselineParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        check_countdown(&params.apply)?;
        let lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::Restore)?;
        let silent = params.mode == RestoreMode::Silent;
        let policy = if silent {
            ConflictPolicy::Skip
        } else {
            params.on_conflict
        };
        let baselines: Vec<BaselineRecord> = match &params.scope {
            RestoreScope::All => s.journal.baselines.clone(),
            RestoreScope::Device { instance_id } => {
                let members: Vec<String> = physical_device_members(&s.keyboards, instance_id)?
                    .iter()
                    .map(|kb| kb.instance_id.clone())
                    .collect();
                s.journal
                    .baselines
                    .iter()
                    .filter(|b| match &b.key.target {
                        WriteTarget::Device { instance_id } => {
                            members.iter().any(|m| m.eq_ignore_ascii_case(instance_id))
                        }
                        WriteTarget::Global => false,
                    })
                    .cloned()
                    .collect()
            }
        };

        // D.5 step 3: current value, baseline and expectation of every value in scope.
        let mut records: Vec<ValueRecord> = Vec::new();
        let mut conflicts: Vec<ConflictInfo> = Vec::new();
        for baseline in &baselines {
            let key = &baseline.key;
            let current = self.read_current(&key.target, &key.name)?;
            let latest = s.journal.latest_record(key);
            let expect = latest
                .and_then(|(_, r)| r.last_written.clone())
                .unwrap_or_else(|| baseline.value.clone());
            let mut record = ValueRecord {
                target: key.target.clone(),
                key_path: baseline.key_path.clone(),
                name: key.name.clone(),
                baseline: baseline.value.clone(),
                before: current.clone().unwrap_or_else(|| baseline.value.clone()),
                intended: baseline.value.clone(),
                last_written: None,
                conflict: None,
                resolve_to: None,
                write_error: None,
                skipped: None,
            };
            match current {
                None => {
                    s.warn(format!(
                        "{}: the device was removed; its value is left alone",
                        baseline.key_path
                    ));
                    record.skipped = Some(SkipReason::DeviceRemoved);
                    records.push(record);
                }
                Some(current) if value_eq(&key.name, &current, &baseline.value) => {}
                Some(current) if value_eq(&key.name, &current, &expect) => records.push(record),
                Some(current) => match policy {
                    ConflictPolicy::Report => {
                        let (op_id, index, last) = match latest {
                            Some((entry, _)) => {
                                let index = entry
                                    .records
                                    .iter()
                                    .position(|r| {
                                        r.key().canonical().eq_ignore_ascii_case(&key.canonical())
                                    })
                                    .unwrap_or(0);
                                (
                                    entry.op_id.clone(),
                                    index,
                                    entry.records.get(index).cloned(),
                                )
                            }
                            None => (baseline.captured_by.clone(), 0, None),
                        };
                        conflicts.push(ConflictInfo {
                            op_id,
                            record: index,
                            key_path: baseline.key_path.clone(),
                            name: key.name.clone(),
                            baseline: baseline.value.clone(),
                            before: last
                                .as_ref()
                                .map_or_else(|| baseline.value.clone(), |r| r.before.clone()),
                            intended: last
                                .as_ref()
                                .map_or_else(|| baseline.value.clone(), |r| r.intended.clone()),
                            last_written: last.and_then(|r| r.last_written),
                            current,
                            write_error: None,
                        });
                    }
                    ConflictPolicy::Skip => {
                        s.warn(format!(
                            "{}\\{}: changed outside MKLM; left alone",
                            baseline.key_path, key.name
                        ));
                        record.skipped = Some(SkipReason::ConflictSkipped);
                        record.conflict = Some(current);
                        records.push(record);
                    }
                    ConflictPolicy::Overwrite => records.push(record),
                },
            }
        }
        if !conflicts.is_empty() {
            let mut result = empty_result(None, Outcome::Conflict, &s.warnings);
            result.conflicts = conflicts;
            return Ok(result);
        }
        if records.iter().all(|r| r.skipped.is_some()) {
            return Ok(empty_result(None, Outcome::NoChange, &s.warnings));
        }
        for record in records.iter().filter(|r| r.skipped.is_none()) {
            check_restore_record(record, &s.keyboards, RestoreTo::Baseline)?;
        }

        // Order the records as they will be written (phase order), skipped ones last.
        let writes: Vec<usize> = (0..records.len())
            .filter(|&i| records[i].skipped.is_none())
            .collect();
        let plan = self.plan_subset(&s, &records, &writes, RestoreTo::Baseline, &expect_before)?;
        let mut ordered: Vec<ValueRecord> = plan
            .steps
            .iter()
            .flat_map(|step| step.writes.iter())
            .map(|w| records[writes[w.record]].clone())
            .collect();
        let count = ordered.len();
        ordered.extend(records.iter().filter(|r| r.skipped.is_some()).cloned());
        let records = ordered;
        let writes: Vec<usize> = (0..count).collect();
        let plan = self.plan_subset(&s, &records, &writes, RestoreTo::Baseline, &expect_before)?;
        check_skipped_boot_time(&plan, &records)?;
        if let Some(violation) = &plan.restores_inv_ps2_violation {
            s.warn(format!(
                "the values before MKLM already broke INV-PS2 and are restored as found: {violation}"
            ));
        }

        // D.5 step 5: open, not in-flight entries on the same values are superseded (C7).
        let keys: Vec<String> = records[..count]
            .iter()
            .map(|r| r.key().canonical().to_ascii_uppercase())
            .collect();
        let supersedes: Vec<OpId> = s
            .journal
            .entries
            .iter()
            .filter(|e| e.state.is_open() && !needs_recovery(e))
            .filter(|e| {
                e.records
                    .iter()
                    .any(|r| keys.contains(&r.key().canonical().to_ascii_uppercase()))
            })
            .map(|e| e.op_id.clone())
            .collect();

        // Values the keyboard's driver does not read (what a cleanup deleted, design m3 A.5)
        // take no effect anywhere: they decide neither a restart nor a reset.
        let read: Vec<ValueRecord> = records[..count]
            .iter()
            .filter(|r| !is_unread_value(&s.keyboards, &r.target, &r.name))
            .cloned()
            .collect();
        let boot_time = read.iter().any(ValueRecord::is_boot_time);
        let apply = if read.is_empty() {
            None
        } else if boot_time {
            Some(PendingAction::RestartPc)
        } else {
            let ids = Self::hid_ids(&read);
            let targets: Vec<&KeyboardDevice> =
                ids.iter().filter_map(|id| s.keyboard(id)).collect();
            let only_usable = !params.apply.other_input_available
                || !other_keyboard_usable(&s.keyboards, &targets);
            Some(apply_method(
                &targets,
                false,
                only_usable,
                params.apply.allow_live_reset,
            ))
        };
        let (mut after_keyboards, mut after_global) = (s.keyboards.clone(), s.global.clone());
        for record in &records[..count] {
            apply_to_model(
                &mut after_keyboards,
                &mut after_global,
                &record.target,
                &record.name,
                &record.intended,
            );
        }
        let device_ids: Vec<String> = records[..count]
            .iter()
            .filter_map(|r| match &r.target {
                WriteTarget::Device { instance_id } => Some(instance_id.clone()),
                WriteTarget::Global => None,
            })
            .collect();
        let only = if boot_time {
            None
        } else {
            Some(device_ids.as_slice())
        };
        let keyboards = self.expected_keyboards(&s, &after_keyboards, &after_global, only);
        let kind = OpKind::RestoreBaseline {
            scope: params.scope.clone(),
            silent,
            supersedes: supersedes.clone(),
        };
        let mut entry = self.create(&mut s, kind, records, apply, Vec::new(), keyboards)?;
        self.supersede(&mut s, &entry.op_id.clone(), &supersedes)?;
        if !self.run_restore(&mut s, &mut entry, &plan, &writes, silent)? {
            self.transition(&mut s, &mut entry, OpState::Conflict, "restore:conflict")?;
            return Ok(self.entry_result(&s, &entry));
        }
        self.transition(&mut s, &mut entry, OpState::Written, "written")?;
        if silent {
            self.close_confirmed(&mut s, &mut entry, &[], "silent")?;
            self.prune(&mut s)?;
            let mut result = self.entry_result(&s, &entry);
            if boot_time {
                result.pending_action = Some(PendingAction::RestartPc);
            }
            return Ok(result);
        }
        self.apply_change(&mut s, &mut entry, lock, params.apply)
    }

    /// Section D.7: act on every eligible entry (`mklm_core::decide_recovery`); with `apply`
    /// allowing it, reset keyboards whose recovered values are not in effect.
    pub fn recover(
        &mut self,
        apply: &ApplyOptions,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        check_countdown(apply)?;
        let _lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::Recover)?;
        let mut result = empty_result(None, Outcome::Recovered, &[]);
        self.recover_all(&mut s, &mut result)?;
        self.prune(&mut s)?;
        self.reset_pending(&mut s, apply)?;
        Ok(self.finish_recovered(&s, result))
    }

    /// Section D.10: undo every open, not in-flight entry, newest first (design review C7).
    /// Recovers in-flight entries first, like `recover`.
    pub fn undo_open(
        &mut self,
        apply: &ApplyOptions,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        check_countdown(apply)?;
        let _lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::Recover)?;
        let mut result = empty_result(None, Outcome::Recovered, &[]);
        self.recover_all(&mut s, &mut result)?;
        let mut targets: Vec<JournalEntry> = s
            .journal
            .entries
            .iter()
            .filter(|e| e.state.is_open() && !needs_recovery(e))
            .cloned()
            .collect();
        targets.sort_by(|a, b| (b.seq, &b.op_id).cmp(&(a.seq, &a.op_id)));
        for mut entry in targets {
            let from = entry.state;
            match entry.state {
                OpState::AwaitingConfirm | OpState::PendingReboot => {
                    if let Err(error) = Self::check_latest(&s, &entry) {
                        s.warn(format!("{}: not undone: {error}", entry.op_id));
                        continue;
                    }
                    match self.revert_entry(&mut s, &mut entry, apply, None, "undo") {
                        Ok(_) => {}
                        Err(EngineError::Restore(error)) => {
                            s.warn(format!("{}: not undone: {error}", entry.op_id));
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
                OpState::Conflict => self.undo_conflict(&mut s, &mut entry, apply)?,
                _ => continue,
            }
            result.recovered.push(RecoveredOp {
                op_id: entry.op_id.clone(),
                from,
                to: entry.state,
                decision: "undo".to_string(),
            });
        }
        self.prune(&mut s)?;
        Ok(self.finish_recovered(&s, result))
    }

    /// Section D.8: apply the user's per-value choices to an operation in `Conflict`.
    pub fn resolve_conflict(
        &mut self,
        params: &ResolveParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        check_countdown(&params.apply)?;
        let _lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::Existing)?;
        let mut entry = Self::find_entry(&s, &params.op_id)?;
        if entry.state != OpState::Conflict {
            return Err(EngineError::InvalidState {
                op_id: params.op_id.clone(),
                state: entry.state,
                action: "resolve a conflict",
            });
        }
        if let Some(bad) = params
            .choices
            .iter()
            .find(|c| c.record >= entry.records.len())
        {
            return Err(RestoreError::NoResolution { record: bad.record }.into());
        }
        let in_effect = values_in_effect(&entry, s.boot);

        // The value each record ends at (`None`: its devnode is gone).
        let mut current: Vec<Option<RegValue>> = Vec::new();
        for record in &entry.records {
            current.push(if record.skipped == Some(SkipReason::DeviceRemoved) {
                None
            } else {
                self.read_current(&record.target, &record.name)?
            });
        }
        let mut targets: Vec<Option<RegValue>> = Vec::new();
        for (index, record) in entry.records.iter().enumerate() {
            let choice = params
                .choices
                .iter()
                .rev()
                .find(|c| c.record == index)
                .map_or(ResolutionChoice::KeepCurrent, |c| c.choice);
            targets.push(current[index].as_ref().map(|now| match choice {
                ResolutionChoice::KeepCurrent => now.clone(),
                ResolutionChoice::UseBefore => record.before.clone(),
                ResolutionChoice::UseIntended => record.intended.clone(),
                ResolutionChoice::UseBaseline => record.baseline.clone(),
            }));
        }

        // C12: never close with INV-PS2 broken.
        self.refresh(&mut s)?;
        let (mut keyboards, mut global) = (s.keyboards.clone(), s.global.clone());
        for (record, target) in entry.records.iter().zip(&targets) {
            if let Some(value) = target {
                apply_to_model(
                    &mut keyboards,
                    &mut global,
                    &record.target,
                    &record.name,
                    value,
                );
            }
        }
        if let Err(violation) = check_inv_ps2(&global, &keyboards) {
            return Err(RestoreError::InvPs2(violation).into());
        }

        for (index, record) in entry.records.iter_mut().enumerate() {
            if current[index].is_none() {
                record.skipped = Some(SkipReason::DeviceRemoved);
            }
        }
        let needs_write: Vec<usize> = (0..entry.records.len())
            .filter(|&i| match (&current[i], &targets[i]) {
                (Some(now), Some(target)) => !value_eq(&entry.records[i].name, now, target),
                _ => false,
            })
            .collect();
        if needs_write.is_empty() {
            self.close_resolution(
                &mut s,
                &mut entry,
                "resolve",
                Some(&params.apply),
                in_effect,
            )?;
        } else {
            for (index, record) in entry.records.iter_mut().enumerate() {
                record.resolve_to = targets[index].clone();
                if record.conflict.is_none() && record.last_written.is_none() {
                    record.conflict = current[index].clone();
                }
            }
            let plan = self.plan_subset(
                &s,
                &entry.records,
                &needs_write,
                RestoreTo::Resolution,
                &expect_seen,
            )?;
            entry.revert_mode = Some(RevertMode::Resolution);
            self.transition(&mut s, &mut entry, OpState::RevertPending, "resolve")?;
            if self.run_restore(&mut s, &mut entry, &plan, &needs_write, false)? {
                self.close_resolution(
                    &mut s,
                    &mut entry,
                    "resolve",
                    Some(&params.apply),
                    in_effect,
                )?;
            } else {
                for record in &mut entry.records {
                    record.resolve_to = None;
                }
                self.transition(&mut s, &mut entry, OpState::Conflict, "resolve:conflict")?;
            }
        }
        self.prune(&mut s)?;
        Ok(self.entry_result(&s, &entry))
    }

    /// "削除する" (design m3 A.5, WP-E1): delete values of one Keyboard-class devnode that its
    /// driver does not read, as a journaled change with the write/flush order of every new
    /// operation (C.5: baselines + `Planned` → FJ, the deletes → FT, `Written` → FJ). Nothing
    /// takes effect, so it then waits in `AwaitingConfirm` without a countdown for the user's
    /// keep (`confirm`) or revert; it is never confirmed automatically.
    ///
    /// Refused like a new `set`: another open entry (`OpInProgress`), a devnode that is not in the
    /// Keyboard-class inventory (`UnknownKeyboard`; a mouse collection never is), a name the driver
    /// reads or any name outside the other stack's type/subtype pair (`PlanRejected`, see
    /// `mklm_core::check_cleanup`), and a machine where INV-PS2 is already broken (`PlanRejected`,
    /// as `check_plan` refuses every plan whose result breaks it, plan 1.5). Values that do not
    /// exist are skipped; when none is left, `NoChange`.
    pub fn cleanup_values(
        &mut self,
        params: &CleanupParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        let lock = self.lock(sink)?;
        let mut s = self.open(sink, Gate::NewOp)?;
        let Some(keyboard) = s.keyboard(&params.instance_id).cloned() else {
            return Err(OperationError::UnknownKeyboard {
                instance_id: params.instance_id.clone(),
            }
            .into());
        };
        let plan_error = |error| {
            EngineError::from(OperationError::Plan(PlanError::Device {
                instance_id: keyboard.instance_id.clone(),
                error,
            }))
        };
        let writes = check_cleanup(&keyboard, &params.names).map_err(plan_error)?;
        if let Err(violation) = check_inv_ps2(&s.global, &s.keyboards) {
            return Err(OperationError::Plan(PlanError::InvPs2(violation)).into());
        }
        let step = PlanStep {
            target: device_target(&keyboard.instance_id),
            writes,
        };
        let records = self.build_records(&s, std::slice::from_ref(&step))?;
        if records.is_empty() {
            return Ok(empty_result(None, Outcome::NoChange, &s.warnings));
        }
        let (mut after_keyboards, mut after_global) = (s.keyboards.clone(), s.global.clone());
        for record in &records {
            apply_to_model(
                &mut after_keyboards,
                &mut after_global,
                &record.target,
                &record.name,
                &record.intended,
            );
        }
        let ids = [keyboard.instance_id.clone()];
        let keyboards =
            self.expected_keyboards(&s, &after_keyboards, &after_global, Some(ids.as_slice()));
        let kind = OpKind::Cleanup {
            instance_id: keyboard.instance_id.clone(),
            names: records.iter().map(|r| r.name.clone()).collect(),
        };
        let mut entry = self.create(&mut s, kind, records, None, Vec::new(), keyboards)?;
        if let Some(result) = self.write_new(&mut s, &mut entry)? {
            return Ok(result);
        }
        self.apply_change(&mut s, &mut entry, lock, ApplyOptions::default())
    }

    /// Saves the machine-wide settings (design m3 B.14, A.5, WP-E2): under the write lock, one
    /// allowlisted `REG_DWORD` written and flushed (`RegistryBackend::write_machine_setting`). Not
    /// journaled (it changes no keyboard); open or unreadable entries do not stop it. Ends
    /// `Confirmed` (the setting is stored) with no operation ID.
    pub fn set_machine_settings(
        &mut self,
        params: &MachineSettingsParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        let _lock = self.lock(sink)?;
        let value = u32::from(params.restore_on_uninstall);
        self.with_retry(|r| r.write_machine_setting(RESTORE_ON_UNINSTALL_VALUE, value))?;
        Ok(empty_result(None, Outcome::Confirmed, &[]))
    }

    /// The parsed journal, without the lock (read-only; what `mklm-cli journal` shows).
    pub fn read_journal(&self) -> Result<Journal, EngineError> {
        let dump = self.registry.read_journal()?;
        let mut journal = Journal::parse(&dump.ops, &dump.baselines);
        if let Some(found) = dump.store_version.filter(|v| *v > STORE_VERSION) {
            journal.unreadable.push(UnreadableEntry {
                name: STORE_VERSION_VALUE.to_string(),
                error: JournalError::NewerSchema {
                    found,
                    supported: STORE_VERSION,
                },
            });
        }
        Ok(journal)
    }

    // ---------------------------------------------------------------------------------------
    // Pre-flight (D.1)
    // ---------------------------------------------------------------------------------------

    fn lock(&mut self, sink: &mut dyn EventSink) -> Result<H::Lock, EngineError> {
        let lock =
            self.host
                .acquire_lock(self.config.lock_timeout)
                .map_err(|error| match error {
                    HostError::Busy => EngineError::Busy,
                    other => EngineError::Host(other),
                })?;
        sink.event(&Event::Locked);
        Ok(lock)
    }

    /// D.1 steps 2-7.
    fn open<'s>(
        &mut self,
        sink: &'s mut dyn EventSink,
        gate: Gate,
    ) -> Result<Session<'s>, EngineError> {
        let journal = self.read_journal()?;
        if let Some(bad) = journal.unreadable.first() {
            return Err(EngineError::JournalUnreadable(bad.error.clone()));
        }
        if gate != Gate::Recover {
            let stuck: Vec<OpId> = journal
                .entries
                .iter()
                .filter(|e| needs_recovery(e))
                .map(|e| e.op_id.clone())
                .collect();
            if !stuck.is_empty() {
                return Err(EngineError::RecoveryNeeded { op_ids: stuck });
            }
        }
        if gate == Gate::NewOp
            && let Some(open) = journal.open_entries().first()
        {
            return Err(EngineError::OpInProgress {
                op_id: open.op_id.clone(),
                state: open.state,
            });
        }
        let inventory = self.devices.keyboards()?;
        let boot = self.host.boot_id()?;
        let process = self.host.current_process()?;
        let mut s = Session {
            sink,
            journal,
            keyboards: inventory.keyboards,
            global: GlobalSettings::default(),
            boot,
            process,
            warnings: Vec::new(),
            inv_ps2: None,
        };
        for warning in inventory.warnings {
            s.warn(warning);
        }
        self.refresh(&mut s)?;
        for warning in self.host.drain_warnings() {
            s.warn(warning);
        }
        // A quarantine moved `Recovery` aside with the base directory: put the recovery files
        // back where the documentation points (G.1), or WinRE finds nothing there. Only a warning
        // when that fails; the values are not being changed yet.
        if self.host.take_recovery_assets_moved() && !s.journal.baselines.is_empty() {
            let baselines = s.journal.baselines.clone();
            self.write_assets(&mut s, &baselines, false)?;
        }
        self.housekeeping(&mut s)?;
        Ok(s)
    }

    /// D.1 step 5: every value is re-read through the backend (the single source of truth);
    /// a value of another type reads as absent for the model (`RegValue::Other`).
    ///
    /// Only a crash fails it (design review C3): it also runs after an entry went in flight, and a
    /// read error must not leave that entry behind, nor let one unreadable key (a phantom, a key
    /// an EDR product locked) stop every request. A value that cannot be read is reported once as
    /// a warning. A PS/2 value (a pin, or the global pair) then counts as absent, so that INV-PS2
    /// is judged on the safe side; any other value keeps what the model had. Whatever is written
    /// is read again right before (compare-and-swap), so a stale model value is never written
    /// over.
    fn refresh(&self, s: &mut Session<'_>) -> Result<(), EngineError> {
        let mut unreadable: Vec<(WriteTarget, BackendError)> = Vec::new();
        let mut read =
            |target: &WriteTarget, name: &str| -> Result<Option<RegValue>, EngineError> {
                match self.registry.read_value(target, name) {
                    Ok(value) => Ok(Some(value)),
                    Err(BackendError::DeviceRemoved { .. }) => Ok(Some(RegValue::Absent)),
                    Err(BackendError::Crashed) => Err(BackendError::Crashed.into()),
                    Err(error) => {
                        if !unreadable.iter().any(|(t, _)| t == target) {
                            unreadable.push((target.clone(), error));
                        }
                        Ok(None)
                    }
                }
            };
        for kb in &mut s.keyboards {
            let target = device_target(&kb.instance_id);
            let old = kb.overrides.clone();
            // `ps2`: an unreadable value counts as absent rather than as the old value.
            let mut dword = |name: &str, previous: Option<u32>, ps2: bool| {
                read(&target, name).map(|value| match value {
                    Some(value) => as_dword(&value),
                    None if ps2 => None,
                    None => previous,
                })
            };
            kb.overrides = DeviceOverrides {
                keyboard_type_override: dword(
                    value_names::HID_TYPE,
                    old.keyboard_type_override,
                    false,
                )?,
                keyboard_subtype_override: dword(
                    value_names::HID_SUBTYPE,
                    old.keyboard_subtype_override,
                    false,
                )?,
                override_keyboard_type: dword(
                    value_names::PS2_TYPE,
                    old.override_keyboard_type,
                    true,
                )?,
                override_keyboard_subtype: dword(
                    value_names::PS2_SUBTYPE,
                    old.override_keyboard_subtype,
                    true,
                )?,
                number_total_keys_override: dword(
                    value_names::HID_TOTAL_KEYS,
                    old.number_total_keys_override,
                    false,
                )?,
                number_function_keys_override: dword(
                    value_names::HID_FUNCTION_KEYS,
                    old.number_function_keys_override,
                    false,
                )?,
                number_indicators_override: dword(
                    value_names::HID_INDICATORS,
                    old.number_indicators_override,
                    false,
                )?,
            };
        }
        let global = WriteTarget::Global;
        let old = s.global.clone();
        let mut string = |name: &str, previous: Option<String>| {
            read(&global, name).map(|value| value.map_or(previous, |value| as_string(&value)))
        };
        let layer_driver_jpn = string(value_names::LAYER_DRIVER_JPN, old.layer_driver_jpn)?;
        let layer_driver_kor = string(value_names::LAYER_DRIVER_KOR, old.layer_driver_kor)?;
        let override_keyboard_identifier = string(
            value_names::KEYBOARD_IDENTIFIER,
            old.override_keyboard_identifier,
        )?;
        let mut pair =
            |name: &str| read(&global, name).map(|value| value.and_then(|value| as_dword(&value)));
        s.global = GlobalSettings {
            layer_driver_jpn,
            layer_driver_kor,
            override_keyboard_identifier,
            override_keyboard_type: pair(value_names::PS2_TYPE)?,
            override_keyboard_subtype: pair(value_names::PS2_SUBTYPE)?,
        };
        for (target, error) in unreadable {
            let path = ValueKey {
                target,
                name: String::new(),
            }
            .key_path();
            let message = format!("{path}: the values could not be read ({error})");
            if !s.warnings.contains(&message) {
                s.warn(message);
            }
        }
        Ok(())
    }

    /// D.1 step 7: clear `apply_pending` that no longer applies; close `RevertedPendingReboot`
    /// entries of an earlier boot.
    fn housekeeping(&mut self, s: &mut Session<'_>) -> Result<(), EngineError> {
        let entries = s.journal.entries.clone();
        for mut entry in entries {
            if entry.state == OpState::RevertedPendingReboot && entry.boot_id != s.boot {
                self.move_to(s, &mut entry, OpState::Reverted, "reboot-observed")?;
                entry.apply_pending = None;
                self.commit(s, &entry)?;
            } else if let Some(pending) = &entry.apply_pending
                // An entry recovery still has to act on is about to change its values.
                && !needs_recovery(&entry)
                && apply_pending_cleared(pending, s.boot, &|id| s.reports_stored_type(id))
            {
                entry.apply_pending = None;
                entry.updated_at = self.host.now();
                self.save(s, &entry)?;
            }
        }
        // A confirmed restore to baseline whose clean-up (C.5) was interrupted: finish it.
        let restores: Vec<JournalEntry> = s
            .journal
            .entries
            .iter()
            .filter(|e| e.state == OpState::Confirmed && is_restore(e))
            .cloned()
            .collect();
        for entry in restores {
            self.cleanup_baselines(s, &entry)?;
        }
        Ok(())
    }

    fn find_entry(s: &Session<'_>, op_id: &OpId) -> Result<JournalEntry, EngineError> {
        s.journal
            .entry(op_id)
            .cloned()
            .ok_or_else(|| EngineError::UnknownOp {
                op_id: op_id.to_string(),
            })
    }

    /// Only the operation that last changed a value may put it back (C.8). A later operation
    /// that was itself put back (`Reverted`, `RevertedPendingReboot`, `Failed`) and left the
    /// value where this one had left it made no net change, so it does not count: otherwise a
    /// `set` whose countdown ran out would make the confirmed change before it impossible to
    /// revert.
    fn check_latest(s: &Session<'_>, entry: &JournalEntry) -> Result<(), EngineError> {
        let this = (entry.seq, &entry.op_id);
        for record in entry.records.iter() {
            let Some(ours) = &record.last_written else {
                continue;
            };
            let key = record.key().canonical();
            for later in s
                .journal
                .entries
                .iter()
                .filter(|e| e.op_id != entry.op_id && (e.seq, &e.op_id) > this)
            {
                let changed = later
                    .records
                    .iter()
                    .filter(|r| r.key().canonical().eq_ignore_ascii_case(&key))
                    .filter_map(|r| r.last_written.as_ref().map(|written| (r, written)))
                    .any(|(r, written)| {
                        let put_back = matches!(
                            later.state,
                            OpState::Reverted | OpState::RevertedPendingReboot | OpState::Failed
                        );
                        !(put_back && value_eq(&r.name, written, ours))
                    });
                if changed {
                    return Err(EngineError::NotLatest {
                        op_id: entry.op_id.clone(),
                        later: later.op_id.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Registry and journal primitives
    // ---------------------------------------------------------------------------------------

    fn with_retry<T>(
        &mut self,
        mut call: impl FnMut(&mut R) -> Result<T, BackendError>,
    ) -> Result<T, BackendError> {
        match call(&mut self.registry) {
            Err(error) if retryable(&error) => call(&mut self.registry),
            result => result,
        }
    }

    /// The current value; `None` when the devnode is gone (distinct from `Absent`, C3).
    fn read_current(
        &self,
        target: &WriteTarget,
        name: &str,
    ) -> Result<Option<RegValue>, BackendError> {
        match self.registry.read_value(target, name) {
            Ok(value) => Ok(Some(value)),
            Err(BackendError::DeviceRemoved { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The current value of every record, `None` for a removed devnode or a record a restore
    /// skipped on purpose (recovery leaves both out of the observation). A value that cannot be
    /// read (not a crash; read twice) fails with [`Unobservable`], naming the records.
    fn observe_values(
        &self,
        entry: &JournalEntry,
    ) -> Result<Result<Vec<Option<RegValue>>, Unobservable>, EngineError> {
        let mut values = Vec::with_capacity(entry.records.len());
        let mut unreadable = Vec::new();
        for (index, record) in entry.records.iter().enumerate() {
            if record.skipped.is_some() {
                values.push(None);
                continue;
            }
            let read = || self.read_current(&record.target, &record.name);
            match read().or_else(|error| {
                if retryable(&error) {
                    read()
                } else {
                    Err(error)
                }
            }) {
                Ok(value) => values.push(value),
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                Err(error) => {
                    values.push(None);
                    unreadable.push((index, error));
                }
            }
        }
        Ok(if unreadable.is_empty() {
            Ok(values)
        } else {
            Err(Unobservable {
                records: unreadable,
            })
        })
    }

    /// An open entry whose values cannot be read (design review C3 and I7): recovery cannot
    /// decide anything about it. One that needs recovery (in flight or counting down) goes to
    /// `Conflict` with the read errors in `write_error`, so that it no longer blocks every other
    /// request and the user decides (`undo`, `resolve`); any other entry is left as it is.
    /// Returns true when the entry moved.
    fn unobservable(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        unobservable: &Unobservable,
        reason: &str,
    ) -> Result<bool, EngineError> {
        let names: Vec<String> = unobservable
            .records
            .iter()
            .filter_map(|(index, error)| {
                entry
                    .records
                    .get(*index)
                    .map(|r| format!(r"{}\{}: {error}", r.key_path, r.name))
            })
            .collect();
        if !needs_recovery(entry) {
            s.warn(format!(
                "{}: left as it is, because a value could not be read: {}",
                entry.op_id,
                names.join("; ")
            ));
            return Ok(false);
        }
        for (index, error) in &unobservable.records {
            if let Some(record) = entry.records.get_mut(*index) {
                record.write_error = Some(format!("could not be read: {error}"));
            }
        }
        s.warn(format!(
            "{}: a value could not be read, so it waits for your decision: {}",
            entry.op_id,
            names.join("; ")
        ));
        self.transition(s, entry, OpState::Conflict, reason)?;
        Ok(true)
    }

    /// J → FJ of one entry (no state change implied).
    fn save(&mut self, s: &mut Session<'_>, entry: &JournalEntry) -> Result<(), EngineError> {
        let json = entry.to_json().map_err(internal)?;
        let slot = JournalSlot::Op(entry.op_id.clone());
        self.with_retry(|r| r.write_journal(&slot, &json))?;
        self.with_retry(|r| r.flush_journal())?;
        s.put(entry);
        Ok(())
    }

    /// Moves the entry in memory (the caller persists it with [`Self::commit`]).
    fn move_to(
        &self,
        s: &Session<'_>,
        entry: &mut JournalEntry,
        to: OpState,
        reason: &str,
    ) -> Result<(), EngineError> {
        entry
            .transition(to, self.host.now(), s.boot, s.process, reason)
            .map_err(internal)?;
        if let Some(line) = entry.history.last_mut() {
            line.boot_time_hint = self.host.boot_time_hint();
        }
        Ok(())
    }

    /// J → FJ, then `StateChanged` (C.5: the event follows the flush).
    fn commit(&mut self, s: &mut Session<'_>, entry: &JournalEntry) -> Result<(), EngineError> {
        self.save(s, entry)?;
        s.sink.event(&Event::StateChanged {
            op_id: entry.op_id.clone(),
            state: entry.state,
        });
        Ok(())
    }

    fn transition(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        to: OpState,
        reason: &str,
    ) -> Result<(), EngineError> {
        self.move_to(s, entry, to, reason)?;
        self.commit(s, entry)
    }

    fn take_over(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        reason: &str,
    ) -> Result<(), EngineError> {
        entry.take_over(self.host.now(), s.boot, s.process, reason);
        self.save(s, entry)
    }

    /// Like [`Self::take_over`] (owner and a history line), but `boot_id` keeps the boot of the
    /// last write phase.
    fn take_over_same_boot(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        reason: &str,
    ) -> Result<(), EngineError> {
        let at = self.host.now();
        entry.owner = s.process;
        entry.updated_at = at;
        entry.history.push(TransitionRecord {
            from: Some(entry.state),
            to: entry.state,
            at,
            boot: s.boot,
            by: s.process,
            reason: reason.to_string(),
            boot_time_hint: self.host.boot_time_hint(),
        });
        if entry.history.len() > MAX_HISTORY {
            let excess = entry.history.len() - MAX_HISTORY;
            entry.history.drain(1..=excess);
        }
        self.save(s, entry)
    }

    /// C.9: after a terminal transition, delete the closed entries nobody needs any more.
    fn prune(&mut self, s: &mut Session<'_>) -> Result<(), EngineError> {
        let ids = s.journal.prunable(self.config.keep_closed_ops);
        let mut deleted: Vec<OpId> = Vec::new();
        for id in ids {
            let slot = JournalSlot::Op(id.clone());
            match self.with_retry(|r| r.delete_journal(&slot)) {
                Ok(()) => deleted.push(id),
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                Err(error) => {
                    s.warn(format!("old journal entries were not pruned: {error}"));
                    break;
                }
            }
        }
        if deleted.is_empty() {
            return Ok(());
        }
        match self.with_retry(|r| r.flush_journal()) {
            Ok(()) => {}
            Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
            Err(error) => s.warn(format!("old journal entries were not pruned: {error}")),
        }
        s.journal.entries.retain(|e| !deleted.contains(&e.op_id));
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // New operations (C.5)
    // ---------------------------------------------------------------------------------------

    /// D.2 step 4: one record per planned write whose value changes; `baseline` from the store,
    /// else the current value.
    fn build_records(
        &self,
        s: &Session<'_>,
        steps: &[PlanStep],
    ) -> Result<Vec<ValueRecord>, EngineError> {
        let mut records = Vec::new();
        for step in steps {
            for write in &step.writes {
                let before = self.registry.read_value(&step.target, &write.name)?;
                let intended = RegValue::from(&write.op);
                if value_eq(&write.name, &before, &intended) {
                    continue;
                }
                let key = ValueKey {
                    target: step.target.clone(),
                    name: write.name.clone(),
                };
                let baseline = s
                    .journal
                    .baseline(&key)
                    .map_or_else(|| before.clone(), |b| b.value.clone());
                records.push(ValueRecord {
                    target: step.target.clone(),
                    key_path: key.key_path(),
                    name: write.name.clone(),
                    baseline,
                    before,
                    intended,
                    last_written: None,
                    conflict: None,
                    resolve_to: None,
                    write_error: None,
                    skipped: None,
                });
            }
        }
        Ok(records)
    }

    /// Plan 1.3 step 1: every value of the global key and of every keyboard's key.
    fn snapshot_context(&self, s: &mut Session<'_>) -> Result<Vec<ContextValue>, EngineError> {
        let mut targets = vec![WriteTarget::Global];
        targets.extend(s.keyboards.iter().map(|kb| device_target(&kb.instance_id)));
        let mut context = Vec::new();
        for target in targets {
            let key_path = ValueKey {
                target: target.clone(),
                name: String::new(),
            }
            .key_path();
            match self.registry.list_values(&target) {
                Ok(values) => {
                    context.extend(values.into_iter().map(|(name, value)| ContextValue {
                        target: target.clone(),
                        key_path: key_path.clone(),
                        name,
                        value,
                    }))
                }
                Err(BackendError::DeviceRemoved { .. }) => {}
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                // The context is a record for the user; a key MKLM cannot read (a phantom, a key
                // an EDR product locked) must not stop the request. The values this operation
                // writes are read on their own before it starts.
                Err(error) => s.warn(format!(
                    "{key_path}: not recorded in the operation's context ({error})"
                )),
            }
        }
        Ok(context)
    }

    fn new_op_id(&mut self, s: &Session<'_>) -> Result<OpId, EngineError> {
        for _ in 0..8 {
            let id = self.host.new_op_id()?;
            let taken = s.journal.entry(&id).is_some()
                || s.journal
                    .unreadable
                    .iter()
                    .any(|u| u.name.eq_ignore_ascii_case(id.as_str()));
            if !taken {
                return Ok(id);
            }
        }
        Err(internal("no unused operation ID"))
    }

    /// Writes the recovery files for `baselines` (G.1). `required`: the operation changes
    /// boot-time values and must not start without them (C10).
    fn write_assets(
        &mut self,
        s: &mut Session<'_>,
        baselines: &[BaselineRecord],
        required: bool,
    ) -> Result<(), EngineError> {
        let failed = |s: &mut Session<'_>, error: HostError| {
            if required {
                Err(EngineError::RecoveryAssetsUnavailable(error))
            } else {
                s.warn(format!(
                    "the offline recovery files could not be written: {error}"
                ));
                Ok(())
            }
        };
        let assets = match render_recovery_assets(baselines, GENERATOR_VERSION, self.host.now()) {
            Ok(assets) => assets,
            Err(error) => {
                return failed(
                    s,
                    HostError::Os {
                        what: error.to_string(),
                        code: 0,
                    },
                );
            }
        };
        match self.host.write_recovery_assets(&assets) {
            Ok(()) => {
                s.sink.event(&Event::RecoveryAssetsWritten {
                    directory: RECOVERY_DIRECTORY.to_string(),
                    skipped: assets.skipped.len(),
                });
                if !assets.skipped.is_empty() {
                    s.warn(format!(
                        "{} value(s) could not be put into restore-offline.cmd; use mklm-baseline.reg",
                        assets.skipped.len()
                    ));
                }
                Ok(())
            }
            Err(error) => failed(s, error),
        }
    }

    /// C.5 steps 2-5: recovery files, cancellation point, baselines + `Planned` → FJ, then
    /// [`Event::Planned`].
    fn create(
        &mut self,
        s: &mut Session<'_>,
        kind: OpKind,
        records: Vec<ValueRecord>,
        apply: Option<PendingAction>,
        context: Vec<ContextValue>,
        keyboards: Vec<ExpectedKeyboard>,
    ) -> Result<JournalEntry, EngineError> {
        let op_id = self.new_op_id(s)?;
        let now = self.host.now();
        let mut new_baselines: Vec<BaselineRecord> = Vec::new();
        for record in records.iter().filter(|r| r.skipped.is_none()) {
            let key = record.key();
            let canonical = key.canonical();
            let known = s.journal.baseline(&key).is_some()
                || new_baselines
                    .iter()
                    .any(|b| b.key.canonical().eq_ignore_ascii_case(&canonical));
            if !known {
                new_baselines.push(BaselineRecord {
                    schema_version: BASELINE_SCHEMA_VERSION,
                    key,
                    key_path: record.key_path.clone(),
                    value: record.before.clone(),
                    captured_at: now,
                    captured_by: op_id.clone(),
                });
            }
        }
        let boot_time = records
            .iter()
            .any(|r| r.skipped.is_none() && r.is_boot_time());
        if !new_baselines.is_empty() || boot_time {
            let mut all = s.journal.baselines.clone();
            all.extend(new_baselines.iter().cloned());
            self.write_assets(s, &all, boot_time)?;
        }
        if s.sink.check_cancelled() {
            return Err(EngineError::Cancelled);
        }

        let reason = match &kind {
            OpKind::SetLayout { .. } => "set-layout",
            OpKind::Migrate { .. } => "migrate",
            OpKind::RestoreBaseline { .. } => "restore-baseline",
            OpKind::Cleanup { .. } => "cleanup",
        };
        let mut entry = JournalEntry {
            // 2 for a cleanup only, so that older builds keep reading the others (design m3 K.13).
            schema_version: kind.schema_version(),
            op_id: op_id.clone(),
            seq: s.journal.next_seq(),
            kind,
            state: OpState::Planned,
            boot_id: s.boot,
            owner: s.process,
            created_at: now,
            updated_at: now,
            apply,
            countdown: None,
            records,
            context,
            failure: None,
            revert_mode: None,
            apply_pending: None,
            history: vec![TransitionRecord {
                from: None,
                to: OpState::Planned,
                at: now,
                boot: s.boot,
                by: s.process,
                reason: reason.to_string(),
                boot_time_hint: self.host.boot_time_hint(),
            }],
        };
        let json = entry.to_json().map_err(internal)?;

        // B… J(Planned) → FJ: one flush makes them durable together.
        let mut written: Vec<JournalSlot> = Vec::new();
        let mut failure = None;
        for baseline in &new_baselines {
            let slot = JournalSlot::Baseline(baseline.key.canonical());
            let text = baseline.to_json().map_err(internal)?;
            match self.with_retry(|r| r.write_journal(&slot, &text)) {
                Ok(()) => written.push(slot),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let slot = JournalSlot::Op(op_id.clone());
        if failure.is_none()
            && let Err(error) = self.with_retry(|r| r.write_journal(&slot, &json))
        {
            failure = Some(error);
        }
        if let Some(error) = failure {
            if error != BackendError::Crashed {
                // Nothing was journaled: take the baselines back (best effort).
                for slot in &written {
                    let _ = self.registry.delete_journal(slot);
                }
                let _ = self.registry.flush_journal();
            }
            return Err(error.into());
        }
        if let Err(error) = self.with_retry(|r| r.flush_journal()) {
            if error != BackendError::Crashed {
                // Visible but maybe not durable: close it rather than leave it in flight (C3).
                entry.failure = Some(FailureReason::WriteError {
                    message: error.to_string(),
                });
                if self
                    .move_to(s, &mut entry, OpState::Failed, "journal-flush-failed")
                    .is_ok()
                {
                    let _ = self.save(s, &entry);
                }
            }
            return Err(error.into());
        }
        s.journal.baselines.extend(new_baselines);
        s.put(&entry);
        s.sink.event(&Event::Planned {
            op_id,
            steps: plan_steps(&entry.records),
            apply,
            keyboards,
        });
        Ok(entry)
    }

    /// C.5 step 6: per step, compare-and-swap on `before` and write, then flush the key.
    fn write_forward(
        &mut self,
        s: &mut Session<'_>,
        entry: &JournalEntry,
    ) -> Result<Forward, EngineError> {
        let steps = group_steps(&entry.records);
        let of = steps.len();
        let mut wrote = false;
        for (number, (target, indices)) in steps.iter().enumerate() {
            for &index in indices {
                let record = &entry.records[index];
                let current = match self.registry.read_value(target, &record.name) {
                    Ok(value) => value,
                    Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                    Err(error) => {
                        return Ok(Forward::Failed {
                            message: error.to_string(),
                        });
                    }
                };
                if !value_eq(&record.name, &current, &record.before) {
                    return Ok(if wrote {
                        Forward::Failed {
                            message: format!(
                                "{}\\{} changed while MKLM was writing",
                                record.key_path, record.name
                            ),
                        }
                    } else {
                        Forward::NothingWritten {
                            name: record.name.clone(),
                        }
                    });
                }
                match self
                    .registry
                    .write_value(target, &record.name, &record.intended)
                {
                    Ok(()) => wrote = true,
                    Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                    Err(error) => {
                        return Ok(Forward::Failed {
                            message: error.to_string(),
                        });
                    }
                }
            }
            match self.registry.flush_target(target) {
                Ok(()) => {}
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                Err(error) => {
                    return Ok(Forward::Failed {
                        message: error.to_string(),
                    });
                }
            }
            s.sink.event(&Event::StepWritten {
                op_id: entry.op_id.clone(),
                step: number + 1,
                of,
            });
        }
        Ok(Forward::Done)
    }

    /// Forward writes and the `Written` transition. `Some(result)` when the operation ended
    /// early: nothing written (`Failed(ConcurrentChange)`) or rolled back after an error.
    fn write_new(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
    ) -> Result<Option<OperationResult>, EngineError> {
        match self.write_forward(s, entry)? {
            Forward::Done => {
                for record in &mut entry.records {
                    record.last_written = Some(record.intended.clone());
                }
                self.transition(s, entry, OpState::Written, "written")?;
                // The model follows the new values (expected types of the apply paths).
                self.refresh(s)?;
                Ok(None)
            }
            Forward::NothingWritten { name } => {
                entry.failure = Some(FailureReason::ConcurrentChange { name });
                self.transition(s, entry, OpState::Failed, "concurrent-change")?;
                self.prune(s)?;
                Ok(Some(self.entry_result(s, entry)))
            }
            Forward::Failed { message } => {
                s.warn(format!("{}: writing failed: {message}", entry.op_id));
                self.roll_back(
                    s,
                    entry,
                    FailureReason::WriteError { message },
                    "write-error",
                    &[],
                    false,
                )?;
                self.prune(s)?;
                Ok(Some(self.entry_result(s, entry)))
            }
        }
    }

    /// D.2 step 10 (and D.5 step 8 for an interactive restore): how the written change takes
    /// effect. Without an `apply` (a cleanup, or a restore of values no driver reads), nothing
    /// has to take effect: `AwaitingConfirm` without a countdown, for the user's keep or revert
    /// (design m3 A.5; never confirmed automatically).
    fn apply_change(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        lock: H::Lock,
        apply: ApplyOptions,
    ) -> Result<OperationResult, EngineError> {
        match entry.apply {
            Some(PendingAction::ResetKeyboard) => self.live_reset(s, entry, lock, apply),
            Some(PendingAction::Reconnect) => {
                self.move_to(s, entry, OpState::AwaitingConfirm, "reconnect")?;
                entry.apply_pending = self.close_pending(s, entry, &[], false)?;
                self.commit(s, entry)?;
                self.reconnect_wait(s, entry, lock, apply)
            }
            Some(PendingAction::RestartPc) => {
                self.move_to(s, entry, OpState::PendingReboot, "restart-pc")?;
                entry.apply_pending = self.close_pending(s, entry, &[], false)?;
                self.commit(s, entry)?;
                drop(lock);
                Ok(self.entry_result(s, entry))
            }
            None => {
                self.move_to(s, entry, OpState::AwaitingConfirm, "nothing-to-apply")?;
                entry.apply_pending = self.close_pending(s, entry, &[], false)?;
                self.commit(s, entry)?;
                drop(lock);
                Ok(self.entry_result(s, entry))
            }
        }
    }

    /// D.2 a: live reset, countdown, keep or revert (with a second reset).
    fn live_reset(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        lock: H::Lock,
        apply: ApplyOptions,
    ) -> Result<OperationResult, EngineError> {
        // S4 / C13: a caller that left does not get its keyboard reset.
        if s.sink.check_cancelled() {
            self.roll_back(
                s,
                entry,
                FailureReason::CallerDisconnected,
                "caller-disconnected",
                &[],
                false,
            )?;
            self.prune(s)?;
            return Ok(self.entry_result(s, entry));
        }
        self.transition(s, entry, OpState::Restarting, "live-reset")?;
        self.refresh(s)?;
        // `apply_method` chose the reset only for keyboards without a ban; this is a second net:
        // never reset a PS/2, built-in, virtual or unproven keyboard in place (plan 1.4).
        let targets: Vec<String> = Self::hid_keyboards(s, entry)
            .into_iter()
            .filter(|id| {
                s.keyboard(id)
                    .is_some_and(|kb| kb.present && structural_reset_bans(kb).is_empty())
            })
            .collect();
        let old: Vec<(String, Option<KeyboardType>)> = targets
            .iter()
            .map(|id| (id.clone(), s.keyboard(id).and_then(|kb| kb.reported_type)))
            .collect();
        let (reapplied, all_back, arrived) = self.reset_keyboards(s, &targets, true);
        if !all_back {
            // Plan 1.4: put the values back and ask for a PC restart.
            self.roll_back_reboot(s, entry, FailureReason::KeyboardDidNotReturn)?;
            self.prune(s)?;
            return Ok(self.entry_result(s, entry));
        }
        let unchanged = arrived.iter().any(|(id, reported)| {
            let was = old
                .iter()
                .find(|(o, _)| o.eq_ignore_ascii_case(id))
                .and_then(|(_, t)| *t);
            reported.is_some() && *reported != s.expected_type(id) && *reported == was
        });
        if unchanged {
            // D.2 a step 5: the reset did not apply it; wait for a reconnect instead.
            entry.apply = Some(PendingAction::Reconnect);
            self.move_to(s, entry, OpState::AwaitingConfirm, "reset-not-applied")?;
            entry.apply_pending = self.close_pending(s, entry, &[], false)?;
            self.commit(s, entry)?;
            return self.reconnect_wait(s, entry, lock, apply);
        }
        let verified = !arrived.is_empty()
            && arrived
                .iter()
                .all(|(id, reported)| reported.is_some() && *reported == s.expected_type(id));
        // 20 or 60 s, checked when the request came in (design m3 WP-E3).
        let seconds = apply.countdown_seconds;
        let now = self.host.now();
        entry.countdown = Some(Countdown {
            seconds,
            deadline: Timestamp(now.0.saturating_add(u64::from(seconds) * 1000)),
        });
        self.transition(s, entry, OpState::AwaitingConfirm, "countdown")?;
        s.sink.event(&Event::CountdownStarted {
            op_id: entry.op_id.clone(),
            seconds,
            verified,
        });
        let end = self.countdown(s, &entry.op_id.clone(), seconds);
        match end {
            CountdownEnd::Keep => {
                self.close_confirmed(s, entry, &reapplied, "keep")?;
            }
            CountdownEnd::RevertNow | CountdownEnd::Disconnected | CountdownEnd::Expired => {
                let (failure, reason) = match end {
                    CountdownEnd::Disconnected => (
                        Some(FailureReason::CallerDisconnected),
                        "caller-disconnected",
                    ),
                    CountdownEnd::Expired => {
                        (Some(FailureReason::CountdownExpired), "countdown-expired")
                    }
                    _ => (None, "revert-now"),
                };
                self.roll_back_with(s, entry, failure, reason, &targets, true)?;
            }
        }
        drop(lock);
        self.prune(s)?;
        Ok(self.entry_result(s, entry))
    }

    /// D.2 a step 7: one tick per second; bounded by the monotonic clock too (C18).
    fn countdown(&mut self, s: &mut Session<'_>, op_id: &OpId, seconds: u32) -> CountdownEnd {
        let limit = Duration::from_secs(u64::from(seconds)) + self.config.countdown_slack;
        let start = self.host.monotonic();
        let mut remaining = seconds;
        loop {
            if remaining == 0 || self.host.monotonic().saturating_sub(start) >= limit {
                return CountdownEnd::Expired;
            }
            s.sink.event(&Event::CountdownTick {
                op_id: op_id.clone(),
                remaining,
            });
            match s.sink.wait_decision(Duration::from_secs(1)) {
                DecisionPoll::Decided(Decision::Keep { op_id: id }) if id == *op_id => {
                    return CountdownEnd::Keep;
                }
                DecisionPoll::Decided(Decision::RevertNow { op_id: id }) if id == *op_id => {
                    return CountdownEnd::RevertNow;
                }
                DecisionPoll::Decided(other) => s.warn(format!(
                    "ignored a decision for operation {}",
                    decision_op(&other)
                )),
                DecisionPoll::Disconnected => return CountdownEnd::Disconnected,
                DecisionPoll::NoDecision => {}
            }
            remaining -= 1;
        }
    }

    /// D.2 b: the entry waits in `AwaitingConfirm` (persisted); the lock is released; wait for the
    /// keyboard, then for a decision. Never reverts on its own.
    fn reconnect_wait(
        &mut self,
        s: &mut Session<'_>,
        entry: &JournalEntry,
        lock: H::Lock,
        apply: ApplyOptions,
    ) -> Result<OperationResult, EngineError> {
        drop(lock);
        self.refresh(s)?;
        let op_id = entry.op_id.clone();
        let targets = Self::hid_keyboards(s, entry);
        let mut decision: Option<Decision> = None;
        let mut stray = 0;
        let start = self.host.monotonic();
        let mut waited = Duration::ZERO;
        loop {
            let reports: Vec<(String, Option<KeyboardType>)> = targets
                .iter()
                .map(|id| (id.clone(), self.devices.reported_type(id).ok().flatten()))
                .collect();
            let listed = reports.iter().any(|(_, r)| r.is_some());
            let as_expected = reports
                .iter()
                .all(|(id, r)| r.is_none() || *r == s.expected_type(id));
            if listed && as_expected {
                for (id, reported) in &reports {
                    if let (Some(_), Some(expected)) = (reported, s.expected_type(id)) {
                        s.sink.event(&Event::KeyboardArrived {
                            instance_id: id.clone(),
                            reported: *reported,
                            expected,
                        });
                    }
                }
                break;
            }
            if waited >= self.config.reconnect_wait
                || self.host.monotonic().saturating_sub(start) >= self.config.reconnect_wait
            {
                break;
            }
            s.sink.event(&Event::WaitingForReconnect {
                op_id: op_id.clone(),
                instance_ids: targets.clone(),
            });
            match s.sink.wait_decision(RECONNECT_TICK) {
                DecisionPoll::Decided(d) if *decision_op(&d) == op_id => {
                    decision = Some(d);
                    break;
                }
                DecisionPoll::Decided(_) => {
                    stray += 1;
                    if stray > MAX_STRAY_DECISIONS {
                        break;
                    }
                }
                DecisionPoll::Disconnected => return Ok(self.entry_result(s, entry)),
                DecisionPoll::NoDecision => {}
            }
            waited += RECONNECT_TICK;
        }
        if decision.is_none() {
            let start = self.host.monotonic();
            loop {
                let elapsed = self.host.monotonic().saturating_sub(start);
                if elapsed >= self.config.decision_wait {
                    break;
                }
                match s.sink.wait_decision(self.config.decision_wait - elapsed) {
                    DecisionPoll::Decided(d) if *decision_op(&d) == op_id => {
                        decision = Some(d);
                        break;
                    }
                    DecisionPoll::Decided(_) => {
                        stray += 1;
                        if stray > MAX_STRAY_DECISIONS {
                            break;
                        }
                    }
                    DecisionPoll::NoDecision | DecisionPoll::Disconnected => break,
                }
            }
        }
        let result = match decision {
            None => return Ok(self.entry_result(s, entry)),
            Some(Decision::Keep { .. }) => self.confirm(&op_id, &mut *s.sink)?,
            Some(Decision::RevertNow { .. }) => self.revert(&op_id, &apply, &mut *s.sink)?,
        };
        let mut warnings = s.warnings.clone();
        warnings.extend(result.warnings);
        Ok(OperationResult { warnings, ..result })
    }

    /// Resets `ids` one at a time (`restart` + `wait_for_arrival`). Returns the keyboards that
    /// came back, whether all did, and what each arrived one reports.
    fn reset_keyboards(
        &mut self,
        s: &mut Session<'_>,
        ids: &[String],
        stop_on_failure: bool,
    ) -> ResetOutcome {
        let mut reapplied = Vec::new();
        let mut arrived = Vec::new();
        let mut all_back = true;
        for id in ids {
            s.sink.event(&Event::ResettingKeyboard {
                instance_id: id.clone(),
            });
            let back = match self.devices.restart(id) {
                Ok(RestartOutcome::Restarted) => {
                    match self
                        .devices
                        .wait_for_arrival(id, self.config.arrival_timeout)
                    {
                        Ok(Arrival::Started { reported }) => {
                            if let Some(expected) = s.expected_type(id) {
                                s.sink.event(&Event::KeyboardArrived {
                                    instance_id: id.clone(),
                                    reported,
                                    expected,
                                });
                            }
                            arrived.push((id.clone(), reported));
                            true
                        }
                        Ok(Arrival::TimedOut) => {
                            s.warn(format!("{id}: did not come back after the reset"));
                            false
                        }
                        Err(error) => {
                            s.warn(format!("{id}: {error}"));
                            false
                        }
                    }
                }
                Ok(RestartOutcome::NeedsReboot) => {
                    s.warn(format!("{id}: Windows asks for a PC restart"));
                    false
                }
                Ok(RestartOutcome::TimedOut) => {
                    s.warn(format!("{id}: the reset did not finish in time"));
                    false
                }
                Err(error) => {
                    s.warn(format!("{id}: the reset failed: {error}"));
                    false
                }
            };
            if back {
                reapplied.push(id.clone());
            } else {
                all_back = false;
                if stop_on_failure {
                    break;
                }
            }
        }
        (reapplied, all_back, arrived)
    }

    // ---------------------------------------------------------------------------------------
    // Putting values back (C.5 "取り消し、ロールバック、解決、undo")
    // ---------------------------------------------------------------------------------------

    /// `plan_restore` over `subset` of `records` (indices), with the model re-read first.
    fn plan_subset(
        &self,
        s: &Session<'_>,
        records: &[ValueRecord],
        subset: &[usize],
        to: RestoreTo,
        expect: &dyn Fn(&ValueRecord) -> Expect,
    ) -> Result<RestorePlan, RestoreError> {
        let chosen: Vec<ValueRecord> = subset.iter().map(|&i| records[i].clone()).collect();
        plan_restore(
            &chosen,
            to,
            &|i| expect(&chosen[i]),
            &s.keyboards,
            &s.global,
            &s.journal.baselines,
        )
    }

    /// Writes a restore plan (C.5): per step, every value is read first; a value already at its
    /// restore value is done, one at its expectation is written, a removed devnode is skipped,
    /// anything else is a conflict and the step is not written at all (with `skip_conflicts`,
    /// the value is skipped instead). A write that keeps failing (one retry) is recorded in
    /// `write_error`. Returns false when a step could not be written completely; the remaining
    /// steps are not written.
    fn run_restore(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        plan: &RestorePlan,
        subset: &[usize],
        skip_conflicts: bool,
    ) -> Result<bool, EngineError> {
        let of = plan.steps.len();
        for (number, step) in plan.steps.iter().enumerate() {
            let mut writes: Vec<(usize, RegValue)> = Vec::new();
            let mut blocked = false;
            let mut skipped_boot_time = false;
            for write in &step.writes {
                let index = subset[write.record];
                let name = entry.records[index].name.clone();
                match self.read_current(&step.target, &name) {
                    Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                    Err(error) => {
                        entry.records[index].write_error = Some(error.to_string());
                        blocked = true;
                    }
                    Ok(None) => entry.records[index].skipped = Some(SkipReason::DeviceRemoved),
                    Ok(Some(current)) if value_eq(&name, &current, &write.value) => {
                        let record = &mut entry.records[index];
                        record.last_written = Some(write.value.clone());
                        record.write_error = None;
                    }
                    Ok(Some(current)) if expect_matches(&write.expect, &name, &current) => {
                        writes.push((index, write.value.clone()));
                    }
                    Ok(Some(current)) => {
                        let record = &mut entry.records[index];
                        record.conflict = Some(current);
                        if skip_conflicts {
                            record.skipped = Some(SkipReason::ConflictSkipped);
                            skipped_boot_time |= record.is_boot_time();
                            let text = format!(
                                "{}\\{}: changed outside MKLM; left alone",
                                record.key_path, record.name
                            );
                            s.warn(text);
                        } else {
                            blocked = true;
                        }
                    }
                }
            }
            if blocked {
                return Ok(false);
            }
            let mut written: Vec<(usize, RegValue)> = Vec::new();
            let mut failed = false;
            for (index, value) in writes {
                let name = entry.records[index].name.clone();
                match self.with_retry(|r| r.write_value(&step.target, &name, &value)) {
                    Ok(()) => written.push((index, value)),
                    Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                    Err(BackendError::DeviceRemoved { .. }) => {
                        entry.records[index].skipped = Some(SkipReason::DeviceRemoved);
                    }
                    Err(error) => {
                        entry.records[index].write_error = Some(error.to_string());
                        failed = true;
                        break;
                    }
                }
            }
            if !written.is_empty() {
                match self.with_retry(|r| r.flush_target(&step.target)) {
                    Ok(()) => {
                        for (index, value) in &written {
                            let record = &mut entry.records[*index];
                            record.last_written = Some(value.clone());
                            record.write_error = None;
                        }
                    }
                    Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                    Err(BackendError::DeviceRemoved { .. }) => {
                        for (index, _) in &written {
                            entry.records[*index].skipped = Some(SkipReason::DeviceRemoved);
                        }
                    }
                    Err(error) => {
                        for (index, _) in &written {
                            entry.records[*index].write_error = Some(error.to_string());
                        }
                        failed = true;
                    }
                }
            }
            s.sink.event(&Event::StepWritten {
                op_id: entry.op_id.clone(),
                step: number + 1,
                of,
            });
            if failed {
                return Ok(false);
            }
            // The later phases were checked against this step being written in full; a skipped
            // i8042prt or global value voids that, so check them again from the actual values.
            if skipped_boot_time && !self.remaining_steps_safe(s, plan, number + 1)? {
                s.warn(format!(
                    "{}: the remaining values cannot be restored without breaking INV-PS2",
                    entry.op_id
                ));
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// True when INV-PS2 holds now and after each of `plan.steps[from..]` in turn.
    fn remaining_steps_safe(
        &self,
        s: &mut Session<'_>,
        plan: &RestorePlan,
        from: usize,
    ) -> Result<bool, EngineError> {
        self.refresh(s)?;
        let (mut keyboards, mut global) = (s.keyboards.clone(), s.global.clone());
        if check_inv_ps2(&global, &keyboards).is_err() {
            return Ok(false);
        }
        for step in plan.steps.iter().skip(from) {
            for write in &step.writes {
                apply_to_model(
                    &mut keyboards,
                    &mut global,
                    &step.target,
                    &write.name,
                    &write.value,
                );
            }
            if check_inv_ps2(&global, &keyboards).is_err() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Plans the way back to `before` over every record not skipped on purpose, then persists
    /// `RevertPending` (`mode`, `failure`) and writes it. Returns whether every step was written.
    /// A plan `plan_restore` rejects is returned as an error before anything is persisted.
    fn revert_values(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        mode: RevertMode,
        failure: Option<FailureReason>,
        reason: &str,
    ) -> Result<bool, EngineError> {
        self.refresh(s)?;
        let subset: Vec<usize> = (0..entry.records.len())
            .filter(|&i| entry.records[i].skipped.is_none())
            .collect();
        let plan = self.plan_subset(
            s,
            &entry.records,
            &subset,
            RestoreTo::Before,
            &expect_written,
        )?;
        entry.revert_mode = Some(mode);
        entry.failure = failure;
        self.transition(s, entry, OpState::RevertPending, reason)?;
        self.run_restore(s, entry, &plan, &subset, false)
    }

    /// `Conflict` because `plan_restore` refused the way back (C12: INV-PS2).
    fn conflict_from_plan(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        error: RestoreError,
        reason: &str,
    ) -> Result<(), EngineError> {
        s.warn(format!(
            "{}: the values cannot be put back safely: {error}",
            entry.op_id
        ));
        if let RestoreError::InvPs2(violation) = error {
            s.inv_ps2 = Some(violation);
        }
        self.transition(s, entry, OpState::Conflict, reason)
    }

    /// Rollback (`RevertMode::Rollback`) of an operation that was not kept, then `Reverted` /
    /// `RevertedPendingReboot` (or `Conflict`). `reset`: keyboards reset again afterwards to
    /// re-apply `before` (live-reset path). `pending`: record `apply_pending`.
    fn roll_back_with(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        failure: Option<FailureReason>,
        reason: &str,
        reset: &[String],
        pending: bool,
    ) -> Result<(), EngineError> {
        let completed = match self.revert_values(s, entry, RevertMode::Rollback, failure, reason) {
            Ok(completed) => completed,
            Err(EngineError::Restore(error)) => {
                return self.conflict_from_plan(s, entry, error, reason);
            }
            Err(error) => return Err(error),
        };
        if !completed {
            return self.transition(s, entry, OpState::Conflict, reason);
        }
        self.refresh(s)?;
        let (reapplied, all_back, _) = self.reset_keyboards(s, reset, false);
        self.close_reverted(s, entry, &reapplied, !all_back, reason, pending)
    }

    fn roll_back(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        failure: FailureReason,
        reason: &str,
        reset: &[String],
        pending: bool,
    ) -> Result<(), EngineError> {
        self.roll_back_with(s, entry, Some(failure), reason, reset, pending)
    }

    /// D.2 a step 4: roll back without a second reset, then `RevertedPendingReboot` with
    /// `apply_pending = RestartPc`.
    fn roll_back_reboot(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        failure: FailureReason,
    ) -> Result<(), EngineError> {
        let reason = "keyboard-did-not-return";
        let completed =
            match self.revert_values(s, entry, RevertMode::Rollback, Some(failure), reason) {
                Ok(completed) => completed,
                Err(EngineError::Restore(error)) => {
                    return self.conflict_from_plan(s, entry, error, reason);
                }
                Err(error) => return Err(error),
            };
        if !completed {
            return self.transition(s, entry, OpState::Conflict, reason);
        }
        self.close_reverted(s, entry, &[], true, reason, true)
    }

    /// `Reverted`, or `RevertedPendingReboot` for boot-time values or a keyboard that did not come
    /// back; `apply_pending` from `apply_pending_on_close` when `pending`.
    fn close_reverted(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        reapplied: &[String],
        did_not_return: bool,
        reason: &str,
        pending: bool,
    ) -> Result<(), EngineError> {
        let to = if did_not_return || entry.touches_boot_time_values() {
            OpState::RevertedPendingReboot
        } else {
            OpState::Reverted
        };
        for record in &mut entry.records {
            record.conflict = None;
        }
        self.move_to(s, entry, to, reason)?;
        entry.apply_pending = if pending {
            self.close_pending(s, entry, reapplied, false)?
        } else {
            None
        };
        self.commit(s, entry)
    }

    /// `Confirmed` (with `apply_pending` for keyboards not in `reapplied`), then the baseline
    /// clean-up of a restore (C.5).
    fn close_confirmed(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        reapplied: &[String],
        reason: &str,
    ) -> Result<(), EngineError> {
        self.move_to(s, entry, OpState::Confirmed, reason)?;
        entry.apply_pending = self.close_pending(s, entry, reapplied, false)?;
        if entry.apply_pending.is_some() {
            s.warn(format!(
                "{}: kept, but not every keyboard runs with the stored values yet",
                entry.op_id
            ));
        }
        self.commit(s, entry)?;
        if is_restore(entry) {
            self.cleanup_baselines(s, entry)?;
        }
        Ok(())
    }

    /// `apply_pending` for an entry that closes (or waits) now (C.11): `apply_pending_on_close`
    /// (`as_of_now`: as if its last write phase were this boot, for values recovery just wrote),
    /// merged with what the entry already holds for this boot, without the keyboards whose Raw
    /// Input already reports the stored type (what the next request's housekeeping would clear).
    ///
    /// Records of values their keyboard's driver does not read (`is_unread_value`, what a cleanup
    /// deleted and a restore to baseline puts back) are left out for every kind of operation, as
    /// `restore_baseline` leaves them out of `apply` (D.11): no driver runs with
    /// anything but its stored values because of them, so no keyboard waits for them.
    fn close_pending(
        &mut self,
        s: &mut Session<'_>,
        entry: &JournalEntry,
        reapplied: &[String],
        as_of_now: bool,
    ) -> Result<Option<ApplyPending>, EngineError> {
        let mut probe = entry.clone();
        probe
            .records
            .retain(|r| !is_unread_value(&s.keyboards, &r.target, &r.name));
        if as_of_now {
            probe.boot_id = s.boot;
        }
        let computed = apply_pending_on_close(&probe, s.boot, reapplied);
        let held = entry
            .apply_pending
            .clone()
            .filter(|pending| pending.since == s.boot);
        let Some(mut pending) = merge_pending(held, computed) else {
            return Ok(None);
        };
        if pending.action == PendingAction::RestartPc {
            return Ok(Some(pending));
        }
        self.refresh(s)?;
        pending.instance_ids.retain(|id| {
            let reported = self.devices.reported_type(id).ok().flatten();
            !(reported.is_some() && reported == s.expected_type(id))
        });
        Ok((!pending.instance_ids.is_empty()).then_some(pending))
    }

    /// Recovery in a later boot than the entry's last write phase: the drivers read the values as
    /// found at that boot. The HID keyboards whose values recovery is about to change (from
    /// `found` to `target`) then run with other values than the stored ones; record that before
    /// anything is written, so that an interrupted recovery keeps it too (C1).
    fn note_boot_pending(
        &self,
        s: &Session<'_>,
        entry: &mut JournalEntry,
        found: &[Option<RegValue>],
        target: &dyn Fn(&ValueRecord) -> Option<RegValue>,
    ) {
        if entry.boot_id == s.boot || is_cleanup(entry) {
            return;
        }
        let mut ids: Vec<String> = Vec::new();
        for (record, value) in entry.records.iter().zip(found) {
            let (Some(value), Some(target)) = (value, target(record)) else {
                continue;
            };
            if record.skipped.is_some()
                || record.is_boot_time()
                || is_unread_value(&s.keyboards, &record.target, &record.name)
                || value_eq(&record.name, value, &target)
            {
                continue;
            }
            if let WriteTarget::Device { instance_id } = &record.target
                && !ids.iter().any(|id| id.eq_ignore_ascii_case(instance_id))
            {
                ids.push(instance_id.clone());
            }
        }
        if ids.is_empty() {
            return;
        }
        let noted = ApplyPending {
            action: entry
                .apply
                .unwrap_or(PendingAction::Reconnect)
                .max(PendingAction::Reconnect),
            instance_ids: ids,
            since: s.boot,
        };
        let held = entry
            .apply_pending
            .take()
            .filter(|pending| pending.since == s.boot);
        entry.apply_pending = merge_pending(held, Some(noted));
    }

    /// D.4 steps 5-9: revert (`RevertMode::Revert`), optionally re-apply by resetting the HID
    /// keyboards, then close.
    fn revert_entry(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        apply: &ApplyOptions,
        failure: Option<FailureReason>,
        reason: &str,
    ) -> Result<OperationResult, EngineError> {
        let in_effect = values_in_effect(entry, s.boot);
        if !self.revert_values(s, entry, RevertMode::Revert, failure, reason)? {
            self.transition(s, entry, OpState::Conflict, reason)?;
            return Ok(self.entry_result(s, entry));
        }
        let (reapplied, all_back) = self.reapply(s, entry, apply, in_effect)?;
        self.close_reverted(s, entry, &reapplied, !all_back, reason, in_effect)?;
        Ok(self.entry_result(s, entry))
    }

    /// D.4 step 9: reset the HID keyboards whose values changed back, when the caller allows it
    /// (C9) and the values may have been in effect.
    fn reapply(
        &mut self,
        s: &mut Session<'_>,
        entry: &JournalEntry,
        apply: &ApplyOptions,
        in_effect: bool,
    ) -> Result<(Vec<String>, bool), EngineError> {
        if !(in_effect && apply.allow_live_reset && apply.other_input_available) {
            return Ok((Vec::new(), true));
        }
        self.refresh(s)?;
        let ids: Vec<String> = Self::hid_keyboards(s, entry)
            .into_iter()
            .filter(|id| {
                s.keyboard(id)
                    .is_some_and(|kb| kb.present && live_reset_bans(kb, false).is_empty())
            })
            .collect();
        let (reapplied, all_back, _) = self.reset_keyboards(s, &ids, false);
        Ok((reapplied, all_back))
    }

    /// D.10 for an entry in `Conflict`: put back only the values still at what the operation
    /// wrote; conflicting values are not written and keep the entry in `Conflict`.
    fn undo_conflict(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        apply: &ApplyOptions,
    ) -> Result<(), EngineError> {
        let in_effect = values_in_effect(entry, s.boot);
        let mut writable = Vec::new();
        let mut conflicting = false;
        for index in 0..entry.records.len() {
            let record = &entry.records[index];
            if record.skipped.is_some() {
                continue;
            }
            let current = match self.read_current(&record.target, &record.name) {
                Ok(current) => current,
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                // Nothing written yet: this entry stays in `Conflict`, and `undo` goes on with
                // the others (it is the way out, design review C7).
                Err(error) => {
                    s.warn(format!(
                        r"{}: not undone: {}\{} could not be read ({error})",
                        entry.op_id, record.key_path, record.name
                    ));
                    return Ok(());
                }
            };
            match current {
                None => entry.records[index].skipped = Some(SkipReason::DeviceRemoved),
                Some(current) if value_eq(&record.name, &current, &record.before) => {}
                Some(current)
                    if value_eq(
                        &record.name,
                        &current,
                        record.last_written.as_ref().unwrap_or(&record.intended),
                    ) =>
                {
                    writable.push(index);
                }
                Some(current) => {
                    entry.records[index].conflict = Some(current);
                    conflicting = true;
                }
            }
        }
        let failure = entry.failure.clone();
        if writable.is_empty() {
            if conflicting {
                s.put(entry);
                return Ok(());
            }
            for record in &mut entry.records {
                if record.skipped.is_none() {
                    record.last_written = Some(record.before.clone());
                }
                record.write_error = None;
            }
            return self.close_reverted(s, entry, &[], false, "undo", in_effect);
        }
        self.refresh(s)?;
        let plan = match self.plan_subset(
            s,
            &entry.records,
            &writable,
            RestoreTo::Before,
            &expect_written,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                s.warn(format!("{}: not undone: {error}", entry.op_id));
                return Ok(());
            }
        };
        entry.revert_mode = Some(RevertMode::Revert);
        entry.failure = failure;
        self.transition(s, entry, OpState::RevertPending, "undo")?;
        let completed = self.run_restore(s, entry, &plan, &writable, false)?;
        if !completed || conflicting {
            return self.transition(s, entry, OpState::Conflict, "undo:conflict");
        }
        for record in &mut entry.records {
            record.write_error = None;
        }
        let (reapplied, all_back) = self.reapply(s, entry, apply, in_effect)?;
        self.close_reverted(s, entry, &reapplied, !all_back, "undo", in_effect)
    }

    /// The end of a resolution (D.8 step 4): every record records its current value as
    /// `last_written`; `state_after_resolution` decides the state.
    fn close_resolution(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        reason: &str,
        apply: Option<&ApplyOptions>,
        in_effect: bool,
    ) -> Result<(), EngineError> {
        // Every value first: a resolution may be in flight here (`RevertPending`), and a value that
        // cannot be read must not leave it there (C3). The user decides again.
        let mut observed = Vec::with_capacity(entry.records.len());
        for index in 0..entry.records.len() {
            let record = &entry.records[index];
            if record.skipped == Some(SkipReason::DeviceRemoved) {
                observed.push(None);
                continue;
            }
            let read = || self.read_current(&record.target, &record.name);
            match read().or_else(|error| {
                if retryable(&error) {
                    read()
                } else {
                    Err(error)
                }
            }) {
                Ok(now) => observed.push(now),
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                // Still in `Conflict` (nothing was written): only this request fails.
                Err(error) if entry.state == OpState::Conflict => return Err(error.into()),
                Err(error) => {
                    s.warn(format!(
                        r"{}\{}: could not be read after the resolution ({error})",
                        record.key_path, record.name
                    ));
                    entry.records[index].write_error = Some(format!("could not be read: {error}"));
                    for record in &mut entry.records {
                        record.resolve_to = None;
                    }
                    return self.transition(s, entry, OpState::Conflict, reason);
                }
            }
        }
        let mut values = Vec::new();
        for (index, now) in observed.into_iter().enumerate() {
            let record = &mut entry.records[index];
            match now {
                None => {
                    record.skipped = Some(SkipReason::DeviceRemoved);
                    values.push(RegValue::Absent);
                }
                Some(value) => {
                    record.last_written = Some(value.clone());
                    values.push(value);
                }
            }
            record.resolve_to = None;
            record.conflict = None;
            record.write_error = None;
        }
        let to = state_after_resolution(entry, &values);
        entry.failure = match to {
            OpState::Failed => Some(FailureReason::ConflictKeptCurrent),
            _ => None,
        };
        let (reapplied, all_back) = match apply {
            Some(apply) if to != OpState::Failed => self.reapply(s, entry, apply, in_effect)?,
            _ => (Vec::new(), true),
        };
        let to = if to == OpState::Reverted && !all_back {
            OpState::RevertedPendingReboot
        } else {
            to
        };
        self.move_to(s, entry, to, reason)?;
        entry.apply_pending = if in_effect {
            self.close_pending(s, entry, &reapplied, false)?
        } else {
            None
        };
        self.commit(s, entry)?;
        if to == OpState::Confirmed && is_restore(entry) {
            self.cleanup_baselines(s, entry)?;
        }
        Ok(())
    }

    /// C.5 "確定した「導入前に戻す」の後始末": delete the baselines whose value is back, flush, and
    /// rewrite the recovery files (a failure there is only a warning).
    fn cleanup_baselines(
        &mut self,
        s: &mut Session<'_>,
        entry: &JournalEntry,
    ) -> Result<(), EngineError> {
        let mut deleted = false;
        for record in entry.records.iter().filter(|r| r.skipped.is_none()) {
            let key = record.key();
            let Some(baseline) = s.journal.baseline(&key).cloned() else {
                continue;
            };
            // Only while this restore is what MKLM last did to the value: a later operation keeps
            // relying on the baseline.
            if s.journal
                .latest_record(&key)
                .is_none_or(|(latest, _)| latest.op_id != entry.op_id)
            {
                continue;
            }
            // A value that cannot be read keeps its baseline (the clean-up runs again on the next
            // request); it must not stop the request (design review C3).
            match self.read_current(&record.target, &record.name) {
                Ok(Some(current)) if value_eq(&record.name, &current, &baseline.value) => {}
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                _ => continue,
            }
            let canonical = baseline.key.canonical();
            let slot = JournalSlot::Baseline(canonical.clone());
            match self.with_retry(|r| r.delete_journal(&slot)) {
                Ok(()) => {
                    deleted = true;
                    s.journal
                        .baselines
                        .retain(|b| !b.key.canonical().eq_ignore_ascii_case(&canonical));
                }
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                Err(error) => {
                    s.warn(format!("baselines were not cleaned up: {error}"));
                    break;
                }
            }
        }
        if !deleted {
            return Ok(());
        }
        match self.with_retry(|r| r.flush_journal()) {
            Ok(()) => {}
            Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
            Err(error) => s.warn(format!("baselines were not cleaned up: {error}")),
        }
        if !s.journal.baselines.is_empty() {
            let baselines = s.journal.baselines.clone();
            self.write_assets(s, &baselines, false)?;
        }
        Ok(())
    }

    /// D.5 step 7: close the superseded entries (one J each, one FJ).
    fn supersede(
        &mut self,
        s: &mut Session<'_>,
        by: &OpId,
        ids: &[OpId],
    ) -> Result<(), EngineError> {
        let mut closed: Vec<JournalEntry> = Vec::new();
        for id in ids {
            let Some(mut entry) = s.journal.entry(id).cloned() else {
                continue;
            };
            if !entry.state.is_open() || !entry.state.can_transition_to(OpState::Failed) {
                continue;
            }
            self.move_to(s, &mut entry, OpState::Failed, "superseded")?;
            entry.failure = Some(FailureReason::Superseded { by: by.clone() });
            entry.apply_pending = self.close_pending(s, &entry, &[], false)?;
            let json = entry.to_json().map_err(internal)?;
            let slot = JournalSlot::Op(entry.op_id.clone());
            self.with_retry(|r| r.write_journal(&slot, &json))?;
            s.put(&entry);
            closed.push(entry);
        }
        if closed.is_empty() {
            return Ok(());
        }
        self.with_retry(|r| r.flush_journal())?;
        for entry in closed {
            s.sink.event(&Event::StateChanged {
                op_id: entry.op_id.clone(),
                state: entry.state,
            });
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Recovery (C.7, D.7)
    // ---------------------------------------------------------------------------------------

    /// D.7 steps 2-3: every open entry, oldest first.
    fn recover_all(
        &mut self,
        s: &mut Session<'_>,
        result: &mut OperationResult,
    ) -> Result<(), EngineError> {
        let ids: Vec<OpId> = s
            .journal
            .open_entries()
            .iter()
            .map(|e| e.op_id.clone())
            .collect();
        for id in ids {
            let Some(mut entry) = s.journal.entry(&id).cloned() else {
                continue;
            };
            if !entry.state.is_open() {
                continue;
            }
            self.refresh(s)?;
            let context = RecoveryContext {
                current_boot: s.boot,
                inv_ps2: check_inv_ps2(&s.global, &s.keyboards).err(),
            };
            let from = entry.state;
            let current = match self.observe_values(&entry)? {
                Ok(current) => current,
                Err(unobservable) => {
                    // Design review C3 / I7: an unreadable value ends in `Conflict` once.
                    if self.unobservable(s, &mut entry, &unobservable, "recover:unreadable")? {
                        result.recovered.push(RecoveredOp {
                            op_id: id,
                            from,
                            to: entry.state,
                            decision: "conflict".to_string(),
                        });
                    }
                    continue;
                }
            };
            let decision = decide_recovery_with_removed(&entry, &current, &context);
            if let Some(label) = self.execute(s, &mut entry, decision, &current)? {
                result.recovered.push(RecoveredOp {
                    op_id: id,
                    from,
                    to: entry.state,
                    decision: label.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Before recovery acts on the values an abandoned entry left, one `flush_target` makes them
    /// durable (`RegFlushKey` flushes the whole SYSTEM hive). The process may have died between
    /// a T and its FT, and recovery must not journal a state that names those values, nor write
    /// a later step after them, while they may still be lost on a power failure (C.5: "エントリは、
    /// 永続化された値より先の状態を名乗らない"; I.2). Any target of the entry will do. A failure is a
    /// warning only: the lazy writer flushes the hive anyway, and failing here would leave the
    /// entry in flight (C3).
    fn flush_inherited(
        &mut self,
        s: &mut Session<'_>,
        entry: &JournalEntry,
    ) -> Result<(), EngineError> {
        let mut targets: Vec<WriteTarget> = Vec::new();
        for record in &entry.records {
            if !targets.contains(&record.target) {
                targets.push(record.target.clone());
            }
        }
        let mut failure = None;
        for target in targets {
            match self.with_retry(|r| r.flush_target(&target)) {
                Ok(()) => return Ok(()),
                Err(BackendError::Crashed) => return Err(BackendError::Crashed.into()),
                Err(BackendError::DeviceRemoved { .. }) => {}
                Err(error) => failure = Some(error),
            }
        }
        if let Some(error) = failure {
            s.warn(format!(
                "{}: the values found could not be flushed to disk: {error}",
                entry.op_id
            ));
        }
        Ok(())
    }

    /// Carries out one recovery decision. Returns its label, `None` for `Leave`.
    fn execute(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        decision: RecoveryDecision,
        current: &[Option<RegValue>],
    ) -> Result<Option<&'static str>, EngineError> {
        if !matches!(decision, RecoveryDecision::Leave { .. }) {
            self.flush_inherited(s, entry)?;
        }
        match decision {
            RecoveryDecision::Leave { .. } => Ok(None),
            RecoveryDecision::MarkNothingWritten => {
                entry.failure = Some(FailureReason::NothingWritten);
                self.move_to(s, entry, OpState::Failed, "recover:nothing-written")?;
                entry.apply_pending = None;
                self.commit(s, entry)?;
                Ok(Some("mark-nothing-written"))
            }
            RecoveryDecision::RollForward { to } => {
                if entry.state == to {
                    // A restore found counting down: drop the countdown, the user decides (C14).
                    entry.countdown = None;
                    entry.take_over(self.host.now(), s.boot, s.process, "recover:drop-countdown");
                    entry.apply_pending = self.close_pending(s, entry, &[], false)?;
                    self.commit(s, entry)?;
                } else {
                    for (record, value) in entry.records.iter_mut().zip(current) {
                        if value.is_some() {
                            record.last_written = Some(record.intended.clone());
                        }
                    }
                    self.move_to(s, entry, to, "recover:roll-forward")?;
                    entry.apply_pending = self.close_pending(s, entry, &[], false)?;
                    self.commit(s, entry)?;
                }
                Ok(Some("roll-forward"))
            }
            RecoveryDecision::CompleteForward { to } => {
                self.complete_forward(s, entry, to, current)?;
                Ok(Some("complete-forward"))
            }
            RecoveryDecision::RollBack { reason } => {
                let label = "recover:roll-back";
                self.note_boot_pending(s, entry, current, &|r| Some(r.before.clone()));
                match self.revert_values(
                    s,
                    entry,
                    RevertMode::Rollback,
                    Some(reason.failure()),
                    label,
                ) {
                    Ok(true) => self.close_reverted(s, entry, &[], false, label, true)?,
                    Ok(false) => self.transition(s, entry, OpState::Conflict, label)?,
                    Err(EngineError::Restore(error)) => {
                        self.conflict_from_plan(s, entry, error, label)?;
                    }
                    Err(error) => return Err(error),
                }
                Ok(Some("roll-back"))
            }
            RecoveryDecision::ContinueRevert => {
                self.continue_revert(s, entry, current)?;
                Ok(Some("continue-revert"))
            }
            RecoveryDecision::RebootObserved { to } => {
                self.move_to(s, entry, to, "recover:reboot-observed")?;
                entry.apply_pending = if to == OpState::Reverted {
                    None
                } else {
                    self.close_pending(s, entry, &[], false)?
                };
                self.commit(s, entry)?;
                Ok(Some("reboot-observed"))
            }
            RecoveryDecision::Conflict { records, inv_ps2 } => {
                for index in records {
                    if let (Some(record), Some(Some(value))) =
                        (entry.records.get_mut(index), current.get(index))
                    {
                        record.conflict = Some(value.clone());
                    }
                }
                if inv_ps2.is_some() {
                    s.inv_ps2 = inv_ps2;
                }
                self.transition(s, entry, OpState::Conflict, "recover:conflict")?;
                Ok(Some("conflict"))
            }
        }
    }

    /// C.7 `CompleteForward` (C14): a restore to baseline is written through, never undone.
    fn complete_forward(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        to: OpState,
        current: &[Option<RegValue>],
    ) -> Result<(), EngineError> {
        let label = "recover:complete-forward";
        let silent = matches!(entry.kind, OpKind::RestoreBaseline { silent: true, .. });
        if to == OpState::AwaitingConfirm && entry.boot_id != s.boot {
            // `AwaitingConfirm` was chosen because the boot-time values were read at a later boot
            // than the one that wrote them. Moving `boot_id` to this boot would make a recovery
            // interrupted from here on choose `PendingReboot` instead (I4), so only the owner
            // changes.
            self.take_over_same_boot(s, entry, "recover:take-over")?;
        } else {
            self.take_over(s, entry, "recover:take-over")?;
        }
        if let OpKind::RestoreBaseline { supersedes, .. } = &entry.kind {
            let ids = supersedes.clone();
            self.supersede(s, &entry.op_id.clone(), &ids)?;
        }
        let mut subset = Vec::new();
        for (index, record) in entry.records.iter_mut().enumerate() {
            let Some(Some(value)) = current.get(index) else {
                if record.skipped.is_none() {
                    record.skipped = Some(SkipReason::DeviceRemoved);
                }
                continue;
            };
            match observe(record, value) {
                Observation::AtBefore => subset.push(index),
                Observation::AtIntended | Observation::Unchanged => {
                    record.last_written = Some(record.intended.clone());
                }
                Observation::Elsewhere if silent => {
                    record.skipped = Some(SkipReason::ConflictSkipped);
                    record.conflict = Some(value.clone());
                }
                Observation::Elsewhere => {}
            }
        }
        if !subset.is_empty() {
            self.refresh(s)?;
            let plan = match self.plan_subset(
                s,
                &entry.records,
                &subset,
                RestoreTo::Baseline,
                &expect_before,
            ) {
                Ok(plan) => plan,
                Err(error) => return self.conflict_from_plan(s, entry, error, label),
            };
            if let Err(error) = check_skipped_boot_time(&plan, &entry.records) {
                return self.conflict_from_plan(s, entry, error, label);
            }
            if !self.run_restore(s, entry, &plan, &subset, silent)? {
                return self.transition(s, entry, OpState::Conflict, label);
            }
        }
        if entry.state == OpState::Planned {
            self.transition(s, entry, OpState::Written, label)?;
        }
        if to == OpState::Confirmed {
            return self.close_confirmed(s, entry, &[], label);
        }
        self.move_to(s, entry, to, label)?;
        // Recovery may have written values in this boot and never resets: judge `apply_pending`
        // as of this boot whatever `boot_id` says.
        entry.apply_pending = self.close_pending(s, entry, &[], true)?;
        self.commit(s, entry)
    }

    /// C.7 `ContinueRevert`: whatever `revert_mode` says, from the values as they are.
    fn continue_revert(
        &mut self,
        s: &mut Session<'_>,
        entry: &mut JournalEntry,
        current: &[Option<RegValue>],
    ) -> Result<(), EngineError> {
        let label = "recover:continue-revert";
        let resolution = entry.revert_mode == Some(RevertMode::Resolution);
        self.note_boot_pending(s, entry, current, &|r| {
            if resolution {
                r.resolve_to.clone()
            } else {
                Some(r.before.clone())
            }
        });
        self.take_over(s, entry, "recover:take-over")?;
        self.refresh(s)?;
        match entry.revert_mode.unwrap_or(RevertMode::Rollback) {
            RevertMode::Resolution => {
                // Only records whose value differs from the user's choice; KeepCurrent ones are
                // already there (C5).
                let subset: Vec<usize> = (0..entry.records.len())
                    .filter(|&i| {
                        let record = &entry.records[i];
                        match (&record.resolve_to, current.get(i)) {
                            (Some(target), Some(Some(value))) => {
                                !value_eq(&record.name, value, target)
                            }
                            _ => false,
                        }
                    })
                    .collect();
                let plan = match self.plan_subset(
                    s,
                    &entry.records,
                    &subset,
                    RestoreTo::Resolution,
                    &expect_seen,
                ) {
                    Ok(plan) => plan,
                    Err(error) => return self.conflict_from_plan(s, entry, error, label),
                };
                if self.run_restore(s, entry, &plan, &subset, false)? {
                    self.close_resolution(s, entry, label, None, true)
                } else {
                    for record in &mut entry.records {
                        record.resolve_to = None;
                    }
                    self.transition(s, entry, OpState::Conflict, label)
                }
            }
            mode => {
                // An undo of a `Conflict` entry (D.10) writes only the values it can and leaves
                // the ones changed outside MKLM alone, then goes back to `Conflict`. Recovery
                // continues it the same way (I4): the values that are neither back at `before`
                // nor at what MKLM wrote are left out, as `undo_conflict` left them out.
                let undoing_conflict = mode == RevertMode::Revert && reverts_a_conflict(entry);
                let mut left_out = false;
                let mut subset: Vec<usize> = Vec::new();
                for (i, record) in entry.records.iter_mut().enumerate() {
                    if record.skipped.is_some() {
                        continue;
                    }
                    if undoing_conflict
                        && let Some(Some(value)) = current.get(i)
                        && !value_eq(&record.name, value, &record.before)
                        && !expect_matches(&expect_written(record), &record.name, value)
                    {
                        record.conflict = Some(value.clone());
                        left_out = true;
                        continue;
                    }
                    subset.push(i);
                }
                let plan = match self.plan_subset(
                    s,
                    &entry.records,
                    &subset,
                    RestoreTo::Before,
                    &expect_written,
                ) {
                    Ok(plan) => plan,
                    Err(error) => return self.conflict_from_plan(s, entry, error, label),
                };
                if !self.run_restore(s, entry, &plan, &subset, false)? || left_out {
                    return self.transition(s, entry, OpState::Conflict, label);
                }
                if mode == RevertMode::Rollback && entry.failure.is_none() {
                    entry.failure = Some(FailureReason::Interrupted);
                }
                self.close_reverted(s, entry, &[], false, label, true)
            }
        }
    }

    /// D.7 step 4: with the caller's permission (C1, C9), reset keyboards whose recovered values
    /// are not in effect, one at a time, and clear them from `apply_pending` once Raw Input
    /// reports the stored type. `Reconnect` entries count too: recovery never resets on its own,
    /// so the values it put back are recorded as at least `Reconnect`.
    fn reset_pending(
        &mut self,
        s: &mut Session<'_>,
        apply: &ApplyOptions,
    ) -> Result<(), EngineError> {
        if !(apply.allow_live_reset && apply.other_input_available) {
            return Ok(());
        }
        self.refresh(s)?;
        let mut reset: Vec<String> = Vec::new();
        let entries = s.journal.entries.clone();
        for mut entry in entries {
            let Some(pending) = entry.apply_pending.clone() else {
                continue;
            };
            if pending.since != s.boot || pending.action == PendingAction::RestartPc {
                continue;
            }
            let ids: Vec<String> = pending
                .instance_ids
                .iter()
                .filter(|id| {
                    s.keyboard(id)
                        .is_some_and(|kb| kb.present && live_reset_bans(kb, false).is_empty())
                })
                .filter(|id| !reset.iter().any(|r| r.eq_ignore_ascii_case(id)))
                .cloned()
                .collect();
            let (reapplied, _, _) = self.reset_keyboards(s, &ids, false);
            reset.extend(reapplied);
            let remaining: Vec<String> = pending
                .instance_ids
                .iter()
                .filter(|id| {
                    let reported = self.devices.reported_type(id).ok().flatten();
                    !(reset.iter().any(|r| r.eq_ignore_ascii_case(id))
                        && reported.is_some()
                        && reported == s.expected_type(id))
                })
                .cloned()
                .collect();
            if remaining.len() != pending.instance_ids.len() {
                entry.apply_pending = (!remaining.is_empty()).then_some(ApplyPending {
                    instance_ids: remaining,
                    ..pending
                });
                entry.updated_at = self.host.now();
                self.save(s, &entry)?;
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Results
    // ---------------------------------------------------------------------------------------

    fn entry_result(&self, s: &Session<'_>, entry: &JournalEntry) -> OperationResult {
        let pending_action = match entry.state {
            OpState::PendingReboot | OpState::RevertedPendingReboot => {
                Some(PendingAction::RestartPc)
            }
            _ => entry.apply_pending.as_ref().map(|p| p.action),
        };
        OperationResult {
            op_id: Some(entry.op_id.clone()),
            outcome: outcome_of(entry.state),
            failure: entry.failure.clone(),
            pending_action,
            conflicts: if entry.state == OpState::Conflict {
                self.conflict_infos(entry)
            } else {
                Vec::new()
            },
            inv_ps2_violation: s.inv_ps2.clone(),
            recovered: Vec::new(),
            warnings: s.warnings.clone(),
        }
    }

    fn with_warnings(result: OperationResult, s: &Session<'_>) -> OperationResult {
        OperationResult {
            warnings: s.warnings.clone(),
            ..result
        }
    }

    /// The values of a `Conflict` entry that were not what MKLM expected, or could not be written.
    fn conflict_infos(&self, entry: &JournalEntry) -> Vec<ConflictInfo> {
        entry
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.conflict.is_some() || r.write_error.is_some())
            .map(|(index, r)| ConflictInfo {
                op_id: entry.op_id.clone(),
                record: index,
                key_path: r.key_path.clone(),
                name: r.name.clone(),
                baseline: r.baseline.clone(),
                before: r.before.clone(),
                intended: r.intended.clone(),
                last_written: r.last_written.clone(),
                current: self
                    .registry
                    .read_value(&r.target, &r.name)
                    .ok()
                    .or_else(|| r.conflict.clone())
                    .unwrap_or(RegValue::Absent),
                write_error: r.write_error.clone(),
            })
            .collect()
    }

    /// The result of `recover` / `undo`: what was done, the conflicts left, and the heaviest
    /// action the user still has to take.
    fn finish_recovered(&self, s: &Session<'_>, mut result: OperationResult) -> OperationResult {
        let mut pending: Option<PendingAction> = None;
        for entry in &s.journal.entries {
            let action = match entry.state {
                OpState::PendingReboot => Some(PendingAction::RestartPc),
                OpState::RevertedPendingReboot if entry.boot_id == s.boot => {
                    Some(PendingAction::RestartPc)
                }
                _ => entry
                    .apply_pending
                    .as_ref()
                    .filter(|p| p.since == s.boot)
                    .map(|p| p.action),
            };
            pending = pending.max(action);
            if entry.state == OpState::Conflict {
                result.conflicts.extend(self.conflict_infos(entry));
            }
        }
        result.pending_action = pending;
        result.inv_ps2_violation = s.inv_ps2.clone();
        result.warnings = s.warnings.clone();
        result
    }

    // ---------------------------------------------------------------------------------------
    // Keyboards
    // ---------------------------------------------------------------------------------------

    /// Instance IDs of the HID keyboards whose running type an entry changes (records of values
    /// that are not boot-time values), in record order. Values their keyboard's driver does not
    /// read change nothing (a cleanup's, design m3 A.5): a cleanup lists none, and such records
    /// of other entries (a restore to baseline that puts them back) are left out, so that no
    /// keyboard is ever reset or waited for because of them.
    fn hid_keyboards(s: &Session<'_>, entry: &JournalEntry) -> Vec<String> {
        if is_cleanup(entry) {
            return Vec::new();
        }
        let read: Vec<ValueRecord> = entry
            .records
            .iter()
            .filter(|r| !is_unread_value(&s.keyboards, &r.target, &r.name))
            .cloned()
            .collect();
        Self::hid_ids(&read)
    }

    fn hid_ids(records: &[ValueRecord]) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for record in records
            .iter()
            .filter(|r| r.skipped.is_none() && !r.is_boot_time())
        {
            if let WriteTarget::Device { instance_id } = &record.target
                && !ids.iter().any(|id| id.eq_ignore_ascii_case(instance_id))
            {
                ids.push(instance_id.clone());
            }
        }
        ids
    }

    /// What each keyboard should report and type once the change is in effect (for the
    /// confirmation UI): `only` lists the keyboards to show, else every kbdhid and i8042prt one.
    fn expected_keyboards(
        &self,
        s: &Session<'_>,
        after_keyboards: &[KeyboardDevice],
        after_global: &GlobalSettings,
        only: Option<&[String]>,
    ) -> Vec<ExpectedKeyboard> {
        let before = assess(&snapshot(s.keyboards.clone(), s.global.clone()));
        let after = assess(&snapshot(after_keyboards.to_vec(), after_global.clone()));
        after
            .keyboards
            .iter()
            .zip(after_keyboards)
            .filter(|(_, kb)| match only {
                Some(ids) => ids
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&kb.instance_id)),
                None => matches!(kb.driver, KeyboardDriver::Kbdhid | KeyboardDriver::I8042prt),
            })
            .map(|(assessed, kb)| {
                let table_after = assessed.after_restart.as_ref().map(|l| l.table.clone());
                let table_before = before
                    .keyboards
                    .iter()
                    .find(|b| b.instance_id.eq_ignore_ascii_case(&kb.instance_id))
                    .and_then(|b| b.after_restart.as_ref().map(|l| l.table.clone()));
                ExpectedKeyboard {
                    instance_id: kb.instance_id.clone(),
                    display_name: kb.display_name.clone(),
                    expected_type: assessed.predicted_type,
                    changes: table_after != table_before,
                    layout_after: table_after,
                }
            })
            .collect()
    }
}
