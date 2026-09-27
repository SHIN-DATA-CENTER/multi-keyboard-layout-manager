//! One request over a helper link (design m2 E.1, E.7, F.3; m3 A.2): send it, hand the helper's
//! events to the front end, pass the user's decisions back, and return how the request ended.
//!
//! The relay knows nothing about how things are shown. A [`Frontend`] does that: the CLI prints
//! text and reads answers from standard input, the GUI updates dialogs on its UI thread and takes
//! decisions from buttons. Both see the same [`SessionView`], which the relay keeps up to date
//! before it shows an event, so that the question a decision answers is always known here: a
//! decision that answers no open question is dropped.
//!
//! The relay works over any [`Link`], so tests play the helper's part.

use std::io;
use std::time::{Duration, Instant};

use mklm_core::{
    Decision, ErrorInfo, Event, ExpectedKeyboard, KeyboardType, OpId, OpState, OperationResult,
    PendingAction,
};
use mklm_ipc::{CallerMessage, HelperMessage, MESSAGE_TIMEOUT, Request};

/// One side of the pipe as the relay sees it.
pub trait Link {
    fn send(&mut self, message: &CallerMessage) -> Result<(), String>;
    /// The next message within `timeout`.
    fn recv(&mut self, timeout: Duration) -> Recv;
    /// The helper's exit code once its process has exited.
    fn helper_exit_code(&mut self) -> Option<u32>;
}

/// What [`Link::recv`] got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recv {
    Message(HelperMessage),
    Timeout,
    /// The pipe closed or a frame was malformed; the session is over.
    Closed(String),
}

/// How a request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    Finished(OperationResult),
    Failed(ErrorInfo),
    /// The pipe closed (or broke the protocol) before the answer; usually the helper died.
    Lost {
        exit_code: Option<u32>,
        detail: String,
        /// The operation was journaled (`Planned` arrived): recovery has something to do.
        planned: bool,
        /// A keep-or-revert countdown was running (design review C8).
        countdown: bool,
    },
    /// No frame for [`RelayConfig::silence_limit`] while the helper process still runs.
    Unresponsive,
    /// The front end asked to stop ([`Frontend::cancel_requested`]) at a point where leaving is
    /// safe: before a request that journals first ([`plans_first`]) was journaled (the helper
    /// then writes nothing, design m2 S4), or while a reconnect waits for keep or revert (the
    /// change stays `AwaitingConfirm` and is decided later). The link is dropped by the caller,
    /// which closes the pipe.
    Abandoned {
        /// True when the operation was journaled (a reconnect wait was left open).
        planned: bool,
    },
}

/// Timing of the relay loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayConfig {
    /// Longest wait for a frame before the front end is polled again.
    pub tick: Duration,
    /// Silence after which a running helper counts as wedged (`MESSAGE_TIMEOUT`; the helper sends
    /// a heartbeat every 10 s whatever it is doing, design review S7).
    pub silence_limit: Duration,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            tick: Duration::from_millis(200),
            silence_limit: MESSAGE_TIMEOUT,
        }
    }
}

/// A keyboard that came back after a reset ([`Event::KeyboardArrived`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrival {
    pub instance_id: String,
    /// What Raw Input reports now; `None` when it cannot tell.
    pub reported: Option<KeyboardType>,
    pub expected: KeyboardType,
}

impl Arrival {
    /// Raw Input reports the type the stored values predict.
    pub fn verified(&self) -> bool {
        self.reported == Some(self.expected)
    }
}

/// The question the helper is waiting on, if any.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Prompt {
    #[default]
    None,
    /// Keep-or-revert countdown after a live reset (design m2 D.2 a): no answer reverts.
    Countdown {
        op_id: OpId,
        seconds: u32,
        /// From the last [`Event::CountdownTick`] (starts at `seconds`).
        remaining: u32,
        /// Raw Input reports the intended type (plan gate G3).
        verified: bool,
    },
    /// Reconnect path (design m2 D.2 b): keep or revert, without a countdown; no answer leaves
    /// the change waiting (`AwaitingConfirm`).
    Reconnect {
        op_id: OpId,
        instance_ids: Vec<String>,
        /// [`Event::WaitingForReconnect`] events so far (one every 10 s).
        reminders: u32,
    },
    /// A decision was sent; the helper is acting on it.
    Answered { op_id: OpId, keep: bool },
}

/// What the helper has reported about the running request, for any front end.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionView {
    /// From [`Event::Planned`].
    pub op_id: Option<OpId>,
    /// The operation was journaled ([`Event::Planned`] arrived).
    pub planned: bool,
    /// How the change takes effect, from [`Event::Planned`].
    pub apply: Option<PendingAction>,
    /// Names and layouts of the keyboards involved, from [`Event::Planned`].
    pub keyboards: Vec<ExpectedKeyboard>,
    /// `(step, of)` of the last [`Event::StepWritten`].
    pub steps_written: Option<(usize, usize)>,
    /// The last flushed state ([`Event::StateChanged`]).
    pub state: Option<OpState>,
    /// The keyboard being reset now ([`Event::ResettingKeyboard`] until it arrives).
    pub resetting: Option<String>,
    /// Every keyboard that came back.
    pub arrivals: Vec<Arrival>,
    pub prompt: Prompt,
    /// A keep-or-revert countdown started in this session (design review C8).
    pub countdown_seen: bool,
    /// [`Event::RecoveryAssetsWritten`]: where the offline recovery files are.
    pub recovery_assets: Option<String>,
    /// [`Event::Warning`] messages, in order (English, from the engine).
    pub warnings: Vec<String>,
}

impl SessionView {
    /// Takes one event into account. Called by the relay before the front end sees the event.
    pub fn observe(&mut self, event: &Event) {
        match event {
            Event::Locked | Event::Heartbeat => {}
            Event::Planned {
                op_id,
                apply,
                keyboards,
                ..
            } => {
                self.op_id = Some(op_id.clone());
                self.planned = true;
                self.apply = *apply;
                self.keyboards = keyboards.clone();
            }
            Event::StateChanged { state, .. } => self.state = Some(*state),
            Event::StepWritten { step, of, .. } => self.steps_written = Some((*step, *of)),
            Event::ResettingKeyboard { instance_id } => {
                self.resetting = Some(instance_id.clone());
            }
            Event::KeyboardArrived {
                instance_id,
                reported,
                expected,
            } => {
                if self
                    .resetting
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(instance_id))
                {
                    self.resetting = None;
                }
                self.arrivals.push(Arrival {
                    instance_id: instance_id.clone(),
                    reported: *reported,
                    expected: *expected,
                });
            }
            Event::CountdownStarted {
                op_id,
                seconds,
                verified,
            } => {
                self.countdown_seen = true;
                if !matches!(self.prompt, Prompt::Answered { .. }) {
                    self.prompt = Prompt::Countdown {
                        op_id: op_id.clone(),
                        seconds: *seconds,
                        remaining: *seconds,
                        verified: *verified,
                    };
                }
            }
            Event::CountdownTick { remaining, .. } => {
                if let Prompt::Countdown {
                    remaining: left, ..
                } = &mut self.prompt
                {
                    *left = *remaining;
                }
            }
            Event::WaitingForReconnect {
                op_id,
                instance_ids,
            } => match &mut self.prompt {
                Prompt::Reconnect { reminders, .. } => *reminders += 1,
                Prompt::Answered { .. } => {}
                Prompt::None | Prompt::Countdown { .. } => {
                    self.prompt = Prompt::Reconnect {
                        op_id: op_id.clone(),
                        instance_ids: instance_ids.clone(),
                        reminders: 1,
                    };
                }
            },
            Event::RecoveryAssetsWritten { directory, .. } => {
                self.recovery_assets = Some(directory.clone());
            }
            Event::Warning { message } => self.warnings.push(message.clone()),
        }
    }

    /// True while a keep-or-revert question is open.
    pub fn question_open(&self) -> bool {
        matches!(
            self.prompt,
            Prompt::Countdown { .. } | Prompt::Reconnect { .. }
        )
    }

    /// True when `decision` answers the open question (same operation).
    pub fn accepts(&self, decision: &Decision) -> bool {
        let op_id = match decision {
            Decision::Keep { op_id } | Decision::RevertNow { op_id } => op_id,
        };
        match &self.prompt {
            Prompt::Countdown { op_id: asked, .. } | Prompt::Reconnect { op_id: asked, .. } => {
                asked == op_id
            }
            Prompt::None | Prompt::Answered { .. } => false,
        }
    }

    /// Records that `decision` was sent.
    pub fn decided(&mut self, decision: &Decision) {
        let (op_id, keep) = match decision {
            Decision::Keep { op_id } => (op_id.clone(), true),
            Decision::RevertNow { op_id } => (op_id.clone(), false),
        };
        self.prompt = Prompt::Answered { op_id, keep };
    }

    /// The display name of a keyboard of this request (its instance ID when unknown).
    pub fn display_name(&self, instance_id: &str) -> String {
        self.keyboards
            .iter()
            .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
            .map_or_else(|| instance_id.to_string(), |kb| kb.display_name.clone())
    }

    /// Every keyboard came back and Raw Input reports the intended type for each.
    pub fn all_arrivals_verified(&self) -> bool {
        !self.arrivals.is_empty() && self.arrivals.iter().all(Arrival::verified)
    }
}

/// Something the orchestrator tells the front end between sessions
/// ([`crate::orchestrator::Orchestrator::run`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A session is about to start (a new [`SessionView`] follows).
    Starting(SessionKind),
    /// The helper stopped before it answered (the pipe closed, or its process exited).
    HelperLost {
        exit: Option<crate::HelperExit>,
        detail: String,
    },
    /// The helper of a session was launched and passed the handshake (after the UAC prompt, if
    /// any); the request is sent next. Until then the session can be left at no cost: a helper
    /// started after the caller went away finds no pipe and writes nothing (design m3 A.4).
    Connected(SessionKind),
    /// The lost helper left an entry that needs recovery; a new helper is launched with
    /// `Recover` now (design m2 E.7, review C8). `countdown`: it died during a countdown.
    RecoveringAfterLoss { countdown: bool },
}

/// Which session of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// The request itself.
    Request,
    /// The recovery after a lost helper.
    Recovery,
}

/// How a request is shown and answered. Every method runs on the thread that runs the relay (the
/// GUI's session worker, the CLI's main thread); a GUI front end marshals to its UI thread.
pub trait Frontend {
    /// Shows one helper event; `view` already includes it. May return a decision taken at this
    /// point (the CLI's `--answer`). Heartbeats arrive here too.
    fn event(&mut self, event: &Event, view: &SessionView) -> io::Result<Option<Decision>>;

    /// Called about every [`RelayConfig::tick`]: a decision the user took since the last call
    /// (the GUI's Keep / Revert buttons, a line typed in the CLI), or a time-based one.
    fn poll(&mut self, view: &SessionView, now: Instant) -> io::Result<Option<Decision>>;

    /// The session is about to end (close an open countdown line, dismiss a dialog).
    fn finish(&mut self) -> io::Result<()>;

    /// The user wants out (the GUI quits, or its window closes during a session). See
    /// [`relay`] for what the relay does about it. Checked every tick.
    fn cancel_requested(&mut self) -> bool {
        false
    }

    /// Between sessions: see [`Notice`].
    fn notice(&mut self, notice: &Notice) -> io::Result<()> {
        let _ = notice;
        Ok(())
    }

    /// The helper was lost and the journal asks for recovery: may a new helper be launched now?
    /// For a non-elevated caller that is a second UAC prompt, so the GUI asks the user first
    /// (design m3 A.2.3, K.6); `false` leaves the recovery to the start-up check and the banner.
    /// The CLI keeps the default (recover at once, design m2 E.7). `countdown`: the helper died
    /// during a keep-or-revert countdown.
    fn confirm_recovery(&mut self, countdown: bool) -> io::Result<bool> {
        let _ = countdown;
        Ok(true)
    }
}

/// What the relay does when the front end asks to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelAction {
    /// A countdown is running: answer "revert now" and wait for the result (a quick, clean
    /// revert; leaving would revert too, through the helper's disconnect handling, C8).
    Revert(OpId),
    /// A request that journals first has not been journaled yet, or a reconnect waits for keep
    /// or revert: leave now.
    Abandon,
    /// The helper is writing, resetting or reverting — or may be, for a request that writes
    /// without announcing a plan: stay until it answers (leaving would not stop it; it finishes
    /// by itself, design m2 E.7, and its result would be shown to no one).
    Wait,
}

/// True for the requests the engine journals (and announces with [`Event::Planned`]) before it
/// writes anything, and that check for a caller that left before writing (design m2 S4):
/// `SetLayout`, `Migrate`, `RestoreBaseline` (and the planned `CleanupValues`, design m3 A.5).
/// `Revert`, `Undo`, `Recover`, `Confirm` and `ResolveConflict` work on existing entries and
/// may write or reset a keyboard without a `Planned` event.
pub fn plans_first(request: &Request) -> bool {
    match request {
        Request::SetLayout(_) | Request::Migrate(_) | Request::RestoreBaseline(_) => true,
        Request::Revert { .. }
        | Request::Confirm { .. }
        | Request::Recover { .. }
        | Request::Undo { .. }
        | Request::ResolveConflict(_) => false,
    }
}

/// The [`CancelAction`] for `request` in the session as `view` shows it (design m3 A.2.2).
pub fn cancel_action(view: &SessionView, request: &Request) -> CancelAction {
    match &view.prompt {
        Prompt::Countdown { op_id, .. } => CancelAction::Revert(op_id.clone()),
        Prompt::Reconnect { .. } => CancelAction::Abandon,
        Prompt::None if !view.planned && plans_first(request) => CancelAction::Abandon,
        Prompt::None | Prompt::Answered { .. } => CancelAction::Wait,
    }
}

/// Sends `request` and relays until the result.
///
/// Decisions from the front end are sent when they answer the open question ([`SessionView::accepts`]);
/// others are dropped. The helper's heartbeats keep the silence limit from expiring while the
/// engine works. When [`Frontend::cancel_requested`] turns true, the relay acts once as
/// [`cancel_action`] says.
pub fn relay(
    link: &mut dyn Link,
    request: Request,
    frontend: &mut dyn Frontend,
    config: RelayConfig,
) -> io::Result<SessionEnd> {
    let mut view = SessionView::default();
    if let Err(detail) = link.send(&CallerMessage::Request(request.clone())) {
        return Ok(lost(link, &view, detail));
    }
    let mut last_frame = Instant::now();
    let mut cancel_handled = false;
    loop {
        if !cancel_handled && frontend.cancel_requested() {
            match cancel_action(&view, &request) {
                CancelAction::Revert(op_id) => {
                    cancel_handled = true;
                    let decision = Decision::RevertNow { op_id };
                    if let Some(end) = deliver(link, frontend, &mut view, decision)? {
                        return Ok(end);
                    }
                }
                CancelAction::Abandon => {
                    frontend.finish()?;
                    return Ok(SessionEnd::Abandoned {
                        planned: view.planned,
                    });
                }
                // Asked again every tick: a countdown that starts later is reverted at once.
                CancelAction::Wait => {}
            }
        }
        if let Some(decision) = frontend.poll(&view, Instant::now())?
            && let Some(end) = deliver(link, frontend, &mut view, decision)?
        {
            return Ok(end);
        }
        match link.recv(config.tick) {
            Recv::Message(message) => {
                last_frame = Instant::now();
                match message {
                    HelperMessage::Event(event) => {
                        view.observe(&event);
                        if let Some(decision) = frontend.event(&event, &view)?
                            && let Some(end) = deliver(link, frontend, &mut view, decision)?
                        {
                            return Ok(end);
                        }
                    }
                    HelperMessage::Result(result) => {
                        frontend.finish()?;
                        return Ok(SessionEnd::Finished(result));
                    }
                    HelperMessage::Error(info) => {
                        frontend.finish()?;
                        return Ok(SessionEnd::Failed(info));
                    }
                    HelperMessage::Hello(_) => {
                        frontend.finish()?;
                        return Ok(lost(link, &view, "unexpected hello frame".to_string()));
                    }
                }
            }
            Recv::Timeout => {
                // The pipe normally breaks when the helper exits; its process handle tells for
                // sure, without waiting for the silence limit (design m2 E.7).
                if link.helper_exit_code().is_some() {
                    frontend.finish()?;
                    return Ok(lost(link, &view, "the helper exited".to_string()));
                }
                if last_frame.elapsed() >= config.silence_limit {
                    frontend.finish()?;
                    return Ok(SessionEnd::Unresponsive);
                }
            }
            Recv::Closed(detail) => {
                frontend.finish()?;
                return Ok(lost(link, &view, detail));
            }
        }
    }
}

/// Sends a decision that answers the open question; `Some(end)` when the link broke.
fn deliver(
    link: &mut dyn Link,
    frontend: &mut dyn Frontend,
    view: &mut SessionView,
    decision: Decision,
) -> io::Result<Option<SessionEnd>> {
    if !view.accepts(&decision) {
        return Ok(None);
    }
    view.decided(&decision);
    match link.send(&CallerMessage::Decision(decision)) {
        Ok(()) => Ok(None),
        Err(detail) => {
            frontend.finish()?;
            Ok(Some(lost(link, view, detail)))
        }
    }
}

fn lost(link: &mut dyn Link, view: &SessionView, detail: String) -> SessionEnd {
    SessionEnd::Lost {
        exit_code: link.helper_exit_code(),
        detail,
        planned: view.planned,
        countdown: view.countdown_seen,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::VecDeque;

    use mklm_core::{
        FailureReason, LayoutTable, Outcome, PlanStep, PlannedWrite, WriteTarget, value_names,
    };

    use super::*;

    pub const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";

    pub fn op() -> OpId {
        OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap()
    }

    pub fn result(outcome: Outcome, failure: Option<FailureReason>) -> OperationResult {
        OperationResult {
            op_id: Some(op()),
            outcome,
            failure,
            pending_action: None,
            conflicts: Vec::new(),
            inv_ps2_violation: None,
            recovered: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn event(event: Event) -> HelperMessage {
        HelperMessage::Event(event)
    }

    /// The events of a USB `set` up to the countdown (design m2 D.2 a).
    pub fn live_reset_events() -> Vec<HelperMessage> {
        vec![
            event(Event::Locked),
            event(Event::Planned {
                op_id: op(),
                steps: vec![PlanStep {
                    target: WriteTarget::Device {
                        instance_id: KEYCHRON.into(),
                    },
                    writes: vec![
                        PlannedWrite::set(value_names::HID_TYPE, 7),
                        PlannedWrite::set(value_names::HID_SUBTYPE, 2),
                    ],
                }],
                apply: Some(PendingAction::ResetKeyboard),
                keyboards: vec![ExpectedKeyboard {
                    instance_id: KEYCHRON.into(),
                    display_name: "Keychron Receiver".into(),
                    expected_type: Some(KeyboardType::JIS),
                    layout_after: Some(LayoutTable::Jis),
                    changes: true,
                }],
            }),
            event(Event::StepWritten {
                op_id: op(),
                step: 1,
                of: 1,
            }),
            event(Event::StateChanged {
                op_id: op(),
                state: OpState::Written,
            }),
            event(Event::ResettingKeyboard {
                instance_id: KEYCHRON.into(),
            }),
            event(Event::Heartbeat),
            event(Event::KeyboardArrived {
                instance_id: KEYCHRON.into(),
                reported: Some(KeyboardType::JIS),
                expected: KeyboardType::JIS,
            }),
            event(Event::CountdownStarted {
                op_id: op(),
                seconds: 20,
                verified: true,
            }),
        ]
    }

    /// Plays the helper: queued frames, then countdown ticks; a decision (or the ticks running
    /// out) produces the engine's answer.
    #[derive(Debug)]
    pub struct FakeHelper {
        pub frames: VecDeque<HelperMessage>,
        pub sent: Vec<CallerMessage>,
        pub ticks: VecDeque<HelperMessage>,
        pub answered: bool,
        pub closed: bool,
        pub exit_code: Option<u32>,
    }

    impl FakeHelper {
        pub fn new(frames: Vec<HelperMessage>) -> Self {
            Self {
                frames: frames.into(),
                sent: Vec::new(),
                ticks: (1..=20)
                    .rev()
                    .map(|remaining| {
                        event(Event::CountdownTick {
                            op_id: op(),
                            remaining,
                        })
                    })
                    .collect(),
                answered: false,
                closed: false,
                exit_code: None,
            }
        }

        pub fn decisions(&self) -> Vec<&Decision> {
            self.sent
                .iter()
                .filter_map(|m| match m {
                    CallerMessage::Decision(d) => Some(d),
                    _ => None,
                })
                .collect()
        }
    }

    impl Link for FakeHelper {
        fn send(&mut self, message: &CallerMessage) -> Result<(), String> {
            if self.closed && self.frames.is_empty() {
                return Err("closed".into());
            }
            if let CallerMessage::Decision(decision) = message {
                self.answered = true;
                self.ticks.clear();
                self.frames.push_back(HelperMessage::Result(match decision {
                    Decision::Keep { .. } => result(Outcome::Confirmed, None),
                    Decision::RevertNow { .. } => result(Outcome::Reverted, None),
                }));
            }
            self.sent.push(message.clone());
            Ok(())
        }

        fn recv(&mut self, _timeout: Duration) -> Recv {
            if let Some(frame) = self.frames.pop_front() {
                return Recv::Message(frame);
            }
            if self.closed {
                return Recv::Closed("the helper closed the pipe".into());
            }
            if let Some(tick) = self.ticks.pop_front() {
                return Recv::Message(tick);
            }
            if !self.answered {
                self.answered = true;
                return Recv::Message(HelperMessage::Result(result(
                    Outcome::Reverted,
                    Some(FailureReason::CountdownExpired),
                )));
            }
            Recv::Timeout
        }

        fn helper_exit_code(&mut self) -> Option<u32> {
            self.exit_code
        }
    }

    /// A front end that records what it saw and answers from a script.
    #[derive(Debug, Default)]
    pub struct ScriptedFrontend {
        /// Decision to give once a question is open, after this many polls.
        pub answer_after_polls: Option<(u32, bool)>,
        /// Ask to cancel on the first poll after this many events.
        pub cancel_after_events: Option<usize>,
        /// A decision to give for any question (even a wrong operation).
        pub stray: Option<Decision>,
        /// Answer "later" when asked whether to recover after a lost helper.
        pub decline_recovery: bool,
        pub polls: u32,
        pub events: Vec<Event>,
        pub finished: u32,
        pub notices: Vec<Notice>,
        /// The `countdown` flag of every recovery question.
        pub recovery_questions: Vec<bool>,
    }

    impl Frontend for ScriptedFrontend {
        fn event(&mut self, event: &Event, _view: &SessionView) -> io::Result<Option<Decision>> {
            self.events.push(event.clone());
            Ok(None)
        }

        fn poll(&mut self, view: &SessionView, _now: Instant) -> io::Result<Option<Decision>> {
            if let Some(stray) = self.stray.take() {
                return Ok(Some(stray));
            }
            if !view.question_open() {
                return Ok(None);
            }
            self.polls += 1;
            match self.answer_after_polls {
                Some((after, keep)) if self.polls > after => {
                    let op_id = view.op_id.clone().unwrap();
                    Ok(Some(if keep {
                        Decision::Keep { op_id }
                    } else {
                        Decision::RevertNow { op_id }
                    }))
                }
                _ => Ok(None),
            }
        }

        fn finish(&mut self) -> io::Result<()> {
            self.finished += 1;
            Ok(())
        }

        fn cancel_requested(&mut self) -> bool {
            self.cancel_after_events
                .is_some_and(|after| self.events.len() >= after)
        }

        fn notice(&mut self, notice: &Notice) -> io::Result<()> {
            self.notices.push(notice.clone());
            Ok(())
        }

        fn confirm_recovery(&mut self, countdown: bool) -> io::Result<bool> {
            self.recovery_questions.push(countdown);
            Ok(!self.decline_recovery)
        }
    }

    pub fn fast() -> RelayConfig {
        RelayConfig {
            tick: Duration::ZERO,
            silence_limit: Duration::from_millis(50),
        }
    }

    fn request() -> Request {
        Request::Recover {
            apply: Default::default(),
        }
    }

    fn set_request() -> Request {
        Request::SetLayout(mklm_ipc::SetLayoutRequest {
            instance_id: KEYCHRON.into(),
            layout: mklm_core::LayoutChoice::Jis,
            apply: Default::default(),
            expected: None,
        })
    }

    #[test]
    fn the_view_follows_a_live_reset() {
        let mut view = SessionView::default();
        for message in live_reset_events() {
            if let HelperMessage::Event(event) = message {
                view.observe(&event);
            }
        }
        assert!(view.planned && view.countdown_seen);
        assert_eq!(view.op_id, Some(op()));
        assert_eq!(view.apply, Some(PendingAction::ResetKeyboard));
        assert_eq!(view.steps_written, Some((1, 1)));
        assert_eq!(view.state, Some(OpState::Written));
        assert_eq!(view.resetting, None);
        assert!(view.all_arrivals_verified());
        assert_eq!(view.display_name(KEYCHRON), "Keychron Receiver");
        assert!(matches!(
            view.prompt,
            Prompt::Countdown {
                remaining: 20,
                verified: true,
                ..
            }
        ));
        view.observe(&Event::CountdownTick {
            op_id: op(),
            remaining: 7,
        });
        assert!(matches!(
            view.prompt,
            Prompt::Countdown { remaining: 7, .. }
        ));
        assert!(view.accepts(&Decision::Keep { op_id: op() }));
        let other = OpId::parse("11111111-2222-4333-8444-555555555555").unwrap();
        assert!(!view.accepts(&Decision::Keep { op_id: other }));
        view.decided(&Decision::Keep { op_id: op() });
        assert!(!view.question_open());
        assert!(!view.accepts(&Decision::RevertNow { op_id: op() }));
    }

    #[test]
    fn a_button_decision_is_sent_once_the_question_is_open() {
        let mut helper = FakeHelper::new(live_reset_events());
        let mut frontend = ScriptedFrontend {
            answer_after_polls: Some((3, true)),
            ..Default::default()
        };
        let end = relay(&mut helper, request(), &mut frontend, fast()).unwrap();
        assert_eq!(end, SessionEnd::Finished(result(Outcome::Confirmed, None)));
        assert_eq!(helper.decisions(), vec![&Decision::Keep { op_id: op() }]);
        assert_eq!(frontend.finished, 1);
    }

    #[test]
    fn stray_decisions_are_dropped() {
        let mut helper = FakeHelper::new(live_reset_events());
        let mut frontend = ScriptedFrontend {
            stray: Some(Decision::Keep { op_id: op() }),
            ..Default::default()
        };
        let end = relay(&mut helper, request(), &mut frontend, fast()).unwrap();
        // Given before any question: dropped, so the countdown runs out.
        assert!(helper.decisions().is_empty());
        assert_eq!(
            end,
            SessionEnd::Finished(result(
                Outcome::Reverted,
                Some(FailureReason::CountdownExpired)
            ))
        );
    }

    #[test]
    fn cancelling_during_the_countdown_reverts_now() {
        let mut helper = FakeHelper::new(live_reset_events());
        let mut frontend = ScriptedFrontend {
            cancel_after_events: Some(8),
            ..Default::default()
        };
        let end = relay(&mut helper, request(), &mut frontend, fast()).unwrap();
        assert_eq!(
            helper.decisions(),
            vec![&Decision::RevertNow { op_id: op() }]
        );
        assert_eq!(end, SessionEnd::Finished(result(Outcome::Reverted, None)));
    }

    #[test]
    fn cancelling_before_the_plan_abandons_and_while_writing_waits() {
        let mut helper = FakeHelper::new(live_reset_events());
        let mut frontend = ScriptedFrontend {
            cancel_after_events: Some(0),
            ..Default::default()
        };
        let end = relay(&mut helper, set_request(), &mut frontend, fast()).unwrap();
        assert_eq!(end, SessionEnd::Abandoned { planned: false });

        // After `Planned` the relay stays while the helper writes and resets; the countdown that
        // follows is answered with "revert now" at once.
        let mut helper = FakeHelper::new(live_reset_events());
        let mut frontend = ScriptedFrontend {
            cancel_after_events: Some(3),
            ..Default::default()
        };
        let end = relay(&mut helper, set_request(), &mut frontend, fast()).unwrap();
        assert_eq!(
            helper.decisions(),
            vec![&Decision::RevertNow { op_id: op() }]
        );
        assert_eq!(end, SessionEnd::Finished(result(Outcome::Reverted, None)));
    }

    #[test]
    fn requests_without_a_plan_event_are_waited_for() {
        // A revert writes and resets without `Planned` (design m3 A.2.2): leaving at once would
        // hide its result (a conflict, a restart it needs) from everyone.
        let revert = Request::Revert {
            op_id: op(),
            apply: Default::default(),
        };
        assert!(!plans_first(&revert) && !plans_first(&request()));
        assert!(plans_first(&set_request()));
        assert_eq!(
            cancel_action(&SessionView::default(), &revert),
            CancelAction::Wait
        );
        let reverted = result(Outcome::Reverted, None);
        let mut helper = FakeHelper::new(vec![
            event(Event::Locked),
            event(Event::StateChanged {
                op_id: op(),
                state: OpState::RevertPending,
            }),
            event(Event::ResettingKeyboard {
                instance_id: KEYCHRON.into(),
            }),
            HelperMessage::Result(reverted.clone()),
        ]);
        helper.ticks.clear();
        helper.answered = true;
        let mut frontend = ScriptedFrontend {
            cancel_after_events: Some(0),
            ..Default::default()
        };
        let end = relay(&mut helper, revert, &mut frontend, fast()).unwrap();
        assert_eq!(end, SessionEnd::Finished(reverted));
        let undo = Request::Undo {
            apply: Default::default(),
        };
        assert_eq!(
            cancel_action(&SessionView::default(), &undo),
            CancelAction::Wait
        );
    }

    #[test]
    fn a_reconnect_wait_can_be_left_open() {
        let mut frames = live_reset_events();
        frames.truncate(4);
        frames.push(event(Event::WaitingForReconnect {
            op_id: op(),
            instance_ids: vec![KEYCHRON.into()],
        }));
        let mut helper = FakeHelper::new(frames);
        helper.ticks.clear();
        let mut frontend = ScriptedFrontend {
            cancel_after_events: Some(5),
            ..Default::default()
        };
        let end = relay(&mut helper, request(), &mut frontend, fast()).unwrap();
        assert_eq!(end, SessionEnd::Abandoned { planned: true });
        assert!(helper.decisions().is_empty());
    }

    #[test]
    fn a_lost_helper_reports_what_was_under_way() {
        let mut helper = FakeHelper::new(live_reset_events());
        helper.ticks.truncate(3);
        helper.answered = true;
        helper.closed = true;
        helper.exit_code = Some(1);
        let mut frontend = ScriptedFrontend::default();
        let end = relay(&mut helper, request(), &mut frontend, fast()).unwrap();
        assert_eq!(
            end,
            SessionEnd::Lost {
                exit_code: Some(1),
                detail: "the helper closed the pipe".into(),
                planned: true,
                countdown: true,
            }
        );
        // Silence past the limit while the process runs.
        let mut helper = FakeHelper::new(Vec::new());
        helper.answered = true;
        helper.ticks.clear();
        let end = relay(&mut helper, request(), &mut frontend, fast()).unwrap();
        assert_eq!(end, SessionEnd::Unresponsive);
    }
}
