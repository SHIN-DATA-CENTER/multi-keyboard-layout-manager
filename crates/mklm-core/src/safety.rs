//! Safety rules from the plan (sections 1.3 and 1.4), as pure functions for the writer.

use serde::{Deserialize, Serialize};

use crate::model::{
    GlobalSettings, INTERNAL_CONTAINER_ID, KeyboardDevice, KeyboardDriver, Transport,
};

/// `DN_STARTED` devnode status flag.
pub const DN_STARTED: u32 = 0x0000_0008;
/// `DN_HAS_PROBLEM` devnode status flag.
pub const DN_HAS_PROBLEM: u32 = 0x0000_0400;
/// `GUID_NULL` as a container ID: identifies no physical device.
pub const NULL_CONTAINER_ID: &str = "{00000000-0000-0000-0000-000000000000}";
/// Transports on which a live reset was verified to apply a change (M0 #2b: USB). Grows as M0
/// verifies more (plan gate G2).
pub const LIVE_RESET_TRANSPORTS: [Transport; 1] = [Transport::Usb];

impl KeyboardDevice {
    /// True when the keyboard belongs to the computer's built-in container.
    /// Checks the container ID as well as `is_internal`, so a half-filled record errs on the safe side.
    pub fn in_internal_container(&self) -> bool {
        self.is_internal
            || self
                .container_id
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(INTERNAL_CONTAINER_ID))
    }

    /// The container ID, unless it is missing or [`NULL_CONTAINER_ID`] (neither identifies a device).
    pub fn known_container_id(&self) -> Option<&str> {
        self.container_id
            .as_deref()
            .filter(|c| !c.eq_ignore_ascii_case(NULL_CONTAINER_ID))
    }
}

/// Reason a keyboard must not be restarted in place (plan 1.4). Such keyboards apply changes on PC
/// restart, or by reconnecting when every ban [`allows_reconnect`](ResetBan::allows_reconnect).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResetBan {
    /// Served by i8042prt.
    Ps2Driver,
    /// Neither kbdhid nor i8042prt: MKLM does not touch it.
    UnsupportedDriver,
    /// Part of the built-in container.
    InternalContainer,
    /// No usable container ID, so it cannot be shown to be external.
    UnknownContainer,
    /// Attached through I2C or SPI.
    I2cOrSpi,
    /// Remote Desktop, VM or software keyboard.
    Virtual,
    /// The transport is unknown, so it may be I2C or SPI.
    UnknownTransport,
    /// A live reset is not verified for this transport (plan gate G2); reconnecting applies the change.
    UnprovenTransport {
        transport: Transport,
    },
    NotPresent,
    /// Devnode status is unknown, so `DN_STARTED` cannot be confirmed.
    StatusUnknown,
    /// Devnode is not started.
    NotStarted,
    /// Devnode reports a problem code.
    HasProblem {
        code: u32,
    },
    /// The user has no other keyboard to type with if the reset goes wrong.
    OnlyUsableKeyboard,
}

impl ResetBan {
    /// True when the ban only means a live reset is unproven: reconnecting the keyboard still applies
    /// a change. Every other ban needs a PC restart.
    pub fn allows_reconnect(&self) -> bool {
        matches!(self, ResetBan::UnprovenTransport { .. })
    }
}

/// Bans that follow from what the keyboard is, independent of its current state.
/// `DN_REMOVABLE` is deliberately not used: HID collections never have it and BLE chains lack it entirely.
/// Only transports in [`LIVE_RESET_TRANSPORTS`] escape a transport ban.
pub fn structural_reset_bans(device: &KeyboardDevice) -> Vec<ResetBan> {
    let mut bans = Vec::new();
    match device.driver {
        KeyboardDriver::I8042prt => bans.push(ResetBan::Ps2Driver),
        KeyboardDriver::Other(_) => bans.push(ResetBan::UnsupportedDriver),
        KeyboardDriver::Kbdhid => {}
    }
    if device.in_internal_container() {
        bans.push(ResetBan::InternalContainer);
    } else if device.known_container_id().is_none() {
        bans.push(ResetBan::UnknownContainer);
    }
    match device.transport {
        Transport::I2c | Transport::Spi => bans.push(ResetBan::I2cOrSpi),
        Transport::Virtual => bans.push(ResetBan::Virtual),
        Transport::Unknown => bans.push(ResetBan::UnknownTransport),
        // Banned through its driver already (the transport is derived from it).
        Transport::Ps2 if device.driver == KeyboardDriver::I8042prt => {}
        transport if !LIVE_RESET_TRANSPORTS.contains(&transport) => {
            bans.push(ResetBan::UnprovenTransport { transport });
        }
        _ => {}
    }
    bans
}

/// Every reason the keyboard must not be reset in place right now; empty means a live reset is allowed.
///
/// `only_usable_keyboard` is true when this is the only keyboard the user can currently type with
/// (e.g. the only one with recent input).
pub fn live_reset_bans(device: &KeyboardDevice, only_usable_keyboard: bool) -> Vec<ResetBan> {
    let mut bans = structural_reset_bans(device);
    if !device.present {
        bans.push(ResetBan::NotPresent);
    }
    match device.dev_node_status {
        None => bans.push(ResetBan::StatusUnknown),
        Some(status) if status & DN_STARTED == 0 => bans.push(ResetBan::NotStarted),
        Some(_) => {}
    }
    match (device.problem_code, device.dev_node_status) {
        (Some(code), _) if code != 0 => bans.push(ResetBan::HasProblem { code }),
        (_, Some(status)) if status & DN_HAS_PROBLEM != 0 => bans.push(ResetBan::HasProblem {
            code: device.problem_code.unwrap_or(0),
        }),
        _ => {}
    }
    if only_usable_keyboard {
        bans.push(ResetBan::OnlyUsableKeyboard);
    }
    bans
}

/// True when [`live_reset_bans`] finds nothing.
pub fn can_live_reset(device: &KeyboardDevice, only_usable_keyboard: bool) -> bool {
    live_reset_bans(device, only_usable_keyboard).is_empty()
}

/// INV-PS2 is violated: without a global type/subtype pair, i8042prt reports 4/0 (US) for these keyboards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error(
    "INV-PS2 violated: no global OverrideKeyboardType/Subtype pair and {} i8042prt keyboard(s) lack explicit OverrideKeyboardType/Subtype: {}",
    keyboards.len(),
    keyboards.join(", ")
)]
pub struct InvPs2Violation {
    /// Instance IDs of the i8042prt keyboards without both device values.
    pub keyboards: Vec<String>,
}

/// Checks INV-PS2 on the state after a planned change.
///
/// `keyboards` must include every i8042prt keyboard, present or not (phantoms too); other drivers are
/// ignored. The global pair counts only when both values exist, and a device counts only when both
/// `OverrideKeyboardType` and `OverrideKeyboardSubtype` exist, because the drivers' handling of a lone
/// value is unverified.
pub fn check_inv_ps2(
    global: &GlobalSettings,
    keyboards: &[KeyboardDevice],
) -> Result<(), InvPs2Violation> {
    if global.fixed_type().is_some() {
        return Ok(());
    }
    let missing: Vec<String> = keyboards
        .iter()
        .filter(|kb| kb.driver == KeyboardDriver::I8042prt && kb.overrides.ps2_type().is_none())
        .map(|kb| kb.instance_id.clone())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(InvPs2Violation { keyboards: missing })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::DeviceOverrides;

    #[test]
    fn keychron_may_be_reset_live() {
        let kb = fixtures::keychron();
        assert_eq!(live_reset_bans(&kb, false), vec![]);
        assert!(can_live_reset(&kb, false));
        assert_eq!(
            live_reset_bans(&kb, true),
            vec![ResetBan::OnlyUsableKeyboard]
        );
    }

    #[test]
    fn internal_ps2_is_banned() {
        let kb = fixtures::internal_ps2();
        let bans = live_reset_bans(&kb, false);
        assert!(bans.contains(&ResetBan::Ps2Driver));
        assert!(bans.contains(&ResetBan::InternalContainer));
    }

    #[test]
    fn internal_detected_from_container_alone() {
        let kb = KeyboardDevice {
            is_internal: false,
            container_id: Some("{00000000-0000-0000-ffff-ffffffffffff}".into()),
            ..fixtures::keychron()
        };
        assert!(kb.in_internal_container());
        assert_eq!(
            structural_reset_bans(&kb),
            vec![ResetBan::InternalContainer]
        );
    }

    #[test]
    fn i2c_spi_virtual_and_unknown_container_are_banned() {
        let i2c = KeyboardDevice {
            transport: Transport::I2c,
            ..fixtures::keychron()
        };
        assert_eq!(structural_reset_bans(&i2c), vec![ResetBan::I2cOrSpi]);
        let spi = KeyboardDevice {
            transport: Transport::Spi,
            ..fixtures::keychron()
        };
        assert_eq!(structural_reset_bans(&spi), vec![ResetBan::I2cOrSpi]);
        // Synthetic service (not what Windows uses), then the real Remote Desktop keyboard.
        let rdp = KeyboardDevice {
            transport: Transport::Virtual,
            driver: KeyboardDriver::Other("TermDD".into()),
            ..fixtures::keychron()
        };
        assert_eq!(
            structural_reset_bans(&rdp),
            vec![ResetBan::UnsupportedDriver, ResetBan::Virtual]
        );
        assert_eq!(
            structural_reset_bans(&fixtures::rdp_keyboard()),
            vec![
                ResetBan::UnsupportedDriver,
                ResetBan::InternalContainer,
                ResetBan::Virtual
            ]
        );
        let no_container = KeyboardDevice {
            container_id: None,
            ..fixtures::keychron()
        };
        assert_eq!(
            structural_reset_bans(&no_container),
            vec![ResetBan::UnknownContainer]
        );
        let null_container = KeyboardDevice {
            container_id: Some(NULL_CONTAINER_ID.into()),
            ..fixtures::keychron()
        };
        assert_eq!(null_container.known_container_id(), None);
        assert_eq!(
            structural_reset_bans(&null_container),
            vec![ResetBan::UnknownContainer]
        );
    }

    #[test]
    fn removable_flag_is_not_consulted() {
        // The Keychron collection has no DN_REMOVABLE (0x4000); the container decides.
        let kb = fixtures::keychron();
        assert_eq!(kb.dev_node_status.map(|s| s & 0x4000), Some(0));
        assert!(can_live_reset(&kb, false));
    }

    #[test]
    fn unproven_and_unknown_transports_are_banned() {
        // G2: only USB is verified. BLE and BT Classic may still reconnect.
        let ble = fixtures::vxe_ble();
        let bans = live_reset_bans(&ble, false);
        assert_eq!(
            bans,
            vec![ResetBan::UnprovenTransport {
                transport: Transport::BluetoothLe
            }]
        );
        assert!(bans.iter().all(ResetBan::allows_reconnect));
        assert!(!can_live_reset(&ble, false));
        let bt = KeyboardDevice {
            transport: Transport::BluetoothClassic,
            ..fixtures::vxe_ble()
        };
        assert!(!can_live_reset(&bt, false));

        // An unknown bus (e.g. SAM, or ACPI without an I2C hint) may be I2C.
        let unknown = KeyboardDevice {
            instance_id: r"HID\SAM&Col01\1".into(),
            parent_chain: vec![r"SAM\KBD\1".into()],
            transport: Transport::Unknown,
            ..fixtures::keychron()
        };
        assert_eq!(
            structural_reset_bans(&unknown),
            vec![ResetBan::UnknownTransport]
        );
        assert!(!ResetBan::UnknownTransport.allows_reconnect());
    }

    #[test]
    fn state_bans() {
        let phantom = fixtures::ms_ble_phantom();
        let bans = live_reset_bans(&phantom, false);
        assert!(bans.contains(&ResetBan::NotPresent));
        assert!(bans.contains(&ResetBan::StatusUnknown));

        let stopped = KeyboardDevice {
            dev_node_status: Some(0x0180_0002),
            ..fixtures::keychron()
        };
        assert_eq!(live_reset_bans(&stopped, false), vec![ResetBan::NotStarted]);

        let problem = KeyboardDevice {
            dev_node_status: Some(0x0180_040A),
            problem_code: Some(10),
            ..fixtures::keychron()
        };
        assert_eq!(
            live_reset_bans(&problem, false),
            vec![ResetBan::HasProblem { code: 10 }]
        );
    }

    #[test]
    fn inv_ps2_holds_on_dev_machine() {
        let snapshot = fixtures::dev_machine();
        assert_eq!(check_inv_ps2(&snapshot.global, &snapshot.keyboards), Ok(()));
    }

    #[test]
    fn inv_ps2_violation_without_device_values() {
        let ps2 = KeyboardDevice {
            overrides: DeviceOverrides::default(),
            ..fixtures::internal_ps2()
        };
        let err = check_inv_ps2(
            &fixtures::global_per_keyboard(),
            &[ps2.clone(), fixtures::keychron()],
        )
        .unwrap_err();
        assert_eq!(
            err.keyboards,
            vec![r"ACPI\FUJ0309\4&320DB4C2&0".to_string()]
        );
        assert!(err.to_string().contains("INV-PS2"));

        // Fixed mode keeps the PS/2 keyboard on the global values.
        assert_eq!(
            check_inv_ps2(&fixtures::global_fixed_jis(), std::slice::from_ref(&ps2)),
            Ok(())
        );
    }

    #[test]
    fn inv_ps2_covers_phantoms_and_partial_values() {
        let phantom = KeyboardDevice {
            instance_id: r"ACPI\PNP0303\4&1&0".into(),
            present: false,
            reported_type: None,
            overrides: DeviceOverrides::default(),
            ..fixtures::internal_ps2()
        };
        let partial = KeyboardDevice {
            overrides: DeviceOverrides {
                override_keyboard_type: Some(7),
                ..Default::default()
            },
            ..fixtures::internal_ps2()
        };
        let err = check_inv_ps2(
            &fixtures::global_per_keyboard(),
            &[fixtures::internal_ps2(), phantom, partial],
        )
        .unwrap_err();
        assert_eq!(err.keyboards.len(), 2);

        // A lone global value is not a pair.
        let lone = GlobalSettings {
            override_keyboard_type: Some(7),
            ..fixtures::global_per_keyboard()
        };
        let bare = KeyboardDevice {
            overrides: DeviceOverrides::default(),
            ..fixtures::internal_ps2()
        };
        assert!(check_inv_ps2(&lone, &[bare]).is_err());
    }

    #[test]
    fn inv_ps2_ignores_hid_keyboards() {
        let hid_names_on_hid = fixtures::keychron();
        assert_eq!(
            check_inv_ps2(&fixtures::global_per_keyboard(), &[hid_names_on_hid]),
            Ok(())
        );
        assert_eq!(check_inv_ps2(&fixtures::global_per_keyboard(), &[]), Ok(()));
    }
}
