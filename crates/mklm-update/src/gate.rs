//! The journal gate of an update (design m5b D.4 step 6, D.7 step 10): no update while an
//! operation is open.
//!
//! The helper reads the journal under the write lock (H1 before it stages, H2 again before it
//! starts the installer), so every entry it sees is either closed, waiting for the user, or
//! abandoned; the update never writes the journal, it only refuses to start (design m5b 0.2 7).

use mklm_core::{Journal, OpState};

use crate::refusal::UpdateRefusal;

/// Unreadable entries → `JournalUnreadable`; an in-flight entry → `RecoveryNeeded`; any other
/// open entry → `OperationOpen { waiting_for_reboot }` (true when one is `PendingReboot`).
///
/// The order follows what the user must do first: a journal MKLM cannot read stops everything
/// (a human looks at it), an abandoned write needs `recover`, and an operation that waits for a
/// keep / revert answer or a restart needs that answer.
pub fn check_journal(journal: &Journal) -> Result<(), UpdateRefusal> {
    if !journal.unreadable.is_empty() {
        return Err(UpdateRefusal::JournalUnreadable);
    }
    let open = journal.open_entries();
    if open.iter().any(|entry| entry.state.is_in_flight()) {
        return Err(UpdateRefusal::RecoveryNeeded);
    }
    if !open.is_empty() {
        return Err(UpdateRefusal::OperationOpen {
            waiting_for_reboot: open
                .iter()
                .any(|entry| entry.state == OpState::PendingReboot),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use mklm_core::fixtures::schema_1_journal;
    use mklm_core::{JournalError, UnreadableEntry};

    use super::*;

    /// The recorded development-machine journal: nine closed operations.
    fn closed_journal() -> Journal {
        let (ops, baselines) = schema_1_journal();
        let journal = Journal::parse(&ops, &baselines);
        assert!(journal.unreadable.is_empty(), "{:?}", journal.unreadable);
        assert!(!journal.entries.is_empty());
        assert!(journal.open_entries().is_empty());
        journal
    }

    /// The fixture journal with its newest entry moved to `state` (field only; the gate reads
    /// nothing else).
    fn with_state(state: OpState) -> Journal {
        let mut journal = closed_journal();
        journal
            .entries
            .last_mut()
            .expect("the fixture has entries")
            .state = state;
        journal
    }

    #[test]
    fn closed_operations_let_the_update_go() {
        assert_eq!(check_journal(&Journal::default()), Ok(()));
        assert_eq!(check_journal(&closed_journal()), Ok(()));
        for state in [
            OpState::Confirmed,
            OpState::Reverted,
            OpState::RevertedPendingReboot,
            OpState::Failed,
        ] {
            assert_eq!(check_journal(&with_state(state)), Ok(()), "{state:?}");
        }
    }

    #[test]
    fn open_operations_stop_it() {
        for state in [
            OpState::Planned,
            OpState::Written,
            OpState::Restarting,
            OpState::RevertPending,
        ] {
            assert_eq!(
                check_journal(&with_state(state)),
                Err(UpdateRefusal::RecoveryNeeded),
                "{state:?}"
            );
        }
        assert_eq!(
            check_journal(&with_state(OpState::AwaitingConfirm)),
            Err(UpdateRefusal::OperationOpen {
                waiting_for_reboot: false
            })
        );
        assert_eq!(
            check_journal(&with_state(OpState::PendingReboot)),
            Err(UpdateRefusal::OperationOpen {
                waiting_for_reboot: true
            })
        );
        assert_eq!(
            check_journal(&with_state(OpState::Conflict)),
            Err(UpdateRefusal::OperationOpen {
                waiting_for_reboot: false
            })
        );
        // Every state is covered by one of the two tests.
        assert_eq!(OpState::ALL.len(), 11);
    }

    #[test]
    fn the_worst_finding_decides() {
        // An in-flight entry next to one that waits for a restart: recovery first.
        let mut journal = with_state(OpState::PendingReboot);
        journal.entries[0].state = OpState::Written;
        assert_eq!(check_journal(&journal), Err(UpdateRefusal::RecoveryNeeded));
        // A restart wait next to a keep-or-revert question: the restart is reported.
        let mut journal = with_state(OpState::PendingReboot);
        journal.entries[0].state = OpState::AwaitingConfirm;
        assert_eq!(
            check_journal(&journal),
            Err(UpdateRefusal::OperationOpen {
                waiting_for_reboot: true
            })
        );
        // Anything unreadable stops it, even with every readable entry closed.
        let mut journal = with_state(OpState::Written);
        journal.unreadable.push(UnreadableEntry {
            name: "0f8c2d4e-5b6a-4c3d-9e8f-a0b1c2d3e4f5".to_string(),
            error: JournalError::Malformed {
                message: "truncated".to_string(),
            },
        });
        assert_eq!(
            check_journal(&journal),
            Err(UpdateRefusal::JournalUnreadable)
        );
        let (ops, baselines) = schema_1_journal();
        let mut ops = ops;
        ops[0].1.push('x');
        let journal = Journal::parse(&ops, &baselines);
        assert_eq!(
            check_journal(&journal),
            Err(UpdateRefusal::JournalUnreadable)
        );
    }
}
