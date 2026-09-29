//! One-call evaluation of a [`SystemSnapshot`]: what every keyboard types now, what it will type after
//! the next restart, and what needs attention. `mklm-cli status --json` prints [`Assessment`] as is.

use serde::{Deserialize, Serialize};

use crate::device::{KeyboardAnomaly, device_apply_action, keyboard_anomalies};
use crate::global::{GlobalAnomaly, global_anomalies};
use crate::group::{DeviceGroup, group_keyboards};
use crate::input::{InputWarning, input_warnings};
use crate::layout::{EffectiveLayout, LayoutBasis, LayoutTable, PendingAction, effective_layout};
use crate::model::{
    GlobalMode, KeyboardDevice, KeyboardDriver, KeyboardType, SystemSnapshot, Transport,
};
use crate::safety::{InvPs2Violation, check_inv_ps2};

/// Evaluation of one keyboard devnode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardAssessment {
    pub instance_id: String,
    pub display_name: String,
    pub present: bool,
    /// In the built-in container (see [`KeyboardDevice::in_internal_container`]).
    pub is_internal: bool,
    pub driver: KeyboardDriver,
    pub transport: Transport,
    /// Complete type/subtype pair stored for the keyboard's driver.
    pub stored_type: Option<KeyboardType>,
    /// Type the driver reports now; `None` when not present.
    pub reported_type: Option<KeyboardType>,
    /// Type the driver will report after the next reset, reconnect or PC restart.
    pub predicted_type: Option<KeyboardType>,
    /// Layout typed now, from the reported type.
    pub current: Option<EffectiveLayout>,
    /// Layout typed once the stored values are applied, from the predicted type.
    pub after_restart: Option<EffectiveLayout>,
    /// Set when `current` and `after_restart` differ. In fixed mode only the table counts, because
    /// the type does not change it there (M0 #2a); an ignored type shows up in `anomalies` instead.
    pub pending_action: Option<PendingAction>,
    pub anomalies: Vec<KeyboardAnomaly>,
}

/// Whether INV-PS2 holds on the stored values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvPs2Status {
    Holds,
    Violated,
    /// No violation among the listed keyboards, but the snapshot left out disconnected ones,
    /// which INV-PS2 also covers.
    Unknown,
}

/// Evaluation of a whole [`SystemSnapshot`], assuming the Japanese IME is the active input method
/// (`input_warnings` says when it may not be).
///
/// Both `current` and `after_restart` use the stored global values; a global change that has not been
/// followed by a restart yet is not visible in one snapshot (the journal tracks it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assessment {
    pub mode: GlobalMode,
    /// The PC's standard layout (`LayerDriver JPN`).
    pub standard_layout: LayoutTable,
    pub global_anomalies: Vec<GlobalAnomaly>,
    pub input_warnings: Vec<InputWarning>,
    /// INV-PS2 on the stored values, taking into account whether every keyboard was listed.
    pub inv_ps2: InvPs2Status,
    /// The keyboards that violate INV-PS2; `None` when none of the listed keyboards does.
    pub inv_ps2_violation: Option<InvPs2Violation>,
    /// Heaviest pending action over all keyboards.
    pub pending_action: Option<PendingAction>,
    pub keyboards: Vec<KeyboardAssessment>,
    pub groups: Vec<DeviceGroup>,
}

/// Evaluates a snapshot that lists every keyboard, disconnected ones too. Pure: reads nothing from
/// the system. See [`assess_with`] for a snapshot of connected keyboards only.
pub fn assess(snapshot: &SystemSnapshot) -> Assessment {
    assess_with(snapshot, true)
}

/// Evaluates a snapshot. `all_keyboards` says whether `snapshot.keyboards` includes the
/// disconnected (phantom) keyboards; without them INV-PS2 can be found violated but not confirmed.
pub fn assess_with(snapshot: &SystemSnapshot, all_keyboards: bool) -> Assessment {
    let global = &snapshot.global;
    let mode = global.mode();
    let standard = global.standard_layout();
    let keyboards: Vec<KeyboardAssessment> = snapshot
        .keyboards
        .iter()
        .map(|kb| assess_keyboard(kb, snapshot, mode, &standard))
        .collect();
    let inv_ps2_violation = check_inv_ps2(global, &snapshot.keyboards).err();
    let inv_ps2 = match (&inv_ps2_violation, all_keyboards) {
        (Some(_), _) => InvPs2Status::Violated,
        (None, true) => InvPs2Status::Holds,
        (None, false) => InvPs2Status::Unknown,
    };
    Assessment {
        mode,
        global_anomalies: global_anomalies(global),
        input_warnings: input_warnings(&snapshot.input),
        inv_ps2,
        inv_ps2_violation,
        pending_action: keyboards.iter().filter_map(|k| k.pending_action).max(),
        groups: group_keyboards(&snapshot.keyboards),
        standard_layout: standard,
        keyboards,
    }
}

fn assess_keyboard(
    kb: &KeyboardDevice,
    snapshot: &SystemSnapshot,
    mode: GlobalMode,
    standard: &LayoutTable,
) -> KeyboardAssessment {
    let predicted_type = kb.predicted_type(&snapshot.global);
    let reported_type = if kb.present { kb.reported_type } else { None };
    // A standard inferred from a missing `LayerDriver JPN` is not confirmed on real hardware.
    let standard_stored = snapshot.global.layer_driver_jpn.is_some();
    let layout = |ty| {
        let mut layout = effective_layout(mode, standard, ty);
        if layout.basis != LayoutBasis::KeyboardType && !standard_stored {
            layout.verified = false;
        }
        layout
    };
    let current = reported_type.map(layout);
    let after_restart = predicted_type.map(layout);
    let pending_action = match (&current, &after_restart) {
        (Some(now), Some(next)) if mode == GlobalMode::Fixed && now.table == next.table => None,
        (Some(now), Some(next)) if now != next => Some(device_apply_action(kb)),
        _ => None,
    };
    KeyboardAssessment {
        instance_id: kb.instance_id.clone(),
        display_name: kb.display_name.clone(),
        present: kb.present,
        is_internal: kb.in_internal_container(),
        driver: kb.driver.clone(),
        transport: kb.transport,
        stored_type: kb.stored_type(),
        reported_type,
        predicted_type,
        current,
        after_restart,
        pending_action,
        anomalies: keyboard_anomalies(kb, &snapshot.global),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::DeviceOverrides;

    fn find<'a>(a: &'a Assessment, instance_id: &str) -> &'a KeyboardAssessment {
        a.keyboards
            .iter()
            .find(|k| k.instance_id == instance_id)
            .unwrap()
    }

    fn table(layout: &Option<EffectiveLayout>) -> Option<LayoutTable> {
        layout.as_ref().map(|l| l.table.clone())
    }

    #[test]
    fn dev_machine_after_m0() {
        let snapshot = fixtures::dev_machine();
        let a = assess(&snapshot);
        assert_eq!(a.mode, GlobalMode::PerKeyboard);
        assert_eq!(a.standard_layout, LayoutTable::Jis);
        assert_eq!(a.global_anomalies, vec![]);
        assert_eq!(a.inv_ps2_violation, None);
        assert_eq!(a.pending_action, None);
        assert_eq!(a.groups.len(), 4);

        // M0 #4 / G1: the internal PS/2 keyboard stays JIS through its own 7/2.
        let ps2 = find(&a, &fixtures::internal_ps2().instance_id);
        assert_eq!(table(&ps2.current), Some(LayoutTable::Jis));
        assert_eq!(table(&ps2.after_restart), Some(LayoutTable::Jis));
        assert_eq!(
            ps2.current.as_ref().unwrap().basis,
            LayoutBasis::KeyboardType
        );
        assert!(ps2.is_internal);

        // M0 #4 / G4: Keychron 4/0 types US.
        let keychron = find(&a, &fixtures::keychron().instance_id);
        assert_eq!(table(&keychron.current), Some(LayoutTable::Us));
        assert_eq!(keychron.stored_type, Some(KeyboardType::US));
        assert!(keychron.current.as_ref().unwrap().verified);
        assert_eq!(keychron.pending_action, None);

        // 0x51 follows the standard (JIS).
        let vxe = find(&a, &fixtures::vxe_ble().instance_id);
        assert_eq!(table(&vxe.current), Some(LayoutTable::Jis));
        assert_eq!(vxe.current.as_ref().unwrap().basis, LayoutBasis::Standard);
        assert_eq!(vxe.stored_type, None);

        // Phantom: nothing typed now, standard once it connects, nothing to do.
        let phantom = find(&a, &fixtures::ms_ble_phantom().instance_id);
        assert_eq!(phantom.current, None);
        assert_eq!(table(&phantom.after_restart), Some(LayoutTable::Jis));
        assert_eq!(phantom.pending_action, None);

        // 00000409 is installed.
        assert!(matches!(
            a.input_warnings.as_slice(),
            [InputWarning::NonJapaneseLayouts { preload, .. }] if preload == &vec!["00000409".to_string()]
        ));
    }

    #[test]
    fn fixed_mode_keeps_keychron_jis() {
        // M0 #2a: 4/0 is reported, but fixed mode maps every keyboard to the standard.
        let snapshot = SystemSnapshot {
            global: fixtures::global_fixed_jis(),
            ..fixtures::dev_machine()
        };
        let a = assess(&snapshot);
        assert_eq!(a.mode, GlobalMode::Fixed);
        let keychron = find(&a, &fixtures::keychron().instance_id);
        assert_eq!(keychron.reported_type, Some(KeyboardType::US));
        assert_eq!(table(&keychron.current), Some(LayoutTable::Jis));
        assert_eq!(
            keychron.current.as_ref().unwrap().basis,
            LayoutBasis::FixedMode
        );
        assert_eq!(keychron.pending_action, None);
        assert_eq!(
            keychron.anomalies,
            vec![KeyboardAnomaly::IgnoredInFixedMode {
                keyboard_type: KeyboardType::US
            }]
        );
        assert_eq!(a.inv_ps2_violation, None);
        assert_eq!(a.inv_ps2, InvPs2Status::Holds);
    }

    #[test]
    fn fixed_mode_type_change_is_not_pending() {
        // M0 #2a before the dongle was replugged: 4/0 stored, 0x51 reported. A reset would change the
        // reported type but not the table, so nothing is pending; the anomaly asks for migration.
        let not_reset = KeyboardDevice {
            reported_type: Some(KeyboardType::HID_UNKNOWN),
            ..fixtures::keychron()
        };
        let snapshot = SystemSnapshot {
            keyboards: vec![fixtures::internal_ps2(), not_reset.clone()],
            global: fixtures::global_fixed_jis(),
            ..fixtures::dev_machine()
        };
        let a = assess(&snapshot);
        let keychron = find(&a, &not_reset.instance_id);
        assert_eq!(keychron.predicted_type, Some(KeyboardType::US));
        assert_eq!(table(&keychron.current), Some(LayoutTable::Jis));
        assert_eq!(table(&keychron.after_restart), Some(LayoutTable::Jis));
        assert_eq!(keychron.pending_action, None);
        assert_eq!(a.pending_action, None);
        assert_eq!(
            keychron.anomalies,
            vec![KeyboardAnomaly::IgnoredInFixedMode {
                keyboard_type: KeyboardType::US
            }]
        );
    }

    #[test]
    fn inv_ps2_needs_every_keyboard_to_hold() {
        let phantom_ps2 = KeyboardDevice {
            instance_id: r"ACPI\PNP0303\4&1&0".into(),
            present: false,
            reported_type: None,
            overrides: DeviceOverrides::default(),
            ..fixtures::internal_ps2()
        };
        let all = SystemSnapshot {
            keyboards: vec![fixtures::internal_ps2(), phantom_ps2, fixtures::keychron()],
            ..fixtures::dev_machine()
        };
        assert_eq!(assess(&all).inv_ps2, InvPs2Status::Violated);
        assert_eq!(assess_with(&all, false).inv_ps2, InvPs2Status::Violated);

        // Leaving the phantom out hides the violation, so "holds" cannot be claimed.
        let connected = SystemSnapshot {
            keyboards: vec![fixtures::internal_ps2(), fixtures::keychron()],
            ..fixtures::dev_machine()
        };
        let a = assess_with(&connected, false);
        assert_eq!(a.inv_ps2_violation, None);
        assert_eq!(a.inv_ps2, InvPs2Status::Unknown);
        assert_eq!(
            serde_json::to_value(&a).unwrap()["inv_ps2"],
            serde_json::json!("unknown")
        );
        assert_eq!(assess(&connected).inv_ps2, InvPs2Status::Holds);
    }

    #[test]
    fn inferred_standard_is_not_verified() {
        // Without LayerDriver JPN the standard reads as US, which nothing has confirmed.
        let snapshot = SystemSnapshot {
            global: crate::model::GlobalSettings {
                override_keyboard_type: Some(7),
                override_keyboard_subtype: Some(0),
                ..Default::default()
            },
            ..fixtures::dev_machine()
        };
        let a = assess(&snapshot);
        let keychron = find(&a, &fixtures::keychron().instance_id);
        assert_eq!(table(&keychron.current), Some(LayoutTable::Us));
        assert!(!keychron.current.as_ref().unwrap().verified);
        // The stored JIS standard in fixed mode is verified (M0 #2a).
        let fixed = SystemSnapshot {
            global: fixtures::global_fixed_jis(),
            ..fixtures::dev_machine()
        };
        let a = assess(&fixed);
        assert!(
            find(&a, &fixtures::keychron().instance_id)
                .current
                .as_ref()
                .unwrap()
                .verified
        );
    }

    #[test]
    fn ps2_without_device_values_turns_us_after_restart() {
        let bare = KeyboardDevice {
            overrides: DeviceOverrides::default(),
            ..fixtures::internal_ps2()
        };
        let snapshot = SystemSnapshot {
            keyboards: vec![bare.clone(), fixtures::keychron()],
            ..fixtures::dev_machine()
        };
        let a = assess(&snapshot);
        let ps2 = find(&a, &bare.instance_id);
        assert_eq!(ps2.predicted_type, Some(KeyboardType::US));
        assert_eq!(table(&ps2.after_restart), Some(LayoutTable::Us));
        assert_eq!(table(&ps2.current), Some(LayoutTable::Jis));
        assert_eq!(ps2.pending_action, Some(PendingAction::RestartPc));
        assert_eq!(a.pending_action, Some(PendingAction::RestartPc));
        assert_eq!(
            a.inv_ps2_violation,
            Some(InvPs2Violation {
                keyboards: vec![bare.instance_id.clone()]
            })
        );
    }

    #[test]
    fn written_but_not_reset_is_pending() {
        // Per-keyboard mode, 4/0 written, keyboard not restarted yet: still 0x51 → standard JIS.
        let not_reset = KeyboardDevice {
            reported_type: Some(KeyboardType::HID_UNKNOWN),
            ..fixtures::keychron()
        };
        let ble_written = KeyboardDevice {
            overrides: DeviceOverrides {
                keyboard_type_override: Some(4),
                keyboard_subtype_override: Some(0),
                ..Default::default()
            },
            ..fixtures::vxe_ble()
        };
        let snapshot = SystemSnapshot {
            keyboards: vec![
                fixtures::internal_ps2(),
                not_reset.clone(),
                ble_written.clone(),
            ],
            ..fixtures::dev_machine()
        };
        let a = assess(&snapshot);
        let keychron = find(&a, &not_reset.instance_id);
        assert_eq!(table(&keychron.current), Some(LayoutTable::Jis));
        assert_eq!(table(&keychron.after_restart), Some(LayoutTable::Us));
        assert_eq!(keychron.pending_action, Some(PendingAction::ResetKeyboard));
        let ble = find(&a, &ble_written.instance_id);
        assert_eq!(ble.pending_action, Some(PendingAction::Reconnect));
        assert_eq!(a.pending_action, Some(PendingAction::Reconnect));
    }

    #[test]
    fn remote_desktop_keyboard_has_no_layout_and_no_anomalies() {
        // Raw Input names no device for it, so nothing is reported; terminpt reads no values.
        let rdp = fixtures::rdp_keyboard();
        let snapshot = SystemSnapshot {
            keyboards: vec![fixtures::internal_ps2(), rdp.clone()],
            ..fixtures::dev_machine()
        };
        let a = assess(&snapshot);
        let ka = find(&a, &rdp.instance_id);
        assert_eq!(ka.transport, Transport::Virtual);
        assert!(ka.is_internal);
        assert_eq!(
            (ka.stored_type, ka.reported_type, ka.predicted_type),
            (None, None, None)
        );
        assert_eq!((&ka.current, &ka.after_restart), (&None, &None));
        assert_eq!(ka.pending_action, None);
        assert_eq!(ka.anomalies, vec![]);
        assert_eq!(a.pending_action, None);
        assert_eq!(a.inv_ps2, InvPs2Status::Holds);
        // A row of its own, although it shares the built-in container.
        assert!(
            a.groups
                .iter()
                .any(|g| g.keyboards == vec![rdp.instance_id.clone()])
        );
    }

    /// Synthetic IDs and service (not what Windows uses; see `fixtures::rdp_keyboard`).
    #[test]
    fn unsupported_driver_has_no_prediction() {
        let rdp = KeyboardDevice {
            instance_id: r"ROOT\RDP_KBD\0000".into(),
            driver: KeyboardDriver::Other("TermDD".into()),
            transport: Transport::Virtual,
            overrides: DeviceOverrides::default(),
            reported_type: Some(KeyboardType::JIS),
            container_id: Some(crate::model::INTERNAL_CONTAINER_ID.into()),
            is_internal: true,
            ..fixtures::keychron()
        };
        let snapshot = SystemSnapshot {
            keyboards: vec![rdp],
            ..fixtures::dev_machine()
        };
        let a = assess(&snapshot);
        assert_eq!(a.keyboards[0].predicted_type, None);
        assert_eq!(a.keyboards[0].after_restart, None);
        assert_eq!(a.keyboards[0].pending_action, None);
        assert_eq!(table(&a.keyboards[0].current), Some(LayoutTable::Jis));
    }

    #[test]
    fn snapshot_json_round_trip() {
        let snapshot = fixtures::dev_machine();
        let json = serde_json::to_string_pretty(&snapshot).unwrap();
        let back: SystemSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snapshot);

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["keyboards"][0]["driver"]["kind"], "i8042prt");
        assert_eq!(value["keyboards"][0]["reported_type"]["type"], 7);
        assert_eq!(value["keyboards"][2]["transport"], "bluetooth-le");
        assert_eq!(
            value["global"]["override_keyboard_type"],
            serde_json::Value::Null
        );

        // What a Remote Desktop client reported: added later, so documents without it still read.
        let mut remote = snapshot.clone();
        remote.os.remote_session = true;
        remote.os.client_keyboard_type = Some(KeyboardType::JIS);
        let value = serde_json::to_value(&remote).unwrap();
        assert_eq!(
            value["os"]["client_keyboard_type"],
            serde_json::json!({ "type": 7, "subtype": 2 })
        );
        assert_eq!(
            serde_json::from_value::<SystemSnapshot>(value).unwrap(),
            remote
        );
        let mut old = serde_json::to_value(&snapshot).unwrap();
        old["os"]
            .as_object_mut()
            .unwrap()
            .remove("client_keyboard_type");
        assert_eq!(
            serde_json::from_value::<SystemSnapshot>(old).unwrap(),
            snapshot
        );

        // The PC's name (design standard-layout UX-5): added later too.
        let mut named = snapshot.clone();
        named.os.computer_name = Some("DESKTOP-3TCSIET".into());
        let value = serde_json::to_value(&named).unwrap();
        assert_eq!(value["os"]["computer_name"], "DESKTOP-3TCSIET");
        let mut old = value;
        old["os"].as_object_mut().unwrap().remove("computer_name");
        assert_eq!(
            serde_json::from_value::<SystemSnapshot>(old).unwrap(),
            snapshot
        );
    }

    #[test]
    fn assessment_json_shape_and_round_trip() {
        let a = assess(&fixtures::dev_machine());
        let value = serde_json::to_value(&a).unwrap();
        assert_eq!(value["mode"], "per-keyboard");
        assert_eq!(value["standard_layout"], "jis");
        assert_eq!(value["keyboards"][1]["current"]["table"], "us");
        assert_eq!(value["keyboards"][1]["current"]["basis"], "keyboard-type");
        assert_eq!(value["input_warnings"][0]["kind"], "non-japanese-layouts");
        assert_eq!(value["pending_action"], serde_json::Value::Null);
        let back: Assessment = serde_json::from_value(value).unwrap();
        assert_eq!(back, a);

        // Every enum shape survives a round trip.
        let bare = KeyboardDevice {
            overrides: DeviceOverrides {
                override_keyboard_type: Some(7),
                keyboard_type_override: Some(4),
                ..Default::default()
            },
            ..fixtures::internal_ps2()
        };
        let messy = SystemSnapshot {
            keyboards: vec![bare],
            global: crate::model::GlobalSettings {
                layer_driver_jpn: Some("kbdnec.dll".into()),
                ..Default::default()
            },
            ..fixtures::dev_machine()
        };
        let a = assess(&messy);
        assert!(a.inv_ps2_violation.is_some());
        assert!(!a.global_anomalies.is_empty());
        assert!(!a.keyboards[0].anomalies.is_empty());
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains(r#""standard_layout":{"other":"kbdnec.dll"}"#));
        assert_eq!(serde_json::from_str::<Assessment>(&json).unwrap(), a);
    }
}
