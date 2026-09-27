//! Offline recovery files (plan 2.3 "helper がないときの回復"): a `.reg` file and
//! `restore-offline.cmd` that put every value MKLM changed back to its baseline, using this PC's
//! real instance paths. Rendering is pure; `mklm-engine` writes the files into
//! `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery` through `mklm-win`.
//!
//! Formats (section G of docs/design/m2-engine.md):
//! - `.reg`: UTF-16LE with BOM, `Windows Registry Editor Version 5.00`, keys under
//!   `HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\…`, `"name"=dword:…`, `"name"="…"`, `"name"=-`.
//! - `.cmd`: ASCII, CRLF, `restore-offline.cmd online` or `restore-offline.cmd offline D:` (WinRE:
//!   loads `D:\Windows\System32\config\SYSTEM`, resolves `Select\Default`, the control set the
//!   next boot uses, to `ControlSet00N`, and stops with a message when `Select\Current` differs;
//!   design review C11).
//!
//! Order (both files). The files do not know the current values, so they use one static order
//! that is safe from any starting point (design review C4): i8042prt values that the baseline sets,
//! then global values it sets, then HID values, then global values it deletes, then i8042prt values
//! it deletes. Pins and the fixed pair are therefore present before anything that relies on them
//! is removed; a power loss in the middle of the script cannot break INV-PS2 unless the baseline
//! itself does.
//!
//! Every string that reaches the `.cmd` must pass [`is_cmd_safe`]; a record that does not is left
//! out with a `rem` line that names only its position, never its text (a USB serial number ends up
//! in instance IDs and must not be able to inject commands).

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use serde::{Deserialize, Serialize};

use crate::journal::{BaselineRecord, Timestamp};

/// File name of the batch file.
pub const RESTORE_CMD_FILE: &str = "restore-offline.cmd";
/// File name of the `.reg` file.
pub const BASELINE_REG_FILE: &str = "mklm-baseline.reg";
/// File name of the explanation next to them (UTF-8 with BOM, Japanese and English).
pub const README_FILE: &str = "README.txt";
/// Hive mount point `restore-offline.cmd offline` uses in WinRE.
pub const OFFLINE_HIVE_MOUNT: &str = r"HKLM\MKLM_OFFLINE";

/// Rendered recovery files, ready to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryAssets {
    /// [`BASELINE_REG_FILE`] content (UTF-16LE with BOM).
    pub reg: Vec<u8>,
    /// [`RESTORE_CMD_FILE`] content (ASCII, CRLF).
    pub cmd: String,
    /// [`README_FILE`] content.
    pub readme: String,
    /// Records left out because they failed [`is_cmd_safe`] (indices into the input).
    pub skipped: Vec<usize>,
}

/// Rendering failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AssetError {
    #[error("no baseline records to render")]
    Empty,
}

/// True when `text` contains only characters that are inert inside a double-quoted `cmd.exe`
/// argument with delayed expansion off: ASCII letters, digits, space and `_ - . \ & # { } ( ) , ; =`.
/// Rejects `"`, `%`, `!`, `^`, `<`, `>`, `|`, control characters and anything non-ASCII.
pub fn is_cmd_safe(text: &str) -> bool {
    todo!("M2")
}

/// Renders the three files from the baseline store.
pub fn render_recovery_assets(
    baselines: &[BaselineRecord],
    generator_version: &str,
    generated_at: Timestamp,
) -> Result<RecoveryAssets, AssetError> {
    todo!("M2")
}
