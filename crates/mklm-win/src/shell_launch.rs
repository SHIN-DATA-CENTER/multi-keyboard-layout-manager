//! H2's COM set-up and the unelevated relaunch of the GUI through the desktop shell (design m5b
//! D.7 step 1, D.10; SECURITY-8).
//!
//! WP-H implements it (every `unsafe` block with its `// SAFETY:` comment).

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::path::Path;
use std::time::Duration;

use crate::error::Error;

/// H2's COM set-up, before any other COM call: `CoInitializeEx(COINIT_MULTITHREADED)` and
/// `CoInitializeSecurity` with `RPC_C_IMP_LEVEL_IDENTIFY` and
/// `EOAC_NO_CUSTOM_MARSHAL | EOAC_DISABLE_AAA` (SECURITY-8). Uninitializes on drop.
#[derive(Debug)]
pub struct RunnerCom {
    /// Skeleton (M5b): WP-H.
    _private: (),
}

pub fn init_com_for_runner() -> Result<RunnerCom, Error> {
    Err(Error::Win32 {
        function: "init_com_for_runner (m5b skeleton)",
        code: 50,
    }) // Skeleton (M5b): WP-H
}

/// Starts `exe arguments` through the desktop shell of this session (IShellWindows →
/// IShellDispatch2::ShellExecute): as the session's interactive user, unelevated. Gives up after
/// `timeout`. Never falls back to this process's own token.
pub fn launch_via_shell(
    exe: &Path,
    arguments: &str,
    directory: &Path,
    timeout: Duration,
) -> Result<(), Error> {
    Err(Error::Win32 {
        function: "launch_via_shell (m5b skeleton)",
        code: 50,
    }) // Skeleton (M5b): WP-H
}
