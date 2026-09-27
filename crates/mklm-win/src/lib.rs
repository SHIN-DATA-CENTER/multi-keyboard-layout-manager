//! Windows API layer for Multi Keyboard Layout Manager (MKLM).
//!
//! All `unsafe` Win32 calls are wrapped here so that the rest of the workspace stays safe code.
//!
//! Read-only (M1; safe to call unelevated at any time):
//! - [`snapshot()`]: one read-only pass that fills [`mklm_core::SystemSnapshot`].
//! - [`devices`]: Keyboard-class devnodes via CfgMgr32, with their "Device Parameters".
//! - [`rawinfo`]: type/subtype reported by the drivers (Raw Input).
//! - [`global`], [`input_lang`], [`os`]: global values, input methods and OS facts.
//! - [`process`]: DLL search hardening every executable applies at startup, and output encoding.
//! - [`journal_store::read_journal_store`], [`proc_identity`], [`elevation::is_elevated`].
//!
//! Registry keys in these modules are only ever opened with `KEY_READ`; device keys only through
//! `CM_Open_DevNode_Key(CM_REGISTRY_HARDWARE)` with `RegDisposition_OpenExisting`.
//!
//! Write-capable (M2; called by `mklm-engine` inside the elevated helper or elevated CLI only,
//! see docs/design/m2-engine.md section A.3):
//! - [`regwrite`]: device hardware keys and `i8042prt\Parameters` opened for writing, `RegFlushKey`.
//! - [`journal_store`]: the journal keys with their protected DACL.
//! - [`devctl`]: live reset (`DIF_PROPERTYCHANGE`) and devnode state polling.
//! - [`protected_dir`]: `%ProgramData%\SHIN DATA CENTER\MKLM` directories and the `LockFileEx` lock.
//!
//! Session plumbing (M2, either side of the pipe):
//! - [`pipe`], [`elevation`]: the helper pipe and the UAC launch.
//! - [`session`]: boot ID, randomness, PC restart and the post-reboot RunOnce entry.
//! - [`console`]: console control events for the in-process fallback of `mklm-cli`.
//! - [`session_end`]: the helper's shutdown order and its hidden window for
//!   `WM_QUERYENDSESSION` / `WM_ENDSESSION` (M3, design m3 WP-E3).
//!
//! GUI and front ends (M3, design m3 A.5):
//! - [`instance`]: the GUI's single instance (mutex and the `activate` / `quit` pipe).
//! - [`notify`]: keyboard arrival and removal notifications.
//! - [`time`]: journal timestamps in local time.
//! - [`machine_settings`]: `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Settings` (read by anyone,
//!   written by the helper only).
//! - `session`: also the GUI's HKCU autostart value.
//! - `ui` (feature `gui`, only `apps/mklm`): theme, high contrast, title bar, the hidden shell
//!   window, the tray balloon, the active input language, allowlisted settings pages.
//!
//! HKLM keys are opened with `KEY_SET_VALUE` only in [`regwrite`], [`journal_store`] (which
//! write through the private `regraw` value I/O) and [`machine_settings`], and the HKCU RunOnce
//! and Run values are written only in [`session`]. The private `security` module names the right
//! only to check ACLs.

#![cfg(windows)]

pub mod console;
pub mod devctl;
pub mod devices;
pub mod elevation;
mod error;
pub mod global;
mod hwkey;
pub mod input_lang;
pub mod instance;
pub mod journal_store;
pub mod machine_settings;
pub mod notify;
pub mod os;
pub mod pipe;
pub mod proc_identity;
pub mod process;
mod props;
pub mod protected_dir;
pub mod rawinfo;
mod reg;
mod regraw;
pub mod regwrite;
mod security;
pub mod session;
pub mod session_end;
mod sys;
pub mod time;
#[cfg(feature = "gui")]
pub mod ui;

use mklm_core::SystemSnapshot;

pub use devices::{KEYBOARD_CLASS_GUID, keyboard_instance_ids, read_keyboard, read_keyboards};
pub use error::{Error, ReadIssue, ReadIssueKind};
pub use global::read_global_settings;
pub use input_lang::{loaded_layouts, read_input_methods};
pub use os::read_os_info;
pub use process::{encode_for_redirected_output, restrict_dll_search};
pub use rawinfo::{RawKeyboard, instance_id_from_interface_path, raw_keyboards};

/// What [`snapshot()`] reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotOptions {
    /// Also list keyboards that are not connected (phantom devnodes). INV-PS2 checks need them.
    pub include_non_present: bool,
}

/// A snapshot together with what could not be read while taking it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReport {
    pub snapshot: SystemSnapshot,
    /// Non-fatal read failures. The affected fields are `None` / empty in `snapshot`, so a caller
    /// that is about to write must stop while this is non-empty.
    pub issues: Vec<ReadIssue>,
}

/// Reads everything MKLM needs in one read-only pass. See [`snapshot_report`] for the issues.
pub fn snapshot(opts: SnapshotOptions) -> Result<SystemSnapshot, Error> {
    snapshot_report(opts).map(|report| report.snapshot)
}

/// Like [`snapshot()`], and also returns the non-fatal read failures.
///
/// Fails when the keyboards cannot be enumerated, the global key exists but cannot be opened, or
/// the OS version cannot be read. A Raw Input failure only leaves every `reported_type` at `None`.
pub fn snapshot_report(opts: SnapshotOptions) -> Result<SnapshotReport, Error> {
    let mut issues = Vec::new();
    let raw = raw_keyboards(&mut issues).unwrap_or_else(|error| {
        issues.push(ReadIssue::new(ReadIssueKind::RawInput, "Raw Input", error));
        Vec::new()
    });
    let keyboards = read_keyboards(opts.include_non_present, &raw, &mut issues)?;
    let global = read_global_settings(&mut issues)?;
    let input = read_input_methods(&mut issues);
    let os = read_os_info(&mut issues)?;
    Ok(SnapshotReport {
        snapshot: SystemSnapshot {
            keyboards,
            global,
            input,
            os,
        },
        issues,
    })
}
