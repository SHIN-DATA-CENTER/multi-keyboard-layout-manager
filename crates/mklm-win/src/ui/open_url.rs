//! The two things the GUI may hand to the shell for updates (design m5b D.13, E.2; G.6 review
//! list): the release page, and the cached installer on the user's button press.
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::path::Path;

use crate::error::Error;

/// Opens `<repository>/releases/tag/v<version>`; `version` must be `X.Y.Z` digits.
pub fn open_release_page(version: &str) -> Result<(), Error> {
    Err(Error::Win32 {
        function: "open_release_page (m5b skeleton)",
        code: 50,
    }) // Skeleton (M5b): WP-C
}

/// `ShellExecuteExW` with `runas` of an installer in the user's update cache, on the user's button
/// press only (design m5b D.13; RELIABILITY-2). The caller's size and SHA-256 check only catches a
/// corrupted file: the signed-in user can swap the file afterwards, so this path is no more
/// trusted than running a downloaded installer by hand (RED-TEAM-2). `Error::Cancelled` when UAC
/// is declined.
pub fn run_installer_interactive(installer: &Path) -> Result<(), Error> {
    Err(Error::Win32 {
        function: "run_installer_interactive (m5b skeleton)",
        code: 50,
    }) // Skeleton (M5b): WP-C
}
