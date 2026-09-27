//! Where the engine reports progress, learns that its caller went away, and gets the user's
//! keep/revert decision.
//!
//! The helper forwards to the pipe; the in-process CLI fallback prints to the console and reads
//! stdin; the silent mode (uninstall) uses [`NullSink`].

use std::time::Duration;

use mklm_core::{Decision, Event};

/// Result of waiting for a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionPoll {
    Decided(Decision),
    /// Nothing within the timeout.
    NoDecision,
    /// The caller is gone (pipe closed). During a countdown this means revert (plan 2.3, E.7).
    Disconnected,
}

/// Progress, cancellation and decisions.
pub trait EventSink {
    fn event(&mut self, event: &Event);

    /// Non-blocking: true once the caller is gone (pipe closed or `Bye` received, Ctrl+C in the
    /// in-process fallback). The engine asks at two points (design review S4, C13): right before
    /// it journals `Planned` (then it writes nothing and fails with `Cancelled`), and right before
    /// `Restarting` (then it rolls back without resetting). Between those points it finishes the
    /// writes it started.
    fn check_cancelled(&mut self) -> bool;

    /// Waits for a [`Decision`]. **Contract**: blocks until a decision arrives, the caller goes
    /// away, or `timeout` has passed; it never returns `NoDecision` early. The countdown calls it
    /// with 1 s per tick and also bounds the whole countdown with `Host::monotonic`, so a sink that
    /// blocks too long cannot stretch it (design review C18).
    fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll;
}

/// Discards events, is never cancelled and never decides: it sleeps for the whole `timeout` (the
/// contract above), then answers `NoDecision`. A countdown therefore expires in real time and
/// reverts; a confirmation wait ends with the operation left in `AwaitingConfirm`. Tests use
/// `memory::ScriptedSink`, which needs no clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullSink;

impl EventSink for NullSink {
    fn event(&mut self, _event: &Event) {}

    fn check_cancelled(&mut self) -> bool {
        false
    }

    fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
        std::thread::sleep(timeout);
        DecisionPoll::NoDecision
    }
}
