//! Starting a helper session (design E.1 to E.3). The work is `mklm_client::launch` (shared with
//! the GUI, design m3 A.2), run by `mklm_client::run_once::run_request`; the CLI gives it this
//! build's ID and its console window as the owner of the UAC prompt.

use std::path::PathBuf;

use mklm_client::launch::{self, LaunchConfig};
use mklm_win::elevation;

pub use mklm_client::LaunchError;

use super::BUILD_ID;

/// The helper next to this executable, checked to carry this build's ID before anything is
/// launched (design E.3 step 1, review S11).
pub fn checked_helper_path() -> Result<PathBuf, LaunchError> {
    launch::checked_helper_path(BUILD_ID)
}

/// How the CLI launches the helper (design E.1): `elevated` as `mklm_win::elevation::is_elevated`
/// says (then `CreateProcessW`, no UAC prompt).
pub fn config(elevated: bool) -> LaunchConfig {
    LaunchConfig {
        build_id: BUILD_ID.to_string(),
        elevated,
        owner_window: elevation::console_window(),
    }
}
