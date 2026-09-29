//! One GUI per session (plan 2.2, 3.9; design m3 F.1), on top of `mklm_win::instance`.
//!
//! Start-up ([`claim`]):
//! 1. `acquire_instance()`. `Secondary`: `send_to_instance(command_for(start), 5 s)` — `Activate`,
//!    `Quit` for `--quit`, `Ping` for `--tray` and `--after-update` — and exit 0. When the pipe's
//!    server is not our own instance (another user or
//!    program squatted the name, `Error::Insecure`), or the pipe never answers, log it and run as
//!    the instance instead — except for `--quit`, which never opens a window.
//! 2. `Primary`: a mutex name taken by another account is logged and MKLM runs on. `--quit` with
//!    no running instance has nothing to do and exits 0.
//! 3. The instance serves the pipe ([`InstanceService`]) on a thread named `mklm-instance` (a
//!    squatted pipe name is logged; MKLM runs without the pipe): `Activate` → `AppMsg::Activate`
//!    (show, focus, `Read`; the page is never changed here), reply `Ok`; `Quit` →
//!    `AppMsg::QuitRequested`, reply `Busy` while the quit waits for a running session, else `Ok`;
//!    `QuitIfIdle` (the update runner's only command, design m5b E.4.1) →
//!    `state::quit_if_idle`, which quits only when nothing is going on and otherwise changes
//!    nothing (`Busy`); `Ping` → `Ok` and nothing else (design m5b D.13 step 5). The handler gets
//!    the time the command arrived: one that reaches the UI thread after [`UI_REPLY_WAIT`] is
//!    ignored, since nobody waits for its reply any more.
//!
//! `--post-reboot` needs nothing of its own here: the running instance reads the journal and
//! shows the post-reboot check (or the recovery) when the next `SystemRead` says it is due
//! (design m2 C17, m3 B.9). An `activate` never replaces a page the user is on: the post-reboot
//! check that a `--tray` instance put in front, or a change being prepared, stays.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use mklm_win::instance::{
    InstanceCommand, InstanceGuard, InstanceReply, InstanceRole, InstanceServer, acquire_instance,
    send_to_instance,
};

use crate::args::StartMode;
use crate::log;

/// How long a second process tries to reach the running instance (Run and RunOnce start at the
/// same sign-in; the pipe exists a moment after the mutex).
pub const CONNECT_RETRY: Duration = Duration::from_secs(5);

/// How long the pipe thread waits for a command before it looks at its stop flag again.
const POLL: Duration = Duration::from_millis(500);

/// How long the pipe thread waits for the UI thread to handle a command (the client waits
/// [`CONNECT_RETRY`] for the reply). The pipe is served from the moment the Slint backend exists,
/// before the window, the tray and the watchers are built: a command that arrives then waits in
/// Slint's queue until the event loop runs, so this covers the rest of the start-up too, and
/// stays below the client's wait so that the reply still reaches it.
pub const UI_REPLY_WAIT: Duration = Duration::from_secs(4);

/// Consecutive pipe failures after which the instance stops serving the pipe (logged).
const MAX_PIPE_FAILURES: u32 = 10;

/// What a second process does with its command line. `--tray` (the Run value) and
/// `--after-update` (the update runner, the after-update RunOnce value) only make sure MKLM runs
/// (`ping`): at sign-in the two start together, and the running instance already shows what its
/// own start found (a result, a check, the wizard). An `activate` from the other one would open
/// the window with nothing to show (design m5b D.13 step 5, E.5; MECHANICS-1).
/// `--post-reboot` and a plain start still activate (design m3 F.1, F.2).
pub fn command_for(start: StartMode) -> InstanceCommand {
    match start {
        StartMode::Quit => InstanceCommand::Quit,
        StartMode::Tray | StartMode::AfterUpdate => InstanceCommand::Ping,
        StartMode::Window | StartMode::PostReboot => InstanceCommand::Activate,
    }
}

/// The reply the running instance gives to `command`. `quit_waits`: a helper session is
/// connected, so the quit happens only when it has ended (design m3 F.5); during the UAC prompt
/// MKLM quits at once, which is `Ok`.
pub fn reply_for(command: InstanceCommand, quit_waits: bool) -> InstanceReply {
    match (command, quit_waits) {
        (InstanceCommand::Quit, true) => InstanceReply::Busy,
        _ => InstanceReply::Ok,
    }
}

/// Which role the mutex gave this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Primary,
    Secondary,
    /// The mutex could not be created or opened (logged).
    Unknown,
}

/// The next step of the start-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Run as the instance.
    Run,
    /// Send [`command_for`] to the running instance.
    Send,
    /// Exit with success (true) or failure.
    Exit(bool),
}

/// The step after the mutex.
pub fn after_mutex(role: Role, start: StartMode) -> Step {
    match (role, start) {
        // No MKLM runs: nothing to quit.
        (Role::Primary, StartMode::Quit) => Step::Exit(true),
        (Role::Primary, _) => Step::Run,
        // Unknown: another instance may run; ask it (a quit must reach it, and an activation
        // falls back to running when nothing answers).
        (Role::Secondary | Role::Unknown, _) => Step::Send,
    }
}

/// The step after sending: `sent` is whether the running instance answered.
pub fn after_send(start: StartMode, sent: bool) -> Step {
    match (sent, start) {
        (true, _) => Step::Exit(true),
        // `--quit` never opens a window: the instance could not be reached.
        (false, StartMode::Quit) => Step::Exit(false),
        // Nothing we trust answered: show MKLM rather than nothing (design m3 F.1).
        (false, _) => Step::Run,
    }
}

/// What start-up does next.
#[derive(Debug)]
pub enum Claim {
    /// Run as the instance; keep the guard (if any) for the life of the process.
    Run(Option<InstanceGuard>),
    /// Another instance was told (or there was nothing to do): exit with this code.
    Exit(ExitCode),
}

/// Decides whether this process is the instance (design m3 F.1, F.6 step 3).
pub fn claim(start: StartMode) -> Claim {
    let (role, guard) = match acquire_instance() {
        Ok(InstanceRole::Primary(guard)) => {
            if guard.name_taken() {
                log::warn(
                    "the single-instance mutex name is taken by another account; running without it",
                );
            }
            (Role::Primary, Some(guard))
        }
        Ok(InstanceRole::Secondary) => (Role::Secondary, None),
        Err(error) => {
            log::warn(format!("the single-instance mutex failed: {error}"));
            (Role::Unknown, None)
        }
    };
    let mut step = after_mutex(role, start);
    if step == Step::Exit(true) {
        log::info("--quit: no MKLM is running; nothing to do");
    }
    if step == Step::Send {
        let command = command_for(start);
        let sent = match send_to_instance(command, CONNECT_RETRY) {
            Ok(reply) => {
                log::info(format!("sent {command:?} to the running MKLM: {reply:?}"));
                true
            }
            Err(error) => {
                log::warn(format!(
                    "sending {command:?} to the running MKLM failed: {error}"
                ));
                false
            }
        };
        step = after_send(start, sent);
    }
    match step {
        Step::Exit(true) => Claim::Exit(ExitCode::SUCCESS),
        Step::Exit(false) => Claim::Exit(ExitCode::FAILURE),
        // (`after_send` never answers `Send`; running is the safe side anyway.)
        Step::Run | Step::Send => Claim::Run(guard),
    }
}

/// The running instance's pipe thread (`mklm-instance`, design m3 A.4).
#[derive(Debug)]
pub struct InstanceService {
    stop: Arc<AtomicBool>,
}

impl InstanceService {
    /// Creates the pipe and serves it on its own thread; `handle` runs on the UI thread for each
    /// command, with the time the pipe thread received it, and returns the reply. `None` (logged)
    /// when the pipe or the thread cannot be created: MKLM then runs without it.
    pub fn start(handle: fn(InstanceCommand, Instant) -> InstanceReply) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let spawned = thread::Builder::new()
            .name("mklm-instance".into())
            .spawn(move || match InstanceServer::create() {
                Ok(server) => serve(server, &thread_stop, handle),
                Err(error) => log::warn(format!(
                    "the single-instance pipe could not be created ({error}); a second start cannot reach this MKLM"
                )),
            });
        match spawned {
            Ok(_) => Some(Self { stop }),
            Err(error) => {
                log::warn(format!(
                    "the single-instance thread could not start: {error}"
                ));
                None
            }
        }
    }

    /// Asks the thread to end (within [`POLL`]); never waits for it.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for InstanceService {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(
    mut server: InstanceServer,
    stop: &AtomicBool,
    handle: fn(InstanceCommand, Instant) -> InstanceReply,
) {
    let mut failures = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let command = match server.next_command_timeout(POLL) {
            Ok(Some(command)) => command,
            Ok(None) => continue,
            Err(error) => {
                failures += 1;
                log::warn(format!("the single-instance pipe failed: {error}"));
                if failures >= MAX_PIPE_FAILURES {
                    log::error("the single-instance pipe is no longer served");
                    return;
                }
                thread::sleep(POLL);
                continue;
            }
        };
        failures = 0;
        log::info(format!("received {command:?} from a second MKLM"));
        let received = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let posted = slint::invoke_from_event_loop(move || {
            let _ = sender.send(handle(command, received));
        });
        // No answer when the UI is gone (MKLM is quitting) or stuck: the client then runs by
        // itself instead of trusting a reply for a window that will not come.
        if posted.is_err() {
            return;
        }
        match receiver.recv_timeout(UI_REPLY_WAIT) {
            Ok(reply) => {
                if let Err(error) = server.reply(reply) {
                    log::warn(format!("replying to a second MKLM failed: {error}"));
                }
            }
            Err(_) => log::warn("the window did not handle a second MKLM's command in time"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_replies() {
        assert_eq!(command_for(StartMode::Quit), InstanceCommand::Quit);
        // The UI thread ignores a `quit-if-idle` older than this (design m5b E.4.1).
        assert_eq!(UI_REPLY_WAIT, crate::state::update::QUIT_IF_IDLE_WAIT);
        for start in [StartMode::Window, StartMode::PostReboot] {
            assert_eq!(command_for(start), InstanceCommand::Activate);
        }
        // The Run value's `--tray` and the after-update value start together at sign-in: the
        // second one only makes sure MKLM runs, and opens no window (design m5b D.13 step 5;
        // MECHANICS-1).
        for start in [StartMode::Tray, StartMode::AfterUpdate] {
            assert_eq!(command_for(start), InstanceCommand::Ping);
        }
        assert_eq!(
            reply_for(InstanceCommand::Activate, true),
            InstanceReply::Ok
        );
        assert_eq!(reply_for(InstanceCommand::Ping, true), InstanceReply::Ok);
        assert_eq!(reply_for(InstanceCommand::Ping, false), InstanceReply::Ok);
        assert_eq!(reply_for(InstanceCommand::Quit, false), InstanceReply::Ok);
        assert_eq!(reply_for(InstanceCommand::Quit, true), InstanceReply::Busy);
    }

    #[test]
    fn start_up_steps() {
        // The first process runs; a second one hands over and exits.
        assert_eq!(after_mutex(Role::Primary, StartMode::Tray), Step::Run);
        assert_eq!(
            after_mutex(Role::Secondary, StartMode::PostReboot),
            Step::Send
        );
        assert_eq!(after_mutex(Role::Unknown, StartMode::Window), Step::Send);
        assert_eq!(after_send(StartMode::PostReboot, true), Step::Exit(true));
        // Nobody we trust answered: run rather than show nothing (design m3 F.1).
        assert_eq!(after_send(StartMode::Window, false), Step::Run);
        assert_eq!(after_send(StartMode::Tray, false), Step::Run);
        // `--quit` never becomes the instance.
        assert_eq!(
            after_mutex(Role::Primary, StartMode::Quit),
            Step::Exit(true)
        );
        assert_eq!(after_mutex(Role::Secondary, StartMode::Quit), Step::Send);
        assert_eq!(after_send(StartMode::Quit, true), Step::Exit(true));
        assert_eq!(after_send(StartMode::Quit, false), Step::Exit(false));
    }
}
