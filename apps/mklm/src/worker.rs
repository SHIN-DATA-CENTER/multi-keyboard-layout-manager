//! The session worker (design m3 A.4): one thread per helper session. It launches the helper
//! (the UAC prompt blocks here, owned by the main window), relays the request with
//! `mklm_client::run_once::run_request` (the orchestrator of the CLI, design m3 A.2, followed by
//! the RunOnce rule on this same thread), and hands every event to the UI thread with
//! `slint::invoke_from_event_loop`. The UI thread passes decisions (Keep / Revert buttons) and
//! the answer to the recovery question through channels and asks it to leave through a flag; it
//! never blocks on the worker — except for the bounded wait at the end of the Windows session
//! ([`wait_for_current_session`], design m3 F.5).
//!
//! Every message carries the session's id, so a message from an older session is dropped
//! (`state::update`). If the thread panics, a guard still reports the end, so that a pending
//! quit is not stuck (review A11).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mklm_client::launch::LaunchConfig;
use mklm_client::orchestrator::{RequestEnd, RequestReport};
use mklm_client::run_once::{PostRebootCommand, RunOnceError, run_request};
use mklm_client::session::{Frontend, Notice, Prompt, SessionEnd, SessionView};
use mklm_core::{ApplyOptions, Decision, Event};
use mklm_ipc::Request;

use crate::app::post;
use crate::state::{AppMsg, SessionId, SessionOutcome};

/// What the UI thread and the end-of-session handler share with a running worker. Atomics and a
/// condition variable only: the shell window's `WM_QUERYENDSESSION` handler sets `cancel`
/// directly, without borrowing the controller (design m3 F.5, review A5).
#[derive(Debug, Default)]
pub struct SessionShared {
    /// `Frontend::cancel_requested`.
    pub cancel: AtomicBool,
    /// The operation was journaled (`Planned`, or any request that writes without it once the
    /// helper answered): at the end of the Windows session the post-reboot RunOnce value is
    /// registered unconditionally, so that the next sign-in shows the recovery.
    pub journaled: AtomicBool,
    /// Set after the request and the RunOnce rule are done.
    done: Mutex<bool>,
    finished: Condvar,
}

impl SessionShared {
    fn mark_done(&self) {
        if let Ok(mut done) = self.done.lock() {
            *done = true;
        }
        self.finished.notify_all();
    }

    /// Waits up to `limit` for the worker to finish; true when it did.
    pub fn wait_done(&self, limit: Duration) -> bool {
        let Ok(done) = self.done.lock() else {
            return false;
        };
        self.finished
            .wait_timeout_while(done, limit, |done| !*done)
            .is_ok_and(|(done, _)| *done)
    }
}

/// The running session, for the end-of-session handler (UI thread; a `Mutex` so that no
/// `RefCell` borrow is involved).
static CURRENT: Mutex<Option<Arc<SessionShared>>> = Mutex::new(None);

fn current() -> Option<Arc<SessionShared>> {
    CURRENT.lock().ok().and_then(|current| current.clone())
}

/// `WM_QUERYENDSESSION`: ask the running session to leave now (a countdown is reverted at once).
/// Returns true when a journaled operation is open (the caller registers the RunOnce value).
pub fn cancel_current_session() -> bool {
    current().is_some_and(|shared| {
        shared.cancel.store(true, Ordering::SeqCst);
        shared.journaled.load(Ordering::SeqCst)
    })
}

/// `WM_ENDSESSION`: wait up to `limit` for the running session to end (design m3 F.5: 3 s).
pub fn wait_for_current_session(limit: Duration) {
    if let Some(shared) = current() {
        shared.wait_done(limit);
    }
}

/// The UI thread's handle on a running session.
#[derive(Debug)]
pub struct SessionWorker {
    decisions: Sender<Decision>,
    recovery_answers: Sender<bool>,
    shared: Arc<SessionShared>,
    thread: Option<JoinHandle<()>>,
}

impl SessionWorker {
    /// Starts session `session` for `request`. `launch` carries the build ID and the main window
    /// (the owner of the UAC prompt). Ends with `AppMsg::SessionEnded` on the UI thread.
    pub fn start(
        session: SessionId,
        request: Request,
        apply: ApplyOptions,
        launch: LaunchConfig,
    ) -> io::Result<Self> {
        let (decisions, decision_receiver) = mpsc::channel();
        let (recovery_answers, answer_receiver) = mpsc::channel();
        let shared = Arc::new(SessionShared::default());
        let worker_shared = shared.clone();
        let writes_without_plan = !mklm_client::session::plans_first(&request);
        if let Ok(mut current) = CURRENT.lock() {
            *current = Some(shared.clone());
        }
        let spawned = thread::Builder::new()
            .name("mklm-session".into())
            .spawn(move || {
                let mut guard = EndGuard {
                    session,
                    shared: worker_shared.clone(),
                    reported: false,
                };
                let mut frontend = GuiFrontend {
                    session,
                    decisions: decision_receiver,
                    recovery_answers: answer_receiver,
                    shared: worker_shared,
                    writes_without_plan,
                };
                let outcome = run_request(
                    launch,
                    request,
                    apply,
                    &mut frontend,
                    PostRebootCommand::Gui,
                );
                let report = outcome
                    .report
                    .unwrap_or_else(|error| lost_report(error.to_string()));
                guard.report(SessionOutcome {
                    report,
                    run_once: outcome.run_once,
                });
            });
        let thread = match spawned {
            Ok(thread) => thread,
            Err(error) => {
                if let Ok(mut current) = CURRENT.lock() {
                    *current = None;
                }
                return Err(error);
            }
        };
        Ok(Self {
            decisions,
            recovery_answers,
            shared,
            thread: Some(thread),
        })
    }

    /// Passes the user's decision (ignored by the relay unless it answers the open question).
    pub fn decide(&self, decision: Decision) {
        let _ = self.decisions.send(decision);
    }

    /// Answers the recovery question (`Frontend::confirm_recovery`).
    pub fn answer_recovery(&self, yes: bool) {
        let _ = self.recovery_answers.send(yes);
    }

    /// Asks the session to leave (quit, end of the Windows session): a countdown is reverted at
    /// once, a request that journals first is abandoned before its plan, anything else is
    /// waited for (design m3 F.5; `mklm_client::session::cancel_action`).
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::SeqCst);
    }

    /// True once the thread has ended.
    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for SessionWorker {
    fn drop(&mut self) {
        // The thread posts its end itself; never block the UI thread on it.
        drop(self.thread.take());
    }
}

fn lost_report(detail: String) -> RequestReport {
    RequestReport {
        first: RequestEnd::Ended(SessionEnd::Lost {
            exit_code: None,
            detail,
            planned: false,
            countdown: false,
        }),
        lost_needs_recovery: None,
        recovery: None,
        recovery_skipped: None,
    }
}

/// Reports the session's end exactly once — also when the thread unwinds from a panic.
struct EndGuard {
    session: SessionId,
    shared: Arc<SessionShared>,
    reported: bool,
}

impl EndGuard {
    fn report(&mut self, outcome: SessionOutcome) {
        self.reported = true;
        self.shared.mark_done();
        post(AppMsg::SessionEnded {
            session: self.session,
            outcome: Box::new(outcome),
        });
    }
}

impl Drop for EndGuard {
    fn drop(&mut self) {
        if !self.reported {
            self.report(SessionOutcome {
                report: lost_report("the session worker panicked".into()),
                run_once: Err(RunOnceError::Check(
                    "not applied: the session worker panicked".into(),
                )),
            });
        }
        if let Ok(mut current) = CURRENT.lock()
            && current
                .as_ref()
                .is_some_and(|shared| Arc::ptr_eq(shared, &self.shared))
        {
            *current = None;
        }
    }
}

/// The GUI as a relay front end: everything goes to the UI thread; decisions and the recovery
/// answer come back through channels.
struct GuiFrontend {
    session: SessionId,
    decisions: Receiver<Decision>,
    recovery_answers: Receiver<bool>,
    shared: Arc<SessionShared>,
    /// The request may write without a `Planned` event (`!plans_first`).
    writes_without_plan: bool,
}

impl Frontend for GuiFrontend {
    fn event(&mut self, event: &Event, view: &SessionView) -> io::Result<Option<Decision>> {
        if view.planned || matches!(view.prompt, Prompt::Countdown { .. }) {
            self.shared.journaled.store(true, Ordering::SeqCst);
        }
        if !matches!(event, Event::Heartbeat) {
            post(AppMsg::SessionEvent {
                session: self.session,
                event: Box::new(event.clone()),
                view: Box::new(view.clone()),
            });
        }
        Ok(None)
    }

    fn poll(&mut self, _view: &SessionView, _now: Instant) -> io::Result<Option<Decision>> {
        Ok(self.decisions.try_recv().ok())
    }

    fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn cancel_requested(&mut self) -> bool {
        self.shared.cancel.load(Ordering::SeqCst)
    }

    fn notice(&mut self, notice: &Notice) -> io::Result<()> {
        // Requests that write without `Planned` (revert, undo, recover …) count as journaled
        // once a helper serves them; a recovery after a lost helper always does.
        if let Notice::Connected(kind) = notice
            && (self.writes_without_plan || *kind == mklm_client::session::SessionKind::Recovery)
        {
            self.shared.journaled.store(true, Ordering::SeqCst);
        }
        post(AppMsg::SessionNotice {
            session: self.session,
            notice: notice.clone(),
        });
        Ok(())
    }

    /// Asks the user on the UI thread and waits for the answer; a quit meanwhile is "no"
    /// (design m3 A.2.3).
    fn confirm_recovery(&mut self, countdown: bool) -> io::Result<bool> {
        post(AppMsg::RecoveryQuestion {
            session: self.session,
            countdown,
        });
        loop {
            if self.cancel_requested() {
                return Ok(false);
            }
            match self
                .recovery_answers
                .recv_timeout(Duration::from_millis(200))
            {
                Ok(answer) => return Ok(answer),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(false),
            }
        }
    }
}
