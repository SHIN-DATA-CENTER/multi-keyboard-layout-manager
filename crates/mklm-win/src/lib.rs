//! Windows API layer for Multi Keyboard Layout Manager (MKLM).
//!
//! Every function in this crate is read-only with respect to the system for now (milestone M1).
//! All `unsafe` Win32 calls are wrapped here so that the rest of the workspace stays safe code.
//!
//! - [`snapshot()`]: one read-only pass that fills [`mklm_core::SystemSnapshot`].
//! - [`devices`]: Keyboard-class devnodes via CfgMgr32, with their "Device Parameters".
//! - [`rawinfo`]: type/subtype reported by the drivers (Raw Input).
//! - [`global`], [`input_lang`], [`os`]: global values, input methods and OS facts.
//! - [`process`]: DLL search hardening every executable applies at startup, and output encoding.
//!
//! Registry keys are only ever opened with `KEY_READ`; device keys only through
//! `CM_Open_DevNode_Key(CM_REGISTRY_HARDWARE)` with `RegDisposition_OpenExisting`.

#![cfg(windows)]

pub mod devices;
mod error;
pub mod global;
mod hwkey;
pub mod input_lang;
pub mod os;
pub mod process;
mod props;
pub mod rawinfo;
mod reg;

use mklm_core::SystemSnapshot;

pub use devices::{KEYBOARD_CLASS_GUID, keyboard_instance_ids, read_keyboard, read_keyboards};
pub use error::{Error, ReadIssue};
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
        issues.push(ReadIssue::new("Raw Input", error));
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
