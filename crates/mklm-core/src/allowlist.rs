//! Allowlist for registry writes (plan 1.5), enforced by the writer before anything is written.
//!
//! These rules govern new writes only. Restoring a journal baseline is validated against the journal.

use serde::{Deserialize, Serialize};

use crate::layout::{KBD101, KBD106, PCAT_101KEY, PCAT_106KEY};
use crate::model::{
    DeviceOverrides, GlobalMode, GlobalSettings, KeyboardDevice, KeyboardDriver, KeyboardType,
    Layout, Transport, value_names,
};
use crate::safety::{InvPs2Violation, check_inv_ps2};

/// Per-device type/subtype pairs MKLM may write (7/0 is added only once M0 #5 verifies it).
pub const DEVICE_TYPES: [KeyboardType; 2] = [KeyboardType::US, KeyboardType::JIS];
/// Global fixed-mode pairs MKLM may write.
pub const GLOBAL_FIXED_TYPES: [KeyboardType; 2] = [KeyboardType::JIS, KeyboardType::US_ON_JAPANESE];
/// `LayerDriver JPN` values MKLM may write.
pub const LAYER_DRIVERS: [&str; 2] = [KBD106, KBD101];
/// `OverrideKeyboardIdentifier` values MKLM may write.
pub const KEYBOARD_IDENTIFIERS: [&str; 2] = [PCAT_106KEY, PCAT_101KEY];

/// Operation on one registry value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueOp {
    /// Write a REG_DWORD.
    Set(u32),
    /// Write a REG_SZ.
    SetString(String),
    Delete,
}

/// One planned registry write: value name and operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedWrite {
    pub name: String,
    pub op: ValueOp,
}

impl PlannedWrite {
    pub fn set(name: &str, value: u32) -> Self {
        Self {
            name: name.to_string(),
            op: ValueOp::Set(value),
        }
    }

    pub fn set_string(name: &str, value: &str) -> Self {
        Self {
            name: name.to_string(),
            op: ValueOp::SetString(value.to_string()),
        }
    }

    pub fn delete(name: &str) -> Self {
        Self {
            name: name.to_string(),
            op: ValueOp::Delete,
        }
    }
}

/// Device writes that give a keyboard `layout`, or make it follow the standard when `layout` is `None`
/// (kbdhid only). `None` when the driver cannot take that assignment.
pub fn device_layout_writes(
    driver: &KeyboardDriver,
    layout: Option<Layout>,
) -> Option<Vec<PlannedWrite>> {
    let (type_name, subtype_name) = pair_names(driver).ok()?;
    match layout {
        Some(layout) => {
            let ty = layout.keyboard_type();
            Some(vec![
                PlannedWrite::set(type_name, ty.ty),
                PlannedWrite::set(subtype_name, ty.subtype),
            ])
        }
        None if *driver == KeyboardDriver::Kbdhid => Some(vec![
            PlannedWrite::delete(type_name),
            PlannedWrite::delete(subtype_name),
        ]),
        None => None,
    }
}

/// Global writes that make `standard` the PC's standard layout, keeping `OverrideKeyboardIdentifier` in step.
pub fn standard_layout_writes(standard: Layout) -> Vec<PlannedWrite> {
    vec![
        PlannedWrite::set_string(value_names::LAYER_DRIVER_JPN, standard.layer_driver()),
        PlannedWrite::set_string(
            value_names::KEYBOARD_IDENTIFIER,
            standard.keyboard_identifier(),
        ),
    ]
}

/// A planned write the allowlist rejects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AllowlistError {
    #[error("keyboards served by `{service}` are read-only")]
    ReadOnlyDriver { service: String },
    #[error("virtual keyboards are read-only")]
    VirtualKeyboard,
    #[error("`{name}` is not on the allowlist")]
    ValueNotAllowed { name: String },
    #[error("`{name}` is written more than once")]
    DuplicateValue { name: String },
    #[error("`{present}` must be written together with `{missing}`")]
    IncompletePair { present: String, missing: String },
    #[error("`{name}` and `{other}` must both be set or both be deleted")]
    MismatchedPair { name: String, other: String },
    #[error("`{name}`: {op:?} is not allowed")]
    OperationNotAllowed { name: String, op: ValueOp },
    #[error("type/subtype {keyboard_type} is not allowed here")]
    TypeNotAllowed { keyboard_type: KeyboardType },
    #[error("`{name}` may not be set to {value:?}")]
    StringNotAllowed { name: String, value: String },
    #[error("resulting global values are inconsistent: {reason}")]
    InconsistentGlobal { reason: String },
}

fn pair_names(driver: &KeyboardDriver) -> Result<(&'static str, &'static str), AllowlistError> {
    match driver {
        KeyboardDriver::Kbdhid => Ok((value_names::HID_TYPE, value_names::HID_SUBTYPE)),
        KeyboardDriver::I8042prt => Ok((value_names::PS2_TYPE, value_names::PS2_SUBTYPE)),
        KeyboardDriver::Other(service) => Err(AllowlistError::ReadOnlyDriver {
            service: service.clone(),
        }),
    }
}

/// Sorts the writes into slots for `allowed` names (exact, canonical spelling); rejects anything else.
fn collect_ops<'a, const N: usize>(
    writes: &'a [PlannedWrite],
    allowed: [&str; N],
) -> Result<[Option<&'a ValueOp>; N], AllowlistError> {
    let mut slots = [None; N];
    for write in writes {
        let index = allowed
            .iter()
            .position(|name| *name == write.name)
            .ok_or_else(|| AllowlistError::ValueNotAllowed {
                name: write.name.clone(),
            })?;
        if slots[index].replace(&write.op).is_some() {
            return Err(AllowlistError::DuplicateValue {
                name: write.name.clone(),
            });
        }
    }
    Ok(slots)
}

/// Resolved operation on a type/subtype pair.
enum PairOp {
    Untouched,
    Set(KeyboardType),
    Delete,
}

fn pair_op(
    (type_name, type_op): (&str, Option<&ValueOp>),
    (subtype_name, subtype_op): (&str, Option<&ValueOp>),
) -> Result<PairOp, AllowlistError> {
    for (name, op) in [(type_name, type_op), (subtype_name, subtype_op)] {
        if let Some(op @ ValueOp::SetString(_)) = op {
            return Err(AllowlistError::OperationNotAllowed {
                name: name.to_string(),
                op: op.clone(),
            });
        }
    }
    match (type_op, subtype_op) {
        (None, None) => Ok(PairOp::Untouched),
        (Some(ValueOp::Set(ty)), Some(ValueOp::Set(subtype))) => {
            Ok(PairOp::Set(KeyboardType::new(*ty, *subtype)))
        }
        (Some(ValueOp::Delete), Some(ValueOp::Delete)) => Ok(PairOp::Delete),
        (Some(_), None) => Err(AllowlistError::IncompletePair {
            present: type_name.to_string(),
            missing: subtype_name.to_string(),
        }),
        (None, Some(_)) => Err(AllowlistError::IncompletePair {
            present: subtype_name.to_string(),
            missing: type_name.to_string(),
        }),
        (Some(_), Some(_)) => Err(AllowlistError::MismatchedPair {
            name: type_name.to_string(),
            other: subtype_name.to_string(),
        }),
    }
}

/// Checks the writes planned for one keyboard and returns its values after them.
///
/// Allowed: the driver's own type/subtype names set together to 4/0 or 7/2, or (kbdhid only) both
/// deleted to follow the standard. PS/2 values are never deleted: without them i8042prt falls back
/// to the global values or US.
pub fn check_device_writes(
    device: &KeyboardDevice,
    writes: &[PlannedWrite],
) -> Result<DeviceOverrides, AllowlistError> {
    let (type_name, subtype_name) = pair_names(&device.driver)?;
    if device.transport == Transport::Virtual {
        return Err(AllowlistError::VirtualKeyboard);
    }
    let [type_op, subtype_op] = collect_ops(writes, [type_name, subtype_name])?;
    let mut after = device.overrides.clone();
    let is_hid = device.driver == KeyboardDriver::Kbdhid;
    let (type_slot, subtype_slot) = if is_hid {
        (
            &mut after.keyboard_type_override,
            &mut after.keyboard_subtype_override,
        )
    } else {
        (
            &mut after.override_keyboard_type,
            &mut after.override_keyboard_subtype,
        )
    };
    match pair_op((type_name, type_op), (subtype_name, subtype_op))? {
        PairOp::Untouched => {}
        PairOp::Set(ty) if DEVICE_TYPES.contains(&ty) => {
            *type_slot = Some(ty.ty);
            *subtype_slot = Some(ty.subtype);
        }
        PairOp::Set(ty) => return Err(AllowlistError::TypeNotAllowed { keyboard_type: ty }),
        PairOp::Delete if is_hid => {
            *type_slot = None;
            *subtype_slot = None;
        }
        PairOp::Delete => {
            return Err(AllowlistError::OperationNotAllowed {
                name: type_name.to_string(),
                op: ValueOp::Delete,
            });
        }
    }
    Ok(after)
}

fn check_string(name: &str, op: &ValueOp, allowed: &[&str]) -> Result<String, AllowlistError> {
    match op {
        ValueOp::SetString(value) if allowed.contains(&value.as_str()) => Ok(value.clone()),
        ValueOp::SetString(value) => Err(AllowlistError::StringNotAllowed {
            name: name.to_string(),
            value: value.clone(),
        }),
        op => Err(AllowlistError::OperationNotAllowed {
            name: name.to_string(),
            op: op.clone(),
        }),
    }
}

/// The standard layout as stored: an allowlisted `LayerDriver JPN` with its matching identifier.
/// Never inferred from missing values.
fn explicit_standard(global: &GlobalSettings) -> Option<Layout> {
    let layer_driver = global.layer_driver_jpn.as_deref()?;
    let identifier = global.override_keyboard_identifier.as_deref()?;
    [Layout::Jis, Layout::Us].into_iter().find(|layout| {
        layer_driver.eq_ignore_ascii_case(layout.layer_driver())
            && identifier.eq_ignore_ascii_case(layout.keyboard_identifier())
    })
}

/// Checks the writes planned for `i8042prt\Parameters` and returns the global values after them.
///
/// Allowed: `LayerDriver JPN` ∈ {kbd106.dll, kbd101.dll}, `OverrideKeyboardIdentifier` ∈
/// {PCAT_106KEY, PCAT_101KEY}, and the global type/subtype deleted together or set to 7/2 or 7/0.
/// Nothing else (`Start`, `LayerDriver KOR`, ...) may be written or deleted. A non-empty batch must
/// leave consistent values: when it touches the standard, the layer driver and identifier must
/// match; when it touches the type or subtype, a fixed-mode pair needs both of them stored and must
/// be the one the Settings app pairs with the standard (JIS 7/2, US 7/0); and a lone global type or
/// subtype must not remain.
pub fn check_global_writes(
    current: &GlobalSettings,
    writes: &[PlannedWrite],
) -> Result<GlobalSettings, AllowlistError> {
    let [jpn_op, identifier_op, type_op, subtype_op] = collect_ops(
        writes,
        [
            value_names::LAYER_DRIVER_JPN,
            value_names::KEYBOARD_IDENTIFIER,
            value_names::PS2_TYPE,
            value_names::PS2_SUBTYPE,
        ],
    )?;
    let mut after = current.clone();
    if let Some(op) = jpn_op {
        after.layer_driver_jpn = Some(check_string(
            value_names::LAYER_DRIVER_JPN,
            op,
            &LAYER_DRIVERS,
        )?);
    }
    if let Some(op) = identifier_op {
        after.override_keyboard_identifier = Some(check_string(
            value_names::KEYBOARD_IDENTIFIER,
            op,
            &KEYBOARD_IDENTIFIERS,
        )?);
    }
    match pair_op(
        (value_names::PS2_TYPE, type_op),
        (value_names::PS2_SUBTYPE, subtype_op),
    )? {
        PairOp::Untouched => {}
        PairOp::Set(ty) if GLOBAL_FIXED_TYPES.contains(&ty) => {
            after.override_keyboard_type = Some(ty.ty);
            after.override_keyboard_subtype = Some(ty.subtype);
        }
        PairOp::Set(ty) => return Err(AllowlistError::TypeNotAllowed { keyboard_type: ty }),
        PairOp::Delete => {
            after.override_keyboard_type = None;
            after.override_keyboard_subtype = None;
        }
    }

    if writes.is_empty() {
        return Ok(after);
    }
    let touches_standard = jpn_op.is_some() || identifier_op.is_some();
    let standard = explicit_standard(&after);
    if touches_standard && standard.is_none() {
        return Err(AllowlistError::InconsistentGlobal {
            reason: format!(
                "LayerDriver JPN {:?} and OverrideKeyboardIdentifier {:?} do not match",
                after.layer_driver_jpn, after.override_keyboard_identifier
            ),
        });
    }
    if after.mode() == GlobalMode::Fixed && after.fixed_type().is_none() {
        return Err(AllowlistError::InconsistentGlobal {
            reason: format!(
                "only one of {} and {} would remain",
                value_names::PS2_TYPE,
                value_names::PS2_SUBTYPE
            ),
        });
    }
    if (touches_standard || type_op.is_some())
        && let Some(fixed) = after.fixed_type()
        && standard.map(Layout::fixed_mode_type) != Some(fixed)
    {
        return Err(AllowlistError::InconsistentGlobal {
            reason: format!(
                "fixed-mode type {fixed} does not match LayerDriver JPN {:?} and \
                 OverrideKeyboardIdentifier {:?}",
                after.layer_driver_jpn, after.override_keyboard_identifier
            ),
        });
    }
    Ok(after)
}

/// Layout to pin every i8042prt keyboard to before the global pair is deleted (plan 1.3, step 2):
/// the one the fixed mode gives them now. Fixed JIS (7/2) pins JIS; fixed US (7/0) pins US, which
/// is written as 4/0 because 7/0 is not allowed per device. `None` outside fixed mode or when the
/// global values are inconsistent, so that the user decides.
pub fn ps2_pin_layout(global: &GlobalSettings) -> Option<Layout> {
    let fixed = global.fixed_type()?;
    let standard = explicit_standard(global)?;
    (standard.fixed_mode_type() == fixed).then_some(standard)
}

/// A plan the safety rules reject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PlanError {
    #[error("{instance_id}: {error}")]
    Device {
        instance_id: String,
        error: AllowlistError,
    },
    #[error("i8042prt\\Parameters: {error}")]
    Global { error: AllowlistError },
    #[error("{instance_id}: no such keyboard")]
    UnknownKeyboard { instance_id: String },
    #[error("{instance_id}: planned more than once")]
    DuplicateKeyboard { instance_id: String },
    #[error(transparent)]
    InvPs2(#[from] InvPs2Violation),
}

/// Registry key a [`PlanStep`] writes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum WriteTarget {
    /// A keyboard's "Device Parameters".
    Device { instance_id: String },
    /// `Services\i8042prt\Parameters`.
    Global,
}

/// The writes to one registry key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub target: WriteTarget,
    pub writes: Vec<PlannedWrite>,
}

/// Values after a plan passed [`check_plan`], and the order to write them in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedPlan {
    pub global: GlobalSettings,
    /// Every keyboard with its values after the plan.
    pub keyboards: Vec<KeyboardDevice>,
    /// The writes in the order the writer must carry them out (plan 1.3): i8042prt keyboards, then
    /// the other keyboards, then the global values. Keys without writes are left out. No step turns
    /// a state that satisfies INV-PS2 into one that violates it, so stopping halfway is safe.
    pub steps: Vec<PlanStep>,
}

/// Checks a whole plan: each keyboard's writes and the global writes against the allowlist, then
/// INV-PS2 after every step of [`CheckedPlan::steps`].
///
/// `keyboards` must include every keyboard (phantoms too), so INV-PS2 sees every i8042prt devnode.
/// Every plan whose result violates INV-PS2 is rejected (plan 1.5), including a plan that writes
/// only HID values while a violation already exists: it must also pin the i8042prt keyboards
/// (see [`ps2_pin_layout`]).
pub fn check_plan(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    device_writes: &[(String, Vec<PlannedWrite>)],
    global_writes: &[PlannedWrite],
) -> Result<CheckedPlan, PlanError> {
    let mut after_keyboards = keyboards.to_vec();
    let mut planned = vec![false; keyboards.len()];
    // (keyboard index or `None` for the global key, step), i8042prt keyboards first.
    let mut ps2_steps = Vec::new();
    let mut other_steps = Vec::new();
    for (instance_id, writes) in device_writes {
        let index = keyboards
            .iter()
            .position(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
            .ok_or_else(|| PlanError::UnknownKeyboard {
                instance_id: instance_id.clone(),
            })?;
        if std::mem::replace(&mut planned[index], true) {
            return Err(PlanError::DuplicateKeyboard {
                instance_id: instance_id.clone(),
            });
        }
        let device = &keyboards[index];
        after_keyboards[index].overrides =
            check_device_writes(device, writes).map_err(|error| PlanError::Device {
                instance_id: device.instance_id.clone(),
                error,
            })?;
        if !writes.is_empty() {
            let step = PlanStep {
                target: WriteTarget::Device {
                    instance_id: device.instance_id.clone(),
                },
                writes: writes.clone(),
            };
            if device.driver == KeyboardDriver::I8042prt {
                ps2_steps.push((Some(index), step));
            } else {
                other_steps.push((Some(index), step));
            }
        }
    }
    let after_global =
        check_global_writes(global, global_writes).map_err(|error| PlanError::Global { error })?;
    let mut steps = ps2_steps;
    steps.append(&mut other_steps);
    if !global_writes.is_empty() {
        let step = PlanStep {
            target: WriteTarget::Global,
            writes: global_writes.to_vec(),
        };
        steps.push((None, step));
    }

    // Replay the steps: none may break INV-PS2, and it must hold at the end.
    let mut state_keyboards = keyboards.to_vec();
    let mut state_global = global.clone();
    let mut violation = check_inv_ps2(&state_global, &state_keyboards).err();
    for (index, _) in &steps {
        match index {
            Some(index) => {
                state_keyboards[*index]
                    .overrides
                    .clone_from(&after_keyboards[*index].overrides);
            }
            None => state_global.clone_from(&after_global),
        }
        let now = check_inv_ps2(&state_global, &state_keyboards).err();
        if violation.is_none()
            && let Some(broken) = now
        {
            return Err(broken.into());
        }
        violation = now;
    }
    if let Some(violation) = violation {
        return Err(violation.into());
    }
    Ok(CheckedPlan {
        global: after_global,
        keyboards: after_keyboards,
        steps: steps.into_iter().map(|(_, step)| step).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::value_names::*;

    fn bare_ps2() -> KeyboardDevice {
        KeyboardDevice {
            overrides: DeviceOverrides::default(),
            ..fixtures::internal_ps2()
        }
    }

    #[test]
    fn hid_assignments() {
        let kb = fixtures::vxe_ble();
        let us = device_layout_writes(&kb.driver, Some(Layout::Us)).unwrap();
        assert_eq!(
            us,
            vec![
                PlannedWrite::set(HID_TYPE, 4),
                PlannedWrite::set(HID_SUBTYPE, 0)
            ]
        );
        assert_eq!(
            check_device_writes(&kb, &us).unwrap().hid_type(),
            Some(KeyboardType::US)
        );

        let jis = device_layout_writes(&kb.driver, Some(Layout::Jis)).unwrap();
        assert_eq!(
            check_device_writes(&kb, &jis).unwrap().hid_type(),
            Some(KeyboardType::JIS)
        );

        let follow = device_layout_writes(&kb.driver, None).unwrap();
        let after = check_device_writes(&fixtures::keychron(), &follow).unwrap();
        assert!(after.is_empty());
        assert_eq!(check_device_writes(&kb, &[]).unwrap(), kb.overrides);
    }

    #[test]
    fn ps2_assignments() {
        let kb = bare_ps2();
        let jis = device_layout_writes(&kb.driver, Some(Layout::Jis)).unwrap();
        assert_eq!(
            jis,
            vec![
                PlannedWrite::set(PS2_TYPE, 7),
                PlannedWrite::set(PS2_SUBTYPE, 2)
            ]
        );
        assert_eq!(
            check_device_writes(&kb, &jis).unwrap().ps2_type(),
            Some(KeyboardType::JIS)
        );
        assert_eq!(device_layout_writes(&kb.driver, None), None);
        assert_eq!(
            check_device_writes(
                &fixtures::internal_ps2(),
                &[
                    PlannedWrite::delete(PS2_TYPE),
                    PlannedWrite::delete(PS2_SUBTYPE)
                ]
            ),
            Err(AllowlistError::OperationNotAllowed {
                name: PS2_TYPE.into(),
                op: ValueOp::Delete
            })
        );
    }

    #[test]
    fn rejects_wrong_names_types_and_shapes() {
        let hid = fixtures::keychron();
        // Other stack's names.
        assert_eq!(
            check_device_writes(
                &hid,
                &[
                    PlannedWrite::set(PS2_TYPE, 4),
                    PlannedWrite::set(PS2_SUBTYPE, 0)
                ]
            ),
            Err(AllowlistError::ValueNotAllowed {
                name: PS2_TYPE.into()
            })
        );
        assert_eq!(
            check_device_writes(
                &bare_ps2(),
                &[
                    PlannedWrite::set(HID_TYPE, 7),
                    PlannedWrite::set(HID_SUBTYPE, 2)
                ]
            ),
            Err(AllowlistError::ValueNotAllowed {
                name: HID_TYPE.into()
            })
        );
        // Preserved-only extras and non-canonical spelling.
        assert!(matches!(
            check_device_writes(&hid, &[PlannedWrite::set(HID_TOTAL_KEYS, 104)]),
            Err(AllowlistError::ValueNotAllowed { .. })
        ));
        assert!(matches!(
            check_device_writes(
                &hid,
                &[
                    PlannedWrite::set("keyboardtypeoverride", 4),
                    PlannedWrite::set(HID_SUBTYPE, 0)
                ]
            ),
            Err(AllowlistError::ValueNotAllowed { .. })
        ));
        // 7/0 and anything else.
        for (ty, sub) in [(7, 0), (0x51, 0), (4, 2), (7, 0xD02)] {
            assert_eq!(
                check_device_writes(
                    &hid,
                    &[
                        PlannedWrite::set(HID_TYPE, ty),
                        PlannedWrite::set(HID_SUBTYPE, sub)
                    ]
                ),
                Err(AllowlistError::TypeNotAllowed {
                    keyboard_type: KeyboardType::new(ty, sub)
                })
            );
        }
        // Half pairs, mixed ops, strings, duplicates.
        assert_eq!(
            check_device_writes(&hid, &[PlannedWrite::set(HID_TYPE, 4)]),
            Err(AllowlistError::IncompletePair {
                present: HID_TYPE.into(),
                missing: HID_SUBTYPE.into()
            })
        );
        assert_eq!(
            check_device_writes(
                &hid,
                &[
                    PlannedWrite::set(HID_TYPE, 4),
                    PlannedWrite::delete(HID_SUBTYPE)
                ]
            ),
            Err(AllowlistError::MismatchedPair {
                name: HID_TYPE.into(),
                other: HID_SUBTYPE.into()
            })
        );
        assert!(matches!(
            check_device_writes(
                &hid,
                &[
                    PlannedWrite::set_string(HID_TYPE, "4"),
                    PlannedWrite::set(HID_SUBTYPE, 0)
                ]
            ),
            Err(AllowlistError::OperationNotAllowed { .. })
        ));
        assert_eq!(
            check_device_writes(
                &hid,
                &[
                    PlannedWrite::set(HID_TYPE, 4),
                    PlannedWrite::set(HID_SUBTYPE, 0),
                    PlannedWrite::set(HID_TYPE, 4)
                ]
            ),
            Err(AllowlistError::DuplicateValue {
                name: HID_TYPE.into()
            })
        );
    }

    #[test]
    fn read_only_keyboards() {
        let rdp = KeyboardDevice {
            driver: KeyboardDriver::Other("TermDD".into()),
            transport: Transport::Virtual,
            ..fixtures::keychron()
        };
        assert_eq!(
            check_device_writes(
                &rdp,
                &device_layout_writes(&KeyboardDriver::Kbdhid, Some(Layout::Us)).unwrap()
            ),
            Err(AllowlistError::ReadOnlyDriver {
                service: "TermDD".into()
            })
        );
        assert_eq!(device_layout_writes(&rdp.driver, Some(Layout::Us)), None);
        let vhf = KeyboardDevice {
            transport: Transport::Virtual,
            ..fixtures::keychron()
        };
        assert_eq!(
            check_device_writes(&vhf, &[]),
            Err(AllowlistError::VirtualKeyboard)
        );
    }

    #[test]
    fn global_migration_and_standard_changes() {
        let fixed = fixtures::global_fixed_jis();
        let migrate = [
            PlannedWrite::delete(PS2_TYPE),
            PlannedWrite::delete(PS2_SUBTYPE),
        ];
        assert_eq!(
            check_global_writes(&fixed, &migrate).unwrap(),
            fixtures::global_per_keyboard()
        );

        let mut to_us = standard_layout_writes(Layout::Us);
        let after = check_global_writes(&fixtures::global_per_keyboard(), &to_us).unwrap();
        assert_eq!(after.layer_driver_jpn.as_deref(), Some("kbd101.dll"));
        assert_eq!(
            after.override_keyboard_identifier.as_deref(),
            Some("PCAT_101KEY")
        );

        // Changing the standard in fixed mode needs the matching fixed pair too.
        assert!(matches!(
            check_global_writes(&fixed, &to_us),
            Err(AllowlistError::InconsistentGlobal { .. })
        ));
        to_us.extend([
            PlannedWrite::set(PS2_TYPE, 7),
            PlannedWrite::set(PS2_SUBTYPE, 0),
        ]);
        assert_eq!(
            check_global_writes(&fixed, &to_us).unwrap().fixed_type(),
            Some(KeyboardType::US_ON_JAPANESE)
        );

        // Back to fixed JIS.
        let back = [
            PlannedWrite::set(PS2_TYPE, 7),
            PlannedWrite::set(PS2_SUBTYPE, 2),
        ];
        assert_eq!(
            check_global_writes(&fixtures::global_per_keyboard(), &back).unwrap(),
            fixed
        );
    }

    #[test]
    fn global_rejections() {
        let g = fixtures::global_per_keyboard();
        assert_eq!(
            check_global_writes(&g, &[PlannedWrite::set("Start", 1)]),
            Err(AllowlistError::ValueNotAllowed {
                name: "Start".into()
            })
        );
        assert!(matches!(
            check_global_writes(&g, &[PlannedWrite::delete("Start")]),
            Err(AllowlistError::ValueNotAllowed { .. })
        ));
        assert!(matches!(
            check_global_writes(
                &g,
                &[PlannedWrite::set_string(LAYER_DRIVER_KOR, "kbd101.dll")]
            ),
            Err(AllowlistError::ValueNotAllowed { .. })
        ));
        assert_eq!(
            check_global_writes(
                &g,
                &[
                    PlannedWrite::set_string(LAYER_DRIVER_JPN, "kbdnec.dll"),
                    PlannedWrite::set_string(KEYBOARD_IDENTIFIER, "PCAT_106KEY")
                ]
            ),
            Err(AllowlistError::StringNotAllowed {
                name: LAYER_DRIVER_JPN.into(),
                value: "kbdnec.dll".into()
            })
        );
        assert!(matches!(
            check_global_writes(&g, &[PlannedWrite::delete(LAYER_DRIVER_JPN)]),
            Err(AllowlistError::OperationNotAllowed { .. })
        ));
        assert!(matches!(
            check_global_writes(
                &g,
                &[PlannedWrite::set_string(KEYBOARD_IDENTIFIER, "PCAT_101KEY")]
            ),
            Err(AllowlistError::InconsistentGlobal { .. })
        ));
        assert_eq!(
            check_global_writes(
                &g,
                &[
                    PlannedWrite::set(PS2_TYPE, 4),
                    PlannedWrite::set(PS2_SUBTYPE, 0)
                ]
            ),
            Err(AllowlistError::TypeNotAllowed {
                keyboard_type: KeyboardType::US
            })
        );
        assert!(matches!(
            check_global_writes(&g, &[PlannedWrite::delete(PS2_TYPE)]),
            Err(AllowlistError::IncompletePair { .. })
        ));
        // 7/0 with a JIS standard is not what the Settings app writes.
        assert!(matches!(
            check_global_writes(
                &g,
                &[
                    PlannedWrite::set(PS2_TYPE, 7),
                    PlannedWrite::set(PS2_SUBTYPE, 0)
                ]
            ),
            Err(AllowlistError::InconsistentGlobal { .. })
        ));
        assert_eq!(check_global_writes(&g, &[]).unwrap(), g);
    }

    #[test]
    fn global_writes_never_leave_a_lone_type() {
        let lone = GlobalSettings {
            override_keyboard_type: Some(7),
            ..fixtures::global_per_keyboard()
        };
        assert!(matches!(
            check_global_writes(&lone, &standard_layout_writes(Layout::Us)),
            Err(AllowlistError::InconsistentGlobal { .. })
        ));
        // The batch has to set or delete both.
        let mut repair = standard_layout_writes(Layout::Us);
        repair.extend([
            PlannedWrite::delete(PS2_TYPE),
            PlannedWrite::delete(PS2_SUBTYPE),
        ]);
        assert_eq!(
            check_global_writes(&lone, &repair).unwrap().mode(),
            GlobalMode::PerKeyboard
        );
    }

    #[test]
    fn fixed_pair_needs_a_stored_standard() {
        // A missing LayerDriver JPN reads as US, but must not let 7/0 through on its own.
        let fixed_us = [
            PlannedWrite::set(PS2_TYPE, 7),
            PlannedWrite::set(PS2_SUBTYPE, 0),
        ];
        assert!(matches!(
            check_global_writes(&GlobalSettings::default(), &fixed_us),
            Err(AllowlistError::InconsistentGlobal { .. })
        ));
        let identifier_missing = GlobalSettings {
            layer_driver_jpn: Some("kbd106.dll".into()),
            ..Default::default()
        };
        assert!(matches!(
            check_global_writes(
                &identifier_missing,
                &[
                    PlannedWrite::set(PS2_TYPE, 7),
                    PlannedWrite::set(PS2_SUBTYPE, 2)
                ]
            ),
            Err(AllowlistError::InconsistentGlobal { .. })
        ));
        let mut with_standard = standard_layout_writes(Layout::Us);
        with_standard.extend(fixed_us);
        let after = check_global_writes(&GlobalSettings::default(), &with_standard).unwrap();
        assert_eq!(after.fixed_type(), Some(KeyboardType::US_ON_JAPANESE));
        assert!(crate::global::global_anomalies(&after).is_empty());
    }

    #[test]
    fn migration_plan_is_checked_for_inv_ps2_and_ordered() {
        let fixed = fixtures::global_fixed_jis();
        let before = vec![fixtures::keychron(), bare_ps2()];
        let delete_global = vec![
            PlannedWrite::delete(PS2_TYPE),
            PlannedWrite::delete(PS2_SUBTYPE),
        ];

        // Deleting the global pair while the PS/2 keyboard has no values violates INV-PS2.
        let err = check_plan(&before, &fixed, &[], &delete_global).unwrap_err();
        assert!(matches!(err, PlanError::InvPs2(_)));

        // Pinning the PS/2 keyboard to the fixed layout in the same plan makes it safe.
        assert_eq!(ps2_pin_layout(&fixed), Some(Layout::Jis));
        let pin = device_layout_writes(&KeyboardDriver::I8042prt, Some(Layout::Jis)).unwrap();
        let us = device_layout_writes(&KeyboardDriver::Kbdhid, Some(Layout::Us)).unwrap();
        let plan = vec![
            (before[0].instance_id.clone(), us.clone()),
            (before[1].instance_id.clone(), pin.clone()),
        ];
        let checked = check_plan(&before, &fixed, &plan, &delete_global).unwrap();
        assert_eq!(checked.global.fixed_type(), None);
        assert_eq!(
            checked.keyboards[1].overrides.ps2_type(),
            Some(KeyboardType::JIS)
        );

        // Plan 1.3 order, whatever order the plan listed: PS/2, then assignments, then global.
        let device = |kb: &KeyboardDevice| WriteTarget::Device {
            instance_id: kb.instance_id.clone(),
        };
        assert_eq!(
            checked.steps,
            vec![
                PlanStep {
                    target: device(&before[1]),
                    writes: pin
                },
                PlanStep {
                    target: device(&before[0]),
                    writes: us
                },
                PlanStep {
                    target: WriteTarget::Global,
                    writes: delete_global
                },
            ]
        );
    }

    #[test]
    fn migration_must_pin_phantom_ps2_keyboards() {
        let fixed = fixtures::global_fixed_jis();
        let phantom = KeyboardDevice {
            instance_id: r"ACPI\PNP0303\4&1&0".into(),
            present: false,
            reported_type: None,
            dev_node_status: None,
            problem_code: None,
            ..bare_ps2()
        };
        let keyboards = vec![bare_ps2(), phantom.clone()];
        let pin = device_layout_writes(&KeyboardDriver::I8042prt, Some(Layout::Jis)).unwrap();
        let delete_global = [
            PlannedWrite::delete(PS2_TYPE),
            PlannedWrite::delete(PS2_SUBTYPE),
        ];
        let present_only = vec![(keyboards[0].instance_id.clone(), pin.clone())];
        assert_eq!(
            check_plan(&keyboards, &fixed, &present_only, &delete_global).unwrap_err(),
            PlanError::InvPs2(InvPs2Violation {
                keyboards: vec![phantom.instance_id.clone()]
            })
        );
        let both = vec![
            (keyboards[0].instance_id.clone(), pin.clone()),
            (phantom.instance_id.clone(), pin),
        ];
        assert!(check_plan(&keyboards, &fixed, &both, &delete_global).is_ok());
    }

    #[test]
    fn migration_from_fixed_us_pins_us() {
        let fixed_us = GlobalSettings {
            layer_driver_jpn: Some("kbd101.dll".into()),
            override_keyboard_identifier: Some("PCAT_101KEY".into()),
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(0),
            ..Default::default()
        };
        // 7/0 itself is not allowed per device; the pin is US, written as 4/0.
        assert!(matches!(
            check_device_writes(
                &bare_ps2(),
                &[
                    PlannedWrite::set(PS2_TYPE, 7),
                    PlannedWrite::set(PS2_SUBTYPE, 0)
                ]
            ),
            Err(AllowlistError::TypeNotAllowed { .. })
        ));
        let layout = ps2_pin_layout(&fixed_us).unwrap();
        assert_eq!(layout, Layout::Us);
        let keyboards = vec![bare_ps2()];
        let plan = vec![(
            keyboards[0].instance_id.clone(),
            device_layout_writes(&KeyboardDriver::I8042prt, Some(layout)).unwrap(),
        )];
        let delete_global = [
            PlannedWrite::delete(PS2_TYPE),
            PlannedWrite::delete(PS2_SUBTYPE),
        ];
        let checked = check_plan(&keyboards, &fixed_us, &plan, &delete_global).unwrap();
        assert_eq!(
            checked.keyboards[0].overrides.ps2_type(),
            Some(KeyboardType::US)
        );

        // No pin outside fixed mode or with inconsistent globals.
        assert_eq!(ps2_pin_layout(&fixtures::global_per_keyboard()), None);
        let mismatch = GlobalSettings {
            override_keyboard_subtype: Some(2),
            ..fixed_us
        };
        assert_eq!(ps2_pin_layout(&mismatch), None);
    }

    #[test]
    fn plans_leaving_inv_ps2_violated_are_rejected() {
        let per_keyboard = fixtures::global_per_keyboard();
        let keyboards = vec![bare_ps2(), fixtures::vxe_ble()];
        let vxe = keyboards[1].instance_id.to_ascii_lowercase();
        let us = device_layout_writes(&KeyboardDriver::Kbdhid, Some(Layout::Us)).unwrap();
        let plan = vec![(vxe, us)];
        // HID-only, but the PS/2 keyboard would still turn US at the next restart.
        assert!(matches!(
            check_plan(&keyboards, &per_keyboard, &plan, &[]),
            Err(PlanError::InvPs2(_))
        ));
        assert!(matches!(
            check_plan(&keyboards, &per_keyboard, &[], &[]),
            Err(PlanError::InvPs2(_))
        ));
        let err = check_plan(
            &keyboards,
            &per_keyboard,
            &[],
            &standard_layout_writes(Layout::Jis),
        )
        .unwrap_err();
        assert!(matches!(err, PlanError::InvPs2(_)));

        // Repairing the violation in the same plan makes it acceptable.
        let mut repaired = plan.clone();
        repaired.push((
            keyboards[0].instance_id.clone(),
            device_layout_writes(&KeyboardDriver::I8042prt, Some(Layout::Jis)).unwrap(),
        ));
        let checked = check_plan(&keyboards, &per_keyboard, &repaired, &[]).unwrap();
        assert_eq!(
            checked.keyboards[1].overrides.hid_type(),
            Some(KeyboardType::US)
        );
        // Going back to fixed mode also repairs it.
        let back = [
            PlannedWrite::set(PS2_TYPE, 7),
            PlannedWrite::set(PS2_SUBTYPE, 2),
        ];
        assert!(check_plan(&keyboards, &per_keyboard, &plan, &back).is_ok());
    }

    #[test]
    fn plan_errors() {
        let keyboards = vec![fixtures::keychron()];
        let g = fixtures::global_per_keyboard();
        assert_eq!(
            check_plan(&keyboards, &g, &[("HID\\NOPE\\1".into(), vec![])], &[]).unwrap_err(),
            PlanError::UnknownKeyboard {
                instance_id: "HID\\NOPE\\1".into()
            }
        );
        let id = keyboards[0].instance_id.clone();
        assert!(matches!(
            check_plan(
                &keyboards,
                &g,
                &[(id.clone(), vec![]), (id.clone(), vec![])],
                &[]
            ),
            Err(PlanError::DuplicateKeyboard { .. })
        ));
        let err = check_plan(
            &keyboards,
            &g,
            &[(id, vec![PlannedWrite::set(HID_TYPE, 7)])],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, PlanError::Device { .. }));
        assert!(err.to_string().contains("KeyboardTypeOverride"));
        assert!(matches!(
            check_plan(&keyboards, &g, &[], &[PlannedWrite::delete("Start")]),
            Err(PlanError::Global { .. })
        ));
    }
}
