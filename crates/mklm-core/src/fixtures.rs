//! Test fixtures reproducing the development machine after M0 (see `docs/research/m0-results.md`),
//! and the schema-1 journal it holds after the M2 real-machine tests (`testdata/journal/`).
//!
//! Compiled for this crate's tests and, with the `test-fixtures` feature, for other crates' tests
//! (`mklm-engine` drives its fakes with them). Never enable the feature in a shipped binary.

use crate::model::*;

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Built-in Fujitsu PS/2 keyboard, pinned to 7/2 by the M0 migration.
pub fn internal_ps2() -> KeyboardDevice {
    KeyboardDevice {
        instance_id: r"ACPI\FUJ0309\4&320DB4C2&0".into(),
        display_name: "日本語 PS/2 キーボード (106/109 キー Ctrl+英数)".into(),
        device_description: Some("日本語 PS/2 キーボード (106/109 キー Ctrl+英数)".into()),
        container_id: Some(INTERNAL_CONTAINER_ID.into()),
        is_internal: true,
        present: true,
        driver: KeyboardDriver::I8042prt,
        transport: Transport::Ps2,
        vendor_id: None,
        product_id: None,
        usb_serial: None,
        hardware_ids: ids(&[r"ACPI\VEN_FUJ&DEV_0309", r"ACPI\FUJ0309", "*FUJ0309"]),
        parent_chain: ids(&[
            r"PCI\VEN_1022&DEV_790E&SUBSYS_004E1E26&REV_51\3&2411E6FE&0&A3",
            r"ACPI\PNP0A08\1",
            r"ACPI_HAL\PNP0C08\0",
            r"ROOT\ACPI_HAL\0000",
            r"HTREE\ROOT\0",
        ]),
        overrides: DeviceOverrides {
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(2),
            ..Default::default()
        },
        reported_type: Some(KeyboardType::JIS),
        dev_node_status: Some(0x0180_000A),
        problem_code: Some(0),
    }
}

/// Keychron USB receiver, keyboard collection COL01, set to US (4/0).
pub fn keychron() -> KeyboardDevice {
    KeyboardDevice {
        instance_id: r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".into(),
        display_name: "Keychron Receiver".into(),
        device_description: Some("HID キーボード デバイス".into()),
        container_id: Some("{F0D991EA-A583-5B9C-800D-48846AC6E633}".into()),
        is_internal: false,
        present: true,
        driver: KeyboardDriver::Kbdhid,
        transport: Transport::Usb,
        vendor_id: Some(0x3434),
        product_id: Some(0xD027),
        usb_serial: Some("B76E483E3F08D96E".into()),
        hardware_ids: ids(&[
            r"HID\VID_3434&PID_D027&REV_0116&MI_00&Col01",
            r"HID\VID_3434&PID_D027&MI_00&Col01",
            r"HID\VID_3434&UP:0001_U:0006",
            "HID_DEVICE_SYSTEM_KEYBOARD",
            "HID_DEVICE_UP:0001_U:0006",
            "HID_DEVICE",
        ]),
        parent_chain: ids(&[
            r"USB\VID_3434&PID_D027&MI_00\7&295E03CA&0&0000",
            r"USB\VID_3434&PID_D027\B76E483E3F08D96E",
        ]),
        overrides: DeviceOverrides {
            keyboard_type_override: Some(4),
            keyboard_subtype_override: Some(0),
            ..Default::default()
        },
        reported_type: Some(KeyboardType::US),
        dev_node_status: Some(0x0180_000A),
        problem_code: Some(0),
    }
}

/// Keyboard collection of the "VXE R1SE+" BLE mouse; no overrides.
pub fn vxe_ble() -> KeyboardDevice {
    KeyboardDevice {
        instance_id: r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F&COL02\B&B4852A&0&0001".into(),
        display_name: "VXE R1SE+".into(),
        device_description: Some("HID キーボード デバイス".into()),
        container_id: Some("{BDA55856-0DCA-5B66-8949-3F281E7B4A28}".into()),
        is_internal: false,
        present: true,
        driver: KeyboardDriver::Kbdhid,
        transport: Transport::BluetoothLe,
        vendor_id: Some(0x25A7),
        product_id: Some(0xFA6C),
        usb_serial: None,
        hardware_ids: ids(&[
            r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&0225a7_PID&fa6c_REV&0300&Col02",
            r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&0225a7_PID&fa6c&Col02",
            r"HID\VID_25A7&UP:0001_U:0006",
            "HID_DEVICE_SYSTEM_KEYBOARD",
        ]),
        parent_chain: ids(&[
            r"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F\A&79F66C9&0&0018",
            r"BTHLE\DEV_F977E93BA63F\9&20C32106&0&F977E93BA63F",
        ]),
        overrides: DeviceOverrides::default(),
        reported_type: Some(KeyboardType::HID_UNKNOWN),
        dev_node_status: Some(0x0180_000A),
        problem_code: Some(0),
    }
}

/// Keyboard collection of the non-present BLE mouse "X3-5.4 Mouse", which reports a Microsoft
/// PnP ID (VID 045E, PID 0040). Not a keyboard, so it cannot stand in for a BLE keyboard test.
pub fn ms_ble_phantom() -> KeyboardDevice {
    KeyboardDevice {
        instance_id: r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&02045E_PID&0040_REV&0300_788712B99B19&COL01\B&1455DB79&0&0000".into(),
        display_name: "X3-5.4 Mouse".into(),
        device_description: Some("HID キーボード デバイス".into()),
        container_id: Some("{15781EFC-8BF6-50C5-9592-EB769DBAE3F4}".into()),
        is_internal: false,
        present: false,
        driver: KeyboardDriver::Kbdhid,
        transport: Transport::BluetoothLe,
        vendor_id: Some(0x045E),
        product_id: Some(0x0040),
        usb_serial: None,
        hardware_ids: ids(&[r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0040&Col01"]),
        parent_chain: ids(&[
            r"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&02045E_PID&0040_REV&0300_788712B99B19\A&6B58F46&0&002D",
            r"BTHLE\DEV_788712B99B19\9&20C32106&0&788712B99B19",
        ]),
        overrides: DeviceOverrides::default(),
        reported_type: None,
        dev_node_status: None,
        problem_code: None,
    }
}

/// The Remote Desktop keyboard of an RDP session, as read in session 1 of the RDP host
/// (DESKTOP-3TCSIET, 2026-09-29; `docs/research/rdp-keyboard.md`): one devnode per session, served
/// by terminpt, in the built-in container. Raw Input names no device for it, so no type is
/// reported. Not part of [`dev_machine`] (another PC, and its snapshots list four keyboards).
pub fn rdp_keyboard() -> KeyboardDevice {
    KeyboardDevice {
        instance_id: r"TERMINPUT_BUS\UMB\2&2C22BCC9&0&SESSION1KEYBOARD0".into(),
        display_name: "リモート デスクトップ キーボード デバイス".into(),
        device_description: Some("リモート デスクトップ キーボード デバイス".into()),
        container_id: Some(INTERNAL_CONTAINER_ID.into()),
        is_internal: true,
        present: true,
        driver: KeyboardDriver::Other("terminpt".into()),
        transport: Transport::Virtual,
        vendor_id: None,
        product_id: None,
        usb_serial: None,
        hardware_ids: ids(&[r"TS_INPT\TS_KBD"]),
        parent_chain: ids(&[
            r"UMB\UMB\1&841921D&0&TERMINPUT_BUS",
            r"ROOT\UMBUS\0000",
            r"HTREE\ROOT\0",
        ]),
        overrides: DeviceOverrides::default(),
        reported_type: None,
        dev_node_status: Some(0x0180_200A),
        problem_code: Some(0),
    }
}

/// Global values after the M0 migration (per-keyboard mode, JIS standard).
pub fn global_per_keyboard() -> GlobalSettings {
    GlobalSettings {
        layer_driver_jpn: Some("kbd106.dll".into()),
        layer_driver_kor: Some("kbd101a.dll".into()),
        override_keyboard_identifier: Some("PCAT_106KEY".into()),
        override_keyboard_type: None,
        override_keyboard_subtype: None,
    }
}

/// Global values before the M0 migration (fixed JIS, what the Settings app writes for "Japanese").
pub fn global_fixed_jis() -> GlobalSettings {
    GlobalSettings {
        override_keyboard_type: Some(7),
        override_keyboard_subtype: Some(2),
        ..global_per_keyboard()
    }
}

pub fn input_methods() -> InputMethods {
    InputMethods {
        user_preload: ids(&["00000411", "00000409"]),
        // M0 recorded only 00000411; the live machine now also lists 00000409.
        sign_in_preload: ids(&["00000411", "00000409"]),
        loaded_layouts: vec![0x0411_0411, 0x0409_0409],
    }
}

/// `(value name, JSON)` pairs of one journal sub-key, as `Journal::parse` takes them.
pub type StoredDocuments = Vec<(String, String)>;

/// The nine operations and the two baselines as the development machine stores them after the
/// M2 real-machine tests (`HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal`, schema 1, copied value
/// by value from the store), in `seq` order, all changes of the Keychron:
/// 1. set to JIS, the countdown ran out (`Reverted`, `CountdownExpired`);
/// 2. and 3. set to JIS, kept, then reverted by the user (`Reverted`);
/// 4. set to JIS, kept, reverted into a conflict and resolved by keeping the outside values
///    (R6, `Failed`, `ConflictKeptCurrent`);
/// 5. set to US and kept (`Confirmed`, the latest record of both values);
/// 6. set to JIS, kept, reverted (`Reverted`);
/// 7. the writer stopped before the keyboard reset; recovery put the values back (`Reverted`,
///    `LiveResetUnconfirmed`, history reasons `recover:roll-back`);
/// 8. and 9. the caller left during the countdown (`Reverted`, `CallerDisconnected`).
///
/// `(value name, JSON)` of `Ops` and of `Baselines`, exactly as the store holds them (design m3
/// WP-E1: journal schema 2 must read them).
pub fn schema_1_journal() -> (StoredDocuments, StoredDocuments) {
    let ops = [
        (
            "bfaca7cd-fdef-4dd0-8d75-6f311d32bc37",
            include_str!("../testdata/journal/schema-1/bfaca7cd-fdef-4dd0-8d75-6f311d32bc37.json"),
        ),
        (
            "1c2cc975-6a72-483e-8d18-f63485bc0739",
            include_str!("../testdata/journal/schema-1/1c2cc975-6a72-483e-8d18-f63485bc0739.json"),
        ),
        (
            "75c67e35-e637-4428-9fcc-dec2f44ce8bd",
            include_str!("../testdata/journal/schema-1/75c67e35-e637-4428-9fcc-dec2f44ce8bd.json"),
        ),
        (
            "371b1633-53a1-4819-8cd1-6f94f9e0cf25",
            include_str!("../testdata/journal/schema-1/371b1633-53a1-4819-8cd1-6f94f9e0cf25.json"),
        ),
        (
            "31f7f7bc-3939-403d-9290-ef1c17065d08",
            include_str!("../testdata/journal/schema-1/31f7f7bc-3939-403d-9290-ef1c17065d08.json"),
        ),
        (
            "bc7cc73f-7fbd-48a6-9ac2-37a2df583783",
            include_str!("../testdata/journal/schema-1/bc7cc73f-7fbd-48a6-9ac2-37a2df583783.json"),
        ),
        (
            "8e9a9970-f7bf-46c7-b779-f914f17bd40d",
            include_str!("../testdata/journal/schema-1/8e9a9970-f7bf-46c7-b779-f914f17bd40d.json"),
        ),
        (
            "1ef48b2f-8fca-4c9b-80f5-dfc0965c17a6",
            include_str!("../testdata/journal/schema-1/1ef48b2f-8fca-4c9b-80f5-dfc0965c17a6.json"),
        ),
        (
            "3fd933ac-d84e-423b-8820-832f8ce537ad",
            include_str!("../testdata/journal/schema-1/3fd933ac-d84e-423b-8820-832f8ce537ad.json"),
        ),
    ]
    .into_iter()
    .map(|(name, json)| (name.to_string(), json.trim().to_string()))
    .collect();
    let baselines: Vec<(String, serde_json::Value)> =
        serde_json::from_str(include_str!("../testdata/journal/schema-1/baselines.json"))
            .unwrap_or_default();
    let baselines = baselines
        .into_iter()
        .map(|(name, json)| (name, json.to_string()))
        .collect();
    (ops, baselines)
}

/// The development machine as verified at the end of M0.
pub fn dev_machine() -> SystemSnapshot {
    SystemSnapshot {
        keyboards: vec![internal_ps2(), keychron(), vxe_ble(), ms_ble_phantom()],
        global: global_per_keyboard(),
        input: input_methods(),
        os: OsInfo {
            build: 26200,
            ubr: None,
            native_arch: "x64".into(),
            remote_session: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{usb_serial_from_chain, vendor_product_from_ids};
    use crate::transport::classify_transport;

    /// The derived fields in the fixtures agree with the parsers.
    #[test]
    fn fixtures_are_self_consistent() {
        for kb in dev_machine().keyboards.into_iter().chain([rdp_keyboard()]) {
            assert_eq!(
                classify_transport(&kb.instance_id, &kb.parent_chain, &kb.driver),
                kb.transport,
                "{}",
                kb.instance_id
            );
            let vp = vendor_product_from_ids(&kb.instance_id, &kb.hardware_ids);
            assert_eq!(vp.map(|v| v.vendor_id), kb.vendor_id, "{}", kb.instance_id);
            assert_eq!(
                vp.map(|v| v.product_id),
                kb.product_id,
                "{}",
                kb.instance_id
            );
            assert_eq!(
                usb_serial_from_chain(&kb.parent_chain),
                kb.usb_serial,
                "{}",
                kb.instance_id
            );
        }
    }
}
