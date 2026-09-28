//! Version rules (design m5b A.2, C.5).
//!
//! WP-0 writes the signatures; WP-U the rules.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use crate::Version;
use crate::refusal::UpdateRefusal;

/// Largest value of one version part (VERSIONINFO fields are 16-bit).
pub const MAX_VERSION_PART: u64 = 65_535;

/// `X.Y.Z` only (design m5b A.2). `BadVersion`.
pub fn parse_release_version(text: &str) -> Result<Version, UpdateRefusal> {
    Err(UpdateRefusal::skeleton()) // Skeleton (M5b): WP-U
}

/// `CARGO_PKG_VERSION`: a pre-release is allowed, build metadata is not. `BadVersion`.
pub fn parse_installed_version(text: &str) -> Result<Version, UpdateRefusal> {
    Err(UpdateRefusal::skeleton()) // Skeleton (M5b): WP-U
}

/// SemVer precedence: `offered > installed`.
pub fn is_newer(offered: &Version, installed: &Version) -> bool {
    false // Skeleton (M5b): WP-U
}
