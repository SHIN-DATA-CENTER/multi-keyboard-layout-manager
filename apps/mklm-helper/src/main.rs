//! `mklm-helper`: the elevated, short-lived process that performs MKLM's writes (plan 2.1).
//!
//! Started only by the MKLM GUI or CLI through UAC (`ShellExecuteExW` "runas") with the fixed
//! command line of `mklm_ipc::HelperArgs`. It connects to the caller's pipe, runs `mklm_engine`
//! for each request and exits on `Bye`, a closed pipe or an idle timeout. No UI toolkit, no
//! network, never reads HKCU or `%APPDATA%` (plan 2.2). Design: docs/design/m2-engine.md, E.
//!
//! The M5 uninstall custom action will add a second, equally fixed command line
//! (`--uninstall-restore`) that runs a silent restore-to-baseline without a pipe.

#![windows_subsystem = "windows"]
#![cfg_attr(not(windows), allow(dead_code))]

mod session;

use std::process::ExitCode;

/// Process exit codes. The caller reads them from the process handle when the pipe never came up.
// Skeleton (M2): most codes are used once `session::run` is implemented.
#[allow(dead_code)]
mod exit {
    /// The session ended normally.
    pub const OK: u8 = 0;
    /// Any other failure (logged).
    pub const FAILURE: u8 = 1;
    /// The command line is not in the fixed format.
    pub const BAD_ARGUMENTS: u8 = 2;
    /// Pipe connection or handshake failed (PID, nonce or version mismatch, timeout).
    pub const HANDSHAKE: u8 = 3;
    /// Not elevated (the manifest should make this impossible).
    pub const NOT_ELEVATED: u8 = 4;
    /// Not Windows 11 24H2 (build 26100) or later, or not Windows at all.
    pub const UNSUPPORTED_OS: u8 = 5;
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
