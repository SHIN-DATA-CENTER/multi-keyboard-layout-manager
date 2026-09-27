//! Plans that put recorded values back: revert, rollback, restore to baseline and conflict
//! resolutions (plan 1.3, 2.3).
//!
//! The allowlist ([`crate::allowlist`]) governs new values; restores are validated against the
//! journal instead: only recorded (target, name) pairs, only names MKLM may write for that target,
//! and only values the journal holds.
//!
//! **Order follows the direction of the change, not a fixed sequence** (design review C4). A fixed
//! "global → others → i8042prt" order is right only when going back to fixed mode; restoring
//! towards per-keyboard mode (undoing a restore-to-baseline, or a resolution that keeps a
//! migration) must pin the PS/2 keyboards before the global pair goes. Every step is put in one of
//! the [`RestorePhase`]s, which run in declaration order:
//!
//! 1. pins added or changed on i8042prt keyboards (both device values present afterwards);
//! 2. the global key, when the fixed pair is present afterwards (added or kept);
//! 3. every other keyboard (HID);
//! 4. the global key, when the fixed pair is absent afterwards (removed or still absent);
//! 5. pins removed from i8042prt keyboards.
//!
//! If the end state satisfies INV-PS2, no intermediate state breaks it: pins only grow before the
//! pair is removed (phase 4), and the pair is present whenever a pin is removed (phase 5). The plan
//! is still replayed step by step and checked, like [`crate::check_plan`] does for new writes.

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use serde::{Deserialize, Serialize};

use crate::allowlist::WriteTarget;
use crate::journal::{RegValue, ValueRecord};
use crate::model::{GlobalSettings, KeyboardDevice};
use crate::safety::InvPs2Violation;

/// Which recorded value a restore writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreTo {
    /// `before`: revert one operation, or roll back a partial one.
    Before,
    /// `baseline`: "MKLM 導入前に戻す".
    Baseline,
    /// `resolve_to`: a conflict resolution (design review C5).
    Resolution,
}

/// What the current value must be for a restore write to go ahead (compare-and-swap). A current
/// value that already equals the restore value ([`crate::value_eq`]) is always skipped as done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Expect {
    /// Must equal this value: `last_written` (revert), `intended` (rollback of an entry that never
    /// reached `Written`), the value the user saw in the conflict (resolution), or the latest
    /// record's `last_written` (restore to baseline).
    Value { value: RegValue },
    /// Anything: only for restore-to-baseline with [`crate::ConflictPolicy::Overwrite`], where the
    /// user chose to overwrite every conflict after seeing the values. Resolutions never use it.
    Any,
}

/// One value to put back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreWrite {
    /// Index into the records the plan was built from.
    pub record: usize,
    pub name: String,
    pub value: RegValue,
    pub expect: Expect,
}

/// Position of a step in the restore order (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestorePhase {
    AddPins,
    GlobalWithPair,
    Other,
    GlobalWithoutPair,
    RemovePins,
}

/// The restore writes to one registry key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreStep {
    pub phase: RestorePhase,
    pub target: WriteTarget,
    pub writes: Vec<RestoreWrite>,
}

/// Restore steps in execution order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePlan {
    pub steps: Vec<RestoreStep>,
    /// Set when the end state violates INV-PS2. Allowed only when that end state is exactly the
    /// recorded baseline of every i8042prt and global value (MKLM puts back a pre-existing
    /// violation, and says so); every other violating plan is rejected.
    pub restores_inv_ps2_violation: Option<InvPs2Violation>,
}

/// A restore the journal rules reject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RestoreError {
    #[error("{name:?} is not a value MKLM writes on {target:?}")]
    NameNotAllowed { target: WriteTarget, name: String },
    #[error("record {record} has no value to compare against")]
    NothingToCompare { record: usize },
    #[error("record {record} has no resolution value")]
    NoResolution { record: usize },
    #[error("a restore step would break INV-PS2: {0}")]
    InvPs2(InvPs2Violation),
}

/// Checks one record against the journal rules: device names must be the type/subtype pair of the
/// keyboard's driver (either stack's pair when the devnode is gone), global names must be one of
/// the four global names ([`crate::GLOBAL_VALUE_NAMES`]), and [`RegValue::Other`] may only be
/// restored as the record's `baseline` (or as a `resolve_to` equal to the baseline or to the value
/// the user saw).
pub fn check_restore_record(
    record: &ValueRecord,
    keyboards: &[KeyboardDevice],
    to: RestoreTo,
) -> Result<(), RestoreError> {
    todo!("M2")
}

/// Builds the ordered restore plan for `records` (phases in the module docs) and replays it on
/// `keyboards` / `global` to check INV-PS2 after every step. Records whose devnode is gone are left
/// out by the caller (`SkipReason::DeviceRemoved`).
///
/// `expect(i)` gives the compare-and-swap expectation of record `i`.
pub fn plan_restore(
    records: &[ValueRecord],
    to: RestoreTo,
    expect: &dyn Fn(usize) -> Expect,
    keyboards: &[KeyboardDevice],
    global: &GlobalSettings,
) -> Result<RestorePlan, RestoreError> {
    todo!("M2")
}
