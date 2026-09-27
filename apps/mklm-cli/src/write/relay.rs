//! The caller's side of one request (design E.1, E.7, F.3): send it, show the helper's events,
//! turn the user's answers into decisions, and return how the request ended.
//!
//! [`Presenter`] turns events into text and answers into [`Decision`]s; the pipe relay
//! ([`relay`]) and the in-process fallback's console sink both drive it. The relay works over any
//! [`Link`], so that tests can play the helper's part and type the user's answers
//! ([`super::input::ScriptedInput`]).

use std::io::{self, Write};
use std::time::{Duration, Instant};

use mklm_core::{
    Decision, ErrorInfo, Event, ExpectedKeyboard, KeyboardType, OpId, OpState, OperationResult,
};
use mklm_ipc::{CallerMessage, HelperMessage, MESSAGE_TIMEOUT, Request};

use super::AnswerArg;
use super::input::{Input, Line, YesNo, yes_no};

/// With `--answer keep` on the reconnect path: how long to wait for the keyboard before
/// answering revert (the engine waits as long, design D.2 b).
const RECONNECT_ANSWER_WAIT: Duration = Duration::from_secs(180);
/// Redirected output shows the countdown every this many seconds (design F.3).
const REDIRECTED_TICK_EVERY: u32 = 5;
/// Reconnect path: a "still waiting" line after this many `WaitingForReconnect` events (10 s each).
const RECONNECT_REMINDER_EVERY: u32 = 6;

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
}

/// Timing of the relay loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayConfig {
    /// Longest wait for a frame before the user's input is looked at again.
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

/// Sends `request` and relays until the result. Decisions are sent as the user (or `--answer`)
/// gives them; the helper's heartbeats keep the silence limit from expiring while the engine
/// works.
pub fn relay(
    link: &mut dyn Link,
    request: Request,
    presenter: &mut Presenter,
    input: &mut dyn Input,
    out: &mut dyn Write,
    config: RelayConfig,
) -> io::Result<SessionEnd> {
    if let Err(detail) = link.send(&CallerMessage::Request(request)) {
        return Ok(presenter.lost(link, detail));
    }
    let mut last_frame = Instant::now();
    loop {
        if presenter.wants_input()
            && let Some(line) = input.next_line(Some(Duration::ZERO))
            && let Some(decision) = presenter.line(line, out)?
            && let Err(detail) = link.send(&CallerMessage::Decision(decision))
        {
            presenter.finish(out)?;
            return Ok(presenter.lost(link, detail));
        }
        if let Some(decision) = presenter.tick(Instant::now(), out)?
            && let Err(detail) = link.send(&CallerMessage::Decision(decision))
        {
            presenter.finish(out)?;
            return Ok(presenter.lost(link, detail));
        }
        match link.recv(config.tick) {
            Recv::Message(message) => {
                last_frame = Instant::now();
                match message {
                    HelperMessage::Event(event) => {
                        let decision = presenter.event(&event, out, last_frame)?;
                        if presenter.take_question_asked() {
                            input.discard_typed_ahead();
                        }
                        if let Some(decision) = decision
                            && let Err(detail) = link.send(&CallerMessage::Decision(decision))
                        {
                            presenter.finish(out)?;
                            return Ok(presenter.lost(link, detail));
                        }
                    }
                    HelperMessage::Result(result) => {
                        presenter.finish(out)?;
                        return Ok(SessionEnd::Finished(result));
                    }
                    HelperMessage::Error(info) => {
                        presenter.finish(out)?;
                        return Ok(SessionEnd::Failed(info));
                    }
                    HelperMessage::Hello(_) => {
                        presenter.finish(out)?;
                        return Ok(presenter.lost(link, "unexpected hello frame".to_string()));
                    }
                }
            }
            Recv::Timeout => {
                // The pipe normally breaks when the helper exits; its process handle tells for
                // sure, without waiting for the silence limit (design E.7).
                if link.helper_exit_code().is_some() {
                    presenter.finish(out)?;
                    return Ok(presenter.lost(link, "the helper exited".to_string()));
                }
                if last_frame.elapsed() >= config.silence_limit {
                    presenter.finish(out)?;
                    return Ok(SessionEnd::Unresponsive);
                }
            }
            Recv::Closed(detail) => {
                presenter.finish(out)?;
                return Ok(presenter.lost(link, detail));
            }
        }
    }
}

/// What the user is being asked, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Prompt {
    None,
    /// Keep-or-revert countdown after a live reset.
    Countdown {
        op_id: OpId,
    },
    /// Reconnect path: keep or revert, no countdown.
    Reconnect {
        op_id: OpId,
        since: Instant,
    },
    /// The user (or `--answer`) has answered.
    Answered,
}

/// Turns events into text and answers into decisions.
#[derive(Debug)]
pub struct Presenter {
    /// Output is a console: the countdown line is rewritten in place.
    console: bool,
    answer: Option<AnswerArg>,
    /// From `Planned`: names and layouts of the keyboards involved.
    keyboards: Vec<ExpectedKeyboard>,
    planned: bool,
    /// `(instance ID, reported, expected)` of every keyboard that came back.
    arrivals: Vec<(String, Option<KeyboardType>, KeyboardType)>,
    prompt: Prompt,
    /// A keep-or-revert countdown started in this session (design review C8).
    countdown_seen: bool,
    /// A keep-or-revert question has just been asked (see [`Presenter::take_question_asked`]).
    question_asked: bool,
    input_eof: bool,
    /// The last line written has no line break yet (the console countdown).
    line_open: bool,
    reconnect_events: u32,
}

impl Presenter {
    pub fn new(console: bool, answer: Option<AnswerArg>) -> Self {
        Self {
            console,
            answer,
            keyboards: Vec::new(),
            planned: false,
            arrivals: Vec::new(),
            prompt: Prompt::None,
            countdown_seen: false,
            question_asked: false,
            input_eof: false,
            line_open: false,
            reconnect_events: 0,
        }
    }

    /// True once after a keep-or-revert question was asked: the caller then drops what the user
    /// typed before it ([`Input::discard_typed_ahead`]).
    pub fn take_question_asked(&mut self) -> bool {
        std::mem::take(&mut self.question_asked)
    }

    /// True while a keep/revert question is open and the user may still type an answer.
    pub fn wants_input(&self) -> bool {
        self.answer.is_none()
            && !self.input_eof
            && matches!(
                self.prompt,
                Prompt::Countdown { .. } | Prompt::Reconnect { .. }
            )
    }

    fn lost(&self, link: &mut dyn Link, detail: String) -> SessionEnd {
        SessionEnd::Lost {
            exit_code: link.helper_exit_code(),
            detail,
            planned: self.planned,
            countdown: self.countdown_seen,
        }
    }

    /// Ends an open countdown line.
    pub fn finish(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if self.line_open {
            writeln!(out)?;
            self.line_open = false;
        }
        out.flush()
    }

    /// Writes one whole line, after closing an open countdown line.
    fn say(&mut self, out: &mut dyn Write, text: &str) -> io::Result<()> {
        self.finish(out)?;
        writeln!(out, "{text}")?;
        out.flush()
    }

    fn name(&self, instance_id: &str) -> String {
        self.keyboards
            .iter()
            .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
            .map_or_else(|| instance_id.to_string(), |kb| kb.display_name.clone())
    }

    fn all_arrivals_verified(&self) -> bool {
        !self.arrivals.is_empty()
            && self
                .arrivals
                .iter()
                .all(|(_, reported, expected)| *reported == Some(*expected))
    }

    fn decide(&mut self, out: &mut dyn Write, op_id: OpId, keep: bool) -> io::Result<Decision> {
        self.prompt = Prompt::Answered;
        if keep {
            self.say(out, "Keeping the change.")?;
            Ok(Decision::Keep { op_id })
        } else {
            self.say(out, "Reverting the change.")?;
            Ok(Decision::RevertNow { op_id })
        }
    }

    /// Shows one event; returns a decision `--answer` makes at this point.
    pub fn event(
        &mut self,
        event: &Event,
        out: &mut dyn Write,
        now: Instant,
    ) -> io::Result<Option<Decision>> {
        match event {
            Event::Locked | Event::Heartbeat => {}
            Event::Planned {
                op_id,
                steps,
                keyboards,
                ..
            } => {
                self.planned = true;
                self.keyboards = keyboards.clone();
                self.say(
                    out,
                    &format!(
                        "Journaled as operation {}; writing {} step(s).",
                        op_id.short(),
                        steps.len()
                    ),
                )?;
            }
            Event::StepWritten { step, of, .. } => {
                self.say(out, &format!("  Wrote step {step} of {of}."))?;
            }
            Event::StateChanged { state, .. } => {
                if *state == OpState::RevertPending {
                    self.say(out, "Putting the previous values back.")?;
                }
            }
            Event::ResettingKeyboard { instance_id } => {
                let name = self.name(instance_id);
                self.say(out, &format!("Resetting {name}..."))?;
            }
            Event::KeyboardArrived {
                instance_id,
                reported,
                expected,
            } => {
                self.arrivals
                    .push((instance_id.clone(), *reported, *expected));
                let name = self.name(instance_id);
                let text = match reported {
                    Some(reported) if reported == expected => {
                        format!("{name} is back; Raw Input reports {reported}, as expected.")
                    }
                    Some(reported) => format!(
                        "{name} is back; Raw Input reports {reported}, expected {expected}."
                    ),
                    None => format!("{name} is back; Raw Input does not report its type."),
                };
                self.say(out, &text)?;
                if let (Prompt::Reconnect { op_id, .. }, Some(AnswerArg::Keep)) =
                    (&self.prompt, self.answer)
                {
                    let op_id = op_id.clone();
                    let keep = self.all_arrivals_verified();
                    return self.decide(out, op_id, keep).map(Some);
                }
            }
            Event::CountdownStarted {
                op_id,
                seconds,
                verified,
            } => return self.countdown_started(out, op_id, *seconds, *verified),
            Event::CountdownTick { remaining, .. } => {
                if matches!(self.prompt, Prompt::Countdown { .. }) {
                    if self.console {
                        write!(
                            out,
                            "\rKeep this layout? [y/N]  reverting automatically in {remaining} s  "
                        )?;
                        self.line_open = true;
                        out.flush()?;
                    } else if remaining.is_multiple_of(REDIRECTED_TICK_EVERY) {
                        self.say(
                            out,
                            &format!(
                                "Reverting automatically in {remaining} s unless you answer y."
                            ),
                        )?;
                    }
                }
            }
            Event::WaitingForReconnect {
                op_id,
                instance_ids,
            } => return self.waiting_for_reconnect(out, op_id, instance_ids, now),
            Event::RecoveryAssetsWritten { directory, skipped } => {
                let mut text = format!("Offline recovery files updated in {directory}");
                if *skipped > 0 {
                    text.push_str(&format!(
                        " ({skipped} value(s) could not go into restore-offline.cmd; the .reg file has them)"
                    ));
                }
                text.push('.');
                self.say(out, &text)?;
            }
            Event::Warning { message } => self.say(out, &format!("warning: {message}"))?,
        }
        Ok(None)
    }

    fn countdown_started(
        &mut self,
        out: &mut dyn Write,
        op_id: &OpId,
        seconds: u32,
        verified: bool,
    ) -> io::Result<Option<Decision>> {
        let changed: Vec<&ExpectedKeyboard> =
            self.keyboards.iter().filter(|kb| kb.changes).collect();
        let shown: Vec<&ExpectedKeyboard> = if changed.is_empty() {
            self.keyboards.iter().collect()
        } else {
            changed
        };
        let mut lines = Vec::new();
        for kb in shown {
            let layout = kb
                .layout_after
                .as_ref()
                .map_or("the new layout", crate::text::table_name);
            let arrival = self
                .arrivals
                .iter()
                .find(|(id, _, _)| id.eq_ignore_ascii_case(&kb.instance_id));
            let check = match arrival {
                Some((_, Some(reported), expected)) if reported == expected => {
                    format!("Raw Input reports {reported}, as expected")
                }
                Some((_, Some(reported), expected)) => {
                    format!("Raw Input reports {reported}, expected {expected}")
                }
                _ => "Raw Input could not confirm it".to_string(),
            };
            lines.push(format!(
                "{}: switched to {layout} ({check}).",
                kb.display_name
            ));
        }
        for text in lines {
            self.say(out, &text)?;
        }
        self.say(
            out,
            "Type Shift+2 in any text box: \" means JIS, @ means US.",
        )?;
        self.prompt = Prompt::Countdown {
            op_id: op_id.clone(),
        };
        self.countdown_seen = true;
        self.question_asked = true;
        match self.answer {
            Some(AnswerArg::Keep) => {
                self.say(
                    out,
                    if verified {
                        "--answer keep: Raw Input reports the intended type."
                    } else {
                        "--answer keep: Raw Input does not confirm the intended type, so revert."
                    },
                )?;
                self.decide(out, op_id.clone(), verified).map(Some)
            }
            Some(AnswerArg::Revert) => self.decide(out, op_id.clone(), false).map(Some),
            None => {
                if self.console {
                    write!(
                        out,
                        "Keep this layout? [y/N]  reverting automatically in {seconds} s  "
                    )?;
                    self.line_open = true;
                    out.flush()?;
                } else {
                    self.say(
                        out,
                        &format!("Keep this layout? [y/N]  reverting automatically in {seconds} s"),
                    )?;
                }
                Ok(None)
            }
        }
    }

    fn waiting_for_reconnect(
        &mut self,
        out: &mut dyn Write,
        op_id: &OpId,
        instance_ids: &[String],
        now: Instant,
    ) -> io::Result<Option<Decision>> {
        self.reconnect_events += 1;
        match &self.prompt {
            Prompt::Reconnect { .. } => {
                if self
                    .reconnect_events
                    .is_multiple_of(RECONNECT_REMINDER_EVERY)
                {
                    self.say(out, "Still waiting for the keyboard to reconnect...")?;
                }
                return Ok(None);
            }
            Prompt::Answered => return Ok(None),
            Prompt::None | Prompt::Countdown { .. } => {}
        }
        let names: Vec<String> = instance_ids.iter().map(|id| self.name(id)).collect();
        self.say(
            out,
            &format!(
                "Reconnect {} so that it picks up the new layout: unplug and replug it \
                 (Bluetooth: turn it off and on).",
                if names.is_empty() {
                    "the keyboard".to_string()
                } else {
                    names.join(", ")
                }
            ),
        )?;
        self.say(
            out,
            "Then type Shift+2 in any text box: \" means JIS, @ means US.",
        )?;
        self.prompt = Prompt::Reconnect {
            op_id: op_id.clone(),
            since: now,
        };
        self.question_asked = true;
        match self.answer {
            Some(AnswerArg::Revert) => self.decide(out, op_id.clone(), false).map(Some),
            Some(AnswerArg::Keep) => Ok(None),
            None => {
                self.say(
                    out,
                    &format!(
                        "Keep this layout? [y/N]  (without an answer it stays written; decide \
                         later with `mklm-cli keep {op}` or `mklm-cli revert {op}`)",
                        op = op_id.short()
                    ),
                )?;
                Ok(None)
            }
        }
    }

    /// Handles one line the user typed while a question is open.
    pub fn line(&mut self, line: Line, out: &mut dyn Write) -> io::Result<Option<Decision>> {
        let op_id = match &self.prompt {
            Prompt::Countdown { op_id } | Prompt::Reconnect { op_id, .. } => op_id.clone(),
            Prompt::None | Prompt::Answered => return Ok(None),
        };
        match line {
            Line::Eof => {
                // No answer: a countdown runs out (and reverts); a reconnect wait stays written.
                self.input_eof = true;
                Ok(None)
            }
            Line::Text(text) => match yes_no(&text) {
                Some(YesNo::Yes) => self.decide(out, op_id, true).map(Some),
                Some(YesNo::No | YesNo::NoAnswer) => self.decide(out, op_id, false).map(Some),
                None => {
                    self.say(out, "Please answer y or n.")?;
                    Ok(None)
                }
            },
        }
    }

    /// Time-based decisions: with `--answer keep` on the reconnect path, a keyboard that has not
    /// come back after the engine's reconnect wait is reverted.
    pub fn tick(&mut self, now: Instant, out: &mut dyn Write) -> io::Result<Option<Decision>> {
        if let (Prompt::Reconnect { op_id, since }, Some(AnswerArg::Keep)) =
            (&self.prompt, self.answer)
            && now.saturating_duration_since(*since) >= RECONNECT_ANSWER_WAIT
        {
            let op_id = op_id.clone();
            self.say(
                out,
                "--answer keep: the keyboard did not report the intended type in time, so revert.",
            )?;
            return self.decide(out, op_id, false).map(Some);
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use mklm_core::{
        FailureReason, LayoutTable, Outcome, PendingAction, PlanStep, PlannedWrite, WriteTarget,
        value_names,
    };

    use super::super::input::{Line, ScriptedInput};
    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";

    fn op() -> OpId {
        OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap()
    }

    fn result(outcome: Outcome, failure: Option<FailureReason>) -> OperationResult {
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

    /// The events of a USB `set` up to the countdown (design D.2 a).
    fn live_reset_events(verified: bool) -> Vec<HelperMessage> {
        let reported = if verified {
            KeyboardType::JIS
        } else {
            KeyboardType::US
        };
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
            HelperMessage::Event(Event::Heartbeat),
            event(Event::KeyboardArrived {
                instance_id: KEYCHRON.into(),
                reported: Some(reported),
                expected: KeyboardType::JIS,
            }),
            event(Event::CountdownStarted {
                op_id: op(),
                seconds: 20,
                verified,
            }),
        ]
    }

    /// Plays the helper: queued frames first; on a decision (or when the frames run out without
    /// one, i.e. the countdown expired) the engine's answer (design D.2 a step 7).
    #[derive(Debug)]
    struct FakeHelper {
        frames: VecDeque<HelperMessage>,
        sent: Vec<CallerMessage>,
        /// Frames that follow the countdown ticks when nobody answers.
        ticks: VecDeque<HelperMessage>,
        answered: bool,
        closed: bool,
        exit_code: Option<u32>,
    }

    impl FakeHelper {
        fn new(frames: Vec<HelperMessage>) -> Self {
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

        fn decisions(&self) -> Vec<&Decision> {
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
            // The pipe breaks once the queued frames are read.
            if self.closed && self.frames.is_empty() {
                return Err("closed".into());
            }
            if let CallerMessage::Decision(decision) = message {
                self.answered = true;
                self.ticks.clear();
                self.frames.push_back(match decision {
                    Decision::Keep { .. } => {
                        HelperMessage::Result(result(Outcome::Confirmed, None))
                    }
                    Decision::RevertNow { .. } => {
                        HelperMessage::Result(result(Outcome::Reverted, None))
                    }
                });
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

    fn request() -> Request {
        Request::Recover {
            apply: Default::default(),
        }
    }

    fn run(
        helper: &mut FakeHelper,
        input: &mut ScriptedInput,
        console: bool,
        answer: Option<AnswerArg>,
    ) -> (SessionEnd, String) {
        let mut out = Vec::new();
        let mut presenter = Presenter::new(console, answer);
        let config = RelayConfig {
            tick: Duration::ZERO,
            silence_limit: Duration::from_millis(50),
        };
        let end = relay(helper, request(), &mut presenter, input, &mut out, config).unwrap();
        (end, String::from_utf8(out).unwrap())
    }

    #[test]
    fn yes_keeps_during_the_countdown() {
        let mut helper = FakeHelper::new(live_reset_events(true));
        let (end, out) = run(&mut helper, &mut ScriptedInput::new(["y"]), false, None);
        assert_eq!(end, SessionEnd::Finished(result(Outcome::Confirmed, None)));
        assert_eq!(helper.decisions(), vec![&Decision::Keep { op_id: op() }]);
        assert!(matches!(helper.sent[0], CallerMessage::Request(_)));
        assert!(
            out.contains(
                "Keychron Receiver: switched to JIS (Raw Input reports 0x7/0x2, as expected)."
            ),
            "{out}"
        );
        assert!(out.contains("Type Shift+2 in any text box: \" means JIS, @ means US."));
        assert!(out.contains("Keep this layout? [y/N]  reverting automatically in 20 s"));
        assert!(out.contains("Keeping the change."));
    }

    #[test]
    fn no_and_enter_revert_and_other_text_asks_again() {
        for answers in [vec!["n"], vec!["no"], vec![""], vec!["what", "n"]] {
            let mut helper = FakeHelper::new(live_reset_events(true));
            let (end, out) = run(
                &mut helper,
                &mut ScriptedInput::new(answers.clone()),
                true,
                None,
            );
            assert_eq!(end, SessionEnd::Finished(result(Outcome::Reverted, None)));
            assert_eq!(
                helper.decisions(),
                vec![&Decision::RevertNow { op_id: op() }]
            );
            assert_eq!(
                out.contains("Please answer y or n."),
                answers.len() == 2,
                "{out}"
            );
        }
    }

    #[test]
    fn end_of_input_lets_the_countdown_run_out() {
        let mut helper = FakeHelper::new(live_reset_events(true));
        let (end, out) = run(&mut helper, &mut ScriptedInput::new([]), false, None);
        assert_eq!(
            end,
            SessionEnd::Finished(result(
                Outcome::Reverted,
                Some(FailureReason::CountdownExpired)
            ))
        );
        assert!(helper.decisions().is_empty());
        // Redirected output: one line every five seconds, no carriage returns.
        assert_eq!(
            out.matches("Reverting automatically in").count(),
            4,
            "{out}"
        );
        assert!(out.contains("Reverting automatically in 5 s unless you answer y."));
        assert!(!out.contains('\r'));
    }

    #[test]
    fn the_console_rewrites_the_countdown_line() {
        let mut helper = FakeHelper::new(live_reset_events(true));
        let (_, out) = run(&mut helper, &mut ScriptedInput::new([]), true, None);
        assert!(out.contains("\rKeep this layout? [y/N]  reverting automatically in 1 s"));
        assert_eq!(out.matches('\r').count(), 20);
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn answer_keep_needs_the_intended_type() {
        let mut helper = FakeHelper::new(live_reset_events(true));
        let (end, _) = run(
            &mut helper,
            &mut ScriptedInput::new([]),
            false,
            Some(AnswerArg::Keep),
        );
        assert_eq!(end, SessionEnd::Finished(result(Outcome::Confirmed, None)));
        assert_eq!(helper.decisions(), vec![&Decision::Keep { op_id: op() }]);

        let mut helper = FakeHelper::new(live_reset_events(false));
        let (end, out) = run(
            &mut helper,
            &mut ScriptedInput::new(["y"]),
            false,
            Some(AnswerArg::Keep),
        );
        assert_eq!(end, SessionEnd::Finished(result(Outcome::Reverted, None)));
        assert_eq!(
            helper.decisions(),
            vec![&Decision::RevertNow { op_id: op() }]
        );
        assert!(out.contains("expected 0x7/0x2"), "{out}");

        let mut helper = FakeHelper::new(live_reset_events(true));
        let (_, _) = run(
            &mut helper,
            &mut ScriptedInput::new([]),
            false,
            Some(AnswerArg::Revert),
        );
        assert_eq!(
            helper.decisions(),
            vec![&Decision::RevertNow { op_id: op() }]
        );
    }

    #[test]
    fn reconnect_path_asks_without_a_countdown() {
        let mut frames = live_reset_events(true);
        frames.truncate(4); // Locked, Planned, StepWritten, Written
        frames.push(event(Event::StateChanged {
            op_id: op(),
            state: OpState::AwaitingConfirm,
        }));
        frames.push(event(Event::WaitingForReconnect {
            op_id: op(),
            instance_ids: vec![KEYCHRON.into()],
        }));
        let mut helper = FakeHelper::new(frames.clone());
        helper.ticks.clear();
        let (end, out) = run(&mut helper, &mut ScriptedInput::new(["y"]), false, None);
        assert_eq!(end, SessionEnd::Finished(result(Outcome::Confirmed, None)));
        assert!(out.contains("Reconnect Keychron Receiver so that it picks up the new layout"));
        assert!(out.contains("mklm-cli keep 3f2a9c1e"), "{out}");

        // --answer keep waits for the keyboard and checks what it reports.
        frames.push(event(Event::KeyboardArrived {
            instance_id: KEYCHRON.into(),
            reported: Some(KeyboardType::JIS),
            expected: KeyboardType::JIS,
        }));
        let mut helper = FakeHelper::new(frames);
        helper.ticks.clear();
        let (end, _) = run(
            &mut helper,
            &mut ScriptedInput::new([]),
            false,
            Some(AnswerArg::Keep),
        );
        assert_eq!(end, SessionEnd::Finished(result(Outcome::Confirmed, None)));
        assert_eq!(helper.decisions(), vec![&Decision::Keep { op_id: op() }]);
    }

    #[test]
    fn a_helper_that_dies_mid_countdown_is_reported_lost() {
        let mut helper = FakeHelper::new(live_reset_events(true));
        helper.ticks.truncate(3);
        helper.answered = true; // no expiry result: the helper dies instead
        helper.closed = true;
        helper.exit_code = Some(1);
        let (end, _) = run(&mut helper, &mut ScriptedInput::new([]), true, None);
        assert_eq!(
            end,
            SessionEnd::Lost {
                exit_code: Some(1),
                detail: "the helper closed the pipe".into(),
                planned: true,
                countdown: true,
            }
        );
    }

    #[test]
    fn silence_and_exits_end_the_relay() {
        // Nothing at all after the request: silence past the limit.
        let mut helper = FakeHelper::new(Vec::new());
        helper.answered = true;
        helper.ticks.clear();
        let (end, _) = run(&mut helper, &mut ScriptedInput::new([]), false, None);
        assert_eq!(end, SessionEnd::Unresponsive);

        // The helper process exited without closing the pipe first.
        let mut helper = FakeHelper::new(Vec::new());
        helper.answered = true;
        helper.ticks.clear();
        helper.exit_code = Some(3);
        let (end, _) = run(&mut helper, &mut ScriptedInput::new([]), false, None);
        assert!(
            matches!(
                end,
                SessionEnd::Lost {
                    exit_code: Some(3),
                    planned: false,
                    countdown: false,
                    ..
                }
            ),
            "{end:?}"
        );
    }

    /// A console that already holds a line when the question appears (an early Enter, a `y` typed
    /// twice): the relay drops it, so only an answer given after the question counts.
    #[derive(Debug)]
    struct TypedAhead {
        lines: VecDeque<Line>,
        discarded: usize,
    }

    impl Input for TypedAhead {
        fn next_line(&mut self, _timeout: Option<Duration>) -> Option<Line> {
            Some(self.lines.pop_front().unwrap_or(Line::Eof))
        }

        fn discard_typed_ahead(&mut self) {
            self.discarded += self.lines.len();
            self.lines.clear();
        }
    }

    #[test]
    fn answers_typed_before_the_question_do_not_count() {
        let mut helper = FakeHelper::new(live_reset_events(true));
        let mut input = TypedAhead {
            lines: VecDeque::from([Line::Text("y".into())]),
            discarded: 0,
        };
        let mut out = Vec::new();
        let mut presenter = Presenter::new(true, None);
        let config = RelayConfig {
            tick: Duration::ZERO,
            silence_limit: Duration::from_millis(50),
        };
        let end = relay(
            &mut helper,
            request(),
            &mut presenter,
            &mut input,
            &mut out,
            config,
        )
        .unwrap();
        assert_eq!(input.discarded, 1);
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
    fn errors_are_passed_through() {
        let info = ErrorInfo {
            code: mklm_core::ErrorCode::Busy,
            message: "busy".into(),
            op_id: None,
            plan_error: None,
        };
        let mut helper = FakeHelper::new(vec![
            event(Event::Warning {
                message: "a warning".into(),
            }),
            HelperMessage::Error(info.clone()),
        ]);
        let (end, out) = run(&mut helper, &mut ScriptedInput::new([]), false, None);
        assert_eq!(end, SessionEnd::Failed(info));
        assert!(out.contains("warning: a warning"));
    }
}
