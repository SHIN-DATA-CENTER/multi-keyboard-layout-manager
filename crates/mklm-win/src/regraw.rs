//! Raw registry value I/O for the write-side modules ([`crate::regwrite`],
//! [`crate::journal_store`]), and the mapping between stored bytes and [`RegValue`].
//!
//! Unlike `windows-registry`'s value iterator, enumeration here never ends silently: a value that
//! grows between the size query and the read is read again, and every other error is reported,
//! so that a journal is never read with entries missing.

use mklm_core::RegValue;
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RegDeleteValueW, RegEnumValueW, RegFlushKey,
    RegQueryInfoKeyW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{PCWSTR, PWSTR};

use crate::error::Error;
use crate::props::to_wide;

/// Attempts while a value keeps growing between the size query and the read.
const MAX_ATTEMPTS: usize = 8;

/// Longest value name the registry allows, in UTF-16 units without the NUL.
const MAX_VALUE_NAME: usize = 16_383;

/// A value as stored: its `REG_*` type and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawValue {
    pub(crate) ty: u32,
    pub(crate) data: Vec<u8>,
}

fn registry_error(path: &str, name: &str, code: WIN32_ERROR) -> Error {
    Error::Registry {
        path: format!(r"{path}\{name}"),
        code: code.0,
    }
}

/// Reads one value; `Ok(None)` when it does not exist. `path` only labels errors.
pub(crate) fn query_value(key: HKEY, path: &str, name: &str) -> Result<Option<RawValue>, Error> {
    let wide_name = to_wide(name);
    let mut data: Vec<u8> = Vec::new();
    for _ in 0..MAX_ATTEMPTS {
        let mut ty = REG_VALUE_TYPE(0);
        let mut len =
            u32::try_from(data.len()).map_err(|_| registry_error(path, name, ERROR_MORE_DATA))?;
        let buffer = (!data.is_empty()).then_some(data.as_mut_ptr());
        // SAFETY: `wide_name` is NUL-terminated and outlives the call; `buffer` is either absent
        // (size query) or `len` writable bytes; `ty` and `len` are valid out pointers.
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(wide_name.as_ptr()),
                None,
                Some(&mut ty),
                buffer,
                Some(&mut len),
            )
        };
        match status {
            ERROR_SUCCESS if buffer.is_some() || len == 0 => {
                data.truncate(len as usize);
                return Ok(Some(RawValue { ty: ty.0, data }));
            }
            // The size query, or the value grew since.
            ERROR_SUCCESS | ERROR_MORE_DATA => data = vec![0; len as usize],
            ERROR_FILE_NOT_FOUND => return Ok(None),
            other => return Err(registry_error(path, name, other)),
        }
    }
    Err(registry_error(path, name, ERROR_MORE_DATA))
}

/// Largest value name (in UTF-16 units, without the NUL) and value data (bytes) of a key.
fn key_maxima(key: HKEY, path: &str) -> Result<(usize, usize), Error> {
    let mut name_len = 0u32;
    let mut data_len = 0u32;
    // SAFETY: `key` is an open key; only the two maxima are requested through valid pointers.
    let status = unsafe {
        RegQueryInfoKeyW(
            key,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut name_len),
            Some(&mut data_len),
            None,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(Error::Registry {
            path: path.to_string(),
            code: status.0,
        });
    }
    Ok((name_len as usize, data_len as usize))
}

/// Every value of a key, in registry order. Names must be valid UTF-16. `path` labels errors.
pub(crate) fn enum_values(key: HKEY, path: &str) -> Result<Vec<(String, RawValue)>, Error> {
    let (mut name_cap, mut data_cap) = key_maxima(key, path)?;
    let mut values = Vec::new();
    let mut index = 0u32;
    let mut attempts = 0usize;
    loop {
        let mut name = vec![0u16; name_cap + 1];
        let mut data = vec![0u8; data_cap.max(1)];
        let mut name_len = u32::try_from(name.len()).unwrap_or(u32::MAX);
        let mut data_len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let mut ty = 0u32;
        // SAFETY: `name` has room for `name_len` units and `data` for `data_len` bytes; every
        // other pointer is a valid out pointer.
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut ty),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        match status {
            ERROR_SUCCESS => {
                let units = name.get(..name_len as usize).unwrap_or(&name);
                let text = String::from_utf16(units).map_err(|_| Error::UnexpectedData {
                    path: format!(r"{path}\{}", String::from_utf16_lossy(units)),
                })?;
                data.truncate(data_len as usize);
                values.push((text, RawValue { ty, data }));
                index += 1;
                attempts = 0;
            }
            ERROR_NO_MORE_ITEMS => return Ok(values),
            ERROR_MORE_DATA if attempts < MAX_ATTEMPTS => {
                // A value (or its name) grew since the maxima were read: retry the same index
                // with the longest name the registry allows and the size it asked for.
                attempts += 1;
                let (_, datas) = key_maxima(key, path)?;
                name_cap = MAX_VALUE_NAME;
                data_cap = datas.max(data_cap).max(data_len as usize);
            }
            other => {
                return Err(Error::Registry {
                    path: path.to_string(),
                    code: other.0,
                });
            }
        }
    }
}

/// One `RegSetValueExW`.
pub(crate) fn set_value(
    key: HKEY,
    path: &str,
    name: &str,
    ty: u32,
    data: &[u8],
) -> Result<(), Error> {
    let wide_name = to_wide(name);
    // SAFETY: `wide_name` is NUL-terminated and `data` is a readable slice; both outlive the call.
    let status = unsafe {
        RegSetValueExW(
            key,
            PCWSTR(wide_name.as_ptr()),
            None,
            REG_VALUE_TYPE(ty),
            Some(data),
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(registry_error(path, name, status))
    }
}

/// One `RegDeleteValueW`; a value that does not exist counts as deleted.
pub(crate) fn delete_value(key: HKEY, path: &str, name: &str) -> Result<(), Error> {
    let wide_name = to_wide(name);
    // SAFETY: `wide_name` is NUL-terminated and outlives the call.
    let status = unsafe { RegDeleteValueW(key, PCWSTR(wide_name.as_ptr())) };
    match status {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        other => Err(registry_error(path, name, other)),
    }
}

/// `RegFlushKey`: everything written to the key's hive so far becomes durable.
pub(crate) fn flush(key: HKEY, path: &str) -> Result<(), Error> {
    // SAFETY: `key` is an open key.
    let status = unsafe { RegFlushKey(key) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(Error::Registry {
            path: path.to_string(),
            code: status.0,
        })
    }
}

/// Text of a `REG_SZ` payload: valid UTF-16, at most one terminating NUL and none inside. `None`
/// for anything else, which then is not round-trippable as text.
pub(crate) fn decode_sz(data: &[u8]) -> Option<String> {
    let (pairs, rest) = data.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    let mut units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    if units.last() == Some(&0) {
        units.pop();
    }
    if units.contains(&0) {
        return None;
    }
    String::from_utf16(&units).ok()
}

/// `REG_SZ` payload of `text`, with its terminating NUL. `None` when `text` contains a NUL.
pub(crate) fn encode_sz(text: &str) -> Option<Vec<u8>> {
    if text.contains('\0') {
        return None;
    }
    Some(
        text.encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect(),
    )
}

/// How MKLM records a stored value: a 4-byte `REG_DWORD` and a clean `REG_SZ` map to their
/// variants; anything else is kept byte for byte as [`RegValue::Other`].
pub(crate) fn to_reg_value(raw: RawValue) -> RegValue {
    match raw.ty {
        ty if ty == REG_DWORD.0 && raw.data.len() == 4 => {
            let bytes = [raw.data[0], raw.data[1], raw.data[2], raw.data[3]];
            RegValue::Dword {
                value: u32::from_le_bytes(bytes),
            }
        }
        ty if ty == REG_SZ.0 => match decode_sz(&raw.data) {
            Some(value) => RegValue::Sz { value },
            None => other(raw),
        },
        _ => other(raw),
    }
}

fn other(raw: RawValue) -> RegValue {
    RegValue::Other {
        reg_type: raw.ty,
        data_hex: raw.data.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

/// Type and bytes to store for `value`; `Ok(None)` for [`RegValue::Absent`] (delete). Fails for
/// text with a NUL or malformed hex.
pub(crate) fn from_reg_value(value: &RegValue) -> Result<Option<(u32, Vec<u8>)>, &'static str> {
    match value {
        RegValue::Absent => Ok(None),
        RegValue::Dword { value } => Ok(Some((REG_DWORD.0, value.to_le_bytes().to_vec()))),
        RegValue::Sz { value } => encode_sz(value)
            .map(|data| Some((REG_SZ.0, data)))
            .ok_or("the text contains a NUL character"),
        RegValue::Other { reg_type, data_hex } => decode_hex(data_hex)
            .map(|data| Some((*reg_type, data)))
            .ok_or("the recorded data is not hexadecimal"),
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::{REG_BINARY, REG_EXPAND_SZ};

    fn raw(ty: u32, data: &[u8]) -> RawValue {
        RawValue {
            ty,
            data: data.to_vec(),
        }
    }

    fn sz(text: &str) -> Vec<u8> {
        encode_sz(text).unwrap_or_default()
    }

    #[test]
    fn dwords_and_strings_map_to_their_variants() {
        assert_eq!(
            to_reg_value(raw(REG_DWORD.0, &[7, 0, 0, 0])),
            RegValue::Dword { value: 7 }
        );
        assert_eq!(
            to_reg_value(raw(REG_SZ.0, &sz("kbd106.dll"))),
            RegValue::Sz {
                value: "kbd106.dll".to_string()
            }
        );
        // Without the terminating NUL, still the same text.
        let mut unterminated = sz("PCAT_106KEY");
        unterminated.truncate(unterminated.len() - 2);
        assert_eq!(
            to_reg_value(raw(REG_SZ.0, &unterminated)),
            RegValue::Sz {
                value: "PCAT_106KEY".to_string()
            }
        );
        assert_eq!(
            to_reg_value(raw(REG_SZ.0, &[0, 0])),
            RegValue::Sz {
                value: String::new()
            }
        );
    }

    #[test]
    fn everything_else_is_kept_byte_for_byte() {
        // A REG_SZ "4" in a DWORD value, a short DWORD, a REG_EXPAND_SZ, text with data after
        // the NUL, an odd length, binary.
        for (ty, data) in [
            (REG_DWORD.0, &[4u8, 0, 0][..]),
            (REG_DWORD.0, &[4, 0, 0, 0, 0, 0, 0, 0][..]),
            (REG_EXPAND_SZ.0, &sz("kbd106.dll")[..]),
            (REG_SZ.0, &[b'a', 0, 0, 0, b'b', 0, 0, 0][..]),
            (REG_SZ.0, &[b'a', 0, b'b'][..]),
            (REG_BINARY.0, &[0xde, 0xad, 0xbe, 0xef][..]),
            (11, &[][..]),
        ] {
            let value = to_reg_value(raw(ty, data));
            let RegValue::Other { reg_type, data_hex } = &value else {
                panic!("{value:?} should be Other");
            };
            assert_eq!(*reg_type, ty);
            assert_eq!(data_hex.len(), data.len() * 2);
            assert_eq!(from_reg_value(&value), Ok(Some((ty, data.to_vec()))));
        }
        assert_eq!(
            to_reg_value(raw(REG_BINARY.0, &[0xde, 0xad, 0x0b])),
            RegValue::Other {
                reg_type: REG_BINARY.0,
                data_hex: "dead0b".to_string()
            }
        );
    }

    #[test]
    fn values_encode_for_writing() {
        assert_eq!(from_reg_value(&RegValue::Absent), Ok(None));
        assert_eq!(
            from_reg_value(&RegValue::Dword { value: 0x51 }),
            Ok(Some((REG_DWORD.0, vec![0x51, 0, 0, 0])))
        );
        assert_eq!(
            from_reg_value(&RegValue::Sz {
                value: "ab".to_string()
            }),
            Ok(Some((REG_SZ.0, vec![b'a', 0, b'b', 0, 0, 0])))
        );
        assert!(
            from_reg_value(&RegValue::Sz {
                value: "a\0b".to_string()
            })
            .is_err()
        );
        for bad in ["abc", "zz", "0x12", "é1"] {
            assert!(
                from_reg_value(&RegValue::Other {
                    reg_type: 3,
                    data_hex: bad.to_string()
                })
                .is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            from_reg_value(&RegValue::Other {
                reg_type: 3,
                data_hex: "00FFa0".to_string()
            }),
            Ok(Some((3, vec![0, 0xff, 0xa0])))
        );
    }

    #[test]
    fn text_round_trips() {
        for text in ["", "kbd106.dll", "日本語", "HID\\VID_3434&PID_D027"] {
            assert_eq!(decode_sz(&sz(text)).as_deref(), Some(text));
        }
        // Unpaired surrogate.
        assert_eq!(decode_sz(&[0x00, 0xd8, 0, 0]), None);
    }

    #[test]
    fn enumerates_and_reads_a_real_key() {
        // Read-only: HKLM\SYSTEM\CurrentControlSet\Control\Session Manager exists on every
        // Windows and has values.
        let key = windows_registry::LOCAL_MACHINE
            .open(r"SYSTEM\CurrentControlSet\Control\Session Manager")
            .expect("Session Manager key");
        let values = enum_values(HKEY(key.as_raw()), "Session Manager").expect("enumerate");
        assert!(!values.is_empty());
        let (name, value) = &values[0];
        assert_eq!(
            query_value(HKEY(key.as_raw()), "Session Manager", name).expect("query"),
            Some(value.clone())
        );
        assert_eq!(
            query_value(HKEY(key.as_raw()), "Session Manager", "MKLM no such value")
                .expect("query"),
            None
        );
    }
}
