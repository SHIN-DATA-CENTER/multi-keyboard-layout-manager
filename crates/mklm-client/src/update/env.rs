//! What this installation can do about updates (design m5b D.2, E.1).
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::path::PathBuf;

use mklm_update::url::Endpoints;
use mklm_update::{Arch, TrustAnchors, Version};
use mklm_win::os::NativeMachine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    NotConfigured,
    NotInstalledCopy {
        exe_dir: PathBuf,
        install_dir: PathBuf,
    },
    Unknown {
        detail: String,
    },
}

#[derive(Debug)]
pub struct UpdateEnv {
    pub installed: Version,
    pub arch: Arch,
    pub native: Option<NativeMachine>,
    pub availability: Availability,
    /// `TrustAnchors::for_this_build()`.
    pub anchors: Option<TrustAnchors>,
    pub endpoints: Endpoints,
    pub install_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// `app_version` = the front end's `CARGO_PKG_VERSION`; `endpoints` = `Endpoints::production()`
/// (development builds: maybe the `--update-endpoint` override).
pub fn environment(app_version: &str, endpoints: Endpoints) -> UpdateEnv {
    // Skeleton (M5b): WP-C (availability, native machine, folders).
    UpdateEnv {
        installed: Version::parse(app_version).unwrap_or(Version::new(0, 0, 0)),
        arch: Arch::of_this_build(),
        native: None,
        availability: Availability::Unknown {
            detail: "not implemented (m5b skeleton)".to_string(),
        },
        anchors: TrustAnchors::for_this_build().ok(),
        endpoints,
        install_dir: PathBuf::new(),
        cache_dir: PathBuf::new(),
    }
}
