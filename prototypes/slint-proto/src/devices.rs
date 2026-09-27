//! Read-only lookups that turn a Raw Input device interface path into something recognizable.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Device_Interface_PropertyW, CM_LOCATE_DEVNODE_NORMAL,
    CM_Locate_DevNodeW, CR_BUFFER_SMALL, CR_SUCCESS, MAX_DEVICE_ID_LEN,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_InstanceId, DEVPROP_TYPE_STRING, DEVPROPTYPE,
};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::UI::Input::{
    GetRawInputDeviceInfoW, GetRawInputDeviceList, RAWINPUTDEVICELIST, RID_DEVICE_INFO,
    RIDI_DEVICEINFO, RIDI_DEVICENAME, RIM_TYPEKEYBOARD,
};
use windows::core::PCWSTR;

/// What the prototype shows for one keyboard seen through winit's `DeviceEvent::Key`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyboardInfo {
    /// Device interface path from `DeviceIdExtWindows::persistent_identifier()`.
    pub path: String,
    /// Devnode instance ID resolved from the interface path.
    pub instance_id: String,
    /// `RIDI_DEVICEINFO` summary (type/subtype/keys).
    pub raw_info: String,
}

impl KeyboardInfo {
    pub fn from_path(path: String) -> Self {
        let instance_id =
            interface_instance_id(&path).unwrap_or_else(|| "（取得できません）".to_owned());
        let raw_info = raw_keyboard_info(&path)
            .unwrap_or_else(|| "（Raw Input の一覧に見つかりません）".to_owned());
        Self {
            path,
            instance_id,
            raw_info,
        }
    }

    /// Short label: the instance ID when known, otherwise the path.
    pub fn label(&self) -> &str {
        if self.instance_id.starts_with('（') {
            &self.path
        } else {
            &self.instance_id
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Resolves `DEVPKEY_Device_InstanceId` of a device interface (cfgmgr32, read-only).
pub fn interface_instance_id(path: &str) -> Option<String> {
    let wide = to_wide(path);
    let mut prop_type = DEVPROPTYPE(0);
    let mut size = 0u32;
    // SAFETY: `wide` is NUL-terminated and outlives the call; a null buffer with size 0 queries the size.
    let cr = unsafe {
        CM_Get_Device_Interface_PropertyW(
            PCWSTR(wide.as_ptr()),
            &DEVPKEY_Device_InstanceId,
            &mut prop_type,
            None,
            &mut size,
            0,
        )
    };
    if cr != CR_BUFFER_SMALL || size == 0 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: `buf` holds at least `size` bytes and stays alive for the call.
    let cr = unsafe {
        CM_Get_Device_Interface_PropertyW(
            PCWSTR(wide.as_ptr()),
            &DEVPKEY_Device_InstanceId,
            &mut prop_type,
            Some(buf.as_mut_ptr().cast()),
            &mut size,
            0,
        )
    };
    if cr != CR_SUCCESS || prop_type != DEVPROP_TYPE_STRING {
        return None;
    }
    let id = from_wide(&buf);
    // The interface property keeps the casing of the interface path (e.g. `Col01`, lowercase hex);
    // round-trip through the devnode to get the canonical instance ID.
    Some(canonical_instance_id(&id).unwrap_or(id))
}

/// Returns the canonical (as stored by PnP) instance ID for a case-insensitive instance ID.
fn canonical_instance_id(id: &str) -> Option<String> {
    let wide = to_wide(id);
    let mut devinst = 0u32;
    // SAFETY: `wide` is NUL-terminated; `devinst` is valid for writes.
    let cr = unsafe {
        CM_Locate_DevNodeW(
            &mut devinst,
            PCWSTR(wide.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    };
    if cr != CR_SUCCESS {
        return None;
    }
    let mut buf = vec![0u16; MAX_DEVICE_ID_LEN as usize + 1];
    // SAFETY: `devinst` was returned by CM_Locate_DevNodeW; `buf` is writable.
    let cr = unsafe { CM_Get_Device_IDW(devinst, &mut buf, 0) };
    (cr == CR_SUCCESS).then(|| from_wide(&buf))
}

fn raw_device_name(handle: HANDLE) -> Option<String> {
    let mut chars = 0u32;
    // SAFETY: a null data pointer asks for the required size in characters.
    let status = unsafe { GetRawInputDeviceInfoW(Some(handle), RIDI_DEVICENAME, None, &mut chars) };
    if status != 0 || chars == 0 {
        return None;
    }
    let mut buf = vec![0u16; chars as usize];
    // SAFETY: `buf` holds `chars` UTF-16 units as reported by the previous call.
    let status = unsafe {
        GetRawInputDeviceInfoW(
            Some(handle),
            RIDI_DEVICENAME,
            Some(buf.as_mut_ptr().cast()),
            &mut chars,
        )
    };
    (status != u32::MAX && status != 0).then(|| from_wide(&buf))
}

fn raw_device_list() -> Vec<RAWINPUTDEVICELIST> {
    let item = size_of::<RAWINPUTDEVICELIST>() as u32;
    for _ in 0..3 {
        let mut count = 0u32;
        // SAFETY: a null list pointer asks for the device count.
        if unsafe { GetRawInputDeviceList(None, &mut count, item) } == u32::MAX {
            return Vec::new();
        }
        let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
        // SAFETY: `list` has room for `count` entries of `item` bytes each.
        let got = unsafe { GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut count, item) };
        if got != u32::MAX {
            list.truncate(got as usize);
            return list;
        }
        // The list grew between the two calls (hot-plug); retry.
    }
    Vec::new()
}

/// All keyboards currently in the Raw Input list (for `--list-keyboards`).
pub fn raw_keyboards() -> Vec<KeyboardInfo> {
    raw_device_list()
        .into_iter()
        .filter(|d| d.dwType == RIM_TYPEKEYBOARD)
        .filter_map(|d| raw_device_name(d.hDevice))
        .map(KeyboardInfo::from_path)
        .collect()
}

/// Finds the keyboard with this interface path in the Raw Input list and summarizes `RIDI_DEVICEINFO`.
pub fn raw_keyboard_info(path: &str) -> Option<String> {
    let dev = raw_device_list()
        .into_iter()
        .filter(|d| d.dwType == RIM_TYPEKEYBOARD)
        .find(|d| raw_device_name(d.hDevice).is_some_and(|n| n.eq_ignore_ascii_case(path)))?;
    let mut info = RID_DEVICE_INFO {
        cbSize: size_of::<RID_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    let mut size = info.cbSize;
    // SAFETY: `info` is a properly sized RID_DEVICE_INFO with cbSize set.
    let status = unsafe {
        GetRawInputDeviceInfoW(
            Some(dev.hDevice),
            RIDI_DEVICEINFO,
            Some((&raw mut info).cast()),
            &mut size,
        )
    };
    if status == u32::MAX || status == 0 || info.dwType != RIM_TYPEKEYBOARD {
        return None;
    }
    // SAFETY: dwType == RIM_TYPEKEYBOARD selects the keyboard union member.
    let kbd = unsafe { info.Anonymous.keyboard };
    Some(format!(
        "type=0x{:X} subtype=0x{:X} keys={} fkeys={}",
        kbd.dwType, kbd.dwSubType, kbd.dwNumberOfKeysTotal, kbd.dwNumberOfFunctionKeys
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_round_trip() {
        let w = to_wide("HID#VID_3434");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(from_wide(&w), "HID#VID_3434");
    }

    #[test]
    fn unknown_path_is_none() {
        assert_eq!(
            interface_instance_id(r"\\?\HID#NOPE#0#{00000000-0000-0000-0000-000000000000}"),
            None
        );
        assert_eq!(raw_keyboard_info("nope"), None);
    }
}
