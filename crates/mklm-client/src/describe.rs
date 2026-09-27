//! Facts both front ends need to word things correctly (design m3 G.2).
//!
//! M2 real-machine test R5: after the helper was killed right after its first write (before any
//! keyboard reset), recovery rolled back with `LiveResetUnconfirmed`, and the CLI said "the change
//! was never kept after the keyboard reset", although no reset had happened. The reason stays
//! (the journal's wording is part of its schema); the text now depends on whether the reset was
//! reached, which the entry's history (or a recovered operation's `from` state) tells.

use mklm_core::{JournalEntry, OpState};

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
