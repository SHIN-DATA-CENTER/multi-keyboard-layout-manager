//! Keyboard type/subtype as the drivers report it right now (Raw Input).
//!
//! M0 showed that `RID_DEVICE_INFO_KEYBOARD.dwType/dwSubType` follow the override values once the
//! driver has re-read them, so this is how MKLM confirms that a change took effect.

use mklm_core::KeyboardType;
use windows::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_DATA, GetLastError, HANDLE,
};
use windows::Win32::UI::Input::{
    GetRawInputDeviceInfoW, GetRawInputDeviceList, RAWINPUTDEVICELIST, RID_DEVICE_INFO,
    RIDI_DEVICEINFO, RIDI_DEVICENAME, RIM_TYPEKEYBOARD,
};

use crate::error::{Error, ReadIssue, ReadIssueKind};
use crate::props::{from_wide, interface_instance_id};

/// Attempts while the device list keeps growing between the size query and the read.
const MAX_ATTEMPTS: usize = 8;

/// A keyboard interface known to Raw Input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawKeyboard {
    /// Device interface path (`RIDI_DEVICENAME`), normalised to the `\\?\` prefix.
    pub interface_path: String,
    /// Instance ID of the devnode behind the interface, when it could be resolved.
    pub instance_id: Option<String>,
    /// `dwType` / `dwSubType` from `RIDI_DEVICEINFO`.
    pub keyboard_type: KeyboardType,
}

/// Lists every keyboard Raw Input knows in this session. A device that cannot be queried (for
/// example because it vanished while the list was read) is skipped with an issue.
pub fn raw_keyboards(issues: &mut Vec<ReadIssue>) -> Result<Vec<RawKeyboard>, Error> {
    let mut keyboards = Vec::new();
    for entry in device_list()? {
        if entry.dwType != RIM_TYPEKEYBOARD {
            continue;
        }
        let interface_path = match device_name(entry.hDevice) {
            Ok(name) => normalize_interface_path(&name),
            Err(error) => {
                issues.push(ReadIssue::new(
                    ReadIssueKind::RawInput,
                    format!("Raw Input device {:?}", entry.hDevice.0),
                    error,
                ));
                continue;
            }
        };
        match keyboard_type(entry.hDevice) {
            Ok(keyboard_type) => keyboards.push(RawKeyboard {
                instance_id: instance_id_from_interface_path(&interface_path),
                interface_path,
                keyboard_type,
            }),
            Err(error) => issues.push(ReadIssue::new(
                ReadIssueKind::RawInput,
                interface_path,
                error,
            )),
        }
    }
    Ok(keyboards)
}

/// Instance ID behind a keyboard interface path (as Raw Input or winit report it).
///
/// Asks CfgMgr32 first; if the interface is unknown there or cannot be read, rebuilds the ID from
/// the path (`\\?\HID#VID_3434&PID_D027&MI_00&Col01#8&148ad7e3&0&0000#{GUID}` →
/// `HID\VID_3434&PID_D027&MI_00&Col01\8&148ad7e3&0&0000`, case as in the path).
pub fn instance_id_from_interface_path(interface_path: &str) -> Option<String> {
    let path = normalize_interface_path(interface_path);
    interface_instance_id(&path)
        .ok()
        .flatten()
        .or_else(|| instance_id_from_path_text(&path))
}

/// Rewrites the NT-namespace prefix `\??\` (seen in some Raw Input names) to `\\?\`.
pub(crate) fn normalize_interface_path(path: &str) -> String {
    match path.strip_prefix(r"\??\") {
        Some(rest) => format!(r"\\?\{rest}"),
        None => path.to_string(),
    }
}

/// Rebuilds `ENUM\DEVICE\INSTANCE` from `\\?\ENUM#DEVICE#INSTANCE#{interface GUID}[\ref]`.
pub(crate) fn instance_id_from_path_text(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\??\"))?;
    let mut parts = rest.split('#');
    let (enumerator, device, instance) = (parts.next()?, parts.next()?, parts.next()?);
    let class = parts.next()?;
    if enumerator.is_empty() || device.is_empty() || instance.is_empty() || !class.starts_with('{')
    {
        return None;
    }
    Some(format!(r"{enumerator}\{device}\{instance}"))
}

fn device_list() -> Result<Vec<RAWINPUTDEVICELIST>, Error> {
    let entry_size = size_of::<RAWINPUTDEVICELIST>() as u32;
    for _ in 0..MAX_ATTEMPTS {
        let mut count = 0u32;
        // SAFETY: without a buffer the call only stores the device count in `count`.
        if unsafe { GetRawInputDeviceList(None, &mut count, entry_size) } == u32::MAX {
            return Err(last_error("GetRawInputDeviceList"));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
        let mut capacity = count;
        // SAFETY: `list` holds `capacity` entries of `entry_size` bytes each.
        let written =
            unsafe { GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut capacity, entry_size) };
        if written == u32::MAX {
            // SAFETY: GetLastError has no preconditions.
            if unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            return Err(last_error("GetRawInputDeviceList"));
        }
        list.truncate(written as usize);
        return Ok(list);
    }
    Err(Error::Win32 {
        function: "GetRawInputDeviceList",
        code: ERROR_INSUFFICIENT_BUFFER.0,
    })
}

/// `RIDI_DEVICENAME` (sizes are in characters for this command).
fn device_name(device: HANDLE) -> Result<String, Error> {
    let mut chars = 0u32;
    // SAFETY: without a buffer the call only stores the required length in `chars`.
    let result = unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_DEVICENAME, None, &mut chars) };
    if result != 0 || chars == 0 {
        return Err(last_error("GetRawInputDeviceInfoW"));
    }
    let mut name = vec![0u16; chars as usize + 1];
    let mut capacity = name.len() as u32;
    // SAFETY: `name` holds `capacity` UTF-16 units, the unit RIDI_DEVICENAME sizes are given in.
    let copied = unsafe {
        GetRawInputDeviceInfoW(
            Some(device),
            RIDI_DEVICENAME,
            Some(name.as_mut_ptr().cast()),
            &mut capacity,
        )
    };
    if copied == 0 || copied == u32::MAX {
        return Err(last_error("GetRawInputDeviceInfoW"));
    }
    Some(from_wide(&name))
        .filter(|name| !name.is_empty())
        .ok_or(Error::Win32 {
            function: "GetRawInputDeviceInfoW",
            code: ERROR_INVALID_DATA.0,
        })
}

/// `RIDI_DEVICEINFO` of a keyboard.
fn keyboard_type(device: HANDLE) -> Result<KeyboardType, Error> {
    let mut info = RID_DEVICE_INFO {
        cbSize: size_of::<RID_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    let mut size = info.cbSize;
    // SAFETY: `info` is a writable RID_DEVICE_INFO of `size` bytes with `cbSize` set, as the API
    // requires.
    let copied = unsafe {
        GetRawInputDeviceInfoW(
            Some(device),
            RIDI_DEVICEINFO,
            Some((&raw mut info).cast()),
            &mut size,
        )
    };
    if copied == 0 || copied == u32::MAX {
        return Err(last_error("GetRawInputDeviceInfoW"));
    }
    if info.dwType != RIM_TYPEKEYBOARD {
        return Err(Error::Win32 {
            function: "GetRawInputDeviceInfoW",
            code: ERROR_INVALID_DATA.0,
        });
    }
    // SAFETY: `dwType` is RIM_TYPEKEYBOARD, so the keyboard member of the union was written.
    let keyboard = unsafe { info.Anonymous.keyboard };
    Ok(KeyboardType::new(keyboard.dwType, keyboard.dwSubType))
}

fn last_error(function: &'static str) -> Error {
    // SAFETY: GetLastError has no preconditions.
    let code = unsafe { GetLastError() };
    Error::Win32 {
        function,
        code: code.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_instance_id() {
        assert_eq!(
            instance_id_from_path_text(
                r"\\?\HID#VID_3434&PID_D027&MI_00&Col01#8&148ad7e3&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}"
            )
            .as_deref(),
            Some(r"HID\VID_3434&PID_D027&MI_00&Col01\8&148ad7e3&0&0000")
        );
        assert_eq!(
            instance_id_from_path_text(
                r"\??\ACPI#FUJ0309#4&320db4c2&0#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}"
            )
            .as_deref(),
            Some(r"ACPI\FUJ0309\4&320db4c2&0")
        );
        assert_eq!(
            instance_id_from_path_text(
                r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&0225a7_PID&fa6c_REV&0300_f977e93ba63f&Col02#b&b4852a&0&0001#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}\KBD"
            )
            .as_deref(),
            Some(
                r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&0225a7_PID&fa6c_REV&0300_f977e93ba63f&Col02\b&b4852a&0&0001"
            )
        );
        assert_eq!(instance_id_from_path_text(r"\\?\HID#VID_1#2"), None);
        assert_eq!(instance_id_from_path_text(r"HID#VID_1#2#{x}"), None);
        assert_eq!(instance_id_from_path_text(r"\\?\HID##2#{x}"), None);
        assert_eq!(instance_id_from_path_text(""), None);
    }

    #[test]
    fn nt_prefix_is_normalised() {
        assert_eq!(
            normalize_interface_path(r"\??\HID#A#B#{C}"),
            r"\\?\HID#A#B#{C}"
        );
        assert_eq!(
            normalize_interface_path(r"\\?\HID#A#B#{C}"),
            r"\\?\HID#A#B#{C}"
        );
    }
}
