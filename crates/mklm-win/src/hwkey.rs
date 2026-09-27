//! The devnode's hardware key ("Device Parameters"), opened read-only through CfgMgr32.
//!
//! Microsoft forbids opening `Enum\...` paths directly, so the key is always reached with
//! `CM_Open_DevNode_Key(CM_REGISTRY_HARDWARE)`. This module never asks for write access and never
//! creates the key.

use mklm_core::{DeviceOverrides, value_names};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Open_DevNode_Key, CM_REGISTRY_HARDWARE, CR_NO_SUCH_REGISTRY_KEY, CR_SUCCESS,
    RegDisposition_OpenExisting,
};
use windows::Win32::System::Registry::{HKEY, KEY_READ};
use windows_registry::Key;

use crate::error::{Error, ReadIssue, ReadIssueKind};
use crate::props::{DevNode, config_ret};
use crate::reg::read_dword;

/// Opens the devnode's "Device Parameters" key with `KEY_READ`. `Ok(None)` when it does not exist.
pub(crate) fn open_hardware_key(node: DevNode) -> Result<Option<Key>, Error> {
    let mut hkey = HKEY(std::ptr::null_mut());
    // SAFETY: `hkey` is a valid out pointer. KEY_READ with RegDisposition_OpenExisting only opens
    // an existing key; nothing is created or written.
    let cr = unsafe {
        CM_Open_DevNode_Key(
            node.raw(),
            KEY_READ.0,
            0,
            RegDisposition_OpenExisting,
            &mut hkey,
            CM_REGISTRY_HARDWARE,
        )
    };
    match cr {
        // SAFETY: on success the API returned a registry handle that this process owns and that
        // must be closed with RegCloseKey, which `Key` does on drop.
        CR_SUCCESS if !hkey.0.is_null() => Ok(Some(unsafe { Key::from_raw(hkey.0) })),
        CR_NO_SUCH_REGISTRY_KEY => Ok(None),
        other => Err(config_ret("CM_Open_DevNode_Key", other)),
    }
}

/// Reads the seven override values of `instance_id`'s "Device Parameters".
///
/// A missing key or value is `None`. Values that cannot be read are also `None`, with an issue.
pub(crate) fn read_overrides(
    node: DevNode,
    instance_id: &str,
    issues: &mut Vec<ReadIssue>,
) -> DeviceOverrides {
    let key_path = format!(r"{instance_id}\Device Parameters");
    let key = match open_hardware_key(node) {
        Ok(Some(key)) => key,
        Ok(None) => return DeviceOverrides::default(),
        Err(error) => {
            issues.push(ReadIssue::new(ReadIssueKind::Values, key_path, error));
            return DeviceOverrides::default();
        }
    };
    let mut read = |name: &str| {
        read_dword(&key, &key_path, name).unwrap_or_else(|error| {
            issues.push(ReadIssue::new(
                ReadIssueKind::Values,
                format!(r"{key_path}\{name}"),
                error,
            ));
            None
        })
    };
    DeviceOverrides {
        keyboard_type_override: read(value_names::HID_TYPE),
        keyboard_subtype_override: read(value_names::HID_SUBTYPE),
        override_keyboard_type: read(value_names::PS2_TYPE),
        override_keyboard_subtype: read(value_names::PS2_SUBTYPE),
        number_total_keys_override: read(value_names::HID_TOTAL_KEYS),
        number_function_keys_override: read(value_names::HID_FUNCTION_KEYS),
        number_indicators_override: read(value_names::HID_INDICATORS),
    }
}
