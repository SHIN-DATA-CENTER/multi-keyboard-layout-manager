//! Device nodes and unified device properties (CfgMgr32), read-only.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW,
    CM_Get_Device_Interface_PropertyW, CM_LOCATE_DEVNODE_PHANTOM, CM_Locate_DevNodeW, CONFIGRET,
    CR_BUFFER_SMALL, CR_INVALID_DEVICE_ID, CR_NO_SUCH_VALUE, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_InstanceId, DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_GUID, DEVPROP_TYPE_STRING,
    DEVPROP_TYPE_STRING_LIST, DEVPROP_TYPE_UINT32, DEVPROPTYPE,
};
use windows::Win32::Foundation::DEVPROPKEY;
use windows::core::PCWSTR;

use crate::error::Error;

/// Attempts for calls whose required buffer size can grow between the size query and the read.
const MAX_ATTEMPTS: usize = 8;

/// A located device node (`DEVINST`).
///
/// A devnode handle is a plain number, not a kernel handle, so it needs no cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DevNode(u32);

impl DevNode {
    /// Locates a devnode by instance ID, including non-present (phantom) devnodes.
    ///
    /// An empty ID would locate the root devnode and an embedded NUL would truncate the ID, so both
    /// are rejected with `CR_INVALID_DEVICE_ID`.
    pub(crate) fn locate(instance_id: &str) -> Result<Self, Error> {
        if instance_id.trim().is_empty() || instance_id.contains('\0') {
            return Err(config_ret("CM_Locate_DevNodeW", CR_INVALID_DEVICE_ID));
        }
        let id = to_wide(instance_id);
        let mut devinst = 0u32;
        // SAFETY: `id` is NUL-terminated and outlives the call; `devinst` is a valid out pointer.
        let cr = unsafe {
            CM_Locate_DevNodeW(&mut devinst, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_PHANTOM)
        };
        check(cr, "CM_Locate_DevNodeW")?;
        Ok(Self(devinst))
    }

    /// The raw `DEVINST`.
    pub(crate) fn raw(self) -> u32 {
        self.0
    }

    /// Data of a property of type `expected`. `Ok(None)` when the property does not exist; an error
    /// when it cannot be read or has another type.
    fn typed(self, key: &DEVPROPKEY, expected: DEVPROPTYPE) -> Result<Option<Vec<u8>>, Error> {
        let property = read_property("CM_Get_DevNode_PropertyW", |ty, buffer, size| {
            // SAFETY: `read_property` passes a buffer of at least `*size` writable bytes (or none
            // with a size of 0); `key`, `ty` and `size` are valid for the duration of the call.
            unsafe { CM_Get_DevNode_PropertyW(self.0, key, ty, buffer, size, 0) }
        })?;
        match property {
            None => Ok(None),
            Some((ty, data)) if ty == expected => Ok(Some(data)),
            Some((ty, data)) => Err(unexpected_property(ty, &data, expected)),
        }
    }

    /// A `DEVPROP_TYPE_STRING` property; `Ok(None)` when missing or empty.
    pub(crate) fn string(self, key: &DEVPROPKEY) -> Result<Option<String>, Error> {
        Ok(self
            .typed(key, DEVPROP_TYPE_STRING)?
            .and_then(|data| decode_string(&data)))
    }

    /// A `DEVPROP_TYPE_STRING_LIST` property; empty when missing.
    pub(crate) fn strings(self, key: &DEVPROPKEY) -> Result<Vec<String>, Error> {
        Ok(self
            .typed(key, DEVPROP_TYPE_STRING_LIST)?
            .map(|data| decode_string_list(&data))
            .unwrap_or_default())
    }

    /// A `DEVPROP_TYPE_UINT32` property.
    pub(crate) fn u32(self, key: &DEVPROPKEY) -> Result<Option<u32>, Error> {
        self.typed(key, DEVPROP_TYPE_UINT32)?
            .map(|data| {
                decode_u32(&data).ok_or_else(|| {
                    unexpected_property(DEVPROP_TYPE_UINT32, &data, DEVPROP_TYPE_UINT32)
                })
            })
            .transpose()
    }

    /// A `DEVPROP_TYPE_BOOLEAN` property.
    pub(crate) fn bool(self, key: &DEVPROPKEY) -> Result<Option<bool>, Error> {
        self.typed(key, DEVPROP_TYPE_BOOLEAN)?
            .map(|data| match data.as_slice() {
                [byte] => Ok(*byte != 0),
                _ => Err(unexpected_property(
                    DEVPROP_TYPE_BOOLEAN,
                    &data,
                    DEVPROP_TYPE_BOOLEAN,
                )),
            })
            .transpose()
    }

    /// A `DEVPROP_TYPE_GUID` property in upper-case braces form.
    pub(crate) fn guid(self, key: &DEVPROPKEY) -> Result<Option<String>, Error> {
        self.typed(key, DEVPROP_TYPE_GUID)?
            .map(|data| {
                format_guid(&data)
                    .ok_or_else(|| unexpected_property(DEVPROP_TYPE_GUID, &data, DEVPROP_TYPE_GUID))
            })
            .transpose()
    }
}

fn unexpected_property(ty: DEVPROPTYPE, data: &[u8], expected: DEVPROPTYPE) -> Error {
    Error::UnexpectedProperty {
        ty: ty.0,
        size: data.len(),
        expected: expected.0,
    }
}

/// Instance IDs matching a CfgMgr32 filter (`CM_Get_Device_ID_ListW`), retrying while the list grows.
pub(crate) fn device_id_list(filter: &str, flags: u32) -> Result<Vec<String>, Error> {
    let filter = to_wide(filter);
    for _ in 0..MAX_ATTEMPTS {
        let mut len = 0u32;
        // SAFETY: `filter` is NUL-terminated and outlives the call; `len` is a valid out pointer.
        let cr = unsafe { CM_Get_Device_ID_List_SizeW(&mut len, PCWSTR(filter.as_ptr()), flags) };
        check(cr, "CM_Get_Device_ID_List_SizeW")?;
        // The size includes the final NUL; keep room for an empty double-NUL list.
        let mut buffer = vec![0u16; (len as usize).max(2)];
        // SAFETY: `filter` is NUL-terminated; the slice length tells the API the buffer size.
        let cr = unsafe { CM_Get_Device_ID_ListW(PCWSTR(filter.as_ptr()), &mut buffer, flags) };
        if cr == CR_BUFFER_SMALL {
            continue;
        }
        check(cr, "CM_Get_Device_ID_ListW")?;
        return Ok(split_multi_sz(&buffer));
    }
    Err(config_ret("CM_Get_Device_ID_ListW", CR_BUFFER_SMALL))
}

/// Instance ID behind a device interface path, from `DEVPKEY_Device_InstanceId`.
pub(crate) fn interface_instance_id(interface_path: &str) -> Result<Option<String>, Error> {
    let path = to_wide(interface_path);
    let property = read_property("CM_Get_Device_Interface_PropertyW", |ty, buffer, size| {
        // SAFETY: `path` is NUL-terminated and outlives the call; `read_property` passes a buffer
        // of at least `*size` writable bytes (or none with a size of 0).
        unsafe {
            CM_Get_Device_Interface_PropertyW(
                PCWSTR(path.as_ptr()),
                &DEVPKEY_Device_InstanceId,
                ty,
                buffer,
                size,
                0,
            )
        }
    })?;
    match property {
        None => Ok(None),
        Some((ty, data)) if ty == DEVPROP_TYPE_STRING => Ok(decode_string(&data)),
        Some((ty, data)) => Err(unexpected_property(ty, &data, DEVPROP_TYPE_STRING)),
    }
}

/// Runs a "query size, then read" CfgMgr32 property call of `function`.
///
/// `Ok(None)` only when the property does not exist (`CR_NO_SUCH_VALUE`). Every other failure,
/// including a buffer size that keeps growing, is an error, so that "unreadable" is never
/// mistaken for "absent".
fn read_property(
    function: &'static str,
    mut call: impl FnMut(&mut DEVPROPTYPE, Option<*mut u8>, &mut u32) -> CONFIGRET,
) -> Result<Option<(DEVPROPTYPE, Vec<u8>)>, Error> {
    let mut buffer: Vec<u8> = Vec::new();
    for _ in 0..MAX_ATTEMPTS {
        let mut ty = DEVPROPTYPE(0);
        // The buffer is only ever sized from a `u32` the API returned.
        let mut size =
            u32::try_from(buffer.len()).map_err(|_| config_ret(function, CR_BUFFER_SMALL))?;
        let pointer = (!buffer.is_empty()).then_some(buffer.as_mut_ptr());
        match call(&mut ty, pointer, &mut size) {
            CR_SUCCESS => {
                buffer.truncate(size as usize);
                return Ok(Some((ty, buffer)));
            }
            CR_NO_SUCH_VALUE => return Ok(None),
            CR_BUFFER_SMALL if size as usize > buffer.len() => buffer = vec![0; size as usize],
            cr => return Err(config_ret(function, cr)),
        }
    }
    Err(config_ret(function, CR_BUFFER_SMALL))
}

pub(crate) fn check(cr: CONFIGRET, function: &'static str) -> Result<(), Error> {
    if cr == CR_SUCCESS {
        Ok(())
    } else {
        Err(config_ret(function, cr))
    }
}

pub(crate) fn config_ret(function: &'static str, cr: CONFIGRET) -> Error {
    Error::ConfigRet {
        function,
        code: cr.0,
    }
}

/// NUL-terminated UTF-16 copy of `s`.
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 text up to the first NUL (or the end of the slice).
pub(crate) fn from_wide(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

fn wide_units(bytes: &[u8]) -> Vec<u16> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect()
}

/// Decodes a `DEVPROP_TYPE_STRING` buffer. Empty strings are `None`.
pub(crate) fn decode_string(bytes: &[u8]) -> Option<String> {
    Some(from_wide(&wide_units(bytes))).filter(|s| !s.is_empty())
}

/// Decodes a `DEVPROP_TYPE_STRING_LIST` (REG_MULTI_SZ) buffer.
pub(crate) fn decode_string_list(bytes: &[u8]) -> Vec<String> {
    split_multi_sz(&wide_units(bytes))
}

/// Splits a double-NUL-terminated UTF-16 list, skipping empty entries.
pub(crate) fn split_multi_sz(wide: &[u16]) -> Vec<String> {
    wide.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

pub(crate) fn decode_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// Formats a 16-byte GUID buffer as `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` (upper case).
pub(crate) fn format_guid(bytes: &[u8]) -> Option<String> {
    let b: &[u8; 16] = bytes.try_into().ok()?;
    let data1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let data2 = u16::from_le_bytes([b[4], b[5]]);
    let data3 = u16::from_le_bytes([b[6], b[7]]);
    Some(format!(
        "{{{data1:08X}-{data2:04X}-{data3:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Devices::DeviceAndDriverInstallation::CR_INVALID_DEVNODE;

    fn utf16_bytes(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn missing_and_unreadable_properties_differ() {
        let fake = |cr: CONFIGRET| read_property("Fake", move |_, _, _| cr);
        assert_eq!(fake(CR_NO_SUCH_VALUE), Ok(None));
        assert_eq!(
            fake(CR_INVALID_DEVNODE),
            Err(Error::ConfigRet {
                function: "Fake",
                code: CR_INVALID_DEVNODE.0
            })
        );
        // A size that keeps growing gives up with an error, never with "missing".
        let mut grow = 0u32;
        let result = read_property("Fake", |_, _, size| {
            grow += 4;
            *size = grow;
            CR_BUFFER_SMALL
        });
        assert_eq!(
            result,
            Err(Error::ConfigRet {
                function: "Fake",
                code: CR_BUFFER_SMALL.0
            })
        );
    }

    #[test]
    fn property_is_read_after_the_size_query() {
        let result = read_property("Fake", |ty, buffer, size| match buffer {
            None => {
                *size = 4;
                CR_BUFFER_SMALL
            }
            Some(pointer) => {
                assert_eq!(*size, 4);
                // SAFETY: `read_property` passes a buffer of `*size` (4) writable bytes.
                unsafe { pointer.copy_from_nonoverlapping([7u8, 0, 0, 0].as_ptr(), 4) };
                *ty = DEVPROP_TYPE_UINT32;
                CR_SUCCESS
            }
        });
        assert_eq!(result, Ok(Some((DEVPROP_TYPE_UINT32, vec![7, 0, 0, 0]))));
    }

    #[test]
    fn strings_stop_at_nul() {
        assert_eq!(
            decode_string(&utf16_bytes("kbdhid\0junk")).as_deref(),
            Some("kbdhid")
        );
        assert_eq!(
            decode_string(&utf16_bytes("HID キーボード デバイス\0")).as_deref(),
            Some("HID キーボード デバイス")
        );
        assert_eq!(decode_string(&utf16_bytes("\0")), None);
        assert_eq!(decode_string(&[]), None);
        // An odd trailing byte is ignored rather than panicking.
        assert_eq!(decode_string(&[b'A', 0, b'B']).as_deref(), Some("A"));
    }

    #[test]
    fn string_lists() {
        assert_eq!(
            decode_string_list(&utf16_bytes(
                "HID\\VID_3434&PID_D027&MI_00&Col01\0HID_DEVICE_SYSTEM_KEYBOARD\0\0"
            )),
            vec![
                "HID\\VID_3434&PID_D027&MI_00&Col01".to_string(),
                "HID_DEVICE_SYSTEM_KEYBOARD".to_string()
            ]
        );
        assert!(decode_string_list(&utf16_bytes("\0\0")).is_empty());
        assert!(split_multi_sz(&[]).is_empty());
    }

    #[test]
    fn numbers_need_exact_size() {
        assert_eq!(decode_u32(&[4, 0, 0, 0]), Some(4));
        assert_eq!(decode_u32(&[0x0A, 0, 0x80, 0x01]), Some(0x0180_000A));
        assert_eq!(decode_u32(&[4, 0, 0]), None);
        assert_eq!(decode_u32(&[4, 0, 0, 0, 0]), None);
    }

    #[test]
    fn guid_upper_braces() {
        // {F0D991EA-A583-5B9C-800D-48846AC6E633} in memory layout.
        let bytes = [
            0xEA, 0x91, 0xD9, 0xF0, 0x83, 0xA5, 0x9C, 0x5B, 0x80, 0x0D, 0x48, 0x84, 0x6A, 0xC6,
            0xE6, 0x33,
        ];
        assert_eq!(
            format_guid(&bytes).as_deref(),
            Some("{F0D991EA-A583-5B9C-800D-48846AC6E633}")
        );
        let internal = [
            0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ];
        assert_eq!(
            format_guid(&internal).as_deref(),
            Some(mklm_core::INTERNAL_CONTAINER_ID)
        );
        assert_eq!(format_guid(&bytes[..15]), None);
    }

    #[test]
    fn wide_round_trip() {
        let wide = to_wide("ACPI\\FUJ0309\\4&320DB4C2&0");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(from_wide(&wide), "ACPI\\FUJ0309\\4&320DB4C2&0");
        assert_eq!(from_wide(&[0x41, 0x42]), "AB");
    }
}
