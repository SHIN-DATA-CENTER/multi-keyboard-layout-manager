//! Per-user folders of the update cache (design m5b E.1, H.5). Not behind the `gui` feature: the
//! CLI uses them too.
//!
//! `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\` holds the user's update record (`state.json`),
//! the last verified manifest and its signature, and the verified installer. It is the signed-in
//! user's own folder, not a trust boundary: the elevated helper never opens anything here (it
//! receives the installer over the pipe, design m5b D.3). Local, not roaming: nothing of it
//! follows the user to another PC.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

use crate::error::Error;
use crate::sys::win32;

/// The cache below the local application data folder.
pub const UPDATE_CACHE_SUBDIR: [&str; 3] = ["SHIN DATA CENTER", "MKLM", "update"];

/// `SHGetKnownFolderPath(FOLDERID_LocalAppData)`: the current user's, never from the environment.
pub fn local_app_data_dir() -> Result<PathBuf, Error> {
    // SAFETY: FOLDERID_LocalAppData is a static known-folder GUID; no token (the current user).
    // The returned string is freed below.
    let text = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None) }
        .map_err(|error| win32("SHGetKnownFolderPath", &error))?;
    // SAFETY: on success `text` is a NUL-terminated string.
    let path = PathBuf::from(OsString::from_wide(unsafe { text.as_wide() }));
    // SAFETY: allocated by SHGetKnownFolderPath with CoTaskMemAlloc; freed once, after the copy.
    unsafe { CoTaskMemFree(Some(text.0.cast_const().cast())) };
    Ok(path)
}

/// `<LocalAppData>\SHIN DATA CENTER\MKLM\update`, created if missing.
pub fn update_cache_dir() -> Result<PathBuf, Error> {
    let dir = update_cache_path()?;
    std::fs::create_dir_all(&dir).map_err(|error| Error::Win32 {
        function: "CreateDirectoryW",
        code: error.raw_os_error().map_or(0, |code| code as u32),
    })?;
    Ok(dir)
}

/// The cache folder's path, not created.
pub(crate) fn update_cache_path() -> Result<PathBuf, Error> {
    Ok(UPDATE_CACHE_SUBDIR
        .iter()
        .fold(local_app_data_dir()?, |path, part| path.join(part)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the folder only: the test never creates the cache (the GUI and the CLI do).
    #[test]
    fn the_local_application_data_folder() {
        let dir = local_app_data_dir().unwrap();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert_eq!(UPDATE_CACHE_SUBDIR, ["SHIN DATA CENTER", "MKLM", "update"]);
    }
}
