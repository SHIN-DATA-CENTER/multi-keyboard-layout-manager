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
//!
//! An update session (design m5b E.4) runs on the same kind of worker
//! ([`SessionWorker::start_update`]): the UAC prompt, then the installer handed to the helper;
//! only one session of either kind at a time.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mklm_client::launch::LaunchConfig;
use mklm_client::orchestrator::{RequestEnd, RequestReport};
use mklm_client::run_once::{PostRebootCommand, RunOnceError, run_request};
use mklm_client::session::{Frontend, Notice, Prompt, SessionEnd, SessionView};
use mklm_client::update::check::Offer;
use mklm_client::update::stage::{StageEnd, StageFrontend};
use mklm_core::{ApplyOptions, Decision, Event};
use mklm_ipc::Request;

use crate::app::post;
use crate::state::update::{SessionProgress, UpdateSessionEnd};
use crate::state::{AppMsg, SessionId, SessionOutcome, UpdateMsg};

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
    /// The relay waits for a reconnect and the user's keep or revert: the end of the Windows
    /// session does not wait for it (the change stays waiting, design m3 F.5).
    pub reconnect: AtomicBool,
    /// A helper is being launched (the UAC prompt may be up) and has not connected yet: the end
    /// of the Windows session does not wait for it either — the cancel flag keeps a helper that
    /// starts later from being served, and nothing is written (design m3 F.5 "UAC 待ち").
    pub launching: AtomicBool,
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

/// `WM_ENDSESSION`: wait up to `limit` for the running session to end (design m3 F.5: 3 s during
/// a countdown or a write). A reconnect wait is not waited for (the relay leaves it at once and
/// the change stays waiting for the user), nor is a helper that has not connected yet (the UAC
/// prompt). True when no session is left running.
pub fn wait_for_current_session(limit: Duration) -> bool {
    match current() {
        None => true,
        Some(shared)
            if shared.reconnect.load(Ordering::SeqCst)
                || shared.launching.load(Ordering::SeqCst) =>
        {
            shared.wait_done(Duration::ZERO)
        }
        Some(shared) => shared.wait_done(limit),
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
        let shared = Arc::new(SessionShared {
            launching: AtomicBool::new(true),
            ..SessionShared::default()
        });
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

    /// Starts update session `session` (design m5b E.4): the cached manifest and installer checked
    /// again (D.2 condition 2; no prompt for a file that no longer matches), the UAC prompt
    /// (`launch`), then `mklm_client::update::stage`. Posts `Notice::Connected` once the helper
    /// is connected, the progress, and ends with `UpdateMsg::SessionEnded`. `Cancel` stops it
    /// until the last byte is sent.
    pub fn start_update(
        session: SessionId,
        offer: Offer,
        installer: PathBuf,
        launch: LaunchConfig,
    ) -> io::Result<Self> {
        let (decisions, _) = mpsc::channel();
        let (recovery_answers, _) = mpsc::channel();
        let shared = Arc::new(SessionShared {
            launching: AtomicBool::new(true),
            ..SessionShared::default()
        });
        let worker_shared = shared.clone();
        if let Ok(mut current) = CURRENT.lock() {
            *current = Some(shared.clone());
        }
        let spawned = thread::Builder::new()
            .name("mklm-session".into())
            .spawn(move || {
                let mut guard = UpdateEndGuard {
                    session,
                    shared: worker_shared.clone(),
                    reported: false,
                };
                let end = run_update_session(session, &offer, &installer, &launch, &worker_shared);
                guard.report(end);
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

/// Reports an update session's end exactly once — also when the thread unwinds from a panic.
struct UpdateEndGuard {
    session: SessionId,
    shared: Arc<SessionShared>,
    reported: bool,
}

impl UpdateEndGuard {
    fn report(&mut self, end: UpdateSessionEnd) {
        self.reported = true;
        self.shared.mark_done();
        post(AppMsg::Update(UpdateMsg::SessionEnded {
            session: self.session,
            end: Box::new(end),
        }));
    }
}

impl Drop for UpdateEndGuard {
    fn drop(&mut self) {
        if !self.reported {
            self.report(UpdateSessionEnd::Stage(StageEnd::Lost(
                "the session worker panicked".into(),
            )));
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

/// The update session on its worker (design m5b D.2, D.3, E.4).
fn run_update_session(
    session: SessionId,
    offer: &Offer,
    installer: &std::path::Path,
    launch: &LaunchConfig,
    shared: &Arc<SessionShared>,
) -> UpdateSessionEnd {
    use mklm_client::update::check::{CheckError, CheckOutcome, reverify_cached};
    use mklm_client::update::status::{RunRecordProbe, read_status};

    // D.2 condition 2: the cached manifest verifies again, and the installer is still the file
    // that was checked (the helper checks everything once more; this saves a useless prompt).
    let env = mklm_client::update::env::environment(
        env!("CARGO_PKG_VERSION"),
        mklm_update::url::Endpoints::production(),
    );
    let cache = mklm_client::update::cache::UpdateCache::new(env.cache_dir.clone());
    let machine = read_status(&env.install_dir, &cache).machine_trust;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let offer = match reverify_cached(&env, &cache, &machine, None, now) {
        Ok(CheckOutcome::Available(fresh))
            if fresh.verified.version == offer.verified.version
                && fresh.downloaded.as_deref() == Some(installer) =>
        {
            fresh
        }
        Ok(_) => {
            return UpdateSessionEnd::NotReady(CheckError::Cache(
                "the downloaded update is no longer the one checked".into(),
            ));
        }
        Err(error) => return UpdateSessionEnd::NotReady(error),
    };
    let mut link = match mklm_client::launch::start(launch) {
        Ok(link) => link,
        Err(error) => return UpdateSessionEnd::NotLaunched(error),
    };
    shared.launching.store(false, Ordering::SeqCst);
    // Cancelled (or quitting) while the prompt was up: nothing is asked of the helper.
    if shared.cancel.load(Ordering::SeqCst) {
        link.close();
        return UpdateSessionEnd::Stage(StageEnd::Cancelled);
    }
    post(AppMsg::SessionNotice {
        session,
        notice: Notice::Connected(mklm_client::session::SessionKind::Request),
    });
    let mut frontend = UpdateFrontend {
        session,
        shared: shared.clone(),
    };
    let end = mklm_client::update::stage::stage(
        &mut link,
        &offer,
        installer,
        &mut frontend,
        &mut RunRecordProbe::new(),
    );
    if matches!(end, StageEnd::HandedOff { .. }) {
        // No `Bye`: the helper exits by itself; the pipe closes with the link.
        drop(link);
    } else {
        // `stage` said `Bye`; give the helper a moment to exit.
        link.close();
    }
    UpdateSessionEnd::Stage(end)
}

/// The update session's front end: progress to the UI thread, the cancel flag from it.
struct UpdateFrontend {
    session: SessionId,
    shared: Arc<SessionShared>,
}

impl StageFrontend for UpdateFrontend {
    fn sent(&mut self, bytes: u64, total: u64) {
        post(AppMsg::Update(UpdateMsg::SessionProgress {
            session: self.session,
            progress: SessionProgress::Sent { bytes, total },
        }));
    }

    fn received(&mut self, _bytes: u64) {}

    fn starting_runner(&mut self) {
        post(AppMsg::Update(UpdateMsg::SessionProgress {
            session: self.session,
            progress: SessionProgress::StartingRunner,
        }));
    }

    fn cancel_requested(&mut self) -> bool {
        self.shared.cancel.load(Ordering::SeqCst)
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
        self.shared.reconnect.store(
            matches!(view.prompt, Prompt::Reconnect { .. }),
            Ordering::SeqCst,
        );
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
        match notice {
            Notice::Starting(_) => self.shared.launching.store(true, Ordering::SeqCst),
            Notice::Connected(_) => self.shared.launching.store(false, Ordering::SeqCst),
            Notice::HelperLost { .. } | Notice::RecoveringAfterLoss { .. } => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The end-of-session handlers (design m3 F.5): the flag is set directly, and the wait is
    /// bounded — and skipped for a reconnect wait. (The only test that touches `CURRENT`.)
    #[test]
    fn the_end_of_the_windows_session() {
        assert!(wait_for_current_session(Duration::from_secs(10)));
        assert!(!cancel_current_session());
        let shared = Arc::new(SessionShared::default());
        *CURRENT.lock().unwrap() = Some(shared.clone());
        // A countdown or a write: waited for, up to the limit.
        let started = Instant::now();
        assert!(!wait_for_current_session(Duration::from_millis(100)));
        assert!(started.elapsed() >= Duration::from_millis(90));
        // The cancel flag is set at once; RunOnce is registered only for a journaled operation.
        assert!(!cancel_current_session());
        assert!(shared.cancel.load(Ordering::SeqCst));
        shared.journaled.store(true, Ordering::SeqCst);
        assert!(cancel_current_session());
        // A reconnect wait: not waited for (the change stays waiting).
        shared.reconnect.store(true, Ordering::SeqCst);
        let started = Instant::now();
        assert!(!wait_for_current_session(Duration::from_secs(10)));
        assert!(started.elapsed() < Duration::from_secs(5));
        // The UAC prompt (no helper connected yet): not waited for either.
        shared.reconnect.store(false, Ordering::SeqCst);
        shared.launching.store(true, Ordering::SeqCst);
        let started = Instant::now();
        assert!(!wait_for_current_session(Duration::from_secs(10)));
        assert!(started.elapsed() < Duration::from_secs(5));
        // Done: no wait.
        shared.mark_done();
        assert!(wait_for_current_session(Duration::from_secs(10)));
        shared.launching.store(false, Ordering::SeqCst);
        assert!(wait_for_current_session(Duration::from_secs(10)));
        *CURRENT.lock().unwrap() = None;
    }
}
