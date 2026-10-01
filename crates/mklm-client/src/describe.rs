//! Facts both front ends need to word things correctly (design m3 G.2).
//!
//! M2 real-machine test R5: after the helper was killed right after its first write (before any
//! keyboard reset), recovery rolled back with `LiveResetUnconfirmed`, and the CLI said "the change
//! was never kept after the keyboard reset", although no reset had happened. The reason stays
//! (the journal's wording is part of its schema); the text now depends on whether the reset was
//! reached, which the entry's history (or a recovered operation's `from` state) tells.

use mklm_core::{JournalEntry, OpState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "state", rename_all = "kebab-case")]
pub enum ShownState {
    Stored(OpState),
    RestartedCheckDue,
    RevertedAndRestarted,
}

pub fn shown_state(entry: &JournalEntry, boot: Option<mklm_core::BootId>) -> ShownState {
    if boot.is_some_and(|boot| boot != entry.boot_id) {
        match entry.state {
            OpState::PendingReboot => return ShownState::RestartedCheckDue,
            OpState::RevertedPendingReboot => return ShownState::RevertedAndRestarted,
            _ => {}
        }
    }
    ShownState::Stored(entry.state)
}

/// Whether the operation got as far as resetting the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetPhase {
    /// Stopped before the reset: the keyboard never switched.
    NotReached,
    /// The reset started (it may or may not have finished).
    Reached,
}

/// From an entry's history: did it ever enter `Restarting`?
pub fn reset_phase(entry: &JournalEntry) -> ResetPhase {
    if entry
        .history
        .iter()
        .any(|line| line.to == OpState::Restarting)
    {
        ResetPhase::Reached
    } else {
        ResetPhase::NotReached
    }
}

/// From the state a recovered operation was found in (`RecoveredOp::from`): `Planned` and
/// `Written` had not reset; `Restarting` and a counting-down `AwaitingConfirm` had.
pub fn reset_phase_from(from: OpState) -> ResetPhase {
    match from {
        OpState::Planned | OpState::Written => ResetPhase::NotReached,
        _ => ResetPhase::Reached,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_display_is_read_only_and_requires_a_known_different_boot() {
        let (ops, baselines) = mklm_core::fixtures::schema_1_journal();
        let mut entry = mklm_core::Journal::parse(&ops, &baselines)
            .entries
            .remove(0);
        for state in [
            OpState::PendingReboot,
            OpState::RevertedPendingReboot,
            OpState::Confirmed,
            OpState::Conflict,
        ] {
            entry.state = state;
            let before = entry.to_json().unwrap();
            assert_eq!(shown_state(&entry, None), ShownState::Stored(state));
            assert_eq!(
                shown_state(&entry, Some(entry.boot_id)),
                ShownState::Stored(state)
            );
            let expected = match state {
                OpState::PendingReboot => ShownState::RestartedCheckDue,
                OpState::RevertedPendingReboot => ShownState::RevertedAndRestarted,
                _ => ShownState::Stored(state),
            };
            assert_eq!(
                shown_state(&entry, Some(mklm_core::BootId(entry.boot_id.0 + 1))),
                expected
            );
            assert_eq!(entry.to_json().unwrap(), before);
        }
    }

    #[test]
    fn phases_from_states() {
        assert_eq!(reset_phase_from(OpState::Planned), ResetPhase::NotReached);
        assert_eq!(reset_phase_from(OpState::Written), ResetPhase::NotReached);
        assert_eq!(reset_phase_from(OpState::Restarting), ResetPhase::Reached);
        assert_eq!(
            reset_phase_from(OpState::AwaitingConfirm),
            ResetPhase::Reached
        );
    }
}
