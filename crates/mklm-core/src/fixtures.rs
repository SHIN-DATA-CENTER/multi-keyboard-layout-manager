//! Test fixtures reproducing the development machine after M0 (see `docs/research/m0-results.md`).

use crate::model::*;

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Built-in Fujitsu PS/2 keyboard, pinned to 7/2 by the M0 migration.
pub(crate) fn internal_ps2() -> KeyboardDevice {
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
pub(crate) fn keychron() -> KeyboardDevice {
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
pub(crate) fn vxe_ble() -> KeyboardDevice {
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
pub(crate) fn ms_ble_phantom() -> KeyboardDevice {
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

/// Global values after the M0 migration (per-keyboard mode, JIS standard).
pub(crate) fn global_per_keyboard() -> GlobalSettings {
    GlobalSettings {
        layer_driver_jpn: Some("kbd106.dll".into()),
        layer_driver_kor: Some("kbd101a.dll".into()),
        override_keyboard_identifier: Some("PCAT_106KEY".into()),
        override_keyboard_type: None,
        override_keyboard_subtype: None,
    }
}

/// Global values before the M0 migration (fixed JIS, what the Settings app writes for "Japanese").
pub(crate) fn global_fixed_jis() -> GlobalSettings {
    GlobalSettings {
        override_keyboard_type: Some(7),
        override_keyboard_subtype: Some(2),
        ..global_per_keyboard()
    }
}

pub(crate) fn input_methods() -> InputMethods {
    InputMethods {
        user_preload: ids(&["00000411", "00000409"]),
        // M0 recorded only 00000411; the live machine now also lists 00000409.
        sign_in_preload: ids(&["00000411", "00000409"]),
        loaded_layouts: vec![0x0411_0411, 0x0409_0409],
    }
}

/// The development machine as verified at the end of M0.
pub(crate) fn dev_machine() -> SystemSnapshot {
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
        for kb in dev_machine().keyboards {
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
