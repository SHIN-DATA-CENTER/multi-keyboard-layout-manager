//! Driver and transport classification from PnP IDs.

use crate::ids::split_instance_id;
use crate::model::{KeyboardDevice, KeyboardDriver, Transport};

/// GATT HID service (HOGP) UUID that Windows embeds in Bluetooth LE HID IDs.
const HOGP_SERVICE: &str = "{00001812-0000-1000-8000-00805F9B34FB}";
/// Bluetooth Classic HID profile UUID that Windows embeds in BTHENUM HID IDs.
const BT_HID_SERVICE: &str = "{00001124-0000-1000-8000-00805F9B34FB}";

/// Hardware ID of the Remote Desktop keyboard ("リモート デスクトップ キーボード デバイス", service
/// `terminpt`). It is a hardware ID, not an instance ID: the devnode itself is enumerated by
/// [`TERMINPUT_BUS`] (see `docs/research/rdp-keyboard.md`).
pub const RDP_KEYBOARD_HARDWARE_ID: &str = r"TS_INPT\TS_KBD";

/// Enumerator of the Remote Desktop input devices: one keyboard devnode per session,
/// `TERMINPUT_BUS\UMB\2&2C22BCC9&0&SESSION1KEYBOARD0` on 26200, below `UMB\…&TERMINPUT_BUS` and
/// `ROOT\UMBUS\0000`, in the built-in container.
pub const TERMINPUT_BUS: &str = "TERMINPUT_BUS";

/// True for the Remote Desktop keyboard of a session: enumerated by [`TERMINPUT_BUS`], or carrying
/// the hardware ID [`RDP_KEYBOARD_HARDWARE_ID`] (both case-insensitive). Such a keyboard types
/// what the Remote Desktop client sends; MKLM only shows it (plan 3.2).
pub fn is_remote_desktop_keyboard(instance_id: &str, hardware_ids: &[String]) -> bool {
    let (enumerator, _, _) = split_instance_id(instance_id);
    enumerator.eq_ignore_ascii_case(TERMINPUT_BUS)
        || hardware_ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case(RDP_KEYBOARD_HARDWARE_ID))
}

impl KeyboardDevice {
    /// See [`is_remote_desktop_keyboard`]. The reader gives it [`Transport::Virtual`]
    /// ([`classify_keyboard_transport`]), and the allowlist refuses to write it whatever its
    /// transport says (`crate::allowlist::check_device_writes`).
    pub fn is_remote_desktop(&self) -> bool {
        is_remote_desktop_keyboard(&self.instance_id, &self.hardware_ids)
    }
}

/// Classifies how a keyboard is attached, from what the reader knows of its devnode: the Remote
/// Desktop keyboard ([`is_remote_desktop_keyboard`], by its enumerator or its hardware ID) is
/// [`Transport::Virtual`] whatever enumerates it and whichever driver serves it, so it is
/// read-only wherever MKLM decides by transport (plan 3.2). Every other keyboard is classified by
/// [`classify_transport_with_bus_service`]. Rules that go by driver still hold: an i8042prt
/// devnode keeps its PS/2 reset ban and INV-PS2.
pub fn classify_keyboard_transport(
    instance_id: &str,
    hardware_ids: &[String],
    parent_chain: &[String],
    driver: &KeyboardDriver,
    bus_service: Option<&str>,
) -> Transport {
    if is_remote_desktop_keyboard(instance_id, hardware_ids) {
        Transport::Virtual
    } else {
        classify_transport_with_bus_service(instance_id, parent_chain, driver, bus_service)
    }
}

impl KeyboardDriver {
    /// Maps a devnode's service name (`DEVPKEY_Device_Service`) to a driver, case-insensitively.
    pub fn from_service(service: &str) -> Self {
        if service.eq_ignore_ascii_case("kbdhid") {
            KeyboardDriver::Kbdhid
        } else if service.eq_ignore_ascii_case("i8042prt") {
            KeyboardDriver::I8042prt
        } else {
            KeyboardDriver::Other(service.to_string())
        }
    }
}

/// Classifies how a keyboard is attached. See [`classify_transport_with_bus_service`].
pub fn classify_transport(
    instance_id: &str,
    parent_chain: &[String],
    driver: &KeyboardDriver,
) -> Transport {
    classify_transport_with_bus_service(instance_id, parent_chain, driver, None)
}

/// Classifies how a keyboard is attached.
///
/// `parent_chain` is nearest first; `bus_service` is the service of the nearest non-HID ancestor
/// (the HID transport minidriver, e.g. `hidi2c`) when the caller knows it.
///
/// Heuristics, in order:
/// 1. `i8042prt` → PS/2.
/// 2. `bus_service` `hidi2c` → I2C; `hidspi` / `hidspicx` → SPI.
/// 3. Walk the device and its ancestors, skipping `HID\` nodes; the first other node decides:
///    `USB\` → USB, `BTHLEDevice\` / `BTHLE\` → Bluetooth LE, `BTHENUM\` → Bluetooth Classic,
///    `ROOT\`, `SWD\`, `HTREE\`, `VMBUS\`, `TERMINPUT_BUS\` (the Remote Desktop keyboard),
///    `TS_*\`, `RDP*\` → virtual, `ACPI\` → I2C or SPI when an ID carries the HID-over-I2C/SPI
///    compatible ID (`PNP0C50` / `PNP0C51`) or an `I2C` / `SPI` token, else unknown. Physical
///    buses sit below `ROOT\ACPI_HAL`, so the virtual prefixes only match software-enumerated
///    devices.
/// 4. Only HID nodes known (e.g. a phantom without a parent): the HOGP / BT HID service UUID or a
///    USB-style `HID\VID_` ID decides.
pub fn classify_transport_with_bus_service(
    instance_id: &str,
    parent_chain: &[String],
    driver: &KeyboardDriver,
    bus_service: Option<&str>,
) -> Transport {
    if *driver == KeyboardDriver::I8042prt {
        return Transport::Ps2;
    }
    if let Some(service) = bus_service {
        if service.eq_ignore_ascii_case("hidi2c") {
            return Transport::I2c;
        }
        if service.eq_ignore_ascii_case("hidspi") || service.eq_ignore_ascii_case("hidspicx") {
            return Transport::Spi;
        }
    }

    let ids: Vec<&str> = std::iter::once(instance_id)
        .chain(parent_chain.iter().map(String::as_str))
        .collect();
    for (index, id) in ids.iter().enumerate() {
        let (enumerator, device, _) = split_instance_id(id);
        let enumerator = enumerator.to_ascii_uppercase();
        let device = device.to_ascii_uppercase();
        match enumerator.as_str() {
            "HID" => continue,
            "USB" => return Transport::Usb,
            "BTHLEDEVICE" | "BTHLE" => return Transport::BluetoothLe,
            "BTHENUM" => return Transport::BluetoothClassic,
            "ROOT" | "SWD" | "HTREE" | "VMBUS" | TERMINPUT_BUS => return Transport::Virtual,
            e if e.starts_with("TS_") || e.starts_with("RDP") || device.starts_with("RDP_") => {
                return Transport::Virtual;
            }
            "ACPI" => return acpi_transport(&ids[..=index]),
            _ => return Transport::Unknown,
        }
    }
    transport_from_hid_id(instance_id)
}

/// HID collection sitting directly on an ACPI device: I2C or SPI only when an ID says so.
fn acpi_transport(ids: &[&str]) -> Transport {
    let has_token = |pred: &dyn Fn(&str) -> bool| {
        ids.iter().any(|id| {
            id.to_ascii_uppercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(pred)
        })
    };
    if has_token(&|t| t == "PNP0C50" || t.starts_with("I2C")) {
        Transport::I2c
    } else if has_token(&|t| t == "PNP0C51" || t == "SPI" || t.starts_with("HIDSPI")) {
        Transport::Spi
    } else {
        Transport::Unknown
    }
}

fn transport_from_hid_id(instance_id: &str) -> Transport {
    let upper = instance_id.to_ascii_uppercase();
    let (enumerator, device, _) = split_instance_id(&upper);
    if upper.contains(HOGP_SERVICE) {
        Transport::BluetoothLe
    } else if upper.contains(BT_HID_SERVICE) {
        Transport::BluetoothClassic
    } else if enumerator == "HID" && device.starts_with("VID_") {
        Transport::Usb
    } else if device.contains("PNP0C50") {
        Transport::I2c
    } else {
        Transport::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const VXE: &str = r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F&COL02\B&B4852A&0&0001";
    const ACPI_TAIL: [&str; 3] = [
        r"ACPI\PNP0A08\0",
        r"ACPI_HAL\PNP0C08\0",
        r"ROOT\ACPI_HAL\0000",
    ];

    #[test]
    fn driver_from_service() {
        assert_eq!(
            KeyboardDriver::from_service("kbdhid"),
            KeyboardDriver::Kbdhid
        );
        assert_eq!(
            KeyboardDriver::from_service("KbdHid"),
            KeyboardDriver::Kbdhid
        );
        assert_eq!(
            KeyboardDriver::from_service("i8042prt"),
            KeyboardDriver::I8042prt
        );
        assert_eq!(
            KeyboardDriver::from_service("TermDD"),
            KeyboardDriver::Other("TermDD".to_string())
        );
        // The real Remote Desktop keyboard's service.
        assert_eq!(
            KeyboardDriver::from_service("terminpt"),
            KeyboardDriver::Other("terminpt".to_string())
        );
    }

    #[test]
    fn usb_keyboard() {
        let parents = chain(&[
            r"USB\VID_3434&PID_D027&MI_00\7&295e03ca&0&0000",
            r"USB\VID_3434&PID_D027\B76E483E3F08D96E",
        ]);
        assert_eq!(
            classify_transport(KEYCHRON, &parents, &KeyboardDriver::Kbdhid),
            Transport::Usb
        );
    }

    #[test]
    fn ble_keyboard_even_with_usb_radio_above() {
        let parents = chain(&[
            r"BTHLEDevice\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&0225a7_PID&fa6c_REV&0300_f977e93ba63f\9&1&0&0023",
            r"BTHLE\Dev_f977e93ba63f\8&2&0&f977e93ba63f",
            r"BTH\MS_BTHLE\7&3&0&3",
            r"USB\VID_8087&PID_0026\6&4&0&10",
        ]);
        assert_eq!(
            classify_transport(VXE, &parents, &KeyboardDriver::Kbdhid),
            Transport::BluetoothLe
        );
    }

    #[test]
    fn bt_classic_keyboard() {
        let id =
            r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002045e_PID&0763&Col01\9&1&0&0000";
        let parents = chain(&[
            r"BTHENUM\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002045e_PID&0763\8&1&0&112233445566_C00000000",
        ]);
        assert_eq!(
            classify_transport(id, &parents, &KeyboardDriver::Kbdhid),
            Transport::BluetoothClassic
        );
    }

    #[test]
    fn ps2_by_driver_despite_root_ancestor() {
        assert_eq!(
            classify_transport(
                r"ACPI\FUJ0309\4&320DB4C2&0",
                &chain(&ACPI_TAIL),
                &KeyboardDriver::I8042prt
            ),
            Transport::Ps2
        );
    }

    #[test]
    fn i2c_by_bus_service_or_id_hint() {
        let id = r"HID\ELAN0001&Col01\5&1&0&0000";
        let mut parents = chain(&[r"ACPI\ELAN0001\1"]);
        parents.extend(chain(&ACPI_TAIL));
        assert_eq!(
            classify_transport(id, &parents, &KeyboardDriver::Kbdhid),
            Transport::Unknown
        );
        assert_eq!(
            classify_transport_with_bus_service(
                id,
                &parents,
                &KeyboardDriver::Kbdhid,
                Some("hidi2c")
            ),
            Transport::I2c
        );
        assert_eq!(
            classify_transport_with_bus_service(
                id,
                &parents,
                &KeyboardDriver::Kbdhid,
                Some("HidSpiCx")
            ),
            Transport::Spi
        );

        let hinted = chain(&[r"ACPI\PNP0C50\1", ACPI_TAIL[0]]);
        assert_eq!(
            classify_transport(id, &hinted, &KeyboardDriver::Kbdhid),
            Transport::I2c
        );
        let spi = chain(&[r"ACPI\PNP0C51\1", ACPI_TAIL[0]]);
        assert_eq!(
            classify_transport(id, &spi, &KeyboardDriver::Kbdhid),
            Transport::Spi
        );
    }

    #[test]
    fn remote_desktop_keyboard_as_windows_enumerates_it() {
        let rdp = crate::fixtures::rdp_keyboard();
        assert_eq!(
            classify_transport(&rdp.instance_id, &rdp.parent_chain, &rdp.driver),
            Transport::Virtual
        );
        // The bus service `umbus` decides nothing.
        assert_eq!(
            classify_transport_with_bus_service(
                &rdp.instance_id,
                &rdp.parent_chain,
                &rdp.driver,
                Some("umbus")
            ),
            Transport::Virtual
        );
        assert!(rdp.is_remote_desktop());
        // By the enumerator alone, in any case, and by the hardware ID alone.
        assert!(is_remote_desktop_keyboard(
            &rdp.instance_id.to_ascii_lowercase(),
            &[]
        ));
        assert!(is_remote_desktop_keyboard(
            r"SOMEBUS\KBD\1",
            &[r"ts_inpt\ts_kbd".to_string()]
        ));
        // A hardware ID that only starts like it, or a parent on the bus, is not enough.
        assert!(!is_remote_desktop_keyboard(
            r"SOMEBUS\KBD\1",
            &[r"TS_INPT\TS_KBD_OTHER".to_string()]
        ));
        assert!(!is_remote_desktop_keyboard(
            r"UMB\UMB\1&841921D&0&TERMINPUT_BUS",
            &[]
        ));
        for kb in crate::fixtures::dev_machine().keyboards {
            assert!(!kb.is_remote_desktop(), "{}", kb.instance_id);
        }
    }

    /// The reader's decision (`mklm_win::devices::read_keyboard`): a Remote Desktop keyboard is
    /// virtual whatever the other heuristics would say, so writability never depends on them.
    #[test]
    fn the_remote_desktop_keyboard_is_always_virtual_to_the_reader() {
        let rdp = crate::fixtures::rdp_keyboard();
        let strings = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        let rdp_hwid = strings(&[RDP_KEYBOARD_HARDWARE_ID]);
        let kbdhid = KeyboardDriver::Kbdhid;
        let on_somebus = |driver: &KeyboardDriver, bus_service: Option<&str>| {
            classify_keyboard_transport(r"SOMEBUS\KBD\1", &rdp_hwid, &[], driver, bus_service)
        };
        // As Windows enumerates it (TERMINPUT_BUS, bus service umbus), and by the enumerator alone.
        assert_eq!(
            classify_keyboard_transport(
                &rdp.instance_id,
                &rdp.hardware_ids,
                &rdp.parent_chain,
                &rdp.driver,
                Some("umbus")
            ),
            Transport::Virtual
        );
        assert_eq!(
            classify_keyboard_transport(&rdp.instance_id, &[], &[], &rdp.driver, None),
            Transport::Virtual
        );
        // The hardware ID alone, on a bus the other heuristics call unknown, or I2C through its
        // bus service, or PS/2 through its driver (its PS/2 reset ban and INV-PS2 go by driver).
        assert_eq!(on_somebus(&kbdhid, None), Transport::Virtual);
        assert_eq!(on_somebus(&kbdhid, Some("hidi2c")), Transport::Virtual);
        assert_eq!(
            on_somebus(&KeyboardDriver::I8042prt, None),
            Transport::Virtual
        );
        let ps2 = crate::fixtures::internal_ps2();
        assert_eq!(
            crate::safety::structural_reset_bans(&KeyboardDevice {
                instance_id: r"SOMEBUS\KBD\1".into(),
                hardware_ids: rdp_hwid.clone(),
                transport: on_somebus(&KeyboardDriver::I8042prt, None),
                ..ps2.clone()
            }),
            vec![
                crate::safety::ResetBan::Ps2Driver,
                crate::safety::ResetBan::InternalContainer,
                crate::safety::ResetBan::Virtual
            ]
        );

        // Any other keyboard is classified as before.
        for kb in crate::fixtures::dev_machine().keyboards {
            assert_eq!(
                classify_keyboard_transport(
                    &kb.instance_id,
                    &kb.hardware_ids,
                    &kb.parent_chain,
                    &kb.driver,
                    None
                ),
                kb.transport,
                "{}",
                kb.instance_id
            );
        }
        let other_hwid = strings(&[r"HID\VID_1234&PID_5678"]);
        assert_eq!(
            classify_keyboard_transport(
                r"SOMEBUS\KBD\1",
                &other_hwid,
                &[],
                &kbdhid,
                Some("hidi2c")
            ),
            Transport::I2c
        );
        assert_eq!(
            classify_keyboard_transport(
                &ps2.instance_id,
                &ps2.hardware_ids,
                &ps2.parent_chain,
                &ps2.driver,
                None
            ),
            Transport::Ps2
        );
    }

    /// Synthetic IDs (not what Windows uses; the real Remote Desktop keyboard is
    /// `fixtures::rdp_keyboard`): virtual enumerators and prefixes still classify as virtual.
    #[test]
    fn virtual_keyboards() {
        let other = KeyboardDriver::Other("TermDD".to_string());
        assert_eq!(
            classify_transport(r"ROOT\RDP_KBD\0000", &[], &other),
            Transport::Virtual
        );
        assert_eq!(
            classify_transport(r"TS_INPT\TS_KBD\1&2&0", &[], &other),
            Transport::Virtual
        );
        assert_eq!(
            classify_transport(
                r"VMBUS\{f912ad6d-2b17-48ea-bd65-f927a61c7684}\5&1",
                &[],
                &other
            ),
            Transport::Virtual
        );
        assert_eq!(
            classify_transport(
                r"HID\VirtualKbd\1&2&0&0000",
                &chain(&[r"ROOT\SYSTEM\0001"]),
                &KeyboardDriver::Kbdhid
            ),
            Transport::Virtual
        );
        assert_eq!(
            classify_transport(
                r"HID\Vhf\2&3&0&0000",
                &chain(&[r"SWD\VHF\1"]),
                &KeyboardDriver::Kbdhid
            ),
            Transport::Virtual
        );
    }

    #[test]
    fn phantom_without_parents_uses_id_shape() {
        let ms = r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&02045E_PID&0040_REV&0300_788712B99B19&COL01\B&1455DB79&0&0000";
        assert_eq!(
            classify_transport(ms, &[], &KeyboardDriver::Kbdhid),
            Transport::BluetoothLe
        );
        assert_eq!(
            classify_transport(KEYCHRON, &[], &KeyboardDriver::Kbdhid),
            Transport::Usb
        );
        assert_eq!(
            classify_transport(
                r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002045e_PID&0763&Col01\9&1",
                &[],
                &KeyboardDriver::Kbdhid
            ),
            Transport::BluetoothClassic
        );
        assert_eq!(
            classify_transport(r"HID\ELAN0001&Col01\5&1", &[], &KeyboardDriver::Kbdhid),
            Transport::Unknown
        );
    }

    #[test]
    fn unknown_bus() {
        assert_eq!(
            classify_transport(
                r"HID\SAM&Col01\1",
                &chain(&[r"SAM\KBD\1"]),
                &KeyboardDriver::Kbdhid
            ),
            Transport::Unknown
        );
    }
}
