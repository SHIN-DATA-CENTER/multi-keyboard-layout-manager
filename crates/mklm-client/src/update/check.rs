//! The update check (design m5b A.5, C.3, C.4, E.1): fetch, verify, record, cache. Never
//! downloads the installer.
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use mklm_update::fetch::{FetchError, Transport};
use mklm_update::{SignatureSlot, TrustState, UpdateRefusal, VerifiedManifest, Version};

use crate::update::cache::UpdateCache;
use crate::update::env::{Availability, UpdateEnv};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub verified: VerifiedManifest,
    pub manifest: Vec<u8>,
    /// The signature that verified (main or alternate).
    pub signature: Vec<u8>,
    pub slot: SignatureSlot,
    pub skipped: bool,
    /// The cached installer whose size and SHA-256 match, if any.
    pub downloaded: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    UpToDate(VerifiedManifest),
    Available(Offer),
    ManualRequired(VerifiedManifest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    Unavailable(Availability),
    Fetch(FetchError),
    Refused(UpdateRefusal),
    Cache(String),
}

/// Fetch, verify (`Purpose::Check`, machine ∪ user state; the alternate signature when
/// `tries_alternate`), record the user state (including `last_failure` and `last_rollback`),
/// cache the manifest. Never downloads the installer.
pub fn check(
    transport: &mut dyn Transport,
    env: &UpdateEnv,
    cache: &UpdateCache,
    machine: &TrustState,
    skipped: Option<&Version>,
    now_unix: u64,
    cancel: &AtomicBool,
) -> Result<CheckOutcome, CheckError> {
    Err(skeleton()) // Skeleton (M5b): WP-C
}

/// The cached manifest verified again (before "update now", after a restart).
pub fn reverify_cached(
    env: &UpdateEnv,
    cache: &UpdateCache,
    machine: &TrustState,
    skipped: Option<&Version>,
    now_unix: u64,
) -> Result<CheckOutcome, CheckError> {
    Err(skeleton()) // Skeleton (M5b): WP-C
}

/// What the WP-0 skeleton's unimplemented functions return (design m5b G.2).
fn skeleton() -> CheckError {
    CheckError::Cache("not implemented (m5b skeleton)".to_string())
}
