//! The `RecordTrust` a session sends after `Welcome` (design m5b C.4; SECURITY-5).
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use mklm_ipc::TrustReport;
use mklm_update::TrustState;

use crate::update::cache::UpdateCache;

/// The `RecordTrust` to send after `Welcome`, if any (design m5b C.4): the cached verified
/// manifest and its signature when the user record `is_ahead_of` the machine record.
pub fn pending_trust_report(cache: &UpdateCache, machine: &TrustState) -> Option<TrustReport> {
    None // Skeleton (M5b): WP-C
}
