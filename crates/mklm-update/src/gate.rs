//! The journal gate of an update (design m5b D.4 step 6, D.7 step 10): no update while an
//! operation is open.
//!
//! WP-0 writes the signature; WP-H the rule.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use crate::refusal::UpdateRefusal;

/// Unreadable entries → `JournalUnreadable`; an in-flight entry → `RecoveryNeeded`; any other
/// open entry → `OperationOpen { waiting_for_reboot }` (true when one is `PendingReboot`).
pub fn check_journal(journal: &mklm_core::Journal) -> Result<(), UpdateRefusal> {
    Err(UpdateRefusal::skeleton()) // Skeleton (M5b): WP-H
}
