//! The machine's update records and the user's, read unelevated (design m5b D.13, D.14, E.2).
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::path::Path;

use mklm_update::TrustState;
use mklm_update::run::{InstallState, RunView, UpdateResult};

use crate::update::cache::{ClientState, UpdateCache};
use crate::update::stage::HandOffProbe;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub machine_trust: TrustState,
    pub run: RunView,
    pub last_result: Option<UpdateResult>,
    /// `InstallState::from_build_ids(update_dir::read_build_ids(install_dir))`.
    pub install: InstallState,
    pub client: ClientState,
    /// English diagnostics of what could not be read (the fields then hold defaults).
    pub warnings: Vec<String>,
}

/// Unelevated: `read_update_store`, `classify_run` (boot ID, process liveness), the build IDs of
/// the three executables in `install_dir`, the user cache.
pub fn read_status(install_dir: &Path, cache: &UpdateCache) -> UpdateStatus {
    // Skeleton (M5b): WP-C
    UpdateStatus {
        machine_trust: TrustState::default(),
        run: RunView::Idle,
        last_result: None,
        install: InstallState::default(),
        client: cache.load_state(),
        warnings: vec!["not implemented (m5b skeleton)".to_string()],
    }
}

/// `HandOffProbe` over `read_update_store` for this process (`current_process_identity`).
#[derive(Debug)]
pub struct RunRecordProbe {
    /// Skeleton (M5b): WP-C keeps what the probe needs here.
    _private: (),
}

impl RunRecordProbe {
    pub fn new() -> RunRecordProbe {
        RunRecordProbe { _private: () }
    }
}

impl Default for RunRecordProbe {
    fn default() -> RunRecordProbe {
        RunRecordProbe::new()
    }
}

impl HandOffProbe for RunRecordProbe {
    fn handed_off(&mut self) -> Option<(String, String)> {
        None // Skeleton (M5b): WP-C
    }
}
