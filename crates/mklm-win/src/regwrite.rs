//! Write access to the two kinds of system keys MKLM changes (M2): a keyboard's hardware key
//! ("Device Parameters") and `Services\i8042prt\Parameters`. Used only by the engine inside the
//! elevated helper or the elevated CLI.
//!
//! - Device keys are opened only through `CM_Locate_DevNodeW(CM_LOCATE_DEVNODE_PHANTOM)` +
//!   `CM_Open_DevNode_Key(KEY_QUERY_VALUE | KEY_SET_VALUE, 0, RegDisposition_OpenAlways, …,
//!   CM_REGISTRY_HARDWARE)`, never through an `Enum\…` path (plan 1.2), and only after checking
//!   that the devnode's class is Keyboard.
//! - The global key is opened with `KEY_QUERY_VALUE | KEY_SET_VALUE` and never created.
//! - [`WritableKey::write`] accepts only the value names MKLM writes on that kind of key
//!   (`mklm_core::DEVICE_VALUE_NAMES` / `GLOBAL_VALUE_NAMES`) and refuses any other with
//!   [`Error::ValueNotAllowed`] (design review S9). Checking the *values* stays the engine's job
//!   (`check_plan`, `plan_restore`); this is the last line of defence under it, so that no missed
//!   check can touch `Start` or another driver setting in the SYSTEM hive.
//! - A devnode that no longer exists (not even as a phantom) is reported as a `CONFIGRET`
//!   `CR_NO_SUCH_DEVNODE` error, which `mklm_engine::win` maps to `BackendError::DeviceRemoved`,
//!   never to "value absent" (design review C3).

use mklm_core::{DEVICE_VALUE_NAMES, GLOBAL_VALUE_NAMES, RegValue};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Open_DevNode_Key, CM_REGISTRY_HARDWARE, CR_FAILURE, CR_SUCCESS, RegDisposition_OpenAlways,
};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_ClassGuid, DEVPKEY_Device_InstanceId};
use windows::Win32::System::Registry::{HKEY, KEY_QUERY_VALUE, KEY_SET_VALUE};
use windows_registry::{Key, LOCAL_MACHINE};

use crate::devices::KEYBOARD_CLASS_GUID;
use crate::error::Error;
use crate::global::I8042PRT_PARAMETERS;
use crate::props::{DevNode, config_ret};
use crate::regraw;

/// A registry key opened for reading and writing values.
#[derive(Debug)]
pub struct WritableKey {
    key: Key,
    /// For error messages, e.g. `HKLM\…\Enum\HID\…\Device Parameters`.
    path: String,
    /// Value names [`WritableKey::write`] accepts: `mklm_core::DEVICE_VALUE_NAMES` for a device
    /// key, `mklm_core::GLOBAL_VALUE_NAMES` for the global key.
    allowed_names: &'static [&'static str],
}

impl WritableKey {
    pub fn path(&self) -> &str {
        &self.path
    }

    fn hkey(&self) -> HKEY {
        HKEY(self.key.as_raw())
    }

    /// Reads one value as it is: `REG_DWORD` (4 bytes) and `REG_SZ` map to their variants, anything
    /// else to [`RegValue::Other`]; a missing value is [`RegValue::Absent`].
    pub fn read(&self, name: &str) -> Result<RegValue, Error> {
        Ok(regraw::query_value(self.hkey(), &self.path, name)?
            .map_or(RegValue::Absent, regraw::to_reg_value))
    }

    /// Every value of the key (`RegEnumValueW`).
    pub fn list(&self) -> Result<Vec<(String, RegValue)>, Error> {
        Ok(regraw::enum_values(self.hkey(), &self.path)?
            .into_iter()
            .map(|(name, raw)| (name, regraw::to_reg_value(raw)))
            .collect())
    }

    /// `RegSetValueExW`, or `RegDeleteValueW` for [`RegValue::Absent`] (a missing value is fine).
    /// Any name outside `allowed_names` fails with [`Error::ValueNotAllowed`] before the call.
    pub fn write(&self, name: &str, value: &RegValue) -> Result<(), Error> {
        // Registry value names compare case-insensitively; write the canonical spelling.
        let Some(name) = self
            .allowed_names
            .iter()
            .copied()
            .find(|allowed| allowed.eq_ignore_ascii_case(name))
        else {
            return Err(Error::ValueNotAllowed {
                path: self.path.clone(),
                name: name.to_string(),
            });
        };
        match regraw::from_reg_value(value) {
            Ok(None) => regraw::delete_value(self.hkey(), &self.path, name),
            Ok(Some((ty, data))) => regraw::set_value(self.hkey(), &self.path, name, ty, &data),
            Err(_) => Err(Error::UnexpectedData {
                path: format!(r"{}\{name}", self.path),
            }),
        }
    }

    /// `RegFlushKey`: flushes the whole hive the key belongs to.
    pub fn flush(&self) -> Result<(), Error> {
        regraw::flush(self.hkey(), &self.path)
    }
}

/// Opens a Keyboard-class devnode's hardware key for writing, creating "Device Parameters" if it
/// does not exist yet (works for phantom devnodes too). Fails with [`Error::NotKeyboard`] for any
/// other class.
pub fn open_device_key_rw(instance_id: &str) -> Result<WritableKey, Error> {
    let node = DevNode::locate(instance_id)?;
    check_keyboard(node, instance_id)?;
    let mut hkey = HKEY::default();
    // SAFETY: `hkey` is a valid out pointer. The key is opened (or created as the devnode's own
    // hardware key) with query and set-value rights only, which is what MKLM writes with.
    let cr = unsafe {
        CM_Open_DevNode_Key(
            node.raw(),
            (KEY_QUERY_VALUE | KEY_SET_VALUE).0,
            0,
            RegDisposition_OpenAlways,
            &mut hkey,
            CM_REGISTRY_HARDWARE,
        )
    };
    if cr != CR_SUCCESS {
        return Err(config_ret("CM_Open_DevNode_Key", cr));
    }
    if hkey.is_invalid() {
        return Err(config_ret("CM_Open_DevNode_Key", CR_FAILURE));
    }
    Ok(WritableKey {
        // SAFETY: on success the API returned a registry handle that this process owns and that
        // must be closed with RegCloseKey, which `Key` does on drop.
        key: unsafe { Key::from_raw(hkey.0) },
        path: format!(r"HKLM\SYSTEM\CurrentControlSet\Enum\{instance_id}\Device Parameters"),
        allowed_names: &DEVICE_VALUE_NAMES,
    })
}

/// The devnode must be the one `instance_id` names (not another one it resolved to) and of the
/// Keyboard setup class (plan 1.2, 1.5).
fn check_keyboard(node: DevNode, instance_id: &str) -> Result<(), Error> {
    if let Some(found) = node
        .string(&DEVPKEY_Device_InstanceId)?
        .filter(|found| !found.eq_ignore_ascii_case(instance_id))
    {
        return Err(Error::OtherDevNode { found });
    }
    let is_keyboard = node
        .guid(&DEVPKEY_Device_ClassGuid)?
        .is_some_and(|class| class.eq_ignore_ascii_case(KEYBOARD_CLASS_GUID));
    if is_keyboard {
        Ok(())
    } else {
        Err(Error::NotKeyboard {
            instance_id: instance_id.to_string(),
        })
    }
}

/// Opens `HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters` for writing.
pub fn open_global_key_rw() -> Result<WritableKey, Error> {
    let path = format!(r"HKLM\{I8042PRT_PARAMETERS}");
    let key = LOCAL_MACHINE
        .options()
        .access((KEY_QUERY_VALUE | KEY_SET_VALUE).0)
        .open(I8042PRT_PARAMETERS)
        .map_err(|error| Error::registry(&path, &error))?;
    Ok(WritableKey {
        key,
        path,
        allowed_names: &GLOBAL_VALUE_NAMES,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `WritableKey` over a key opened with `KEY_READ` only. The tests below only pass names the
    /// gate refuses, which return before any registry call; and even a call that got through could
    /// not write through this handle. Real writes are covered by the machine tests (design H.2).
    fn read_only(path: &str, allowed_names: &'static [&'static str]) -> Option<WritableKey> {
        let key = LOCAL_MACHINE.open(path).ok()?;
        Some(WritableKey {
            key,
            path: format!(r"HKLM\{path}"),
            allowed_names,
        })
    }

    #[test]
    fn write_refuses_names_outside_the_allowlist_before_any_call() {
        let Some(key) = read_only(I8042PRT_PARAMETERS, &GLOBAL_VALUE_NAMES) else {
            return;
        };
        for name in [
            "Start",
            "",
            "KeyboardTypeOverride",
            "LayerDriver KOR",
            "LayerDriver JPN ",
            "Type",
        ] {
            assert_eq!(
                key.write(name, &RegValue::Absent),
                Err(Error::ValueNotAllowed {
                    path: key.path().to_string(),
                    name: name.to_string()
                })
            );
        }
    }

    #[test]
    fn device_keys_accept_only_the_override_names() {
        let Some(key) = read_only(r"SYSTEM\CurrentControlSet\Control", &DEVICE_VALUE_NAMES) else {
            return;
        };
        for name in [
            "LayerDriver JPN",
            "NumberOfFunctionKeysOverride",
            "UpperFilters",
        ] {
            assert!(matches!(
                key.write(name, &RegValue::Absent),
                Err(Error::ValueNotAllowed { .. })
            ));
        }
    }

    #[test]
    fn reads_the_global_key() {
        let Some(key) = read_only(I8042PRT_PARAMETERS, &GLOBAL_VALUE_NAMES) else {
            return;
        };
        let values = key.list().expect("list i8042prt parameters");
        for (name, value) in &values {
            assert_eq!(key.read(name).as_ref(), Ok(value));
        }
        assert_eq!(key.read("MKLM no such value"), Ok(RegValue::Absent));
    }

    #[test]
    fn only_keyboard_class_devnodes_pass_the_class_check() {
        // Read-only property checks; no key is opened.
        if let Ok(node) = DevNode::locate(r"HTREE\ROOT\0") {
            assert!(matches!(
                check_keyboard(node, r"HTREE\ROOT\0"),
                Err(Error::NotKeyboard { .. })
            ));
        }
        let keyboards = crate::keyboard_instance_ids(true).unwrap_or_default();
        for id in keyboards.iter().take(3) {
            if let Ok(node) = DevNode::locate(id) {
                assert_eq!(check_keyboard(node, id), Ok(()), "{id}");
                assert_eq!(check_keyboard(node, &id.to_ascii_lowercase()), Ok(()));
            }
        }
    }
}
