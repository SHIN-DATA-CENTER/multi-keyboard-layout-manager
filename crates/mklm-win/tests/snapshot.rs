//! Snapshot tests against the live system (read-only).
//!
//! The always-on tests assume nothing about the attached devices (CI runners may have no keyboard).
//! The `#[ignore]` tests encode the M0 facts of the development machine
//! (docs/research/m0-results.md); run them there with `cargo test -p mklm-win -- --include-ignored`.

#![cfg(windows)]

use mklm_core::{
    GlobalMode, INTERNAL_CONTAINER_ID, KeyboardDevice, KeyboardDriver, KeyboardType, LayoutTable,
    SystemSnapshot, Transport, assess, check_inv_ps2, parse_klid,
};
use mklm_win::{SnapshotOptions, raw_keyboards, read_keyboard, snapshot, snapshot_report};

fn all_keyboards() -> SystemSnapshot {
    snapshot(SnapshotOptions {
        include_non_present: true,
    })
    .expect("snapshot with non-present keyboards")
}

fn is_braced_upper_guid(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 38
        && bytes[0] == b'{'
        && bytes[37] == b'}'
        && s[1..37].char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_digit() || matches!(c, 'A'..='F')
            }
        })
}

fn check_keyboard(kb: &KeyboardDevice) {
    let id = &kb.instance_id;
    assert!(id.contains('\\'), "{id}: not an instance ID");
    assert!(
        !kb.display_name.trim().is_empty(),
        "{id}: empty display name"
    );
    if let Some(container) = &kb.container_id {
        assert!(
            is_braced_upper_guid(container),
            "{id}: container {container}"
        );
    }
    assert_eq!(
        kb.is_internal,
        kb.container_id.as_deref() == Some(INTERNAL_CONTAINER_ID),
        "{id}: is_internal"
    );
    assert!(
        kb.parent_chain
            .iter()
            .all(|parent| !parent.eq_ignore_ascii_case(id) && parent.contains('\\')),
        "{id}: bad parent chain {:?}",
        kb.parent_chain
    );
    for (i, a) in kb.parent_chain.iter().enumerate() {
        assert!(
            kb.parent_chain[i + 1..]
                .iter()
                .all(|b| !a.eq_ignore_ascii_case(b)),
            "{id}: parent chain repeats {a}"
        );
    }
    // Every Remote Desktop keyboard is virtual (read-only, plan 3.2), whatever serves it. The
    // `--all` listing checks this on any host that has served Remote Desktop: the keyboard of a
    // disconnected session stays as a non-present `…SESSIONnKEYBOARD0` devnode.
    if kb.is_remote_desktop() {
        assert_eq!(kb.transport, Transport::Virtual, "{id}");
    } else if kb.driver == KeyboardDriver::I8042prt {
        assert_eq!(kb.transport, Transport::Ps2, "{id}");
    }
    if !kb.present {
        assert_eq!(
            kb.reported_type, None,
            "{id}: non-present devices report nothing"
        );
    }
    assert_eq!(kb.vendor_id.is_some(), kb.product_id.is_some(), "{id}");
    if kb.usb_serial.is_some() {
        assert_eq!(kb.transport, Transport::Usb, "{id}");
    }
    assert!(kb.hardware_ids.iter().all(|hwid| !hwid.is_empty()), "{id}");
}

#[test]
fn snapshot_is_well_formed() {
    let report = snapshot_report(SnapshotOptions::default()).expect("snapshot");
    for issue in &report.issues {
        eprintln!("issue: {issue}");
    }
    let snap = &report.snapshot;

    for kb in &snap.keyboards {
        assert!(
            kb.present,
            "{}: default options list present keyboards only",
            kb.instance_id
        );
        check_keyboard(kb);
    }
    let ids: Vec<String> = snap
        .keyboards
        .iter()
        .map(|kb| kb.instance_id.to_ascii_uppercase())
        .collect();
    assert!(
        ids.windows(2).all(|w| w[0] < w[1]),
        "sorted and unique: {ids:?}"
    );

    for klid in snap
        .input
        .user_preload
        .iter()
        .chain(&snap.input.sign_in_preload)
    {
        assert!(
            parse_klid(klid).is_some(),
            "Preload entry {klid:?} is not a KLID"
        );
    }
    assert!(snap.os.build > 0);
    assert!(!snap.os.native_arch.is_empty());
    // What a Remote Desktop client reported is read only in a remote session.
    if !snap.os.remote_session {
        assert_eq!(snap.os.client_keyboard_type, None);
    }

    // The core evaluation must accept whatever the live system looks like.
    let assessment = assess(snap);
    assert_eq!(assessment.keyboards.len(), snap.keyboards.len());
}

#[test]
fn non_present_listing_is_a_superset() {
    let present = snapshot(SnapshotOptions::default()).expect("snapshot");
    let all = all_keyboards();
    for kb in &all.keyboards {
        check_keyboard(kb);
    }
    for kb in &present.keyboards {
        assert!(
            all.keyboards
                .iter()
                .any(|other| other.instance_id.eq_ignore_ascii_case(&kb.instance_id)),
            "{} missing from the full listing",
            kb.instance_id
        );
    }
    assert_eq!(present.global, all.global);
}

#[test]
fn raw_input_paths_are_normalised() {
    let mut issues = Vec::new();
    for raw in raw_keyboards(&mut issues).expect("Raw Input device list") {
        assert!(
            raw.interface_path.starts_with(r"\\?\"),
            "{}",
            raw.interface_path
        );
        if let Some(id) = &raw.instance_id {
            assert!(id.contains('\\'), "{id}");
        }
    }
    for issue in &issues {
        eprintln!("issue: {issue}");
    }
}

#[test]
fn invalid_instance_ids_are_rejected() {
    // An empty ID would locate the root devnode and a NUL would truncate the ID.
    for id in [
        "",
        "  ",
        "\0ACPI\\FUJ0309\\4&320DB4C2&0",
        "ACPI\\FUJ0309\0x",
    ] {
        let mut issues = Vec::new();
        assert_eq!(read_keyboard(id, &[], &mut issues), None, "{id:?}");
        assert_eq!(issues.len(), 1, "{id:?}: {issues:?}");
    }
    // A devnode that does not exist is simply absent.
    let mut issues = Vec::new();
    assert_eq!(
        read_keyboard(r"HID\MKLM_DOES_NOT_EXIST\0", &[], &mut issues),
        None
    );
    assert_eq!(issues, []);
}

// ---- Development machine (M0 facts) -------------------------------------------------------------

const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
const FUJ0309: &str = r"ACPI\FUJ0309\4&320DB4C2&0";
const VXE_COL02: &str = r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F&COL02\B&B4852A&0&0001";

fn find<'a>(snap: &'a SystemSnapshot, instance_id: &str) -> &'a KeyboardDevice {
    snap.keyboards
        .iter()
        .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
        .unwrap_or_else(|| panic!("{instance_id} not found"))
}

#[test]
#[ignore = "development machine only (Keychron receiver plugged in)"]
fn dev_keychron_types_us_over_usb() {
    let snap = all_keyboards();
    let kb = find(&snap, KEYCHRON);
    assert_eq!(kb.instance_id, KEYCHRON, "canonical upper-case instance ID");
    assert!(kb.present);
    assert_eq!(kb.driver, KeyboardDriver::Kbdhid);
    assert_eq!(kb.transport, Transport::Usb);
    assert_eq!((kb.vendor_id, kb.product_id), (Some(0x3434), Some(0xD027)));
    assert_eq!(kb.usb_serial.as_deref(), Some("B76E483E3F08D96E"));
    assert_eq!(
        kb.container_id.as_deref(),
        Some("{F0D991EA-A583-5B9C-800D-48846AC6E633}")
    );
    assert!(!kb.is_internal);
    assert_eq!(kb.display_name, "Keychron Receiver");
    // Canonical instance IDs, the same spelling as `instance_id` and `Get-PnpDevice`.
    assert_eq!(
        kb.parent_chain,
        [
            r"USB\VID_3434&PID_D027&MI_00\7&295E03CA&0&0000",
            r"USB\VID_3434&PID_D027\B76E483E3F08D96E"
        ]
    );
    assert_eq!(kb.overrides.keyboard_type_override, Some(4));
    assert_eq!(kb.overrides.keyboard_subtype_override, Some(0));
    assert_eq!(kb.overrides.override_keyboard_type, None);
    assert_eq!(kb.reported_type, Some(KeyboardType::US));

    let assessment = assess(&snap);
    let kb = assessment
        .keyboards
        .iter()
        .find(|k| k.instance_id == KEYCHRON)
        .unwrap();
    assert_eq!(
        kb.current.as_ref().map(|l| &l.table),
        Some(&LayoutTable::Us)
    );
}

#[test]
#[ignore = "development machine only (Fujitsu internal PS/2 keyboard)"]
fn dev_internal_ps2_types_jis() {
    let snap = all_keyboards();
    let kb = find(&snap, FUJ0309);
    assert_eq!(kb.instance_id, FUJ0309);
    assert!(kb.present);
    assert_eq!(kb.driver, KeyboardDriver::I8042prt);
    assert_eq!(kb.transport, Transport::Ps2);
    assert!(kb.is_internal);
    assert_eq!(kb.container_id.as_deref(), Some(INTERNAL_CONTAINER_ID));
    assert_eq!(kb.overrides.override_keyboard_type, Some(7));
    assert_eq!(kb.overrides.override_keyboard_subtype, Some(2));
    assert_eq!(kb.overrides.keyboard_type_override, None);
    assert_eq!(kb.reported_type, Some(KeyboardType::JIS));
    assert_eq!(kb.usb_serial, None);
}

#[test]
#[ignore = "development machine only (VXE R1SE+ mouse paired over BLE, connected or not)"]
fn dev_ble_mouse_keyboard_collection() {
    // The static properties hold whether the mouse is connected or not; what Raw Input reports
    // depends on it.
    let snap = all_keyboards();
    let kb = find(&snap, VXE_COL02);
    assert_eq!(kb.driver, KeyboardDriver::Kbdhid);
    assert_eq!(kb.transport, Transport::BluetoothLe);
    assert_eq!(kb.display_name, "VXE R1SE+");
    assert_eq!((kb.vendor_id, kb.product_id), (Some(0x25A7), Some(0xFA6C)));
    assert!(!kb.is_internal);
    assert!(kb.overrides.is_empty());
    if kb.present {
        assert_eq!(kb.reported_type, Some(KeyboardType::HID_UNKNOWN));
    } else {
        assert_eq!(kb.reported_type, None);
    }
}

#[test]
#[ignore = "development machine only (BLE mouse \"X3-5.4 Mouse\", connected or not)"]
fn dev_phantom_ble_mouse_keyboard_collection() {
    // A BLE mouse that reports a Microsoft PnP ID (045E/0040), not a BLE keyboard. It was not
    // connected in M0 (a phantom); whether it is now only decides which lists show it.
    let snap = all_keyboards();
    let mouse = snap
        .keyboards
        .iter()
        .find(|kb| kb.vendor_id == Some(0x045E) && kb.product_id == Some(0x0040))
        .expect("VID 045E PID 0040 keyboard collection");
    assert_eq!(mouse.display_name, "X3-5.4 Mouse");
    assert_eq!(mouse.transport, Transport::BluetoothLe);
    assert_eq!(mouse.driver, KeyboardDriver::Kbdhid);
    if !mouse.present {
        assert_eq!(mouse.reported_type, None);
    }

    // The list without non-present keyboards shows it exactly when it is connected.
    let present = snapshot(SnapshotOptions::default()).unwrap();
    let listed = present
        .keyboards
        .iter()
        .any(|kb| kb.instance_id.eq_ignore_ascii_case(&mouse.instance_id));
    assert_eq!(listed, mouse.present);
}

#[test]
#[ignore = "development machine only (migrated to per-keyboard mode in M0 #4)"]
fn dev_global_is_per_keyboard() {
    let snap = all_keyboards();
    let global = &snap.global;
    assert_eq!(global.layer_driver_jpn.as_deref(), Some("kbd106.dll"));
    assert_eq!(global.layer_driver_kor.as_deref(), Some("kbd101a.dll"));
    assert_eq!(
        global.override_keyboard_identifier.as_deref(),
        Some("PCAT_106KEY")
    );
    assert_eq!(global.override_keyboard_type, None);
    assert_eq!(global.override_keyboard_subtype, None);
    assert_eq!(global.mode(), GlobalMode::PerKeyboard);
    // Every i8042prt keyboard, phantoms included, carries explicit values.
    assert!(check_inv_ps2(global, &snap.keyboards).is_ok());
}

#[test]
#[ignore = "development machine only (Japanese + English input methods)"]
fn dev_input_methods() {
    let snap = all_keyboards();
    assert_eq!(snap.input.user_preload, ["00000411", "00000409"]);
    // M0 recorded only 00000411 for the sign-in screen; English may have been added since.
    assert_eq!(
        snap.input.sign_in_preload.first().map(String::as_str),
        Some("00000411")
    );
    assert!(snap.input.loaded_layouts.contains(&0x0411_0411));
    assert!(snap.input.loaded_layouts.contains(&0x0409_0409));
    assert_eq!(snap.os.native_arch, "x64");
    assert!(snap.os.build >= 26200);
}
