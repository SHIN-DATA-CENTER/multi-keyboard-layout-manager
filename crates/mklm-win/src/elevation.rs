//! Elevation (M2): the token check and launching the helper, through UAC when the caller is not
//! elevated and directly when it is.

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(unused_variables, dead_code)]

use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::Error;

/// File name of the helper.
pub const HELPER_EXE: &str = "mklm-helper.exe";

/// Name of the VERSIONINFO string that carries the build ID both executables embed
/// (design review S11).
pub const BUILD_ID_VERSION_KEY: &str = "MKLMBuildId";

/// True when this process's token is elevated (`GetTokenInformation(TokenElevation)`).
pub fn is_elevated() -> Result<bool, Error> {
    todo!("M2")
}

/// A helper process started by [`launch_elevated`] or [`spawn_from_elevated`].
#[derive(Debug)]
pub struct ElevatedProcess {
    handle: OwnedHandle,
    pid: u32,
}

impl ElevatedProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Exit code once it has exited; `None` while it runs.
    pub fn exit_code(&self) -> Result<Option<u32>, Error> {
        todo!("M2")
    }

    /// Waits up to `timeout` for it to exit.
    pub fn wait(&self, timeout: Duration) -> Result<Option<u32>, Error> {
        todo!("M2")
    }
}

/// Unelevated caller: `ShellExecuteExW` with verb `runas`, the absolute `exe`, `parameters` (the
/// fixed helper arguments), `lpDirectory` = System32, `SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC |
/// SEE_MASK_FLAG_NO_UI` and `SW_HIDE`. `owner_window` parents the UAC prompt (the GUI window, or
/// the console window for the CLI). Fails with [`Error::Cancelled`] when the user declines.
pub fn launch_elevated(
    exe: &Path,
    parameters: &str,
    owner_window: Option<isize>,
) -> Result<ElevatedProcess, Error> {
    todo!("M2")
}

/// Elevated caller (an administrator console, Safe Mode): `CreateProcessW` of the absolute `exe`
/// with `parameters`, current directory System32, `CREATE_NO_WINDOW`; no UAC prompt and no
/// dependency on the Appinfo service. The helper still runs as a separate process, so that closing
/// the console or pressing Ctrl+C disconnects the pipe and a running countdown reverts at once
/// (design review C8).
pub fn spawn_from_elevated(exe: &Path, parameters: &str) -> Result<ElevatedProcess, Error> {
    todo!("M2")
}

/// M2 helper discovery: `mklm-helper.exe` in the directory of the running executable, as an
/// absolute path, checked to be a regular file (not a reparse point). M5 replaces this with the
/// install location recorded in HKLM.
pub fn helper_path() -> Result<PathBuf, Error> {
    todo!("M2")
}

/// The build ID in `exe`'s VERSIONINFO ([`BUILD_ID_VERSION_KEY`], `GetFileVersionInfoW` +
/// `VerQueryValueW`). The caller compares it with its own before launching the helper, so that a
/// stale helper fails before the UAC prompt (design review S11).
pub fn file_build_id(exe: &Path) -> Result<String, Error> {
    todo!("M2")
}
