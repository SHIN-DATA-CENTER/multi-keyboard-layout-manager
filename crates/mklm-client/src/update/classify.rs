//! Check failures as the user sees them (design m5b E.6): transient or structural, and the
//! message ID.
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use crate::update::cache::ErrorClass;
use crate::update::check::CheckError;

/// Design m5b E.6: the class and message ID of a check failure. `not_found_days` = days since the
/// run of failures began (NotFound turns structural after `NOT_FOUND_STRUCTURAL_DAYS`). `None`
/// for what is not a failure (both `Cancelled`s, `Unavailable(NotConfigured)`,
/// `Unavailable(NotInstalledCopy)`): not recorded, `last_check` unchanged. `Rollback` is
/// `Structural` (FIX-VERIFICATION-12).
pub fn classify(error: &CheckError, not_found_days: u64) -> Option<(ErrorClass, &'static str)> {
    None // Skeleton (M5b): WP-C
}
