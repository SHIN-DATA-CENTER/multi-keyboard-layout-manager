//! The keyboards a request plans with (design m2 F.2 step 4, review S2): every Keyboard-class
//! devnode, phantoms included (INV-PS2 needs them). Problems that make the list incomplete stop a
//! write, as they would stop the helper; the others are warnings the caller shows.
//!
//! Blocking: enumeration takes up to a few hundred milliseconds. The GUI calls this on its I/O
//! worker thread.

use mklm_core::SystemSnapshot;
use mklm_win::{ReadIssue, ReadIssueKind, SnapshotOptions};

/// A complete enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    pub snapshot: SystemSnapshot,
    /// Some override or global value could not be read as MKLM expects (e.g. a value of another
    /// type): the snapshot shows it as absent, while the helper reads it as it is. Then only the
    /// helper may conclude that nothing needs writing.
    pub uncertain_values: bool,
    /// Problems that do not stop a write.
    pub warnings: Vec<ReadIssue>,
}

/// Why the keyboards cannot be planned with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryError {
    /// The enumeration itself failed.
    Read(mklm_win::Error),
    /// Problems that stop a write (`ReadIssueKind::blocks_writes`), and the other warnings.
    Incomplete {
        blocking: Vec<ReadIssue>,
        warnings: Vec<ReadIssue>,
    },
}

/// Reads the keyboards for planning.
pub fn read_inventory() -> Result<Inventory, InventoryError> {
    let report = mklm_win::snapshot_report(SnapshotOptions {
        include_non_present: true,
    })
    .map_err(InventoryError::Read)?;
    let (blocking, warnings): (Vec<ReadIssue>, Vec<ReadIssue>) = report
        .issues
        .into_iter()
        .partition(|issue| issue.kind.blocks_writes());
    if !blocking.is_empty() {
        return Err(InventoryError::Incomplete { blocking, warnings });
    }
    Ok(Inventory {
        uncertain_values: warnings
            .iter()
            .any(|issue| issue.kind == ReadIssueKind::Values),
        warnings,
        snapshot: report.snapshot,
    })
}

/// The keyboards for display only (current values, Raw Input): every problem is a warning. The
/// way out when something went wrong (`undo`, the journal view) must not fail because a keyboard
/// property could not be read; the helper checks again before it writes.
pub fn read_display_snapshot() -> Result<(SystemSnapshot, Vec<ReadIssue>), mklm_win::Error> {
    mklm_win::snapshot_report(SnapshotOptions {
        include_non_present: true,
    })
    .map(|report| (report.snapshot, report.issues))
}
