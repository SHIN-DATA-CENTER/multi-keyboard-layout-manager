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

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(unused_variables, dead_code)]

use mklm_core::RegValue;
use windows_registry::Key;

use crate::error::Error;

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

    /// Reads one value as it is: `REG_DWORD` (4 bytes) and `REG_SZ` map to their variants, anything
    /// else to [`RegValue::Other`]; a missing value is [`RegValue::Absent`].
    pub fn read(&self, name: &str) -> Result<RegValue, Error> {
        todo!("M2")
    }

    /// Every value of the key (`RegEnumValueW`).
    pub fn list(&self) -> Result<Vec<(String, RegValue)>, Error> {
        todo!("M2")
    }

    /// `RegSetValueExW`, or `RegDeleteValueW` for [`RegValue::Absent`] (a missing value is fine).
    /// Any name outside `allowed_names` fails with [`Error::ValueNotAllowed`] before the call.
    pub fn write(&self, name: &str, value: &RegValue) -> Result<(), Error> {
        todo!("M2")
    }

    /// `RegFlushKey`: flushes the whole hive the key belongs to.
    pub fn flush(&self) -> Result<(), Error> {
        todo!("M2")
    }
}

/// Opens a Keyboard-class devnode's hardware key for writing, creating "Device Parameters" if it
/// does not exist yet (works for phantom devnodes too). Fails with [`Error::NotKeyboard`] for any
/// other class.
pub fn open_device_key_rw(instance_id: &str) -> Result<WritableKey, Error> {
    todo!("M2")
}

/// Opens `HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters` for writing.
pub fn open_global_key_rw() -> Result<WritableKey, Error> {
    todo!("M2")
}
