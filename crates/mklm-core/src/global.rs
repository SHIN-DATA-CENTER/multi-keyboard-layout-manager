//! Rules for the global values under `Services\i8042prt\Parameters`.

use serde::{Deserialize, Serialize};

use crate::layout::{LayoutTable, PendingAction};
use crate::model::{GlobalMode, GlobalSettings, KeyboardType, value_names};

impl GlobalSettings {
    /// Fixed when either global `OverrideKeyboardType` or `OverrideKeyboardSubtype` exists.
    pub fn mode(&self) -> GlobalMode {
        if self.override_keyboard_type.is_some() || self.override_keyboard_subtype.is_some() {
            GlobalMode::Fixed
        } else {
            GlobalMode::PerKeyboard
        }
    }

    /// The PC's standard layout, from `LayerDriver JPN`.
    pub fn standard_layout(&self) -> LayoutTable {
        LayoutTable::from_layer_driver(self.layer_driver_jpn.as_deref())
    }

    /// Global type/subtype when both values exist.
    pub fn fixed_type(&self) -> Option<KeyboardType> {
        Some(KeyboardType::new(
            self.override_keyboard_type?,
            self.override_keyboard_subtype?,
        ))
    }
}

/// Inconsistencies in the global values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum GlobalAnomaly {
    /// Only one of the global `OverrideKeyboardType/Subtype` exists (still fixed mode).
    IncompleteFixedType { present: String, missing: String },
    /// Fixed-mode type differs from what the Settings app writes for the standard layout
    /// (JIS → 7/2, US → 7/0).
    FixedTypeMismatch {
        keyboard_type: KeyboardType,
        expected: KeyboardType,
    },
    /// `LayerDriver JPN` names a DLL other than kbd106/kbd106n/kbd101.
    UnknownLayerDriver { layer_driver: String },
    /// `OverrideKeyboardIdentifier` does not match `LayerDriver JPN` (the IME reads it).
    IdentifierMismatch {
        identifier: Option<String>,
        expected: String,
    },
}

/// Lists every inconsistency in the global values.
pub fn global_anomalies(global: &GlobalSettings) -> Vec<GlobalAnomaly> {
    let mut anomalies = Vec::new();
    match (
        global.override_keyboard_type,
        global.override_keyboard_subtype,
    ) {
        (Some(_), None) => anomalies.push(GlobalAnomaly::IncompleteFixedType {
            present: value_names::PS2_TYPE.to_string(),
            missing: value_names::PS2_SUBTYPE.to_string(),
        }),
        (None, Some(_)) => anomalies.push(GlobalAnomaly::IncompleteFixedType {
            present: value_names::PS2_SUBTYPE.to_string(),
            missing: value_names::PS2_TYPE.to_string(),
        }),
        _ => {}
    }

    let standard = global.standard_layout();
    match standard.layout() {
        Some(layout) => {
            if let Some(ty) = global.fixed_type()
                && ty != layout.fixed_mode_type()
            {
                anomalies.push(GlobalAnomaly::FixedTypeMismatch {
                    keyboard_type: ty,
                    expected: layout.fixed_mode_type(),
                });
            }
            // A system that never had Japanese settings has none of the values; that is consistent.
            // A fixed pair always comes with the standard it belongs to.
            let untouched = global.layer_driver_jpn.is_none()
                && global.override_keyboard_identifier.is_none()
                && global.mode() == GlobalMode::PerKeyboard;
            let expected = layout.keyboard_identifier();
            let matches = global
                .override_keyboard_identifier
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(expected));
            if !untouched && !matches {
                anomalies.push(GlobalAnomaly::IdentifierMismatch {
                    identifier: global.override_keyboard_identifier.clone(),
                    expected: expected.to_string(),
                });
            }
        }
        None => anomalies.push(GlobalAnomaly::UnknownLayerDriver {
            layer_driver: global.layer_driver_jpn.clone().unwrap_or_default(),
        }),
    }
    anomalies
}

/// Action needed before a change of the global values takes effect.
///
/// user32 and the IME read them at sign-in and i8042prt at boot, so any change needs a PC restart
/// (signing out is enough for HID keyboards only; MKLM always asks for a restart).
pub fn global_change_action(
    before: &GlobalSettings,
    after: &GlobalSettings,
) -> Option<PendingAction> {
    let same_str = |a: &Option<String>, b: &Option<String>| match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    };
    let unchanged = same_str(&before.layer_driver_jpn, &after.layer_driver_jpn)
        && same_str(
            &before.override_keyboard_identifier,
            &after.override_keyboard_identifier,
        )
        && before.override_keyboard_type == after.override_keyboard_type
        && before.override_keyboard_subtype == after.override_keyboard_subtype;
    (!unchanged).then_some(PendingAction::RestartPc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn mode_detection() {
        assert_eq!(
            fixtures::global_per_keyboard().mode(),
            GlobalMode::PerKeyboard
        );
        assert_eq!(fixtures::global_fixed_jis().mode(), GlobalMode::Fixed);
        let only_type = GlobalSettings {
            override_keyboard_type: Some(7),
            ..fixtures::global_per_keyboard()
        };
        assert_eq!(only_type.mode(), GlobalMode::Fixed);
        assert_eq!(only_type.fixed_type(), None);
        let only_subtype = GlobalSettings {
            override_keyboard_subtype: Some(2),
            ..fixtures::global_per_keyboard()
        };
        assert_eq!(only_subtype.mode(), GlobalMode::Fixed);
        assert_eq!(GlobalSettings::default().mode(), GlobalMode::PerKeyboard);
    }

    #[test]
    fn standard_layout() {
        assert_eq!(
            fixtures::global_per_keyboard().standard_layout(),
            LayoutTable::Jis
        );
        assert_eq!(GlobalSettings::default().standard_layout(), LayoutTable::Us);
        assert_eq!(
            fixtures::global_fixed_jis().fixed_type(),
            Some(KeyboardType::JIS)
        );
    }

    #[test]
    fn dev_machine_globals_are_consistent() {
        assert!(global_anomalies(&fixtures::global_per_keyboard()).is_empty());
        assert!(global_anomalies(&fixtures::global_fixed_jis()).is_empty());
        assert!(global_anomalies(&GlobalSettings::default()).is_empty());
        let settings_app_english = GlobalSettings {
            layer_driver_jpn: Some("kbd101.dll".into()),
            layer_driver_kor: None,
            override_keyboard_identifier: Some("PCAT_101KEY".into()),
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(0),
        };
        assert!(global_anomalies(&settings_app_english).is_empty());
    }

    #[test]
    fn flags_inconsistent_globals() {
        let g = GlobalSettings {
            layer_driver_jpn: Some("kbd106.dll".into()),
            override_keyboard_identifier: Some("PCAT_101KEY".into()),
            override_keyboard_type: Some(7),
            override_keyboard_subtype: None,
            ..Default::default()
        };
        let anomalies = global_anomalies(&g);
        assert!(anomalies.contains(&GlobalAnomaly::IncompleteFixedType {
            present: "OverrideKeyboardType".into(),
            missing: "OverrideKeyboardSubtype".into(),
        }));
        assert!(anomalies.contains(&GlobalAnomaly::IdentifierMismatch {
            identifier: Some("PCAT_101KEY".into()),
            expected: "PCAT_106KEY".into(),
        }));

        let mismatch = GlobalSettings {
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(0),
            ..fixtures::global_per_keyboard()
        };
        assert_eq!(
            global_anomalies(&mismatch),
            vec![GlobalAnomaly::FixedTypeMismatch {
                keyboard_type: KeyboardType::US_ON_JAPANESE,
                expected: KeyboardType::JIS,
            }]
        );

        let other = GlobalSettings {
            layer_driver_jpn: Some("kbdnec.dll".into()),
            ..Default::default()
        };
        assert_eq!(
            global_anomalies(&other),
            vec![GlobalAnomaly::UnknownLayerDriver {
                layer_driver: "kbdnec.dll".into()
            }]
        );

        let missing_identifier = GlobalSettings {
            layer_driver_jpn: Some("kbd106.dll".into()),
            ..Default::default()
        };
        assert_eq!(
            global_anomalies(&missing_identifier),
            vec![GlobalAnomaly::IdentifierMismatch {
                identifier: None,
                expected: "PCAT_106KEY".into(),
            }]
        );

        // A fixed pair without any standard is not an untouched system.
        let bare_fixed = GlobalSettings {
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(0),
            ..Default::default()
        };
        assert_eq!(
            global_anomalies(&bare_fixed),
            vec![GlobalAnomaly::IdentifierMismatch {
                identifier: None,
                expected: "PCAT_101KEY".into(),
            }]
        );
    }

    #[test]
    fn global_changes_need_restart() {
        let before = fixtures::global_fixed_jis();
        assert_eq!(global_change_action(&before, &before), None);
        let lower = GlobalSettings {
            layer_driver_jpn: Some("KBD106.DLL".into()),
            ..before.clone()
        };
        assert_eq!(global_change_action(&before, &lower), None);
        assert_eq!(
            global_change_action(&before, &fixtures::global_per_keyboard()),
            Some(PendingAction::RestartPc)
        );
        let kor_only = GlobalSettings {
            layer_driver_kor: None,
            ..before.clone()
        };
        assert_eq!(global_change_action(&before, &kor_only), None);
    }
}
