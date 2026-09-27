//! Planned writes of the operations a user starts (plan 1.3, 3.4) and how they take effect
//! (plan 1.4). Pure: the engine feeds the result to [`crate::check_plan`], which enforces the
//! allowlist and INV-PS2 and fixes the write order.

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use serde::{Deserialize, Serialize};

use crate::allowlist::{CheckedPlan, PlanError, PlannedWrite};
use crate::journal::LayoutChoice;
use crate::layout::PendingAction;
use crate::model::{GlobalSettings, KeyboardDevice, Layout};
use crate::report::ApplyOptions;

/// Device writes: `(instance ID, writes)` per keyboard, the input of [`crate::check_plan`].
pub type DeviceWrites = Vec<(String, Vec<PlannedWrite>)>;

/// An operation the rules refuse before any allowlist check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OperationError {
    #[error("{instance_id}: no such keyboard")]
    UnknownKeyboard { instance_id: String },
    /// Fixed mode ignores per-device values (M0 #2a): assigning a layout needs a migration first.
    #[error("the PC is in fixed mode ({fixed:?}); migrate to per-keyboard mode first")]
    MigrationRequired { fixed: Option<Layout> },
    #[error("the PC is already in per-keyboard mode")]
    NotFixedMode,
    /// The global values do not say which layout the fixed mode gives (see `ps2_pin_layout`).
    #[error("the global values are inconsistent; fix them before migrating")]
    InconsistentGlobal,
    #[error("{instance_id}: \"follow the standard\" is only possible for HID keyboards")]
    StandardNotAllowed { instance_id: String },
    /// Plan 1.5: the `LayerDriver JPN` value to write names a DLL that is not in System32. Checked
    /// by the engine through `Host::system32_file_exists` before `check_plan` (design review S8).
    #[error("{dll} is not in System32")]
    LayerDriverMissing { dll: String },
    #[error(transparent)]
    Plan(#[from] PlanError),
}

/// A new operation as planned from one inventory: the checked writes and how they take effect.
///
/// The unelevated dry run (CLI, GUI) and the engine inside the helper both call
/// [`plan_set_layout`] / [`plan_migration`], so that what the user approves and what is written
/// come from one function (design review S6). The engine compares `checked.steps` and `apply` with
/// the approved [`crate::ExpectedPlan`] and writes nothing when either differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationPlan {
    /// Allowlist, INV-PS2 and write order ([`crate::check_plan`]).
    pub checked: CheckedPlan,
    /// Every devnode the operation writes, in plan order.
    pub instance_ids: Vec<String>,
    /// How the change takes effect ([`apply_method`]).
    pub apply: PendingAction,
    /// True when the target counts as the only usable keyboard (plan 1.4).
    pub only_usable_keyboard: bool,
}

/// Plans "set layout" (design D.2 steps 2, 3 and 5): [`set_layout_writes`], [`crate::check_plan`],
/// then [`apply_method`], with `only_usable_keyboard` true when `options.other_input_available` is
/// false or no other keyboard outside the target's container is present, `DN_STARTED` and not
/// virtual.
pub fn plan_set_layout(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    instance_id: &str,
    choice: LayoutChoice,
    options: &ApplyOptions,
) -> Result<OperationPlan, OperationError> {
    todo!("M2")
}

/// Plans the migration (design D.3 steps 2 and 3): [`migration_writes`], [`crate::check_plan`];
/// `apply` is always [`PendingAction::RestartPc`].
pub fn plan_migration(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    standard: Layout,
    assignments: &[(String, LayoutChoice)],
) -> Result<OperationPlan, OperationError> {
    todo!("M2")
}

/// The keyboards one assignment covers (plan 3.4: every kbdhid collection of the physical device):
/// all kbdhid keyboards sharing the requested keyboard's external, known container ID; just the
/// keyboard itself for i8042prt, the internal container or an unknown container. Phantoms of the
/// same container are included (their values apply when they reconnect).
pub fn physical_device_members<'a>(
    keyboards: &'a [KeyboardDevice],
    instance_id: &str,
) -> Result<Vec<&'a KeyboardDevice>, OperationError> {
    todo!("M2")
}

/// Device writes of "set layout" in per-keyboard mode. Refuses fixed mode
/// ([`OperationError::MigrationRequired`]).
pub fn set_layout_writes(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    instance_id: &str,
    choice: LayoutChoice,
) -> Result<DeviceWrites, OperationError> {
    todo!("M2")
}

/// Device and global writes of the migration (plan 1.3):
/// - every i8042prt keyboard, phantoms included, is pinned to its assignment if the user gave one,
///   else to [`crate::ps2_pin_layout`] (the layout fixed mode gives it now);
/// - each HID assignment covers its [`physical_device_members`];
/// - the global type/subtype are deleted and the standard is set to `standard` (only the values
///   that differ are written).
///
/// Refuses per-keyboard mode ([`OperationError::NotFixedMode`]) and inconsistent globals.
pub fn migration_writes(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    standard: Layout,
    assignments: &[(String, LayoutChoice)],
) -> Result<(DeviceWrites, Vec<PlannedWrite>), OperationError> {
    todo!("M2")
}

/// How a change reaches the drivers (the heaviest over all targets):
/// - [`PendingAction::RestartPc`] when global values change, for any keyboard whose
///   [`crate::device_apply_action`] is a restart, and for the only usable keyboard (plan 1.4:
///   warn and recommend a PC restart), and when a live-reset candidate is not started or has a
///   problem;
/// - [`PendingAction::Reconnect`] for unproven transports (BLE/BT), for keyboards that are not
///   present, and when `allow_live_reset` is false;
/// - [`PendingAction::ResetKeyboard`] when every target passes [`crate::live_reset_bans`].
pub fn apply_method(
    targets: &[&KeyboardDevice],
    writes_global: bool,
    only_usable_keyboard: bool,
    allow_live_reset: bool,
) -> PendingAction {
    todo!("M2")
}
