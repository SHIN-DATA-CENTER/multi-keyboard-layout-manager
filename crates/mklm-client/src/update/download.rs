//! The installer download into the user's cache (design m5b A.8, C.8, E.1).
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use mklm_update::fetch::{FetchError, Transport};

use crate::update::cache::UpdateCache;
use crate::update::check::Offer;
use crate::update::env::UpdateEnv;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Fetch(FetchError),
    Cache(String),
}

/// Into `<name>.part`, verified, then renamed to `<name>`; `progress(received, total)`.
pub fn download(
    transport: &mut dyn Transport,
    env: &UpdateEnv,
    cache: &UpdateCache,
    offer: &Offer,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, DownloadError> {
    // Skeleton (M5b): WP-C
    Err(DownloadError::Cache(
        "not implemented (m5b skeleton)".to_string(),
    ))
}
