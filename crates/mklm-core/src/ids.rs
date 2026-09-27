//! Parsing of PnP device IDs and input-locale identifiers.
//!
//! Everything here works on the strings Windows hands out (instance IDs, hardware IDs, KLIDs,
//! HKLs) and never touches the system.

use serde::{Deserialize, Serialize};

/// Who assigned a vendor ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VendorIdSource {
    /// USB-IF (USB devices, and Bluetooth devices that report a USB vendor ID).
    UsbIf,
    /// Bluetooth SIG company identifier.
    BluetoothSig,
    /// Any other source code, or a format without one.
    Unknown,
}

/// Vendor and product ID found in a device ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VendorProduct {
    pub vendor_id: u16,
    pub product_id: u16,
    pub vendor_id_source: VendorIdSource,
}

/// Extracts the vendor and product ID from an instance ID or hardware ID.
///
/// Recognised shapes (case-insensitive):
/// - USB: `VID_3434&PID_D027`
/// - Bluetooth LE (HOGP): `_DEV_VID&0225A7_PID&FA6C`; the first byte of the vendor field is the
///   vendor-ID source (`01` Bluetooth SIG, `02` USB-IF), the last four digits are the vendor ID.
/// - Bluetooth Classic: `_VID&0002045E_PID&0763`; the first four digits are the source.
pub fn parse_vendor_product(id: &str) -> Option<VendorProduct> {
    let upper = id.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let mut from = 0;
    while let Some(pos) = upper[from..].find("VID") {
        let at = from + pos;
        from = at + 3;
        if !is_boundary(bytes, at) {
            continue;
        }
        if let Some(found) = parse_at(bytes, at) {
            return Some(found);
        }
    }
    None
}

/// First vendor/product ID found in the instance ID, else in the hardware IDs (in order).
pub fn vendor_product_from_ids(
    instance_id: &str,
    hardware_ids: &[String],
) -> Option<VendorProduct> {
    std::iter::once(instance_id)
        .chain(hardware_ids.iter().map(String::as_str))
        .find_map(parse_vendor_product)
}

fn is_boundary(bytes: &[u8], at: usize) -> bool {
    at == 0 || matches!(bytes[at - 1], b'\\' | b'&' | b'_' | b'#' | b'}')
}

fn hex_run(bytes: &[u8], start: usize) -> &[u8] {
    let tail = bytes.get(start..).unwrap_or(&[]);
    let len = tail.iter().take_while(|b| b.is_ascii_hexdigit()).count();
    &tail[..len]
}

fn hex_value(digits: &[u8]) -> Option<u32> {
    std::str::from_utf8(digits)
        .ok()
        .and_then(|s| u32::from_str_radix(s, 16).ok())
}

/// Parses `VID<sep><digits><sep>PID<sep><4 digits>` starting at `at` (the `V` of `VID`).
fn parse_at(bytes: &[u8], at: usize) -> Option<VendorProduct> {
    let sep = *bytes.get(at + 3)?;
    let digits = hex_run(bytes, at + 4);
    let (vendor_id_source, vendor_id) = match (sep, digits.len()) {
        (b'_', 4) => (VendorIdSource::UsbIf, hex_value(digits)?),
        (b'&', 4) => (VendorIdSource::Unknown, hex_value(digits)?),
        (b'&', 6) => (
            source_from_code(hex_value(&digits[..2])?),
            hex_value(&digits[2..])?,
        ),
        (b'&', 8) => (
            source_from_code(hex_value(&digits[..4])?),
            hex_value(&digits[4..])?,
        ),
        _ => return None,
    };
    // The product ID must follow right after one separator.
    let pid_at = at + 4 + digits.len() + 1;
    if !matches!(bytes.get(pid_at - 1), Some(b'&' | b'_'))
        || bytes.get(pid_at..pid_at + 3) != Some(b"PID")
    {
        return None;
    }
    if !matches!(bytes.get(pid_at + 3), Some(b'&' | b'_')) {
        return None;
    }
    let pid_digits = hex_run(bytes, pid_at + 4);
    if pid_digits.len() != 4 {
        return None;
    }
    Some(VendorProduct {
        vendor_id: u16::try_from(vendor_id).ok()?,
        product_id: u16::try_from(hex_value(pid_digits)?).ok()?,
        vendor_id_source,
    })
}

fn source_from_code(code: u32) -> VendorIdSource {
    match code {
        1 => VendorIdSource::BluetoothSig,
        2 => VendorIdSource::UsbIf,
        _ => VendorIdSource::Unknown,
    }
}

/// Splits an instance ID into enumerator, device part and instance suffix.
pub(crate) fn split_instance_id(id: &str) -> (&str, &str, &str) {
    let mut parts = id.splitn(3, '\\');
    (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    )
}

/// Serial number embedded in a USB device instance ID such as `USB\VID_3434&PID_D027\B76E483E3F08D96E`.
///
/// Windows generates the suffix (e.g. `7&295e03ca&0&0000`, always containing `&`) when the device has
/// no usable serial number, so only a suffix without `&` is a serial. Interface nodes (`&MI_xx`) never
/// carry one.
pub fn usb_serial_from_instance_id(instance_id: &str) -> Option<String> {
    let (enumerator, device, suffix) = split_instance_id(instance_id);
    if !enumerator.eq_ignore_ascii_case("USB")
        || device.to_ascii_uppercase().contains("&MI_")
        || suffix.is_empty()
        || suffix.contains(['&', '\\'])
    {
        return None;
    }
    Some(suffix.to_string())
}

/// Serial number of the USB device a keyboard hangs off: the nearest `USB\` ancestor that is not an
/// interface node decides (`parent_chain` is nearest first).
pub fn usb_serial_from_chain(parent_chain: &[String]) -> Option<String> {
    parent_chain
        .iter()
        .find(|id| {
            let (enumerator, device, _) = split_instance_id(id);
            enumerator.eq_ignore_ascii_case("USB") && !device.to_ascii_uppercase().contains("&MI_")
        })
        .and_then(|id| usb_serial_from_instance_id(id))
}

/// Language ID of Japanese (Japan), which is also the layout ID of kbdjpn.dll (KLID `00000411`).
pub const LANGID_JAPANESE: u16 = 0x0411;

/// Parses a KLID (keyboard layout ID, 8 hex digits such as `"00000411"` or `"E0200411"`).
pub fn parse_klid(klid: &str) -> Option<u32> {
    let klid = klid.trim();
    if klid.len() != 8 || !klid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(klid, 16).ok()
}

/// True when the KLID loads the Japanese layout kbdjpn.dll, which per-keyboard layouts need:
/// `00000411`, or an IMM32 IME `E0xx0411` (whose layout file is kbdjpn.dll).
///
/// The language alone does not decide: `a0000411` is a custom layout. Preload substitutes
/// (`Dxxxxxxx`, see `Keyboard Layout\Substitutes`) must be resolved first; unresolved ones are not
/// Japanese. Unparsable KLIDs are not Japanese.
pub fn klid_has_japanese_layout(klid: &str) -> bool {
    parse_klid(klid).is_some_and(|value| {
        value == u32::from(LANGID_JAPANESE)
            || (value >> 28 == 0xE && value as u16 == LANGID_JAPANESE)
    })
}

/// True when the HKL's layout is kbdjpn.dll: its layout part (high word) is `0x0411`, or it is an
/// IMM32 IME HKL `E0xx0411`.
///
/// The input language (low word) alone does not decide: `0x04090411` is the Japanese language with
/// the US layout (kbdus.dll), and `0x04110409` is English with the Japanese layout.
pub const fn hkl_has_japanese_layout(hkl: u32) -> bool {
    let layout = (hkl >> 16) as u16;
    layout == LANGID_JAPANESE || (layout >> 12 == 0xE && hkl as u16 == LANGID_JAPANESE)
}

/// Formats the low 32 bits of an HKL the way Windows tools print it, e.g. `04110411`.
pub fn hkl_to_string(hkl: u32) -> String {
    format!("{hkl:08X}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(
        vendor_id: u16,
        product_id: u16,
        vendor_id_source: VendorIdSource,
    ) -> Option<VendorProduct> {
        Some(VendorProduct {
            vendor_id,
            product_id,
            vendor_id_source,
        })
    }

    #[test]
    fn usb_style_ids() {
        let expected = vp(0x3434, 0xD027, VendorIdSource::UsbIf);
        assert_eq!(
            parse_vendor_product(r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000"),
            expected
        );
        assert_eq!(
            parse_vendor_product(r"USB\VID_3434&PID_D027\B76E483E3F08D96E"),
            expected
        );
        assert_eq!(
            parse_vendor_product(r"usb\vid_3434&pid_d027&mi_00"),
            expected
        );
        assert_eq!(
            parse_vendor_product("HID\\VID_3434&PID_D027&REV_0100&MI_00&Col01"),
            expected
        );
    }

    #[test]
    fn ble_hogp_ids() {
        assert_eq!(
            parse_vendor_product(
                r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F&COL02\B&B4852A&0&0001"
            ),
            vp(0x25A7, 0xFA6C, VendorIdSource::UsbIf)
        );
        assert_eq!(
            parse_vendor_product(
                r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&02045E_PID&0040_REV&0300_788712B99B19&COL01\B&1455DB79&0&0000"
            ),
            vp(0x045E, 0x0040, VendorIdSource::UsbIf)
        );
        assert_eq!(
            parse_vendor_product(
                r"BTHLEDevice\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&01000f_PID&1234_REV&0001_aabbccddeeff"
            ),
            vp(0x000F, 0x1234, VendorIdSource::BluetoothSig)
        );
    }

    #[test]
    fn bt_classic_ids() {
        assert_eq!(
            parse_vendor_product(
                r"BTHENUM\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002045e_PID&0763\8&1234&0&112233445566_C00000000"
            ),
            vp(0x045E, 0x0763, VendorIdSource::UsbIf)
        );
        assert_eq!(
            parse_vendor_product(
                r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&0267&Col01"
            ),
            vp(0x004C, 0x0267, VendorIdSource::BluetoothSig)
        );
    }

    #[test]
    fn ids_without_vendor_product() {
        assert_eq!(parse_vendor_product(r"ACPI\FUJ0309\4&320DB4C2&0"), None);
        assert_eq!(parse_vendor_product(r"ROOT\RDP_KBD\0000"), None);
        assert_eq!(parse_vendor_product(r"HID\VID_12&PID_3456"), None);
        assert_eq!(parse_vendor_product(r"HID\VID_1234&XID_3456"), None);
        assert_eq!(parse_vendor_product(r"HID\VID_1234&PID_345"), None);
        // "VID" inside another token is not a vendor field.
        assert_eq!(parse_vendor_product(r"HID\DAVID_1234&PID_3456"), None);
        assert_eq!(parse_vendor_product(""), None);
    }

    #[test]
    fn falls_back_to_hardware_ids() {
        let hwids = vec![r"HID\VID_046D&PID_C52B&MI_00&Col01".to_string()];
        assert_eq!(
            vendor_product_from_ids(r"HID\ELAN0001&Col01\5&1&0&0000", &hwids),
            vp(0x046D, 0xC52B, VendorIdSource::UsbIf)
        );
        assert_eq!(
            vendor_product_from_ids(r"ACPI\FUJ0309\4&320DB4C2&0", &[]),
            None
        );
    }

    #[test]
    fn usb_serial_only_without_ampersand() {
        assert_eq!(
            usb_serial_from_instance_id(r"USB\VID_3434&PID_D027\B76E483E3F08D96E").as_deref(),
            Some("B76E483E3F08D96E")
        );
        assert_eq!(
            usb_serial_from_instance_id(r"USB\VID_3434&PID_D027&MI_00\7&295e03ca&0&0000"),
            None
        );
        assert_eq!(
            usb_serial_from_instance_id(r"USB\VID_046D&PID_C52B\5&1b0c7a4b&0&2"),
            None
        );
        assert_eq!(
            usb_serial_from_instance_id(r"HID\VID_3434&PID_D027\B76E483E3F08D96E"),
            None
        );
        assert_eq!(usb_serial_from_instance_id(r"USB\VID_3434&PID_D027\"), None);
        assert_eq!(usb_serial_from_instance_id(r"USB\VID_3434&PID_D027"), None);
    }

    #[test]
    fn usb_serial_from_parent_chain() {
        let chain = vec![
            r"USB\VID_3434&PID_D027&MI_00\7&295e03ca&0&0000".to_string(),
            r"USB\VID_3434&PID_D027\B76E483E3F08D96E".to_string(),
        ];
        assert_eq!(
            usb_serial_from_chain(&chain).as_deref(),
            Some("B76E483E3F08D96E")
        );

        // The nearest USB device decides; a hub further up never lends its serial.
        let no_serial = vec![
            r"USB\VID_046D&PID_C52B&MI_00\6&2a3b&0&0000".to_string(),
            r"USB\VID_046D&PID_C52B\5&1b0c7a4b&0&2".to_string(),
            r"USB\VID_05E3&PID_0610\HUBSERIAL01".to_string(),
        ];
        assert_eq!(usb_serial_from_chain(&no_serial), None);

        let ble = vec![r"BTHLEDevice\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&0225a7_PID&fa6c_REV&0300_f977e93ba63f\9&1&0&0023".to_string()];
        assert_eq!(usb_serial_from_chain(&ble), None);
        assert_eq!(usb_serial_from_chain(&[]), None);
    }

    #[test]
    fn klid_layout() {
        assert!(klid_has_japanese_layout("00000411"));
        assert!(klid_has_japanese_layout("E0200411"));
        assert!(klid_has_japanese_layout("e0010411"));
        assert!(!klid_has_japanese_layout("00000409"));
        assert!(!klid_has_japanese_layout("d0010409"));
        // Japanese language, but not kbdjpn.dll: a custom layout or an unresolved substitute.
        assert!(!klid_has_japanese_layout("a0000411"));
        assert!(!klid_has_japanese_layout("d0010411"));
        assert!(!klid_has_japanese_layout("E0200409"));
        assert!(!klid_has_japanese_layout("411"));
        assert!(!klid_has_japanese_layout("0000041G"));
        assert_eq!(parse_klid("00000411"), Some(0x411));
        assert_eq!(parse_klid(" 00000411 "), Some(0x411));
    }

    #[test]
    fn hkl_layout() {
        assert!(hkl_has_japanese_layout(0x0411_0411));
        assert!(hkl_has_japanese_layout(0xE020_0411));
        assert!(!hkl_has_japanese_layout(0x0409_0409));
        // The layout part decides, not the language.
        assert!(!hkl_has_japanese_layout(0x0409_0411));
        assert!(hkl_has_japanese_layout(0x0411_0409));
        assert!(!hkl_has_japanese_layout(0xE020_0409));
        assert!(!hkl_has_japanese_layout(0x0000_0811));
        assert_eq!(hkl_to_string(0x0411_0411), "04110411");
        assert_eq!(hkl_to_string(0xE020_0411), "E0200411");
    }
}
