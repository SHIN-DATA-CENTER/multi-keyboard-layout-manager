//! Machine-wide MKLM settings in `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Settings` (plan 3.13;
//! design m3 B.14, WP-E2): today only "restore the keyboards when MKLM is uninstalled", which the
//! MSI's uninstall custom action reads (M5).
//!
//! Anyone may read (the key carries the journal's DACL, `JOURNAL_KEY_SDDL`: Users read). Only the
//! elevated helper writes, under the write lock, on `Request::SetMachineSettings` (design m3
//! A.5). HKLM is opened for writing only here, in `regwrite` and in `journal_store` (design m2 K,
//! extended by m3 J).

use windows::Win32::System::Registry::{KEY_READ, KEY_SET_VALUE, REG_DWORD};
use windows_registry::LOCAL_MACHINE;

use crate::error::Error;
use crate::journal_store::{open_or_create_product_subkey, raw_key};
use crate::reg::{open_read, read_dword};
use crate::regraw;

/// The settings key, relative to `HKEY_LOCAL_MACHINE`.
pub const SETTINGS_KEY: &str = mklm_core::MACHINE_SETTINGS_KEY;

/// `REG_DWORD` 1 or 0. Absent means 1 (the plan's default: restore on uninstall).
pub const RESTORE_ON_UNINSTALL_VALUE: &str = mklm_core::RESTORE_ON_UNINSTALL_VALUE;

/// The machine-wide settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineSettings {
    pub restore_on_uninstall: bool,
}

impl Default for MachineSettings {
    fn default() -> Self {
        Self {
            restore_on_uninstall: true,
        }
    }
}

/// Reads the settings unelevated. A missing key or value gives the default; a value of another
/// type is an error (the GUI then shows the default and a warning).
pub fn read_machine_settings() -> Result<MachineSettings, Error> {
    let Some(key) = open_read(LOCAL_MACHINE, "HKLM", SETTINGS_KEY)? else {
        return Ok(MachineSettings::default());
    };
    let path = format!(r"HKLM\{SETTINGS_KEY}");
    Ok(MachineSettings {
        restore_on_uninstall: read_dword(&key, &path, RESTORE_ON_UNINSTALL_VALUE)?
            .is_none_or(|value| value != 0),
    })
}

/// Writes [`RESTORE_ON_UNINSTALL_VALUE`] (elevated helper only, under the write lock): 1 or 0.
///
/// Creates or opens the key like `journal_store::JournalStore::open_or_create` (`RegCreateKeyExW`
/// with `JOURNAL_KEY_SDDL`, owner and DACL of `SHIN DATA CENTER`, `MKLM` and `Settings` verified,
/// the same `Insecure` rules), then `RegSetValueExW(REG_DWORD)` and `RegFlushKey`: durable once it
/// returns.
pub fn write_restore_on_uninstall(value: bool) -> Result<(), Error> {
    write_setting(RESTORE_ON_UNINSTALL_VALUE, u32::from(value))
}

/// One `REG_DWORD` under [`SETTINGS_KEY`]. The name is checked against the static list
/// `mklm_core::MACHINE_SETTING_NAMES` (exact spelling), as `regwrite` checks the keyboard values
/// (design m2 S9), before any key is opened.
fn write_setting(name: &str, value: u32) -> Result<(), Error> {
    let path = format!(r"HKLM\{SETTINGS_KEY}");
    check_setting_name(&path, name)?;
    let key = open_or_create_product_subkey("Settings", SETTINGS_KEY, KEY_READ | KEY_SET_VALUE)?;
    regraw::set_value(
        raw_key(&key),
        &path,
        name,
        REG_DWORD.0,
        &value.to_le_bytes(),
    )?;
    regraw::flush(raw_key(&key), &path)
}

/// Only the names of `mklm_core::MACHINE_SETTING_NAMES` may be written.
fn check_setting_name(path: &str, name: &str) -> Result<(), Error> {
    if mklm_core::MACHINE_SETTING_NAMES.contains(&name) {
        Ok(())
    } else {
        Err(Error::ValueNotAllowed {
            path: path.to_string(),
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_setting_or_the_default() {
        // Read-only: on a machine without the key this is the default.
        let settings = read_machine_settings().unwrap();
        let _ = settings.restore_on_uninstall;
        assert!(MachineSettings::default().restore_on_uninstall);
    }

    #[test]
    fn only_the_listed_settings_may_be_written() {
        // Checked before any key is opened: nothing here touches the registry.
        let path = format!(r"HKLM\{SETTINGS_KEY}");
        assert_eq!(
            check_setting_name(&path, RESTORE_ON_UNINSTALL_VALUE),
            Ok(())
        );
        for name in ["Start", "restoreonuninstall", "InstallDir", ""] {
            assert!(
                matches!(
                    write_setting(name, 1),
                    Err(Error::ValueNotAllowed { name: refused, .. }) if refused == name
                ),
                "{name}"
            );
        }
        assert_eq!(SETTINGS_KEY, r"SOFTWARE\SHIN DATA CENTER\MKLM\Settings");
        assert_eq!(RESTORE_ON_UNINSTALL_VALUE, "RestoreOnUninstall");
    }
}
