//! Per-user folders of the update cache (design m5b E.1). Not behind the `gui` feature: the CLI
//! uses them too.
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::path::PathBuf;

use crate::error::Error;

/// `SHGetKnownFolderPath(FOLDERID_LocalAppData)`.
pub fn local_app_data_dir() -> Result<PathBuf, Error> {
    Err(Error::Win32 {
        function: "local_app_data_dir (m5b skeleton)",
        code: 50,
    }) // Skeleton (M5b): WP-C
}

/// `<LocalAppData>\SHIN DATA CENTER\MKLM\update`, created if missing.
pub fn update_cache_dir() -> Result<PathBuf, Error> {
    Err(Error::Win32 {
        function: "update_cache_dir (m5b skeleton)",
        code: 50,
    }) // Skeleton (M5b): WP-C
}
