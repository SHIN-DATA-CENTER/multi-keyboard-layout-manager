//! Test fixtures reproducing the development machine after M0 (see `docs/research/m0-results.md`),
//! the schema-1 journal it holds after the M2 real-machine tests (`testdata/journal/schema-1`),
//! and the journal of the boot-ID bug that 0.1.0 left on a desktop PC
//! (`testdata/journal/legacy-guid`, docs/research/boot-id.md).
//!
//! Compiled for this crate's tests and, with the `test-fixtures` feature, for other crates' tests
//! (`mklm-engine` drives its fakes with them). Never enable the feature in a shipped binary.

use crate::boot::CurrentBoot;
use crate::journal::BootId;
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

/// The loader boot GUID that 0.1.0 recorded on the desktop PC of the boot-ID bug (2026-09-29):
/// the same value in every boot from 2026-09-28 13:29 to at least 11:37 the next day, full
/// restarts included (docs/research/boot-id.md).
pub const LEGACY_PC_GUID: BootId = BootId(0x9845_bda6_baa7_11f1_adca_ca98_8d51_3a4f);
/// `KUSER_SHARED_DATA.BootId` of that PC after its 11:37:07 restart.
pub const LEGACY_PC_BOOT_COUNTER: u32 = 7;
/// `BootTime - BootTimeBias` of the 11:37:07 boot (the one after both entries were written).
pub const LEGACY_PC_BOOT_TIME_AFTER_RESTART: u64 = 134_351_230_275_000_000;
/// `BootTime - BootTimeBias` of the 11:34:20 boot, in which both entries were last written.
pub const LEGACY_PC_BOOT_TIME_OF_WRITES: u64 = 134_351_228_605_000_000;
/// `BootTime - BootTimeBias` of the 2026-09-28 13:29 boot, in which the first entry was created.
pub const LEGACY_PC_BOOT_TIME_OF_FIRST_WRITE: u64 = 134_350_433_725_000_000;
/// The migration to JIS that was reverted before any restart (`reverted-pending-reboot`).
pub const LEGACY_REVERTED_OP: &str = "c10d2d38-ec81-4c57-a7d6-9592199945ff";
/// The migration to US that waits for the restart (`pending-reboot`, `apply_pending` restart).
pub const LEGACY_PENDING_OP: &str = "d724c149-9bee-4348-9481-a544f9980f4b";

/// The two operations and eight baselines of the boot-ID bug, exactly as 0.1.0 stored them on
/// the desktop PC (`HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal`, schema 1, copied value by value
/// on 2026-09-29 after the 11:37 restart): `c10d2d38` ([`LEGACY_REVERTED_OP`]) and `d724c149`
/// ([`LEGACY_PENDING_OP`]), both with the boot ID [`LEGACY_PC_GUID`] and history lines that
/// carry `boot_time_hint`. `(value name, JSON)` of `Ops` and of `Baselines`.
pub fn legacy_guid_journal() -> (StoredDocuments, StoredDocuments) {
    let ops = [
        (
            LEGACY_REVERTED_OP,
            include_str!(
                "../testdata/journal/legacy-guid/c10d2d38-ec81-4c57-a7d6-9592199945ff.json"
            ),
        ),
        (
            LEGACY_PENDING_OP,
            include_str!(
                "../testdata/journal/legacy-guid/d724c149-9bee-4348-9481-a544f9980f4b.json"
            ),
        ),
    ]
    .into_iter()
    .map(|(name, json)| (name.to_string(), json.trim().to_string()))
    .collect();
    let baselines: Vec<(String, serde_json::Value)> = serde_json::from_str(include_str!(
        "../testdata/journal/legacy-guid/baselines.json"
    ))
    .unwrap_or_default();
    let baselines = baselines
        .into_iter()
        .map(|(name, json)| (name, json.to_string()))
        .collect();
    (ops, baselines)
}

/// The current boot of that PC as the fixed build reads it, with `boot_time` as
/// `BootTime - BootTimeBias` ([`LEGACY_PC_BOOT_TIME_AFTER_RESTART`] after the restart,
/// [`LEGACY_PC_BOOT_TIME_OF_WRITES`] as if the fix had been installed before it).
pub fn legacy_pc_boot(boot_time: u64) -> CurrentBoot {
    CurrentBoot {
        id: BootId::from_boot_counter(LEGACY_PC_BOOT_COUNTER),
        boot_time: Some(boot_time),
        legacy_guid: Some(LEGACY_PC_GUID),
    }
}

/// The Remote Desktop host of 2026-09-29 (`DESKTOP-3TCSIET`, design standard-layout 0.1).
pub const DESKTOP_PC_NAME: &str = "DESKTOP-3TCSIET";

/// The PS/2 node of the desktop PC: no hardware behind it (not started, problem code 24), pinned
/// to US (4/0) by the migration `271b6909`.
pub fn desktop_ps2() -> KeyboardDevice {
    KeyboardDevice {
        instance_id: r"ACPI\PNP0303\0".into(),
        display_name: "標準 PS/2 キーボード".into(),
        device_description: Some("標準 PS/2 キーボード".into()),
        container_id: Some(INTERNAL_CONTAINER_ID.into()),
        is_internal: true,
        present: true,
        driver: KeyboardDriver::I8042prt,
        transport: Transport::Ps2,
        vendor_id: None,
        product_id: None,
        usb_serial: None,
        hardware_ids: ids(&[r"ACPI\VEN_PNP&DEV_0303", r"ACPI\PNP0303", "*PNP0303"]),
        parent_chain: ids(&[
            r"PCI\VEN_8086&DEV_7A84&SUBSYS_31181565&REV_11\3&11583659&0&F8",
            r"ACPI\PNP0A08\0",
            r"ACPI_HAL\PNP0C08\0",
            r"ROOT\ACPI_HAL\0000",
            r"HTREE\ROOT\0",
        ]),
        overrides: DeviceOverrides {
            override_keyboard_type: Some(4),
            override_keyboard_subtype: Some(0),
            ..Default::default()
        },
        reported_type: None,
        dev_node_status: Some(0x4180_2400),
        problem_code: Some(24),
    }
}

/// A USB HID keyboard collection of the desktop PC, as `mklm-cli status --json --all` read it in
/// the RDP-born session 2 (Raw Input of that session lists none of the PC's keyboards, so no type
/// is reported).
fn desktop_hid(
    instance_id: &str,
    display_name: &str,
    container_id: &str,
    vid_pid: (u16, u16),
    hardware_ids: &[&str],
    parent_chain: &[&str],
    overrides: DeviceOverrides,
) -> KeyboardDevice {
    KeyboardDevice {
        instance_id: instance_id.into(),
        display_name: display_name.into(),
        device_description: Some("HID キーボード デバイス".into()),
        container_id: Some(container_id.into()),
        is_internal: false,
        present: true,
        driver: KeyboardDriver::Kbdhid,
        transport: Transport::Usb,
        vendor_id: Some(vid_pid.0),
        product_id: Some(vid_pid.1),
        usb_serial: None,
        hardware_ids: ids(hardware_ids),
        parent_chain: ids(parent_chain),
        overrides,
        reported_type: None,
        dev_node_status: Some(0x0180_000A),
        problem_code: Some(0),
    }
}

fn us_pair() -> DeviceOverrides {
    DeviceOverrides {
        keyboard_type_override: Some(4),
        keyboard_subtype_override: Some(0),
        ..Default::default()
    }
}

/// "USB Keyboard" (04D9:1818), assigned US.
pub fn desktop_usb_keyboard() -> KeyboardDevice {
    desktop_hid(
        r"HID\VID_04D9&PID_1818&MI_00\7&183DDD3D&0&0000",
        "USB Keyboard",
        "{15A651F8-BAA8-11F1-B5A1-806E6F6E6963}",
        (0x04D9, 0x1818),
        &[
            r"HID\VID_04D9&PID_1818&REV_0101&MI_00",
            r"HID\VID_04D9&PID_1818&MI_00",
            r"HID\VID_04D9&UP:0001_U:0006",
            "HID_DEVICE_SYSTEM_KEYBOARD",
            "HID_DEVICE_UP:0001_U:0006",
            "HID_DEVICE",
        ],
        &[
            r"USB\VID_04D9&PID_1818&MI_00\6&246ECAD8&0&0000",
            r"USB\VID_04D9&PID_1818\5&361281AE&0&6",
        ],
        us_pair(),
    )
}

/// The first keyboard collection (`MI_00`) of the "2.4G Wireless Device" (1D57:FA60); no values,
/// so it follows the standard.
pub fn desktop_wireless() -> KeyboardDevice {
    desktop_hid(
        r"HID\VID_1D57&PID_FA60&MI_00\7&14A99BDA&0&0000",
        "2.4G Wireless Device",
        "{15A651F5-BAA8-11F1-B5A1-806E6F6E6963}",
        (0x1D57, 0xFA60),
        &[
            r"HID\VID_1D57&PID_FA60&REV_0114&MI_00",
            r"HID\VID_1D57&PID_FA60&MI_00",
            r"HID\VID_1D57&UP:0001_U:0006",
            "HID_DEVICE_SYSTEM_KEYBOARD",
            "HID_DEVICE_UP:0001_U:0006",
            "HID_DEVICE",
        ],
        &[
            r"USB\VID_1D57&PID_FA60&MI_00\6&A08016F&0&0000",
            r"USB\VID_1D57&PID_FA60\5&361281AE&0&5",
        ],
        DeviceOverrides::default(),
    )
}

/// The second keyboard collection (`MI_03`) of the same receiver: same container, no values.
pub fn desktop_wireless_second() -> KeyboardDevice {
    desktop_hid(
        r"HID\VID_1D57&PID_FA60&MI_03\7&3A907B5E&0&0000",
        "2.4G Wireless Device",
        "{15A651F5-BAA8-11F1-B5A1-806E6F6E6963}",
        (0x1D57, 0xFA60),
        &[
            r"HID\VID_1D57&PID_FA60&REV_0114&MI_03",
            r"HID\VID_1D57&PID_FA60&MI_03",
            r"HID\VID_1D57&UP:0001_U:0006",
            "HID_DEVICE_SYSTEM_KEYBOARD",
            "HID_DEVICE_UP:0001_U:0006",
            "HID_DEVICE",
        ],
        &[
            r"USB\VID_1D57&PID_FA60&MI_03\6&A08016F&0&0003",
            r"USB\VID_1D57&PID_FA60\5&361281AE&0&5",
        ],
        DeviceOverrides::default(),
    )
}

/// The Keychron receiver on the desktop PC (another USB port than on the M0 machine, hence
/// another instance ID), assigned US.
pub fn desktop_keychron() -> KeyboardDevice {
    KeyboardDevice {
        usb_serial: Some("B76E483E3F08D96E".into()),
        ..desktop_hid(
            r"HID\VID_3434&PID_D027&MI_00&COL01\7&5211D3A&0&0000",
            "Keychron Receiver",
            "{F0D991EA-A583-5B9C-800D-48846AC6E633}",
            (0x3434, 0xD027),
            &[
                r"HID\VID_3434&PID_D027&REV_0116&MI_00&Col01",
                r"HID\VID_3434&PID_D027&MI_00&Col01",
                r"HID\VID_3434&UP:0001_U:0006",
                "HID_DEVICE_SYSTEM_KEYBOARD",
                "HID_DEVICE_UP:0001_U:0006",
                "HID_DEVICE",
            ],
            &[
                r"USB\VID_3434&PID_D027&MI_00\6&295E03CA&0&0000",
                r"USB\VID_3434&PID_D027\B76E483E3F08D96E",
            ],
            us_pair(),
        )
    }
}

/// "VXE Mouse 1K Dongle" (3554:F58E): the keyboard collection of a mouse dongle, no values.
pub fn desktop_vxe() -> KeyboardDevice {
    desktop_hid(
        r"HID\VID_3554&PID_F58E&MI_00\8&18DB8B6B&0&0000",
        "VXE Mouse 1K Dongle",
        "{15A651FA-BAA8-11F1-B5A1-806E6F6E6963}",
        (0x3554, 0xF58E),
        &[
            r"HID\VID_3554&PID_F58E&REV_0110&MI_00",
            r"HID\VID_3554&PID_F58E&MI_00",
            r"HID\VID_3554&UP:0001_U:0006",
            "HID_DEVICE_SYSTEM_KEYBOARD",
            "HID_DEVICE_UP:0001_U:0006",
            "HID_DEVICE",
        ],
        &[
            r"USB\VID_3554&PID_F58E&MI_00\7&25CF5594&0&0000",
            r"USB\VID_3554&PID_F58E\6&2287062C&0&3",
        ],
        DeviceOverrides::default(),
    )
}

/// The Remote Desktop keyboard of the desktop PC's session 2 (RDP-born, 23:14).
pub fn desktop_rdp_keyboard() -> KeyboardDevice {
    KeyboardDevice {
        instance_id: r"TERMINPUT_BUS\UMB\2&2C22BCC9&0&SESSION2KEYBOARD0".into(),
        ..rdp_keyboard()
    }
}

/// The desktop PC of the RDP work (design standard-layout 0.1), as `mklm-cli status --json --all`
/// read it in its RDP-born session 2 on 2026-09-30 after `271b6909` was kept: per-keyboard mode,
/// JIS standard; the PS/2 node, USB Keyboard and Keychron Receiver at US (4/0); the two
/// collections of the 2.4G Wireless Device and the VXE dongle follow the standard; the Remote
/// Desktop keyboard of session 2. No keyboard is disconnected. Raw Input of that session lists
/// none of the PC's keyboards, so every `reported_type` is `None`.
pub fn desktop_pc() -> SystemSnapshot {
    SystemSnapshot {
        keyboards: vec![
            desktop_ps2(),
            desktop_usb_keyboard(),
            desktop_wireless(),
            desktop_wireless_second(),
            desktop_keychron(),
            desktop_vxe(),
            desktop_rdp_keyboard(),
        ],
        global: global_per_keyboard(),
        input: InputMethods {
            user_preload: ids(&["00000411"]),
            sign_in_preload: ids(&["00000411"]),
            loaded_layouts: vec![0x0411_0411],
        },
        os: OsInfo {
            build: 26200,
            ubr: Some(9457),
            native_arch: "x64".into(),
            remote_session: true,
            client_keyboard_type: Some(KeyboardType::JIS),
            computer_name: Some(DESKTOP_PC_NAME.into()),
        },
    }
}

/// [`desktop_pc`] seen from a console session: the same values, and Raw Input reports the stored
/// types of the connected, started HID keyboards (the PS/2 node has no hardware).
pub fn desktop_pc_console() -> SystemSnapshot {
    let mut snapshot = desktop_pc();
    snapshot.keyboards.retain(|kb| !kb.is_remote_desktop());
    for kb in &mut snapshot.keyboards {
        if kb.driver == KeyboardDriver::Kbdhid {
            kb.reported_type = kb.predicted_type(&snapshot.global);
        }
    }
    snapshot.os.remote_session = false;
    snapshot.os.client_keyboard_type = None;
    snapshot
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
            client_keyboard_type: None,
            computer_name: None,
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
        for kb in dev_machine()
            .keyboards
            .into_iter()
            .chain([rdp_keyboard()])
            .chain(desktop_pc().keyboards)
        {
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
