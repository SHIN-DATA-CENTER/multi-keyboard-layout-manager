//! Per-keyboard rules: stored and predicted type, value anomalies and the action that applies a change.

use serde::{Deserialize, Serialize};

use crate::layout::PendingAction;
use crate::model::{
    DeviceOverrides, GlobalMode, GlobalSettings, KeyboardDevice, KeyboardDriver, KeyboardType,
    value_names,
};
use crate::safety::{ResetBan, structural_reset_bans};

/// Type kbdhid reports without overrides, and i8042prt without device or global values.
const HID_DEFAULT: KeyboardType = KeyboardType::HID_UNKNOWN;
const PS2_DEFAULT: KeyboardType = KeyboardType::US;

impl DeviceOverrides {
    /// `KeyboardTypeOverride` / `KeyboardSubtypeOverride` when both exist.
    pub fn hid_type(&self) -> Option<KeyboardType> {
        Some(KeyboardType::new(
            self.keyboard_type_override?,
            self.keyboard_subtype_override?,
        ))
    }

    /// `OverrideKeyboardType` / `OverrideKeyboardSubtype` when both exist.
    pub fn ps2_type(&self) -> Option<KeyboardType> {
        Some(KeyboardType::new(
            self.override_keyboard_type?,
            self.override_keyboard_subtype?,
        ))
    }

    /// The complete pair the given driver reads, if stored. Other drivers read none.
    pub fn stored_type(&self, driver: &KeyboardDriver) -> Option<KeyboardType> {
        match driver {
            KeyboardDriver::Kbdhid => self.hid_type(),
            KeyboardDriver::I8042prt => self.ps2_type(),
            KeyboardDriver::Other(_) => None,
        }
    }

    /// Registry names of the values that exist, in model order.
    pub fn present_value_names(&self) -> Vec<&'static str> {
        [
            (self.keyboard_type_override, value_names::HID_TYPE),
            (self.keyboard_subtype_override, value_names::HID_SUBTYPE),
            (self.override_keyboard_type, value_names::PS2_TYPE),
            (self.override_keyboard_subtype, value_names::PS2_SUBTYPE),
            (self.number_total_keys_override, value_names::HID_TOTAL_KEYS),
            (
                self.number_function_keys_override,
                value_names::HID_FUNCTION_KEYS,
            ),
            (self.number_indicators_override, value_names::HID_INDICATORS),
        ]
        .into_iter()
        .filter_map(|(value, name)| value.map(|_| name))
        .collect()
    }

    /// True when no override value exists.
    pub fn is_empty(&self) -> bool {
        self.present_value_names().is_empty()
    }
}

/// Type the driver will report the next time it starts the keyboard (reset, reconnect or PC restart).
///
/// kbdhid: its own values, else 0x51/0. i8042prt: device values, else global values, else 4/0
/// (resolved per value; a lone value is flagged by [`keyboard_anomalies`]). Other drivers: `None`.
pub fn predict_type(
    driver: &KeyboardDriver,
    overrides: &DeviceOverrides,
    global: &GlobalSettings,
) -> Option<KeyboardType> {
    match driver {
        KeyboardDriver::Kbdhid => Some(KeyboardType::new(
            overrides.keyboard_type_override.unwrap_or(HID_DEFAULT.ty),
            overrides
                .keyboard_subtype_override
                .unwrap_or(HID_DEFAULT.subtype),
        )),
        KeyboardDriver::I8042prt => Some(KeyboardType::new(
            overrides
                .override_keyboard_type
                .or(global.override_keyboard_type)
                .unwrap_or(PS2_DEFAULT.ty),
            overrides
                .override_keyboard_subtype
                .or(global.override_keyboard_subtype)
                .unwrap_or(PS2_DEFAULT.subtype),
        )),
        KeyboardDriver::Other(_) => None,
    }
}

impl KeyboardDevice {
    /// The complete pair stored for this keyboard's driver.
    pub fn stored_type(&self) -> Option<KeyboardType> {
        self.overrides.stored_type(&self.driver)
    }

    /// See [`predict_type`].
    pub fn predicted_type(&self, global: &GlobalSettings) -> Option<KeyboardType> {
        predict_type(&self.driver, &self.overrides, global)
    }
}

/// Problems with a keyboard's stored values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum KeyboardAnomaly {
    /// Only one value of the pair the driver reads exists.
    IncompletePair { present: String, missing: String },
    /// Values named for the other driver stack; this keyboard's driver ignores them.
    ForeignValueNames { names: Vec<String> },
    /// Override values on a devnode served by neither kbdhid nor i8042prt.
    ValuesOnUnsupportedDriver { names: Vec<String> },
    /// Override values on a non-keyboard devnode (e.g. the mouse collection of the same device).
    ValuesOnNonKeyboard { names: Vec<String> },
    /// A stored type MKLM never writes whose per-keyboard mapping is unverified on real hardware:
    /// 7/0 (M0 #5) or NEC 7/0xD02.
    UnverifiedType { keyboard_type: KeyboardType },
    /// A stored type MKLM never writes and that has no table of its own.
    UnexpectedType { keyboard_type: KeyboardType },
    /// Fixed mode: the stored type would select another table, but is ignored (M0 #2a).
    IgnoredInFixedMode { keyboard_type: KeyboardType },
}

/// Lists every anomaly in a keyboard's stored values.
pub fn keyboard_anomalies(
    device: &KeyboardDevice,
    global: &GlobalSettings,
) -> Vec<KeyboardAnomaly> {
    let overrides = &device.overrides;
    let present = overrides.present_value_names();
    let own: [&str; 2] = match device.driver {
        KeyboardDriver::Kbdhid => [value_names::HID_TYPE, value_names::HID_SUBTYPE],
        KeyboardDriver::I8042prt => [value_names::PS2_TYPE, value_names::PS2_SUBTYPE],
        KeyboardDriver::Other(_) => {
            if present.is_empty() {
                return Vec::new();
            }
            return vec![KeyboardAnomaly::ValuesOnUnsupportedDriver {
                names: present.iter().map(|n| n.to_string()).collect(),
            }];
        }
    };

    let mut anomalies = Vec::new();
    match (present.contains(&own[0]), present.contains(&own[1])) {
        (true, false) => anomalies.push(KeyboardAnomaly::IncompletePair {
            present: own[0].to_string(),
            missing: own[1].to_string(),
        }),
        (false, true) => anomalies.push(KeyboardAnomaly::IncompletePair {
            present: own[1].to_string(),
            missing: own[0].to_string(),
        }),
        _ => {}
    }

    // kbdhid also reads the KeyboardNumber*Override extras; i8042prt reads none of the HID names.
    let foreign: Vec<String> = present
        .iter()
        .filter(|name| match device.driver {
            KeyboardDriver::Kbdhid => {
                **name == value_names::PS2_TYPE || **name == value_names::PS2_SUBTYPE
            }
            _ => !own.contains(name),
        })
        .map(|name| name.to_string())
        .collect();
    if !foreign.is_empty() {
        anomalies.push(KeyboardAnomaly::ForeignValueNames { names: foreign });
    }

    if let Some(ty) = device.stored_type() {
        let own_table = ty.per_keyboard_table();
        // 4/0 and 7/2 are what MKLM writes, verified in M0 #4 and #2b.
        if ty != KeyboardType::US && ty != KeyboardType::JIS {
            anomalies.push(
                if ty == KeyboardType::US_ON_JAPANESE || own_table.is_some() {
                    KeyboardAnomaly::UnverifiedType { keyboard_type: ty }
                } else {
                    KeyboardAnomaly::UnexpectedType { keyboard_type: ty }
                },
            );
        }
        if global.mode() == GlobalMode::Fixed
            && let Some(table) = own_table
            && table != global.standard_layout()
        {
            anomalies.push(KeyboardAnomaly::IgnoredInFixedMode { keyboard_type: ty });
        }
    }
    anomalies
}

/// Anomaly for override values found on a devnode outside the Keyboard class. No driver reads them there.
pub fn non_keyboard_anomaly(overrides: &DeviceOverrides) -> Option<KeyboardAnomaly> {
    let names = overrides.present_value_names();
    (!names.is_empty()).then(|| KeyboardAnomaly::ValuesOnNonKeyboard {
        names: names.iter().map(|n| n.to_string()).collect(),
    })
}

/// Lightest action that makes a changed device value take effect for this keyboard, derived from
/// [`structural_reset_bans`] so that it never offers a reset the bans forbid.
///
/// No ban: a reset (USB, M0 #2b). Only [`ResetBan::UnprovenTransport`]: reconnecting, until a reset
/// is verified for the transport (plan gate G2). Any other ban (PS/2, internal, I2C/SPI, virtual,
/// unknown container or transport): a PC restart.
pub fn device_apply_action(device: &KeyboardDevice) -> PendingAction {
    let bans = structural_reset_bans(device);
    if bans.is_empty() {
        PendingAction::ResetKeyboard
    } else if bans.iter().all(ResetBan::allows_reconnect) {
        PendingAction::Reconnect
    } else {
        PendingAction::RestartPc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::Transport;

    #[test]
    fn stored_type_uses_driver_names() {
        let hid = DeviceOverrides {
            keyboard_type_override: Some(4),
            keyboard_subtype_override: Some(0),
            ..Default::default()
        };
        assert_eq!(
            hid.stored_type(&KeyboardDriver::Kbdhid),
            Some(KeyboardType::US)
        );
        assert_eq!(hid.stored_type(&KeyboardDriver::I8042prt), None);
        assert_eq!(hid.stored_type(&KeyboardDriver::Other("x".into())), None);

        let ps2 = DeviceOverrides {
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(2),
            ..Default::default()
        };
        assert_eq!(
            ps2.stored_type(&KeyboardDriver::I8042prt),
            Some(KeyboardType::JIS)
        );
        assert_eq!(ps2.stored_type(&KeyboardDriver::Kbdhid), None);

        let lone = DeviceOverrides {
            keyboard_type_override: Some(4),
            ..Default::default()
        };
        assert_eq!(lone.hid_type(), None);
        assert!(!lone.is_empty());
        assert!(DeviceOverrides::default().is_empty());
    }

    #[test]
    fn present_names() {
        let all = DeviceOverrides {
            keyboard_type_override: Some(4),
            keyboard_subtype_override: Some(0),
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(2),
            number_total_keys_override: Some(104),
            number_function_keys_override: Some(12),
            number_indicators_override: Some(3),
        };
        assert_eq!(all.present_value_names().len(), 7);
        assert_eq!(
            fixtures::keychron().overrides.present_value_names(),
            vec!["KeyboardTypeOverride", "KeyboardSubtypeOverride"]
        );
    }

    #[test]
    fn hid_prediction() {
        let global = fixtures::global_fixed_jis();
        assert_eq!(
            fixtures::keychron().predicted_type(&global),
            Some(KeyboardType::US)
        );
        assert_eq!(
            fixtures::vxe_ble().predicted_type(&global),
            Some(KeyboardType::HID_UNKNOWN)
        );
        // kbdhid never reads the global values.
        let lone = DeviceOverrides {
            keyboard_type_override: Some(7),
            ..Default::default()
        };
        assert_eq!(
            predict_type(&KeyboardDriver::Kbdhid, &lone, &global),
            Some(KeyboardType::new(7, 0))
        );
    }

    #[test]
    fn ps2_prediction() {
        let bare = DeviceOverrides::default();
        let device_jis = fixtures::internal_ps2().overrides;
        let pk = fixtures::global_per_keyboard();
        let fixed = fixtures::global_fixed_jis();
        let ps2 = KeyboardDriver::I8042prt;
        assert_eq!(
            predict_type(&ps2, &device_jis, &pk),
            Some(KeyboardType::JIS)
        );
        assert_eq!(predict_type(&ps2, &bare, &fixed), Some(KeyboardType::JIS));
        assert_eq!(predict_type(&ps2, &bare, &pk), Some(KeyboardType::US));
        let device_us = DeviceOverrides {
            override_keyboard_type: Some(4),
            override_keyboard_subtype: Some(0),
            ..Default::default()
        };
        assert_eq!(
            predict_type(&ps2, &device_us, &fixed),
            Some(KeyboardType::US)
        );
        assert_eq!(
            predict_type(&KeyboardDriver::Other("x".into()), &bare, &pk),
            None
        );
    }

    #[test]
    fn dev_machine_has_no_anomalies_in_per_keyboard_mode() {
        let snapshot = fixtures::dev_machine();
        for kb in &snapshot.keyboards {
            assert_eq!(
                keyboard_anomalies(kb, &snapshot.global),
                vec![],
                "{}",
                kb.instance_id
            );
        }
    }

    #[test]
    fn keychron_us_is_ignored_in_fixed_mode() {
        assert_eq!(
            keyboard_anomalies(&fixtures::keychron(), &fixtures::global_fixed_jis()),
            vec![KeyboardAnomaly::IgnoredInFixedMode {
                keyboard_type: KeyboardType::US
            }]
        );
        // PS/2 7/2 matches the JIS standard, so nothing is ignored.
        assert_eq!(
            keyboard_anomalies(&fixtures::internal_ps2(), &fixtures::global_fixed_jis()),
            vec![]
        );
    }

    #[test]
    fn flags_wrong_names_and_partial_pairs() {
        let ps2_names_on_hid = KeyboardDevice {
            overrides: DeviceOverrides {
                override_keyboard_type: Some(4),
                override_keyboard_subtype: Some(0),
                ..Default::default()
            },
            ..fixtures::keychron()
        };
        assert_eq!(
            keyboard_anomalies(&ps2_names_on_hid, &fixtures::global_per_keyboard()),
            vec![KeyboardAnomaly::ForeignValueNames {
                names: vec![
                    "OverrideKeyboardType".into(),
                    "OverrideKeyboardSubtype".into()
                ]
            }]
        );

        let hid_names_on_ps2 = KeyboardDevice {
            overrides: DeviceOverrides {
                keyboard_type_override: Some(7),
                keyboard_subtype_override: Some(2),
                number_total_keys_override: Some(106),
                override_keyboard_type: Some(7),
                ..Default::default()
            },
            ..fixtures::internal_ps2()
        };
        assert_eq!(
            keyboard_anomalies(&hid_names_on_ps2, &fixtures::global_per_keyboard()),
            vec![
                KeyboardAnomaly::IncompletePair {
                    present: "OverrideKeyboardType".into(),
                    missing: "OverrideKeyboardSubtype".into()
                },
                KeyboardAnomaly::ForeignValueNames {
                    names: vec![
                        "KeyboardTypeOverride".into(),
                        "KeyboardSubtypeOverride".into(),
                        "KeyboardNumberTotalKeysOverride".into()
                    ]
                },
            ]
        );

        // kbdhid reads the KeyboardNumber*Override extras itself.
        let extras = KeyboardDevice {
            overrides: DeviceOverrides {
                number_total_keys_override: Some(104),
                keyboard_subtype_override: Some(0),
                ..Default::default()
            },
            ..fixtures::keychron()
        };
        assert_eq!(
            keyboard_anomalies(&extras, &fixtures::global_per_keyboard()),
            vec![KeyboardAnomaly::IncompletePair {
                present: "KeyboardSubtypeOverride".into(),
                missing: "KeyboardTypeOverride".into()
            }]
        );
    }

    #[test]
    fn values_on_non_keyboard_collection() {
        // e.g. 4/0 written to the Keychron mouse collection (COL03) by mistake.
        let mouse = DeviceOverrides {
            keyboard_type_override: Some(4),
            keyboard_subtype_override: Some(0),
            ..Default::default()
        };
        assert_eq!(
            non_keyboard_anomaly(&mouse),
            Some(KeyboardAnomaly::ValuesOnNonKeyboard {
                names: vec![
                    "KeyboardTypeOverride".into(),
                    "KeyboardSubtypeOverride".into()
                ]
            })
        );
        assert_eq!(non_keyboard_anomaly(&DeviceOverrides::default()), None);
    }

    #[test]
    fn flags_unverified_unexpected_and_unsupported() {
        let pk = fixtures::global_per_keyboard();
        let with = |ty: u32, sub: u32| KeyboardDevice {
            overrides: DeviceOverrides {
                keyboard_type_override: Some(ty),
                keyboard_subtype_override: Some(sub),
                ..Default::default()
            },
            ..fixtures::keychron()
        };
        assert_eq!(
            keyboard_anomalies(&with(7, 0), &pk),
            vec![KeyboardAnomaly::UnverifiedType {
                keyboard_type: KeyboardType::US_ON_JAPANESE
            }]
        );
        assert_eq!(
            keyboard_anomalies(&with(0x51, 0), &pk),
            vec![KeyboardAnomaly::UnexpectedType {
                keyboard_type: KeyboardType::HID_UNKNOWN
            }]
        );
        // NEC has a table of its own, but it is not verified on real hardware.
        assert_eq!(
            keyboard_anomalies(&with(7, 0xD02), &pk),
            vec![KeyboardAnomaly::UnverifiedType {
                keyboard_type: KeyboardType::NEC
            }]
        );
        assert_eq!(keyboard_anomalies(&with(7, 2), &pk), vec![]);

        let other = KeyboardDevice {
            driver: KeyboardDriver::Other("vendorkbd".into()),
            ..fixtures::keychron()
        };
        assert_eq!(
            keyboard_anomalies(&other, &pk),
            vec![KeyboardAnomaly::ValuesOnUnsupportedDriver {
                names: vec![
                    "KeyboardTypeOverride".into(),
                    "KeyboardSubtypeOverride".into()
                ]
            }]
        );
    }

    #[test]
    fn apply_actions() {
        assert_eq!(
            device_apply_action(&fixtures::keychron()),
            PendingAction::ResetKeyboard
        );
        assert_eq!(
            device_apply_action(&fixtures::vxe_ble()),
            PendingAction::Reconnect
        );
        assert_eq!(
            device_apply_action(&fixtures::internal_ps2()),
            PendingAction::RestartPc
        );
        let i2c = KeyboardDevice {
            transport: Transport::I2c,
            ..fixtures::keychron()
        };
        assert_eq!(device_apply_action(&i2c), PendingAction::RestartPc);
        // Unverified wireless transports reconnect; an unknown transport may be I2C.
        let bt = KeyboardDevice {
            transport: Transport::BluetoothClassic,
            ..fixtures::keychron()
        };
        assert_eq!(device_apply_action(&bt), PendingAction::Reconnect);
        let unknown = KeyboardDevice {
            transport: Transport::Unknown,
            ..fixtures::keychron()
        };
        assert_eq!(device_apply_action(&unknown), PendingAction::RestartPc);
        let null_container = KeyboardDevice {
            container_id: Some(crate::safety::NULL_CONTAINER_ID.into()),
            ..fixtures::keychron()
        };
        assert_eq!(
            device_apply_action(&null_container),
            PendingAction::RestartPc
        );
    }
}
