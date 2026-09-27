//! Starting a helper session (design E.1 to E.3). The work is `mklm_client::launch` (shared with
//! the GUI, design m3 A.2); the CLI gives it this build's ID and its console window as the owner
//! of the UAC prompt.

use std::path::PathBuf;

use mklm_client::launch::{self, LaunchConfig};
use mklm_win::elevation;

pub use mklm_client::LaunchError;
pub use mklm_client::launch::HelperSession;

use super::BUILD_ID;

/// The helper next to this executable, checked to carry this build's ID before anything is
/// launched (design E.3 step 1, review S11).
pub fn checked_helper_path() -> Result<PathBuf, LaunchError> {
    launch::checked_helper_path(BUILD_ID)
}

/// Creates the pipe, launches the helper and runs the handshake (design E.1).
pub fn start(elevated: bool) -> Result<HelperSession, LaunchError> {
    launch::start(&LaunchConfig {
        build_id: BUILD_ID.to_string(),
        elevated,
        owner_window: elevation::console_window(),
    })
}
