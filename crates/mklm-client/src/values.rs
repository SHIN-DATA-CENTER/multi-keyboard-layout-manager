//! Stored values as the unelevated snapshot reads them (for previews and displays; the helper
//! reads every value again itself before it writes).

use mklm_core::{GlobalSettings, KeyboardDevice, RegValue, ValueOp, WriteTarget, value_names};

/// The value a planned write leaves behind.
pub fn op_value(op: &ValueOp) -> RegValue {
    match op {
        ValueOp::Set(value) => RegValue::Dword { value: *value },
        ValueOp::SetString(value) => RegValue::Sz {
            value: value.clone(),
        },
        ValueOp::Delete => RegValue::Absent,
    }
}

/// The stored value `name` of `target` as the unelevated snapshot read it: `None` when the
/// keyboard is not in the snapshot (removed). Values of an unexpected type read as absent here;
/// the helper reads every value again itself before it writes.
pub fn model_value(
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
    target: &WriteTarget,
    name: &str,
) -> Option<RegValue> {
    let dword =
        |value: Option<u32>| value.map_or(RegValue::Absent, |value| RegValue::Dword { value });
    let string = |value: &Option<String>| {
        value
            .as_ref()
            .map_or(RegValue::Absent, |value| RegValue::Sz {
                value: value.clone(),
            })
    };
    let is = |candidate: &str| name.eq_ignore_ascii_case(candidate);
    match target {
        WriteTarget::Device { instance_id } => {
            let kb = keyboards
                .iter()
                .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))?;
            let o = &kb.overrides;
            Some(if is(value_names::HID_TYPE) {
                dword(o.keyboard_type_override)
            } else if is(value_names::HID_SUBTYPE) {
                dword(o.keyboard_subtype_override)
            } else if is(value_names::PS2_TYPE) {
                dword(o.override_keyboard_type)
            } else if is(value_names::PS2_SUBTYPE) {
                dword(o.override_keyboard_subtype)
            } else if is(value_names::HID_TOTAL_KEYS) {
                dword(o.number_total_keys_override)
            } else if is(value_names::HID_FUNCTION_KEYS) {
                dword(o.number_function_keys_override)
            } else if is(value_names::HID_INDICATORS) {
                dword(o.number_indicators_override)
            } else {
                RegValue::Absent
            })
        }
        WriteTarget::Global => Some(if is(value_names::PS2_TYPE) {
            dword(global.override_keyboard_type)
        } else if is(value_names::PS2_SUBTYPE) {
            dword(global.override_keyboard_subtype)
        } else if is(value_names::LAYER_DRIVER_JPN) {
            string(&global.layer_driver_jpn)
        } else if is(value_names::LAYER_DRIVER_KOR) {
            string(&global.layer_driver_kor)
        } else if is(value_names::KEYBOARD_IDENTIFIER) {
            string(&global.override_keyboard_identifier)
        } else {
            RegValue::Absent
        }),
    }
}
