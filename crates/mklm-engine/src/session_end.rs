//! What the process that runs the engine does when the Windows session ends (design m3 A.5
//! WP-E3, F.5, J.18).
//!
//! A sign-out, shutdown or restart may terminate the helper while a keep-or-revert countdown runs,
//! before the caller's pipe closes (the GUI is terminated in the same session end), so the
//! "disconnect means revert" rule (design m2 E.7) may never fire. The helper therefore watches the
//! session itself (`mklm_win::session_end`) and, on `WM_QUERYENDSESSION`, reverts a running
//! countdown exactly as if the caller had answered `RevertNow`; the caller's own answer, if it
//! came first, stands. An operation still in flight before its countdown (its values being
//! written, or its keyboard reset) is waited for too: the engine's cancellation points roll it
//! back, or its countdown reverts as soon as it starts. A change waiting for a reconnect stays in
//! `AwaitingConfirm` (nothing is reverted without a countdown, design m2 D.2 b). The last resort
//! stays the recovery at the next start (the RunOnce entry the GUI registers).
//!
//! [`SessionEnd`] is the state shared with the window thread; [`SessionEndSink`] wraps the
//! request's [`EventSink`] to follow the countdown and to answer for the user when the session
//! ends. Pure Rust, tested with the in-memory fakes (the window itself is `mklm_win`'s).

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mklm_core::{Decision, Event, OpId, OpState};

use crate::sink::{DecisionPoll, EventSink};

/// While a countdown runs, the wait for a decision is cut into slices of this length, so that a
/// session end reaches it within a fraction of a second rather than at the next one-second tick.
pub const COUNTDOWN_SLICE: Duration = Duration::from_millis(100);

/// How long the session end looks for a decision the caller already sent before it answers
/// `RevertNow` itself: long enough to take a frame that has arrived, too short to delay the
/// revert.
const ALREADY_SENT_POLL: Duration = Duration::from_millis(1);

/// Where the request stands, as far as a session end cares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Phase {
    /// Nothing is in flight and no countdown runs.
    #[default]
    Idle,
    /// The operation is in flight before any countdown (`Planned`, `Written`, `Restarting`, or a
    /// revert's `RevertPending`): its values are being written or its keyboard reset. While the
    /// session ends, the engine's cancellation points roll it back, or its countdown reverts at
    /// the first wait; the window waits (bounded) for either. It lasts through the
    /// `AwaitingConfirm` that precedes `CountdownStarted`, and ends with a terminal state or with
    /// a wait for a decision without a countdown (a reconnect wait, design m2 D.2 b).
    InFlight(OpId),
    /// A countdown runs for this operation.
    Counting(OpId),
    /// Its revert started (`RevertPending` flushed).
    Reverting(OpId),
    /// The revert's values are written and flushed (its last step); what is left is re-applying
    /// the old layout with a reset and closing the entry.
    Restored(OpId),
}

impl Phase {
    fn op(&self) -> Option<&OpId> {
        match self {
            Phase::Idle => None,
            Phase::InFlight(op)
            | Phase::Counting(op)
            | Phase::Reverting(op)
            | Phase::Restored(op) => Some(op),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    /// `WM_QUERYENDSESSION` arrived and no `WM_ENDSESSION(FALSE)` since.
    ending: bool,
    phase: Phase,
}

/// Session-end state shared by the window thread (`query_end_session`, `end_session`) and the
/// request thread ([`SessionEndSink`]).
#[derive(Debug, Default)]
pub struct SessionEnd {
    state: Mutex<State>,
    changed: Condvar,
}

impl SessionEnd {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking request thread must not disable the session-end handling.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until `done(phase)` holds or `wait` has passed; true when it holds.
    fn wait_until(
        &self,
        mut state: MutexGuard<'_, State>,
        wait: Duration,
        done: impl Fn(&Phase) -> bool,
    ) -> bool {
        let deadline = Instant::now().checked_add(wait);
        loop {
            if done(&state.phase) {
                return true;
            }
            let left = match deadline {
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                None => wait,
            };
            if left.is_zero() {
                return false;
            }
            state = match self.changed.wait_timeout(state, left) {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }

    /// `WM_QUERYENDSESSION`: the session may end. A running countdown reverts at once, as on
    /// `RevertNow` (unless the caller answered first), and new writes are not started. An
    /// operation in flight before its countdown (writing, or resetting its keyboard) is rolled
    /// back at the engine's next cancellation point, or its countdown reverts as soon as it
    /// starts. Waits up to `wait` for the values to be back (the reverted values written and
    /// flushed, or the operation closed); true when nothing is left that still has to put values
    /// back. The window answers `TRUE` either way.
    pub fn query_end_session(&self, wait: Duration) -> bool {
        let mut state = self.lock();
        state.ending = true;
        self.changed.notify_all();
        self.wait_until(state, wait, |phase| {
            matches!(phase, Phase::Idle | Phase::Restored(_))
        })
    }

    /// `WM_ENDSESSION`. `ending` true: the process may be terminated once this returns; waits up
    /// to `wait` for a revert, or an operation in flight, to finish (the old layout re-applied,
    /// the entry closed). False: the session goes on; writes may start again. True when nothing
    /// is in progress.
    pub fn end_session(&self, ending: bool, wait: Duration) -> bool {
        let mut state = self.lock();
        state.ending = ending;
        self.changed.notify_all();
        if !ending {
            return true;
        }
        self.wait_until(state, wait, |phase| *phase == Phase::Idle)
    }

    /// True between `WM_QUERYENDSESSION` and a `WM_ENDSESSION(FALSE)`.
    pub fn is_ending(&self) -> bool {
        self.lock().ending
    }

    /// True while a countdown runs (not yet kept or reverted).
    pub fn is_counting_down(&self) -> bool {
        matches!(self.lock().phase, Phase::Counting(_))
    }

    /// Follows the request's events.
    fn observe(&self, event: &Event) {
        let mut state = self.lock();
        let next = match (event, &state.phase) {
            (Event::Planned { op_id, .. }, Phase::Idle) => Some(Phase::InFlight(op_id.clone())),
            (Event::StateChanged { op_id, state: to }, Phase::Idle) if to.is_in_flight() => {
                Some(Phase::InFlight(op_id.clone()))
            }
            (Event::StateChanged { op_id, state: to }, Phase::InFlight(op)) if op == op_id => {
                (!to.is_in_flight() && *to != OpState::AwaitingConfirm).then_some(Phase::Idle)
            }
            (Event::WaitingForReconnect { op_id, .. }, Phase::InFlight(op)) if op == op_id => {
                Some(Phase::Idle)
            }
            (Event::CountdownStarted { op_id, .. }, _) => Some(Phase::Counting(op_id.clone())),
            (Event::StateChanged { op_id, state: to }, phase) if phase.op() == Some(op_id) => {
                match (to, phase) {
                    (OpState::RevertPending, Phase::Counting(_) | Phase::Reverting(_)) => {
                        Some(Phase::Reverting(op_id.clone()))
                    }
                    (OpState::RevertPending, _) => None,
                    _ => Some(Phase::Idle),
                }
            }
            (Event::StepWritten { op_id, step, of }, Phase::Reverting(reverting))
                if reverting == op_id && step >= of =>
            {
                Some(Phase::Restored(op_id.clone()))
            }
            _ => None,
        };
        if let Some(next) = next {
            state.phase = next;
            self.changed.notify_all();
        }
    }

    /// The request waits for a decision without a countdown (a reconnect wait, or the wait for a
    /// later decision): an operation in flight is not any more, and the session end does not
    /// wait for it (nothing is reverted without a countdown, design m2 D.2 b).
    fn waits_without_countdown(&self) {
        let mut state = self.lock();
        if matches!(state.phase, Phase::InFlight(_)) {
            state.phase = Phase::Idle;
            self.changed.notify_all();
        }
    }

    /// True when the session is ending and a countdown runs, i.e. when `revert_due` would answer.
    fn revert_is_due(&self) -> bool {
        let state = self.lock();
        state.ending && matches!(state.phase, Phase::Counting(_))
    }

    /// The operation whose countdown must revert now: the session is ending and a countdown runs.
    /// Taken once (the countdown is then reverting).
    fn revert_due(&self) -> Option<OpId> {
        let mut state = self.lock();
        match &state.phase {
            Phase::Counting(op) if state.ending => {
                let op = op.clone();
                state.phase = Phase::Reverting(op.clone());
                self.changed.notify_all();
                Some(op)
            }
            _ => None,
        }
    }

    /// The request ended (whatever its result): nothing is in progress any more.
    fn finish(&self) {
        let mut state = self.lock();
        state.phase = Phase::Idle;
        self.changed.notify_all();
    }
}

/// Wraps the sink of one request (the helper's pipe sink) with the session-end rules:
/// - events go through unchanged, and tell [`SessionEnd`] where the countdown stands;
/// - while the session is ending, `check_cancelled` is true: nothing new is journaled and no
///   keyboard is reset (the engine's two cancellation points, design m2 S4);
/// - during a countdown, the wait for a decision is sliced ([`COUNTDOWN_SLICE`]) and answers
///   `RevertNow` for the running operation as soon as the session ends, unless the caller's own
///   decision has already arrived;
/// - dropping it (the request returned) marks the request finished.
pub struct SessionEndSink<'a> {
    inner: &'a mut dyn EventSink,
    guard: &'a SessionEnd,
}

impl std::fmt::Debug for SessionEndSink<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEndSink")
            .field("guard", &self.guard)
            .finish_non_exhaustive()
    }
}

impl<'a> SessionEndSink<'a> {
    pub fn new(inner: &'a mut dyn EventSink, guard: &'a SessionEnd) -> Self {
        Self { inner, guard }
    }

    /// The answer once the session end is due to revert the running countdown: a decision the
    /// caller already sent (a frame that has arrived, or one the inner sink queued) comes first
    /// (design m3 A.5: "呼び出し元が先に送っていれば何もしない"); otherwise `RevertNow`. `None`
    /// while no revert is due.
    fn answer_or_revert(&mut self) -> Option<DecisionPoll> {
        if !self.guard.revert_is_due() {
            return None;
        }
        if let answer @ DecisionPoll::Decided(_) = self.inner.wait_decision(ALREADY_SENT_POLL) {
            return Some(answer);
        }
        self.guard
            .revert_due()
            .map(|op_id| DecisionPoll::Decided(Decision::RevertNow { op_id }))
    }
}

impl Drop for SessionEndSink<'_> {
    fn drop(&mut self) {
        self.guard.finish();
    }
}

impl EventSink for SessionEndSink<'_> {
    fn event(&mut self, event: &Event) {
        self.guard.observe(event);
        self.inner.event(event);
    }

    fn check_cancelled(&mut self) -> bool {
        self.guard.is_ending() || self.inner.check_cancelled()
    }

    fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
        if let Some(answer) = self.answer_or_revert() {
            return answer;
        }
        if !self.guard.is_counting_down() {
            // A reconnect wait (or any other): the session end changes nothing there.
            self.guard.waits_without_countdown();
            return self.inner.wait_decision(timeout);
        }
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return self.inner.wait_decision(timeout);
        };
        loop {
            let slice = deadline
                .saturating_duration_since(Instant::now())
                .min(COUNTDOWN_SLICE);
            let started = Instant::now();
            match self.inner.wait_decision(slice) {
                DecisionPoll::NoDecision => {}
                answer => return answer,
            }
            if let Some(answer) = self.answer_or_revert() {
                return answer;
            }
            // Past the deadline, or an inner sink that does not block (a test sink needs no
            // clock): the wait is over; the countdown asks again on its next tick.
            if Instant::now() >= deadline || started.elapsed() < slice {
                return DecisionPoll::NoDecision;
            }
        }
    }
}
