//! A whole request as the user sees it (design m2 E.7 / F.2 steps 5 to 7, review C8; m3 A.2):
//! launch a helper, relay the request, and when the helper is lost after the operation was
//! journaled and the journal now asks for recovery, launch a new helper with `Recover` — at once
//! for the CLI, after the user agreed for the GUI ([`Frontend::confirm_recovery`]: non-elevated
//! callers get a second UAC prompt, design m3 A.2.3), and never once the front end asked to
//! stop. The post-reboot RunOnce rule ([`crate::run_once`]) runs right after the request on the
//! same thread, whatever the report says ([`Orchestrator::run_then`]; design m2 C17, m3 A.2.3).
//!
//! The orchestrator works over a [`Launcher`] and a [`JournalSource`], so tests (and the GUI's
//! view-model tests) replace the helper and the journal with fakes. On Windows,
//! [`crate::launch::HelperLauncher`] and [`crate::journal::LiveJournal`] are the real ones.

use std::fmt;
use std::io;

use mklm_core::ApplyOptions;
use mklm_ipc::Request;

use crate::HelperExit;
use crate::session::{Frontend, Link, Notice, RelayConfig, SessionEnd, SessionKind, relay};

/// Why no helper session came up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The user declined the UAC prompt (nothing was written).
    Declined,
    /// Anything else: `kind` for the GUI's text, `message` in English for logs and the CLI.
    Failed {
        kind: LaunchFailure,
        message: String,
    },
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::Declined => write!(f, "the administrator permission was declined"),
            LaunchError::Failed { message, .. } => write!(f, "{message}"),
        }
    }
}

/// The kind of a [`LaunchError::Failed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaunchFailure {
    /// `mklm-helper.exe` is not next to this executable (or not a regular file).
    HelperMissing,
    /// The helper's build ID could not be read, or it is from another build (design m2 E.3):
    /// reinstall (during development, rebuild the workspace). Checked before any UAC prompt.
    OtherBuild,
    /// Pipe name, nonce or pipe creation failed.
    Setup,
    /// The helper process could not be started (other than a declined UAC prompt), with the
    /// Win32 error when there is one: 225 / 226 (`ERROR_VIRUS_INFECTED` / `_DELETED`: an
    /// antivirus blocked it), 1260 (`ERROR_ACCESS_DISABLED_BY_POLICY`), … (design m3 B.17).
    StartFailed(Option<u32>),
    /// The helper exited before it connected.
    ExitedEarly(HelperExit),
    /// The helper did not connect in time.
    NoConnection,
    /// The handshake failed (greeting, version, build ID, nonce, PID).
    Handshake,
}

/// A connected, verified helper.
pub trait HelperLink: Link {
    /// Says `Bye` and gives the helper a moment to exit.
    fn close(self: Box<Self>);
}

/// Starts helper sessions.
pub trait Launcher {
    /// Launches a helper and runs the handshake. Blocks while the UAC prompt is shown.
    fn launch(&mut self) -> Result<Box<dyn HelperLink>, LaunchError>;
}

/// The journal, read unelevated.
pub trait JournalSource {
    /// True when some entry's `attention` is `Recover` now (design m2 C.7).
    fn needs_recovery(&mut self) -> Result<bool, String>;
}

/// How one session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestEnd {
    NotLaunched(LaunchError),
    Ended(SessionEnd),
}

/// Everything that happened for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestReport {
    pub first: RequestEnd,
    /// Set when the first session ended [`SessionEnd::Lost`]: whether the journal asked for
    /// recovery (`Err`: it could not be read).
    pub lost_needs_recovery: Option<Result<bool, String>>,
    /// The recovery session after a lost helper, when it ran.
    pub recovery: Option<RequestEnd>,
    /// The journal asked for recovery after a lost helper, but none was launched (design m3
    /// A.2.3): the start-up check and the banner offer it later.
    pub recovery_skipped: Option<RecoverySkip>,
}

/// Why no recovery session followed a lost helper that left something to recover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverySkip {
    /// The front end was asked to stop (quit, end of the Windows session).
    Cancelled,
    /// The user chose "later" ([`Frontend::confirm_recovery`] returned false).
    Declined,
}

/// Runs requests through helper sessions.
#[derive(Debug)]
pub struct Orchestrator<L, J> {
    pub launcher: L,
    pub journal: J,
    pub config: RelayConfig,
    /// False: never recover after a lost helper (the recovery request itself, a dry run).
    pub recover_lost: bool,
}

impl<L: Launcher, J: JournalSource> Orchestrator<L, J> {
    pub fn new(launcher: L, journal: J) -> Self {
        Self {
            launcher,
            journal,
            config: RelayConfig::default(),
            recover_lost: true,
        }
    }

    /// Runs `request`. `apply` is used for the recovery after a lost helper (the same options
    /// the user gave, design m2 E.7).
    pub fn run(
        &mut self,
        request: Request,
        apply: ApplyOptions,
        frontend: &mut dyn Frontend,
    ) -> io::Result<RequestReport> {
        let first = self.session(request, SessionKind::Request, frontend)?;
        let mut report = RequestReport {
            first,
            lost_needs_recovery: None,
            recovery: None,
            recovery_skipped: None,
        };
        let RequestEnd::Ended(SessionEnd::Lost {
            exit_code,
            detail,
            countdown,
            ..
        }) = &report.first
        else {
            return Ok(report);
        };
        frontend.notice(&Notice::HelperLost {
            exit: exit_code.map(HelperExit),
            detail: detail.clone(),
        })?;
        let countdown = *countdown;
        let needs = self.journal.needs_recovery();
        let recover = self.recover_lost && needs == Ok(true);
        report.lost_needs_recovery = Some(needs);
        if !recover {
            return Ok(report);
        }
        // Never a new UAC prompt after the user chose to quit, and none the user did not agree
        // to (design m3 A.2.3, K.6).
        if frontend.cancel_requested() {
            report.recovery_skipped = Some(RecoverySkip::Cancelled);
            return Ok(report);
        }
        if !frontend.confirm_recovery(countdown)? {
            report.recovery_skipped = Some(if frontend.cancel_requested() {
                RecoverySkip::Cancelled
            } else {
                RecoverySkip::Declined
            });
            return Ok(report);
        }
        frontend.notice(&Notice::RecoveringAfterLoss { countdown })?;
        report.recovery =
            Some(self.session(Request::Recover { apply }, SessionKind::Recovery, frontend)?);
        Ok(report)
    }

    /// [`Self::run`], then `after` on the same thread — also when `run` failed. The front ends
    /// apply the post-reboot RunOnce rule here (design m2 C17, m3 A.2.3), so that it is done
    /// before the request is reported as ended and a quit cannot overtake it.
    pub fn run_then<T>(
        &mut self,
        request: Request,
        apply: ApplyOptions,
        frontend: &mut dyn Frontend,
        after: impl FnOnce() -> T,
    ) -> (io::Result<RequestReport>, T) {
        let report = self.run(request, apply, frontend);
        (report, after())
    }

    fn session(
        &mut self,
        request: Request,
        kind: SessionKind,
        frontend: &mut dyn Frontend,
    ) -> io::Result<RequestEnd> {
        frontend.notice(&Notice::Starting(kind))?;
        let mut link = match self.launcher.launch() {
            Ok(link) => link,
            Err(error) => return Ok(RequestEnd::NotLaunched(error)),
        };
        if let Err(error) = frontend.notice(&Notice::Connected(kind)) {
            link.close();
            return Err(error);
        }
        let end = relay(link.as_mut(), request, frontend, self.config);
        link.close();
        Ok(RequestEnd::Ended(end?))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use mklm_core::{Event, FailureReason, OpState, Outcome, RecoveredOp};
    use mklm_ipc::HelperMessage;

    use super::*;
    use crate::session::tests::{
        FakeHelper, ScriptedFrontend, fast, live_reset_events, op, result,
    };

    impl HelperLink for FakeHelper {
        fn close(self: Box<Self>) {}
    }

    /// Hands out prepared helpers; records the requests they got.
    #[derive(Debug, Default)]
    struct FakeLauncher {
        helpers: VecDeque<Result<FakeHelper, LaunchError>>,
    }

    impl Launcher for FakeLauncher {
        fn launch(&mut self) -> Result<Box<dyn HelperLink>, LaunchError> {
            match self.helpers.pop_front().expect("an unexpected launch") {
                Ok(helper) => Ok(Box::new(helper)),
                Err(error) => Err(error),
            }
        }
    }

    #[derive(Debug)]
    struct FakeJournal(Result<bool, String>);

    impl JournalSource for FakeJournal {
        fn needs_recovery(&mut self) -> Result<bool, String> {
            self.0.clone()
        }
    }

    fn fake_orchestrator(
        helpers: Vec<Result<FakeHelper, LaunchError>>,
        needs_recovery: Result<bool, String>,
    ) -> Orchestrator<FakeLauncher, FakeJournal> {
        let mut orchestrator = Orchestrator::new(
            FakeLauncher {
                helpers: helpers.into(),
            },
            FakeJournal(needs_recovery),
        );
        orchestrator.config = fast();
        orchestrator
    }

    fn request() -> Request {
        Request::Undo {
            apply: ApplyOptions::default(),
        }
    }

    #[test]
    fn a_declined_prompt_ends_the_request() {
        let mut orchestrator = fake_orchestrator(vec![Err(LaunchError::Declined)], Ok(false));
        let mut frontend = ScriptedFrontend::default();
        let report = orchestrator
            .run(request(), ApplyOptions::default(), &mut frontend)
            .unwrap();
        assert_eq!(report.first, RequestEnd::NotLaunched(LaunchError::Declined));
        assert_eq!(report.recovery, None);
        assert_eq!(
            frontend.notices,
            vec![Notice::Starting(SessionKind::Request)]
        );
    }

    #[test]
    fn a_helper_lost_during_the_countdown_is_followed_by_a_recovery() {
        let mut dying = FakeHelper::new(live_reset_events());
        dying.ticks.truncate(3);
        dying.answered = true;
        dying.closed = true;
        dying.exit_code = Some(1);
        let mut recovered = result(Outcome::Recovered, None);
        recovered.op_id = None;
        recovered.recovered.push(RecoveredOp {
            op_id: op(),
            from: OpState::AwaitingConfirm,
            to: OpState::Reverted,
            decision: "roll-back".into(),
        });
        let mut recovering = FakeHelper::new(vec![HelperMessage::Result(recovered.clone())]);
        recovering.answered = true;
        recovering.ticks.clear();
        let mut orchestrator = fake_orchestrator(vec![Ok(dying), Ok(recovering)], Ok(true));
        let mut frontend = ScriptedFrontend::default();
        let report = orchestrator
            .run(request(), ApplyOptions::default(), &mut frontend)
            .unwrap();
        assert!(matches!(
            report.first,
            RequestEnd::Ended(SessionEnd::Lost {
                countdown: true,
                planned: true,
                ..
            })
        ));
        assert_eq!(report.lost_needs_recovery, Some(Ok(true)));
        assert_eq!(
            report.recovery,
            Some(RequestEnd::Ended(SessionEnd::Finished(recovered)))
        );
        assert_eq!(
            frontend.notices,
            vec![
                Notice::Starting(SessionKind::Request),
                Notice::Connected(SessionKind::Request),
                Notice::HelperLost {
                    exit: Some(HelperExit(1)),
                    detail: "the helper closed the pipe".into()
                },
                Notice::RecoveringAfterLoss { countdown: true },
                Notice::Starting(SessionKind::Recovery),
                Notice::Connected(SessionKind::Recovery),
            ]
        );
        assert_eq!(report.recovery_skipped, None);
    }

    fn dying_during_the_countdown() -> FakeHelper {
        let mut dying = FakeHelper::new(live_reset_events());
        dying.ticks.truncate(3);
        dying.answered = true;
        dying.closed = true;
        dying.exit_code = Some(1);
        dying
    }

    #[test]
    fn no_recovery_prompt_the_user_did_not_agree_to() {
        // "Later": nothing is launched (the fake launcher panics on an unexpected launch); the
        // journal still asks for recovery, which the banner offers.
        let mut orchestrator = fake_orchestrator(vec![Ok(dying_during_the_countdown())], Ok(true));
        let mut frontend = ScriptedFrontend {
            decline_recovery: true,
            ..Default::default()
        };
        let report = orchestrator
            .run(request(), ApplyOptions::default(), &mut frontend)
            .unwrap();
        assert_eq!(report.lost_needs_recovery, Some(Ok(true)));
        assert_eq!(report.recovery, None);
        assert_eq!(report.recovery_skipped, Some(RecoverySkip::Declined));
        assert_eq!(frontend.recovery_questions, vec![true]);
        assert!(
            !frontend
                .notices
                .contains(&Notice::RecoveringAfterLoss { countdown: true })
        );
    }

    #[test]
    fn no_recovery_prompt_after_the_user_chose_to_quit() {
        // Quit during the countdown: the relay answers "revert now", the helper dies on it, and
        // no second UAC prompt follows (design m3 A.2.3).
        let mut orchestrator = fake_orchestrator(vec![Ok(dying_during_the_countdown())], Ok(true));
        let mut frontend = ScriptedFrontend {
            cancel_after_events: Some(8),
            ..Default::default()
        };
        let report = orchestrator
            .run(request(), ApplyOptions::default(), &mut frontend)
            .unwrap();
        assert!(matches!(
            report.first,
            RequestEnd::Ended(SessionEnd::Lost {
                countdown: true,
                ..
            })
        ));
        assert_eq!(report.recovery, None);
        assert_eq!(report.recovery_skipped, Some(RecoverySkip::Cancelled));
        assert!(frontend.recovery_questions.is_empty());
    }

    #[test]
    fn the_after_step_runs_whatever_the_request_did() {
        let mut orchestrator = fake_orchestrator(vec![Err(LaunchError::Declined)], Ok(false));
        let mut frontend = ScriptedFrontend::default();
        let (report, after) =
            orchestrator.run_then(request(), ApplyOptions::default(), &mut frontend, || 7);
        assert_eq!(
            report.unwrap().first,
            RequestEnd::NotLaunched(LaunchError::Declined)
        );
        assert_eq!(after, 7);
    }

    #[test]
    fn no_recovery_when_the_journal_does_not_ask_for_it() {
        let mut dying = FakeHelper::new(vec![HelperMessage::Event(Event::Locked)]);
        dying.answered = true;
        dying.ticks.clear();
        dying.closed = true;
        let mut orchestrator = fake_orchestrator(vec![Ok(dying)], Ok(false));
        let mut frontend = ScriptedFrontend::default();
        let report = orchestrator
            .run(request(), ApplyOptions::default(), &mut frontend)
            .unwrap();
        assert_eq!(report.lost_needs_recovery, Some(Ok(false)));
        assert_eq!(report.recovery, None);

        // A finished request is reported as is, and nothing else is launched (the fake launcher
        // panics on an unexpected launch).
        let mut helper = FakeHelper::new(live_reset_events());
        let expired = result(Outcome::Reverted, Some(FailureReason::CountdownExpired));
        helper
            .frames
            .push_back(HelperMessage::Result(expired.clone()));
        let mut orchestrator = fake_orchestrator(vec![Ok(helper)], Ok(true));
        let report = orchestrator
            .run(request(), ApplyOptions::default(), &mut frontend)
            .unwrap();
        assert_eq!(
            report.first,
            RequestEnd::Ended(SessionEnd::Finished(expired))
        );
        assert_eq!(report.lost_needs_recovery, None);
        assert!(orchestrator.launcher.helpers.is_empty());
    }
}
