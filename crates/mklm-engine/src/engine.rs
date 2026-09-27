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
//! The engine takes its own parameter types ([`crate::params`]) and reports with
//! `mklm_core::report`; it does not know the pipe protocol (design review S5).

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(unused_variables, dead_code)]

use std::time::Duration;

use mklm_core::{ApplyOptions, Journal, OpId, OperationResult};

use crate::backend::RegistryBackend;
use crate::device::DeviceController;
use crate::error::EngineError;
use crate::host::Host;
use crate::params::{MigrateParams, ResolveParams, RestoreBaselineParams, SetLayoutParams};
use crate::sink::EventSink;

/// Tunables. The defaults are the product values; tests shorten nothing because the engine counts
/// countdown ticks and the fake host's monotonic clock only moves when the test moves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// Keep-or-revert countdown after a live reset (plan 3.5: 15-20 s).
    pub countdown_seconds: u32,
    /// Extra monotonic time a countdown may take beyond `countdown_seconds` before it is treated
    /// as expired whatever the sink does (design review C18).
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
            countdown_seconds: 20,
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
        todo!("M2")
    }

    /// Section D.3: fixed → per-keyboard mode (plan 1.3); always ends in `PendingReboot`.
    pub fn migrate(
        &mut self,
        params: &MigrateParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        todo!("M2")
    }

    /// Section D.4: restore `before` of the latest operation on its values (compare-and-swap).
    /// Refuses a `Confirmed` restore-to-baseline (design review C6).
    pub fn revert(
        &mut self,
        op_id: &OpId,
        apply: &ApplyOptions,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        todo!("M2")
    }

    /// Section D.6: keep an operation in `AwaitingConfirm` (or `PendingReboot` seen from a new
    /// boot). Checks Raw Input; keeps with `apply_pending` when a keyboard does not report its
    /// stored type yet (design review C1).
    pub fn confirm(
        &mut self,
        op_id: &OpId,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        todo!("M2")
    }

    /// Section D.5: "MKLM 導入前に戻す" as a new operation that writes the baselines. Closes open,
    /// not in-flight entries as `Failed(Superseded)` instead of refusing (design review C7).
    pub fn restore_baseline(
        &mut self,
        params: &RestoreBaselineParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        todo!("M2")
    }

    /// Section D.7: act on every eligible entry (`mklm_core::decide_recovery`); with `apply`
    /// allowing it, reset keyboards whose recovered values are not in effect.
    pub fn recover(
        &mut self,
        apply: &ApplyOptions,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        todo!("M2")
    }

    /// Section D.10: undo every open, not in-flight entry, newest first (design review C7).
    /// Recovers in-flight entries first, like `recover`.
    pub fn undo_open(
        &mut self,
        apply: &ApplyOptions,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        todo!("M2")
    }

    /// Section D.8: apply the user's per-value choices to an operation in `Conflict`.
    pub fn resolve_conflict(
        &mut self,
        params: &ResolveParams,
        sink: &mut dyn EventSink,
    ) -> Result<OperationResult, EngineError> {
        todo!("M2")
    }

    /// The parsed journal, without the lock (read-only; what `mklm-cli journal` shows).
    pub fn read_journal(&self) -> Result<Journal, EngineError> {
        todo!("M2")
    }
}
