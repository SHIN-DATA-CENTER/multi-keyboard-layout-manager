//! One GUI per session (plan 2.2, 3.9; design m3 F.1), on top of `mklm_win::instance`.
//!
//! Start-up (WP-W1 / WP-U6):
//! 1. `acquire_instance()`. `Secondary`: `send_to_instance(Activate, 5 s)` (or `Quit` for
//!    `--quit`) and exit 0. When the pipe's server is not our own instance (another user or
//!    program squatted the name, `Error::Insecure`), or the pipe never answers, log it and run as
//!    the instance instead.
//! 2. `Primary`: create the `InstanceServer` (a squatted pipe name is logged; MKLM runs without
//!    the pipe) and serve it on a thread named `mklm-instance`: `Activate` →
//!    `AppMsg::Activate` (show, focus, `Read`; the page is never changed here), reply `Ok`;
//!    `Quit` → `AppMsg::QuitRequested`, reply `Busy` while a session runs, else `Ok`.
//!
//! `--post-reboot` needs nothing of its own here: the running instance reads the journal and
//! shows the post-reboot check (or the recovery) when the next `SystemRead` says it is due
//! (design m2 C17, m3 B.9). An `activate` never replaces a page the user is on: the post-reboot
//! check that a `--tray` instance put in front, or a change being prepared, stays.

use std::time::Duration;

use mklm_win::instance::{InstanceCommand, InstanceReply};

/// How long a second process tries to reach the running instance (Run and RunOnce start at the
/// same sign-in; the pipe exists a moment after the mutex).
pub const CONNECT_RETRY: Duration = Duration::from_secs(5);

/// What a second process does with its command line.
pub fn command_for(start: crate::args::StartMode) -> InstanceCommand {
    match start {
        crate::args::StartMode::Quit => InstanceCommand::Quit,
        _ => InstanceCommand::Activate,
    }
}

/// The reply the running instance gives to `command` while `session_running`.
pub fn reply_for(command: InstanceCommand, session_running: bool) -> InstanceReply {
    match (command, session_running) {
        (InstanceCommand::Quit, true) => InstanceReply::Busy,
        _ => InstanceReply::Ok,
    }
}
