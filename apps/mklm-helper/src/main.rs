//! `mklm-helper`: the elevated, short-lived process that performs MKLM's writes (plan 2.1).
//!
//! Started only by the MKLM GUI or CLI, through UAC (`ShellExecuteExW` "runas") or, from an
//! elevated caller, with `CreateProcessW`, with the fixed command line of `mklm_ipc::HelperArgs`.
//! It connects to the caller's pipe, runs `mklm_engine` for each request and exits on `Bye`, a
//! closed pipe or an idle timeout. No UI toolkit, no network, never reads HKCU or `%APPDATA%`
//! (plan 2.2). Design: docs/design/m2-engine.md, sections A.6 and E.
//!
//! A second, equally fixed command line, `--uninstall-restore` (M5), is run by the elevated
//! uninstaller: a silent restore-to-baseline without a pipe, exit 0 / 3010 (restart needed) /
//! 6 (busy) / 1.
//!
//! A third, `--run-update <run-id>` (M5b, docs/design/m5b-updater.md D.7), is run by the helper
//! itself: its copy `mklm-update-runner.exe` in a protected run folder installs a verified update
//! (exit 0 installed / 7 not installed). In a pipe session the helper also stages updates
//! (`StageUpdate`) and records verified manifests (`RecordTrust`). It never uses the network.

#![windows_subsystem = "windows"]
#![cfg_attr(not(windows), allow(dead_code))]

mod run_update;
mod session;
mod update;

use std::process::ExitCode;

/// Process exit codes (design E.8). The caller reads them from the process handle when the pipe
/// never came up.
mod exit {
    /// The session ended normally (`Bye`, a closed pipe, the idle timeout).
    pub const OK: u8 = 0;
    /// Any other failure.
    pub const FAILURE: u8 = 1;
    /// The command line is not in the fixed format.
    pub const BAD_ARGUMENTS: u8 = 2;
    /// Pipe connection or handshake failed (PID, nonce, version or build mismatch, timeout).
    pub const HANDSHAKE: u8 = 3;
    /// Not elevated (the manifest should make this impossible).
    pub const NOT_ELEVATED: u8 = 4;
    /// Not Windows 11 24H2 (build 26100) or later, or not Windows at all.
    pub const UNSUPPORTED_OS: u8 = 5;
    /// `--run-update`: not installed, or failed; the reason is in `LastResult` (none before
    /// `ready`). Also when the installer did not finish within 60 minutes (design m5b D.15).
    pub const NOT_UPDATED: u8 = 7;
}

fn main() -> ExitCode {
    #[cfg(windows)]
    {
        // Plan 2.2: before anything else can load a DLL. Failing to harden is fatal here, unlike
        // in the unelevated tools: this process runs with administrator rights.
        if mklm_win::restrict_dll_search().is_err() {
            return ExitCode::from(exit::FAILURE);
        }
        session::run()
    }
    #[cfg(not(windows))]
    {
        ExitCode::from(exit::UNSUPPORTED_OS)
    }
}
