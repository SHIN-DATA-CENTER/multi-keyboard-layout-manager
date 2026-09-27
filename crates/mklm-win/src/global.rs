//! Global keyboard settings under `Services\i8042prt\Parameters`, read-only.

use mklm_core::{GlobalSettings, value_names};
use windows_registry::LOCAL_MACHINE;

use crate::error::{Error, ReadIssue, ReadIssueKind};
use crate::reg::{open_read, read_dword, read_string};

/// Key holding the global values, relative to `HKEY_LOCAL_MACHINE`.
pub const I8042PRT_PARAMETERS: &str = r"SYSTEM\CurrentControlSet\Services\i8042prt\Parameters";

/// Reads the global values. A missing key or value is `None`.
///
/// Fails only when the key exists but cannot be opened. A value of an unexpected type is `None`
/// with an issue.
pub fn read_global_settings(issues: &mut Vec<ReadIssue>) -> Result<GlobalSettings, Error> {
    let Some(key) = open_read(LOCAL_MACHINE, "HKLM", I8042PRT_PARAMETERS)? else {
        return Ok(GlobalSettings::default());
    };
    let path = format!(r"HKLM\{I8042PRT_PARAMETERS}");
    let mut string = |name: &str| {
        read_string(&key, &path, name).unwrap_or_else(|error| {
            issues.push(ReadIssue::new(
                ReadIssueKind::Values,
                format!(r"{path}\{name}"),
                error,
            ));
            None
        })
    };
    let layer_driver_jpn = string(value_names::LAYER_DRIVER_JPN);
    let layer_driver_kor = string(value_names::LAYER_DRIVER_KOR);
    let override_keyboard_identifier = string(value_names::KEYBOARD_IDENTIFIER);
    let mut dword = |name: &str| {
        read_dword(&key, &path, name).unwrap_or_else(|error| {
            issues.push(ReadIssue::new(
                ReadIssueKind::Values,
                format!(r"{path}\{name}"),
                error,
            ));
            None
        })
    };
    Ok(GlobalSettings {
        layer_driver_jpn,
        layer_driver_kor,
        override_keyboard_identifier,
        override_keyboard_type: dword(value_names::PS2_TYPE),
        override_keyboard_subtype: dword(value_names::PS2_SUBTYPE),
    })
}
