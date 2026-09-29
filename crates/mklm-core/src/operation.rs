//! Planned writes of the operations a user starts (plan 1.3, 3.4) and how they take effect
//! (plan 1.4). Pure: the engine feeds the result to [`crate::check_plan`], which enforces the
//! allowlist and INV-PS2 and fixes the write order.

use serde::{Deserialize, Serialize};

use crate::allowlist::{
    AllowlistError, CheckedPlan, PlanError, PlannedWrite, WriteTarget, check_plan,
    device_layout_writes, is_read_only_device, ps2_pin_layout, standard_layout_writes,
    stored_standard,
};
use crate::device::device_apply_action;
use crate::journal::{LayoutChoice, RegValue, ValueKey, value_eq};
use crate::layout::PendingAction;
use crate::model::{
    GlobalMode, GlobalSettings, KeyboardDevice, KeyboardDriver, Layout, Transport, value_names,
};
use crate::report::ApplyOptions;
use crate::safety::{DN_STARTED, live_reset_bans};

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
    /// Design standard-layout B.4: the stored standard is not one MKLM writes
    /// ([`crate::stored_standard`] is `None`: values missing, `kbd106n.dll`, `kbdnec.dll`, or an
    /// identifier that does not match), so MKLM cannot tell what the keyboards that follow it
    /// type now. The stored values, for the technical details.
    #[error(
        "the standard layout's values are not in a form MKLM knows (LayerDriver JPN {layer_driver:?}, \
         OverrideKeyboardIdentifier {identifier:?})"
    )]
    UnknownStandard {
        layer_driver: Option<String>,
        identifier: Option<String>,
    },
    /// Design standard-layout B.3: a physical keyboard given to follow the new standard has no
    /// collection that follows the standard now (assigned, PS/2, Remote Desktop, or one MKLM
    /// cannot write).
    #[error("{instance_id}: does not follow the standard now")]
    NotFollowingStandard { instance_id: String },
    /// Design standard-layout B.3 (6): a writable kbdhid or i8042prt keyboard stores only one of
    /// the two values of its pair, so what it types now is unknown.
    #[error("only one of the two values that set a layout is stored for: {}", instance_ids.join(", "))]
    IncompleteValues { instance_ids: Vec<String> },
    /// Design standard-layout B.3 (7): an i8042prt keyboard is pinned to a type without a table of
    /// its own (7/0, 0x51/0, …), so it would change with the standard.
    #[error("PS/2 keyboards pinned to a type that sets no layout: {}", instance_ids.join(", "))]
    Ps2WithoutTable { instance_ids: Vec<String> },
    #[error(transparent)]
    Plan(#[from] PlanError),
}

/// An operation as planned from one inventory: the checked writes and how they take effect.
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

/// Devnodes written by `checked`, in step order.
fn written_devices(checked: &CheckedPlan) -> Vec<String> {
    checked
        .steps
        .iter()
        .filter_map(|step| match &step.target {
            WriteTarget::Device { instance_id } => Some(instance_id.clone()),
            WriteTarget::Global => None,
        })
        .collect()
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
    let writes = set_layout_writes(keyboards, global, instance_id, choice)?;
    let checked = check_plan(keyboards, global, &writes, &[])?;
    let members = physical_device_members(keyboards, instance_id)?;
    let only_usable_keyboard =
        !options.other_input_available || !other_keyboard_usable(keyboards, &members);
    let apply = apply_method(
        &members,
        false,
        only_usable_keyboard,
        options.allow_live_reset,
    );
    Ok(OperationPlan {
        instance_ids: written_devices(&checked),
        checked,
        apply,
        only_usable_keyboard,
    })
}

/// True when some keyboard outside the target's physical device (its members and its external
/// container) is connected, `DN_STARTED` and not virtual.
fn other_keyboard_usable(keyboards: &[KeyboardDevice], members: &[&KeyboardDevice]) -> bool {
    let container = members.first().and_then(|kb| external_container(kb));
    keyboards.iter().any(|kb| {
        let is_member = members
            .iter()
            .any(|m| m.instance_id.eq_ignore_ascii_case(&kb.instance_id));
        let same_container = container.is_some_and(|c| {
            external_container(kb).is_some_and(|other| other.eq_ignore_ascii_case(c))
        });
        !is_member
            && !same_container
            && kb.present
            && kb.dev_node_status.is_some_and(|s| s & DN_STARTED != 0)
            && kb.transport != Transport::Virtual
    })
}

/// The container ID when it identifies one external device (known, not the built-in container).
fn external_container(kb: &KeyboardDevice) -> Option<&str> {
    if kb.in_internal_container() {
        None
    } else {
        kb.known_container_id()
    }
}

/// Plans the migration (design D.3 steps 2 and 3): [`migration_writes`], [`crate::check_plan`];
/// `apply` is always [`PendingAction::RestartPc`].
pub fn plan_migration(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    standard: Layout,
    assignments: &[(String, LayoutChoice)],
) -> Result<OperationPlan, OperationError> {
    let (device_writes, global_writes) =
        migration_writes(keyboards, global, standard, assignments)?;
    let checked = check_plan(keyboards, global, &device_writes, &global_writes)?;
    Ok(OperationPlan {
        instance_ids: written_devices(&checked),
        checked,
        apply: PendingAction::RestartPc,
        only_usable_keyboard: false,
    })
}

/// The keyboards one assignment covers (plan 3.4: every kbdhid collection of the physical device):
/// all kbdhid keyboards sharing the requested keyboard's external, known container ID; just the
/// keyboard itself for i8042prt, the internal container or an unknown container. Phantoms of the
/// same container are included (their values apply when they reconnect).
pub fn physical_device_members<'a>(
    keyboards: &'a [KeyboardDevice],
    instance_id: &str,
) -> Result<Vec<&'a KeyboardDevice>, OperationError> {
    let requested = keyboards
        .iter()
        .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
        .ok_or_else(|| OperationError::UnknownKeyboard {
            instance_id: instance_id.to_string(),
        })?;
    let container = match (&requested.driver, external_container(requested)) {
        (KeyboardDriver::Kbdhid, Some(container)) => container,
        _ => return Ok(vec![requested]),
    };
    Ok(keyboards
        .iter()
        .filter(|kb| {
            kb.driver == KeyboardDriver::Kbdhid
                && external_container(kb).is_some_and(|c| c.eq_ignore_ascii_case(container))
        })
        .collect())
}

/// The device writes that give `kb` the layout of `choice`.
fn choice_writes(
    kb: &KeyboardDevice,
    choice: LayoutChoice,
) -> Result<Vec<PlannedWrite>, OperationError> {
    if let Some(writes) = device_layout_writes(&kb.driver, choice.layout()) {
        return Ok(writes);
    }
    match &kb.driver {
        KeyboardDriver::I8042prt => Err(OperationError::StandardNotAllowed {
            instance_id: kb.instance_id.clone(),
        }),
        KeyboardDriver::Other(service) => Err(PlanError::Device {
            instance_id: kb.instance_id.clone(),
            error: AllowlistError::ReadOnlyDriver {
                service: service.clone(),
            },
        }
        .into()),
        // kbdhid takes every choice.
        KeyboardDriver::Kbdhid => Ok(Vec::new()),
    }
}

/// Device writes of "set layout" in per-keyboard mode. Refuses fixed mode
/// ([`OperationError::MigrationRequired`]).
pub fn set_layout_writes(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    instance_id: &str,
    choice: LayoutChoice,
) -> Result<DeviceWrites, OperationError> {
    let members = physical_device_members(keyboards, instance_id)?;
    if global.mode() == GlobalMode::Fixed {
        return Err(OperationError::MigrationRequired {
            fixed: ps2_pin_layout(global),
        });
    }
    members
        .into_iter()
        .map(|kb| Ok((kb.instance_id.clone(), choice_writes(kb, choice)?)))
        .collect()
}

/// Device and global writes of the migration (plan 1.3):
/// - every i8042prt keyboard, phantoms included, is pinned to its assignment if the user gave one,
///   else to [`crate::ps2_pin_layout`] (the layout fixed mode gives it now);
/// - each HID assignment covers its [`physical_device_members`];
/// - the global type/subtype are deleted and the standard is set to `standard` (only the values
///   that differ are written).
///
/// Refuses per-keyboard mode ([`OperationError::NotFixedMode`]) and inconsistent globals.
///
/// Device writes list the i8042prt keyboards first (in inventory order), then the HID assignments
/// in the order given. An i8042prt keyboard assigned twice is refused
/// ([`PlanError::DuplicateKeyboard`]); overlapping HID assignments are refused by `check_plan`.
pub fn migration_writes(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    standard: Layout,
    assignments: &[(String, LayoutChoice)],
) -> Result<(DeviceWrites, Vec<PlannedWrite>), OperationError> {
    if global.mode() != GlobalMode::Fixed {
        return Err(OperationError::NotFixedMode);
    }
    let pin = ps2_pin_layout(global).ok_or(OperationError::InconsistentGlobal)?;

    // Resolve every assignment first, so that an unknown keyboard is reported as such.
    let mut ps2_assigned: Vec<(&KeyboardDevice, LayoutChoice)> = Vec::new();
    let mut hid_writes: DeviceWrites = Vec::new();
    for (instance_id, choice) in assignments {
        let requested = keyboards
            .iter()
            .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
            .ok_or_else(|| OperationError::UnknownKeyboard {
                instance_id: instance_id.clone(),
            })?;
        if requested.driver == KeyboardDriver::I8042prt {
            if ps2_assigned
                .iter()
                .any(|(kb, _)| kb.instance_id == requested.instance_id)
            {
                return Err(PlanError::DuplicateKeyboard {
                    instance_id: requested.instance_id.clone(),
                }
                .into());
            }
            // Validates the choice (no "standard" for PS/2) before anything else is planned.
            choice_writes(requested, *choice)?;
            ps2_assigned.push((requested, *choice));
        } else {
            for kb in physical_device_members(keyboards, instance_id)? {
                hid_writes.push((kb.instance_id.clone(), choice_writes(kb, *choice)?));
            }
        }
    }

    // Pins and assignments of the i8042prt keyboards go into one write each (check_plan refuses a
    // keyboard planned twice).
    let mut device_writes: DeviceWrites = Vec::new();
    for kb in keyboards
        .iter()
        .filter(|kb| kb.driver == KeyboardDriver::I8042prt)
    {
        let choice = ps2_assigned
            .iter()
            .find(|(assigned, _)| assigned.instance_id == kb.instance_id)
            .map_or(
                match pin {
                    Layout::Jis => LayoutChoice::Jis,
                    Layout::Us => LayoutChoice::Us,
                },
                |(_, choice)| *choice,
            );
        device_writes.push((kb.instance_id.clone(), choice_writes(kb, choice)?));
    }
    device_writes.append(&mut hid_writes);

    let mut global_writes = vec![
        PlannedWrite::delete(value_names::PS2_TYPE),
        PlannedWrite::delete(value_names::PS2_SUBTYPE),
    ];
    let stored = |name: &str| match name {
        value_names::LAYER_DRIVER_JPN => global.layer_driver_jpn.as_deref(),
        value_names::KEYBOARD_IDENTIFIER => global.override_keyboard_identifier.as_deref(),
        _ => None,
    };
    for write in standard_layout_writes(standard) {
        let current = stored(&write.name).map_or(RegValue::Absent, |value| RegValue::Sz {
            value: value.to_string(),
        });
        if !value_eq(&write.name, &current, &RegValue::from(&write.op)) {
            global_writes.push(write);
        }
    }
    Ok((device_writes, global_writes))
}

/// How a change reaches the drivers (the heaviest over all targets):
/// - [`PendingAction::RestartPc`] when global values change, for any keyboard whose
///   [`crate::device_apply_action`] is a restart, and for the only usable keyboard (plan 1.4:
///   warn and recommend a PC restart), and when a live-reset candidate is not started or has a
///   problem;
/// - [`PendingAction::Reconnect`] for unproven transports (BLE/BT), for keyboards that are not
///   present, and when `allow_live_reset` is false;
/// - [`PendingAction::ResetKeyboard`] when every target passes [`crate::live_reset_bans`].
///
/// For a keyboard that could be reset in place (`device_apply_action` is a reset), the checks run
/// in this order: not present → `Reconnect`; `allow_live_reset` false → `Reconnect` (the user chose
/// not to reset, so the only-usable-keyboard rule, which guards a reset, does not apply; this is
/// what lets `--no-reset --yes` avoid a PC restart, design review S12); only usable keyboard →
/// `RestartPc`; any other [`crate::live_reset_bans`] (status unknown, not started, a problem) →
/// `RestartPc`. With no targets and no global writes: `ResetKeyboard` (nothing to reset).
pub fn apply_method(
    targets: &[&KeyboardDevice],
    writes_global: bool,
    only_usable_keyboard: bool,
    allow_live_reset: bool,
) -> PendingAction {
    if writes_global {
        return PendingAction::RestartPc;
    }
    targets
        .iter()
        .map(|kb| match device_apply_action(kb) {
            PendingAction::ResetKeyboard => {
                if !kb.present || !allow_live_reset {
                    PendingAction::Reconnect
                } else if only_usable_keyboard || !live_reset_bans(kb, false).is_empty() {
                    PendingAction::RestartPc
                } else {
                    PendingAction::ResetKeyboard
                }
            }
            action => action,
        })
        .max()
        .unwrap_or(PendingAction::ResetKeyboard)
}

/// A kbdhid keyboard that is not read-only, stores both values of its pair or neither, and whose
/// stored values give it no table of its own (0x51/0 without values, 7/0, any unknown type): it
/// types with the PC's standard layout (design standard-layout B.3). A lone value
/// (`IncompletePair`) is no follower: [`set_standard_writes`] refuses it. Connected or not.
pub fn follows_standard(kb: &KeyboardDevice, global: &GlobalSettings) -> bool {
    kb.driver == KeyboardDriver::Kbdhid
        && !is_read_only_device(kb)
        && kb.overrides.keyboard_type_override.is_some()
            == kb.overrides.keyboard_subtype_override.is_some()
        && kb
            .predicted_type(global)
            .is_some_and(|ty| ty.per_keyboard_table().is_none())
}

/// A keyboard MKLM writes values to: served by kbdhid or i8042prt, not virtual, not the Remote
/// Desktop keyboard (the devnodes of [`crate::check_device_writes`]).
fn assignable(kb: &KeyboardDevice) -> bool {
    matches!(kb.driver, KeyboardDriver::Kbdhid | KeyboardDriver::I8042prt)
        && !is_read_only_device(kb)
}

/// True when only one value of the pair the keyboard's driver reads is stored.
fn lone_value(kb: &KeyboardDevice) -> bool {
    let o = &kb.overrides;
    match kb.driver {
        KeyboardDriver::Kbdhid => {
            o.keyboard_type_override.is_some() != o.keyboard_subtype_override.is_some()
        }
        KeyboardDriver::I8042prt => {
            o.override_keyboard_type.is_some() != o.override_keyboard_subtype.is_some()
        }
        KeyboardDriver::Other(_) => false,
    }
}

/// Device and global writes of "change the PC's standard layout" in per-keyboard mode (design
/// standard-layout B.2, B.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardChange {
    /// The stored standard ([`crate::stored_standard`]).
    pub from: Layout,
    pub to: Layout,
    /// One pin to `from` per follower that is not left to follow, in inventory order.
    pub device_writes: DeviceWrites,
    /// The standard's values that differ from the stored ones (none when `to == from`).
    pub global_writes: Vec<PlannedWrite>,
    /// The followers pinned to `from`, and those left to follow `to` (instance IDs, inventory
    /// order).
    pub pinned: Vec<String>,
    pub following: Vec<String>,
    /// Keyboards MKLM cannot pin (read-only kbdhid, other drivers; never the Remote Desktop
    /// keyboard, which the texts treat on its own), with whether their stored values make them
    /// follow the standard: `Some(true)` / `Some(false)`, `None` for another driver, whose type is
    /// unknown (design standard-layout B.3 (8)).
    pub not_assignable: Vec<(String, Option<bool>)>,
}

impl StandardChange {
    /// True when nothing is to be written (`to` is the stored standard).
    pub fn is_empty(&self) -> bool {
        self.device_writes.is_empty() && self.global_writes.is_empty()
    }
}

/// Plans the writes of a standard change (design standard-layout B.4). Refusals, in this order,
/// write nothing: fixed mode ([`OperationError::MigrationRequired`]), a stored standard MKLM does
/// not know ([`OperationError::UnknownStandard`]); then, unless `to` is the stored standard (an
/// empty change): a lone value on a writable keyboard ([`OperationError::IncompleteValues`]), a
/// PS/2 keyboard pinned to a type without a table ([`OperationError::Ps2WithoutTable`]), a
/// `follow` keyboard that does not exist ([`OperationError::UnknownKeyboard`]) or has no follower
/// among the collections of its physical device ([`OperationError::NotFollowingStandard`]).
/// `check_plan` then refuses a PS/2 keyboard without a pin ([`crate::plan_set_standard`]).
///
/// Every follower ([`follows_standard`]), connected or not, hidden or not, is pinned to `from`,
/// except the followers of the physical keyboards in `follow` ([`physical_device_members`]),
/// which get nothing written and follow `to` after the restart.
pub fn set_standard_writes(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    to: Layout,
    follow: &[String],
) -> Result<StandardChange, OperationError> {
    if global.mode() == GlobalMode::Fixed {
        return Err(OperationError::MigrationRequired {
            fixed: ps2_pin_layout(global),
        });
    }
    let from = stored_standard(global).ok_or_else(|| OperationError::UnknownStandard {
        layer_driver: global.layer_driver_jpn.clone(),
        identifier: global.override_keyboard_identifier.clone(),
    })?;
    let not_assignable: Vec<(String, Option<bool>)> = keyboards
        .iter()
        .filter(|kb| !kb.is_remote_desktop() && !assignable(kb))
        .map(|kb| {
            (
                kb.instance_id.clone(),
                kb.predicted_type(global)
                    .map(|ty| ty.per_keyboard_table().is_none()),
            )
        })
        .collect();
    let mut change = StandardChange {
        from,
        to,
        device_writes: Vec::new(),
        global_writes: Vec::new(),
        pinned: Vec::new(),
        following: Vec::new(),
        not_assignable,
    };
    if to == from {
        return Ok(change);
    }

    let ids = |pick: &dyn Fn(&KeyboardDevice) -> bool| -> Vec<String> {
        keyboards
            .iter()
            .filter(|kb| pick(kb))
            .map(|kb| kb.instance_id.clone())
            .collect()
    };
    let incomplete = ids(&|kb| assignable(kb) && lone_value(kb));
    if !incomplete.is_empty() {
        return Err(OperationError::IncompleteValues {
            instance_ids: incomplete,
        });
    }
    let ps2_without_table = ids(&|kb| {
        kb.driver == KeyboardDriver::I8042prt
            && assignable(kb)
            && kb
                .overrides
                .ps2_type()
                .is_some_and(|ty| ty.per_keyboard_table().is_none())
    });
    if !ps2_without_table.is_empty() {
        return Err(OperationError::Ps2WithoutTable {
            instance_ids: ps2_without_table,
        });
    }

    let mut left_to_follow: Vec<String> = Vec::new();
    for requested in follow {
        let members = physical_device_members(keyboards, requested)?;
        let followers: Vec<&KeyboardDevice> = members
            .into_iter()
            .filter(|kb| follows_standard(kb, global))
            .collect();
        if followers.is_empty() {
            let instance_id = keyboards
                .iter()
                .find(|kb| kb.instance_id.eq_ignore_ascii_case(requested))
                .map_or_else(|| requested.clone(), |kb| kb.instance_id.clone());
            return Err(OperationError::NotFollowingStandard { instance_id });
        }
        for kb in followers {
            if !left_to_follow
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&kb.instance_id))
            {
                left_to_follow.push(kb.instance_id.clone());
            }
        }
    }

    for kb in keyboards.iter().filter(|kb| follows_standard(kb, global)) {
        if left_to_follow
            .iter()
            .any(|id| id.eq_ignore_ascii_case(&kb.instance_id))
        {
            change.following.push(kb.instance_id.clone());
            continue;
        }
        let writes = device_layout_writes(&kb.driver, Some(from)).ok_or_else(|| {
            OperationError::StandardNotAllowed {
                instance_id: kb.instance_id.clone(),
            }
        })?;
        change.device_writes.push((kb.instance_id.clone(), writes));
        change.pinned.push(kb.instance_id.clone());
    }

    let stored = |name: &str| match name {
        value_names::LAYER_DRIVER_JPN => global.layer_driver_jpn.as_deref(),
        value_names::KEYBOARD_IDENTIFIER => global.override_keyboard_identifier.as_deref(),
        _ => None,
    };
    for write in standard_layout_writes(to) {
        let current = stored(&write.name).map_or(RegValue::Absent, |value| RegValue::Sz {
            value: value.to_string(),
        });
        if !value_eq(&write.name, &current, &RegValue::from(&write.op)) {
            change.global_writes.push(write);
        }
    }
    Ok(change)
}

/// A standard change as checked ([`crate::check_plan`]: allowlist, INV-PS2, write order) with
/// what it does to whom. `plan.apply` is always [`PendingAction::RestartPc`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPlan {
    pub plan: OperationPlan,
    pub change: StandardChange,
}

/// Plans a standard change (design standard-layout B.4): [`set_standard_writes`], then
/// [`crate::check_plan`], which orders the pins (the other keyboards) before the global values
/// and refuses a PS/2 keyboard without a pin (`PlanError::InvPs2`). The preview (CLI, GUI) and
/// the engine call this one function (design review S6).
pub fn plan_set_standard(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    to: Layout,
    follow: &[String],
) -> Result<StandardPlan, OperationError> {
    let change = set_standard_writes(keyboards, global, to, follow)?;
    let checked = check_plan(
        keyboards,
        global,
        &change.device_writes,
        &change.global_writes,
    )?;
    Ok(StandardPlan {
        plan: OperationPlan {
            instance_ids: written_devices(&checked),
            checked,
            apply: PendingAction::RestartPc,
            only_usable_keyboard: false,
        },
        change,
    })
}

/// The global values a standard change records without writing them (`before` = `intended` =
/// `Absent`): the fixed-mode pair, so that a pair written later by anyone else (the Settings
/// app's "English keyboard", say) is a conflict when writing, after the restart, on keep and on
/// revert (design standard-layout B.2, SAFETY-7). No baseline is captured for them.
pub fn standard_guards() -> [ValueKey; 2] {
    [value_names::PS2_TYPE, value_names::PS2_SUBTYPE].map(|name| ValueKey {
        target: WriteTarget::Global,
        name: name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allowlist::PlanStep;
    use crate::fixtures;
    use crate::model::{DeviceOverrides, KeyboardType, SystemSnapshot};
    use crate::safety::InvPs2Violation;
    use value_names::*;

    const RESET_AND_OTHER_INPUT: ApplyOptions = ApplyOptions {
        allow_live_reset: true,
        other_input_available: true,
        countdown_seconds: crate::report::DEFAULT_COUNTDOWN_SECONDS,
    };

    /// Another collection of the Keychron receiver (same container, different interface).
    fn keychron_second() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"HID\VID_3434&PID_D027&MI_01&COL01\8&2&0&0000".into(),
            container_id: Some("{f0d991ea-a583-5b9c-800d-48846ac6e633}".into()),
            ..fixtures::keychron()
        }
    }

    /// A collection of the same receiver that is not connected now.
    fn keychron_phantom() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"HID\VID_3434&PID_D027&MI_02&COL01\8&3&0&0000".into(),
            present: false,
            reported_type: None,
            dev_node_status: None,
            problem_code: None,
            overrides: DeviceOverrides::default(),
            ..fixtures::keychron()
        }
    }

    fn bare_ps2() -> KeyboardDevice {
        KeyboardDevice {
            overrides: DeviceOverrides::default(),
            ..fixtures::internal_ps2()
        }
    }

    fn phantom_ps2() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"ACPI\PNP0303\4&1&0".into(),
            present: false,
            reported_type: None,
            dev_node_status: None,
            problem_code: None,
            ..bare_ps2()
        }
    }

    fn ids(members: &[&KeyboardDevice]) -> Vec<String> {
        members.iter().map(|kb| kb.instance_id.clone()).collect()
    }

    fn pair(ty: u32, subtype: u32, names: (&str, &str)) -> Vec<PlannedWrite> {
        vec![
            PlannedWrite::set(names.0, ty),
            PlannedWrite::set(names.1, subtype),
        ]
    }

    const HID: (&str, &str) = (HID_TYPE, HID_SUBTYPE);
    const PS2: (&str, &str) = (PS2_TYPE, PS2_SUBTYPE);

    #[test]
    fn physical_device_members_follow_the_container() {
        let keyboards = vec![
            fixtures::internal_ps2(),
            fixtures::keychron(),
            fixtures::vxe_ble(),
            keychron_second(),
            keychron_phantom(),
            // Same container, but not served by kbdhid: not written.
            KeyboardDevice {
                instance_id: r"HID\VID_3434&PID_D027&MI_03\8&4&0&0000".into(),
                driver: KeyboardDriver::Other("vendor".into()),
                ..fixtures::keychron()
            },
        ];
        let keychron = fixtures::keychron().instance_id;
        let members = physical_device_members(&keyboards, &keychron.to_ascii_lowercase()).unwrap();
        assert_eq!(
            ids(&members),
            vec![
                keychron.clone(),
                keychron_second().instance_id,
                keychron_phantom().instance_id
            ]
        );
        // Asking for any collection gives the same device.
        assert_eq!(
            ids(&physical_device_members(&keyboards, &keychron_phantom().instance_id).unwrap()),
            ids(&members)
        );
        // i8042prt and the internal container: the keyboard alone.
        let internal_hid = KeyboardDevice {
            instance_id: r"HID\ELAN0001&Col01\5&1&0&0000".into(),
            driver: KeyboardDriver::Kbdhid,
            transport: Transport::I2c,
            ..fixtures::internal_ps2()
        };
        let with_internal = vec![fixtures::internal_ps2(), internal_hid.clone()];
        assert_eq!(
            ids(&physical_device_members(&with_internal, &internal_hid.instance_id).unwrap()),
            vec![internal_hid.instance_id.clone()]
        );
        assert_eq!(
            ids(
                &physical_device_members(&with_internal, &fixtures::internal_ps2().instance_id)
                    .unwrap()
            ),
            vec![fixtures::internal_ps2().instance_id]
        );
        // No usable container: the keyboard alone.
        let unknown = vec![
            KeyboardDevice {
                container_id: None,
                ..fixtures::keychron()
            },
            KeyboardDevice {
                container_id: None,
                ..keychron_second()
            },
        ];
        assert_eq!(
            physical_device_members(&unknown, &keychron).unwrap().len(),
            1
        );
        assert_eq!(
            physical_device_members(&keyboards, r"HID\NOPE\1"),
            Err(OperationError::UnknownKeyboard {
                instance_id: r"HID\NOPE\1".into()
            })
        );
    }

    #[test]
    fn set_layout_writes_rules() {
        let snapshot = fixtures::dev_machine();
        let keychron = fixtures::keychron().instance_id;
        // Fixed mode ignores per-device values (M0 #2a).
        assert_eq!(
            set_layout_writes(
                &snapshot.keyboards,
                &fixtures::global_fixed_jis(),
                &keychron,
                LayoutChoice::Jis
            ),
            Err(OperationError::MigrationRequired {
                fixed: Some(Layout::Jis)
            })
        );
        // An unknown keyboard is reported as such, whatever the mode.
        assert!(matches!(
            set_layout_writes(
                &snapshot.keyboards,
                &fixtures::global_fixed_jis(),
                "HID\\NOPE\\1",
                LayoutChoice::Jis
            ),
            Err(OperationError::UnknownKeyboard { .. })
        ));
        let ps2 = fixtures::internal_ps2().instance_id;
        assert_eq!(
            set_layout_writes(
                &snapshot.keyboards,
                &snapshot.global,
                &ps2,
                LayoutChoice::Standard
            ),
            Err(OperationError::StandardNotAllowed {
                instance_id: ps2.clone()
            })
        );
        assert_eq!(
            set_layout_writes(
                &snapshot.keyboards,
                &snapshot.global,
                &ps2,
                LayoutChoice::Us
            )
            .unwrap(),
            vec![(ps2, pair(4, 0, PS2))]
        );
        assert_eq!(
            set_layout_writes(
                &snapshot.keyboards,
                &snapshot.global,
                &keychron,
                LayoutChoice::Standard
            )
            .unwrap(),
            vec![(
                keychron,
                vec![
                    PlannedWrite::delete(HID_TYPE),
                    PlannedWrite::delete(HID_SUBTYPE)
                ]
            )]
        );
        // Synthetic (not an ID Windows uses): a virtual keyboard of another driver.
        let rdp = KeyboardDevice {
            instance_id: r"TS_INPT\TS_KBD\1".into(),
            driver: KeyboardDriver::Other("TermDD".into()),
            transport: Transport::Virtual,
            container_id: None,
            ..fixtures::keychron()
        };
        assert!(matches!(
            set_layout_writes(
                std::slice::from_ref(&rdp),
                &snapshot.global,
                &rdp.instance_id,
                LayoutChoice::Us
            ),
            Err(OperationError::Plan(PlanError::Device {
                error: AllowlistError::ReadOnlyDriver { .. },
                ..
            }))
        ));
        // The Remote Desktop keyboard as Windows lists it (plan 3.2: read-only).
        let real = fixtures::rdp_keyboard();
        let mut keyboards = snapshot.keyboards.clone();
        keyboards.push(real.clone());
        for choice in [LayoutChoice::Jis, LayoutChoice::Us, LayoutChoice::Standard] {
            assert!(
                matches!(
                    set_layout_writes(&keyboards, &snapshot.global, &real.instance_id, choice),
                    Err(OperationError::Plan(PlanError::Device {
                        error: AllowlistError::ReadOnlyDriver { .. },
                        ..
                    }))
                ),
                "{choice:?}"
            );
        }
    }

    #[test]
    fn plan_set_layout_on_the_development_machine() {
        let snapshot = fixtures::dev_machine();
        let keychron = fixtures::keychron().instance_id;
        let plan = plan_set_layout(
            &snapshot.keyboards,
            &snapshot.global,
            &keychron,
            LayoutChoice::Jis,
            &RESET_AND_OTHER_INPUT,
        )
        .unwrap();
        assert_eq!(plan.apply, PendingAction::ResetKeyboard);
        assert!(!plan.only_usable_keyboard);
        assert_eq!(plan.instance_ids, vec![keychron.clone()]);
        assert_eq!(
            plan.checked.steps,
            vec![PlanStep {
                target: WriteTarget::Device {
                    instance_id: keychron.clone()
                },
                writes: pair(7, 2, HID)
            }]
        );

        // No other way to type declared: the only usable keyboard is not reset (plan 1.4).
        let alone = ApplyOptions {
            allow_live_reset: true,
            other_input_available: false,
            ..ApplyOptions::default()
        };
        let plan = plan_set_layout(
            &snapshot.keyboards,
            &snapshot.global,
            &keychron,
            LayoutChoice::Jis,
            &alone,
        )
        .unwrap();
        assert!(plan.only_usable_keyboard);
        assert_eq!(plan.apply, PendingAction::RestartPc);

        // --no-reset: reconnect, with or without another input (design review S12).
        for other_input_available in [false, true] {
            let no_reset = ApplyOptions {
                allow_live_reset: false,
                other_input_available,
                ..ApplyOptions::default()
            };
            let plan = plan_set_layout(
                &snapshot.keyboards,
                &snapshot.global,
                &keychron,
                LayoutChoice::Us,
                &no_reset,
            )
            .unwrap();
            assert_eq!(plan.apply, PendingAction::Reconnect);
        }

        // BLE: reconnect (no verified live reset). PS/2: restart.
        let plan = plan_set_layout(
            &snapshot.keyboards,
            &snapshot.global,
            &fixtures::vxe_ble().instance_id,
            LayoutChoice::Us,
            &RESET_AND_OTHER_INPUT,
        )
        .unwrap();
        assert_eq!(plan.apply, PendingAction::Reconnect);
        let plan = plan_set_layout(
            &snapshot.keyboards,
            &snapshot.global,
            &fixtures::internal_ps2().instance_id,
            LayoutChoice::Us,
            &RESET_AND_OTHER_INPUT,
        )
        .unwrap();
        assert_eq!(plan.apply, PendingAction::RestartPc);
    }

    #[test]
    fn plan_set_layout_covers_every_collection_of_the_device() {
        let mut keyboards = fixtures::dev_machine().keyboards;
        keyboards.push(keychron_second());
        let global = fixtures::global_per_keyboard();
        let keychron = fixtures::keychron().instance_id;
        let plan = plan_set_layout(
            &keyboards,
            &global,
            &keychron,
            LayoutChoice::Jis,
            &RESET_AND_OTHER_INPUT,
        )
        .unwrap();
        assert_eq!(
            plan.instance_ids,
            vec![keychron.clone(), keychron_second().instance_id]
        );
        assert_eq!(plan.apply, PendingAction::ResetKeyboard);
        // A phantom collection is written too; it applies when it reconnects.
        keyboards.push(keychron_phantom());
        let plan = plan_set_layout(
            &keyboards,
            &global,
            &keychron,
            LayoutChoice::Jis,
            &RESET_AND_OTHER_INPUT,
        )
        .unwrap();
        assert_eq!(plan.instance_ids.len(), 3);
        assert_eq!(plan.apply, PendingAction::Reconnect);
    }

    #[test]
    fn only_usable_keyboard_ignores_the_same_device_and_unusable_keyboards() {
        let global = fixtures::global_per_keyboard();
        let keychron = fixtures::keychron().instance_id;
        let pinned = fixtures::internal_ps2();
        let not_started = KeyboardDevice {
            dev_node_status: Some(0x0180_0002),
            ..pinned.clone()
        };
        // Synthetic (not an ID Windows uses): any virtual keyboard.
        let virtual_kb = KeyboardDevice {
            instance_id: r"TS_INPT\TS_KBD\1".into(),
            transport: Transport::Virtual,
            driver: KeyboardDriver::Other("TermDD".into()),
            container_id: None,
            ..fixtures::keychron()
        };
        // The Remote Desktop keyboard as Windows lists it, classified as the reader does: started
        // and present, but it types what the client sends (design m2 D.2: not virtual).
        let rdp = fixtures::rdp_keyboard();
        let rdp = KeyboardDevice {
            transport: crate::transport::classify_transport(
                &rdp.instance_id,
                &rdp.parent_chain,
                &rdp.driver,
            ),
            ..rdp
        };
        let cases = [
            (vec![fixtures::keychron(), pinned.clone()], false),
            (vec![fixtures::keychron(), keychron_second()], true),
            (vec![fixtures::keychron(), not_started], true),
            (vec![fixtures::keychron(), virtual_kb], true),
            (vec![fixtures::keychron(), rdp], true),
            (
                vec![
                    fixtures::keychron(),
                    KeyboardDevice {
                        present: false,
                        ..pinned.clone()
                    },
                ],
                true,
            ),
        ];
        for (keyboards, only_usable) in cases {
            let plan = plan_set_layout(
                &keyboards,
                &global,
                &keychron,
                LayoutChoice::Jis,
                &RESET_AND_OTHER_INPUT,
            )
            .unwrap();
            assert_eq!(plan.only_usable_keyboard, only_usable, "{keyboards:#?}");
        }
    }

    #[test]
    fn plan_set_layout_refuses_a_broken_inv_ps2() {
        let keyboards = vec![bare_ps2(), fixtures::keychron()];
        assert_eq!(
            plan_set_layout(
                &keyboards,
                &fixtures::global_per_keyboard(),
                &fixtures::keychron().instance_id,
                LayoutChoice::Jis,
                &RESET_AND_OTHER_INPUT,
            ),
            Err(OperationError::Plan(PlanError::InvPs2(InvPs2Violation {
                keyboards: vec![bare_ps2().instance_id]
            })))
        );
    }

    /// The development machine before M0: fixed JIS, nothing stored per device.
    fn before_m0() -> Vec<KeyboardDevice> {
        vec![
            bare_ps2(),
            phantom_ps2(),
            KeyboardDevice {
                overrides: DeviceOverrides::default(),
                ..fixtures::keychron()
            },
            fixtures::vxe_ble(),
        ]
    }

    #[test]
    fn plan_migration_pins_every_ps2_keyboard() {
        let keyboards = before_m0();
        let keychron = fixtures::keychron().instance_id;
        let plan = plan_migration(
            &keyboards,
            &fixtures::global_fixed_jis(),
            Layout::Jis,
            &[(keychron.clone(), LayoutChoice::Us)],
        )
        .unwrap();
        assert_eq!(plan.apply, PendingAction::RestartPc);
        assert!(!plan.only_usable_keyboard);
        let device = |id: &str| WriteTarget::Device {
            instance_id: id.to_string(),
        };
        assert_eq!(
            plan.checked.steps,
            vec![
                PlanStep {
                    target: device(&bare_ps2().instance_id),
                    writes: pair(7, 2, PS2)
                },
                PlanStep {
                    target: device(&phantom_ps2().instance_id),
                    writes: pair(7, 2, PS2)
                },
                PlanStep {
                    target: device(&keychron),
                    writes: pair(4, 0, HID)
                },
                // The standard is already JIS: only the pair goes.
                PlanStep {
                    target: WriteTarget::Global,
                    writes: vec![
                        PlannedWrite::delete(PS2_TYPE),
                        PlannedWrite::delete(PS2_SUBTYPE)
                    ]
                },
            ]
        );
        assert_eq!(
            plan.instance_ids,
            vec![bare_ps2().instance_id, phantom_ps2().instance_id, keychron]
        );
    }

    #[test]
    fn plan_migration_assignments_and_standards() {
        let keyboards = before_m0();
        let ps2 = bare_ps2().instance_id;
        // An assignment to the PS/2 keyboard replaces its pin (one write per keyboard).
        let (devices, _) = migration_writes(
            &keyboards,
            &fixtures::global_fixed_jis(),
            Layout::Jis,
            &[(ps2.to_ascii_lowercase(), LayoutChoice::Us)],
        )
        .unwrap();
        assert_eq!(
            devices,
            vec![
                (ps2.clone(), pair(4, 0, PS2)),
                (phantom_ps2().instance_id, pair(7, 2, PS2)),
            ]
        );

        // Fixed US pins US (4/0; 7/0 is not allowed per device), and a JIS standard is written.
        let fixed_us = GlobalSettings {
            layer_driver_jpn: Some("kbd101.dll".into()),
            override_keyboard_identifier: Some("PCAT_101KEY".into()),
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(0),
            ..Default::default()
        };
        let plan = plan_migration(&keyboards, &fixed_us, Layout::Jis, &[]).unwrap();
        assert_eq!(plan.checked.steps[0].writes, pair(4, 0, PS2));
        assert_eq!(
            plan.checked.steps.last().unwrap().writes,
            vec![
                PlannedWrite::delete(PS2_TYPE),
                PlannedWrite::delete(PS2_SUBTYPE),
                PlannedWrite::set_string(LAYER_DRIVER_JPN, "kbd106.dll"),
                PlannedWrite::set_string(KEYBOARD_IDENTIFIER, "PCAT_106KEY"),
            ]
        );
        assert_eq!(
            plan.checked.global.layer_driver_jpn.as_deref(),
            Some("kbd106.dll")
        );
        // Keeping US writes no standard; a different spelling of the same value is not a change.
        let (_, global) = migration_writes(&keyboards, &fixed_us, Layout::Us, &[]).unwrap();
        assert_eq!(global.len(), 2);
        let shouting = GlobalSettings {
            layer_driver_jpn: Some("KBD106.DLL".into()),
            ..fixtures::global_fixed_jis()
        };
        let (_, global) = migration_writes(&keyboards, &shouting, Layout::Jis, &[]).unwrap();
        assert_eq!(global.len(), 2);
    }

    #[test]
    fn plan_migration_refusals() {
        let keyboards = before_m0();
        let ps2 = bare_ps2().instance_id;
        assert_eq!(
            plan_migration(
                &keyboards,
                &fixtures::global_per_keyboard(),
                Layout::Jis,
                &[]
            ),
            Err(OperationError::NotFixedMode)
        );
        let mismatch = GlobalSettings {
            override_keyboard_subtype: Some(0),
            ..fixtures::global_fixed_jis()
        };
        assert_eq!(
            plan_migration(&keyboards, &mismatch, Layout::Jis, &[]),
            Err(OperationError::InconsistentGlobal)
        );
        let fixed = fixtures::global_fixed_jis();
        assert_eq!(
            plan_migration(
                &keyboards,
                &fixed,
                Layout::Jis,
                &[(r"HID\NOPE\1".into(), LayoutChoice::Us)]
            ),
            Err(OperationError::UnknownKeyboard {
                instance_id: r"HID\NOPE\1".into()
            })
        );
        assert_eq!(
            plan_migration(
                &keyboards,
                &fixed,
                Layout::Jis,
                &[(ps2.clone(), LayoutChoice::Standard)]
            ),
            Err(OperationError::StandardNotAllowed {
                instance_id: ps2.clone()
            })
        );
        assert_eq!(
            plan_migration(
                &keyboards,
                &fixed,
                Layout::Jis,
                &[
                    (ps2.clone(), LayoutChoice::Us),
                    (ps2.clone(), LayoutChoice::Jis)
                ]
            ),
            Err(OperationError::Plan(PlanError::DuplicateKeyboard {
                instance_id: ps2
            }))
        );
        let keychron = fixtures::keychron().instance_id;
        assert!(matches!(
            plan_migration(
                &keyboards,
                &fixed,
                Layout::Jis,
                &[
                    (keychron.clone(), LayoutChoice::Us),
                    (keychron, LayoutChoice::Jis)
                ]
            ),
            Err(OperationError::Plan(PlanError::DuplicateKeyboard { .. }))
        ));
        // "Follow the standard" is fine for HID keyboards.
        assert!(
            plan_migration(
                &keyboards,
                &fixed,
                Layout::Us,
                &[(fixtures::vxe_ble().instance_id, LayoutChoice::Standard)]
            )
            .is_ok()
        );
    }

    #[test]
    fn apply_method_table() {
        use PendingAction::*;
        let usb = fixtures::keychron();
        let ble = fixtures::vxe_ble();
        let ps2 = fixtures::internal_ps2();
        let phantom = keychron_phantom();
        let problem = KeyboardDevice {
            dev_node_status: Some(0x0180_040A),
            problem_code: Some(10),
            ..fixtures::keychron()
        };
        let unknown_status = KeyboardDevice {
            dev_node_status: None,
            ..fixtures::keychron()
        };
        let not_started = KeyboardDevice {
            dev_node_status: Some(0x0180_0002),
            ..fixtures::keychron()
        };
        let no_container = KeyboardDevice {
            container_id: None,
            ..fixtures::keychron()
        };
        // (targets, writes_global, only_usable, allow_live_reset) → action
        let table: Vec<(Vec<&KeyboardDevice>, bool, bool, bool, PendingAction)> = vec![
            (vec![&usb], false, false, true, ResetKeyboard),
            (vec![&usb], false, false, false, Reconnect),
            (vec![&usb], false, true, true, RestartPc),
            (vec![&usb], false, true, false, Reconnect),
            (vec![&usb], true, false, true, RestartPc),
            (vec![], true, false, true, RestartPc),
            (vec![&ble], false, false, true, Reconnect),
            (vec![&ble], false, true, true, Reconnect),
            (vec![&ps2], false, false, true, RestartPc),
            (vec![&ps2], false, false, false, RestartPc),
            (vec![&phantom], false, false, true, Reconnect),
            (vec![&problem], false, false, true, RestartPc),
            (vec![&unknown_status], false, false, true, RestartPc),
            (vec![&not_started], false, false, true, RestartPc),
            (vec![&not_started], false, false, false, Reconnect),
            (vec![&no_container], false, false, true, RestartPc),
            (vec![&usb, &ble], false, false, true, Reconnect),
            (vec![&usb, &phantom], false, false, true, Reconnect),
            (vec![&usb, &ps2], false, false, true, RestartPc),
            (vec![], false, false, true, ResetKeyboard),
        ];
        for (targets, writes_global, only_usable, allow, want) in table {
            assert_eq!(
                apply_method(&targets, writes_global, only_usable, allow),
                want,
                "{:?} global={writes_global} only={only_usable} allow={allow}",
                ids(&targets)
            );
        }
    }

    // --- Design standard-layout B.3, B.4: changing the standard in per-keyboard mode ----------

    fn desktop() -> (Vec<KeyboardDevice>, GlobalSettings) {
        let snapshot = fixtures::desktop_pc();
        (snapshot.keyboards, snapshot.global)
    }

    fn device(id: &str) -> WriteTarget {
        WriteTarget::Device {
            instance_id: id.to_string(),
        }
    }

    /// A phantom of the 2.4G receiver on another port: no values, not connected.
    fn wireless_phantom() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"HID\VID_1D57&PID_FA60&MI_00\7&99999999&0&0000".into(),
            container_id: Some("{15A651F6-BAA8-11F1-B5A1-806E6F6E6963}".into()),
            present: false,
            reported_type: None,
            dev_node_status: None,
            problem_code: None,
            ..fixtures::desktop_wireless()
        }
    }

    /// A read-only kbdhid keyboard (a VM's virtual keyboard) and a keyboard of another driver
    /// (Hyper-V's basic session keyboard): MKLM writes neither.
    fn read_only_hid() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"HID\VMBUS&KEYBOARD\1".into(),
            display_name: "Virtual keyboard".into(),
            transport: Transport::Virtual,
            container_id: None,
            ..fixtures::desktop_vxe()
        }
    }

    fn hyperkbd() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"VMBUS\{F912AD6D-2B17-48EA-BD65-F927A61C7684}\1".into(),
            display_name: "Microsoft Hyper-V Virtual Keyboard".into(),
            driver: KeyboardDriver::Other("hyperkbd".into()),
            transport: Transport::Virtual,
            container_id: None,
            ..fixtures::desktop_vxe()
        }
    }

    #[test]
    fn set_standard_pins_every_follower_by_default() {
        let (keyboards, global) = desktop();
        let plan = plan_set_standard(&keyboards, &global, Layout::Us, &[]).unwrap();
        let wireless = fixtures::desktop_wireless().instance_id;
        let second = fixtures::desktop_wireless_second().instance_id;
        let vxe = fixtures::desktop_vxe().instance_id;
        assert_eq!(plan.change.from, Layout::Jis);
        assert_eq!(plan.change.to, Layout::Us);
        assert_eq!(
            plan.change.pinned,
            vec![wireless.clone(), second.clone(), vxe.clone()]
        );
        assert!(plan.change.following.is_empty());
        assert!(plan.change.not_assignable.is_empty());
        // Pins (JIS, the standard now) first, then the standard; nothing on the PS/2 node, the
        // assigned keyboards or the Remote Desktop keyboard.
        assert_eq!(
            plan.plan.checked.steps,
            vec![
                PlanStep {
                    target: device(&wireless),
                    writes: pair(7, 2, HID)
                },
                PlanStep {
                    target: device(&second),
                    writes: pair(7, 2, HID)
                },
                PlanStep {
                    target: device(&vxe),
                    writes: pair(7, 2, HID)
                },
                PlanStep {
                    target: WriteTarget::Global,
                    writes: vec![
                        PlannedWrite::set_string(LAYER_DRIVER_JPN, "kbd101.dll"),
                        PlannedWrite::set_string(KEYBOARD_IDENTIFIER, "PCAT_101KEY"),
                    ]
                },
            ]
        );
        assert_eq!(plan.plan.instance_ids, vec![wireless, second, vxe]);
        assert_eq!(plan.plan.apply, PendingAction::RestartPc);
        assert!(!plan.plan.only_usable_keyboard);
        // What each keyboard types after the plan: the same table as now, except for nobody.
        let before = crate::assess(&fixtures::desktop_pc());
        let after = crate::assess(&SystemSnapshot {
            keyboards: plan.plan.checked.keyboards.clone(),
            global: plan.plan.checked.global.clone(),
            ..fixtures::desktop_pc()
        });
        for (b, a) in before.keyboards.iter().zip(&after.keyboards) {
            assert_eq!(
                b.after_restart.as_ref().map(|l| &l.table),
                a.after_restart.as_ref().map(|l| &l.table),
                "{}",
                b.instance_id
            );
        }
        // Back to JIS from a US standard: the followers are pinned to US (4/0).
        let us = GlobalSettings {
            layer_driver_jpn: Some("kbd101.dll".into()),
            override_keyboard_identifier: Some("PCAT_101KEY".into()),
            ..global
        };
        let plan = plan_set_standard(&keyboards, &us, Layout::Jis, &[]).unwrap();
        assert_eq!(plan.change.from, Layout::Us);
        assert_eq!(plan.plan.checked.steps[0].writes, pair(4, 0, HID));
        assert_eq!(
            plan.plan.checked.steps.last().unwrap().writes,
            vec![
                PlannedWrite::set_string(LAYER_DRIVER_JPN, "kbd106.dll"),
                PlannedWrite::set_string(KEYBOARD_IDENTIFIER, "PCAT_106KEY"),
            ]
        );
    }

    #[test]
    fn set_standard_follow_leaves_the_device() {
        let (keyboards, global) = desktop();
        let wireless = fixtures::desktop_wireless().instance_id;
        let second = fixtures::desktop_wireless_second().instance_id;
        let vxe = fixtures::desktop_vxe().instance_id;
        // Either collection names the physical keyboard, in any case.
        for requested in [second.to_ascii_lowercase(), wireless.clone()] {
            let plan =
                plan_set_standard(&keyboards, &global, Layout::Us, &[requested.clone()]).unwrap();
            assert_eq!(plan.change.pinned, vec![vxe.clone()], "{requested}");
            assert_eq!(
                plan.change.following,
                vec![wireless.clone(), second.clone()]
            );
            assert_eq!(plan.plan.instance_ids, vec![vxe.clone()]);
        }
        // Every follower left to follow: only the standard is written.
        let plan = plan_set_standard(
            &keyboards,
            &global,
            Layout::Us,
            &[wireless.clone(), vxe.clone(), wireless.clone()],
        )
        .unwrap();
        assert!(plan.change.pinned.is_empty());
        assert_eq!(plan.change.following, vec![wireless, second, vxe]);
        assert_eq!(plan.plan.checked.steps.len(), 1);
        assert_eq!(plan.plan.checked.steps[0].target, WriteTarget::Global);
    }

    #[test]
    fn set_standard_to_the_same_layout_writes_nothing() {
        let (keyboards, global) = desktop();
        let plan = plan_set_standard(&keyboards, &global, Layout::Jis, &[]).unwrap();
        assert!(plan.change.is_empty());
        assert!(plan.change.pinned.is_empty() && plan.change.following.is_empty());
        assert!(plan.plan.checked.steps.is_empty());
        // Another spelling of the same standard is the same standard.
        let shouting = GlobalSettings {
            layer_driver_jpn: Some("KBD106.DLL".into()),
            override_keyboard_identifier: Some("pcat_106key".into()),
            ..global
        };
        assert!(
            set_standard_writes(&keyboards, &shouting, Layout::Jis, &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn set_standard_pins_phantoms_and_replaces_7_0() {
        let (mut keyboards, global) = desktop();
        keyboards.push(wireless_phantom());
        // A follower that stores 7/0 (a type MKLM never writes): pinned to JIS, 7/2.
        let seven_zero = KeyboardDevice {
            overrides: DeviceOverrides {
                keyboard_type_override: Some(7),
                keyboard_subtype_override: Some(0),
                ..Default::default()
            },
            ..fixtures::desktop_vxe()
        };
        let vxe = seven_zero.instance_id.clone();
        let index = keyboards
            .iter()
            .position(|kb| kb.instance_id == vxe)
            .unwrap();
        keyboards[index] = seven_zero;
        let plan = plan_set_standard(&keyboards, &global, Layout::Us, &[]).unwrap();
        assert!(plan.change.pinned.contains(&wireless_phantom().instance_id));
        assert!(plan.change.pinned.contains(&vxe));
        let after = plan
            .plan
            .checked
            .keyboards
            .iter()
            .find(|kb| kb.instance_id == vxe)
            .unwrap();
        assert_eq!(after.overrides.hid_type(), Some(KeyboardType::JIS));
        // Left to follow, the phantom is written nothing.
        let plan = plan_set_standard(
            &keyboards,
            &global,
            Layout::Us,
            &[wireless_phantom().instance_id],
        )
        .unwrap();
        assert_eq!(plan.change.following, vec![wireless_phantom().instance_id]);
        assert!(
            !plan
                .plan
                .instance_ids
                .contains(&wireless_phantom().instance_id)
        );
    }

    #[test]
    fn set_standard_lists_keyboards_it_cannot_pin() {
        let (mut keyboards, global) = desktop();
        let assigned_read_only = KeyboardDevice {
            instance_id: r"HID\VMBUS&KEYBOARD\2".into(),
            overrides: DeviceOverrides {
                keyboard_type_override: Some(4),
                keyboard_subtype_override: Some(0),
                ..Default::default()
            },
            ..read_only_hid()
        };
        keyboards.extend([read_only_hid(), hyperkbd(), assigned_read_only.clone()]);
        let plan = plan_set_standard(&keyboards, &global, Layout::Us, &[]).unwrap();
        assert_eq!(
            plan.change.not_assignable,
            vec![
                (read_only_hid().instance_id, Some(true)),
                (hyperkbd().instance_id, None),
                (assigned_read_only.instance_id, Some(false)),
            ]
        );
        // Neither is pinned, and the Remote Desktop keyboard is not listed.
        assert!(!plan.change.pinned.contains(&read_only_hid().instance_id));
        assert!(
            plan.change
                .not_assignable
                .iter()
                .all(|(id, _)| !id.starts_with("TERMINPUT_BUS"))
        );
    }

    #[test]
    fn follows_standard_table() {
        let global = fixtures::global_per_keyboard();
        let hid = |ty: Option<u32>, subtype: Option<u32>| KeyboardDevice {
            overrides: DeviceOverrides {
                keyboard_type_override: ty,
                keyboard_subtype_override: subtype,
                ..Default::default()
            },
            ..fixtures::desktop_vxe()
        };
        let cases = [
            (hid(None, None), true),
            (hid(Some(0x51), Some(0)), true),
            (hid(Some(7), Some(0)), true),
            (hid(Some(8), Some(3)), true),
            (hid(Some(4), Some(0)), false),
            (hid(Some(7), Some(2)), false),
            (hid(Some(7), Some(0xD02)), false),
            // A lone value: no follower (the operation refuses it).
            (hid(Some(7), None), false),
            (hid(None, Some(0)), false),
            (fixtures::desktop_ps2(), false),
            (
                KeyboardDevice {
                    overrides: DeviceOverrides::default(),
                    ..fixtures::desktop_ps2()
                },
                false,
            ),
            (fixtures::desktop_rdp_keyboard(), false),
            (read_only_hid(), false),
            (hyperkbd(), false),
            (wireless_phantom(), true),
        ];
        for (kb, follows) in cases {
            assert_eq!(
                follows_standard(&kb, &global),
                follows,
                "{} {:?}",
                kb.instance_id,
                kb.overrides
            );
        }
    }

    #[test]
    fn set_standard_refusals() {
        let (keyboards, global) = desktop();
        // Fixed mode: the standard is every keyboard's layout; a migration chooses it.
        let fixed = GlobalSettings {
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(2),
            ..global.clone()
        };
        assert_eq!(
            plan_set_standard(&keyboards, &fixed, Layout::Us, &[]),
            Err(OperationError::MigrationRequired {
                fixed: Some(Layout::Jis)
            })
        );
        // A lone global value is fixed mode too.
        let lone = GlobalSettings {
            override_keyboard_type: Some(7),
            ..global.clone()
        };
        assert!(matches!(
            plan_set_standard(&keyboards, &lone, Layout::Us, &[]),
            Err(OperationError::MigrationRequired { fixed: None })
        ));
        // A standard MKLM does not write: kbdnec, a mismatched identifier, no values.
        for (layer_driver, identifier) in [
            (Some("kbdnec.dll"), Some("PCAT_106KEY")),
            (Some("kbd106n.dll"), Some("PCAT_106KEY")),
            (Some("kbd106.dll"), Some("PCAT_101KEY")),
            (None, None),
        ] {
            let odd = GlobalSettings {
                layer_driver_jpn: layer_driver.map(str::to_string),
                override_keyboard_identifier: identifier.map(str::to_string),
                ..global.clone()
            };
            assert_eq!(
                plan_set_standard(&keyboards, &odd, Layout::Us, &[]),
                Err(OperationError::UnknownStandard {
                    layer_driver: layer_driver.map(str::to_string),
                    identifier: identifier.map(str::to_string),
                }),
                "{layer_driver:?} {identifier:?}"
            );
        }

        // One value of a pair, on a kbdhid phantom or a PS/2 keyboard.
        let lone_hid = KeyboardDevice {
            overrides: DeviceOverrides {
                keyboard_type_override: Some(4),
                ..Default::default()
            },
            ..wireless_phantom()
        };
        let lone_ps2 = KeyboardDevice {
            instance_id: r"ACPI\PNP0303\1".into(),
            present: false,
            overrides: DeviceOverrides {
                override_keyboard_subtype: Some(2),
                ..Default::default()
            },
            ..fixtures::desktop_ps2()
        };
        let mut with_lone = keyboards.clone();
        with_lone.extend([lone_hid.clone(), lone_ps2.clone()]);
        assert_eq!(
            plan_set_standard(&with_lone, &global, Layout::Us, &[]),
            Err(OperationError::IncompleteValues {
                instance_ids: vec![lone_hid.instance_id.clone(), lone_ps2.instance_id.clone()]
            })
        );
        // A lone value on a read-only keyboard is not refused (never written).
        let mut read_only_lone = keyboards.clone();
        read_only_lone.push(KeyboardDevice {
            overrides: DeviceOverrides {
                keyboard_type_override: Some(4),
                ..Default::default()
            },
            ..read_only_hid()
        });
        assert!(plan_set_standard(&read_only_lone, &global, Layout::Us, &[]).is_ok());

        // A PS/2 keyboard pinned to a type without a table (7/0), connected or a phantom.
        for present in [true, false] {
            let ps2_7_0 = KeyboardDevice {
                present,
                overrides: DeviceOverrides {
                    override_keyboard_type: Some(7),
                    override_keyboard_subtype: Some(0),
                    ..Default::default()
                },
                ..fixtures::desktop_ps2()
            };
            let mut with = keyboards.clone();
            with[0] = ps2_7_0.clone();
            assert_eq!(
                plan_set_standard(&with, &global, Layout::Us, &[]),
                Err(OperationError::Ps2WithoutTable {
                    instance_ids: vec![ps2_7_0.instance_id.clone()]
                })
            );
            // The lone value comes first.
            let mut both = with.clone();
            both.push(lone_hid.clone());
            assert!(matches!(
                plan_set_standard(&both, &global, Layout::Us, &[]),
                Err(OperationError::IncompleteValues { .. })
            ));
        }

        // `--follow` of a keyboard without a follower: assigned, PS/2, Remote Desktop, another
        // driver's keyboard.
        let mut with_hyperkbd = keyboards.clone();
        with_hyperkbd.push(hyperkbd());
        for kb in [
            fixtures::desktop_keychron(),
            fixtures::desktop_ps2(),
            fixtures::desktop_rdp_keyboard(),
            hyperkbd(),
        ] {
            assert_eq!(
                plan_set_standard(
                    &with_hyperkbd,
                    &global,
                    Layout::Us,
                    &[kb.instance_id.to_ascii_lowercase()]
                ),
                Err(OperationError::NotFollowingStandard {
                    instance_id: kb.instance_id.clone()
                }),
                "{}",
                kb.instance_id
            );
        }
        assert_eq!(
            plan_set_standard(&keyboards, &global, Layout::Us, &[r"HID\NOPE\1".into()]),
            Err(OperationError::UnknownKeyboard {
                instance_id: r"HID\NOPE\1".into()
            })
        );
        // A PS/2 keyboard without a pin: INV-PS2 (check_plan).
        let mut unpinned = keyboards.clone();
        unpinned[0].overrides = DeviceOverrides::default();
        assert_eq!(
            plan_set_standard(&unpinned, &global, Layout::Us, &[]),
            Err(OperationError::Plan(PlanError::InvPs2(InvPs2Violation {
                keyboards: vec![fixtures::desktop_ps2().instance_id]
            })))
        );
        // The same standard writes nothing, so a lone value does not stop it.
        let mut lone_hid_only = keyboards.clone();
        lone_hid_only.push(lone_hid);
        assert!(
            plan_set_standard(&lone_hid_only, &global, Layout::Jis, &[])
                .unwrap()
                .change
                .is_empty()
        );
    }

    #[test]
    fn standard_guards_are_the_global_pair() {
        let guards = standard_guards();
        assert_eq!(
            guards.iter().map(ValueKey::canonical).collect::<Vec<_>>(),
            vec![
                "global|OverrideKeyboardType".to_string(),
                "global|OverrideKeyboardSubtype".to_string()
            ]
        );
        // The names the global allowlist knows, never written by a standard change.
        let (keyboards, global) = desktop();
        let change = set_standard_writes(&keyboards, &global, Layout::Us, &[]).unwrap();
        assert!(
            change
                .global_writes
                .iter()
                .all(|w| guards.iter().all(|g| g.name != w.name))
        );
    }
}
