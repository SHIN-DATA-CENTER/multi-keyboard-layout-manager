//! Plans that put recorded values back: revert, rollback, restore to baseline and conflict
//! resolutions (plan 1.3, 2.3).
//!
//! The allowlist ([`crate::allowlist`]) governs new values; restores are validated against the
//! journal instead: only recorded (target, name) pairs, only names MKLM may write for that target,
//! and only values the journal holds.
//!
//! **Order follows the direction of the change, not a fixed sequence** (design review C4). A fixed
//! "global → others → i8042prt" order is right only when going back to fixed mode; restoring
//! towards per-keyboard mode (undoing a restore-to-baseline, or a resolution that keeps a
//! migration) must pin the PS/2 keyboards before the global pair goes. Every step is put in one of
//! the [`RestorePhase`]s, which run in declaration order:
//!
//! 1. pins added or changed on i8042prt keyboards (both device values present afterwards);
//! 2. the global key, when the fixed pair is present afterwards (added or kept);
//! 3. every other keyboard (HID), except those of phase 4b;
//! 4. the global key, when the fixed pair is absent afterwards (removed or still absent);
//!    4b. in a restore that stays in per-keyboard mode (the global pair absent before and after
//!    it), the HID keyboards whose writes leave them without a table of their own: they follow
//!    the standard afterwards, so the standard is put back first (design standard-layout B.6);
//! 5. pins removed from i8042prt keyboards.
//!
//! If the end state satisfies INV-PS2, no intermediate state breaks it: pins only grow before the
//! pair is removed (phase 4), and the pair is present whenever a pin is removed (phase 5). The plan
//! is still replayed step by step and checked, like [`crate::check_plan`] does for new writes.
//!
//! "Before" the plan, the global pair is what the records of the pair expect ([`Expect`]; the
//! current value for [`Expect::Any`] or without such a record), so that a pair written by someone
//! else keeps the order the journal planned: a standard change is then still undone standard
//! first, and the global step stops at the conflict before any pin is removed.
//!
//! For INV-PS2 a value only counts as a pin or as part of the global pair when it is a `REG_DWORD`:
//! a [`RegValue::Other`] baseline (say, a `REG_SZ` "7") is restored byte for byte but is not
//! assumed to pin anything.

use serde::{Deserialize, Serialize};

use crate::allowlist::{DEVICE_VALUE_NAMES, GLOBAL_VALUE_NAMES, WriteTarget, unread_value_names};
use crate::device::predict_type;
use crate::journal::{
    BaselineRecord, RegValue, ValueKey, ValueRecord, is_ps2_value_name, same_target, value_eq,
};
use crate::model::{DeviceOverrides, GlobalSettings, KeyboardDevice, KeyboardDriver, value_names};
use crate::safety::{InvPs2Violation, check_inv_ps2};

/// Which recorded value a restore writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreTo {
    /// `before`: revert one operation, or roll back a partial one.
    Before,
    /// `baseline`: "MKLM 導入前に戻す".
    Baseline,
    /// `resolve_to`: a conflict resolution (design review C5).
    Resolution,
}

impl RestoreTo {
    /// The value `record` is restored to, or `None` for a resolution without `resolve_to`.
    pub fn value(self, record: &ValueRecord) -> Option<&RegValue> {
        match self {
            RestoreTo::Before => Some(&record.before),
            RestoreTo::Baseline => Some(&record.baseline),
            RestoreTo::Resolution => record.resolve_to.as_ref(),
        }
    }
}

/// What the current value must be for a restore write to go ahead (compare-and-swap). A current
/// value that already equals the restore value ([`crate::value_eq`]) is always skipped as done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Expect {
    /// Must equal this value: `last_written` (revert), `intended` (rollback of an entry that never
    /// reached `Written`), the value the user saw in the conflict (resolution), or the latest
    /// record's `last_written` (restore to baseline).
    Value { value: RegValue },
    /// Anything: only for restore-to-baseline with [`crate::ConflictPolicy::Overwrite`], where the
    /// user chose to overwrite every conflict after seeing the values. Resolutions never use it.
    Any,
}

/// One value to put back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreWrite {
    /// Index into the records the plan was built from.
    pub record: usize,
    pub name: String,
    pub value: RegValue,
    pub expect: Expect,
}

/// Position of a step in the restore order (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestorePhase {
    AddPins,
    GlobalWithPair,
    Other,
    GlobalWithoutPair,
    /// A restore that stays in per-keyboard mode (no global pair before or after it): the HID
    /// keyboards left without a table of their own by their writes (a pin removed, 7/0 put back).
    /// They follow the standard afterwards, so they go after the global values; a standard change
    /// is then undone standard first, pins last (design standard-layout B.6, SAFETY-1).
    FollowStandard,
    RemovePins,
}

/// The restore writes to one registry key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreStep {
    pub phase: RestorePhase,
    pub target: WriteTarget,
    pub writes: Vec<RestoreWrite>,
}

/// Restore steps in execution order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePlan {
    pub steps: Vec<RestoreStep>,
    /// Set when the end state violates INV-PS2. Allowed only when the plan writes none of the
    /// values INV-PS2 reads (it cannot change the violation it reports), or when that end state
    /// has every i8042prt pin and the global type/subtype pair at its recorded baseline, values
    /// outside the plan included (MKLM puts back a pre-existing violation, and says so); every
    /// other violating plan is rejected.
    pub restores_inv_ps2_violation: Option<InvPs2Violation>,
}

/// A restore the journal rules reject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RestoreError {
    #[error("{name:?} is not a value MKLM writes on {target:?}")]
    NameNotAllowed { target: WriteTarget, name: String },
    #[error("record {record} has no value to compare against")]
    NothingToCompare { record: usize },
    #[error("record {record} has no resolution value")]
    NoResolution { record: usize },
    #[error("a restore step would break INV-PS2: {0}")]
    InvPs2(InvPs2Violation),
}

const HID_NAMES: [&str; 2] = [value_names::HID_TYPE, value_names::HID_SUBTYPE];
const PS2_NAMES: [&str; 2] = [value_names::PS2_TYPE, value_names::PS2_SUBTYPE];

fn find_keyboard(keyboards: &[KeyboardDevice], instance_id: &str) -> Option<usize> {
    keyboards
        .iter()
        .position(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
}

/// Checks one record against the journal rules: device names must be the type/subtype pair of the
/// keyboard's driver, or the other stack's pair that it does not read (what `CleanupValues`
/// deleted and a revert or a restore to baseline puts back, design m3 A.5), either stack's pair
/// when the devnode is gone, and nothing on a devnode of another driver; global names must be one
/// of the four global names ([`crate::GLOBAL_VALUE_NAMES`]); and [`RegValue::Other`] may only be
/// restored as the record's `baseline` (or as a `resolve_to` equal to the baseline or to the value
/// the user saw).
///
/// Names compare exactly (MKLM records the canonical spelling). A value MKLM may not write (an
/// `Other` that is not the baseline) is reported as [`RestoreError::NameNotAllowed`] for that
/// name. Errors name record 0; [`plan_restore`] reports the record's real index.
pub fn check_restore_record(
    record: &ValueRecord,
    keyboards: &[KeyboardDevice],
    to: RestoreTo,
) -> Result<(), RestoreError> {
    check_record(0, record, keyboards, to)
}

fn check_record(
    index: usize,
    record: &ValueRecord,
    keyboards: &[KeyboardDevice],
    to: RestoreTo,
) -> Result<(), RestoreError> {
    let not_allowed = || RestoreError::NameNotAllowed {
        target: record.target.clone(),
        name: record.name.clone(),
    };
    let allowed: (&[&str], &[&str]) = match &record.target {
        WriteTarget::Global => (&GLOBAL_VALUE_NAMES, &[]),
        WriteTarget::Device { instance_id } => match find_keyboard(keyboards, instance_id) {
            Some(i) => {
                let driver = &keyboards[i].driver;
                let own: &[&str] = match driver {
                    KeyboardDriver::Kbdhid => &HID_NAMES,
                    KeyboardDriver::I8042prt => &PS2_NAMES,
                    KeyboardDriver::Other(_) => &[],
                };
                (own, unread_value_names(driver))
            }
            None => (&DEVICE_VALUE_NAMES, &[]),
        },
    };
    let name = record.name.as_str();
    if !allowed.0.contains(&name) && !allowed.1.contains(&name) {
        return Err(not_allowed());
    }
    let value = to
        .value(record)
        .ok_or(RestoreError::NoResolution { record: index })?;
    if matches!(value, RegValue::Other { .. }) {
        let is_baseline = value_eq(&record.name, value, &record.baseline);
        let seen_by_user = to == RestoreTo::Resolution
            && record
                .conflict
                .as_ref()
                .is_some_and(|seen| value_eq(&record.name, value, seen));
        if !is_baseline && !seen_by_user {
            return Err(not_allowed());
        }
    }
    Ok(())
}

/// The model's view of a stored DWORD: only a `REG_DWORD` counts.
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

/// Applies restore writes to a keyboard's override values (as the drivers would read them).
fn apply_device(overrides: &mut DeviceOverrides, writes: &[RestoreWrite]) {
    for write in writes {
        let slot = match write.name.as_str() {
            value_names::HID_TYPE => &mut overrides.keyboard_type_override,
            value_names::HID_SUBTYPE => &mut overrides.keyboard_subtype_override,
            value_names::PS2_TYPE => &mut overrides.override_keyboard_type,
            value_names::PS2_SUBTYPE => &mut overrides.override_keyboard_subtype,
            _ => continue,
        };
        *slot = as_dword(&write.value);
    }
}

/// Applies restore writes to the global values.
fn apply_global(global: &mut GlobalSettings, writes: &[RestoreWrite]) {
    for write in writes {
        match write.name.as_str() {
            value_names::PS2_TYPE => global.override_keyboard_type = as_dword(&write.value),
            value_names::PS2_SUBTYPE => global.override_keyboard_subtype = as_dword(&write.value),
            value_names::LAYER_DRIVER_JPN => global.layer_driver_jpn = as_string(&write.value),
            value_names::KEYBOARD_IDENTIFIER => {
                global.override_keyboard_identifier = as_string(&write.value);
            }
            _ => {}
        }
    }
}

/// True when the restore puts every i8042prt device value and every global value it writes back
/// to the record's baseline.
fn restores_baselines_only(records: &[ValueRecord], to: RestoreTo) -> bool {
    records
        .iter()
        .filter(|record| record.is_boot_time())
        .all(|record| {
            to.value(record)
                .is_some_and(|value| value_eq(&record.name, value, &record.baseline))
        })
}

/// True when the plan writes a value INV-PS2 reads: an i8042prt pin or the global type/subtype
/// pair (both use the PS/2 value names). The PS/2 names on a HID collection, which only a cleanup
/// ever records, are read by no driver and do not count ([`ValueRecord::is_boot_time`]).
fn touches_inv_ps2(records: &[ValueRecord]) -> bool {
    records
        .iter()
        .any(|record| is_ps2_value_name(&record.name) && record.is_boot_time())
}

/// True when every value INV-PS2 reads is at its baseline in the end state (`keyboards`,
/// `global` after the plan): the type/subtype pin of every i8042prt keyboard and the global pair.
/// A value in the plan is compared with its record's baseline, any other value with its entry in
/// `baselines`; a value with neither was never changed by MKLM, so it is as MKLM found it.
/// Compared as the drivers read them (only a `REG_DWORD` counts).
fn inv_ps2_values_at_baseline(
    records: &[ValueRecord],
    baselines: &[BaselineRecord],
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
) -> bool {
    let at_baseline = |target: &WriteTarget, name: &str, now: Option<u32>| {
        let key = ValueKey {
            target: target.clone(),
            name: name.to_string(),
        };
        let recorded = records
            .iter()
            .find(|record| record.key().same_value(&key))
            .map(|record| &record.baseline)
            .or_else(|| {
                baselines
                    .iter()
                    .find(|baseline| baseline.key.same_value(&key))
                    .map(|baseline| &baseline.value)
            });
        recorded.is_none_or(|baseline| as_dword(baseline) == now)
    };
    let global_ok = at_baseline(
        &WriteTarget::Global,
        value_names::PS2_TYPE,
        global.override_keyboard_type,
    ) && at_baseline(
        &WriteTarget::Global,
        value_names::PS2_SUBTYPE,
        global.override_keyboard_subtype,
    );
    global_ok
        && keyboards
            .iter()
            .filter(|kb| kb.driver == KeyboardDriver::I8042prt)
            .all(|kb| {
                let target = WriteTarget::Device {
                    instance_id: kb.instance_id.clone(),
                };
                at_baseline(
                    &target,
                    value_names::PS2_TYPE,
                    kb.overrides.override_keyboard_type,
                ) && at_baseline(
                    &target,
                    value_names::PS2_SUBTYPE,
                    kb.overrides.override_keyboard_subtype,
                )
            })
}

/// Builds the ordered restore plan for `records` (phases in the module docs) and replays it on
/// `keyboards` / `global` to check INV-PS2 after every step. Records whose devnode is gone are left
/// out by the caller (`SkipReason::DeviceRemoved`).
///
/// `expect(i)` gives the compare-and-swap expectation of record `i`. `baselines` are the
/// journal's recorded baselines (`Journal::baselines`): a violating end state is accepted as
/// "the values before MKLM" only if the values *outside* the plan are at their baselines too
/// (a device-scoped restore of a PS/2 keyboard must not unpin it while the global pair that
/// MKLM removed stays removed).
///
/// One step per registry key, in the order the key first appears in `records`, then sorted by
/// phase (stable, so forward order is kept inside a phase). The replay follows `check_plan`: no
/// step may turn a state that satisfies INV-PS2 into one that violates it, and the end state must
/// satisfy it, unless [`RestorePlan::restores_inv_ps2_violation`] applies.
pub fn plan_restore(
    records: &[ValueRecord],
    to: RestoreTo,
    expect: &dyn Fn(usize) -> Expect,
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    baselines: &[BaselineRecord],
) -> Result<RestorePlan, RestoreError> {
    let mut steps: Vec<RestoreStep> = Vec::new();
    for (index, record) in records.iter().enumerate() {
        check_record(index, record, keyboards, to)?;
        let value = to
            .value(record)
            .ok_or(RestoreError::NoResolution { record: index })?
            .clone();
        let write = RestoreWrite {
            record: index,
            name: record.name.clone(),
            value,
            expect: expect(index),
        };
        match steps
            .iter_mut()
            .find(|step| same_target(&step.target, &record.target))
        {
            Some(step) => step.writes.push(write),
            None => steps.push(RestoreStep {
                // Decided below, once every write of the key is known.
                phase: RestorePhase::Other,
                target: record.target.clone(),
                writes: vec![write],
            }),
        }
    }

    // Per-keyboard mode before and after the plan (design standard-layout B.6): no value of the
    // global pair, before as the pair's records expect it, after with the restore values.
    let expected_pair_value = |name: &str, current: Option<u32>| -> Option<u32> {
        match records
            .iter()
            .position(|r| r.target == WriteTarget::Global && r.name.eq_ignore_ascii_case(name))
        {
            Some(index) => match expect(index) {
                Expect::Value { value } => as_dword(&value),
                Expect::Any => current,
            },
            None => current,
        }
    };
    let pair_before = (
        expected_pair_value(value_names::PS2_TYPE, global.override_keyboard_type),
        expected_pair_value(value_names::PS2_SUBTYPE, global.override_keyboard_subtype),
    );
    let mut global_after = global.clone();
    for step in steps.iter().filter(|s| s.target == WriteTarget::Global) {
        apply_global(&mut global_after, &step.writes);
    }
    let stays_per_keyboard = pair_before == (None, None)
        && global_after.override_keyboard_type.is_none()
        && global_after.override_keyboard_subtype.is_none();

    for step in &mut steps {
        step.phase = match &step.target {
            WriteTarget::Global => {
                let mut after = global.clone();
                apply_global(&mut after, &step.writes);
                if after.fixed_type().is_some() {
                    RestorePhase::GlobalWithPair
                } else {
                    RestorePhase::GlobalWithoutPair
                }
            }
            WriteTarget::Device { instance_id } => {
                let known = find_keyboard(keyboards, instance_id).map(|i| &keyboards[i]);
                let is_ps2 = match known {
                    Some(kb) => kb.driver == KeyboardDriver::I8042prt,
                    // A devnode that is gone: its names say which stack it was.
                    None => step.writes.iter().all(|w| is_ps2_value_name(&w.name)),
                };
                let mut after = known.map(|kb| kb.overrides.clone()).unwrap_or_default();
                apply_device(&mut after, &step.writes);
                let is_hid = match known {
                    Some(kb) => kb.driver == KeyboardDriver::Kbdhid,
                    None => !is_ps2,
                };
                if is_ps2 {
                    if after.ps2_type().is_some() {
                        RestorePhase::AddPins
                    } else {
                        RestorePhase::RemovePins
                    }
                } else if stays_per_keyboard
                    && is_hid
                    && predict_type(&KeyboardDriver::Kbdhid, &after, &global_after)
                        .is_some_and(|ty| ty.per_keyboard_table().is_none())
                {
                    RestorePhase::FollowStandard
                } else {
                    RestorePhase::Other
                }
            }
        };
    }
    steps.sort_by_key(|step| step.phase);

    // Replay: the same rule as `check_plan`.
    let mut state_keyboards = keyboards.to_vec();
    let mut state_global = global.clone();
    let mut violation = check_inv_ps2(&state_global, &state_keyboards).err();
    let mut broken_by_step = None;
    for step in &steps {
        match &step.target {
            WriteTarget::Global => apply_global(&mut state_global, &step.writes),
            WriteTarget::Device { instance_id } => {
                if let Some(i) = find_keyboard(&state_keyboards, instance_id) {
                    apply_device(&mut state_keyboards[i].overrides, &step.writes);
                }
            }
        }
        let now = check_inv_ps2(&state_global, &state_keyboards).err();
        if violation.is_none() && broken_by_step.is_none() {
            broken_by_step.clone_from(&now);
        }
        violation = now;
    }
    // A plan that writes nothing INV-PS2 reads leaves the violation it found (a phantom PS/2
    // keyboard that appeared, say) exactly as it was; otherwise the whole end state must be the
    // values before MKLM.
    let exception = !touches_inv_ps2(records)
        || (restores_baselines_only(records, to)
            && inv_ps2_values_at_baseline(records, baselines, &state_keyboards, &state_global));
    match (violation, broken_by_step) {
        (Some(end), _) if exception => Ok(RestorePlan {
            steps,
            restores_inv_ps2_violation: Some(end),
        }),
        (Some(end), _) => Err(RestoreError::InvPs2(end)),
        (None, Some(broken)) => Err(RestoreError::InvPs2(broken)),
        (None, None) => Ok(RestorePlan {
            steps,
            restores_inv_ps2_violation: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::layout::{LayoutTable, effective_layout};
    use crate::model::value_names::*;
    use crate::test_support::*;

    fn keychron_id() -> String {
        fixtures::keychron().instance_id
    }

    fn ps2_id() -> String {
        fixtures::internal_ps2().instance_id
    }

    fn with_overrides(kb: KeyboardDevice, overrides: DeviceOverrides) -> KeyboardDevice {
        KeyboardDevice { overrides, ..kb }
    }

    fn ps2_pinned(ty: u32, subtype: u32) -> KeyboardDevice {
        with_overrides(
            fixtures::internal_ps2(),
            DeviceOverrides {
                override_keyboard_type: Some(ty),
                override_keyboard_subtype: Some(subtype),
                ..Default::default()
            },
        )
    }

    fn ps2_bare() -> KeyboardDevice {
        with_overrides(fixtures::internal_ps2(), DeviceOverrides::default())
    }

    fn keychron_with(pair: Option<(u32, u32)>) -> KeyboardDevice {
        with_overrides(
            fixtures::keychron(),
            DeviceOverrides {
                keyboard_type_override: pair.map(|p| p.0),
                keyboard_subtype_override: pair.map(|p| p.1),
                ..Default::default()
            },
        )
    }

    /// Replays `plan` the way the engine writes it and checks INV-PS2 after every step.
    fn replay(plan: &RestorePlan, keyboards: &[KeyboardDevice], global: &GlobalSettings) {
        let mut keyboards = keyboards.to_vec();
        let mut global = global.clone();
        for (n, step) in plan.steps.iter().enumerate() {
            match &step.target {
                WriteTarget::Global => apply_global(&mut global, &step.writes),
                WriteTarget::Device { instance_id } => {
                    let i = find_keyboard(&keyboards, instance_id).unwrap();
                    apply_device(&mut keyboards[i].overrides, &step.writes);
                }
            }
            assert_eq!(
                check_inv_ps2(&global, &keyboards),
                Ok(()),
                "after step {n}: {step:?}"
            );
        }
    }

    fn last_written(records: &[ValueRecord]) -> impl Fn(usize) -> Expect + '_ {
        |i| Expect::Value {
            value: records[i].intended.clone(),
        }
    }

    fn phases(plan: &RestorePlan) -> Vec<(RestorePhase, WriteTarget)> {
        plan.steps
            .iter()
            .map(|s| (s.phase, s.target.clone()))
            .collect()
    }

    /// Records of the migration of the development machine from fixed JIS (plan 1.3), in forward
    /// order: pin the PS/2 keyboard, assign the Keychron US, delete the global pair.
    fn migration_records() -> Vec<ValueRecord> {
        vec![
            record(device(&ps2_id()), PS2_TYPE, RegValue::Absent, dword(7)),
            record(device(&ps2_id()), PS2_SUBTYPE, RegValue::Absent, dword(2)),
            record(device(&keychron_id()), HID_TYPE, RegValue::Absent, dword(4)),
            record(
                device(&keychron_id()),
                HID_SUBTYPE,
                RegValue::Absent,
                dword(0),
            ),
            record(WriteTarget::Global, PS2_TYPE, dword(7), RegValue::Absent),
            record(WriteTarget::Global, PS2_SUBTYPE, dword(2), RegValue::Absent),
        ]
    }

    #[test]
    fn migrated_back_to_fixed_mode_adds_the_pair_first() {
        let records = migration_records();
        let keyboards = vec![ps2_pinned(7, 2), keychron_with(Some((4, 0)))];
        let global = fixtures::global_per_keyboard();
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &global,
            &[],
        )
        .unwrap();
        assert_eq!(
            phases(&plan),
            vec![
                (RestorePhase::GlobalWithPair, WriteTarget::Global),
                (RestorePhase::Other, device(&keychron_id())),
                (RestorePhase::RemovePins, device(&ps2_id())),
            ]
        );
        // The reverse of the forward order, as plan 1.3 says for this direction.
        let order: Vec<usize> = plan
            .steps
            .iter()
            .flat_map(|s| s.writes.iter().map(|w| w.record))
            .collect();
        assert_eq!(order, vec![4, 5, 2, 3, 0, 1]);
        assert_eq!(plan.steps[0].writes[0].value, dword(7));
        assert_eq!(
            plan.steps[0].writes[0].expect,
            Expect::Value {
                value: RegValue::Absent
            }
        );
        assert_eq!(plan.restores_inv_ps2_violation, None);
        replay(&plan, &keyboards, &global);
    }

    #[test]
    fn fixed_back_to_per_keyboard_mode_pins_first() {
        // Undo of a restore-to-baseline that had gone back to fixed mode: its records, listed with
        // the global key first on purpose.
        let records = vec![
            record(WriteTarget::Global, PS2_TYPE, RegValue::Absent, dword(7)),
            record(WriteTarget::Global, PS2_SUBTYPE, RegValue::Absent, dword(2)),
            record(device(&keychron_id()), HID_TYPE, dword(4), RegValue::Absent),
            record(
                device(&keychron_id()),
                HID_SUBTYPE,
                dword(0),
                RegValue::Absent,
            ),
            record(device(&ps2_id()), PS2_TYPE, dword(7), RegValue::Absent),
            record(device(&ps2_id()), PS2_SUBTYPE, dword(2), RegValue::Absent),
        ];
        let keyboards = vec![ps2_bare(), keychron_with(None)];
        let global = fixtures::global_fixed_jis();
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &global,
            &[],
        )
        .unwrap();
        assert_eq!(
            phases(&plan),
            vec![
                (RestorePhase::AddPins, device(&ps2_id())),
                (RestorePhase::Other, device(&keychron_id())),
                (RestorePhase::GlobalWithoutPair, WriteTarget::Global),
            ]
        );
        replay(&plan, &keyboards, &global);
        // A fixed "global first" order would have been refused.
        let bare = vec![ps2_bare(), keychron_with(None)];
        let mut global_first = global.clone();
        global_first.override_keyboard_type = None;
        global_first.override_keyboard_subtype = None;
        assert!(check_inv_ps2(&global_first, &bare).is_err());
    }

    #[test]
    fn ps2_value_change_keeps_the_pin() {
        // Revert of "PS/2 → US" in per-keyboard mode.
        let records = vec![
            record(device(&ps2_id()), PS2_TYPE, dword(7), dword(4)),
            record(device(&ps2_id()), PS2_SUBTYPE, dword(2), dword(0)),
        ];
        let keyboards = vec![ps2_pinned(4, 0), keychron_with(Some((4, 0)))];
        let global = fixtures::global_per_keyboard();
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &global,
            &[],
        )
        .unwrap();
        assert_eq!(
            phases(&plan),
            vec![(RestorePhase::AddPins, device(&ps2_id()))]
        );
        replay(&plan, &keyboards, &global);
    }

    #[test]
    fn standard_only_changes() {
        let records = vec![
            record(
                WriteTarget::Global,
                LAYER_DRIVER_JPN,
                sz("kbd106.dll"),
                sz("kbd101.dll"),
            ),
            record(
                WriteTarget::Global,
                KEYBOARD_IDENTIFIER,
                sz("PCAT_106KEY"),
                sz("PCAT_101KEY"),
            ),
        ];
        let keyboards = vec![ps2_pinned(7, 2)];
        let per_keyboard = GlobalSettings {
            layer_driver_jpn: Some("kbd101.dll".into()),
            override_keyboard_identifier: Some("PCAT_101KEY".into()),
            ..fixtures::global_per_keyboard()
        };
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &per_keyboard,
            &[],
        )
        .unwrap();
        assert_eq!(
            phases(&plan),
            vec![(RestorePhase::GlobalWithoutPair, WriteTarget::Global)]
        );
        assert_eq!(plan.steps[0].writes.len(), 2);
        replay(&plan, &keyboards, &per_keyboard);
        let fixed = GlobalSettings {
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(0),
            ..per_keyboard
        };
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &[ps2_bare()],
            &fixed,
            &[],
        )
        .unwrap();
        assert_eq!(
            phases(&plan),
            vec![(RestorePhase::GlobalWithPair, WriteTarget::Global)]
        );
    }

    #[test]
    fn value_names_are_checked_against_the_driver() {
        let keyboards = vec![ps2_pinned(7, 2), keychron_with(Some((4, 0)))];
        let rejected = |r: ValueRecord| {
            assert_eq!(
                check_restore_record(&r, &keyboards, RestoreTo::Before),
                Err(RestoreError::NameNotAllowed {
                    target: r.target.clone(),
                    name: r.name.clone()
                }),
                "{r:?}"
            );
        };
        rejected(record(WriteTarget::Global, "Start", dword(1), dword(4)));
        rejected(record(
            WriteTarget::Global,
            LAYER_DRIVER_KOR,
            sz("a"),
            sz("b"),
        ));
        rejected(record(WriteTarget::Global, HID_TYPE, dword(4), dword(7)));
        rejected(record(
            device(&keychron_id()),
            HID_TOTAL_KEYS,
            dword(4),
            dword(7),
        ));
        rejected(record(
            device(&ps2_id()),
            HID_TOTAL_KEYS,
            dword(4),
            dword(7),
        ));
        rejected(record(
            device(&keychron_id()),
            "keyboardtypeoverride",
            dword(4),
            dword(7),
        ));
        // Synthetic ID and service (the real one is `fixtures::rdp_keyboard`): another driver.
        let rdp = KeyboardDevice {
            instance_id: r"TS_INPT\TS_KBD\1".into(),
            driver: KeyboardDriver::Other("TermDD".into()),
            ..fixtures::keychron()
        };
        let with_rdp = vec![rdp.clone()];
        assert!(matches!(
            check_restore_record(
                &record(device(&rdp.instance_id), HID_TYPE, dword(4), dword(7)),
                &with_rdp,
                RestoreTo::Before
            ),
            Err(RestoreError::NameNotAllowed { .. })
        ));
        for ok in [
            record(WriteTarget::Global, PS2_TYPE, dword(7), RegValue::Absent),
            record(WriteTarget::Global, KEYBOARD_IDENTIFIER, sz("x"), sz("y")),
            record(
                device(&ps2_id().to_ascii_lowercase()),
                PS2_SUBTYPE,
                dword(2),
                dword(0),
            ),
            record(device(&keychron_id()), HID_SUBTYPE, dword(0), dword(2)),
            // The other stack's pair, which the driver does not read: what a cleanup deleted
            // (design m3 A.5) and its revert or a restore to baseline puts back.
            record(device(&ps2_id()), HID_TYPE, dword(7), RegValue::Absent),
            record(
                device(&keychron_id()),
                PS2_SUBTYPE,
                dword(2),
                RegValue::Absent,
            ),
        ] {
            assert_eq!(
                check_restore_record(&ok, &keyboards, RestoreTo::Before),
                Ok(())
            );
        }
        // A devnode that is gone: either stack's pair, nothing else.
        let gone = device(r"HID\VID_1111&PID_2222\1");
        for name in DEVICE_VALUE_NAMES {
            assert_eq!(
                check_restore_record(
                    &record(gone.clone(), name, dword(4), dword(7)),
                    &keyboards,
                    RestoreTo::Before
                ),
                Ok(())
            );
        }
        rejected(record(gone, "Start", dword(4), dword(7)));
        // plan_restore reports the record's index.
        let records = vec![
            record(device(&keychron_id()), HID_TYPE, dword(4), dword(7)),
            record(WriteTarget::Global, "Start", dword(1), dword(4)),
        ];
        assert!(matches!(
            plan_restore(
                &records,
                RestoreTo::Before,
                &|_| Expect::Any,
                &keyboards,
                &fixtures::global_per_keyboard(), &[]),
            Err(RestoreError::NameNotAllowed { name, .. }) if name == "Start"
        ));
    }

    #[test]
    fn values_no_driver_reads_do_not_touch_inv_ps2() {
        // Per-keyboard mode with an unpinned PS/2 keyboard: INV-PS2 is already broken.
        let keyboards = vec![ps2_bare(), keychron_with(Some((4, 0)))];
        let global = fixtures::global_per_keyboard();
        assert!(check_inv_ps2(&global, &keyboards).is_err());
        // Putting back what a cleanup deleted (the PS/2 pair on the Keychron, the HID pair on the
        // PS/2 keyboard) changes nothing INV-PS2 reads: shown, not refused.
        let records = vec![
            record(device(&keychron_id()), PS2_TYPE, dword(7), RegValue::Absent),
            record(
                device(&keychron_id()),
                PS2_SUBTYPE,
                dword(2),
                RegValue::Absent,
            ),
            record(device(&ps2_id()), HID_TYPE, dword(7), RegValue::Absent),
        ];
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &global,
            &[],
        )
        .unwrap();
        assert!(plan.restores_inv_ps2_violation.is_some());
        assert_eq!(plan.steps.iter().map(|s| s.writes.len()).sum::<usize>(), 3);
        assert!(records.iter().all(|r| !r.is_boot_time()));
        // The PS/2 keyboard's own pin does count (unless it goes back to its baseline).
        let mut unpin = record(device(&ps2_id()), PS2_TYPE, RegValue::Absent, dword(4));
        unpin.baseline = dword(7);
        let pin = vec![unpin];
        assert!(matches!(
            plan_restore(
                &pin,
                RestoreTo::Before,
                &last_written(&pin),
                &keyboards,
                &global,
                &[]
            ),
            Err(RestoreError::InvPs2(_))
        ));
    }

    #[test]
    fn other_values_are_restored_only_as_the_baseline() {
        let keyboards = vec![keychron_with(Some((4, 0)))];
        let odd = RegValue::Other {
            reg_type: 1,
            data_hex: "340000".into(),
        };
        // Op 1 wrote over a REG_SZ "4": its before is the baseline.
        let first = record(device(&keychron_id()), HID_TYPE, odd.clone(), dword(7));
        for to in [RestoreTo::Before, RestoreTo::Baseline] {
            assert_eq!(check_restore_record(&first, &keyboards, to), Ok(()));
        }
        // An `Other` that is not the baseline is never written.
        let mut foreign = record(device(&keychron_id()), HID_TYPE, odd.clone(), dword(7));
        foreign.baseline = dword(4);
        assert!(matches!(
            check_restore_record(&foreign, &keyboards, RestoreTo::Before),
            Err(RestoreError::NameNotAllowed { .. })
        ));
        assert_eq!(
            check_restore_record(&foreign, &keyboards, RestoreTo::Baseline),
            Ok(())
        );
        // A resolution may keep the value the user saw.
        foreign.resolve_to = Some(odd.clone());
        assert!(check_restore_record(&foreign, &keyboards, RestoreTo::Resolution).is_err());
        foreign.conflict = Some(odd.clone());
        assert_eq!(
            check_restore_record(&foreign, &keyboards, RestoreTo::Resolution),
            Ok(())
        );
        foreign.resolve_to = Some(RegValue::Other {
            reg_type: 3,
            data_hex: "00".into(),
        });
        assert!(check_restore_record(&foreign, &keyboards, RestoreTo::Resolution).is_err());
        // Restored byte for byte, but not taken for a pin.
        let plan = plan_restore(
            std::slice::from_ref(&first),
            RestoreTo::Baseline,
            &|_| Expect::Any,
            &keyboards,
            &fixtures::global_per_keyboard(),
            &[],
        )
        .unwrap();
        assert_eq!(plan.steps[0].writes[0].value, odd);
    }

    #[test]
    fn resolutions_need_a_value_for_every_record() {
        let keyboards = vec![keychron_with(Some((7, 2)))];
        let mut records = vec![
            record(device(&keychron_id()), HID_TYPE, dword(4), dword(7)),
            record(device(&keychron_id()), HID_SUBTYPE, dword(0), dword(2)),
        ];
        records[0].resolve_to = Some(dword(4));
        assert_eq!(
            plan_restore(
                &records,
                RestoreTo::Resolution,
                &|_| Expect::Any,
                &keyboards,
                &fixtures::global_per_keyboard(),
                &[]
            ),
            Err(RestoreError::NoResolution { record: 1 })
        );
        records[1].resolve_to = Some(dword(0));
        let plan = plan_restore(
            &records,
            RestoreTo::Resolution,
            &|i| Expect::Value {
                value: records[i].intended.clone(),
            },
            &keyboards,
            &fixtures::global_per_keyboard(),
            &[],
        )
        .unwrap();
        let values: Vec<&RegValue> = plan.steps[0].writes.iter().map(|w| &w.value).collect();
        assert_eq!(values, vec![&dword(4), &dword(0)]);
        assert_eq!(
            check_restore_record(
                &record(device(&keychron_id()), HID_TYPE, dword(4), dword(7)),
                &keyboards,
                RestoreTo::Resolution
            ),
            Err(RestoreError::NoResolution { record: 0 })
        );
    }

    /// A baseline record as the journal keeps it.
    fn baseline_of(target: WriteTarget, name: &str, value: RegValue) -> BaselineRecord {
        BaselineRecord {
            schema_version: crate::BASELINE_SCHEMA_VERSION,
            key: ValueKey {
                target,
                name: name.to_string(),
            },
            key_path: String::new(),
            value,
            captured_at: crate::Timestamp(0),
            captured_by: op_id(1),
        }
    }

    /// Design D.1 table: a violating end state is "the values before MKLM" only when every
    /// i8042prt pin and the global pair are at their baselines, including values outside the plan.
    /// A device-scoped restore of the PS/2 keyboard on a migrated PC (pair removed by MKLM) holds
    /// only the pin records, all at baseline, but must not unpin it while the pair stays removed.
    #[test]
    fn a_violation_is_baseline_only_when_values_outside_the_plan_are_too() {
        let records = vec![
            record(device(&ps2_id()), PS2_TYPE, RegValue::Absent, dword(7)),
            record(device(&ps2_id()), PS2_SUBTYPE, RegValue::Absent, dword(2)),
        ];
        let keyboards = vec![ps2_pinned(7, 2)];
        let migrated = fixtures::global_per_keyboard();
        // Migrated from fixed JIS: the pair's baseline is 7/2, and it is outside this plan.
        let mut baselines = vec![
            baseline_of(device(&ps2_id()), PS2_TYPE, RegValue::Absent),
            baseline_of(device(&ps2_id()), PS2_SUBTYPE, RegValue::Absent),
            baseline_of(WriteTarget::Global, PS2_TYPE, dword(7)),
            baseline_of(WriteTarget::Global, PS2_SUBTYPE, dword(2)),
        ];
        let violation = InvPs2Violation {
            keyboards: vec![ps2_id()],
        };
        assert_eq!(
            plan_restore(
                &records,
                RestoreTo::Baseline,
                &last_written(&records),
                &keyboards,
                &migrated,
                &baselines,
            ),
            Err(RestoreError::InvPs2(violation.clone()))
        );
        // The same plan when the PC was in per-keyboard mode before MKLM: the violation is the
        // one MKLM found.
        baselines.truncate(2);
        baselines.push(baseline_of(WriteTarget::Global, PS2_TYPE, RegValue::Absent));
        let plan = plan_restore(
            &records,
            RestoreTo::Baseline,
            &last_written(&records),
            &keyboards,
            &migrated,
            &baselines,
        )
        .unwrap();
        assert_eq!(plan.restores_inv_ps2_violation, Some(violation.clone()));
        // Another i8042prt keyboard that MKLM pinned and this plan leaves pinned is not at its
        // baseline either.
        let other_ps2 = KeyboardDevice {
            instance_id: r"ACPI\PNP0303\4&2&0".into(),
            ..ps2_pinned(7, 2)
        };
        baselines.push(baseline_of(
            device(&other_ps2.instance_id),
            PS2_TYPE,
            RegValue::Absent,
        ));
        assert_eq!(
            plan_restore(
                &records,
                RestoreTo::Baseline,
                &last_written(&records),
                &[ps2_pinned(7, 2), other_ps2],
                &migrated,
                &baselines,
            ),
            Err(RestoreError::InvPs2(violation))
        );
    }

    #[test]
    fn a_pre_existing_violation_may_only_come_back_as_the_baseline() {
        // Before MKLM: per-keyboard mode, the PS/2 keyboard unpinned (it typed US). MKLM pinned it.
        let records = vec![
            record(device(&ps2_id()), PS2_TYPE, RegValue::Absent, dword(7)),
            record(device(&ps2_id()), PS2_SUBTYPE, RegValue::Absent, dword(2)),
        ];
        let keyboards = vec![ps2_pinned(7, 2)];
        let global = fixtures::global_per_keyboard();
        let plan = plan_restore(
            &records,
            RestoreTo::Baseline,
            &last_written(&records),
            &keyboards,
            &global,
            &[],
        )
        .unwrap();
        assert_eq!(
            plan.restores_inv_ps2_violation,
            Some(InvPs2Violation {
                keyboards: vec![ps2_id()]
            })
        );
        assert_eq!(plan.steps[0].phase, RestorePhase::RemovePins);

        // Removing a pin that was not there before MKLM is refused.
        let mut not_baseline = records.clone();
        for r in &mut not_baseline {
            r.baseline = r.intended.clone();
        }
        assert_eq!(
            plan_restore(
                &not_baseline,
                RestoreTo::Before,
                &last_written(&not_baseline),
                &keyboards,
                &global,
                &[]
            ),
            Err(RestoreError::InvPs2(InvPs2Violation {
                keyboards: vec![ps2_id()]
            }))
        );

        // A HID-only restore cannot change INV-PS2: an existing violation (an unpinned phantom that
        // appeared) is reported, not a reason to refuse.
        let phantom = KeyboardDevice {
            instance_id: r"ACPI\PNP0303\4&1&0".into(),
            present: false,
            ..ps2_bare()
        };
        let hid = vec![record(device(&keychron_id()), HID_TYPE, dword(4), dword(7))];
        let plan = plan_restore(
            &hid,
            RestoreTo::Before,
            &last_written(&hid),
            &[keychron_with(Some((7, 2))), phantom.clone()],
            &global,
            &[],
        )
        .unwrap();
        assert_eq!(
            plan.restores_inv_ps2_violation,
            Some(InvPs2Violation {
                keyboards: vec![phantom.instance_id.clone()]
            })
        );
    }

    #[test]
    fn steps_group_writes_per_key_and_keep_expectations() {
        let records = migration_records();
        let keyboards = vec![ps2_pinned(7, 2), keychron_with(Some((4, 0)))];
        let plan = plan_restore(
            &records,
            RestoreTo::Baseline,
            &|i| {
                if i == 2 {
                    Expect::Any
                } else {
                    Expect::Value {
                        value: records[i].intended.clone(),
                    }
                }
            },
            &keyboards,
            &fixtures::global_per_keyboard(),
            &[],
        )
        .unwrap();
        assert_eq!(plan.steps.len(), 3);
        for step in &plan.steps {
            assert_eq!(step.writes.len(), 2);
            for write in &step.writes {
                assert_eq!(write.name, records[write.record].name);
                assert_eq!(write.value, records[write.record].baseline);
            }
        }
        assert_eq!(plan.steps[1].writes[0].expect, Expect::Any);
        // An empty restore is an empty plan.
        assert_eq!(
            plan_restore(
                &[],
                RestoreTo::Before,
                &|_| Expect::Any,
                &keyboards,
                &fixtures::global_per_keyboard(),
                &[]
            ),
            Ok(RestorePlan {
                steps: vec![],
                restores_inv_ps2_violation: None
            })
        );
    }

    #[test]
    fn a_removed_ps2_devnode_is_phased_by_its_values() {
        let gone = r"ACPI\PNP0303\4&9&0";
        let records = vec![
            record(device(gone), PS2_TYPE, RegValue::Absent, dword(7)),
            record(device(gone), PS2_SUBTYPE, RegValue::Absent, dword(2)),
            record(WriteTarget::Global, PS2_TYPE, dword(7), RegValue::Absent),
            record(WriteTarget::Global, PS2_SUBTYPE, dword(2), RegValue::Absent),
        ];
        let keyboards = vec![ps2_pinned(7, 2)];
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &fixtures::global_per_keyboard(),
            &[],
        )
        .unwrap();
        assert_eq!(
            phases(&plan),
            vec![
                (RestorePhase::GlobalWithPair, WriteTarget::Global),
                (RestorePhase::RemovePins, device(gone)),
            ]
        );
    }

    // --- Design standard-layout B.6: a standard change is undone standard first --------------

    fn wireless_id() -> String {
        fixtures::desktop_wireless().instance_id
    }

    fn vxe_id() -> String {
        fixtures::desktop_vxe().instance_id
    }

    fn second_id() -> String {
        fixtures::desktop_wireless_second().instance_id
    }

    /// The desktop PC after "standard JIS → US" wrote its records: the followers pinned to JIS,
    /// the standard US, the fixed-mode pair absent (per-keyboard mode).
    fn after_standard_change() -> (Vec<KeyboardDevice>, GlobalSettings) {
        let mut keyboards = fixtures::desktop_pc().keyboards;
        for kb in &mut keyboards {
            if [wireless_id(), second_id(), vxe_id()].contains(&kb.instance_id) {
                kb.overrides.keyboard_type_override = Some(7);
                kb.overrides.keyboard_subtype_override = Some(2);
            }
        }
        let global = GlobalSettings {
            layer_driver_jpn: Some("kbd101.dll".into()),
            override_keyboard_identifier: Some("PCAT_101KEY".into()),
            ..fixtures::global_per_keyboard()
        };
        (keyboards, global)
    }

    /// The records of that change in forward order: pins, then the check-only records of the
    /// global pair and the standard.
    fn standard_change_records() -> Vec<ValueRecord> {
        let mut records = Vec::new();
        for id in [wireless_id(), second_id(), vxe_id()] {
            records.push(record(device(&id), HID_TYPE, RegValue::Absent, dword(7)));
            records.push(record(device(&id), HID_SUBTYPE, RegValue::Absent, dword(2)));
        }
        records.extend([
            record(
                WriteTarget::Global,
                PS2_TYPE,
                RegValue::Absent,
                RegValue::Absent,
            ),
            record(
                WriteTarget::Global,
                PS2_SUBTYPE,
                RegValue::Absent,
                RegValue::Absent,
            ),
            record(
                WriteTarget::Global,
                LAYER_DRIVER_JPN,
                sz("kbd106.dll"),
                sz("kbd101.dll"),
            ),
            record(
                WriteTarget::Global,
                KEYBOARD_IDENTIFIER,
                sz("PCAT_106KEY"),
                sz("PCAT_101KEY"),
            ),
        ]);
        records
    }

    /// The table each keyboard types with in a state (I8 of the crash tests), for the keyboards
    /// MKLM can write.
    fn tables(keyboards: &[KeyboardDevice], global: &GlobalSettings) -> Vec<Option<LayoutTable>> {
        keyboards
            .iter()
            .map(|kb| {
                let writable =
                    matches!(kb.driver, KeyboardDriver::Kbdhid | KeyboardDriver::I8042prt);
                writable
                    .then(|| kb.predicted_type(global))
                    .flatten()
                    .map(|ty| effective_layout(global.mode(), &global.standard_layout(), ty).table)
            })
            .collect()
    }

    /// Replays `plan` like [`replay`] and asserts I8: no keyboard's table changes on the way.
    fn replay_keeping_tables(
        plan: &RestorePlan,
        keyboards: &[KeyboardDevice],
        global: &GlobalSettings,
    ) {
        replay(plan, keyboards, global);
        let expected = tables(keyboards, global);
        let mut keyboards = keyboards.to_vec();
        let mut global = global.clone();
        for (n, step) in plan.steps.iter().enumerate() {
            match &step.target {
                WriteTarget::Global => apply_global(&mut global, &step.writes),
                WriteTarget::Device { instance_id } => {
                    let i = find_keyboard(&keyboards, instance_id).unwrap();
                    apply_device(&mut keyboards[i].overrides, &step.writes);
                }
            }
            assert_eq!(
                tables(&keyboards, &global),
                expected,
                "a table changed after step {n}: {step:?}"
            );
        }
    }

    #[test]
    fn a_standard_change_is_reverted_standard_first() {
        let records = standard_change_records();
        let (keyboards, global) = after_standard_change();
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &global,
            &[],
        )
        .unwrap();
        assert_eq!(
            phases(&plan),
            vec![
                (RestorePhase::GlobalWithoutPair, WriteTarget::Global),
                (RestorePhase::FollowStandard, device(&wireless_id())),
                (RestorePhase::FollowStandard, device(&second_id())),
                (RestorePhase::FollowStandard, device(&vxe_id())),
            ]
        );
        replay_keeping_tables(&plan, &keyboards, &global);
        // The old order (pins first) would have let the followers type US in between.
        let mut early = keyboards.clone();
        let i = find_keyboard(&early, &wireless_id()).unwrap();
        early[i].overrides = DeviceOverrides::default();
        assert_ne!(tables(&early, &global), tables(&keyboards, &global));
    }

    #[test]
    fn a_rollback_of_a_standard_change_puts_the_standard_back_first() {
        let records = standard_change_records();
        let (keyboards, global) = after_standard_change();
        // A rollback expects `intended` (nothing was confirmed).
        let intended = |i: usize| Expect::Value {
            value: records[i].intended.clone(),
        };
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &intended,
            &keyboards,
            &global,
            &[],
        )
        .unwrap();
        assert_eq!(plan.steps[0].target, WriteTarget::Global);
        assert!(
            plan.steps[1..]
                .iter()
                .all(|s| s.phase == RestorePhase::FollowStandard)
        );
        replay_keeping_tables(&plan, &keyboards, &global);
        // The check-only records are part of the global step, first: a pair that appeared stops
        // it before any pin is removed.
        let global_step = &plan.steps[0];
        assert_eq!(
            global_step
                .writes
                .iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>(),
            vec![PS2_TYPE, PS2_SUBTYPE, LAYER_DRIVER_JPN, KEYBOARD_IDENTIFIER]
        );
    }

    #[test]
    fn the_order_follows_the_expected_pair_not_the_current_one() {
        let records = standard_change_records();
        let (keyboards, global) = after_standard_change();
        // The Settings app switched the PC to fixed mode (English) after the change.
        let fixed_now = GlobalSettings {
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(0),
            ..global
        };
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &fixed_now,
            &[],
        )
        .unwrap();
        // Still the standard first: its compare-and-swap on the pair fails there, and no pin has
        // been removed.
        assert_eq!(plan.steps[0].target, WriteTarget::Global);
        assert_eq!(plan.steps[0].phase, RestorePhase::GlobalWithoutPair);
        assert_eq!(
            plan.steps[0].writes[0].expect,
            Expect::Value {
                value: RegValue::Absent
            }
        );
        assert!(
            plan.steps[1..]
                .iter()
                .all(|s| s.phase == RestorePhase::FollowStandard)
        );
        // Without records of the pair, the current pair decides: fixed mode, the order of a
        // restore towards fixed mode (the global values first, then the keyboards).
        let without_guards: Vec<ValueRecord> = records
            .iter()
            .filter(|r| r.target != WriteTarget::Global || !is_ps2_value_name(&r.name))
            .cloned()
            .collect();
        let plan = plan_restore(
            &without_guards,
            RestoreTo::Before,
            &last_written(&without_guards),
            &keyboards,
            &fixed_now,
            &[],
        )
        .unwrap();
        assert_eq!(plan.steps[0].phase, RestorePhase::GlobalWithPair);
        assert!(
            plan.steps[1..]
                .iter()
                .all(|s| s.phase == RestorePhase::Other)
        );
    }

    #[test]
    fn pins_added_by_a_restore_go_before_the_standard() {
        // Undoing a restore to baseline that stayed in per-keyboard mode: it had deleted the
        // Keychron's pin (US) and put the standard back to JIS. The revert adds the pin, which has a
        // table of its own, before the standard goes back to US, so the keyboard never follows
        // the standard in between.
        let keychron = fixtures::desktop_keychron().instance_id;
        let records = vec![
            record(device(&keychron), HID_TYPE, dword(4), RegValue::Absent),
            record(device(&keychron), HID_SUBTYPE, dword(0), RegValue::Absent),
            record(
                WriteTarget::Global,
                LAYER_DRIVER_JPN,
                sz("kbd101.dll"),
                sz("kbd106.dll"),
            ),
            record(
                WriteTarget::Global,
                KEYBOARD_IDENTIFIER,
                sz("PCAT_101KEY"),
                sz("PCAT_106KEY"),
            ),
        ];
        let mut keyboards = fixtures::desktop_pc().keyboards;
        let i = find_keyboard(&keyboards, &keychron).unwrap();
        keyboards[i].overrides = DeviceOverrides::default();
        let global = fixtures::global_per_keyboard();
        let plan = plan_restore(
            &records,
            RestoreTo::Before,
            &last_written(&records),
            &keyboards,
            &global,
            &[],
        )
        .unwrap();
        assert_eq!(
            phases(&plan),
            vec![
                (RestorePhase::Other, device(&keychron)),
                (RestorePhase::GlobalWithoutPair, WriteTarget::Global),
            ]
        );
        replay(&plan, &keyboards, &global);
    }
}
