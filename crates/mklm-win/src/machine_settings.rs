//! Machine-wide MKLM settings in `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Settings` (plan 3.13;
//! design m3 B.14, WP-E2): today only "restore the keyboards when MKLM is uninstalled", which the
//! MSI's uninstall custom action reads (M5).
//!
//! Anyone may read (the key carries the journal's DACL, `JOURNAL_KEY_SDDL`: Users read). Only the
//! elevated helper writes, under the write lock, on `Request::SetMachineSettings` (design m3
//! A.5). HKLM is opened for writing only here, in `regwrite` and in `journal_store` (design m2 K,
//! extended by m3 J).

use windows_registry::LOCAL_MACHINE;

use crate::error::Error;
use crate::reg::{open_read, read_dword};

/// The settings key, relative to `HKEY_LOCAL_MACHINE`.
pub const SETTINGS_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM\Settings";

/// `REG_DWORD` 1 or 0. Absent means 1 (the plan's default: restore on uninstall).
pub const RESTORE_ON_UNINSTALL_VALUE: &str = "RestoreOnUninstall";

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

/// Writes [`RESTORE_ON_UNINSTALL_VALUE`] (elevated helper only, under the write lock).
///
/// Implementation (WP-E2): create or open the key like `journal_store::JournalStore::open_or_create`
/// (`RegCreateKeyExW` with `JOURNAL_KEY_SDDL`, owner and DACL verified on an existing key, the
/// same `Insecure` rules), `RegSetValueExW(REG_DWORD, 0 or 1)`, `RegFlushKey`. The value name is
/// checked against a static list, as `regwrite` does (design m2 S9).
pub fn write_restore_on_uninstall(value: bool) -> Result<(), Error> {
    let _ = value;
    todo!("WP-E2: write HKLM\\...\\MKLM\\Settings\\RestoreOnUninstall in the helper")
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
}
